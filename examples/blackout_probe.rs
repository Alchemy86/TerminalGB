//! Lose a wild battle with a party of one, and see whether the whiteout
//! completes.
//!
//! Gen 1 does not end a run when your last Pokémon faints. `HandleBlackOut`
//! (`home/overworld.asm`) fades the screen out, stops the music, halves your
//! money, and warps you to `wLastBlackoutMap` — the last Pokémon Centre you
//! healed at, which before Viridian is Pallet Town. The screen comes back and
//! you are standing outside your own front door.
//!
//! This drives that sequence with **no agent, no policy and no plugin in the
//! observation loop**, and reports what the machine actually did. It runs the
//! identical cartridge twice from one byte-identical save state, changing
//! exactly one thing between the two runs: whether the machine has a sound
//! chip.
//!
//! ```text
//! cargo run --release --features image,plugin-battle-control,plugin-trainer \
//!     --example blackout_probe -- --rom "$HOME/roms/blue.gb"
//! ```
//!
//! # Why the sound chip is the variable
//!
//! `HandleBlackOut`'s second instruction is `call StopMusic`, and `StopMusic`
//! ends in a spin:
//!
//! ```text
//! StopMusic::
//!     ld [wAudioFadeOutControl], a   ; a = $08
//!     ...
//! .wait
//!     ld a, [wAudioFadeOutControl]
//!     and a
//!     jr nz, .wait                   ; until the audio engine clears it
//! ```
//!
//! The audio engine clears it in `FadeOutAudio` (`home/fade_audio.asm`), which
//! walks the master volume down **by reading `rAUDVOL` ($FF24) back,
//! decrementing both nibbles and writing it again**, finishing only when the
//! read-back is exactly zero. A machine with no APU answers `$FF` to every read
//! in `$FF10-$FF3F` and drops every write, so the volume never falls, the flag
//! is never cleared, and `StopMusic` spins for ever behind an already
//! faded-out screen.
//!
//! That is not a hypothesis this file asserts — it is what the two runs below
//! measure, and both `wAudioFadeOutControl` and the `$FF24` read-back are in
//! the per-arm report.
//!
//! # It starts a NEW GAME, and that is the safety story
//!
//! The cartridge is copied into `target/` first, because the MBC writes its
//! `.sav` next to the ROM path. Nothing reads or writes a real save. Every run
//! boots the copy into a brand-new game, so the player has **no Pokémon** and
//! `wLastBlackoutMap` is the game's own starting value — which is exactly the
//! state the reported failure happens in.
//!
//! # See also
//!
//! - `testharness/blackout.rs` — the gate, sharing this file's implementation.
//! - `tools/blackout/README.md` — the same question asked of mGBA.
//! - `mmu::audio_presence_tests` — the mechanism, with no cartridge at all.

#[path = "support/blackout.rs"]
mod blackout;

use blackout::Outcome;
use std::path::PathBuf;

const STAGE_DIR: &str = "target/blackout-probe";

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            println!("error: {e}");
            println!(
                "help: cargo run --release --example blackout_probe -- --rom <blue.gb>"
            );
            std::process::exit(2);
        }
    };

    let rom = match std::fs::read(&args.rom) {
        Ok(r) => r,
        Err(e) => {
            println!("error: cannot read {}: {e}", args.rom.display());
            println!("help: pass --rom <path to a Pokémon Red or Blue cartridge>");
            std::process::exit(2);
        }
    };
    let staged = match blackout::stage(&args.rom, &rom, STAGE_DIR) {
        Ok(p) => p,
        Err(e) => {
            println!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("blackout probe");
    println!("  rom      {}", args.rom.display());
    println!("  staged   {}", staged.display());
    println!(
        "  observe  {} frames of no input after the faint",
        args.frames
    );
    println!();

    // One preparation, one state, two arms. Sharing the state is what makes the
    // comparison a comparison: the two runs start from a byte-identical machine
    // and differ in exactly one thing.
    let setup_dir = PathBuf::from(STAGE_DIR);
    let state = match blackout::prepare(&rom, &staged, Some(&setup_dir)) {
        Ok(s) => s,
        Err(e) => {
            println!("error: {e}");
            std::process::exit(1);
        }
    };
    let state_path = setup_dir.join("blacked-out.gbss");
    if let Err(e) = std::fs::write(&state_path, &state) {
        println!("error: cannot write {}: {e}", state_path.display());
        std::process::exit(1);
    }
    println!(
        "  prepared {} ({} bytes) — the party has just fainted",
        state_path.display(),
        state.len()
    );
    println!("  setup    {STAGE_DIR}/{{party.bin,setup.txt}} — for a foreign emulator");
    println!();

    let arms: Vec<bool> = match args.apu {
        Some(on) => vec![on],
        None => vec![false, true],
    };

    let mut results = Vec::new();
    for apu in arms {
        match blackout::observe(&rom, &staged, &state, apu, args.frames) {
            Ok((o, gb)) => {
                report(&o, apu);
                if let Some(dir) = &args.shots {
                    if let Err(e) =
                        save_shot(&gb, dir, if apu { "with-apu" } else { "no-apu" })
                    {
                        println!("  (no screenshot: {e})");
                    }
                }
                results.push((apu, o));
            }
            Err(e) => {
                println!("[{}] error: {e}", arm_name(apu));
                std::process::exit(1);
            }
        }
    }

    println!("verdict");
    for (apu, r) in &results {
        println!(
            "  {:<26} {}",
            arm_name(*apu),
            if r.recovered {
                "the whiteout COMPLETED — the player woke up"
            } else {
                "the whiteout NEVER COMPLETED — still black, still in the blackout"
            }
        );
    }
    if results.len() == 2 {
        let (_, no_apu) = &results[0];
        let (_, apu) = &results[1];
        println!();
        if !no_apu.recovered && apu.recovered {
            println!(
                "  The only difference between those two runs is one line: whether the"
            );
            println!("  machine was given a sound chip. Without one the cartridge's own");
            println!(
                "  blackout sequence cannot finish, because `FadeOutAudio` waits on a"
            );
            println!(
                "  master-volume read-back that a machine with no APU never lowers."
            );
            std::process::exit(0);
        }
        if no_apu.recovered && apu.recovered {
            println!("  Both arms recovered — the sound chip is not the variable here.");
            std::process::exit(0);
        }
        println!("  Neither arm recovered, or only the silent one did: this is NOT the");
        println!("  mechanism this probe was written to test. Read the per-arm report.");
        std::process::exit(1);
    }
}

fn arm_name(apu: bool) -> &'static str {
    if apu {
        "with a sound chip"
    } else {
        "with no sound chip"
    }
}

fn report(o: &Outcome, apu: bool) {
    println!("[{}]", arm_name(apu));
    println!(
        "  at the faint            wCurMap {} ($D35E), wLastBlackoutMap {} ($D719)",
        o.map_at_faint, o.last_blackout_map
    );
    println!(
        "  after {:>5} frames      wCurMap {}, wIsInBattle ${:02X} ($D057)",
        o.observed, o.map_at_end, o.is_in_battle_at_end
    );
    println!(
        "  screen                  {} distinct shade(s){}",
        o.shades_at_end,
        match o.screen_back_at {
            Some(f) => format!(", came back at frame {f}"),
            None => ", never came back".to_string(),
        }
    );
    println!(
        "  lead                    {}/{} HP at the faint, {}/{} now{}",
        o.lead_hp_at_faint,
        o.lead_max_at_faint,
        o.lead_hp_at_end,
        o.lead_max_hp_at_end,
        if o.lead_hp_at_end > 0 && o.lead_hp_at_end == o.lead_max_hp_at_end {
            "  (healed — HandleBlackOut's ResetStatusAndHalveMoneyOnBlackout ran)"
        } else {
            ""
        }
    );
    println!(
        "  wAudioFadeOutControl    ${:02X} ($CFC7){}",
        o.fade_control_at_end,
        if o.fade_control_at_end == 0 {
            "  (cleared — the fade finished)"
        } else {
            "  (NON-ZERO — StopMusic is still spinning)"
        }
    );
    println!(
        "  wAudioFadeOutCounter    ${:02X} ($CFC9)",
        o.fade_counter_at_end
    );
    println!(
        "  rAUDVOL read-back       ${:02X} ($FF24){}",
        o.audvol_at_end,
        if o.audvol_at_end == 0xFF {
            "  (no APU answers $FF, so the fade can never reach 0)"
        } else {
            ""
        }
    );
    println!();
}

/// Through the `image` crate, as the frontend's screenshot keys do — not
/// through `termimage`'s writer, which is *stored* deflate: right for a 60 Hz
/// wire and wrong for a file somebody keeps.
fn save_shot(
    gb: &terminalgb::gameboy::Gameboy,
    dir: &std::path::Path,
    label: &str,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    let path = dir.join(format!("{label}.png"));
    let img = image::RgbaImage::from_raw(160, 144, gb.image().to_vec())
        .ok_or("the framebuffer is not 160x144x4")?;
    img.save(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

struct Args {
    rom: PathBuf,
    frames: u32,
    apu: Option<bool>,
    shots: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Args, String> {
        let mut rom = None;
        let mut frames = 3000u32;
        let mut apu = None;
        let mut shots = None;
        let mut it = std::env::args().skip(1);
        while let Some(a) = it.next() {
            match a.as_str() {
                "--rom" => rom = Some(PathBuf::from(need(&mut it, "--rom")?)),
                "--frames" => {
                    frames = need(&mut it, "--frames")?
                        .parse()
                        .map_err(|_| "--frames wants a number".to_string())?
                }
                "--apu" => {
                    apu = Some(match need(&mut it, "--apu")?.as_str() {
                        "on" => true,
                        "off" => false,
                        other => {
                            return Err(format!("--apu wants on or off, not {other:?}"))
                        }
                    })
                }
                "--shots" => shots = Some(PathBuf::from(need(&mut it, "--shots")?)),
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        Ok(Args {
            rom: rom.ok_or("--rom is required")?,
            frames,
            apu,
            shots,
        })
    }
}

fn need(it: &mut impl Iterator<Item = String>, flag: &str) -> Result<String, String> {
    it.next().ok_or_else(|| format!("{flag} wants a value"))
}

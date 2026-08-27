//! Gen 1's whiteout must complete, and every host that runs a real cartridge
//! must give it a machine that can complete it.
//!
//! Losing your last Pokémon in Gen 1 is not a dead end. `HandleBlackOut`
//! (`home/overworld.asm`) fades the screen out, stops the music, halves your
//! money and warps you to `wLastBlackoutMap` — before your first Pokémon Centre
//! that is Pallet Town, so you wake up outside your own front door.
//!
//! It did not complete here, and the cause was not the emulator: `StopMusic`
//! spins until the audio engine clears `wAudioFadeOutControl`, and
//! `FadeOutAudio` (`home/fade_audio.asm`) clears it only after walking the
//! master volume down to zero — which it does by **reading `rAUDVOL` back**,
//! decrementing both nibbles and writing it again. A machine with no APU
//! answers `$FF` to every read in `$FF10-$FF3F` and drops every write, so the
//! volume never falls, the flag is never cleared, and the screen stays black
//! for ever. `terminalgb-mcp` shipped such a machine by default, and so did
//! every example here that drove a real cartridge.
//!
//! Three checks:
//!
//! 1. **The audit.** Every host in this repository that builds a machine to run
//!    a real cartridge installs a sound chip. Needs no cartridge, so it always
//!    runs — and it is the only one of the three that catches a *new* harness
//!    falling into the same trap, which has now happened four times.
//! 2. **The whiteout completes.** Driven on the real cartridge through the
//!    game's own out-of-battle blackout path, which needs no party and no
//!    plugin: `wOutOfBattleBlackout` is written only by `engine/events/poison.asm`
//!    and read only by `OverworldLoop`, which jumps straight to
//!    `HandleBlackOut`.
//! 3. **The teeth.** The same drive on a machine with no sound chip must NOT
//!    complete. Without this the second check would pass on an emulator that
//!    had quietly stopped emulating anything at all.
//!
//! Checks 2 and 3 need the player's own Pokémon Red/Blue and skip with exit 0
//! without one, like `testharness/colosseum.rs`. The mechanism underneath is
//! pinned with no cartridge at all in `mmu::audio_presence_tests`, and the
//! whole thing is reproduced end to end, with a battle, by
//! `examples/blackout_probe.rs`.

// The reproduction itself is shared with `examples/blackout_probe.rs`, the same
// way `testharness/paperclaims.rs` shares `examples/support/paperclaims.rs` —
// so the thing that is measured and the thing that is gated cannot drift apart.
#[path = "../examples/support/blackout.rs"]
mod blackout;

use blackout::PALLET_TOWN;
use std::path::{Path, PathBuf};

const STAGE_DIR: &str = "target/blackout-harness";

fn main() {
    let mut failed = 0;

    // [1] runs everywhere, including CI, and needs nothing.
    match audit_hosts() {
        Ok(n) => println!("[1] audit: {n} cartridge-driving host(s), every one installs a sound chip  PASS"),
        Err(e) => {
            println!("[1] audit  FAIL\n{e}");
            failed += 1;
        }
    }

    let Some(rom_path) = find_rom() else {
        println!(
            "\nblackout: no Pokémon Red/Blue cartridge found — set POKE_ROM=<path> \
             or drop one in ~/roms. Skipping checks 2 and 3 (exit 0)."
        );
        std::process::exit(if failed > 0 { 1 } else { 0 });
    };
    println!("\nblackout: cartridge {}", rom_path.display());
    let rom = std::fs::read(&rom_path).expect("cannot read the cartridge");
    let staged =
        blackout::stage(&rom_path, &rom, STAGE_DIR).expect("cannot stage the cartridge");

    // One preparation, two arms, one byte-identical starting machine — so the
    // only difference between checks 2 and 3 is the sound chip.
    let state = match blackout::prepare(&rom, &staged, None) {
        Ok(s) => s,
        Err(e) => {
            println!("[2/3] FAIL — could not reach the faint: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "blackout: prepared a machine one frame past the faint ({} bytes), \
         wLastBlackoutMap {PALLET_TOWN}",
        state.len()
    );

    match blackout::observe(&rom, &staged, &state, true, 3000) {
        Ok((o, _)) => {
            println!("[2] with a sound chip: {}", describe(&o));
            if o.recovered && o.map_at_end == PALLET_TOWN {
                println!(
                    "[2] the whiteout completes and the player wakes in Pallet Town, \
                     healed to {}/{}  PASS",
                    o.lead_hp_at_end, o.lead_max_hp_at_end
                );
            } else {
                println!(
                    "[2] FAIL — expected the whiteout to complete with wCurMap {PALLET_TOWN}"
                );
                failed += 1;
            }
        }
        Err(e) => {
            println!("[2] FAIL — {e}");
            failed += 1;
        }
    }

    match blackout::observe(&rom, &staged, &state, false, 3000) {
        Ok((o, _)) => {
            println!("[3] with no sound chip: {}", describe(&o));
            if !o.recovered {
                println!(
                    "[3] teeth: without a sound chip it does not complete, so check 2 \
                     is measuring something  PASS"
                );
            } else {
                println!(
                    "[3] FAIL — it completed without a sound chip, so check 2 proves \
                     nothing. Has the APU stopped being optional, or has the game \
                     stopped reading rAUDVOL?"
                );
                failed += 1;
            }
        }
        Err(e) => {
            println!("[3] FAIL — {e}");
            failed += 1;
        }
    }

    if failed > 0 {
        println!("\n{failed} check(s) failed");
        std::process::exit(1);
    }
    println!("\nall checks passed");
}

// ---------------------------------------------------------------------------
// [1] the audit
// ---------------------------------------------------------------------------

/// Every file here builds a machine to run a real, commercial cartridge, so
/// every one must give it a sound chip.
///
/// A text scan rather than a type: making every host import a shared
/// constructor is a large mechanical change across crates other people are
/// editing, and the drift that matters is "somebody built a machine and did not
/// think about the chip". Same shape, and the same reasoning, as
/// `testharness/gen1atlas.rs`'s anti-drift gate over `PLUGIN_SOURCES`.
///
/// **Adding a host that drives a real cartridge means adding it here.** Files
/// that drive only test ROMs are deliberately absent: `headless.rs` (the
/// minimal embedding proof), `mealybug.rs` and `cpp_suite.rs` (gated
/// conformance runners), and the rendering benches.
const CARTRIDGE_HOSTS: &[&str] = &[
    "src/mcp/session.rs",
    "examples/support/blackout.rs",
    "examples/scenario.rs",
    "examples/longplay.rs",
    "examples/claude_plays.rs",
    "examples/shiny_hunt.rs",
    "examples/rom_script.rs",
    "examples/gen1_battle.rs",
    "examples/gen1_battle_sim.rs",
    "examples/gen1_catch_drill.rs",
    "examples/gen1_learn_drill.rs",
    "examples/gen1_cancel_exit.rs",
    "examples/gen1_paper_claims.rs",
    "examples/gen1_trade.rs",
    "examples/gen2_timecapsule.rs",
    "examples/worldlobby.rs",
    "examples/link_two.rs",
    "examples/lobby_drift.rs",
    "examples/bgb_link.rs",
    "examples/trainer_layout.rs",
    "examples/warproom_probe.rs",
    "examples/screen_text.rs",
    "examples/offer_list.rs",
    "examples/portal_trade.rs",
    "examples/pewter_chain.rs",
    "examples/support/paperclaims.rs",
];

fn audit_hosts() -> Result<usize, String> {
    // The harness runs from the crate root.
    let mut missing = Vec::new();
    let mut absent = Vec::new();
    for f in CARTRIDGE_HOSTS {
        match std::fs::read_to_string(f) {
            Ok(s) => {
                // A call, not a mention: `core_bench.rs` explains the trap in
                // a comment and would otherwise satisfy a bare `contains`.
                if !s.contains(".enable_audio(") {
                    missing.push(*f);
                }
            }
            Err(_) => absent.push(*f),
        }
    }
    if !absent.is_empty() {
        return Err(format!(
            "    these files are named in CARTRIDGE_HOSTS and are not there:\n{}\n    \
             Renamed or deleted? Update the list.",
            absent
                .iter()
                .map(|f| format!("      {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    if !missing.is_empty() {
        return Err(format!(
            "    these hosts build a machine for a real cartridge and never call \
             `enable_audio`:\n{}\n    A machine with no APU is not a quiet Game Boy: \
             Gen 1's whiteout and its Pokémon Centre nurse both wait on the sound \
             chip and never finish without one. Install \
             `terminalgb::sound::SilentPlayer`.",
            missing
                .iter()
                .map(|f| format!("      {f}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(CARTRIDGE_HOSTS.len())
}

// ---------------------------------------------------------------------------
// [2] and [3] the whiteout itself
// ---------------------------------------------------------------------------

fn describe(o: &blackout::Outcome) -> String {
    format!(
        "after {} frames wCurMap {}, wIsInBattle ${:02X}, {} shade(s) on screen, \
         wAudioFadeOutControl ${:02X}, rAUDVOL ${:02X}",
        o.observed,
        o.map_at_end,
        o.is_in_battle_at_end,
        o.shades_at_end,
        o.fade_control_at_end,
        o.audvol_at_end
    )
}

// ---------------------------------------------------------------------------
// the cartridge
// ---------------------------------------------------------------------------

fn looks_like_red_or_blue(p: &Path) -> bool {
    let Ok(d) = std::fs::read(p) else {
        return false;
    };
    if d.len() < 0x150 {
        return false;
    }
    let title: String = d[0x134..0x143]
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| *b as char)
        .collect();
    title.starts_with("POKEMON RED") || title.starts_with("POKEMON BLUE")
}

fn find_rom() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_ROM") {
        let p = PathBuf::from(p);
        return looks_like_red_or_blue(&p).then_some(p);
    }
    let mut dirs = vec![PathBuf::from("roms")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("roms"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "gb").unwrap_or(false)
                && looks_like_red_or_blue(&p)
            {
                return Some(p);
            }
        }
    }
    None
}

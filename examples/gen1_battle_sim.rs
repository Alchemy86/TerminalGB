//! **The cartridge oracle.** Run real battles on the real cartridge and hold
//! the model in `terminalgb-gen1-battle` to what the game actually did.
//!
//! ```text
//! cargo run --release --features gen1-battle,plugin-battle-control \
//!   --example gen1_battle_sim -- --rom ~/roms/blue.gb
//! ```
//!
//! # What this is for
//!
//! A battle engine is a hundred small arithmetic claims, and every one of them
//! can be plausibly wrong. This is the thing that makes them falsifiable: it
//! starts battles on the player's own cartridge, samples WRAM every frame,
//! rebuilds the battle state the model's way, and compares. **Where they
//! disagree the model is wrong.** The exit status is non-zero on any
//! disagreement, so it can gate a change.
//!
//! # What it checks, and how strongly
//!
//! * **Exactly** — stat recomputation from stat stages, badge boosts, the burn
//!   and paralysis penalties and the order they compose in; poison and burn
//!   residual damage. No dice anywhere near these.
//! * **Within a band** — damage. `RandomizeDamage` multiplies by a byte in
//!   `217..=255`, so an observed hit pins the model's pre-random figure to a
//!   15 % window; inverting it turns each hit into a check, and a systematically
//!   biased model is caught by the *mean* implied roll even when every
//!   individual hit passes.
//!
//! The split is [`terminalgb::gen1battle`]'s and is explained there. Nothing is
//! counted as agreement that the harness could not actually determine.
//!
//! # Two things this deliberately does not do
//!
//! It does not touch the player's ROM directory: the cartridge is copied into
//! `target/` first, because the MBC writes a `.sav` next to the ROM it is given
//! and this harness presses buttons at menus. And it never writes a baseline of
//! its own output — the expectation here is the *cartridge's* behaviour, which
//! is the same rule `testharness/baseline.txt` follows.

use std::path::{Path, PathBuf};
use std::time::Instant;

use terminalgb::gameboy::Gameboy;
use terminalgb::gen1battle::{check, diff, Frame, Tally};
use terminalgb_gen1_battle::data::GameData;
use terminalgb_plugin_api::KeypadKey;
use terminalgb_plugin_battle_control::{preset::PresetFile, wram, BattlePlugin};

struct Args {
    rom: Option<PathBuf>,
    presets: Vec<String>,
    frames: u32,
    state: Option<PathBuf>,
    verbose: bool,
}

fn parse() -> Args {
    let mut a = Args {
        rom: None,
        presets: Vec::new(),
        frames: 5000,
        state: None,
        verbose: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rom" => a.rom = it.next().map(PathBuf::from),
            "--preset" => {
                if let Some(v) = it.next() {
                    a.presets.push(v);
                }
            }
            "--frames" => {
                a.frames = it.next().and_then(|v| v.parse().ok()).unwrap_or(5000)
            }
            "--state" => a.state = it.next().map(PathBuf::from),
            "--verbose" | "-v" => a.verbose = true,
            "--help" | "-h" => {
                eprintln!(
                    "gen1_battle_sim — hold the battle model to the real cartridge\n\n\
                     --rom <path>      the cartridge (default: a Pokémon in ~/roms)\n\
                     --preset <name>   a battle-control preset; repeatable, default: all\n\
                     --frames <n>      frames per battle (default 5000)\n\
                     --state <path>    a save state to start from, instead of a new game\n\
                     --verbose         print every observation\n"
                );
                std::process::exit(0);
            }
            other => eprintln!("ignoring unknown argument `{other}`"),
        }
    }
    a
}

fn find_rom() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home).join("roms");
    let mut best: Option<PathBuf> = None;
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.ends_with(".gb") && (name.contains("blue") || name.contains("red")) {
            best = Some(p);
            break;
        }
    }
    best
}

/// Copy the cartridge into `target/` before playing it.
///
/// **Never point this at `~/roms`.** The MBC writes a `.sav` beside whatever
/// path it is handed, and this harness presses buttons at menus — a mis-step
/// would write over a real save. The repository has destroyed a Pokémon Blue
/// battery file exactly once and does not intend to do it twice.
fn stage(rom: &Path) -> Result<PathBuf, String> {
    let dir = PathBuf::from("target/gen1-battle-sim");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = dir.join(rom.file_name().ok_or("the ROM path has no filename")?);
    std::fs::copy(rom, &dest).map_err(|e| format!("cannot stage the ROM: {e}"))?;
    // A stale save from an earlier run would start the game somewhere else.
    let _ = std::fs::remove_file(dest.with_extension("sav"));
    Ok(dest)
}

fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    for _ in 0..frames {
        gb.frame();
    }
    gb.keyup(key);
}

fn run_frames(gb: &mut Gameboy, n: u32) {
    for _ in 0..n {
        gb.frame();
    }
}

fn menu_is_open(gb: &mut Gameboy) -> bool {
    let tiles: Vec<u8> = (0..20 * 18)
        .map(|i| gb.peek(wram::addr::TILE_MAP + i as u16))
        .collect();
    terminalgb_plugin_trainer::text::decode_screen(&tiles).contains("OPTION")
}

/// Mash through the opening to the overworld.
///
/// The same route `examples/gen1_battle.rs` takes, and for the same reasons:
/// alternating START and A plays the intro on its own, and the menu is closed
/// with **B**, never A — both dismiss a text box, but B can never *open* one,
/// and an A one frame late opens the START menu, which then swallows every
/// direction key for the rest of the run.
fn new_game(gb: &mut Gameboy) -> Result<(), String> {
    let mut running = false;
    for step in 0..500u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        hold(gb, key, 4);
        run_frames(gb, 20);
        if gb.peek(0xDA43) > 0 || gb.peek(0xDA44) > 4 {
            running = true;
            break;
        }
    }
    if !running {
        return Err("never reached the overworld from the title screen".into());
    }
    for _ in 0..8 {
        if !menu_is_open(gb) {
            break;
        }
        hold(gb, KeypadKey::B, 4);
        run_frames(gb, 20);
    }
    run_frames(gb, 60);
    if menu_is_open(gb) {
        return Err("could not close the START menu".into());
    }
    Ok(())
}

/// Play one preset and check every observation it produces.
fn one_battle(
    staged: &Path,
    rom: &[u8],
    data: &GameData,
    presets: &PresetFile,
    name: &str,
    args: &Args,
    tally: &mut Tally,
) -> Result<u32, String> {
    let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
    let plugin = BattlePlugin::new(rom.to_vec(), presets.clone())
        .map_err(|e| format!("plugin: {e}"))?;
    gb.add_plugin(Box::new(plugin))
        .map_err(|e| format!("plugin: {e}"))?;

    match &args.state {
        Some(p) => {
            let bytes = std::fs::read(p)
                .map_err(|e| format!("cannot read {}: {e}", p.display()))?;
            gb.load_state(&bytes)
                .map_err(|e| format!("save state: {e:?}"))?;
        }
        None => new_game(&mut gb)?,
    }

    gb.plugin_mut::<BattlePlugin>()
        .ok_or("the plugin vanished")?
        .arm(name)
        .map_err(|e| e.to_string())?;

    // **Do not observe while the plugin is still writing.**
    // `BattlePlugin` re-applies its opposition override until it reads back
    // intact, for up to `PATCH_WINDOW_FRAMES` (40) after the battle starts —
    // so a frame taken inside that window can hold a half-written struct. The
    // first run of this harness reported four "disagreements" in stat
    // recomputation that were entirely this: the model was right and the
    // harness was reading the plugin's own edits.
    //
    // 90 frames is the 40 the plugin needs plus the send-out animation, which
    // is when the engine writes `wBattleMon` from the party.
    const SETTLE_FRAMES: u32 = 90;

    let mut previous: Option<Frame> = None;
    let mut observations = 0u32;
    let mut saw_battle = false;
    let mut battle_began_at: Option<u32> = None;
    for f in 0..args.frames {
        // **The battle has to be played.** A Gen 1 battle sits at its own menu
        // until a button arrives, so a harness that only watches observes a
        // still picture — the first version of this file ran 3000 frames and
        // reported zero observations while the plugin cheerfully said "battle
        // started".
        //
        // A press every `MENU_GAP` frames advances text and takes FIGHT then the
        // first move. Gen 1's `HandleMenuInput` delays several frames after
        // accepting a button and does not see one that arrives inside that
        // delay, so a faster mash would look exactly like dropped input.
        // `plugins/agent` records the same constant for the same reason.
        const MENU_GAP: u32 = 24;
        if f % MENU_GAP == 0 {
            gb.keydown(KeypadKey::A);
        } else if f % MENU_GAP == 4 {
            gb.keyup(KeypadKey::A);
        }
        gb.frame();
        let now = Frame::read(&mut gb, data);
        if now.is_some() && battle_began_at.is_none() {
            battle_began_at = Some(f);
        }
        if now.is_none() {
            battle_began_at = None;
        }
        let settled = battle_began_at
            .map(|s| f >= s + SETTLE_FRAMES)
            .unwrap_or(false);
        match (&previous, &now) {
            (Some(before), Some(after)) if settled => {
                for obs in diff(before, after) {
                    let v = check(data, &obs);
                    if args.verbose {
                        println!("    {obs:?} -> {v:?}");
                    }
                    tally.record(v);
                    observations += 1;
                }
            }
            (None, Some(_)) => saw_battle = true,
            _ => {}
        }
        previous = now;
    }
    if !saw_battle {
        // Say *why*, not just that. A harness that reports "no battle" without
        // the plugin's own phase sends the next person hunting in the wrong
        // half of the system.
        let phase = gb
            .plugin::<BattlePlugin>()
            .map(|p| p.phase().label())
            .unwrap_or_else(|| "no plugin".into());
        let log = gb
            .plugin::<BattlePlugin>()
            .map(|p| p.log().join("; "))
            .unwrap_or_default();
        return Err(format!(
            "preset `{name}` never produced a battle to observe              (plugin phase: {phase}; wIsInBattle=${:02X}; log: {log})",
            gb.peek(wram::addr::IS_IN_BATTLE)
        ));
    }
    Ok(observations)
}

fn main() {
    let args = parse();
    let rom_path = match args.rom.clone().or_else(find_rom) {
        Some(p) => p,
        None => {
            eprintln!(
                "no cartridge. Pass --rom <path>, or put a Pokémon Red/Blue image in ~/roms.\n\
                 This repository ships no game data and never will; the cartridge is yours."
            );
            std::process::exit(2);
        }
    };
    let rom = match std::fs::read(&rom_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read {}: {e}", rom_path.display());
            std::process::exit(2);
        }
    };

    // Decode the cartridge's own tables. A refusal here means the model would
    // have been built on guesses, which is worse than not running.
    let data = match GameData::load_from_rom(&rom) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("this cartridge cannot be read as Generation 1 battle data: {e}");
            std::process::exit(2);
        }
    };
    println!(
        "cartridge   {} ({} moves, {} type-chart rows)",
        rom_path.display(),
        data.moves().len(),
        data.chart().entries().len()
    );

    let staged = match stage(&rom_path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    // Load the harness's own presets — battles chosen to *produce observations*
    // rather than to be watchable. `examples/gen1_battle_sim.preset` explains
    // what each one is for and why the built-in list is the wrong corpus here.
    let own = Path::new("examples/gen1_battle_sim.preset");
    let presets = if own.exists() {
        match PresetFile::load(own) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("cannot read {}: {e}", own.display());
                std::process::exit(2);
            }
        }
    } else {
        eprintln!(
            "note: {} not found (run from the repository root), falling back to the
             built-in presets — those are cinematics and produce very few observations.",
            own.display()
        );
        PresetFile::builtin()
    };
    let wanted: Vec<String> = if args.presets.is_empty() {
        presets.names()
    } else {
        args.presets.clone()
    };

    let started = Instant::now();
    let mut tally = Tally::default();
    let mut ran = 0usize;
    for name in &wanted {
        print!("battle      {name} … ");
        use std::io::Write;
        let _ = std::io::stdout().flush();
        match one_battle(&staged, &rom, &data, &presets, name, &args, &mut tally) {
            Ok(n) => {
                println!("{n} observation(s)");
                ran += 1;
            }
            // A preset that will not start is a harness problem, not a model
            // one, and must not be counted as agreement.
            Err(e) => println!("SKIPPED: {e}"),
        }
    }

    println!(
        "\n{} battle(s) in {:.1}s\n",
        ran,
        started.elapsed().as_secs_f32()
    );
    print!("{}", tally.to_text());

    if ran == 0 {
        eprintln!(
            "\nNo battle ran, so nothing was checked. That is not a pass — fix the harness\n\
             before reading anything into the model."
        );
        std::process::exit(2);
    }
    if !tally.disagreements.is_empty() {
        std::process::exit(1);
    }
}

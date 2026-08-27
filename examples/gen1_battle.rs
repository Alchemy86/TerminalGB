//! Drive the `gen1-battle` plugin against a real Pokémon Red/Blue cartridge,
//! headlessly: start a battle from a preset, play the cinematic, and report
//! what the game actually did.
//!
//! This is the proof for `src/plugin/battle`. The unit tests there run against
//! a synthetic cartridge and can show that the right bytes go to the right
//! addresses; only this can show that the *engine* answers — that a written
//! `wCurOpponent` becomes a battle with the right opponent, that the battle
//! plays normally, and that it ends.
//!
//! ```text
//! # what is available
//! cargo run --release --example gen1_battle -- --list
//!
//! # every preset resolves against this cartridge (names, moves, classes)
//! cargo run --release --example gen1_battle -- --check
//!
//! # the captain's clip, with screenshots
//! cargo run --release --example gen1_battle -- --preset charizard-flamethrower \
//!     --shots target/battle-shots
//!
//! # the same clip three times, byte-compared
//! cargo run --release --example gen1_battle -- --preset charizard-flamethrower --repeat 3
//!
//! # ...and recorded, through the ordinary video path
//! cargo run --release --example gen1_battle -- --preset charizard-flamethrower \
//!     --video - | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 \
//!     -i - -pix_fmt yuv420p target/charizard.mp4
//! ```
//!
//! # It starts a NEW GAME, and that is the safety story
//!
//! With no `--state`, this boots the cartridge and mashes through the title
//! screen into a brand-new game. The player is standing in their bedroom with
//! **no Pokémon** — which is exactly why the preset supplies a party. So the
//! whole demonstration needs no save file, reads nobody's save, and writes
//! nothing anyone would miss. The ROM is copied into `target/` first regardless,
//! because the MBC writes its `.sav` next to the ROM path (see AGENTS.md,
//! "Never point the emulator at `~/roms` in a test").
//!
//! `--state <file>` loads a save state instead, for driving a battle from a
//! real game. That reads the file and never writes it.

use terminalgb::gameboy::Gameboy;
use terminalgb::video::{Destination, Pace, PixelFormat, Source, VideoOut, VideoSpec};
use terminalgb::KeypadKey;
use terminalgb_plugin_battle_control::preset::PresetFile;
use terminalgb_plugin_battle_control::{wram, BattlePlugin, Phase};

use std::path::{Path, PathBuf};
use std::time::Instant;

fn main() {
    let args = match Args::parse() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("gen1_battle: {e}");
            std::process::exit(2);
        }
    };

    let presets = match &args.presets {
        Some(p) => match PresetFile::load(p) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("gen1_battle: {e}");
                std::process::exit(2);
            }
        },
        None => PresetFile::builtin(),
    };

    if args.list {
        println!("battle presets:");
        for p in &presets.presets {
            println!("  {:<24} {}", p.name, p.summary());
            if p.touches_party() {
                println!("  {:<24}   (replaces the party for the fight)", "");
            }
            if !p.script.is_empty() {
                println!(
                    "  {:<24}   cinematic, {} steps, up to {} frames",
                    "",
                    p.script.steps.len(),
                    p.script.max_frames()
                );
            }
        }
        return;
    }

    let Some(rom_path) = args.rom.clone().or_else(find_rom) else {
        println!(
            "gen1_battle: no Pokémon Red/Blue cartridge found. Pass --rom <path> \
             or set GB_ROM. Skipping."
        );
        return;
    };
    let rom = match std::fs::read(&rom_path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("gen1_battle: cannot read {}: {e}", rom_path.display());
            std::process::exit(2);
        }
    };
    let staged = match stage(&rom_path, &rom) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gen1_battle: {e}");
            std::process::exit(2);
        }
    };
    println!("cartridge: {}", rom_path.display());
    println!("staged to: {}", staged.display());

    if args.check {
        std::process::exit(check_every_preset(&rom, presets));
    }

    let name = args
        .preset
        .clone()
        .unwrap_or_else(|| presets.presets[0].name.clone());

    let mut fails = 0;
    let mut signatures: Vec<Run> = Vec::new();
    for run in 0..args.repeat {
        if args.repeat > 1 {
            println!("\n--- run {} of {} ---", run + 1, args.repeat);
        }
        // Screenshots and video come from the first run only: the point of a
        // repeat is to compare, and writing the same PNGs three times says
        // nothing.
        let capture = run == 0;
        match play(&staged, &rom, &presets, &name, &args, capture) {
            Ok(r) => signatures.push(r),
            Err(e) => {
                eprintln!("FAIL {e}");
                fails += 1;
            }
        }
    }

    if signatures.len() > 1 {
        println!("\n--- reproducibility ---");
        let first = &signatures[0];
        let mut identical = true;
        for (i, r) in signatures.iter().enumerate().skip(1) {
            let same_frames = r.frame_hash == first.frame_hash;
            let same_len = r.frames == first.frames;
            let same_log = r.log == first.log;
            println!(
                "  run {}: frames {} ({}), hash {:016x} ({}), log ({})",
                i + 1,
                r.frames,
                yes_no(same_len),
                r.frame_hash,
                yes_no(same_frames),
                yes_no(same_log),
            );
            identical &= same_frames && same_len && same_log;
        }
        println!(
            "  run 1: frames {}, hash {:016x}",
            first.frames, first.frame_hash
        );
        if identical {
            println!("  PASS every run produced byte-identical frames");
        } else {
            eprintln!("  FAIL the runs diverged");
            fails += 1;
        }
    }

    if fails > 0 {
        std::process::exit(1);
    }
}

fn yes_no(b: bool) -> &'static str {
    if b {
        "same"
    } else {
        "DIFFERENT"
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

struct Args {
    rom: Option<PathBuf>,
    presets: Option<PathBuf>,
    preset: Option<String>,
    state: Option<PathBuf>,
    shots: Option<PathBuf>,
    video: Option<String>,
    frames: u32,
    repeat: usize,
    list: bool,
    check: bool,
    trace: bool,
    save_state: Option<PathBuf>,
    save_at: u32,
}

impl Args {
    fn parse() -> Result<Args, String> {
        let mut a = Args {
            rom: std::env::var("GB_ROM").ok().map(PathBuf::from),
            presets: std::env::var("GB_BATTLES").ok().map(PathBuf::from),
            preset: None,
            state: None,
            shots: None,
            video: None,
            frames: 2400,
            repeat: 1,
            list: false,
            check: false,
            trace: false,
            save_state: None,
            save_at: 0,
        };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            let mut val = || it.next().ok_or_else(|| format!("{arg} needs a value"));
            match arg.as_str() {
                "--list" => a.list = true,
                "--trace" => a.trace = true,
                "--save-state" => a.save_state = Some(PathBuf::from(val()?)),
                "--save-at" => {
                    a.save_at = val()?.parse().map_err(|_| "--save-at wants a number")?
                }
                "--check" => a.check = true,
                "--rom" => a.rom = Some(PathBuf::from(val()?)),
                "--presets" => a.presets = Some(PathBuf::from(val()?)),
                "--preset" => a.preset = Some(val()?),
                "--state" => a.state = Some(PathBuf::from(val()?)),
                "--shots" => a.shots = Some(PathBuf::from(val()?)),
                "--video" => a.video = Some(val()?),
                "--frames" => {
                    a.frames = val()?.parse().map_err(|_| "--frames wants a number")?
                }
                "--repeat" => {
                    a.repeat = val()?.parse().map_err(|_| "--repeat wants a number")?
                }
                "-h" | "--help" => {
                    println!("{}", HELP);
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument '{other}'")),
            }
        }
        if a.repeat == 0 {
            a.repeat = 1;
        }
        Ok(a)
    }
}

const HELP: &str = "\
gen1_battle — trigger a Generation 1 battle from a preset, headlessly

  --list                 show the presets and stop
  --check                resolve every preset against the cartridge and stop
  --rom <path>           the cartridge (or $GB_ROM, or ~/roms/*Blue*.gb)
  --presets <file>       a preset file (or $GB_BATTLES); default is built-in
  --preset <name>        which battle to start; default is the first
  --state <file>         load a save state instead of starting a new game
  --shots <dir>          write PNGs of the clip
  --video <dest>         raw RGBA frames to a file, a FIFO, or '-' for stdout
  --frames <n>           frame budget after arming (default 2400)
  --repeat <n>           run n times and compare the frames
  --trace                print the game's own screen every 20 frames
  --save-state <file>    write a save state at frame --save-at of the run
  --save-at <n>          which frame to write it at (default: the last)
";

// ---------------------------------------------------------------------------
// Finding and staging a cartridge
// ---------------------------------------------------------------------------

fn find_rom() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join("roms");
    let mut best = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let p = entry.path();
        let name = p.file_name()?.to_string_lossy().to_ascii_lowercase();
        if !name.ends_with(".gb") && !name.ends_with(".gbc") {
            continue;
        }
        if name.contains("blue") || name.contains("red") {
            best = Some(p);
        }
    }
    best
}

/// Copy the cartridge into `target/` before running it.
///
/// The MBC writes its battery save **next to the ROM path**, so running a
/// cartridge in place drops `.sav` files into whatever directory it came from —
/// and for the captain's `~/roms` that directory holds a real save with a
/// traded Mewtwo in it. Copying first is not caution, it is the rule.
fn stage(src: &Path, rom: &[u8]) -> Result<PathBuf, String> {
    let dir = PathBuf::from("target/gen1-battle");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {dir:?}: {e}"))?;
    let name = src
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("rom.gb"));
    let dst = dir.join(name);
    std::fs::write(&dst, rom).map_err(|e| format!("cannot write {dst:?}: {e}"))?;
    // A stale save from a previous run would put the game on the CONTINUE
    // screen and change everything downstream. Start from nothing every time —
    // that is also what makes a repeat comparable.
    for ext in ["sav", "srm", "gbsave"] {
        let _ = std::fs::remove_file(dst.with_extension(ext));
    }
    Ok(dst)
}

// ---------------------------------------------------------------------------
// --check
// ---------------------------------------------------------------------------

/// Build a plan for every preset against the real cartridge.
///
/// A preset that names a Pokémon, a move or a trainer class this game does not
/// have would otherwise fail the moment somebody pressed the key. This is the
/// check that catches it in CI instead.
fn check_every_preset(rom: &[u8], presets: PresetFile) -> i32 {
    println!("\n[check] every preset resolves against this cartridge");
    let mut bad = 0;
    for p in &presets.presets {
        let single = PresetFile {
            presets: vec![p.clone()],
        };
        match BattlePlugin::new(rom.to_vec(), single) {
            Ok(mut plugin) => match plugin.arm(&p.name) {
                Ok(()) => println!("    ok   {:<24} {}", p.name, p.summary()),
                Err(e) => {
                    println!("    FAIL {:<24} {e}", p.name);
                    bad += 1;
                }
            },
            Err(e) => {
                println!("    FAIL {:<24} {e}", p.name);
                bad += 1;
            }
        }
    }
    if bad == 0 {
        println!("    {} presets, all resolved", presets.presets.len());
        0
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// One run
// ---------------------------------------------------------------------------

struct Run {
    frames: u32,
    frame_hash: u64,
    log: Vec<String>,
}

fn play(
    staged: &Path,
    rom: &[u8],
    presets: &PresetFile,
    name: &str,
    args: &Args,
    capture: bool,
) -> Result<Run, String> {
    let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let plugin = BattlePlugin::new(rom.to_vec(), presets.clone())
        .map_err(|e| format!("plugin: {e}"))?;
    gb.add_plugin(Box::new(plugin))
        .map_err(|e| format!("plugin: {e}"))?;

    // Get to the overworld.
    match &args.state {
        Some(p) => {
            let bytes = std::fs::read(p)
                .map_err(|e| format!("cannot read save state {}: {e}", p.display()))?;
            gb.load_state(&bytes)
                .map_err(|e| format!("save state: {e:?}"))?;
            println!("loaded save state {}", p.display());
        }
        None => new_game(&mut gb)?,
    }
    println!(
        "in the overworld: party {}, map ${:02X}",
        gb.peek(wram::addr::PARTY_COUNT),
        gb.peek(0xD35E)
    );

    // Arm.
    let preset = presets.get(name).map_err(|e| e.to_string())?.clone();
    let preset_summary = preset.summary();
    // A preset with no cinematic sets a battle up and hands control back, so
    // there is no "done" to wait for. Success is the battle existing and
    // playing normally for a while.
    let scripted = !preset.script.is_empty();
    let budget = if scripted {
        args.frames
    } else {
        args.frames.min(900)
    };
    gb.plugin_mut::<BattlePlugin>()
        .ok_or("the plugin vanished")?
        .arm(name)
        .map_err(|e| e.to_string())?;
    println!("armed '{name}': {preset_summary}");

    // Optional outputs.
    let mut video = args.video.as_ref().filter(|_| capture).map(|dest| {
        let mut spec =
            VideoSpec::new(Destination::parse(dest).unwrap_or(Destination::Stdout));
        spec.format = PixelFormat::Rgba;
        spec.source = Source::Native;
        spec.pace = Pace::Emulated;
        let (v, err) = VideoOut::open(spec);
        if let Some(e) = err {
            eprintln!("video: {e}");
        }
        v
    });
    let shots = args.shots.as_ref().filter(|_| capture);
    if let Some(dir) = shots {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }

    // Run.
    let mut hash = Fnv::new();
    let mut frames = 0u32;
    let mut shot_no = 0usize;
    let mut last_phase = Phase::Idle;
    let mut saw_battle = false;
    let mut logged = 0usize;
    let mut enemy_party: Option<(u8, Vec<u8>)> = None;
    let mut final_party: Option<(u8, Vec<u8>)> = None;
    let mut opponent = None;
    let now = Instant::now();
    while frames < budget {
        gb.frame();
        frames += 1;
        hash.write(gb.image());
        if let Some(v) = video.as_mut() {
            v.frame(gb.image(), 160, 144, now);
        }

        // Record what the engine actually built, the first time it exists.
        if !saw_battle && gb.peek(wram::addr::IS_IN_BATTLE) != 0 {
            saw_battle = true;
        }
        // A trainer's roster lives in `wEnemyMons`; a wild opponent has none.
        // Sampled twice on purpose: `ReadTrainer` fills it from the cartridge
        // *before* `wIsInBattle` is set, and a custom team replaces it a frame
        // or two later — so "what the ROM said" and "what is being fought" are
        // different answers and both are worth printing.
        if saw_battle {
            let n = gb.peek(wram::addr::ENEMY_PARTY_COUNT);
            if (1..=6).contains(&n) {
                let species = (0..n as u16)
                    .map(|i| gb.peek(wram::addr::ENEMY_PARTY_SPECIES + i))
                    .collect();
                if enemy_party.is_none() {
                    enemy_party = Some((n, species));
                } else {
                    final_party = Some((n, species));
                }
            }
        }
        if saw_battle && opponent.is_none() {
            let species = gb.peek(wram::addr::ENEMY_MON);
            let level = gb.peek(wram::addr::ENEMY_MON + wram::battle_field::LEVEL as u16);
            if species != 0 && level != 0 {
                opponent = Some((species, level));
            }
        }

        if args.trace && frames % 20 == 0 {
            println!("--- frame {frames} ---");
            for row in 0..18u16 {
                // Decode row by row: `decode_screen` renders some tiles as
                // whole words ("POKé", "TRAINER"), so the string is not 360
                // characters and cannot be sliced by index.
                let tiles: Vec<u8> = (0..20u16)
                    .map(|x| gb.peek(wram::addr::TILE_MAP + row * 20 + x))
                    .collect();
                println!(
                    "  |{}|",
                    terminalgb_plugin_trainer::text::decode_screen(&tiles)
                );
            }
        }
        if capture {
            if let Some(path) = &args.save_state {
                if frames == args.save_at.max(1) {
                    let _ = std::fs::write(path, gb.save_state());
                    println!("  frame {frames:5}: save state -> {}", path.display());
                }
            }
        }
        // Print each log line as it appears, with the frame it happened on:
        // a cinematic is debugged by "which step, and when".
        if let Some(p) = gb.plugin::<BattlePlugin>() {
            while logged < p.log().len() {
                println!("  frame {frames:5}: {}", p.log()[logged]);
                logged += 1;
            }
        }
        let phase = gb
            .plugin::<BattlePlugin>()
            .map(|p| p.phase())
            .unwrap_or(Phase::Idle);
        if phase != last_phase {
            println!("  frame {frames:5}: {}", phase.label());
            if let Some(dir) = shots {
                shot_no += 1;
                save_shot(&gb, dir, shot_no, &phase.label())?;
            }
            last_phase = phase.clone();
        } else if shots.is_some() && phase == Phase::Fighting && frames % 40 == 0 {
            shot_no += 1;
            save_shot(&gb, shots.unwrap(), shot_no, "fighting")?;
        }
        if matches!(phase, Phase::Done | Phase::Failed(_)) {
            // A few more frames so a screenshot of the aftermath exists.
            for _ in 0..30 {
                gb.frame();
                frames += 1;
                hash.write(gb.image());
                if let Some(v) = video.as_mut() {
                    v.frame(gb.image(), 160, 144, now);
                }
            }
            break;
        }
    }
    if let Some(v) = video.as_mut() {
        v.flush();
    }
    if let Some(dir) = shots {
        shot_no += 1;
        save_shot(&gb, dir, shot_no, "after")?;
        println!("  {shot_no} screenshots in {}", dir.display());
    }

    let plugin = gb.plugin::<BattlePlugin>().ok_or("the plugin vanished")?;
    let phase = plugin.phase();
    let log: Vec<String> = plugin.log().to_vec();
    let refused = plugin.refused_writes();
    let started = plugin.battles_started();
    for l in log.iter().skip(logged) {
        println!("  log: {l}");
    }
    match opponent {
        Some((s, l)) => {
            println!("  active opponent: species ${s:02X} at level {l}")
        }
        None => println!("  no active opponent was read"),
    }
    let fmt = |(n, species): &(u8, Vec<u8>)| {
        let list: Vec<String> = species.iter().map(|s| format!("${s:02X}")).collect();
        format!("{n} — {}", list.join(", "))
    };
    if let Some(p) = &enemy_party {
        println!("  enemy party as ReadTrainer left it: {}", fmt(p));
    }
    if let Some(p) = &final_party {
        println!("  enemy party being fought:           {}", fmt(p));
    }
    println!("  frames {frames}, frame hash {:016x}", hash.finish());

    if refused != 0 {
        return Err(format!(
            "the plugin refused {refused} of its own writes — it tried to write \
             outside WRAM/HRAM, which is a bug in the plugin, not in the preset"
        ));
    }
    if started != 1 {
        return Err(format!("{started} battles were started, expected 1"));
    }
    if !saw_battle {
        return Err("the game never entered a battle".into());
    }
    if !scripted {
        println!(
            "  (no cinematic in this preset: the battle was set up and left \
             running, which is what 'play it by hand' means)"
        );
        return Ok(Run {
            frames,
            frame_hash: hash.finish(),
            log,
        });
    }
    match phase {
        Phase::Done => {}
        Phase::Failed(m) => return Err(format!("the plugin gave up: {m}")),
        other => {
            return Err(format!(
                "ran out of frames in phase '{}'; try --frames",
                other.label()
            ))
        }
    }
    Ok(Run {
        frames,
        frame_hash: hash.finish(),
        log,
    })
}

/// Mash through the title screen into a brand-new game.
///
/// The play clock only advances once the game is really running
/// (`wPlayTimeMinutes` / `wPlayTimeSeconds`), so that is the signal rather than
/// a frame count. Same trick as `testharness/trainer.rs`.
///
/// **Then it closes the START menu**, which is not a detail. Mashing START and
/// A alternately is what gets through the title screen and Oak's introduction,
/// and the last START press leaves the menu open — at which point the overworld
/// loop is inside `DisplayTextID` and never reaches its `wCurOpponent` test, so
/// an armed battle waits forever. That is a real property of the technique and
/// not a quirk of this harness: `wCurOpponent` is polled by the overworld loop
/// and by nothing else, so a battle can only start while the player has control.
fn new_game(gb: &mut Gameboy) -> Result<(), String> {
    let mut running = false;
    for step in 0..500u32 {
        let key = if step % 2 == 0 {
            KeypadKey::Start
        } else {
            KeypadKey::A
        };
        hold(gb, key, 4);
        run(gb, 20);
        if gb.peek(0xDA43) > 0 || gb.peek(0xDA44) > 4 {
            running = true;
            break;
        }
    }
    if !running {
        return Err("never reached the overworld from the title screen".into());
    }
    // Close whatever is open. B, never A: both dismiss a text box, but B also
    // closes a menu and can never open one (AGENTS.md, "Driving a real wild
    // encounter headlessly").
    for _ in 0..8 {
        if !menu_is_open(gb) {
            break;
        }
        hold(gb, KeypadKey::B, 4);
        run(gb, 20);
    }
    run(gb, 60);
    if menu_is_open(gb) {
        return Err("could not close the START menu".into());
    }
    Ok(())
}

/// Read the game's own screen and look for the START menu.
///
/// Gen 1 writes character codes straight into `wTileMap` and loads its font so
/// a tile id *is* the code of the glyph it draws, so the tilemap reads back as
/// the text a player can see. `OPTION` appears on the START menu and nowhere in
/// the overworld.
fn menu_is_open(gb: &mut Gameboy) -> bool {
    let tiles: Vec<u8> = (0..20 * 18)
        .map(|i| gb.peek(wram::addr::TILE_MAP + i as u16))
        .collect();
    terminalgb_plugin_trainer::text::decode_screen(&tiles).contains("OPTION")
}

fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    run(gb, frames);
    gb.keyup(key);
}

fn run(gb: &mut Gameboy, frames: u32) {
    for _ in 0..frames {
        gb.frame();
    }
}

/// Write a PNG through the `image` crate, as the frontend's screenshot keys do.
/// Not through `termimage`'s writer: that one is *stored* deflate, which is
/// right for a 60 Hz wire and wrong for a file someone keeps.
fn save_shot(gb: &Gameboy, dir: &Path, n: usize, label: &str) -> Result<(), String> {
    let safe: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let path = dir.join(format!("{n:02}-{safe}.png"));
    let img = image::RgbaImage::from_raw(160, 144, gb.image().to_vec())
        .ok_or("the framebuffer is not 160x144x4")?;
    img.save(&path)
        .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// FNV-1a, the same hash the test harness uses. No new dependency.
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 ^= b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01B3);
        }
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

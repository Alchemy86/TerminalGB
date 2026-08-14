//! Evidence tool for the agent's screen-text channel: drive a real Gen 1
//! cartridge to several distinct kinds of screen and print, for each one, what
//! the agent's observation layer actually decodes — the raw 20x18 grid, the
//! compacted text a model is sent, the mode classification, and the
//! trust/waiting/cursor signals beside it.
//!
//! ```text
//! # the opening: title screen, main menu, Oak's speech, the naming keyboard
//! cargo run --release --features plugin-agent --example screen_text -- \
//!     --rom <blue.gb> --tour
//!
//! # a loaded save: CONTINUE, the overworld, the START menu, a dialogue box
//! cargo run --release --features plugin-agent --example screen_text -- \
//!     --rom <blue.gb> --save <blue.sav> --menu-tour
//!
//! # any saved state (e.g. one gen1_battle wrote mid-battle)
//! cargo run --release --features plugin-agent --example screen_text -- \
//!     --rom <blue.gb> --state <file.gbss>
//! ```
//!
//! The ROM (and save, if given) are staged into `target/` first — the MBC
//! writes a `.sav` beside the ROM path, and a tool must never write next to
//! the user's cartridge.

use std::path::{Path, PathBuf};
use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_agent::observe::{classify, Gen1Observer, Screen};
use terminalgb_plugin_api::Capability;

fn usage() -> ! {
    eprintln!(
        "usage: screen_text --rom <path> [--save <path>] (--tour | --menu-tour | --stability | --state <gbss>)"
    );
    std::process::exit(2);
}

fn stage(rom: &Path, save: Option<&Path>) -> PathBuf {
    let dir = Path::new("target").join("screen-text");
    std::fs::create_dir_all(&dir).expect("create staging dir");
    let staged = dir.join("cart.gb");
    std::fs::copy(rom, &staged).expect("stage rom");
    let sav = staged.with_extension("sav");
    let _ = std::fs::remove_file(&sav);
    if let Some(s) = save {
        std::fs::copy(s, &sav).expect("stage save");
    }
    staged
}

fn run(gb: &mut Gameboy, frames: u32) {
    for _ in 0..frames {
        gb.frame();
    }
}

/// Tap a key and run until the press has been performed plus a settle.
fn tap(gb: &mut Gameboy, key: KeypadKey, settle: u32) {
    gb.tap(key);
    while !gb.input_idle() {
        gb.frame();
    }
    run(gb, settle);
}

fn read_screen(gb: &mut Gameboy) -> Screen {
    gb.with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
        Screen::read(m)
    })
}

fn dump(gb: &mut Gameboy, label: &str) -> Screen {
    let (screen, game) = gb
        .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
            (Screen::read(m), Gen1Observer::new(true).read(m))
        });
    let mode = classify(&screen, &game);
    println!("==================== {label} ====================");
    println!(
        "mode={} kind={} trust={} waiting={} cursor={:?},{:?} selected={:?} distinct={}",
        mode.name(),
        screen.kind().name(),
        screen.trust().label(),
        screen.waiting,
        screen.cursor_row,
        screen.cursor_col,
        screen.selected(),
        screen.distinct,
    );
    println!("--- raw 20x18 grid ---");
    for line in screen.text.lines() {
        println!("|{line}|");
    }
    if std::env::var("GB_SCREEN_IDS").is_ok() {
        println!("--- raw tile ids ---");
        for (r, row) in screen.tiles.chunks(20).enumerate() {
            let ids: Vec<String> = row.iter().map(|t| format!("{t:02X}")).collect();
            println!("{r:2}: {}", ids.join(" "));
        }
    }
    println!("--- compact_text (what the model is sent) ---");
    let compact = screen.compact_text();
    if compact.is_empty() {
        println!("(empty)");
    } else {
        for line in compact.lines() {
            println!("|{line}|");
        }
    }
    println!();
    screen
}

/// How still does the tilemap actually hold? The snapshot only sends text once
/// the screen has stopped changing, so anything that ticks the tilemap for ever
/// — a blinking `▼`, an animated tile — would starve that gate and blind the
/// agent on exactly the screens it most needs to read. Measure it rather than
/// assuming: sample every frame and report the longest run of identical reads.
fn stability(gb: &mut Gameboy, label: &str, frames: u32) {
    let mut last = 0u64;
    let mut run = 0u32;
    let mut best = 0u32;
    let mut changes = 0u32;
    for i in 0..frames {
        gb.frame();
        let h = read_screen(gb).hash;
        if i > 0 && h == last {
            run += 1;
        } else {
            if i > 0 {
                changes += 1;
            }
            best = best.max(run);
            run = 1;
        }
        last = h;
    }
    best = best.max(run);
    println!(
        "stability[{label}]: over {frames} frames, {changes} change(s), \
         longest identical run {best} frames"
    );
}

/// Press A whenever the game shows the ▼ arrow, until `stop` matches the
/// screen text or the frame budget runs out. Screen-driven, like every robust
/// harness in this repo: never a fixed frame count.
fn advance_until(gb: &mut Gameboy, stop: &str, rounds: u32) -> bool {
    for _ in 0..rounds {
        let s = read_screen(gb);
        if s.text.contains(stop) {
            return true;
        }
        // Press A whether or not the ▼ arrow is showing this instant — it
        // blinks, and some prompts take A without one. The check above runs
        // before every press, so the target screen is never overshot.
        tap(gb, KeypadKey::A, 12);
    }
    false
}

fn main() {
    let mut rom = None;
    let mut save = None;
    let mut mode = String::new();
    let mut state = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom = args.next().map(PathBuf::from),
            "--save" => save = args.next().map(PathBuf::from),
            "--state" => {
                mode = "state".into();
                state = args.next().map(PathBuf::from);
            }
            "--tour" => mode = "tour".into(),
            "--stability" => mode = "stability".into(),
            "--menu-tour" => mode = "menu-tour".into(),
            _ => usage(),
        }
    }
    let rom = rom.unwrap_or_else(|| usage());
    let staged = stage(&rom, save.as_deref());
    let data = std::fs::read(&staged).expect("read rom");
    let mut gb = Gameboy::new(data, Some(staged.clone()));

    match mode.as_str() {
        "tour" => {
            // Power-on. The Blue intro runs a logo and an attract animation
            // before the title screen.
            run(&mut gb, 700);
            dump(&mut gb, "intro (Game Freak logo tiles, no text on screen)");
            // Let the intro run on: the Game Freak / copyright screens draw
            // artwork as sequential tiles through the font range, which is
            // the graphics-as-text case the trust flag exists for.
            let mut salad = false;
            for _ in 0..24 {
                run(&mut gb, 100);
                if read_screen(&mut gb).graphics_as_text() {
                    salad = true;
                    dump(&mut gb, "intro artwork (tiles overlap the font)");
                    break;
                }
            }
            assert!(salad, "never saw the graphics-as-text intro screen");
            // Screen-driven: press START until the main menu is up. The press
            // before the menu appears was made on the title screen, so dump
            // the screen seen just before that transition as "title".
            let mut before = read_screen(&mut gb);
            let mut reached = false;
            for _ in 0..80 {
                let s = read_screen(&mut gb);
                if s.text.contains("NEW GAME") {
                    reached = true;
                    break;
                }
                before = s;
                tap(&mut gb, KeypadKey::Start, 40);
            }
            assert!(reached, "never reached the main menu");
            println!("(title screen was the frame before the menu — raw grid:)");
            for line in before.text.lines() {
                println!("|{line}|");
            }
            println!();
            dump(&mut gb, "main menu (NEW GAME / OPTION)");
            // A on NEW GAME starts Oak's speech.
            tap(&mut gb, KeypadKey::A, 60);
            // Advance to the first full text box.
            let mut ok = false;
            for _ in 0..400 {
                let s = read_screen(&mut gb);
                if s.waiting {
                    ok = true;
                    break;
                }
                run(&mut gb, 10);
            }
            assert!(ok, "never reached Oak's speech");
            dump(&mut gb, "dialogue (Oak's speech, waiting for A)");
            // On to the naming screen. Oak asks "YOUR NAME?" over a preset
            // list first in Gen 1 (NEW NAME / RED / ASH / JACK).
            let reached = advance_until(&mut gb, "NEW NAME", 400);
            assert!(reached, "never reached the naming prompt");
            run(&mut gb, 30);
            dump(&mut gb, "name prompt (preset list)");
            // NEW NAME -> the on-screen keyboard.
            tap(&mut gb, KeypadKey::A, 90);
            dump(&mut gb, "naming keyboard");
        }
        "menu-tour" => {
            // With a save staged the main menu gains CONTINUE, cursor on it.
            // Screen-driven: press START through the intro until it shows.
            run(&mut gb, 700);
            let mut reached = false;
            for _ in 0..80 {
                if read_screen(&mut gb).text.contains("CONTINUE") {
                    reached = true;
                    break;
                }
                tap(&mut gb, KeypadKey::Start, 40);
            }
            assert!(reached, "never reached the main menu");
            dump(&mut gb, "main menu (CONTINUE / NEW GAME / OPTION)");
            tap(&mut gb, KeypadKey::A, 30);
            dump(&mut gb, "continue confirmation (player / badges / time)");
            tap(&mut gb, KeypadKey::A, 60);
            // The map fades in; wait until the observer calls it overworld.
            let mut ok = false;
            for _ in 0..100 {
                let (s, g) = gb
                    .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                        (Screen::read(m), Gen1Observer::new(true).read(m))
                    });
                if classify(&s, &g) == terminalgb_plugin_agent::observe::Mode::Overworld {
                    ok = true;
                    break;
                }
                run(&mut gb, 10);
            }
            assert!(ok, "never reached the overworld");
            dump(&mut gb, "overworld (Viridian Pokemon Center)");
            tap(&mut gb, KeypadKey::Start, 40);
            dump(&mut gb, "START menu");
            tap(&mut gb, KeypadKey::B, 40);
            // Talk to whoever is straight ahead — in the Viridian Pokémon
            // Center save that is the counter to the north.
            tap(&mut gb, KeypadKey::Up, 20);
            tap(&mut gb, KeypadKey::Up, 20);
            tap(&mut gb, KeypadKey::A, 10);
            let mut talking = false;
            for _ in 0..60 {
                if read_screen(&mut gb).waiting {
                    talking = true;
                    break;
                }
                run(&mut gb, 10);
            }
            assert!(talking, "nobody answered");
            dump(&mut gb, "dialogue (the nurse)");
            stability(&mut gb, "dialogue waiting on the blinking arrow", 240);
        }
        "stability" => {
            run(&mut gb, 700);
            let mut reached = false;
            for _ in 0..80 {
                if read_screen(&mut gb).text.contains("CONTINUE") {
                    reached = true;
                    break;
                }
                tap(&mut gb, KeypadKey::Start, 40);
            }
            assert!(reached, "never reached the main menu");
            stability(&mut gb, "main menu", 240);
            tap(&mut gb, KeypadKey::A, 30);
            tap(&mut gb, KeypadKey::A, 60);
            let mut ok = false;
            for _ in 0..100 {
                let (s, g) = gb
                    .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                        (Screen::read(m), Gen1Observer::new(true).read(m))
                    });
                if classify(&s, &g) == terminalgb_plugin_agent::observe::Mode::Overworld {
                    ok = true;
                    break;
                }
                run(&mut gb, 10);
            }
            assert!(ok, "never reached the overworld");
            stability(&mut gb, "overworld standing still", 240);
            tap(&mut gb, KeypadKey::Start, 40);
            stability(&mut gb, "START menu", 240);
            tap(&mut gb, KeypadKey::B, 40);
            tap(&mut gb, KeypadKey::Up, 20);
            tap(&mut gb, KeypadKey::Up, 20);
            tap(&mut gb, KeypadKey::A, 10);
            let mut talking = false;
            for _ in 0..60 {
                if read_screen(&mut gb).waiting {
                    talking = true;
                    break;
                }
                run(&mut gb, 10);
            }
            assert!(talking, "nobody answered");
            stability(&mut gb, "dialogue waiting on the blinking arrow", 240);
        }
        "state" => {
            let st = state.unwrap_or_else(|| usage());
            let bytes = std::fs::read(&st).expect("read state");
            gb.load_state(&bytes).expect("load state");
            run(&mut gb, 2);
            dump(&mut gb, "loaded state");
            // If this is a battle, A through the announcement to the menu.
            if advance_until(&mut gb, "FIGHT", 40) {
                run(&mut gb, 20);
                dump(&mut gb, "battle menu (FIGHT / PKMN / ITEM / RUN)");
                // And one level deeper: the move list.
                tap(&mut gb, KeypadKey::A, 40);
                dump(&mut gb, "move list");
            }
        }
        _ => usage(),
    }
}

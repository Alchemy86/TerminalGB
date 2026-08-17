//! What Pokémon Red and Blue put on screen when a Pokémon learns a move or
//! evolves — traced frame by frame against a real cartridge.
//!
//! ```text
//! cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
//!     --example gen1_learn_drill -- trace  --rom <blue.gb>
//! cargo run --release ... --example gen1_learn_drill -- cancel --rom <blue.gb>
//! cargo run --release ... --example gen1_learn_drill -- moveset --rom <blue.gb>
//! ```
//!
//! # Why
//!
//! `docs/gen1/catching.md` established that a wild battle and a trainer battle
//! are the **same picture** at the moment a policy wants to act, and that the
//! only separator is an opening line nine frames wide. Two modal interruptions
//! arrive after a battle — *learn a move* and *evolve* — and confusing them
//! would be expensive, so they get the same treatment: trace every frame,
//! decode `wTileMap` as text, and say exactly what is on screen and for how
//! long.
//!
//! It needs the player's own Pokémon Red or Blue. **It starts a new game**, so
//! it reads no save file, and it stages the ROM into `target/` first because
//! the MBC writes its `.sav` next to whatever path it is handed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_battle_control::wram::addr::TILE_MAP;
use terminalgb_plugin_battle_control::{preset::PresetFile, BattlePlugin};
use terminalgb_plugin_trainer::gen1::{addr as tr, party};

const PRESETS: &str = include_str!("gen1_learn_drill.preset");

/// Gen 1's `HandleMenuInput` delays several frames after accepting a press, so
/// a burst arrives out of phase. Same constant the catch drill uses.
const MENU_GAP: u32 = 24;
/// A whole battle, generously. A CHANSEY at level 10 dies to two HYPER BEAMs.
const BATTLE_FRAMES: u32 = 4000;

// ---------------------------------------------------------------------------

fn find_rom() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    for e in std::fs::read_dir(Path::new(&home).join("roms")).ok()? {
        let p = e.ok()?.path();
        let n = p.file_name()?.to_string_lossy().to_lowercase();
        if n.ends_with(".gb") && (n.contains("blue") || n.contains("red")) {
            return Some(p);
        }
    }
    None
}

fn stage(rom: &Path) -> Result<PathBuf, String> {
    let dir = PathBuf::from("target/gen1-learn-drill");
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let dest = dir.join(rom.file_name().ok_or("the ROM path has no filename")?);
    std::fs::copy(rom, &dest).map_err(|e| format!("cannot stage the ROM: {e}"))?;
    let _ = std::fs::remove_file(dest.with_extension("sav"));
    Ok(dest)
}

fn run(gb: &mut Gameboy, n: u32) {
    for _ in 0..n {
        gb.frame();
    }
}

fn hold(gb: &mut Gameboy, key: KeypadKey, n: u32) {
    gb.keydown(key);
    run(gb, n);
    gb.keyup(key);
}

fn tiles(gb: &mut Gameboy) -> Vec<u8> {
    (0..20u16 * 18).map(|i| gb.peek(TILE_MAP + i)).collect()
}

/// `wTileMap` decoded row by row.
///
/// **`text::decode_screen` returns one flat 360-character string**, so a
/// caller that wants rows has to cut the tiles up first — 20 at a time — or
/// every row runs into the next and nothing about the layout survives.
fn screen(gb: &mut Gameboy) -> String {
    let t = tiles(gb);
    (0..18)
        .map(|r| terminalgb_plugin_trainer::text::decode_screen(&t[r * 20..r * 20 + 20]))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The screen as one line with every run of blanks collapsed, which is what a
/// phrase is searched in: Gen 1 wraps a sentence across rows and pads each row
/// out with spaces, so `is trying to learn` is only contiguous after this.
fn oneline(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// One line of the screen, so a report can quote what a player sees.
fn flat(s: &str) -> String {
    s.split('\n')
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The text box: Gen 1 draws dialogue on the bottom rows, so that is where
/// every prompt this drill is looking for lives.
fn textbox(s: &str) -> String {
    let rows: Vec<&str> = s.split('\n').collect();
    rows.iter()
        .rev()
        .take(5)
        .rev()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" / ")
}

/// The START menu is open, which means the overworld loop is inside
/// `DisplayTextID` and will never poll `wCurOpponent`.
fn menu_is_open(gb: &mut Gameboy) -> bool {
    screen(gb).contains("OPTION")
}

/// Mash through the title screen into a brand-new game, then close the menu.
///
/// The play clock is the signal, not a frame count — the same trick
/// `examples/gen1_battle.rs` and `testharness/trainer.rs` use.
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
        if gb.peek(tr::PLAY_TIME_MINUTES) > 0 || gb.peek(tr::PLAY_TIME_SECONDS) > 4 {
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
        run(gb, 20);
    }
    run(gb, 60);
    if menu_is_open(gb) {
        return Err("could not close the START menu".into());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// the party, read back
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mon {
    species: u8,
    level: u8,
    moves: [u8; 4],
}

fn read_mon(gb: &mut Gameboy) -> Mon {
    let base = tr::PARTY_MONS;
    let mut moves = [0u8; 4];
    for (i, m) in moves.iter_mut().enumerate() {
        *m = gb.peek(base + party::MOVES + i as u16);
    }
    Mon {
        species: gb.peek(base + party::SPECIES),
        level: gb.peek(base + party::LEVEL),
        moves,
    }
}

/// The four ranges that make up the party, so the drill can keep a levelled
/// Pokémon across battles.
///
/// **`BattlePlugin` restores everything it wrote when the battle ends** — the
/// save-safety rule `docs/gen1/battle-control.md` records — so a preset that
/// injects a party hands it back afterwards and every level gained inside the
/// battle goes with it. This drill wants the opposite: it snapshots the party
/// while the battle is still running, and writes that back after the plugin has
/// finished restoring. Declared assistance, and it only ever touches WRAM.
const PARTY_BLOCKS: [(u16, u16); 5] = [
    (tr::PARTY_COUNT, 1),
    (tr::PARTY_SPECIES, 7),
    (tr::PARTY_MONS, 6 * 44),
    (tr::PARTY_MON_OT, 6 * 11),
    (tr::PARTY_MON_NICKS, 6 * 11),
];

fn snapshot_party(gb: &mut Gameboy) -> Vec<u8> {
    let mut out = Vec::new();
    for (at, n) in PARTY_BLOCKS {
        for i in 0..n {
            out.push(gb.peek(at + i));
        }
    }
    out
}

fn restore_party(gb: &mut Gameboy, snap: &[u8]) {
    let mut i = 0;
    for (at, n) in PARTY_BLOCKS {
        for k in 0..n {
            gb.debug_write(at + k, snap[i]);
            i += 1;
        }
    }
    // **And top the PP up.** A drill that fights a dozen battles on one HYPER
    // BEAM runs it out of its 5 PP, and a Pokemon with no usable move parks the
    // battle menu for ever — which reads exactly like the harness hanging.
    // The low six bits of a PP byte are the count (`ram/wram.asm`,
    // `wPartyMon1PP`); 63 is the largest it can hold.
    for slot in 0..6u16 {
        for m in 0..4u16 {
            gb.debug_write(tr::PARTY_MONS + slot * 44 + party::PP + m, 63);
        }
    }
}

fn read_enemy_moves(gb: &mut Gameboy) -> [u8; 4] {
    let mut moves = [0u8; 4];
    for (i, m) in moves.iter_mut().enumerate() {
        *m = gb.peek(tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::MOVES + i as u16);
    }
    moves
}

// ---------------------------------------------------------------------------
// arming and fighting
// ---------------------------------------------------------------------------

struct Arena {
    gb: Gameboy,
}

impl Arena {
    fn boot(staged: &Path, rom: &[u8], presets: &PresetFile) -> Result<Arena, String> {
        let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
        let plugin = BattlePlugin::new(rom.to_vec(), presets.clone())
            .map_err(|e| format!("plugin: {e}"))?;
        gb.add_plugin(Box::new(plugin))
            .map_err(|e| format!("plugin: {e}"))?;
        new_game(&mut gb)?;
        Ok(Arena { gb })
    }

    fn arm(&mut self, name: &str) -> Result<(), String> {
        self.gb
            .plugin_mut::<BattlePlugin>()
            .ok_or("the plugin vanished")?
            .arm(name)
            .map_err(|e| e.to_string())
    }

    /// Step until the engine reports a battle.
    fn await_battle(&mut self, limit: u32) -> Result<(), String> {
        for f in 0..limit {
            self.gb.frame();
            if f % MENU_GAP == 0 {
                self.gb.keydown(KeypadKey::A);
            } else if f % MENU_GAP == 4 {
                self.gb.keyup(KeypadKey::A);
            }
            if matches!(self.gb.peek(tr::IS_IN_BATTLE), 1 | 2) {
                self.gb.keyup(KeypadKey::A);
                return Ok(());
            }
        }
        Err("no battle started".into())
    }

    /// Fight to the end, sampling the screen on **every** frame.
    ///
    /// Pressing A on the cadence the menus accept is all the policy there is:
    /// A on the top menu opens FIGHT, A again takes move slot 1, and A carries
    /// every text box. The preset puts the strong move in slot 1.
    fn fight(&mut self, limit: u32, cancel_evolution: bool) -> (Vec<(u32, String)>, Option<Vec<u8>>) {
        let mut out = Vec::new();
        let mut cancelled = false;
        let mut snap: Option<Vec<u8>> = None;
        for f in 0..limit {
            // **Answer the evolution prompt with B when asked to.** B is the
            // cancel; it has to be pressed while the prompt is up, so it
            // replaces the A of this cadence rather than being added to it.
            let s = screen(&mut self.gb);
            // **B has to be HELD, not tapped.** `EvolveMon` prints its line
            // and then runs the animation, polling the held-button state each
            // iteration; a four-frame pulse advances the text and is long gone
            // by the time the animation asks. So while the phrase is up, B is
            // down on every frame and A is never pressed at all — measured,
            // because two earlier versions of this pressed B and the Pokemon
            // evolved anyway.
            let one = oneline(&s);
            let evolving = one.contains("evolving") || one.contains("grew to level");
            if cancel_evolution && (evolving || cancelled) {
                cancelled = true;
                self.gb.keyup(KeypadKey::A);
                // Pulse rather than hold: Gen 1 reads some buttons through
                // `JoypadLowSensitivity`, which only sees a transition, so a
                // held button can register exactly once.
                if f % 4 < 2 {
                    self.gb.keydown(KeypadKey::B);
                } else {
                    self.gb.keyup(KeypadKey::B);
                }
            } else {
                self.gb.keyup(KeypadKey::B);
                if f % MENU_GAP == 0 {
                    self.gb.keydown(KeypadKey::A);
                } else if f % MENU_GAP == 4 {
                    self.gb.keyup(KeypadKey::A);
                }
            }
            self.gb.frame();
            out.push((f, s));
            // Snapshot whenever there IS a party. The evolution and the
            // move-learn happen *after* `wIsInBattle` has gone back to zero,
            // so a snapshot gated on being in a battle captures the Pokemon
            // one moment too early and puts the pre-evolution form back.
            if f % 8 == 0 && self.gb.peek(tr::PARTY_COUNT) > 0
                && self.gb.peek(tr::PARTY_SPECIES) != 0
            {
                snap = Some(snapshot_party(&mut self.gb));
            }
            if f > 60 && self.gb.peek(tr::IS_IN_BATTLE) == 0 && f % MENU_GAP == MENU_GAP - 1 {
                // Out of battle and the screen has settled: give the
                // post-battle prompts room to appear and be answered.
                if out
                    .iter()
                    .rev()
                    .take(240)
                    .all(|(_, t)| !t.contains("learn") && !t.contains("evolv"))
                {
                    break;
                }
            }
        }
        self.gb.keyup(KeypadKey::A);
        self.gb.keyup(KeypadKey::B);
        (out, snap)
    }
}

// ---------------------------------------------------------------------------
// the phrases the two prompts are made of
// ---------------------------------------------------------------------------

/// Substrings a decoded screen is searched for, with the name to report them
/// under. Every one of these was **read off the traced screens**, not recalled.
const PHRASES: &[(&str, &str)] = &[
    ("grew to level", "level-up announcement"),
    ("is trying to", "learn: the offer"),
    ("learn more than", "learn: the four-move refusal"),
    ("Delete an older", "learn: the question"),
    ("Which move should", "learn: the move menu"),
    ("Poof!", "learn: the confirmation"),
    ("did not learn", "learn: declined"),
    ("Abandon learning", "learn: abandon check"),
    ("evolving", "evolve: the announcement"),
    ("stopped evolving", "evolve: cancelled"),
    ("evolved into", "evolve: the confirmation"),
];

fn windows(trace: &[(u32, String)]) -> BTreeMap<&'static str, (u32, u32, usize, String)> {
    let mut out: BTreeMap<&str, (u32, u32, usize, String)> = BTreeMap::new();
    for (f, s) in trace {
        for (needle, label) in PHRASES {
            if oneline(s).contains(*needle) {
                let e = out
                    .entry(label)
                    .or_insert((*f, *f, 0, oneline(s).chars().take(150).collect()));
                e.1 = *f;
                e.2 += 1;
            }
        }
    }
    out
}

/// The whole screen, as a player sees it, at the frame a phrase first appears.
///
/// This is the evidence for the only question that matters to a screen-only
/// student: **is what it needs actually legible?**
fn show_screens(trace: &[(u32, String)]) {
    // **The LAST frame of a phrase's window, not the first.** Gen 1 types text
    // out a character at a time and draws a menu after the question that opens
    // it, so the first frame a phrase appears on shows half a sentence and no
    // list at all.
    for (needle, label) in PHRASES {
        let last = trace
            .iter()
            .filter(|(_, s)| oneline(s).contains(*needle))
            .next_back();
        if let Some((f, s)) = last {
            println!("    --- {label}, frame {f} (the last it is up) ---");
            for row in s.split('\n') {
                println!("    |{row}|");
            }
            // ...and again once the box has scrolled, because a Gen 1 text box
            // holds two lines and these sentences are three.
            if let Some((f2, s2)) = trace.iter().find(|(x, _)| *x == f + 30) {
                println!("    --- {label}, frame {f2} (30 frames later) ---");
                for row in s2.split('\n') {
                    println!("    |{row}|");
                }
            }
        }
    }
}

fn report_windows(trace: &[(u32, String)]) {
    if std::env::var_os("GB_LEARN_SCREENS").is_some() {
        show_screens(trace);
    }
    let w = windows(trace);
    if w.is_empty() {
        println!("    (no prompt phrase appeared at all)");
        return;
    }
    for (label, (a, b, n, text)) in &w {
        println!("    {label:<32} frames {a}..{b}, {n} of them");
        println!("      {text}");
    }
    // The sampling constraint, exactly as `catching.md` measures it for the
    // wild/trainer line: a window narrower than a sampler's interval is a
    // window that sampler can miss.
    for (_, label) in PHRASES {
        let frames: Vec<u32> = trace
            .iter()
            .filter(|(_, s)| {
                let one = oneline(s);
                PHRASES
                    .iter()
                    .any(|(needle, l)| l == label && one.contains(*needle))
            })
            .map(|(f, _)| *f)
            .collect();
        if frames.is_empty() {
            continue;
        }
        let miss: Vec<String> = [1u32, 2, 4, 6, 8, 12, 16, 24, 32, 48]
            .iter()
            .map(|iv| {
                let missed = (0..*iv).filter(|ph| !frames.iter().any(|f| f % iv == *ph)).count();
                format!("{iv}:{missed}/{iv}")
            })
            .collect();
        println!("    {label:<32} phases that MISS it: {}", miss.join(" "));
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

fn presets() -> Result<PresetFile, String> {
    PresetFile::parse(PRESETS, "gen1_learn_drill.preset").map_err(|e| e.to_string())
}

fn levels_to(
    a: &mut Arena,
    scenario: &str,
    target: u8,
    cancel: bool,
    setup: bool,
) -> Result<Vec<(u32, String)>, String> {
    let mut trace = Vec::new();
    for round in 0..10 {
        let before = read_mon(&mut a.gb);
        if (round > 0 || !setup) && before.level >= target {
            break;
        }
        // Round 0 sets the party up; every round after it is the opponent
        // alone, so the experience the party has gained is not written over.
        a.arm(if round == 0 && setup { scenario } else { "chansey" })?;
        a.await_battle(4000)?;
        if round == 0 {
            let m = read_mon(&mut a.gb);
            println!(
                "    party before: species {} level {} moves {:?}",
                m.species, m.level, m.moves
            );
        }
        let (t, snap) = a.fight(BATTLE_FRAMES, cancel);
        trace = t;
        if let Some(snap) = snap {
            restore_party(&mut a.gb, &snap);
        }
        let after = read_mon(&mut a.gb);
        if a.gb.peek(tr::IS_IN_BATTLE) == 0xFF {
            return Err(format!(
                "round {round}: the player blacked out — the opponent is too \
                 strong for this preset (wIsInBattle $FF)"
            ));
        }
        println!(
            "    round {round}: level {} -> {}, species {} -> {}, {} frames, in_battle {}",
            before.level, after.level, before.species, after.species,
            trace.len(), a.gb.peek(tr::IS_IN_BATTLE)
        );
        if std::env::var_os("GB_LEARN_DUMP").is_some() {
            let mut last = String::new();
            for (f, t) in &trace {
                let one = textbox(t);
                if one != last {
                    println!("      f{f:<5} {}", one.chars().take(120).collect::<String>());
                    last = one;
                }
            }
        }
        if after.level >= target {
            break;
        }
    }
    Ok(trace)
}

fn cmd_trace(rom: &Path) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let ps = presets()?;

    for (scenario, target, what) in [
        ("learn-bulbasaur", 13u8, "a move-learn prompt and nothing else"),
        ("evolve-bulbasaur", 16, "an evolution and nothing else"),
        ("evolve-and-learn-abra", 16, "an evolution AND a move at the same level"),
    ] {
        println!("== {scenario}: {what} ==");
        let mut a = Arena::boot(&staged, &bytes, &ps)?;
        let trace = levels_to(&mut a, scenario, target, false, true)?;
        let m = read_mon(&mut a.gb);
        println!(
            "    party after:  species {} level {} moves {:?}",
            m.species, m.level, m.moves
        );
        report_windows(&trace);
        println!();
    }
    Ok(())
}

fn cmd_cancel(rom: &Path) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let ps = presets()?;
    println!("== cancelling an evolution: is it offered again? ==");
    let mut a = Arena::boot(&staged, &bytes, &ps)?;
    let trace = levels_to(&mut a, "evolve-bulbasaur", 16, true, true)?;
    let m = read_mon(&mut a.gb);
    println!("    after pressing B: species {} level {}", m.species, m.level);
    report_windows(&trace);
    println!("    -- now level it once more, pressing nothing but A --");
    // **No setup preset this time.** Re-arming the scenario would inject a
    // fresh level 15 BULBASAUR and answer a different question entirely; the
    // point is what happens to the Pokemon that just refused.
    let trace2 = levels_to(&mut a, "evolve-bulbasaur", m.level + 1, false, false)?;
    let m2 = read_mon(&mut a.gb);
    println!("    after the next level-up: species {} level {}", m2.species, m2.level);
    report_windows(&trace2);
    Ok(())
}

/// Does a wild Pokémon really hold the moves `WriteMonMoves` would give it?
///
/// `tools/gen1-tables/learn_table.py` rests on "a caught Pokémon's move set is
/// a function of its species and its level" — which is the claim that makes the
/// decision table finite. This is that claim, measured.
fn cmd_moveset(rom: &Path) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let ps = presets()?;
    println!("== what a wild Pokemon at a level actually holds ==");
    println!("   (compare with `learn_table.py`'s `moveset_at`, which is what");
    println!("    makes the decision table finite)");
    for name in [
        "moveset-tentacool",
        "moveset-nidorino",
        "moveset-pidgeotto",
    ] {
        let mut a = Arena::boot(&staged, &bytes, &ps)?;
        a.arm(name)?;
        a.await_battle(4000)?;
        run(&mut a.gb, 240);
        let lv = a
            .gb
            .peek(tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::LEVEL);
        println!("    {name:<20} level {lv:<3} moves {:?}", read_enemy_moves(&mut a.gb));
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rom: Option<PathBuf> = None;
    let mut cmd = String::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rom" => {
                i += 1;
                rom = args.get(i).map(PathBuf::from);
            }
            other if cmd.is_empty() => cmd = other.to_string(),
            other => {
                eprintln!("gen1_learn_drill: unexpected argument {other:?}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let rom = match rom.or_else(find_rom) {
        Some(r) => r,
        None => {
            eprintln!(
                "gen1_learn_drill: no cartridge. Pass --rom <blue.gb>; none is \
                 distributed with this repository."
            );
            std::process::exit(2);
        }
    };
    let r = match cmd.as_str() {
        "trace" => cmd_trace(&rom),
        "cancel" => cmd_cancel(&rom),
        "moveset" => cmd_moveset(&rom),
        _ => Err("usage: gen1_learn_drill <trace|cancel|moveset> --rom <blue.gb>".into()),
    };
    if let Err(e) = r {
        eprintln!("gen1_learn_drill: {e}");
        std::process::exit(1);
    }
}

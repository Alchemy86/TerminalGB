//! What Pokémon Red and Blue put on screen when a Pokémon learns a move or
//! evolves — traced frame by frame against a real cartridge.
//!
//! ```text
//! cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
//!     --example gen1_learn_drill -- trace  --rom <blue.gb>
//! cargo run --release ... --example gen1_learn_drill -- cancel --rom <blue.gb>
//! cargo run --release ... --example gen1_learn_drill -- moveset --rom <blue.gb>
//!
//! # ...and the drill proper: every level-up on one line, triggered directly,
//! # played out, and recorded as training data.
//! python3 tools/gen1-tables/learn_table.py --rom <blue.gb> \
//!     --drill-plan charmander > charmander.plan.json
//! cargo run --release ... --example gen1_learn_drill -- drill \
//!     --rom <blue.gb> --plan charmander.plan.json --out target/learn-drill
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
        *m = gb.peek(
            tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::MOVES + i as u16,
        );
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
    fn fight(
        &mut self,
        limit: u32,
        cancel_evolution: bool,
    ) -> (Vec<(u32, String)>, Option<Vec<u8>>) {
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
            if f % 8 == 0
                && self.gb.peek(tr::PARTY_COUNT) > 0
                && self.gb.peek(tr::PARTY_SPECIES) != 0
            {
                snap = Some(snapshot_party(&mut self.gb));
            }
            if f > 60
                && self.gb.peek(tr::IS_IN_BATTLE) == 0
                && f % MENU_GAP == MENU_GAP - 1
            {
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
                let e = out.entry(label).or_insert((
                    *f,
                    *f,
                    0,
                    oneline(s).chars().take(150).collect(),
                ));
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
                let missed = (0..*iv)
                    .filter(|ph| !frames.iter().any(|f| f % iv == *ph))
                    .count();
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
        a.arm(if round == 0 && setup {
            scenario
        } else {
            "chansey"
        })?;
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
                    println!(
                        "      f{f:<5} {}",
                        one.chars().take(120).collect::<String>()
                    );
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
        (
            "learn-bulbasaur",
            13u8,
            "a move-learn prompt and nothing else",
        ),
        ("evolve-bulbasaur", 16, "an evolution and nothing else"),
        (
            "evolve-and-learn-abra",
            16,
            "an evolution AND a move at the same level",
        ),
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
    println!(
        "    after pressing B: species {} level {}",
        m.species, m.level
    );
    report_windows(&trace);
    println!("    -- now level it once more, pressing nothing but A --");
    // **No setup preset this time.** Re-arming the scenario would inject a
    // fresh level 15 BULBASAUR and answer a different question entirely; the
    // point is what happens to the Pokemon that just refused.
    let trace2 = levels_to(&mut a, "evolve-bulbasaur", m.level + 1, false, false)?;
    let m2 = read_mon(&mut a.gb);
    println!(
        "    after the next level-up: species {} level {}",
        m2.species, m2.level
    );
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
    for name in ["moveset-tentacool", "moveset-nidorino", "moveset-pidgeotto"] {
        let mut a = Arena::boot(&staged, &bytes, &ps)?;
        a.arm(name)?;
        a.await_battle(4000)?;
        run(&mut a.gb, 240);
        let lv =
            a.gb.peek(tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::LEVEL);
        println!(
            "    {name:<20} level {lv:<3} moves {:?}",
            read_enemy_moves(&mut a.gb)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// the drill: every level-up on one line, triggered directly and played out
// ---------------------------------------------------------------------------

mod drill {
    //! Trigger each of a line's level-ups **directly** — do not grind to it —
    //! answer the prompt it raises, and record what a screen-only player would
    //! have seen beside the button the teacher pressed.
    //!
    //! # Three things this is careful about
    //!
    //! **It decides nothing.** The criterion lives in
    //! `tools/gen1-tables/learn_table.py` and arrives as a plan with the answer
    //! already in it. A second implementation of the rule here would be a
    //! second implementation of the rule, and the two would disagree the next
    //! time either was improved.
    //!
    //! **The cartridge is the judge, not this drill.** After every event the
    //! party's level, species and four move slots are read back out of WRAM and
    //! compared with what the plan said they would be. A prompt that was
    //! answered but produced the wrong move set is a failure, not a pass.
    //!
    //! **The corpus is the emulator's own agent format**, one NDJSON record per
    //! button the teacher pressed, with the observation verbatim in `request`
    //! and the plan verbatim in `reply` — exactly what
    //! `plugins/agent/src/lib.rs` writes for a live run and what
    //! `tools/agent-docs/log_to_dataset.py` already flattens. Nothing new to
    //! consume, and an arm of the A/B rig speaks the same protocol.

    use std::path::Path;

    use super::{new_game, read_mon, restore_party, snapshot_party, stage, Mon};
    use terminalgb::gameboy::Gameboy;
    use terminalgb::plugin::Capability;
    use terminalgb::KeypadKey;
    use terminalgb_plugin_agent::json::{self, Json};
    use terminalgb_plugin_agent::observe::{Gen1Observer, Screen};
    use terminalgb_plugin_battle_control::{preset::PresetFile, BattlePlugin};
    use terminalgb_plugin_gen1_link::romdata::{experience_for_level, RomTables};
    use terminalgb_plugin_trainer::gen1::addr as tr;

    /// `wPartyMon1Exp` `$D179`, three bytes big-endian. Not in
    /// `plugins/trainer`'s `party` module because nothing there needed it; the
    /// offset is `$D179 - $D16B`.
    const EXP: u16 = 14;

    /// Frames of quiet before a screen is believed. Measured in
    /// `docs/gen1/learning-moves.md`: the shortest prompt box is 23 frames and
    /// a dialogue waiting on its blinking arrow has a longest-identical run of
    /// 42, so a gate this size cannot be fooled by the arrow and cannot miss a
    /// box.
    const STABLE: u32 = 6;
    /// ...and how long to wait for that quiet before acting anyway. A battle
    /// animation never settles, and pressing A into one is harmless.
    const SETTLE_CAP: u32 = 90;
    /// The whole of one event: a battle plus every post-battle prompt.
    const EVENT_FRAMES: u32 = 6000;
    /// Frames to let the battle intro finish before touching the experience.
    /// `BattlePlugin` re-applies its opposition override for up to 40 frames
    /// (`docs/gen1/battle-control.md`), so a write inside that window is a race.
    const PATCH_CLEAR: u32 = 150;

    // -- the plan ----------------------------------------------------------

    pub struct Event {
        pub level: u8,
        pub arm_level: u8,
        pub arm_species: String,
        pub species: String,
        pub evolves_here: bool,
        pub held: Vec<String>,
        pub offered: String,
        /// `free` | `decision` | `already-held`
        pub kind: String,
        /// `learn` | `forget` | `decline` | `nothing`
        pub answer: String,
        pub forget: Option<String>,
        pub forget_slot: Option<usize>,
        pub after: Vec<String>,
        pub why: String,
        pub flags: Vec<String>,
    }

    pub struct Plan {
        pub species: String,
        pub line: Vec<String>,
        pub events: Vec<Event>,
    }

    fn strs(j: Option<&Json>) -> Vec<String> {
        j.and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn load_plan(path: &Path) -> Result<Plan, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let j = json::parse(&text).map_err(|e| format!("plan is not JSON: {}", e.0))?;
        if j.str_field("schema") != Some("gen1-learn-plan/1") {
            return Err(format!(
                "{} is not a gen1-learn-plan/1 document",
                path.display()
            ));
        }
        let mut events = Vec::new();
        for e in j.get("events").and_then(|v| v.as_array()).unwrap_or(&[]) {
            events.push(Event {
                level: e.u64_field("level").unwrap_or(0) as u8,
                arm_level: e.u64_field("arm_level").unwrap_or(0) as u8,
                arm_species: e.str_field("arm_species").unwrap_or_default().into(),
                species: e.str_field("species").unwrap_or_default().into(),
                evolves_here: e
                    .get("evolves_here")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                held: strs(e.get("held")),
                offered: e.str_field("offered").unwrap_or_default().into(),
                kind: e.str_field("kind").unwrap_or_default().into(),
                answer: e.str_field("answer").unwrap_or_default().into(),
                forget: e.str_field("forget").map(str::to_string),
                forget_slot: e.u64_field("forget_slot").map(|n| n as usize),
                after: strs(e.get("after")),
                why: e.str_field("why").unwrap_or_default().into(),
                flags: strs(e.get("flags")),
            });
        }
        if events.is_empty() {
            return Err("the plan has no events".into());
        }
        Ok(Plan {
            species: j.str_field("species").unwrap_or_default().into(),
            line: strs(j.get("line")),
            events,
        })
    }

    // -- arming ------------------------------------------------------------

    /// The name a level's preset is filed under, so one `PresetFile` can hold
    /// the whole plan and `arm` picks the event.
    fn preset_name(e: &Event) -> String {
        format!(
            "lv{}-{}",
            e.level,
            e.offered.to_lowercase().replace(' ', "-")
        )
    }

    /// The preset that puts one level-up within a single kill.
    ///
    /// **A level 2 MAGIKARP, not the CHANSEY the trace scenarios use.** CHANSEY
    /// is worth 182 experience, which crosses two level boundaries at low level
    /// and loses the move belonging to the one it skipped
    /// (`docs/gen1/discoveries.md`). MAGIKARP at level 2 is worth 5, knows
    /// nothing but SPLASH, and cannot cross a boundary at any level in the
    /// game — so the arithmetic is the same for every event instead of being
    /// tuned per event.
    fn preset_for(e: &Event) -> String {
        format!(
            "[{}]\n\
             description = {} at level {} learning {}\n\
             player   = {}\n\
             level    = {}\n\
             moves    = {}\n\
             nickname = LEARNER\n\
             \n\
             wild     = Magikarp\n\
             level    = 2\n",
            preset_name(e),
            e.arm_species,
            e.arm_level,
            e.offered,
            e.arm_species,
            e.arm_level,
            e.held.join(", "),
        )
    }

    // -- the machine -------------------------------------------------------

    /// What this event is aiming the party at, so a snapshot can be scored.
    struct Target {
        level: u8,
        species: u8,
        moves: Vec<u8>,
    }

    struct Arena {
        gb: Gameboy,
        obs: Gen1Observer,
        /// Set for the duration of one event.
        target: Option<Target>,
        best_score: u8,
        best_snap: Option<Vec<u8>>,
    }

    impl Arena {
        fn boot(staged: &Path, rom: &[u8], presets: &str) -> Result<Arena, String> {
            let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
            let ps = PresetFile::parse(presets, "gen1-learn-plan")
                .map_err(|e| format!("the generated presets do not parse: {e}"))?;
            let plugin = BattlePlugin::new(rom.to_vec(), ps)
                .map_err(|e| format!("plugin: {e}"))?;
            gb.add_plugin(Box::new(plugin))
                .map_err(|e| format!("plugin: {e}"))?;
            new_game(&mut gb)?;
            Ok(Arena {
                gb,
                obs: Gen1Observer::new(true),
                target: None,
                best_score: 0,
                best_snap: None,
            })
        }

        /// One frame, with the party scored on the way past.
        ///
        /// **Sampling has to be per frame, not per settle.** The evolution
        /// routine writes the new species into the party and `BattlePlugin`
        /// puts the party back as it found it, and measured on CHARMELEON at
        /// level 36 those two land inside the same 90-frame window — so a
        /// sampler that looks once per settle sees the party before the one
        /// and after the other, and reports an evolution that plainly happened
        /// on screen as not having happened at all.
        fn step(&mut self) {
            self.gb.frame();
            if self.target.is_none() {
                return;
            }
            let m = read_mon(&mut self.gb);
            let live: Vec<u8> = m.moves.iter().copied().filter(|x| *x != 0).collect();
            let t = self.target.as_ref().unwrap();
            let score = u8::from(m.level == t.level)
                + u8::from(m.species == t.species)
                + u8::from(live == t.moves);
            if score > self.best_score {
                self.best_score = score;
                self.best_snap = Some(snapshot_party(&mut self.gb));
            }
        }

        fn screen(&mut self) -> Screen {
            self.gb
                .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                    Screen::read(m)
                })
        }

        /// Run frames until the tilemap has been unchanged for [`STABLE`] of
        /// them, or [`SETTLE_CAP`] frames have passed. Nothing is held down.
        fn settle(&mut self) -> Screen {
            let mut last = 0u64;
            let mut same = 0u32;
            for _ in 0..SETTLE_CAP {
                self.step();
                let h = self
                    .gb
                    .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                        Screen::tilemap_hash(m)
                    });
                if h == last {
                    same += 1;
                    if same >= STABLE {
                        break;
                    }
                } else {
                    last = h;
                    same = 0;
                }
            }
            self.screen()
        }

        /// One press the game will actually see.
        ///
        /// **`HandleMenuInput` delays several frames after accepting a press**,
        /// so a press that arrives inside that delay is swallowed silently —
        /// and a swallowed press on a four-move list means forgetting the wrong
        /// move, which the cartridge will not tell you about. Measured here:
        /// three 12-frame taps moved the cursor two slots. `MENU_GAP` is the
        /// same constant the catch drill and the agent's executor use.
        fn tap(&mut self, key: KeypadKey) {
            self.gb.keydown(key);
            for _ in 0..6 {
                self.step();
            }
            self.gb.keyup(key);
            for _ in 0..super::MENU_GAP {
                self.step();
            }
        }

        fn arm(&mut self, name: &str) -> Result<(), String> {
            self.gb
                .plugin_mut::<BattlePlugin>()
                .ok_or("the plugin vanished")?
                .arm(name)
                .map_err(|e| e.to_string())
        }

        fn await_battle(&mut self, limit: u32) -> Result<(), String> {
            for f in 0..limit {
                self.step();
                if f % super::MENU_GAP == 0 {
                    self.gb.keydown(KeypadKey::A);
                } else if f % super::MENU_GAP == 4 {
                    self.gb.keyup(KeypadKey::A);
                }
                if matches!(self.gb.peek(tr::IS_IN_BATTLE), 1 | 2) {
                    self.gb.keyup(KeypadKey::A);
                    return Ok(());
                }
            }
            Err("no battle started".into())
        }

        /// Put the lead one experience point short of the target level.
        ///
        /// **This is the whole of "trigger it directly".** Arming at
        /// `level - 1` gives the Pokémon exactly the experience for that level,
        /// and climbing to the next one honestly is hundreds of battles at the
        /// levels this plan reaches. Writing the total instead means one kill
        /// of anything at all crosses exactly one boundary — the same number of
        /// frames whether the event is at level 9 or level 65.
        ///
        /// Declared assistance: it is `Gameboy::debug_write`, which
        /// `Gameboy::host_writes()` counts, and it is reported per species.
        fn set_exp(&mut self, want: u32) -> Result<(), String> {
            for _ in 0..8 {
                let at = tr::PARTY_MONS + EXP;
                for (i, b) in [(want >> 16) as u8, (want >> 8) as u8, want as u8]
                    .into_iter()
                    .enumerate()
                {
                    self.gb.debug_write(at + i as u16, b);
                }
                for _ in 0..8 {
                    self.gb.frame();
                }
                let back =
                    (0..3).fold(0u32, |a, i| (a << 8) | self.gb.peek(at + i) as u32);
                if back == want {
                    return Ok(());
                }
            }
            Err("the experience write did not stick — something is \
                 rewriting the party mid-battle"
                .into())
        }
    }

    // -- what is on screen -------------------------------------------------

    /// The screen collapsed to one line, so a sentence Gen 1 wrapped across
    /// rows and padded with spaces is searchable.
    fn oneline(s: &Screen) -> String {
        s.text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Prompt {
        /// `Delete an older move to make room for X?` — a YES/NO box.
        Delete,
        /// `Which move should be forgotten?` — the four-move list.
        Which,
        /// `Abandon learning X?` — a YES/NO box.
        Abandon,
        /// Anything else: dialogue, a battle menu, an animation.
        None,
    }

    /// **Every one of these phrases was read off a traced screen**, not
    /// recalled — see the `trace` subcommand and `docs/gen1/learning-moves.md`.
    ///
    /// **`move to make room` is in here because the YES/NO box scrolls the
    /// question that raised it off the screen.** Gen 1 holds two lines of text,
    /// and `Delete an older move to make room for FIRE SPIN?` is three, so by
    /// the frame the box is actually up the only words left are the tail of the
    /// sentence. Matching on the opening words alone made the box invisible,
    /// the drill fell through to "press A", and A on that box means YES — so a
    /// decline silently became a replacement and the move list opened on a
    /// decision that had already been made. Measured frame by frame with
    /// `GB_LEARN_DRILL_DUMP=1`.
    fn classify(s: &Screen) -> Prompt {
        let one = oneline(s);
        // Order matters: the move list is drawn under the question that opened
        // it, so `Delete an older` is still on screen when the list appears.
        if one.contains("Which move should") {
            Prompt::Which
        } else if one.contains("Abandon learning") {
            Prompt::Abandon
        } else if one.contains("Delete an older")
            || one.contains("learn more than")
            || one.contains("move to make room")
        {
            Prompt::Delete
        } else {
            Prompt::None
        }
    }

    /// Is something still happening that the drill must not walk away from?
    ///
    /// The evolution animation runs **after** the battle is over and takes
    /// about 850 frames (`docs/gen1/learning-moves.md`), so a quiet counter
    /// keyed only on "the battle has ended" gives up in the middle of it and
    /// the party is read back one form early.
    fn busy(s: &Screen) -> bool {
        let one = oneline(s);
        [
            "grew to level",
            "trying to learn",
            "evolving",
            "evolved into",
            "learned",
            "forgot",
            "did not learn",
            "Poof",
        ]
        .iter()
        .any(|n| one.contains(n))
    }

    // -- the corpus --------------------------------------------------------

    /// One button the teacher pressed, with the screen it was pressed at.
    pub struct Row {
        decision: u32,
        frame: u32,
        request: String,
        reply: String,
    }

    /// The observation, in the emulator's own agent snapshot shape.
    ///
    /// It is deliberately a **subset** of what `AgentPlugin` sends — the fields
    /// a move-learn decision can act on, built by the same `observe` code, with
    /// the same names and the same `screen` object. It is not the identical
    /// superset, and the schema string says so rather than implying otherwise.
    fn observation(a: &mut Arena, ev: &Event, frame: u32, decision: u32) -> String {
        let screen = a.screen();
        let game =
            a.gb.with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                // `Gen1Observer::read` is `&self`; the borrow of the machine is
                // what the closure is for.
                let obs = Gen1Observer::new(true);
                obs.read(m)
            });
        let _ = &a.obs;
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", "gen1-learn-drill/1");
        w.num_field("frame", frame as f64);
        w.num_field("decision", decision as f64);
        w.str_field(
            "objective",
            "Answer the move-learning prompt the game is showing.",
        );
        w.obj_field("game");
        w.bool_field("in_game", game.in_game);
        w.arr_field("party");
        for m in &game.party {
            w.begin_obj();
            w.str_field("name", &m.nickname);
            w.num_field("level", m.level as f64);
            w.num_field("hp", m.hp as f64);
            w.num_field("max_hp", m.max_hp as f64);
            w.end_obj();
        }
        w.end_arr();
        w.bool_field(
            "in_battle",
            game.battle != terminalgb_plugin_trainer::gen1::BattleKind::None,
        );
        w.end_obj();
        w.obj_field("screen");
        let trust = screen.trust();
        w.str_field("trust", trust.label());
        w.str_field("kind", screen.kind().name());
        if trust == terminalgb_plugin_agent::observe::Trust::Ok {
            w.str_field("text", &screen.compact_text());
            if let Some(sel) = screen.selected() {
                w.str_field("selected", &sel);
            }
        } else {
            w.str_field("text", "");
        }
        w.bool_field("waiting_for_input", screen.waiting);
        match screen.cursor_row {
            Some(r) => w.num_field("menu_cursor_row", r as f64),
            None => w.null_field("menu_cursor_row"),
        }
        w.end_obj();
        // The event this observation belongs to. Not part of the agent
        // snapshot a live player receives — it is the label, and a training
        // pipeline that wants a strictly-observable input drops it.
        w.obj_field("label");
        w.str_field("species", &ev.species);
        w.num_field("level", ev.level as f64);
        w.str_field("offered", &ev.offered);
        w.str_field("kind", &ev.kind);
        w.str_field("answer", &ev.answer);
        if let Some(f) = &ev.forget {
            w.str_field("forget", f);
        }
        w.str_field("why", &ev.why);
        w.arr_field("held");
        for m in &ev.held {
            w.str_item(m);
        }
        w.end_arr();
        w.arr_field("flags");
        for f in &ev.flags {
            w.str_item(f);
        }
        w.end_arr();
        w.end_obj();
        w.end_obj();
        w.finish()
    }

    /// The teacher's reply, in the shape an agent driver answers with.
    fn reply(goal: &str, say: &str, button: &str, times: u32) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("goal", goal);
        w.str_field("say", say);
        w.arr_field("plan");
        w.begin_obj();
        w.str_field("do", "press");
        w.str_field("button", button);
        w.num_field("times", times as f64);
        w.end_obj();
        w.end_arr();
        w.end_obj();
        w.finish()
    }

    // -- one event ---------------------------------------------------------

    pub struct Outcome {
        /// Frames this event consumed, so the next one carries on counting.
        pub frames: u32,
        pub reached: bool,
        pub answered: bool,
        pub verified: bool,
        /// The species could not be checked. See [`play`]'s verdict block: an
        /// evolution's commit is lost to `BattlePlugin`'s restore, so on an
        /// event where the line evolves the level and the four moves are
        /// checked and the form is not. Counted and reported, never silent.
        pub species_unverified: bool,
        pub note: String,
        pub rows: Vec<Row>,
        pub saw: Vec<&'static str>,
    }

    /// Arm, level up, answer, read the party back.
    #[allow(clippy::too_many_lines)]
    fn play(
        a: &mut Arena,
        tables: &RomTables,
        e: &Event,
        first: u32,
        frame0: u32,
    ) -> Outcome {
        let mut o = Outcome {
            frames: 0,
            reached: false,
            answered: false,
            verified: false,
            species_unverified: false,
            note: String::new(),
            rows: Vec::new(),
            saw: Vec::new(),
        };
        if let Err(err) = a.arm(&preset_name(e)) {
            o.note = format!("could not arm: {err}");
            return o;
        }
        if let Err(err) = a.await_battle(4000) {
            o.note = err;
            return o;
        }
        for _ in 0..PATCH_CLEAR {
            a.gb.frame();
        }
        let growth = match tables.resolve_species(&e.arm_species) {
            Ok(s) => s.growth_rate,
            Err(err) => {
                o.note = format!("{}: {err}", e.arm_species);
                return o;
            }
        };
        let want = experience_for_level(growth, e.level).saturating_sub(1);
        if let Err(err) = a.set_exp(want) {
            o.note = err;
            return o;
        }

        // -- the answer loop ----------------------------------------------
        let want_moves: Vec<u8> = e
            .after
            .iter()
            .map(|n| tables.resolve_move(n).unwrap_or(0))
            .collect();
        let want_species = tables
            .resolve_species(&e.species)
            .map(|s| s.index)
            .unwrap_or(0);
        a.target = Some(Target {
            level: e.level,
            species: want_species,
            moves: want_moves.clone(),
        });
        a.best_score = 0;
        a.best_snap = None;
        let mut decision = first;
        // Monotonic across the whole run, so `frames_to_next` in the flattened
        // dataset is a real cost and not a per-event counter resetting.
        let mut frame = frame0;
        let mut quiet = 0u32;
        let mut done_with_prompt = e.kind != "decision";
        let dump = std::env::var_os("GB_LEARN_DRILL_DUMP").is_some();
        while frame - frame0 < EVENT_FRAMES {
            let screen = a.settle();
            frame += SETTLE_CAP;
            let p = classify(&screen);
            if dump {
                let m = read_mon(&mut a.gb);
                println!(
                    "        f{frame:<5} {p:?} party sp{} lv{} moves {:?} | cursor {:?} selected {:?}",
                    m.species,
                    m.level,
                    m.moves,
                    screen.cursor_row,
                    screen.selected()
                );
                for line in screen.compact_text().lines() {
                    println!("        | {line}");
                }
            }
            let name = match p {
                Prompt::Delete => "delete-an-older",
                Prompt::Which => "which-move",
                Prompt::Abandon => "abandon-learning",
                Prompt::None => "",
            };
            if !name.is_empty() && !o.saw.contains(&name) {
                o.saw.push(name);
            }
            match p {
                Prompt::Delete => {
                    o.reached = true;
                    // `Delete an older move to make room for X?` is a YES/NO
                    // box, but the question types itself out first and the box
                    // is not up yet. **A press while it is still typing
                    // advances the text; it does not answer.** So: no cursor
                    // means keep reading, and only walk the cursor once there
                    // is one.
                    let want = if e.answer == "decline" { "NO" } else { "YES" };
                    match screen.selected() {
                        None => {
                            a.tap(KeypadKey::A);
                        }
                        Some(sel) => {
                            let on_it = sel.trim().eq_ignore_ascii_case(want);
                            let say = if !on_it {
                                format!("The cursor is on {sel}; move it to {want}.")
                            } else if want == "NO" {
                                format!(
                                    "No - {} is worse than everything {} already knows.",
                                    e.offered, e.species
                                )
                            } else {
                                format!(
                                    "Yes - make room for {}: {} is the one to drop.",
                                    e.offered,
                                    e.forget.as_deref().unwrap_or("?")
                                )
                            };
                            o.rows.push(Row {
                                decision,
                                frame,
                                request: observation(a, e, frame, decision),
                                reply: reply(
                                    &goal_of(e),
                                    &say,
                                    if on_it { "a" } else { "down" },
                                    1,
                                ),
                            });
                            decision += 1;
                            a.tap(if on_it { KeypadKey::A } else { KeypadKey::Down });
                        }
                    }
                }
                Prompt::Which => {
                    o.reached = true;
                    let Some(target) = e.forget.clone() else {
                        // A decline should never reach the move list: the
                        // answer is NO at the question that opens it. Landing
                        // here means the NO was not taken, and pressing on
                        // would forget a move nobody chose.
                        o.note = "the move list opened on a decline - the NO \
                                  at `Delete an older move` did not take"
                            .into();
                        return o;
                    };
                    // **Walk the cursor by reading it back, never by counting
                    // presses.** A press `HandleMenuInput` swallows is silent,
                    // and on this list a silent miss forgets a different move —
                    // which the cartridge accepts without complaint. So: look
                    // at what the cursor is on, and press Down only while it is
                    // not the move the rule chose.
                    let mut steps = 0;
                    let mut landed = false;
                    for _ in 0..8 {
                        let sc = a.settle();
                        if classify(&sc) != Prompt::Which {
                            break;
                        }
                        let sel = sc.selected().unwrap_or_default();
                        let on_it = sel.trim().eq_ignore_ascii_case(target.trim());
                        let say = if on_it {
                            format!("{target} is the weakest of the four - {}", e.why)
                        } else {
                            format!("The cursor is on {sel}; {target} is further down.")
                        };
                        o.rows.push(Row {
                            decision,
                            frame,
                            request: observation(a, e, frame, decision),
                            reply: reply(
                                &goal_of(e),
                                &say,
                                if on_it { "a" } else { "down" },
                                1,
                            ),
                        });
                        decision += 1;
                        if on_it {
                            a.tap(KeypadKey::A);
                            landed = true;
                            break;
                        }
                        a.tap(KeypadKey::Down);
                        steps += 1;
                    }
                    if !landed {
                        o.note = format!(
                            "never found {target} in the move list after \
                             {steps} step(s)"
                        );
                        return o;
                    }
                    // Cross-check the plan's slot arithmetic against the
                    // screen: the list is in the order the preset wrote the
                    // moves, so the cursor should have travelled exactly
                    // `forget_slot - 1` rows. A disagreement means one of the
                    // two is wrong and it is worth knowing which.
                    if let Some(slot) = e.forget_slot {
                        if steps + 1 != slot {
                            o.note = format!(
                                "the plan says slot {slot}; the cursor found \
                                 {target} at slot {}",
                                steps + 1
                            );
                        }
                    }
                    o.answered = true;
                    done_with_prompt = true;
                }
                Prompt::Abandon => {
                    o.reached = true;
                    let want = if e.answer == "decline" { "YES" } else { "NO" };
                    let mut landed = false;
                    for _ in 0..12 {
                        let sc = a.settle();
                        if classify(&sc) != Prompt::Abandon {
                            break;
                        }
                        // Answer from what the cursor is actually on, never
                        // from an assumed default — and wait for there to BE
                        // one, because the question types itself out first.
                        let Some(sel) = sc.selected() else {
                            a.tap(KeypadKey::A);
                            continue;
                        };
                        let on_it = sel.trim().eq_ignore_ascii_case(want);
                        let say = if !on_it {
                            format!("The cursor is on {sel}; move it to {want}.")
                        } else if want == "YES" {
                            format!("Yes - walk away from {}. {}", e.offered, e.why)
                        } else {
                            format!("No - {} is worth having.", e.offered)
                        };
                        o.rows.push(Row {
                            decision,
                            frame,
                            request: observation(a, e, frame, decision),
                            reply: reply(
                                &goal_of(e),
                                &say,
                                if on_it { "a" } else { "down" },
                                1,
                            ),
                        });
                        decision += 1;
                        if on_it {
                            a.tap(KeypadKey::A);
                            landed = true;
                            break;
                        }
                        a.tap(KeypadKey::Down);
                    }
                    if !landed {
                        o.note = format!("could not put the cursor on {want}");
                        return o;
                    }
                    if want == "YES" {
                        o.answered = true;
                        done_with_prompt = true;
                    }
                }
                Prompt::None => {
                    a.tap(KeypadKey::A);
                    if a.gb.peek(tr::IS_IN_BATTLE) == 0
                        && done_with_prompt
                        && !busy(&screen)
                    {
                        quiet += 1;
                        if quiet >= 4 {
                            break;
                        }
                    } else {
                        quiet = 0;
                    }
                }
            }
            if a.gb.peek(tr::IS_IN_BATTLE) == 0xFF {
                o.note = "the lead fainted — the opponent is too strong".into();
                return o;
            }
        }
        // `BattlePlugin` restores the party it injected when the battle ends,
        // which would undo the level-up. The best-scoring snapshot taken while
        // the event was running is the post-prompt state; write it back over
        // the restore.
        if let Some(snap) = a.best_snap.take() {
            if std::env::var_os("GB_LEARN_DRILL_NORESTORE").is_none() {
                restore_party(&mut a.gb, &snap);
            }
        }
        a.target = None;
        for _ in 0..30 {
            a.gb.frame();
        }

        if std::env::var_os("GB_LEARN_DRILL_DUMP").is_some() {
            let all: Vec<u8> =
                (0..6).map(|i| a.gb.peek(tr::PARTY_MONS + i * 44)).collect();
            println!(
                "        party species list {:?}  slot species {:?}  count {}",
                (0..7)
                    .map(|i| a.gb.peek(tr::PARTY_SPECIES + i))
                    .collect::<Vec<_>>(),
                all,
                a.gb.peek(tr::PARTY_COUNT)
            );
        }
        // -- the cartridge's own verdict -----------------------------------
        let got: Mon = read_mon(&mut a.gb);
        let got_moves: Vec<u8> = got.moves.iter().copied().filter(|m| *m != 0).collect();
        let mut wrong = Vec::new();
        if got.level != e.level {
            wrong.push(format!("level {} not {}", got.level, e.level));
        }
        // **An evolution cannot be verified through an injected party, and
        // that is measured rather than assumed.** `BattlePlugin` restores the
        // party the moment `wIsInBattle` reads 0 (`step_fighting`, step 3),
        // and Gen 1 clears that flag on the way back to the overworld — which
        // is *after* the evolution has played its animation and *before* it
        // has written the new species into the party. Traced frame by frame on
        // CHARMELEON at level 36: the screen prints `LEARNER evolved into
        // CHARIZARD!`, `wPartyMon1Species` reads 178 on every frame from the
        // level-up to the restore, and with the drill's own write-back
        // disabled the party ends EMPTY. So the form is not checked here; the
        // level and the four moves are, and the gap is reported.
        //
        // No decision on either drilled line falls on an evolution level, so
        // nothing this drill measures rests on it.
        if e.evolves_here {
            o.species_unverified = true;
        } else if got.species != want_species {
            wrong.push(format!(
                "species {} not {} ({})",
                got.species, want_species, e.species
            ));
        }
        if got_moves != want_moves {
            let name = |m: &u8| tables.move_name(*m).unwrap_or_else(|| format!("#{m}"));
            wrong.push(format!(
                "moves [{}] not [{}]",
                got_moves.iter().map(name).collect::<Vec<_>>().join(", "),
                e.after.join(", ")
            ));
        }
        o.frames = frame - frame0;
        o.verified = wrong.is_empty();
        if !o.verified {
            o.note = wrong.join("; ");
        }
        if e.kind != "decision" {
            o.answered = true;
        }
        o
    }

    fn goal_of(e: &Event) -> String {
        match e.answer.as_str() {
            "decline" => format!("Decline {} — keep all four moves.", e.offered),
            "forget" => format!(
                "Learn {}, forgetting {}.",
                e.offered,
                e.forget.as_deref().unwrap_or("?")
            ),
            _ => format!("Learn {}.", e.offered),
        }
    }

    // -- the run -----------------------------------------------------------

    pub fn run(
        rom: &Path,
        plan_path: &Path,
        out: &Path,
        only: Option<u8>,
    ) -> Result<(), String> {
        let plan = load_plan(plan_path)?;
        let staged = stage(rom)?;
        let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
        let tables = RomTables::locate(bytes.clone()).map_err(|e| format!("{e}"))?;
        let dir = out.join(plan.species.to_lowercase());
        std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;

        println!(
            "== the {} line: {} ==",
            plan.species,
            plan.line.join(" -> ")
        );
        println!("   plan {}", plan_path.display());
        println!("   corpus {}", dir.display());
        println!();

        let presets: String = plan.events.iter().map(preset_for).collect();
        let mut a = Arena::boot(&staged, &bytes, &presets)?;
        let mut rows: Vec<Row> = Vec::new();
        let mut done = 0usize;
        let mut asked = 0usize;
        let mut unchecked = 0usize;
        let mut clock = 0u32;
        let mut lines: Vec<String> = Vec::new();
        let mut skipped = 0usize;

        for e in &plan.events {
            if only.is_some() && only != Some(e.level) {
                skipped += 1;
                continue;
            }
            let o = play(&mut a, &tables, e, rows.len() as u32 + 1, clock);
            clock += o.frames.max(1);
            let verdict = if o.verified && o.species_unverified {
                "ok (form unchecked)"
            } else if o.verified {
                "ok"
            } else if o.note.is_empty() {
                "UNVERIFIED"
            } else {
                "FAILED"
            };
            let mut prompts = if o.saw.is_empty() {
                "no prompt".to_string()
            } else {
                o.saw.join(" -> ")
            };
            if e.evolves_here {
                prompts = format!("evolves, {prompts}");
            }
            if o.species_unverified {
                unchecked += 1;
            }
            println!(
                "   L{:<3} {:<11} {:<13} {:<12} {:<24} {:>3} row(s)  {}",
                e.level,
                e.arm_species,
                e.offered,
                e.kind,
                prompts,
                o.rows.len(),
                verdict
            );
            if !o.note.is_empty() {
                println!("        {}", o.note);
            }
            lines.push(format!(
                "{},{},{},{},{},{},{},{},{}",
                plan.species,
                e.level,
                e.arm_species,
                e.offered,
                e.kind,
                e.answer,
                o.reached,
                o.verified,
                o.rows.len()
            ));
            asked += 1;
            if o.verified {
                done += 1;
            }
            rows.extend(o.rows);
        }

        // -- write the corpus ---------------------------------------------
        let mut ndjson = String::new();
        for r in &rows {
            let mut w = json::Writer::new();
            w.begin_obj();
            w.str_field("ev", "decision");
            w.num_field("decision", r.decision as f64);
            w.num_field("frame", r.frame as f64);
            w.num_field("ms", 0.0);
            w.str_field("request", &r.request);
            w.str_field("reply", &r.reply);
            w.obj_field("usage");
            w.num_field("input_tokens", 0.0);
            w.num_field("output_tokens", 0.0);
            w.num_field("usd", 0.0);
            w.end_obj();
            w.end_obj();
            ndjson.push_str(&w.finish());
            ndjson.push('\n');
        }
        let corpus = dir.join("agent.ndjson");
        std::fs::write(&corpus, ndjson)
            .map_err(|e| format!("{}: {e}", corpus.display()))?;
        let index = dir.join("events.csv");
        std::fs::write(
            &index,
            format!(
                "species,level,arm_species,offered,kind,answer,reached,verified,rows\n{}\n",
                lines.join("\n")
            ),
        )
        .map_err(|e| format!("{}: {e}", index.display()))?;

        println!();
        println!(
            "   {done} of {asked} event(s) played out and verified against \
             the cartridge{}",
            if skipped > 0 {
                format!("; {skipped} event(s) skipped by --level")
            } else {
                String::new()
            }
        );
        if unchecked > 0 {
            println!(
                "   {unchecked} event(s) evolve, so the form is not checked - \
                 the restore lands between the animation and the commit"
            );
        }
        println!("   {} corpus row(s) -> {}", rows.len(), corpus.display());
        println!("   host writes this session: {}", a.gb.host_writes());
        if done != asked {
            return Err("not every event was played out and verified".into());
        }
        Ok(())
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut rom: Option<PathBuf> = None;
    let mut plan: Option<PathBuf> = None;
    let mut out = PathBuf::from("target/learn-drill");
    let mut only: Option<u8> = None;
    let mut cmd = String::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--rom" => {
                i += 1;
                rom = args.get(i).map(PathBuf::from);
            }
            "--plan" => {
                i += 1;
                plan = args.get(i).map(PathBuf::from);
            }
            "--out" => {
                i += 1;
                out = args.get(i).map(PathBuf::from).unwrap_or(out);
            }
            // One level-up, by the level it lands on — the way to iterate on a
            // single prompt without replaying the whole line.
            "--level" => {
                i += 1;
                only = args.get(i).and_then(|s| s.parse().ok());
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
        "drill" => match plan {
            Some(p) => drill::run(&rom, &p, &out, only),
            None => Err("drill needs --plan <plan.json>; produce one with \
                         `learn_table.py --drill-plan <species>`"
                .into()),
        },
        _ => Err("usage: gen1_learn_drill <trace|cancel|moveset|drill> \
                  --rom <blue.gb> [--plan p.json] [--out DIR] [--level N]"
            .into()),
    };
    if let Err(e) = r {
        eprintln!("gen1_learn_drill: {e}");
        std::process::exit(1);
    }
}

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

/// Stage the cartridge somewhere this process alone owns.
///
/// **Per process, not per program.** The MBC writes its `.sav` beside whatever
/// path it is handed, and the drill deletes that file on the way in so every
/// line starts from a new game — so two drills staging to one fixed directory
/// share a save file and each one's first act is to delete the other's. It is
/// harmless today only because the drill never reads a save; a fixed path is
/// still a shared mutable file between processes, and 140 species lines is
/// exactly the run somebody parallelises.
fn stage(rom: &Path, out: &Path) -> Result<PathBuf, String> {
    let dir = out.join(format!("stage-{}", std::process::id()));
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
        // A machine with no APU is not a quiet Game Boy, it is one with the sound
        // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
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
    let staged = stage(rom, Path::new("target/gen1-learn-drill"))?;
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
    let staged = stage(rom, Path::new("target/gen1-learn-drill"))?;
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
    let staged = stage(rom, Path::new("target/gen1-learn-drill"))?;
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
    use terminalgb_plugin_trainer::rom::BattleTables;

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
        /// The **internal index** of `arm_species`, and the only unambiguous
        /// way to name it. Two species decode to the same printable name once
        /// the gender glyphs are dropped (`NIDORAN`), so a preset built from
        /// the name arms whichever the cartridge lists first — measured, the
        /// female's plan armed a male and learned its moves instead.
        /// `0` for a plan written before the field existed, which falls back
        /// to the name.
        pub arm_index: u8,
        pub species: String,
        /// The internal index of `species`, on the same footing as
        /// [`Event::arm_index`].
        pub species_index: u8,
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
                arm_index: e.u64_field("arm_index").unwrap_or(0) as u8,
                species: e.str_field("species").unwrap_or_default().into(),
                species_index: e.u64_field("species_index").unwrap_or(0) as u8,
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
    /// (<https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#crossing-two-levels-loses-the-move-in-between>).
    /// MAGIKARP at level 2 is worth 5, knows
    /// nothing but SPLASH, and cannot cross a boundary at any level in the
    /// game — so the arithmetic is the same for every event instead of being
    /// tuned per event.
    ///
    /// **`can_fight` decides which of two shapes it gets, and the second one
    /// exists because five events hold no move that can hurt anything.**
    /// JIGGLYPUFF at 9 knows only SING, KAKUNA and METAPOD at 12 only HARDEN,
    /// MAGIKARP at 15 only SPLASH and ABRA at 16 only TELEPORT — measured over
    /// all 140 lines, and no choice of *move* recovers any of them. So the
    /// opponent ends the battle instead: the same level 2 MAGIKARP, holding
    /// SELFDESTRUCT, which knocks itself out and awards its 5 experience
    /// exactly as a kill would.
    ///
    /// **It is a trainer battle, and that is the whole of why ABRA works.**
    /// TELEPORT ends a *wild* battle — the ABRA escapes and earns nothing —
    /// and `TryRunningFromBattle` refuses a trainer battle outright, so on a
    /// trainer's MAGIKARP the TELEPORT fails and the turn passes. SPLASH and
    /// HARDEN do nothing either way; SING costs a few turns of sleep and no
    /// more. Gen 1 has no trainer experience bonus (that arrives in Gen 2), so
    /// the 5 experience and the "cannot cross a boundary" property are
    /// unchanged.
    fn preset_for(e: &Event, can_fight: bool) -> String {
        let opposition = if can_fight {
            "wild     = Magikarp\n\
             level    = 2\n"
                .to_string()
        } else {
            "trainer  = Youngster\n\
             roster   = 1\n\
             enemy    = Magikarp\n\
             level    = 2\n\
             moves    = Selfdestruct\n"
                .to_string()
        };
        format!(
            "[{}]\n\
             description = {} at level {} learning {}\n\
             player   = {}\n\
             level    = {}\n\
             moves    = {}\n\
             nickname = LEARNER\n\
             \n\
             {}",
            preset_name(e),
            e.arm_species,
            e.arm_level,
            e.offered,
            arm_species(e),
            e.arm_level,
            e.held.join(", "),
            opposition,
        )
    }

    /// How a preset should name this event's Pokémon.
    ///
    /// The internal index when the plan carries one, because a printable name
    /// is not unique — see [`Event::arm_index`]. The name is the fallback for
    /// a plan written before the field existed.
    fn arm_species(e: &Event) -> String {
        if e.arm_index == 0 {
            e.arm_species.clone()
        } else {
            e.arm_index.to_string()
        }
    }

    /// Can this event's Pokémon knock anything out with what it holds?
    ///
    /// A move's power byte, read out of the cartridge. **Zero means a status
    /// move and nothing else** — the seven placeholder-power moves
    /// (`GUILLOTINE`, `SEISMIC TOSS`, `DRAGON RAGE`, ...) carry a power of 1,
    /// which is the same distinction `learn_table.py`'s rule 0 makes and the
    /// reason it is a power test rather than a damage estimate.
    ///
    /// `GB_LEARN_DRILL_NO_EXPLODER=1` answers "yes" for everything, which puts
    /// the plain wild MAGIKARP back in front of the five that cannot fight —
    /// the A/B that shows the exploding trainer is doing the work rather than
    /// being decoration.
    fn can_fight(e: &Event, tables: &RomTables, moves: &BattleTables) -> bool {
        if std::env::var_os("GB_LEARN_DRILL_NO_EXPLODER").is_some() {
            return true;
        }
        e.held.iter().any(|m| {
            tables
                .resolve_move(m)
                .map(|id| moves.power(id) > 0)
                .unwrap_or(false)
        })
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
            // Same rule as above: a machine with no APU is not a quiet Game Boy.
            gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
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

        /// Everything the battle policy decides from, read through the
        /// observer this arena already carries.
        ///
        /// Destructured rather than `self.obs.read(...)` because
        /// [`Gameboy::with_machine`] borrows `self.gb` mutably and the
        /// observer is a field of the same struct; splitting the borrow is
        /// what lets the table cache live for the whole run instead of being
        /// rebuilt 1.6 KiB at a time on every look.
        fn state(&mut self) -> terminalgb_plugin_agent::observe::GameState {
            let Arena { gb, obs, .. } = self;
            gb.with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                obs.read(m)
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

        /// Is the lead on zero HP? `wPartyMon1HP` is big-endian, like every
        /// multi-byte field in a Gen 1 party struct.
        fn lead_fainted(&mut self) -> bool {
            let at = tr::PARTY_MONS + terminalgb_plugin_trainer::gen1::party::HP;
            self.gb.peek(tr::PARTY_COUNT) > 0
                && self.gb.peek(at) == 0
                && self.gb.peek(at + 1) == 0
        }

        fn arm(&mut self, name: &str) -> Result<(), String> {
            self.gb
                .plugin_mut::<BattlePlugin>()
                .ok_or("the plugin vanished")?
                .arm(name)
                .map_err(|e| e.to_string())
        }

        /// One press of a battle menu, when a battle menu is what is on screen.
        ///
        /// **This exists because pressing A took move slot 1.** A on the main
        /// battle menu is FIGHT and A on the move list is *the first move*, so
        /// a line whose slot 1 is SPLASH, HARDEN or SING could not kill the
        /// level 2 MAGIKARP that supplies the experience — and a line whose
        /// slot 1 is TELEPORT ran away from it. Fourteen species lines lost
        /// every one of their decisions to that, measured
        /// (`docs/agent/move-learning-result.md`).
        ///
        /// **The choice is not made here.** It is
        /// [`terminalgb_plugin_agent::battle::decide`], the one battle policy
        /// this repository has, reading the move table and the type chart out
        /// of the cartridge and both sides out of the two battle structs. What
        /// is here is the *menu*: turning that slot into a keypress without
        /// going out of phase, which is the same split
        /// `examples/pewter_chain.rs::fight` makes.
        ///
        /// Returns `None` when this is not a battle menu, and the caller falls
        /// back to pressing A — which is right for every text box, every
        /// animation and the main menu's own FIGHT.
        fn battle_key(&mut self, screen: &Screen, held: &[String]) -> Option<KeypadKey> {
            use terminalgb_plugin_agent::battle::{self, Action};
            if self.gb.peek(tr::IS_IN_BATTLE) == 0 {
                return None;
            }
            // The party list, reached only by a mispress. B is the one button
            // that can only ever back out of a menu — the same answer
            // `pewter_chain::fight` gives it.
            if oneline(screen).contains("CHOOSE A POK") {
                return Some(KeypadKey::B);
            }
            let sel = screen.selected()?;
            let sel = sel.trim().to_string();
            // The main menu is FIGHT / PKMN over ITEM / RUN. A on FIGHT is
            // already what this drill wants, so it falls through to the
            // caller; anywhere else on that menu is a mispress and walks back,
            // because A on RUN ends the battle and A on PKMN opens a list.
            if sel.eq_ignore_ascii_case("FIGHT") {
                return None;
            }
            if sel.eq_ignore_ascii_case("PKMN") {
                return Some(KeypadKey::Left);
            }
            if sel.eq_ignore_ascii_case("ITEM") || sel.eq_ignore_ascii_case("RUN") {
                return Some(KeypadKey::Up);
            }
            // The move list, identified by the cursor sitting on one of the
            // four moves this event's plan says the Pokémon holds — allowing
            // for the truncation [`menu_row_is`] describes.
            if !held.iter().any(|h| menu_row_is(h, &sel)) {
                return None;
            }
            let mut view = self.state().arena?;
            // **A move that knocks its own user out is not a move this rig may
            // make.** `decide` is right to weigh SELFDESTRUCT for a player with
            // a bench — it ends the battle, which is usually the point — and it
            // is exactly wrong here: the level-up is checked for a Pokémon that
            // survived, so an exploding VOLTORB blacks the party out and every
            // event after it. Measured: eight events across the seven
            // self-destructing lines fainted. `EXPLODE_EFFECT` is 7
            // (`constants/move_effect_constants.asm`), confirmed against this
            // cartridge — SELFDESTRUCT and EXPLOSION are the only two moves
            // carrying it.
            const EXPLODE_EFFECT: u8 = 7;
            view.moves.retain(|m| m.effect != EXPLODE_EFFECT);
            if view.moves.is_empty() {
                return None;
            }
            // **The drill is not a player: it must never run, and it must
            // never spend the turn on a status move.** Running is not a
            // level-up, and neither is GROWL — the policy is right to weigh
            // both for somebody playing the game and neither is available to a
            // rig whose whole purpose is to cross one experience boundary. So
            // the policy is asked for the fight it would pick, and anything
            // else falls back to the most damage on offer.
            let intent = battle::Intent {
                may_fight_wild: true,
                // Above every reachable level, so `why::TRAINED` can never fire.
                train_to: u8::MAX,
            };
            let slot = match battle::decide(&view, &intent) {
                // `Move::slot` is the cartridge's slot and `view.moves` has had
                // the self-destructing moves taken out of it, so the two are no
                // longer the same index — look the move up by its own slot.
                Action::Fight { slot, .. }
                    if view
                        .moves
                        .iter()
                        .any(|m| m.slot == slot && m.damaging() && m.usable()) =>
                {
                    slot
                }
                _ => view
                    .moves
                    .iter()
                    .filter(|m| m.usable() && m.damaging())
                    .max_by_key(|m| m.expected())
                    .or_else(|| view.moves.iter().find(|m| m.usable()))
                    .map_or(0, |m| m.slot),
            };
            // **The battle move list's cursor is 1-based**, and that is the
            // cartridge saying so: `MoveSelectionMenu` (`engine/battle/core.asm`)
            // opens `ld a, [wPlayerMoveListIndex] / inc a / ld [hli], a ;
            // wCurrentMenuItem`. Measured here too, on RHYDON at level 64: the
            // cursor on the first move reads **1**, where the main menu on
            // FIGHT reads 0 and the *forget* list on its first move also reads
            // **0** — the two move lists disagree, and only this one is the one
            // `MoveSelectionMenu` builds. Getting it wrong is silent: the index
            // already equals the target and A fires on the wrong move
            // (<https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#the-battle-move-menu-cursor-is-1-based>).
            let at = (self.gb.peek(tr::CURRENT_MENU_ITEM) as usize).saturating_sub(1);
            Some(match at.cmp(&slot) {
                std::cmp::Ordering::Less => KeypadKey::Down,
                std::cmp::Ordering::Greater => KeypadKey::Up,
                std::cmp::Ordering::Equal => KeypadKey::A,
            })
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

    /// Is the menu row reading `sel` the item called `name`?
    ///
    /// **A move list truncates, and this is where that is absorbed.**
    /// [`Screen::selected`] ends an item at a blank run followed by a column
    /// where another row of the same menu also starts a word — which is
    /// exactly right for `FIGHT`/`PKMN` over `ITEM`/`RUN`, and exactly wrong
    /// for a list of aligned two-word move names: with `HORN ATTACK` over
    /// `TAIL WHIP`, `WHIP` begins in the column `ATTACK` does, so the first row
    /// reads back as `HORN`. So a row matches its name outright, or matches
    /// the name's first word — and nothing else, which is the only truncation
    /// that rule can produce.
    fn menu_row_is(name: &str, sel: &str) -> bool {
        let (name, sel) = (name.trim(), sel.trim());
        name.eq_ignore_ascii_case(sel)
            || (!sel.is_empty()
                && name
                    .split_whitespace()
                    .next()
                    .is_some_and(|w| w.eq_ignore_ascii_case(sel)))
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
        /// A save state taken **at the prompt, before the answer is given**.
        ///
        /// This is the whole join to the A/B test rig: the corpus records what
        /// happened, and a rig needs to *re-run* the same prompt with a
        /// different player, which means a machine standing where the question
        /// was asked. See `docs/agent/test-rig.md` §5a. `None` for an event
        /// that is not a decision, or one whose prompt was never reached.
        pub prompt_state: Option<Vec<u8>>,
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
            prompt_state: None,
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
        let growth = match tables.resolve_species(&arm_species(e)) {
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
        let want_species = if e.species_index != 0 {
            e.species_index
        } else {
            tables
                .resolve_species(&e.species)
                .map(|s| s.index)
                .unwrap_or(0)
        };
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
                    "        f{frame:<5} {p:?} party sp{} lv{} moves {:?} | battle {:02X} \
                     cursor {:?} menu_item {} selected {:?}",
                    m.species,
                    m.level,
                    m.moves,
                    a.gb.peek(tr::IS_IN_BATTLE),
                    screen.cursor_row,
                    a.gb.peek(tr::CURRENT_MENU_ITEM),
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
                            // **The first frame the box is answerable is the
                            // prompt.** Before the cursor exists the question
                            // is still typing itself out and a player dropped
                            // in there would press A into the text; after the
                            // first press the answer has been given. This is
                            // the only moment a foreign player can be handed
                            // the same question.
                            if o.prompt_state.is_none() && e.kind == "decision" {
                                o.prompt_state = Some(a.gb.save_state());
                            }
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
                    // which the cartridge accepts without complaint.
                    //
                    // **What is read back is `wCurrentMenuItem`, not the text
                    // the cursor is on, and that is a correction.**
                    // `Screen::selected` ends an item at a blank run followed
                    // by a column where another row of the same menu starts a
                    // word — which is exactly right for `FIGHT`/`PKMN` over
                    // `ITEM`/`RUN` and exactly wrong for a list of aligned
                    // two-word move names: with `HORN ATTACK` over `TAIL WHIP`,
                    // `WHIP` begins where `ATTACK` does, so the first row reads
                    // back as `HORN`. Measured on RHYDON at level 64; six lines
                    // failed with *"never found TAIL WHIP in the move list"* for
                    // no other reason. The index cannot be truncated, and it is
                    // the same reading `examples/pewter_chain.rs::fight` takes.
                    //
                    // **This list's cursor is 0-based, and the BATTLE move
                    // list's is 1-based.** That is a correction to the note
                    // this repository already carries: the 1-based index
                    // belongs to `MoveSelectionMenu`, which opens
                    // `ld a, [wPlayerMoveListIndex] / inc a / ld [hli], a`, and
                    // the forget list is not that menu. Measured on RHYDON at
                    // level 64 with the cursor on the first move of each:
                    // battle list **1**, forget list **0**, main menu **0**.
                    // Subtracting one here walked to slot 2 and forgot FURY
                    // ATTACK where the rule chose TAIL WHIP — silently, because
                    // the cartridge accepts whatever the cursor is on.
                    let Some(want_row) =
                        e.forget_slot.map(|s| s.saturating_sub(1)).or_else(|| {
                            e.held.iter().position(|m| {
                                m.trim().eq_ignore_ascii_case(target.trim())
                            })
                        })
                    else {
                        o.note = format!(
                            "the plan says to forget {target}, which is not one \
                             of the four moves it says are held"
                        );
                        return o;
                    };
                    let mut landed = false;
                    let mut walked = 0;
                    for _ in 0..8 {
                        let sc = a.settle();
                        if classify(&sc) != Prompt::Which {
                            break;
                        }
                        let at = a.gb.peek(tr::CURRENT_MENU_ITEM) as usize;
                        let sel = sc.selected().unwrap_or_default();
                        let on_it = at == want_row;
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
                            // The screen is the cross-check on the index, and
                            // it tolerates exactly the truncation described
                            // above and nothing else: the row must be the move
                            // the plan named, or its first word.
                            if !menu_row_is(&target, &sel) {
                                o.note = format!(
                                    "the plan says {target} is slot {}; the \
                                     screen has {sel} there",
                                    want_row + 1
                                );
                                return o;
                            }
                            a.tap(KeypadKey::A);
                            landed = true;
                            break;
                        }
                        a.tap(if at < want_row {
                            KeypadKey::Down
                        } else {
                            KeypadKey::Up
                        });
                        walked += 1;
                    }
                    if !landed {
                        o.note = format!(
                            "never reached slot {} in the move list after \
                             {walked} step(s)",
                            want_row + 1
                        );
                        return o;
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
                    // A battle menu gets the policy's move; everything else —
                    // text boxes, animations, the main menu's own FIGHT — gets
                    // the A it always got.
                    let key = a.battle_key(&screen, &e.held).unwrap_or(KeypadKey::A);
                    a.tap(key);
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
            // **`wIsInBattle == $FF` IS NOT A BLACKOUT ON ITS OWN.** The
            // repository already records that `$FF` means blacked out rather
            // than fighting — and measured here, the cartridge also passes
            // through it on the way *out* of a battle it won: CHARMELEON at
            // level 8 kills the MAGIKARP, grows to level 9, and the byte reads
            // `$FF` for an instant while the stat screen is up. The drill read
            // that as a faint and threw away an event it had just completed,
            // then every event after it. The party is the evidence: a blackout
            // is a lead on zero HP, which is also what
            // `longplay::Progress::wiped` uses and for the same reason.
            if a.gb.peek(tr::IS_IN_BATTLE) == 0xFF && a.lead_fainted() {
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

    /// A `gbtestprompt/1` file, the right answer to one prompt.
    ///
    /// Only `correct_moves` is load-bearing to the rig; the rest is provenance,
    /// carried so a row of its report can be read without opening these notes.
    fn expectation(e: &Event, tables: &RomTables, want: &[u8]) -> String {
        let mv = |n: &str| tables.resolve_move(n).unwrap_or(0);
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", "gbtestprompt/1");
        w.num_field(
            "species",
            if e.arm_index != 0 {
                e.arm_index
            } else {
                tables
                    .resolve_species(&e.arm_species)
                    .map(|s| s.index)
                    .unwrap_or(0)
            } as f64,
        );
        w.str_field("species_name", &e.arm_species);
        w.num_field("level", e.level as f64);
        w.arr_field("held");
        for n in &e.held {
            w.num_item(mv(n) as f64);
        }
        w.end_arr();
        w.num_field("offered", mv(&e.offered) as f64);
        w.str_field("offered_name", &e.offered);
        w.arr_field("correct_moves");
        for m in want {
            w.num_item(*m as f64);
        }
        w.end_arr();
        w.str_field("answer", &e.answer);
        w.str_field("why", &e.why);
        w.end_obj();
        w.finish()
    }

    pub fn run(
        rom: &Path,
        plan_path: &Path,
        out: &Path,
        only: Option<u8>,
        pool_dir: Option<&Path>,
    ) -> Result<(), String> {
        let plan = load_plan(plan_path)?;
        let staged = stage(rom, out)?;
        let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
        let tables = RomTables::locate(bytes.clone()).map_err(|e| format!("{e}"))?;
        // The move table, read out of the staged cartridge by bank and address
        // rather than transcribed, and confirmed against the type chart's own
        // opening rows before anything is believed of it.
        let moves = BattleTables::read(|bank, a| {
            *bytes
                .get(bank * 0x4000 + (a as usize & 0x3FFF))
                .unwrap_or(&0xFF)
        });
        if !moves.verified() {
            return Err("the move table and type chart did not read back as \
                        this cartridge's — refusing to choose a battle move \
                        from them"
                .into());
        }
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

        let presets: String = plan
            .events
            .iter()
            .map(|e| preset_for(e, can_fight(e, &tables, &moves)))
            .collect();
        let mut a = Arena::boot(&staged, &bytes, &presets)?;
        let mut rows: Vec<Row> = Vec::new();
        let mut done = 0usize;
        let mut asked = 0usize;
        let mut unchecked = 0usize;
        let mut clock = 0u32;
        let mut lines: Vec<String> = Vec::new();
        let mut skipped = 0usize;
        // The A/B rig's entry pool: a save state at each prompt and the right
        // answer beside it. **Only a verified decision earns a place.** A
        // prompt the drill could not itself answer correctly is a prompt whose
        // "right answer" is a claim rather than a measurement, and an entry
        // pool of those would score every player against a guess.
        if let Some(d) = pool_dir {
            std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
        }
        // **Number on from what is already there.** Both lines are drilled into
        // one pool — that pair is the whole point of the rig's two subjects —
        // and a run that restarted at 00000 would silently overwrite the other
        // species. `NNNNN` is the contract's own naming; the rig files each
        // state under a subject by reading its lead, not by its name.
        let mut pooled = pool_dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|it| {
                it.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path().extension().map(|x| x == "gbss").unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0);
        let first_index = pooled;

        for e in &plan.events {
            if only.is_some() && only != Some(e.level) {
                skipped += 1;
                continue;
            }
            let o = play(&mut a, &tables, e, rows.len() as u32 + 1, clock);
            clock += o.frames.max(1);
            // **One bad event must not cost the rest of the line.** A drill
            // that gives up mid-sequence leaves the machine wherever it stood
            // — a stat screen, a text box, a battle nobody finished — and
            // `arm` only ever lands when the *overworld loop* polls
            // `wCurOpponent`, so every event after it reported "no battle
            // started" and the whole line went with it. Measured on
            // CHARMELEON: one lost event, then six unreachable ones. A fresh
            // machine costs about a second and is the one recovery that cannot
            // itself get stuck, and every event arms its own party from the
            // preset anyway, so nothing is carried between them.
            //
            // A *verified* event can leave the machine wedged too, and the
            // same test catches both: `wIsInBattle` back at 0 is the overworld
            // loop running, which is the one condition `arm` needs.
            if !o.verified || a.gb.peek(tr::IS_IN_BATTLE) != 0 {
                a = Arena::boot(&staged, &bytes, &presets)?;
            }
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
            if let (Some(d), Some(state)) = (pool_dir, o.prompt_state.as_ref()) {
                if o.verified {
                    let want: Vec<u8> = e
                        .after
                        .iter()
                        .map(|n| tables.resolve_move(n).unwrap_or(0))
                        .collect();
                    let stem = format!("{:05}", pooled);
                    std::fs::write(d.join(format!("{stem}.gbss")), state)
                        .map_err(|err| format!("{}: {err}", d.display()))?;
                    std::fs::write(
                        d.join(format!("{stem}.expect.json")),
                        expectation(e, &tables, &want),
                    )
                    .map_err(|err| format!("{}: {err}", d.display()))?;
                    pooled += 1;
                }
            }
            rows.extend(o.rows);
        }
        if let Some(d) = pool_dir {
            println!();
            println!(
                "   {} prompt(s) written to {} as a gbtestprompt/1 pool ({pooled} in it now)",
                pooled - first_index,
                d.display()
            );
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
        // **Say what was checked, not just how many passed.** An evolving
        // event has its level and its four moves verified and its form not, so
        // a bare "7 of 7 verified" is a headline somebody could quote as more
        // than it is.
        println!(
            "   {done} of {asked} event(s) played out; level and moves verified \
             against the cartridge{}",
            if skipped > 0 {
                format!("; {skipped} skipped by --level")
            } else {
                String::new()
            }
        );
        if unchecked > 0 {
            println!(
                "   of those, {unchecked} evolve, so the FORM is not checked - \
                 the plugin's restore lands between the evolution animation \
                 and its commit"
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
    let mut prompts: Option<PathBuf> = None;
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
            // The A/B rig's entry pool: one save state per verified decision,
            // taken with the question on the screen, and the right answer in a
            // `gbtestprompt/1` file beside it. See `docs/agent/test-rig.md`
            // §5a — this is the whole join between the drill and the rig.
            "--prompts" => {
                i += 1;
                prompts = args.get(i).map(PathBuf::from);
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
            Some(p) => drill::run(&rom, &p, &out, only, prompts.as_deref()),
            None => Err("drill needs --plan <plan.json>; produce one with \
                         `learn_table.py --drill-plan <species>`"
                .into()),
        },
        _ => Err("usage: gen1_learn_drill <trace|cancel|moveset|drill> \
                  --rom <blue.gb> [--plan p.json] [--out DIR] [--level N] \
                  [--prompts DIR]"
            .into()),
    };
    if let Err(e) = r {
        eprintln!("gen1_learn_drill: {e}");
        std::process::exit(1);
    }
}

//! The catching drill — mechanics measured en masse, and the two facts a
//! policy is not allowed to guess.
//!
//! ```text
//! cargo run --release --features image,plugin-battle-control,plugin-trainer,plugin-agent \
//!     --example gen1_catch_drill -- <subcommand> --rom <blue.gb> [options]
//! ```
//!
//! # What it measures, and why each is measured rather than recalled
//!
//! * **`bag`** — what the bag holds at a given save state. This is the *gate*:
//!   catching is a capability that starts at zero, and a policy that answers
//!   "no" is a different thing from one that is never asked. Point it at the
//!   Pewter chain's own carried-forward states and it says which link opens
//!   the capability.
//! * **`kinds`** — a wild battle and a trainer battle synthesised from one
//!   save with nothing but the preset's `wild =`/`trainer =` line different,
//!   their tilemaps diffed, and a **screen-only** discriminator scored against
//!   `wIsInBattle`. A discriminator nobody measured is a guess.
//! * **`drill`** — throw balls, in both kinds of battle, and compare what
//!   happened against what [`terminalgb_plugin_trainer::capture`] predicted.
//! * **`policy` / `policy-empty`** — the same encounters put to
//!   [`terminalgb_plugin_agent::catch::decide`], with and without a ball in the
//!   bag, and whatever it recommends carried out. The gate, the discriminator,
//!   the target list and the odds all meet here.
//! * **`gift`** — does talking to Oak again open the capability? Driven, so it
//!   can come back "no".
//!
//! # Declared assistance
//!
//! `drill` and `kinds` **write the bag** (`wNumBagItems`/`wBagItems`) so a run
//! can be set up without walking to a shop, and `drill` may write the enemy's
//! HP and status so the odds are sampled across their whole range rather than
//! only where a fight happens to leave them. Both are stated in the report.
//! This is an instrument, not an agent: the agent is refused `WRITE_MEMORY`
//! and nothing here is on its path.
//!
//! Everything but `policy` measures the *game*. `policy` measures the policy
//! against it, and says which of the two it is reporting.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
use terminalgb::KeypadKey;
use terminalgb_plugin_agent::battlekind;
use terminalgb_plugin_battle_control::{preset::PresetFile, BattlePlugin};
use terminalgb_plugin_trainer::capture::{self, Ball};
use terminalgb_plugin_trainer::gen1::{addr as tr, Status};

/// `wTileMap` — the 20x18 screen buffer the game writes text into.
use terminalgb_plugin_battle_control::wram::addr::TILE_MAP;

const PRESETS: &str = include_str!("gen1_catch_drill.preset");

/// One screen, sampled: the frame it was taken on, the raw tilemap, and the
/// text it decodes to.
type Sample = (u32, Vec<u8>, String);

/// A Gen 1 battle sits at its own menu until a button arrives, and
/// `HandleMenuInput` delays several frames after accepting one — a faster mash
/// looks exactly like dropped input. Same constant, same reason, as
/// `examples/support/paperclaims.rs`.
const MENU_GAP: u32 = 24;

/// `BattlePlugin` re-applies its opposition override until it reads back
/// intact, and the send-out animation writes `wBattleMon` after that. A sample
/// taken inside this window holds a half-written struct.
const SETTLE_FRAMES: u32 = 120;

// ---------------------------------------------------------------------------
// item ids
// ---------------------------------------------------------------------------

/// Which ball a bag item id is. **One mapping, and it lives beside the routine
/// the odds come from** — `capture::ball_for_item`, whose ids are the
/// cartridge's own `cp` immediates at `03:5702`, `03:5707` and `03:5711`. A
/// second copy here is how an instrument and the thing it measures end up
/// talking about different items.
fn ball_of(item: u8) -> Option<Ball> {
    capture::ball_for_item(item)
}

// ---------------------------------------------------------------------------
// plumbing
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

/// Copy the cartridge into `target/` before playing it. **Never point this at
/// `~/roms`** — the MBC writes a `.sav` beside whatever path it is handed.
fn stage(rom: &Path) -> Result<PathBuf, String> {
    let dir = PathBuf::from("target/gen1-catch-drill");
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

fn tap(gb: &mut Gameboy, key: KeypadKey) {
    gb.keydown(key);
    run(gb, 4);
    gb.keyup(key);
    run(gb, MENU_GAP - 4);
}

fn tiles(gb: &mut Gameboy) -> Vec<u8> {
    (0..20u16 * 18).map(|i| gb.peek(TILE_MAP + i)).collect()
}

fn screen(gb: &mut Gameboy) -> String {
    terminalgb_plugin_trainer::text::decode_screen(&tiles(gb))
}

/// Read a big-endian 16-bit field, which is how Gen 1 stores every HP and stat.
fn be16(gb: &mut Gameboy, at: u16) -> u16 {
    ((gb.peek(at) as u16) << 8) | gb.peek(at + 1) as u16
}

// ---------------------------------------------------------------------------
// the bag
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Bag {
    items: Vec<(u8, u8)>,
}

impl Bag {
    fn read(gb: &mut Gameboy) -> Bag {
        let n = gb.peek(tr::NUM_BAG_ITEMS) as usize;
        Bag {
            items: (0..n.min(tr::MAX_BAG_ITEMS))
                .map(|i| {
                    let at = tr::BAG_ITEMS + (i * 2) as u16;
                    (gb.peek(at), gb.peek(at + 1))
                })
                .collect(),
        }
    }

    fn count(&self, item: u8) -> u8 {
        self.items
            .iter()
            .find(|(i, _)| *i == item)
            .map(|(_, c)| *c)
            .unwrap_or(0)
    }

    fn balls(&self) -> Vec<(u8, u8)> {
        self.items
            .iter()
            .copied()
            .filter(|(i, _)| ball_of(*i).is_some())
            .collect()
    }
}

/// **Declared assistance.** Replace the bag outright, so a drill needs no walk
/// to a shop. `wBagItems` is a list of id/quantity pairs terminated by `$FF`
/// (`ram/wram.asm`, `wBagItems: ds BAG_ITEM_CAPACITY * 2 + 1`).
fn write_bag(gb: &mut Gameboy, items: &[(u8, u8)]) {
    gb.debug_write(tr::NUM_BAG_ITEMS, items.len() as u8);
    for (i, (id, qty)) in items.iter().enumerate() {
        let at = tr::BAG_ITEMS + (i * 2) as u16;
        gb.debug_write(at, *id);
        gb.debug_write(at + 1, *qty);
    }
    gb.debug_write(tr::BAG_ITEMS + (items.len() * 2) as u16, 0xFF);
}

fn item_name(id: u8) -> String {
    // Only the handful this drill talks about; anything else is reported as its
    // number rather than guessed at.
    match id {
        1 => "MASTER BALL".into(),
        2 => "ULTRA BALL".into(),
        3 => "GREAT BALL".into(),
        4 => "POKé BALL".into(),
        8 => "SAFARI BALL".into(),
        0x14 => "POTION".into(),
        0x13 => "SUPER POTION".into(),
        0x46 => "OAK's PARCEL".into(),
        other => format!("item #{other}"),
    }
}

// ---------------------------------------------------------------------------
// arming a battle
// ---------------------------------------------------------------------------

struct Arena {
    gb: Gameboy,
}

impl Arena {
    fn start(
        staged: &Path,
        rom: &[u8],
        presets: &PresetFile,
        state: &[u8],
        name: &str,
        jitter: u32,
    ) -> Result<Arena, String> {
        let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
        // A machine with no APU is not a quiet Game Boy, it is one with the sound
        // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
        let plugin = BattlePlugin::new(rom.to_vec(), presets.clone())
            .map_err(|e| format!("plugin: {e}"))?;
        gb.add_plugin(Box::new(plugin))
            .map_err(|e| format!("plugin: {e}"))?;
        gb.load_state(state).map_err(|e| format!("state: {e:?}"))?;
        // **The jitter is not decoration.** Emulation is deterministic, so two
        // trials from the same state with the same inputs are one trial wearing
        // two hats. `Random_` folds `rDIV` in on every call, so the lever is
        // *time* — the same lever `plugins/gen1-shinyhunt` pulls.
        run(&mut gb, jitter);
        gb.plugin_mut::<BattlePlugin>()
            .ok_or("the plugin vanished")?
            .arm(name)
            .map_err(|e| e.to_string())?;
        Ok(Arena { gb })
    }

    /// Step until the engine reports a battle, then let the send-out settle.
    fn await_battle(&mut self, limit: u32) -> Result<u8, String> {
        for f in 0..limit {
            self.gb.frame();
            if f % MENU_GAP == 0 {
                self.gb.keydown(KeypadKey::A);
            } else if f % MENU_GAP == 4 {
                self.gb.keyup(KeypadKey::A);
            }
            let k = self.gb.peek(tr::IS_IN_BATTLE);
            if k == 1 || k == 2 {
                self.gb.keyup(KeypadKey::A);
                run(&mut self.gb, SETTLE_FRAMES);
                return Ok(self.gb.peek(tr::IS_IN_BATTLE));
            }
        }
        Err("no battle started".into())
    }

    /// Advance to the battle's own top menu (`FIGHT`/`PKMN` over `ITEM`/`RUN`).
    fn await_menu(&mut self, limit: u32) -> Result<(), String> {
        for _ in 0..limit / MENU_GAP {
            let s = screen(&mut self.gb);
            if s.contains("FIGHT") && s.contains("ITEM") {
                return Ok(());
            }
            tap(&mut self.gb, KeypadKey::B);
        }
        Err(format!(
            "never reached the battle menu; screen was {:?}",
            screen(&mut self.gb)
        ))
    }
}

// ---------------------------------------------------------------------------
// throwing a ball
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// The party grew: the Pokémon is ours.
    Caught,
    /// The battle carried on.
    BrokeFree,
    /// `TrainerBlockedText` — the game refused the throw.
    TrainerBlocked,
    /// The opponent ran, fainted, or the battle otherwise ended without a
    /// capture. Recorded separately so it can never be counted as either.
    BattleEndedOtherwise,
    /// The bag had no ball to select.
    NoBall,
}

impl Outcome {
    fn label(self) -> &'static str {
        match self {
            Outcome::Caught => "caught",
            Outcome::BrokeFree => "broke free",
            Outcome::TrainerBlocked => "trainer blocked",
            Outcome::BattleEndedOtherwise => "battle ended otherwise",
            Outcome::NoBall => "no ball",
        }
    }
}

#[derive(Debug, Clone)]
struct Throw {
    outcome: Outcome,
    /// What `capture::odds` said before the ball left our hand.
    predicted: f64,
    ball: Ball,
    item: u8,
    /// The ball count before and after — the only way to answer "does a wasted
    /// throw cost a ball?".
    balls_before: u8,
    balls_after: u8,
    text: String,
}

/// Open the bag from the battle menu, select `item`, and throw it.
///
/// Every step is driven by what is on the screen, never by a frame count — the
/// rule `docs/gen1/reading-the-screen.md` states and the same one the preset
/// scripts follow. The cursor row is read back from `wCurrentMenuItem` rather
/// than counted, so a swallowed press repeats instead of going out of phase.
fn throw(gb: &mut Gameboy, item: u8) -> Result<Throw, String> {
    let ball = ball_of(item).ok_or_else(|| format!("item {item} is not a ball"))?;
    let before = Bag::read(gb);
    let balls_before = before.count(item);
    if balls_before == 0 {
        return Ok(Throw {
            outcome: Outcome::NoBall,
            predicted: 0.0,
            ball,
            item,
            balls_before,
            balls_after: 0,
            text: String::new(),
        });
    }
    let target = capture::Target {
        catch_rate: gb.peek(tr::ENEMY_ACTUAL_CATCH_RATE),
        hp: be16(
            gb,
            tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP,
        ),
        max_hp: be16(
            gb,
            tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::MAX_HP,
        ),
        status: Status(
            gb.peek(tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::STATUS),
        ),
    };
    let predicted = capture::odds(target, ball).chance;
    let party_before = gb.peek(tr::PARTY_COUNT);

    // The battle menu is `FIGHT PKMN` over `ITEM RUN`: down then A opens the
    // bag. **Down, not right** — PKMN is the one to the right of FIGHT.
    tap(gb, KeypadKey::Down);
    tap(gb, KeypadKey::A);

    // The bag list is now up. Walk the cursor to the ball by name.
    let want = item_name(item);
    let mut found = false;
    for _ in 0..(tr::MAX_BAG_ITEMS + 4) {
        let s = screen(gb);
        if !s.contains(&want) {
            return Err(format!("the bag list does not show {want:?}: {s:?}"));
        }
        // `wCurrentMenuItem` is the row inside the visible window, and the list
        // scrolls, so compare the *pointed* line against the name rather than
        // counting rows.
        if pointed_line(gb).map(|l| l.contains(&want)).unwrap_or(false) {
            found = true;
            break;
        }
        tap(gb, KeypadKey::Down);
    }
    if !found {
        return Err(format!("could not put the cursor on {want:?}"));
    }
    tap(gb, KeypadKey::A);

    // Everything after this is the game's. Watch for the four endings.
    let mut text = String::new();
    let mut outcome = None;
    for step in 0..200u32 {
        let s = screen(gb);
        if s.contains("blocked the") {
            outcome = Some(Outcome::TrainerBlocked);
            text = s;
            break;
        }
        if gb.peek(tr::PARTY_COUNT) > party_before {
            outcome = Some(Outcome::Caught);
            text = s;
            break;
        }
        if step > 2 && gb.peek(tr::IS_IN_BATTLE) == 0 {
            outcome = Some(Outcome::BattleEndedOtherwise);
            text = s;
            break;
        }
        if step > 2 && s.contains("FIGHT") && s.contains("ITEM") {
            outcome = Some(Outcome::BrokeFree);
            text = s;
            break;
        }
        tap(gb, KeypadKey::B);
    }
    let outcome = outcome.ok_or_else(|| {
        format!("the throw never resolved; screen was {:?}", screen(gb))
    })?;
    // A capture asks for a nickname. Answer NO, which is the second row of the
    // YES/NO box, so it needs a Down before the A.
    if outcome == Outcome::Caught {
        for _ in 0..12 {
            let s = screen(gb);
            if s.contains("YES") && s.contains("NO") {
                tap(gb, KeypadKey::Down);
                tap(gb, KeypadKey::A);
                break;
            }
            tap(gb, KeypadKey::B);
        }
    }
    let after = Bag::read(gb);
    Ok(Throw {
        outcome,
        predicted,
        ball,
        item,
        balls_before,
        balls_after: after.count(item),
        text,
    })
}

/// The line the `▶` cursor is on, decoded.
fn pointed_line(gb: &mut Gameboy) -> Option<String> {
    let t = tiles(gb);
    let row = (0..18).find(|r| (0..20).any(|c| t[r * 20 + c] == 0xED))?;
    Some(terminalgb_plugin_trainer::text::decode_screen(
        &t[row * 20..row * 20 + 20],
    ))
}

// ---------------------------------------------------------------------------
// subcommands
// ---------------------------------------------------------------------------

fn cmd_bag(rom: &Path, states: &[PathBuf]) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    println!("{:<14} {:>5} {:>6}  bag", "state", "party", "money");
    for s in states {
        let st = std::fs::read(s).map_err(|e| format!("{}: {e}", s.display()))?;
        let mut gb = Gameboy::new(bytes.clone(), Some(staged.clone()));
        // Same rule as above: a machine with no APU is not a quiet Game Boy.
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
        gb.load_state(&st).map_err(|e| format!("state: {e:?}"))?;
        let bag = Bag::read(&mut gb);
        let money = terminalgb_plugin_trainer::text::bcd3([
            gb.peek(tr::PLAYER_MONEY),
            gb.peek(tr::PLAYER_MONEY + 1),
            gb.peek(tr::PLAYER_MONEY + 2),
        ]);
        let listed = bag
            .items
            .iter()
            .map(|(i, q)| format!("{} x{q}", item_name(*i)))
            .collect::<Vec<_>>()
            .join(", ");
        let name = s.file_name().unwrap_or_default().to_string_lossy();
        println!(
            "{:<14} {:>5} {:>6}  {}",
            name,
            gb.peek(tr::PARTY_COUNT),
            money,
            if listed.is_empty() {
                "(empty)".into()
            } else {
                listed
            }
        );
        let balls: u32 = bag.balls().iter().map(|(_, q)| *q as u32).sum();
        if balls > 0 {
            println!("{:<14} -> {balls} ball(s): CATCHING IS AVAILABLE HERE", "");
        }
    }
    Ok(())
}

/// `wEventFlags` + 4. Its bit 5 is `EVENT_GOT_POKEDEX` (link 09's own
/// evidence); the ball gift sits on **bit 4** of the same byte, and that is
/// read out of the cartridge, not recalled — `07:52D9` is
/// `ld hl,$D74B / bit 4,[hl] / set 4,[hl] / jr nz,+ / ld bc,$0405 /
/// call GiveItem`, i.e. *if this bit is clear, set it and hand over five of
/// item 4*. The text it then prints is Oak's own: "You must catch them! Use
/// these to capture wild POKéMON. <PLAYER> got 5 POKé BALLs!".
const EVENT_BYTE_D74B: u16 = 0xD74B;
const BALL_GIFT_BIT: u8 = 1 << 4;

/// `wJoyIgnore` — a mask of buttons the game is discarding. `$FF` while a
/// cutscene owns the player.
const JOY_IGNORE: u16 = 0xCD6B;

/// Does talking to Oak again, after the Pokédex, open the capability?
///
/// Driven rather than argued: load the state, mash A while nudging the player
/// towards him, and watch the bag and the flag. A measurement, so it can come
/// back "no".
fn cmd_gift(rom: &Path, state: &Path, frames: u32) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let st = std::fs::read(state).map_err(|e| format!("{}: {e}", state.display()))?;
    let mut gb = Gameboy::new(bytes, Some(staged.clone()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    gb.load_state(&st).map_err(|e| format!("state: {e:?}"))?;
    let before = Bag::read(&mut gb);
    println!(
        "  $D74B = {:#04X} (bit4 balls-from-oak {}, bit5 got-pokedex {}), \
         wJoyIgnore {:#04X}, wCurMapScript {:#04X}",
        gb.peek(EVENT_BYTE_D74B),
        gb.peek(EVENT_BYTE_D74B) & BALL_GIFT_BIT != 0,
        gb.peek(EVENT_BYTE_D74B) & 0x20 != 0,
        gb.peek(0xCD6B),
        gb.peek(0xD5F0)
    );
    println!(
        "before: map {:#04X} at ({}, {}), ${:#06X} bit4 = {}, bag {:?}",
        gb.peek(0xD35E),
        gb.peek(0xD362),
        gb.peek(0xD361),
        EVENT_BYTE_D74B,
        gb.peek(EVENT_BYTE_D74B) & BALL_GIFT_BIT != 0,
        before.items
    );
    // **`wJoyIgnore` first.** Link 09's exit state is captured *inside* the
    // Pokédex cutscene — its goal is the flag, and the script sets the flag
    // before it hands control back — so the joypad is being ignored ($FF) and
    // every button press is discarded. A probe that starts pressing here
    // measures nothing and reports "not opened" with total confidence.
    for _ in 0..6000 {
        if gb.peek(JOY_IGNORE) == 0 {
            break;
        }
        gb.frame();
    }
    println!("  wJoyIgnore cleared to {:#04X}", gb.peek(JOY_IGNORE));

    // Walk up to Oak and talk. `Up` because link 09 leaves the player south of
    // him; A is the only button that starts a conversation, and B would back
    // out of the one it starts.
    let mut opened = None;
    let mut seen: Vec<String> = Vec::new();
    let mut visited: std::collections::BTreeSet<(u8, u8)> = Default::default();
    for step in 0..frames / MENU_GAP {
        // A advances a text box and starts a conversation; B would back out of
        // the one it starts. A nudge every so often, because the player may be
        // standing away from him.
        // **B before a direction, A only when standing still.** A text box eats
        // a direction press, so a loop that mashes A can never walk; and B
        // closes a box but never opens one, which is the whole reason the Gen 1
        // navigators in this repository use it.
        match step % 4 {
            0 => tap(&mut gb, KeypadKey::B),
            1 | 2 => {
                let dir = [
                    KeypadKey::Down,
                    KeypadKey::Left,
                    KeypadKey::Up,
                    KeypadKey::Up,
                    KeypadKey::Right,
                    KeypadKey::Up,
                ][(step / 4) as usize % 6];
                tap(&mut gb, dir);
            }
            _ => tap(&mut gb, KeypadKey::A),
        }
        visited.insert((gb.peek(0xD362), gb.peek(0xD361)));
        let s = screen(&mut gb);
        if !s.trim().is_empty() && !seen.iter().any(|p| *p == s) {
            seen.push(s);
        }
        let bag = Bag::read(&mut gb);
        let balls: u32 = bag.balls().iter().map(|(_, q)| *q as u32).sum();
        if balls > 0 {
            opened = Some((step, bag, screen(&mut gb)));
            break;
        }
    }
    match opened {
        Some((step, bag, s)) => {
            println!(
                "OPENED after {} presses: bag {:?}, {:#06X} bit4 = {}",
                step + 1,
                bag.items,
                EVENT_BYTE_D74B,
                gb.peek(EVENT_BYTE_D74B) & BALL_GIFT_BIT != 0
            );
            println!("{}", indent(&s));
        }
        None => {
            println!(
                "NOT OPENED in {frames} frames — the bag is still {:?}, map {:#04X} at ({}, {})",
                Bag::read(&mut gb).items,
                gb.peek(0xD35E),
                gb.peek(0xD362),
                gb.peek(0xD361)
            );
            println!("      visited {visited:?}");
            for s in seen.iter().take(6) {
                println!("{}\n      ---", indent(s));
            }
        }
    }
    Ok(())
}

fn cmd_kinds(rom: &Path, state: &Path, trials: u32) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let presets = PresetFile::parse(PRESETS, "gen1_catch_drill.preset")
        .map_err(|e| e.to_string())?;
    let st = std::fs::read(state).map_err(|e| format!("{}: {e}", state.display()))?;

    // -----------------------------------------------------------------
    // 1. Trace one battle of each kind, frame window by frame window, from
    //    the moment the engine reports a battle to the moment its own menu is
    //    up. The pair is matched: one save, one challenger, and only the
    //    preset's `wild =`/`trainer =` line different.
    // -----------------------------------------------------------------
    println!("== the matched pair, traced from the battle's first frame ==");
    let mut traces: BTreeMap<&str, Vec<Sample>> = BTreeMap::new();
    for name in ["pair-wild", "pair-trainer"] {
        let mut a = Arena::start(&staged, &bytes, &presets, &st, name, 0)?;
        // Step to the battle WITHOUT the settle wait, so the opening text is
        // still on screen when sampling starts.
        let mut started = None;
        for f in 0..2400u32 {
            a.gb.frame();
            if f % MENU_GAP == 0 {
                a.gb.keydown(KeypadKey::A);
            } else if f % MENU_GAP == 4 {
                a.gb.keyup(KeypadKey::A);
            }
            let k = a.gb.peek(tr::IS_IN_BATTLE);
            if k == 1 || k == 2 {
                started = Some(k);
                break;
            }
        }
        let kind = started.ok_or_else(|| format!("{name}: no battle started"))?;
        a.gb.keyup(KeypadKey::A);
        let mut samples = Vec::new();
        for f in 0..1200u32 {
            a.gb.frame();
            if f % MENU_GAP == 0 {
                a.gb.keydown(KeypadKey::A);
            } else if f % MENU_GAP == 4 {
                a.gb.keyup(KeypadKey::A);
            }
            let t = tiles(&mut a.gb);
            let s = terminalgb_plugin_trainer::text::decode_screen(&t);
            samples.push((f, t, s));
        }
        println!("  {name}: wIsInBattle {kind}, {} samples", samples.len());
        traces.insert(name, samples);
    }
    let w = &traces["pair-wild"];
    let t = &traces["pair-trainer"];

    // Where does the TEXT decide it, and for how long?
    for (name, tr_) in [("wild", w), ("trainer", t)] {
        let decided: Vec<u32> = tr_
            .iter()
            .filter(|(_, _, s)| battlekind::from_text(s).is_some())
            .map(|(f, _, _)| *f)
            .collect();
        match (decided.first(), decided.last()) {
            (Some(a), Some(b)) => println!(
                "  {name}: the text decides it on {} of {} samples, frames {a}..{b}",
                decided.len(),
                tr_.len()
            ),
            _ => println!("  {name}: the text NEVER decides it"),
        }
    }

    // Is there a cell that separates them on EVERY sample? That is what a
    // policy consulting the screen at the moment it wants to throw would need.
    let n = w.len().min(t.len());
    let mut always: Vec<usize> = (0..20 * 18).collect();
    for i in 0..n {
        let (_, wt, _) = &w[i];
        let (_, tt, _) = &t[i];
        always.retain(|&c| wt[c] != tt[c]);
    }
    println!(
        "  cells that differ on ALL {n} paired samples: {}",
        if always.is_empty() {
            "NONE".to_string()
        } else {
            always
                .iter()
                .map(|c| format!("r{}c{}", c / 20, c % 20))
                .collect::<Vec<_>>()
                .join(" ")
        }
    );
    let mins = (0..n)
        .map(|i| (0..20 * 18).filter(|&c| w[i].1[c] != t[i].1[c]).count())
        .min()
        .unwrap_or(0);
    println!("  fewest cells differing on any paired sample: {mins}");

    // **How wide is the window, in frames?** A policy samples the screen at
    // some rate, and a signal narrower than that rate is a signal it will miss.
    for (name, tr_) in [("wild", w), ("trainer", t)] {
        let frames: Vec<u32> = tr_
            .iter()
            .filter(|(_, _, s)| battlekind::from_text(s).is_some())
            .map(|(f, _, _)| *f)
            .collect();
        println!(
            "  {name}: the text decides it on {} of {} frames{}",
            frames.len(),
            tr_.len(),
            match (frames.first(), frames.last()) {
                (Some(a), Some(b)) => format!(", frames {a}..{b}"),
                _ => " — NEVER".into(),
            }
        );
        // The longest run of consecutive frames, which is what a sampler at a
        // given interval is guaranteed to land in.
        let mut best = 0u32;
        let mut runlen = 0u32;
        let mut prev = None;
        for f in &frames {
            runlen = match prev {
                Some(p) if *f == p + 1 => runlen + 1,
                _ => 1,
            };
            best = best.max(runlen);
            prev = Some(*f);
        }
        println!("    longest unbroken window: {best} frames");
        // And the miss rate at each sampling interval, over every phase.
        let hit: Vec<String> = [1u32, 2, 4, 6, 8, 12, 16, 24, 32, 48]
            .iter()
            .map(|iv| {
                let missed = (0..*iv)
                    .filter(|ph| !frames.iter().any(|f| f % iv == *ph))
                    .count();
                format!("{iv}:{}/{iv}", missed)
            })
            .collect();
        println!(
            "    phases that MISS it entirely, by sampling interval: {}",
            hit.join("  ")
        );
    }

    // -----------------------------------------------------------------
    // 2. Score the discriminator the way a policy would use it: a latch fed
    //    every sample from the battle's first frame, asked at the menu.
    // -----------------------------------------------------------------
    println!("\n== the discriminator, scored against wIsInBattle at the menu ==");
    let mut confusion: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    let mut stateless: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for i in 0..trials {
        for name in ["pair-wild", "pair-trainer"] {
            let mut a = Arena::start(&staged, &bytes, &presets, &st, name, i * 7)?;
            let mut watch = battlekind::Watcher::default();
            let mut truth = 0u8;
            let mut at_menu = false;
            for f in 0..3600u32 {
                a.gb.frame();
                if f % MENU_GAP == 0 {
                    a.gb.keydown(KeypadKey::A);
                } else if f % MENU_GAP == 4 {
                    a.gb.keyup(KeypadKey::A);
                }
                let k = a.gb.peek(tr::IS_IN_BATTLE);
                if k == 1 || k == 2 {
                    truth = k;
                    if f % 8 == 0 {
                        let tl = tiles(&mut a.gb);
                        watch.observe(&tl);
                        let s = terminalgb_plugin_trainer::text::decode_screen(&tl);
                        if f > 200 && s.contains("FIGHT") && s.contains("ITEM") {
                            at_menu = true;
                            break;
                        }
                    }
                } else if truth != 0 {
                    break;
                }
            }
            if !at_menu {
                return Err(format!("{name}: never reached the menu"));
            }
            let truth = if truth == 1 { "wild" } else { "trainer" };
            *confusion
                .entry((truth, watch.verdict().label()))
                .or_insert(0) += 1;
            let bare = battlekind::from_tiles(&tiles(&mut a.gb));
            *stateless.entry((truth, bare.label())).or_insert(0) += 1;
        }
    }
    for (label, table) in [("latched", &confusion), ("stateless", &stateless)] {
        let total: u32 = table.values().sum();
        let wrong: u32 = table
            .iter()
            .filter(|((tr_, sd), _)| tr_ != sd)
            .map(|(_, n)| *n)
            .sum();
        println!("  -- {label} --");
        for ((truth, said), n) in table.iter() {
            println!("     {truth:<9} {said:<9} {n}");
        }
        println!(
            "     wrong-or-unknown {wrong}/{total} = {:.2}%",
            100.0 * wrong as f64 / total.max(1) as f64
        );
    }
    Ok(())
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("      |{l}|"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The mechanics drill: many throws, every outcome recorded, and the catch
/// rate compared against what `capture::odds` predicted.
fn cmd_drill(
    rom: &Path,
    state: &Path,
    scenarios: &[String],
    trials: u32,
) -> Result<(), String> {
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let presets = PresetFile::parse(PRESETS, "gen1_catch_drill.preset")
        .map_err(|e| e.to_string())?;
    let st = std::fs::read(state).map_err(|e| format!("{}: {e}", state.display()))?;

    println!("== declared assistance: the bag is written, and so is the enemy's HP ==");
    let mut throws: Vec<(String, Throw)> = Vec::new();
    for name in scenarios {
        for t in 0..trials {
            let mut a = Arena::start(&staged, &bytes, &presets, &st, name, t * 11)?;
            let flag = a.await_battle(2400)?;
            // Twenty of each ball: enough that a run never ends on an empty
            // bag, which would be a *different* measurement.
            write_bag(&mut a.gb, &[(4, 20), (3, 20), (2, 20)]);
            a.await_menu(1200)?;
            // Sweep the opponent across its HP range so the odds are sampled
            // where they vary, not only at full health.
            let max = be16(
                &mut a.gb,
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::MAX_HP,
            );
            let want = (max as u32 * (1 + t % 8) / 8).max(1) as u16;
            a.gb.debug_write(
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP,
                (want >> 8) as u8,
            );
            a.gb.debug_write(
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP + 1,
                want as u8,
            );
            // A status is worth 25 or 12 subtracted from the first random
            // byte, so sweeping it is sweeping the odds — and it is the half of
            // the formula a modern reading gets most wrong.
            let st_byte = [0u8, 0, 0x04, 0x08][(t as usize / 8) % 4];
            a.gb.debug_write(
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::STATUS,
                st_byte,
            );
            let item = [4u8, 3, 2][(t as usize) % 3];
            match throw(&mut a.gb, item) {
                Ok(th) => {
                    if flag == 2 && th.outcome != Outcome::TrainerBlocked {
                        println!(
                            "  NOTE trainer battle, outcome {} — {:?}",
                            th.outcome.label(),
                            th.text
                        );
                    }
                    throws.push((name.clone(), th));
                }
                Err(e) => println!("  {name} trial {t}: {e}"),
            }
        }
    }
    report(&throws);
    Ok(())
}

/// The policy, exercised end to end: synthesise an encounter, ask
/// [`terminalgb_plugin_agent::catch::decide`], and carry out whatever it says.
///
/// This is the part that makes the whole thing more than a calculator — the
/// gate, the discriminator, the target list and the odds all meet here, and
/// what comes out is the rate the *policy* achieves rather than the rate a
/// blind sweep achieves.
fn cmd_policy(
    rom: &Path,
    state: &Path,
    scenarios: &[String],
    trials: u32,
    with_balls: bool,
) -> Result<(), String> {
    use terminalgb_plugin_agent::catch::{self, Situation, Verdict, Want};
    let staged = stage(rom)?;
    let bytes = std::fs::read(&staged).map_err(|e| e.to_string())?;
    let presets = PresetFile::parse(PRESETS, "gen1_catch_drill.preset")
        .map_err(|e| e.to_string())?;
    let st = std::fs::read(state).map_err(|e| format!("{}: {e}", state.display()))?;

    // The target list `gen1_tables.py --plan brock,misty` produced from this
    // cartridge. Its species is an INTERNAL index, which is what
    // `wEnemyMonSpecies` holds.
    let wants = vec![Want {
        species: 84,
        cap: 1,
        why: "PIKACHU: electric, and Misty is water",
    }];
    println!(
        "== the policy, with{} a ball in the bag ==",
        if with_balls { "" } else { "OUT" }
    );
    let mut verdicts: BTreeMap<(String, &'static str), u32> = BTreeMap::new();
    let mut throws: Vec<(String, Throw)> = Vec::new();
    let mut decisions = 0u32;
    let mut non_decisions = 0u32;
    for name in scenarios {
        for t in 0..trials {
            let mut a = Arena::start(&staged, &bytes, &presets, &st, name, t * 13)?;
            let flag = a.await_battle(2400)?;
            if with_balls {
                write_bag(&mut a.gb, &[(4, 20), (3, 5)]);
            } else {
                write_bag(&mut a.gb, &[(0x14, 3)]); // potions only
            }
            a.await_menu(1200)?;
            // Chip the opponent the way a fight would, so the odds vary.
            let max = be16(
                &mut a.gb,
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::MAX_HP,
            );
            let want = (max as u32 * (1 + t % 6) / 6).max(1) as u16;
            a.gb.debug_write(
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP,
                (want >> 8) as u8,
            );
            a.gb.debug_write(
                tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP + 1,
                want as u8,
            );

            // The discriminator, as a screen-only player would run it: a latch
            // fed from the battle's first frame. `Arena::await_battle` has
            // already gone past the opening line, so it is re-derived here from
            // the memory truth and the latch is exercised separately by
            // `kinds`. This measures the *policy*, not the discriminator.
            let kind = match flag {
                1 => battlekind::ScreenVerdict::Wild,
                _ => battlekind::ScreenVerdict::Trainer,
            };
            let party: Vec<u8> = {
                let n = a.gb.peek(tr::PARTY_COUNT) as usize;
                (0..n.min(6))
                    .map(|i| a.gb.peek(tr::PARTY_SPECIES + i as u16))
                    .collect()
            };
            let items: Vec<terminalgb_plugin_agent::battle::Item> = Bag::read(&mut a.gb)
                .items
                .iter()
                .map(|(id, count)| terminalgb_plugin_agent::battle::Item {
                    id: *id,
                    count: *count,
                })
                .collect();
            let s = Situation {
                kind,
                battle_type: Some(
                    terminalgb_plugin_trainer::gen1::BattleType::from_flag(
                        a.gb.peek(tr::BATTLE_TYPE),
                    ),
                ),
                species: a.gb.peek(
                    tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::SPECIES,
                ),
                target: capture::Target {
                    catch_rate: a.gb.peek(tr::ENEMY_ACTUAL_CATCH_RATE),
                    hp: be16(
                        &mut a.gb,
                        tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::HP,
                    ),
                    max_hp: max,
                    status: Status(a.gb.peek(
                        tr::ENEMY_MON + terminalgb_plugin_trainer::gen1::battle::STATUS,
                    )),
                },
                items: &items,
                party: &party,
                our_hp: be16(
                    &mut a.gb,
                    tr::BATTLE_MON + terminalgb_plugin_trainer::gen1::battle::HP,
                ),
                incoming_crit: 0,
            };
            let v = catch::decide(&s, &wants);
            if v.is_decision() {
                decisions += 1;
            } else {
                non_decisions += 1;
            }
            let label: &'static str = match &v {
                Verdict::Unavailable => "unavailable (not a decision)",
                Verdict::Throw(_) => "throw",
                Verdict::Weaken { .. } => "weaken first",
                Verdict::No { why } => why,
            };
            *verdicts.entry((name.clone(), label)).or_insert(0) += 1;
            if let Verdict::Throw(th) = v {
                match throw(&mut a.gb, th.item) {
                    Ok(r) => throws.push((name.clone(), r)),
                    Err(e) => println!("  {name} trial {t}: {e}"),
                }
            }
        }
    }
    println!("  {:<16} {:<42} n", "scenario", "verdict");
    for ((n, l), c) in &verdicts {
        println!("  {n:<16} {l:<42} {c}");
    }
    println!("  decisions {decisions}, non-decisions {non_decisions}");
    if !throws.is_empty() {
        report(&throws);
    }
    Ok(())
}

fn report(throws: &[(String, Throw)]) {
    println!("\n== outcomes ==");
    let mut by: BTreeMap<(&str, &str), u32> = BTreeMap::new();
    for (n, t) in throws {
        *by.entry((n.as_str(), t.outcome.label())).or_insert(0) += 1;
    }
    for ((n, o), c) in &by {
        println!("  {n:<16} {o:<24} {c}");
    }
    // Which ball was thrown, because "61 % caught" over a mixture of balls is
    // an average nobody can act on without knowing the mixture.
    let mut balls: BTreeMap<(&'static str, u8), u32> = BTreeMap::new();
    for (_, t) in throws {
        *balls.entry((t.ball.short(), t.item)).or_insert(0) += 1;
    }
    println!(
        "  balls thrown: {}",
        balls
            .iter()
            .map(|((b, i), c)| format!("{b} (item {i}) x{c}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    println!("\n== does a refused throw cost a ball? ==");
    let mut cost: BTreeMap<(&str, bool), u32> = BTreeMap::new();
    for (_, t) in throws {
        cost.entry((t.outcome.label(), t.balls_after < t.balls_before))
            .and_modify(|c| *c += 1)
            .or_insert(1);
    }
    for ((o, spent), c) in &cost {
        println!(
            "  {o:<24} ball {} — {c} throw(s)",
            if *spent { "SPENT" } else { "kept" }
        );
    }

    println!("\n== the catch rate against what capture::odds predicted ==");
    // Bucket by predicted probability so the comparison is a calibration curve
    // rather than one aggregate number, which can be right on average while
    // being wrong everywhere.
    let mut buckets: BTreeMap<u32, (u32, u32, f64)> = BTreeMap::new();
    for (_, t) in throws {
        if matches!(t.outcome, Outcome::TrainerBlocked | Outcome::NoBall) {
            continue;
        }
        let b = ((t.predicted * 10.0) as u32).min(9);
        let e = buckets.entry(b).or_insert((0, 0, 0.0));
        e.0 += 1;
        if t.outcome == Outcome::Caught {
            e.1 += 1;
        }
        e.2 += t.predicted;
    }
    println!(
        "  {:<14} {:>6} {:>8} {:>10} {:>10}",
        "predicted", "n", "caught", "observed", "expected"
    );
    let (mut n_all, mut c_all, mut p_all) = (0u32, 0u32, 0.0f64);
    for (b, (n, c, p)) in &buckets {
        println!(
            "  {:<14} {:>6} {:>8} {:>9.1}% {:>9.1}%",
            format!("{}-{}%", b * 10, b * 10 + 10),
            n,
            c,
            100.0 * *c as f64 / *n as f64,
            100.0 * p / *n as f64
        );
        n_all += n;
        c_all += c;
        p_all += p;
    }
    if n_all > 0 {
        let obs = c_all as f64 / n_all as f64;
        let exp = p_all / n_all as f64;
        // A binomial standard error on the expectation, so "agrees" is a
        // statement with a bound rather than an impression.
        let se = (exp * (1.0 - exp) / n_all as f64).sqrt();
        println!(
            "  ALL            {n_all:>6} {c_all:>8} {:>9.1}% {:>9.1}%   \
             z = {:+.2} (1 s.e. = {:.2}%)",
            100.0 * obs,
            100.0 * exp,
            (obs - exp) / se.max(f64::EPSILON),
            100.0 * se
        );
    }
}

// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!(
            "gen1_catch_drill — the catching mechanics, measured\n\n\
             bag    --rom R --state F [--state F ...]\n\
             gift   --rom R --state F [--trials N]   (N thousand frames)\n\
             kinds  --rom R --state F [--trials N]\n\
             drill  --rom R --state F [--scenario NAME ...] [--trials N]\n\
             policy --rom R --state F [--scenario NAME ...] [--trials N]\n\
             policy-empty  ...the same, with no ball in the bag\n"
        );
        std::process::exit(2);
    }
    let cmd = args[0].clone();
    let mut rom: Option<PathBuf> = None;
    let mut states: Vec<PathBuf> = Vec::new();
    let mut scenarios: Vec<String> = Vec::new();
    let mut trials = 8u32;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--rom" => {
                rom = args.get(i + 1).map(PathBuf::from);
                i += 2;
            }
            "--state" => {
                if let Some(s) = args.get(i + 1) {
                    states.push(PathBuf::from(s));
                }
                i += 2;
            }
            "--scenario" => {
                if let Some(s) = args.get(i + 1) {
                    scenarios.push(s.clone());
                }
                i += 2;
            }
            "--trials" => {
                trials = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(8);
                i += 2;
            }
            other => {
                eprintln!("unknown argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    let rom = match rom.or_else(find_rom) {
        Some(r) => r,
        None => {
            eprintln!(
                "no cartridge: pass --rom, or put Pokémon Red/Blue in ~/roms. \
                 None is distributed with this repository."
            );
            std::process::exit(0);
        }
    };
    if scenarios.is_empty() {
        scenarios = vec![
            "pair-wild".into(),
            "pair-trainer".into(),
            "wild-pikachu".into(),
            "wild-caterpie".into(),
            "wild-mewtwo".into(),
        ];
    }
    let r = match cmd.as_str() {
        "bag" => cmd_bag(&rom, &states),
        "gift" => match states.first() {
            Some(s) => cmd_gift(&rom, s, trials.max(1) * 1000),
            None => Err("gift needs --state".into()),
        },
        "kinds" => match states.first() {
            Some(s) => cmd_kinds(&rom, s, trials),
            None => Err("kinds needs --state".into()),
        },
        "policy" | "policy-empty" => match states.first() {
            Some(s) => cmd_policy(&rom, s, &scenarios, trials, cmd == "policy"),
            None => Err("policy needs --state".into()),
        },
        "drill" => match states.first() {
            Some(s) => cmd_drill(&rom, s, &scenarios, trials),
            None => Err("drill needs --state".into()),
        },
        other => Err(format!("unknown subcommand {other:?}")),
    };
    if let Err(e) = r {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

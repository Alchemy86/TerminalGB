//! The measurement layer behind `examples/gen1_paper_claims.rs` and
//! `testharness/paperclaims.rs`.
//!
//! Shared because the two need to agree *by construction*: the example is the
//! instrument that produced the figures in
//! [`docs/gen1/paper-claims.md`](../../docs/gen1/paper-claims.md), and the
//! harness is the gate that stops those figures rotting. Two copies of a
//! measurement are two measurements.
//!
//! Nothing here reads `terminalgb-gen1-battle`. Every number this module
//! produces is a byte the cartridge wrote to its own work RAM.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use terminalgb::gameboy::Gameboy;
// **`terminalgb::KeypadKey`, not `terminalgb_plugin_api::KeypadKey`.**
// A `harness = false` test target can resolve the plugin-api crate to a second
// rlib, and then the two spellings are different types with the same name —
// `Gameboy::keydown` stops accepting the key with an error that reads like a
// broken signature. Going through the re-export cannot pick the wrong one.
use terminalgb::KeypadKey;
use terminalgb_plugin_battle_control::{preset::PresetFile, wram, BattlePlugin};

/// The preset file both callers use, compiled in so a test needs no path.
pub const PRESETS: &str = include_str!("../gen1_paper_claims.preset");

/// How much measuring to do. The example fills this from the command line; the
/// harness picks numbers that gate without taking all afternoon.
pub struct Config {
    pub trials: u32,
    pub frames: u32,
    pub crit_frames: u32,
    pub verbose: bool,
    pub trace: bool,
}

// ---------------------------------------------------------------------------
// The addresses. Every one of these is `pret/pokered`'s own symbol, taken from
// a build of the disassembly whose `pokeblue.gbc` is byte-identical to the
// cartridge this harness plays (sha1 d7037c83e1ae5b39bde3c30787637ba1d4c48ce2,
// `roms.sha1` line 2), and every one appears in `third_party/atlasgb`'s
// `pokemon-rb` atlas with `rom,live` evidence. They are not guesses.
// ---------------------------------------------------------------------------

/// `wPlayerBattleStatus1` — `STORING_ENERGY`(0) `THRASHING_ABOUT`(1)
/// `ATTACKING_MULTIPLE_TIMES`(2) `FLINCHED`(3) `CHARGING_UP`(4)
/// `USING_TRAPPING_MOVE`(5) `INVULNERABLE`(6) `CONFUSED`(7).
pub const PLAYER_BATTLE_STATUS1: u16 = 0xD062;
/// `wPlayerBattleStatus2` — `USING_X_ACCURACY`(0) `PROTECTED_BY_MIST`(1)
/// `GETTING_PUMPED`(2) `HAS_SUBSTITUTE_UP`(4) `NEEDS_TO_RECHARGE`(5)
/// `USING_RAGE`(6) `SEEDED`(7).
pub const PLAYER_BATTLE_STATUS2: u16 = 0xD063;
/// `wPlayerBattleStatus3` — `BADLY_POISONED`(0) `HAS_LIGHT_SCREEN_UP`(1)
/// `HAS_REFLECT_UP`(2) `TRANSFORMED`(3).
pub const PLAYER_BATTLE_STATUS3: u16 = 0xD064;
pub const ENEMY_BATTLE_STATUS1: u16 = 0xD067;
pub const ENEMY_BATTLE_STATUS2: u16 = 0xD068;
pub const ENEMY_BATTLE_STATUS3: u16 = 0xD069;
/// `wPlayerNumAttacksLeft` — shared by Thrash, Bide, trapping and multi-hit.
pub const PLAYER_NUM_ATTACKS_LEFT: u16 = 0xD06A;
pub const PLAYER_CONFUSED_COUNTER: u16 = 0xD06B;
pub const PLAYER_TOXIC_COUNTER: u16 = 0xD06C;
/// `wPlayerDisabledMove` — high nibble the slot (1-4), low nibble turns left.
pub const PLAYER_DISABLED_MOVE: u16 = 0xD06D;
pub const ENEMY_NUM_ATTACKS_LEFT: u16 = 0xD06F;
pub const ENEMY_CONFUSED_COUNTER: u16 = 0xD070;
pub const ENEMY_TOXIC_COUNTER: u16 = 0xD071;
pub const ENEMY_DISABLED_MOVE: u16 = 0xD072;
/// `wCriticalHitOrOHKO` — 0 none, 1 critical hit, 2 OHKO.
pub const CRITICAL_HIT_OR_OHKO: u16 = 0xD05E;

/// Bit positions inside the three status bytes (`constants/battle_constants.asm`).
pub const BIT_STORING_ENERGY: u8 = 0;
pub const BIT_THRASHING_ABOUT: u8 = 1;
pub const BIT_USING_TRAPPING_MOVE: u8 = 5;
pub const BIT_CONFUSED: u8 = 7;
pub const BIT_GETTING_PUMPED: u8 = 2;
pub const BIT_BADLY_POISONED: u8 = 0;
pub const BIT_HAS_LIGHT_SCREEN_UP: u8 = 1;
pub const BIT_HAS_REFLECT_UP: u8 = 2;

/// Field offsets inside a `battle_struct` we read directly.
pub const HP_OFF: u16 = 1;
pub const MAX_HP_OFF: u16 = 15;

// ---------------------------------------------------------------------------

pub fn find_rom() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let dir = Path::new(&home).join("roms");
    for e in std::fs::read_dir(dir).ok()? {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_string_lossy().to_lowercase();
        if name.ends_with(".gb") && (name.contains("blue") || name.contains("red")) {
            return Some(p);
        }
    }
    None
}

/// Copy the cartridge into `target/` before playing it.
///
/// **Never point this at `~/roms`.** The MBC writes a `.sav` beside whatever
/// path it is handed, and this harness presses buttons at menus.
pub fn stage(rom: &Path) -> Result<PathBuf, String> {
    let dir = PathBuf::from("target/gen1-paper-claims");
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let dest = dir.join(rom.file_name().ok_or("the ROM path has no filename")?);
    std::fs::copy(rom, &dest).map_err(|e| format!("cannot stage the ROM: {e}"))?;
    let _ = std::fs::remove_file(dest.with_extension("sav"));
    Ok(dest)
}

pub fn hold(gb: &mut Gameboy, key: KeypadKey, frames: u32) {
    gb.keydown(key);
    for _ in 0..frames {
        gb.frame();
    }
    gb.keyup(key);
}

pub fn run_frames(gb: &mut Gameboy, n: u32) {
    for _ in 0..n {
        gb.frame();
    }
}

pub fn menu_is_open(gb: &mut Gameboy) -> bool {
    let tiles: Vec<u8> = (0..20 * 18)
        .map(|i| gb.peek(wram::addr::TILE_MAP + i as u16))
        .collect();
    terminalgb_plugin_trainer::text::decode_screen(&tiles).contains("OPTION")
}

/// Mash through the opening to the overworld. Same route, and the same
/// reasoning, as `examples/gen1_battle_sim.rs`: alternating START and A plays
/// the intro on its own, and the menu is closed with **B**, never A.
pub fn new_game(gb: &mut Gameboy) -> Result<(), String> {
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

/// A Gen 1 battle sits at its own menu until a button arrives, so a harness
/// that only watches observes a still picture. `HandleMenuInput` delays several
/// frames after accepting a press and does not see one that arrives inside that
/// delay, so a faster mash looks exactly like dropped input.
pub const MENU_GAP: u32 = 24;

pub fn press_cadence(gb: &mut Gameboy, f: u32) {
    if f % MENU_GAP == 0 {
        gb.keydown(KeypadKey::A);
    } else if f % MENU_GAP == 4 {
        gb.keyup(KeypadKey::A);
    }
}

pub fn u16_be(gb: &mut Gameboy, addr: u16) -> u16 {
    ((gb.peek(addr) as u16) << 8) | gb.peek(addr + 1) as u16
}

pub fn bit(v: u8, b: u8) -> bool {
    v & (1 << b) != 0
}

/// One armed battle, ready to be stepped.
pub struct Battle {
    pub gb: Gameboy,
}

impl Battle {
    /// Load the overworld state, idle for `jitter` frames so the RNG is
    /// somewhere else than last time, then arm the preset.
    ///
    /// **The jitter is not decoration.** Emulation is deterministic, so two
    /// trials from the same state with the same inputs are the same trial
    /// wearing a different hat. `Random_` folds `rDIV` in on every call, so the
    /// lever is *time* — the same lever `plugins/gen1-shinyhunt` pulls.
    pub fn start(
        staged: &Path,
        rom: &[u8],
        presets: &PresetFile,
        state: &[u8],
        name: &str,
        jitter: u32,
    ) -> Result<Battle, String> {
        let mut gb = Gameboy::new(rom.to_vec(), Some(staged.to_path_buf()));
        let plugin = BattlePlugin::new(rom.to_vec(), presets.clone())
            .map_err(|e| format!("plugin: {e}"))?;
        gb.add_plugin(Box::new(plugin))
            .map_err(|e| format!("plugin: {e}"))?;
        gb.load_state(state)
            .map_err(|e| format!("save state: {e:?}"))?;
        run_frames(&mut gb, jitter);
        gb.plugin_mut::<BattlePlugin>()
            .ok_or("the plugin vanished")?
            .arm(name)
            .map_err(|e| e.to_string())?;
        Ok(Battle { gb })
    }

    pub fn in_battle(&mut self) -> bool {
        self.gb.peek(wram::addr::IS_IN_BATTLE) == 1
    }
}

/// `BattlePlugin` re-applies its opposition override until it reads back
/// intact, for up to 40 frames after the battle starts, and the send-out
/// animation writes `wBattleMon` after that. A sample taken inside that window
/// holds a half-written struct.
pub const SETTLE_FRAMES: u32 = 90;

// ---------------------------------------------------------------------------
// A duration run. One counter, one flag: record every 0 -> n transition (the
// roll the game made) and the descending run that follows.
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct Episode {
    /// The value the game rolled, as it first appeared in the counter.
    pub rolled: u8,
    /// Every distinct value the counter took, in order, including the final 0.
    pub run: Vec<u8>,
    /// Turn-starts on which the counter decremented to a NONZERO value, i.e.
    /// the turns on which the effect was still in force after its own tick.
    pub nonzero_ticks: u32,
    /// Turn-starts on which the counter decremented at all.
    pub ticks: u32,
    /// Whether the associated status bit was set for the whole run.
    pub flag_held: bool,
    /// The flag went down while the counter still had turns left.
    pub ended_early: bool,
    /// `(frame, value, status byte)` at every change — only filled under
    /// `--trace`, because an anomaly in a duration run is a false accusation
    /// waiting to happen and guessing at one is worse than measuring it.
    pub trace: Vec<(u32, u8, u8)>,
}

pub struct DurationRun {
    pub label: &'static str,
    pub counter: u16,
    /// The status byte and bit that says the effect is in force, if there is
    /// one. Confusion and Disable have their own bit; a `None` here means the
    /// counter is the only witness.
    pub flag: Option<(u16, u8)>,
    /// Read only the low nibble (Disable packs the slot in the high one).
    pub low_nibble: bool,
    pub episodes: Vec<Episode>,
    /// For Disable: the slots the game picked, and how often.
    pub slots: BTreeMap<u8, u32>,
}

impl DurationRun {
    pub fn new(
        label: &'static str,
        counter: u16,
        flag: Option<(u16, u8)>,
        low_nibble: bool,
    ) -> Self {
        DurationRun {
            label,
            counter,
            flag,
            low_nibble,
            episodes: Vec::new(),
            slots: BTreeMap::new(),
        }
    }

    /// **An episode is a FLAG interval, not a counter interval, and that is not
    /// a stylistic choice.**
    ///
    /// A trapping move that MISSES clears `USING_TRAPPING_MOVE`
    /// (`MoveHitTest`'s `.moveMissed`) and leaves `wPlayerNumAttacksLeft`
    /// standing. Keyed on the counter, the next Wrap's fresh roll then looks
    /// like a decrement of the old one — measured here as a counter going
    /// `4 -> 2` in one step, which would have been reported as a duration this
    /// game has never rolled. Keyed on the flag, the same data reads correctly
    /// and the early end is recorded as what it is.
    pub fn sample(
        &mut self,
        gb: &mut Gameboy,
        open: &mut Option<Episode>,
        frame: u32,
        trace: bool,
    ) {
        let raw = gb.peek(self.counter);
        let v = if self.low_nibble { raw & 0x0F } else { raw };
        let status = self.flag.map(|(a, _)| gb.peek(a)).unwrap_or(raw);
        // With no status bit of its own — Disable — the counter IS the flag:
        // `.AnyMoveDisabledCheck` tests the byte for zero and nothing else.
        let flag_on = self.flag.map(|(a, b)| bit(gb.peek(a), b)).unwrap_or(v != 0);

        match open.as_mut() {
            None => {
                if flag_on && v != 0 {
                    if self.low_nibble {
                        *self.slots.entry(raw >> 4).or_insert(0) += 1;
                    }
                    *open = Some(Episode {
                        rolled: v,
                        run: vec![v],
                        nonzero_ticks: 0,
                        ticks: 0,
                        flag_held: true,
                        ended_early: false,
                        trace: if trace {
                            vec![(frame, v, status)]
                        } else {
                            Vec::new()
                        },
                    });
                }
            }
            Some(ep) => {
                let last = *ep.run.last().unwrap();
                if v != last {
                    ep.run.push(v);
                    if trace {
                        ep.trace.push((frame, v, status));
                    }
                    if v < last {
                        ep.ticks += 1;
                        if v != 0 {
                            ep.nonzero_ticks += 1;
                        }
                    }
                }
                if !flag_on {
                    // The interval is over. It ended EARLY if the counter still
                    // had turns in it — the missed-trapping-move case above,
                    // and the only way a lock stops before its own countdown.
                    let mut done = open.take().unwrap();
                    done.ended_early = *done.run.last().unwrap() != 0;
                    self.episodes.push(done);
                }
            }
        }
    }

    pub fn report(&self, verbose: bool) {
        let mut hist: BTreeMap<u8, u32> = BTreeMap::new();
        for e in &self.episodes {
            *hist.entry(e.rolled).or_insert(0) += 1;
        }
        let n = self.episodes.len();
        if n == 0 {
            println!("  {:<26} NO SAMPLES", self.label);
            return;
        }
        let lo = *hist.keys().next().unwrap();
        let hi = *hist.keys().last().unwrap();
        let mut ticks: BTreeMap<u32, u32> = BTreeMap::new();
        let mut nz: BTreeMap<u32, u32> = BTreeMap::new();
        let early = self.episodes.iter().filter(|e| e.ended_early).count();
        for e in self.episodes.iter().filter(|e| !e.ended_early) {
            *ticks.entry(e.ticks).or_insert(0) += 1;
            *nz.entry(e.nonzero_ticks).or_insert(0) += 1;
        }
        println!(
            "  {:<26} {n} episode(s); rolled {lo}-{hi}  {}",
            self.label,
            hist.iter()
                .map(|(k, v)| format!("{k}x{v}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        println!(
            "  {:<26}   decrements per episode: {}   of which left it nonzero: {}",
            "",
            ticks
                .iter()
                .map(|(k, v)| format!("{k}x{v}"))
                .collect::<Vec<_>>()
                .join(" "),
            nz.iter()
                .map(|(k, v)| format!("{k}x{v}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        if early > 0 {
            println!(
                "  {:<26}   {early} ended EARLY (the flag went down with turns left) and are \n  {:<26}   excluded from the two histograms above",
                "", ""
            );
        }
        if !self.slots.is_empty() {
            println!(
                "  {:<26}   move slot chosen: {}",
                "",
                self.slots
                    .iter()
                    .map(|(k, v)| format!("{k}x{v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        if verbose {
            for (i, e) in self.episodes.iter().enumerate() {
                println!(
                    "      [{i:>3}] rolled {} run {:?}{}",
                    e.rolled,
                    e.run,
                    if e.ended_early { "  ENDED EARLY" } else { "" }
                );
                if !e.trace.is_empty() {
                    println!(
                        "            {}",
                        e.trace
                            .iter()
                            .map(|(f, v, s)| format!("f{f}={v}/${s:02X}"))
                            .collect::<Vec<_>>()
                            .join(" ")
                    );
                }
            }
        }
    }

    pub fn summary(&self) -> (usize, u8, u8) {
        let lo = self.episodes.iter().map(|e| e.rolled).min().unwrap_or(0);
        let hi = self.episodes.iter().map(|e| e.rolled).max().unwrap_or(0);
        (self.episodes.len(), lo, hi)
    }
}

/// One duration claim: which battle produces it, which counter carries it, and
/// which status bit says it is in force.
///
/// A named struct rather than a tuple because the shape is `(u16, Option<(u16,
/// u8)>, bool)` and a reader has no chance of telling the counter address from
/// the flag address at a call site.
pub struct DurationClaim {
    /// The `--claim` word.
    pub key: &'static str,
    pub preset: &'static str,
    pub label: &'static str,
    pub counter: u16,
    /// The status byte and bit that says the effect is in force. `None` means
    /// the counter is its own flag — Disable, whose check is `and a; jr z`.
    pub flag: Option<(u16, u8)>,
    /// Read only the low nibble; Disable packs the slot into the high one.
    pub low_nibble: bool,
    /// The roll range measured on the retail cartridge. A value outside it is a
    /// finding, not noise — see `docs/gen1/paper-claims.md`.
    pub rolls: (u8, u8),
}

/// The five the paper names, in the order that page reports them.
pub const DURATION_CLAIMS: [DurationClaim; 5] = [
    DurationClaim {
        key: "confusion",
        preset: "claim-confusion",
        label: "Confusion (enemy)",
        counter: ENEMY_CONFUSED_COUNTER,
        flag: Some((ENEMY_BATTLE_STATUS1, BIT_CONFUSED)),
        low_nibble: false,
        rolls: (2, 5),
    },
    DurationClaim {
        key: "disable",
        preset: "claim-disable",
        label: "Disable (enemy)",
        counter: ENEMY_DISABLED_MOVE,
        flag: None,
        low_nibble: true,
        rolls: (1, 8),
    },
    DurationClaim {
        key: "thrash",
        preset: "claim-thrash",
        label: "Thrash lock (player)",
        counter: PLAYER_NUM_ATTACKS_LEFT,
        flag: Some((PLAYER_BATTLE_STATUS1, BIT_THRASHING_ABOUT)),
        low_nibble: false,
        rolls: (2, 3),
    },
    DurationClaim {
        key: "bide",
        preset: "claim-bide",
        label: "Bide (player)",
        counter: PLAYER_NUM_ATTACKS_LEFT,
        flag: Some((PLAYER_BATTLE_STATUS1, BIT_STORING_ENERGY)),
        low_nibble: false,
        rolls: (2, 3),
    },
    DurationClaim {
        key: "trapping",
        preset: "claim-trapping",
        label: "Partial trapping (player)",
        counter: PLAYER_NUM_ATTACKS_LEFT,
        flag: Some((PLAYER_BATTLE_STATUS1, BIT_USING_TRAPPING_MOVE)),
        low_nibble: false,
        rolls: (1, 4),
    },
];

/// Play `trials` battles of one preset, watching one counter.
#[allow(clippy::too_many_arguments)]
pub fn measure_duration(
    staged: &Path,
    rom: &[u8],
    presets: &PresetFile,
    state: &[u8],
    claim: &DurationClaim,
    args: &Config,
) -> Result<DurationRun, String> {
    let mut run =
        DurationRun::new(claim.label, claim.counter, claim.flag, claim.low_nibble);
    for trial in 0..args.trials {
        let mut b =
            Battle::start(staged, rom, presets, state, claim.preset, trial * 7 + 1)?;
        let mut began: Option<u32> = None;
        let mut open: Option<Episode> = None;
        for f in 0..args.frames {
            press_cadence(&mut b.gb, f);
            b.gb.frame();
            let now = b.in_battle();
            if now && began.is_none() {
                began = Some(f);
            }
            if !now {
                began = None;
                open = None;
                continue;
            }
            if began.map(|s| f >= s + SETTLE_FRAMES).unwrap_or(false) {
                run.sample(&mut b.gb, &mut open, f, args.trace);
            }
        }
    }
    Ok(run)
}

// ---------------------------------------------------------------------------
// Toxic: the residual formula, tick by tick.
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ToxicTick {
    pub counter: u8,
    pub hp_before: u16,
    pub hp_after: u16,
    pub max_hp: u16,
    /// `BADLY_POISONED` in `wEnemyBattleStatus3` at the moment of the tick.
    /// The counter is only *used* while this is set, so a tick recorded with it
    /// clear would be measuring something else.
    pub badly_poisoned: bool,
}

pub fn measure_toxic(
    staged: &Path,
    rom: &[u8],
    presets: &PresetFile,
    state: &[u8],
    preset: &str,
    args: &Config,
) -> Result<Vec<ToxicTick>, String> {
    let mut ticks = Vec::new();
    for trial in 0..args.trials {
        let mut b = Battle::start(staged, rom, presets, state, preset, trial * 7 + 1)?;
        let mut began: Option<u32> = None;
        let mut prev_counter = 0u8;
        let mut prev_hp = 0u16;
        for f in 0..args.frames {
            press_cadence(&mut b.gb, f);
            b.gb.frame();
            let now = b.in_battle();
            if now && began.is_none() {
                began = Some(f);
            }
            if !now {
                began = None;
                prev_counter = 0;
                continue;
            }
            if !began.map(|s| f >= s + SETTLE_FRAMES).unwrap_or(false) {
                continue;
            }
            let counter = b.gb.peek(ENEMY_TOXIC_COUNTER);
            let hp = u16_be(&mut b.gb, wram::addr::ENEMY_MON + HP_OFF);
            let max_hp = u16_be(&mut b.gb, wram::addr::ENEMY_MON + MAX_HP_OFF);
            // The counter is incremented inside the same routine that subtracts
            // the damage, so the frame the counter moves is the frame the HP
            // moves. Read the drop across that one frame.
            if counter > prev_counter && prev_hp > 0 && max_hp > 0 {
                ticks.push(ToxicTick {
                    counter,
                    hp_before: prev_hp,
                    hp_after: hp,
                    max_hp,
                    badly_poisoned: bit(
                        b.gb.peek(ENEMY_BATTLE_STATUS3),
                        BIT_BADLY_POISONED,
                    ),
                });
            }
            prev_counter = counter;
            prev_hp = hp;
        }
    }
    Ok(ticks)
}

// ---------------------------------------------------------------------------
// Screens: where the bit lives, and whether a switch keeps it.
// ---------------------------------------------------------------------------

pub struct ScreenResult {
    pub label: &'static str,
    /// Did the bit ever come up at all?
    pub raised: bool,
    /// The whole of `wPlayerBattleStatus3` while it was up.
    pub status3_while_up: u8,
    /// `$D060..=$D069` — `wPlayerStatsToDouble` through `wEnemyBattleStatus3`,
    /// which is exactly the block `SendOutMon` zeroes — before the switch...
    pub block_before: Vec<u8>,
    /// ...and after it.
    pub block_after: Vec<u8>,
    /// Was the bit still up after the player switched Pokémon?
    pub survived_switch: Option<bool>,
    /// The other side's status3 while ours was up — a *side* condition in a
    /// later generation would be visible to neither side's volatile block.
    pub enemy_status3_while_up: u8,
}

/// What the game has drawn, as text. Gen 1 loads its font so a tile id **is**
/// the character it draws, so the tilemap reads back as the screen.
pub fn screen(gb: &mut Gameboy) -> String {
    let tiles: Vec<u8> = (0..20 * 18)
        .map(|i| gb.peek(wram::addr::TILE_MAP + i as u16))
        .collect();
    terminalgb_plugin_trainer::text::decode_screen(&tiles)
}

/// Idle (or advance text with A) until the screen says `needle`.
///
/// **Navigate by reading the screen, not by counting frames.** The same rule
/// `examples/gen1_trade.rs` records: a fixed button script is hostage to
/// animation length, and a menu that arrives one frame late turns a working
/// route into a silent no-op.
pub fn wait_for(gb: &mut Gameboy, needle: &str, max_frames: u32, nudge: bool) -> bool {
    for f in 0..max_frames {
        if screen(gb).contains(needle) {
            gb.keyup(KeypadKey::A);
            run_frames(gb, 4);
            return true;
        }
        if nudge {
            if f % MENU_GAP == 0 {
                gb.keydown(KeypadKey::A);
            } else if f % MENU_GAP == 4 {
                gb.keyup(KeypadKey::A);
            }
        }
        gb.frame();
    }
    gb.keyup(KeypadKey::A);
    false
}

pub fn tap(gb: &mut Gameboy, key: KeypadKey) {
    hold(gb, key, 6);
    run_frames(gb, 20);
}

/// One screen to measure: the battle that raises it, and the bit it sets.
pub struct ScreenClaim {
    pub preset: &'static str,
    pub label: &'static str,
    pub bit: u8,
}

/// The two the paper names, in the order this page reports them.
pub const SCREEN_CLAIMS: [ScreenClaim; 2] = [
    ScreenClaim {
        preset: "claim-reflect",
        label: "Reflect",
        bit: BIT_HAS_REFLECT_UP,
    },
    ScreenClaim {
        preset: "claim-light-screen",
        label: "Light Screen",
        bit: BIT_HAS_LIGHT_SCREEN_UP,
    },
];

pub fn measure_screen(
    staged: &Path,
    rom: &[u8],
    presets: &PresetFile,
    state: &[u8],
    claim: &ScreenClaim,
    args: &Config,
) -> Result<ScreenResult, String> {
    let (preset, label, bit_no) = (claim.preset, claim.label, claim.bit);
    let mut b = Battle::start(staged, rom, presets, state, preset, 3)?;
    let mut began: Option<u32> = None;
    let mut raised = false;
    let mut status3_while_up = 0u8;
    let mut enemy_status3_while_up = 0u8;

    // Phase 1 — use the screen move (slot 1) until the bit is up.
    for f in 0..args.frames {
        press_cadence(&mut b.gb, f);
        b.gb.frame();
        let now = b.in_battle();
        if now && began.is_none() {
            began = Some(f);
        }
        if !now {
            began = None;
            continue;
        }
        if !began.map(|s| f >= s + SETTLE_FRAMES).unwrap_or(false) {
            continue;
        }
        let s3 = b.gb.peek(PLAYER_BATTLE_STATUS3);
        if bit(s3, bit_no) {
            raised = true;
            status3_while_up = s3;
            enemy_status3_while_up = b.gb.peek(ENEMY_BATTLE_STATUS3);
            break;
        }
    }
    if !raised {
        return Ok(ScreenResult {
            label,
            raised,
            status3_while_up,
            block_before: Vec::new(),
            block_after: Vec::new(),
            survived_switch: None,
            enemy_status3_while_up,
        });
    }

    // The five bytes `SendOutMon` zeroes, before and after: a screen that
    // belonged to the SIDE would have to be recorded somewhere else, because
    // this block is per-active by construction.
    let block_before: Vec<u8> = (0xD060u16..=0xD069).map(|a| b.gb.peek(a)).collect();

    // Phase 2 — get to the battle menu, choose PKMN, and switch to the second
    // party member. Every step waits for the screen to say it has arrived.
    b.gb.keyup(KeypadKey::A);
    // The move menu may be open from the cadence; B backs out of it and can
    // never open one, which is why it is B and not A.
    for _ in 0..3 {
        if screen(&mut b.gb).contains("FIGHT") {
            break;
        }
        tap(&mut b.gb, KeypadKey::B);
    }
    let at_menu = wait_for(&mut b.gb, "FIGHT", 1200, true);
    let mut route = vec![format!("battle menu: {at_menu}")];
    if at_menu {
        // The Gen 1 battle menu is `FIGHT PKMN` over `ITEM RUN`, so PKMN is to
        // the RIGHT of FIGHT. Pressing Down takes ITEM and opens the bag, whose
        // CANCEL entry looks enough like a party list to fool a needle search —
        // which is exactly what the first version of this did.
        tap(&mut b.gb, KeypadKey::Right);
        tap(&mut b.gb, KeypadKey::A);
        // The party list opens on the active Pokémon; the reserve is one down.
        let at_party = wait_for(&mut b.gb, "Choose a POK", 300, false);
        if args.trace {
            let n = b.gb.peek(wram::addr::PARTY_COUNT);
            let sp: Vec<u8> = (0..6)
                .map(|i| b.gb.peek(wram::addr::PARTY_SPECIES + i))
                .collect();
            println!(
                "      party count {n} species {sp:?} active ${:02X}",
                b.gb.peek(wram::addr::BATTLE_MON)
            );
            let sc = screen(&mut b.gb);
            for r in 0..18 {
                println!("      | {}", &sc[r * 20..(r + 1) * 20]);
            }
        }
        route.push(format!("party list: {at_party}"));
        tap(&mut b.gb, KeypadKey::Down);
        tap(&mut b.gb, KeypadKey::A);
        // SWITCH / STATS / CANCEL, with SWITCH first.
        let at_submenu = wait_for(&mut b.gb, "SWITCH", 300, false);
        route.push(format!("switch submenu: {at_submenu}"));
        tap(&mut b.gb, KeypadKey::A);
        run_frames(&mut b.gb, 400);
    }
    if args.verbose {
        println!("      route: {}", route.join(" / "));
    }

    let switched =
        b.gb.peek(wram::addr::BATTLE_MON) != b.gb.peek(wram::addr::PARTY_SPECIES);
    let after_bit = bit(b.gb.peek(PLAYER_BATTLE_STATUS3), bit_no);
    let block_after: Vec<u8> = (0xD060u16..=0xD069).map(|a| b.gb.peek(a)).collect();

    Ok(ScreenResult {
        label,
        raised,
        status3_while_up,
        block_before,
        block_after,
        survived_switch: if switched { Some(after_bit) } else { None },
        enemy_status3_while_up,
    })
}

// ---------------------------------------------------------------------------
// The critical-hit rate, with and without Focus Energy.
//
// The attacker is the OPPOSITION on purpose. It picks a move at random from its
// four slots, so a Focus Energy in slot 1 lands early and stays up (nothing
// clears `GETTING_PUMPED` short of a switch), and the other three slots keep
// attacking. The player's own move does nothing, so nobody faints and the
// sample size is bounded only by the frame budget.
// ---------------------------------------------------------------------------

pub struct CritRun {
    pub attacks: u32,
    pub crits: u32,
    /// Why the sampling stopped, and where the two HP bars were left. A crit
    /// rate over a battle that ended early is a rate over a truncated sample,
    /// and a harness that does not say so invites a wrong conclusion.
    pub stopped: String,
    /// Attacks seen before `GETTING_PUMPED` came up, discarded from the count.
    pub before_pump: u32,
    pub pumped: bool,
    pub battles: u32,
}

pub fn measure_crit(
    staged: &Path,
    rom: &[u8],
    presets: &PresetFile,
    state: &[u8],
    preset: &str,
    want_focus_energy: bool,
    args: &Config,
) -> Result<CritRun, String> {
    let mut out = CritRun {
        attacks: 0,
        crits: 0,
        before_pump: 0,
        pumped: false,
        battles: 0,
        stopped: String::new(),
    };
    // **One battle is worth about forty attacks and no more, and the reason is
    // PP.** The player's Splash has the game's maximum 40 PP; when slot 1 runs
    // out the move menu refuses it and the battle stands still, so a longer
    // frame budget buys nothing. Measured: 20 attacks in 12,000 frames, 40 in
    // 60,000, and still 40 in 120,000. The sample therefore comes from many
    // battles rather than one long one.
    for trial in 0..args.trials {
        let mut b = Battle::start(staged, rom, presets, state, preset, trial * 11 + 3)?;
        out.battles += 1;
        let mut began: Option<u32> = None;
        let mut prev_hp = 0u16;
        for f in 0..args.crit_frames {
            press_cadence(&mut b.gb, f);
            b.gb.frame();
            let now = b.in_battle();
            if now && began.is_none() {
                began = Some(f);
            }
            if !now {
                if began.is_some() {
                    out.stopped = format!(
                        "battle {trial} ended at frame {f}: player HP {} / {}",
                        u16_be(&mut b.gb, wram::addr::BATTLE_MON + HP_OFF),
                        u16_be(&mut b.gb, wram::addr::BATTLE_MON + MAX_HP_OFF),
                    );
                    break;
                }
                continue;
            }
            if !began.map(|s| f >= s + SETTLE_FRAMES).unwrap_or(false) {
                continue;
            }
            let pumped = bit(b.gb.peek(ENEMY_BATTLE_STATUS2), BIT_GETTING_PUMPED);
            out.pumped |= pumped;
            let hp = u16_be(&mut b.gb, wram::addr::BATTLE_MON + HP_OFF);
            if prev_hp > 0 && hp < prev_hp {
                // The player's own move deals no damage and the opposition's is
                // the only thing that can move this bar, so a drop is one
                // attack, and `wCriticalHitOrOHKO` still holds that attack's
                // verdict: the player is faster and its own Splash zeroed the
                // flag *before* the attack, not after.
                if want_focus_energy && !pumped {
                    out.before_pump += 1;
                } else {
                    out.attacks += 1;
                    if b.gb.peek(CRITICAL_HIT_OR_OHKO) == 1 {
                        out.crits += 1;
                    }
                }
            }
            prev_hp = hp;
        }
    }
    Ok(out)
}

/// Wilson score interval, so a rate over a few hundred samples is reported as a
/// measurement rather than as a number. The normal approximation degenerates at
/// the ends, which is exactly where a 1/4 rate lives.
pub fn wilson(k: u32, n: u32) -> (f64, f64) {
    wilson_at(k, n, 1.96)
}

/// The same interval at a chosen `z`.
///
/// A **gate** wants a wide one. At 95 % a passing check still fails one time in
/// twenty by luck, and a test that cries wolf every twentieth run gets loosened
/// until it means nothing; `testharness/paperclaims.rs` uses z = 3.29 (99.9 %),
/// which still separates the measured rate from four times itself by a mile.
pub fn wilson_at(k: u32, n: u32, z: f64) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let p = k as f64 / n;
    let d = 1.0 + z * z / n;
    let c = p + z * z / (2.0 * n);
    let s = z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt();
    ((c - s) / d, (c + s) / d)
}

// ---------------------------------------------------------------------------

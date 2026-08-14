//! **Cable Club Colosseum harness** — the regression for a crash that destroyed
//! save files.
//!
//! ## The bug this exists to keep fixed
//!
//! `CableClub_DoBattleOrTrade` (`pret/pokered`, `engine/link/cable_club.asm`) is
//! one function for both Cable Club rooms. Trade Centre and Colosseum exchange
//! byte-for-byte the same 644 transfers and diverge only afterwards, on
//! `wLinkState` — a value that never crosses the cable. So the virtual link
//! partner, which only knew how to trade, walked into a **real link battle** and
//! answered each turn with a trade answer.
//!
//! `SelectEnemyMove` bounds-checks nothing it receives. Turn 2 of the trade
//! state machine says "confirm", which reads as *move slot 2*; a Pokémon with
//! fewer than three moves has nothing there, and `GetCurrentMove` then does
//! `dec a; AddNTimes Moves, 6` on move id 0 — 1,530 bytes past a 165-entry
//! table — and `_JumpMoveEffect` `jp hl`s into the result. The CPU leaves the
//! program. With the emulator's **shipped default offer** (a level 5 Mew, one
//! move) this happened deterministically at frame 4038, and within six frames
//! the runaway CPU had changed **4,378 bytes of cartridge RAM**, which the
//! battery autosave then wrote over the player's save file.
//!
//! No public test ROM covers any of this: Blargg, Mooneye and acid2 test one
//! console in isolation and nothing in the suite plugs a second Game Boy in.
//! That gap is itself the finding.
//!
//! ## What it checks
//!
//! Two configurations, each driven from the title screen through the link
//! receptionist into the Colosseum:
//!
//! 1. **the shipped default offer** — the exact configuration that crashed;
//! 2. **a two-Pokémon party** — which reaches the *other* unbounded value,
//!    `EnemySendOutFirstMon`'s `sub 4`, where a move index answered at a
//!    replacement rendezvous makes the game field party slot 252 as a
//!    MissingNo-class glitch Pokémon.
//!
//! and asserts, for both:
//!
//! * `wLinkState` only ever holds a real `LINK_STATE_*` value — the game never
//!   leaves its own code;
//! * nothing reported a machine fault;
//! * the battery file is byte-identical from the moment the battle starts;
//! * the partner is in its battle phase and has counted **no** trade (it used to
//!   report a phantom trade during every battle);
//! * the party the partner decoded out of the game's half of the exchange is the
//!   player's real team — `seen_party()` had never once worked against a
//!   cartridge.
//!
//! ## Running it
//!
//! ```text
//! cargo test --release --test colosseum
//! ```
//!
//! It needs a cartridge (`POKE_ROM=<path>`, or one in `~/roms`) and a save
//! standing in the Viridian Pokémon Center — `GB_CABLECLUB_SAVE=<path>`, or the
//! fixture `cargo run --release --example gen1_trade -- --rom <rom> --make-save`
//! leaves in the temp directory. Without both it skips with exit 0 and says how
//! to produce them. `COLOSSEUM_VERBOSE=1` narrates the run;
//! `COLOSSEUM_WIRE=<path>` dumps every serial byte with its frame.
//!
//! **Never point it at a real save.** It stages the cartridge *and* the save
//! into `target/` first, because the MBC writes `.sav` next to the ROM and this
//! harness presses buttons at menus.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use terminalgb::gameboy::Gameboy;
use terminalgb::plugin::{Capability, Event, Machine, Plugin};
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_link::{protocol::Phase, spec::TradeSpec, Gen1LinkPartner};
use terminalgb_plugin_trainer::Game;

// --- WRAM addresses --------------------------------------------------------
// The first group is copied from `examples/gen1_trade.rs` / `src/trainer/gen1.rs`,
// where they are already cross-checked against this cartridge.
const W_PARTY_COUNT: u16 = 0xD163;
const W_PARTY_MON_NICKS: u16 = 0xD2B5;
const W_CUR_MAP: u16 = 0xD35E;
const W_Y_COORD: u16 = 0xD361;
const W_X_COORD: u16 = 0xD362;
const W_LINK_STATE: u16 = 0xD12B;
const W_IS_IN_BATTLE: u16 = 0xD057;
const W_ENEMY_MON_NICK: u16 = 0xCFDA;
const NAME_LEN: usize = 11;

const MAP_VIRIDIAN_POKECENTER: u8 = 0x29;
const MAP_TRADE_CENTER: u8 = 0xEF;
const MAP_COLOSSEUM: u8 = 0xF0;

const RECEPTIONIST_X: u8 = 11;
const RECEPTIONIST_Y: u8 = 3;
const W_WARP_ENTRIES: u16 = 0xD3AF;

// --- the observer ----------------------------------------------------------

#[derive(Clone, Copy)]
struct WireByte {
    frame: u64,
    sent: u8,
    received: u8,
}

#[derive(Default)]
struct WireLogInner {
    frame: u64,
    bytes: Vec<WireByte>,
}

/// Observe-only plugin: records every completed serial exchange with the frame
/// it landed on. Declares nothing but `OBSERVE`, so the host would refuse it a
/// peek or a poke.
struct WireLog(Arc<Mutex<WireLogInner>>);

impl Plugin for WireLog {
    fn name(&self) -> &str {
        "wire-log"
    }
    fn capabilities(&self) -> Capability {
        Capability::OBSERVE
    }
    fn status(&self) -> String {
        format!("{} bytes", self.0.lock().unwrap().bytes.len())
    }
    fn on_event(&mut self, event: &Event, _m: &mut Machine) {
        let mut g = self.0.lock().unwrap();
        match *event {
            Event::Frame { number } => g.frame = number,
            Event::SerialTransfer { sent, received } => {
                let frame = g.frame;
                g.bytes.push(WireByte {
                    frame,
                    sent,
                    received,
                });
            }
            // `Event` is `#[non_exhaustive]`: a host may deliver a category
            // built after this code, and ignoring it is the correct answer.
            _ => {}
        }
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

// ---------------------------------------------------------------------------
// Locating the cartridge and the save fixture
// ---------------------------------------------------------------------------

/// Read the cartridge header directly, without booting anything.
fn identify(path: &Path) -> Option<Game> {
    let data = std::fs::read(path).ok()?;
    if data.len() < 0x150 {
        return None;
    }
    let title: String = data[0x134..0x144]
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| b as char)
        .collect();
    let checksum = ((data[0x14E] as u16) << 8) | data[0x14F] as u16;
    Game::detect(&title, checksum)
}

fn find_rom() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("POKE_ROM") {
        let p = PathBuf::from(p);
        return identify(&p).map(|_| p);
    }
    let mut dirs = vec![PathBuf::from("tests"), PathBuf::from("roms")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("roms"));
    }
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.extension().map(|x| x == "gb").unwrap_or(false) && identify(&p).is_some()
            {
                return Some(p);
            }
        }
    }
    None
}

/// A save standing in the Viridian Pokémon Center with a party — the state the
/// Cable Club needs. Produced by `gen1_trade --make-save`, never taken from a
/// player's own save directory.
fn find_save() -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(p) = std::env::var("GB_CABLECLUB_SAVE") {
        candidates.push(PathBuf::from(p));
    }
    candidates.push(std::env::temp_dir().join("gameboy-gen1-trade/cableclub.sav"));
    candidates.into_iter().find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// One run
// ---------------------------------------------------------------------------

struct Outcome {
    /// Every distinct value `wLinkState` held, in order.
    link_states: Vec<u8>,
    /// The first value that is not a real `LINK_STATE_*`, if any.
    bad_link_state: Option<(u32, u8)>,
    /// What the emulator itself concluded, if anything.
    fault: Option<String>,
    reached_battle: bool,
    phase: Phase,
    trades_completed: u32,
    battle_turns: u32,
    seen_party: Option<(String, Vec<String>)>,
    /// The battery file at the moment the battle started, and at the end.
    sav_at_battle: Vec<u8>,
    sav_at_end: Vec<u8>,
    /// Enemy nicknames seen on screen, in order — the glitch-Pokémon detector.
    enemy_nicks: Vec<String>,
}

fn run(
    rom_path: &Path,
    save_path: &Path,
    tag: &str,
    spec: TradeSpec,
    frames: u32,
) -> Outcome {
    let verbose = std::env::var("COLOSSEUM_VERBOSE").is_ok();
    // Stage both the cartridge and the save into target/: the MBC writes `.sav`
    // next to the ROM, and this harness presses buttons at menus.
    let dir = PathBuf::from("target/colosseum-harness").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("work dir");
    let work = dir.join("colosseum.gb");
    std::fs::copy(rom_path, &work).expect("stage rom");
    std::fs::copy(save_path, work.with_extension("sav")).expect("stage save");

    let rom = std::fs::read(&work).expect("read rom");
    let mut gb = Gameboy::new(rom.clone(), Some(work.clone()));
    let partner = Gen1LinkPartner::new(rom, &spec).expect("build link partner");
    println!("  offering: {}", partner.offer());
    gb.add_plugin(Box::new(partner))
        .expect("attach link partner");

    let log = Arc::new(Mutex::new(WireLogInner::default()));
    gb.add_plugin(Box::new(WireLog(log.clone())))
        .expect("attach wire log");

    let mut bot = Bot {
        want_colosseum: true,
        ..Bot::default()
    };
    let mut out = Outcome {
        link_states: Vec::new(),
        bad_link_state: None,
        fault: None,
        reached_battle: false,
        phase: Phase::Handshake,
        trades_completed: 0,
        battle_turns: 0,
        seen_party: None,
        sav_at_battle: Vec::new(),
        sav_at_end: Vec::new(),
        enemy_nicks: Vec::new(),
    };
    let sav = work.with_extension("sav");

    for frame in 0..frames {
        bot.act(&mut gb, verbose);
        gb.frame();

        let ls = gb.peek(W_LINK_STATE);
        if out.link_states.last() != Some(&ls) {
            out.link_states.push(ls);
            if verbose {
                println!("  [f{frame:6}] wLinkState = {ls:#04x}");
            }
        }
        // `wLinkState` holds nothing but LINK_STATE_* while the game is running
        // its own code. Only meaningful once the link is up: this emulator boots
        // with randomised WRAM, so before then the byte is noise.
        if out.bad_link_state.is_none() && out.reached_battle && !valid_link_state(ls) {
            out.bad_link_state = Some((frame, ls));
        }
        if !out.reached_battle && ls == LINK_STATE_BATTLING {
            out.reached_battle = true;
            // Force the battery out so this is a measurement of cartridge RAM,
            // not of the autosave's 60-quiet-frame timer.
            gb.flush_battery();
            out.sav_at_battle = std::fs::read(&sav).unwrap_or_default();
        }
        if out.reached_battle && gb.peek(W_IS_IN_BATTLE) != 0 {
            let nick = decode(&gb.peek_range(W_ENEMY_MON_NICK, NAME_LEN));
            if !nick.trim().is_empty() && out.enemy_nicks.last() != Some(&nick) {
                out.enemy_nicks.push(nick);
            }
        }
        if out.fault.is_none() {
            out.fault = gb.fault().map(str::to_string);
        }
    }

    gb.flush_battery();
    out.sav_at_end = std::fs::read(&sav).unwrap_or_default();
    if let Some(p) = gb.plugin::<Gen1LinkPartner>() {
        out.phase = p.phase();
        out.trades_completed = p.trades_completed();
        out.battle_turns = p.battle_turns();
        out.seen_party = p.seen_party().map(|party| {
            (
                party.trainer_name.clone(),
                party
                    .mons
                    .iter()
                    .map(|m| format!("{} Lv{}", m.nickname, m.level))
                    .collect(),
            )
        });
        if verbose {
            for e in p.log() {
                println!("  log: {e}");
            }
        }
    }
    if let Ok(path) = std::env::var("COLOSSEUM_WIRE") {
        let g = log.lock().unwrap();
        let mut s = String::new();
        for b in &g.bytes {
            s.push_str(&format!("{} {:02X} {:02X}\n", b.frame, b.sent, b.received));
        }
        let _ = std::fs::write(format!("{path}.{tag}"), s);
    }
    if verbose {
        println!("  final screen:\n{}", indent(&screen_text(&mut gb)));
    }
    out
}

/// The seven values `wLinkState` can legitimately hold
/// (`constants/serial_constants.asm`). Spelled out here rather than imported so
/// the harness states its own expectation independently of the code under test.
fn valid_link_state(v: u8) -> bool {
    matches!(v, 0x00 | 0x01 | 0x02 | 0x03 | 0x04 | 0x05 | 0x32)
}

const LINK_STATE_BATTLING: u8 = 0x04;

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

/// Shared checks: whatever else a configuration is testing, the game must stay
/// inside its own program and the save file must not move.
fn common_problems(out: &Outcome) -> Vec<String> {
    let mut p = Vec::new();
    if !out.reached_battle {
        p.push(
            "the bot never reached a link battle (wLinkState never became $04), so \
             this case proves nothing"
                .to_string(),
        );
        return p;
    }
    if let Some((frame, v)) = out.bad_link_state {
        p.push(format!(
            "wLinkState became {v:#04x} at frame {frame}, which Pokémon never writes \
             there — the CPU has left the program"
        ));
    }
    if let Some(f) = &out.fault {
        p.push(format!("the emulator reported a machine fault: {f}"));
    }
    // **The banks that hold a save must not move.** Not the whole file: SRAM
    // bank 0 is `pret/pokered`'s "Sprite Buffers" (`layout.link`, `ram/sram.asm`)
    // — scratch the game legitimately writes all through a battle's animations,
    // and asserting on it would fail for a healthy run. Banks 1-3 are "Save
    // Data", "Saved Boxes 1" and "Saved Boxes 2": the party, the badges, the
    // money, the PC boxes. A Colosseum battle writes none of them —
    // `cable_club.asm`'s battle branch heals the party and returns, and only the
    // *trade* branch calls `SavePartyAndDexData` — so any change there is
    // damage, and it is where the crashed CPU could have landed.
    const SAVE_DATA: usize = 0x2000;
    let changed = out
        .sav_at_end
        .iter()
        .zip(out.sav_at_battle.iter())
        .skip(SAVE_DATA)
        .filter(|(a, b)| a != b)
        .count();
    if changed > 0 || out.sav_at_end.len() != out.sav_at_battle.len() {
        p.push(format!(
            "the saved-game banks changed during the battle: {changed} bytes differ \
             from $2000 on (party, badges, boxes). A Colosseum battle writes nothing \
             persistent, so this is a crashed CPU reaching cartridge RAM"
        ));
    }
    if out.trades_completed != 0 {
        p.push(format!(
            "the partner counted {} trade(s) during a battle: the trade state machine \
             is still answering battle turns",
            out.trades_completed
        ));
    }
    if out.phase != Phase::Battling && out.phase != Phase::Connected {
        p.push(format!(
            "the partner ended in {:?}, not a battle phase",
            out.phase
        ));
    }
    p
}

/// The shipped default: a level 5 Mew with one move. This is the configuration
/// that crashed the game on turn 2 of the very first battle.
fn case_default_offer(rom: &Path, save: &Path) -> Case {
    println!("\n  [1] the shipped default offer (level 5 Mew, one move)");
    let out = run(rom, save, "default", TradeSpec::default(), 12_000);
    let mut problems = common_problems(&out);

    // The other half of the fix: `seen_party` decodes the game's half of the
    // exchange. It silently returned None after every real exchange before,
    // because the offsets it indexed at were one byte out.
    match &out.seen_party {
        None => problems.push(
            "the partner decoded no party out of the game's half of the exchange — \
             the receive-side framing is wrong again"
                .to_string(),
        ),
        Some((trainer, mons)) if trainer.is_empty() || mons.is_empty() => problems.push(
            format!("the decoded party is empty: trainer {trainer:?}, {mons:?}"),
        ),
        Some(_) => {}
    }

    Case {
        name: "the shipped default offer survives a Colosseum battle",
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            format!(
                "{} battle turns, wLinkState stayed in {:02X?}, save untouched, \
                 decoded the player's team as {:?}",
                out.battle_turns,
                out.link_states,
                out.seen_party.as_ref().map(|(t, m)| (t, m))
            )
        } else {
            problems.join("; ")
        },
    }
}

/// A two-Pokémon party, so the first one faints and the game asks *which
/// Pokémon comes next*. That rendezvous shares its encoding with a move choice
/// and `EnemySendOutFirstMon` computes `wWhichPokemon = received - 4` with no
/// bounds check, so answering it with a move index fields party slot 252 — a
/// glitch Pokémon whose name changes every turn.
fn case_replacement_after_a_faint(rom: &Path, save: &Path) -> Case {
    println!("\n  [2] a two-Pokémon party: the faint / replacement rendezvous");
    let spec = TradeSpec::parse(
        "trainer  = ORACLE\n\
         \n\
         species  = Rattata\n\
         level    = 3\n\
         nickname = FIRST\n\
         moves    = Growl, Leer, Tail Whip\n\
         dvs      = 0, 0, 0, 0\n\
         \n\
         species  = Chansey\n\
         level    = 50\n\
         nickname = SECOND\n\
         moves    = Growl, Leer, Tail Whip\n\
         dvs      = 15, 15, 15, 15\n",
        "two-mon party",
    )
    .expect("the fixture spec parses");

    let out = run(rom, save, "two-mon", spec, 12_000);
    let mut problems = common_problems(&out);

    // Read the answer off the game's own screen: after the first Pokémon
    // faints the second must come out, by name. A glitch Pokémon's nickname is
    // a moving window of unrelated WRAM and is never "SECOND".
    if !out.enemy_nicks.iter().any(|n| n.contains("FIRST")) {
        problems.push(format!(
            "the first Pokémon was never on screen (saw {:?})",
            out.enemy_nicks
        ));
    } else if !out.enemy_nicks.iter().any(|n| n.contains("SECOND")) {
        problems.push(format!(
            "after the first Pokémon fainted the partner did not send out the second: \
             the game showed {:?}. A move index answered at the replacement \
             rendezvous underflows `wWhichPokemon` to party slot 252",
            out.enemy_nicks
        ));
    }

    Case {
        name: "a fainted Pokémon is replaced by a real one, not a glitch",
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            format!(
                "{} battle turns; the game showed {:?} and nothing else",
                out.battle_turns, out.enemy_nicks
            )
        } else {
            problems.join("; ")
        },
    }
}

fn main() {
    let Some(rom) = find_rom() else {
        println!(
            "colosseum: no Pokémon Red/Blue cartridge found — set POKE_ROM=<path> \
             or drop one in ~/roms. Skipping (exit 0)."
        );
        return;
    };
    let Some(save) = find_save() else {
        println!(
            "colosseum: no cable-club save fixture found. Set \
             GB_CABLECLUB_SAVE=<path>, or produce one with\n  cargo run --release \
             --example gen1_trade -- --rom {} --make-save\nSkipping (exit 0).",
            rom.display()
        );
        return;
    };
    println!(
        "colosseum: cartridge {}\n           save      {}",
        rom.display(),
        save.display()
    );

    let cases = vec![
        case_default_offer(&rom, &save),
        case_replacement_after_a_faint(&rom, &save),
    ];

    let mut failed = 0;
    println!("\nCable Club Colosseum\n");
    for c in &cases {
        if c.ok {
            println!("  PASS  {}\n        {}", c.name, c.detail);
        } else {
            failed += 1;
            println!("  FAIL  {}\n        {}", c.name, c.detail);
        }
    }
    println!("\n  {}/{} passed\n", cases.len() - failed, cases.len());
    if failed > 0 {
        std::process::exit(1);
    }
}

// --- the bot ---------------------------------------------------------------

#[derive(Default)]
struct Bot {
    hold: Option<(KeypadKey, u32)>,
    cooldown: u32,
    stage: Stage,
    ticks: u32,
    stuck: u32,
    last_pos: (u8, u8),
    blocked: u32,
    spawn: Option<(u8, u8)>,
    menu_moved: bool,
    menu_downs: u32,
    want_colosseum: bool,
    /// Which move slot the player picks, per turn (cycled). Empty = always 0.
    player_moves: Vec<usize>,
    turn: usize,
    /// Down-presses already made in the move menu this turn.
    move_cursor: usize,
    in_move_menu: bool,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    #[default]
    Continue,
    EnterRoom,
    WalkToReceptionist,
    TalkAndSave,
    LinkMenu,
    InRoom,
    AtConsole,
    Battle,
}

impl Bot {
    fn press(&mut self, gb: &mut Gameboy, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((key, hold));
    }

    fn step_towards(&mut self, gb: &mut Gameboy, x: u8, y: u8, tx: u8, ty: u8) {
        let moved = (x, y) != self.last_pos;
        self.last_pos = (x, y);
        if moved {
            self.blocked = 0;
        } else {
            self.blocked += 1;
        }
        let prefer_x = (self.blocked / 3) % 2 == 0;
        let dx = match x.cmp(&tx) {
            std::cmp::Ordering::Less => Some(KeypadKey::Right),
            std::cmp::Ordering::Greater => Some(KeypadKey::Left),
            std::cmp::Ordering::Equal => None,
        };
        let dy = match y.cmp(&ty) {
            std::cmp::Ordering::Less => Some(KeypadKey::Down),
            std::cmp::Ordering::Greater => Some(KeypadKey::Up),
            std::cmp::Ordering::Equal => None,
        };
        if let Some(k) = if prefer_x { dx.or(dy) } else { dy.or(dx) } {
            self.press(gb, k, 12);
        }
    }

    fn act(&mut self, gb: &mut Gameboy, verbose: bool) {
        if let Some((key, left)) = self.hold {
            if left <= 1 {
                gb.keyup(key);
                self.hold = None;
                self.cooldown = 6;
            } else {
                self.hold = Some((key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }
        self.ticks += 1;
        let text = screen_text(gb);
        let map = gb.peek(W_CUR_MAP);
        let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
        let link = gb.peek(W_LINK_STATE);
        let in_battle = gb.peek(W_IS_IN_BATTLE);
        let set = |b: &mut Self, s: Stage| {
            if b.stage != s {
                if verbose {
                    println!("  bot: {:?} -> {:?}", b.stage, s);
                }
                b.stage = s;
                b.stuck = 0;
            }
        };

        // Only after the console has been used: WRAM is uninitialised garbage
        // at boot and `wIsInBattle` reads non-zero there.
        if self.stage == Stage::AtConsole && in_battle != 0 {
            set(self, Stage::Battle);
        }

        // --- the cable-club menu, wherever the stage machine thinks it is ---
        // `CableClubOptionsText` draws TRADE CENTER / COLOSSEUM / CANCEL with the
        // cursor at rows 7 / 9 / 11 (`engine/menus/main_menu.asm`
        // `.updateCursorPosition`). This has to override the stage machine,
        // because `wLinkState` is only set *after* the choice is made, so the
        // bot is still in its "talk to the receptionist" stage while the menu is
        // up — which is exactly how the trade driver ends up in TRADE CENTER.
        // NOTE: this must stay live for as long as the menu is on screen. The
        // first version stopped after one A press and let the stage machine
        // resume pressing Up/A at the receptionist — which walked the cursor
        // back to TRADE CENTER, and the wire recorded exactly that: $D1, $D1,
        // ..., $D0, $D0, ..., $D4.
        if link == 0
            && text.contains("TRADE CENTER")
            && text.contains("COLOSSEUM")
            && self.menu_downs < 200
        {
            let needle = if self.want_colosseum {
                "COLOSSEUM"
            } else {
                "TRADE CENTER"
            };
            let want_row = text.lines().position(|l| l.contains(needle)).unwrap_or(9);
            let cur = cursor_row(gb);
            if verbose && self.menu_downs == 0 {
                println!(
                    "  cable-club menu: COLOSSEUM on row {want_row}, cursor {cur:?}\n{}",
                    indent(&text)
                );
            }
            match cur {
                Some(r) if r == want_row => {
                    if verbose {
                        println!("  cable-club menu: cursor on COLOSSEUM, pressing A");
                    }
                    self.menu_downs += 1;
                    self.press(gb, KeypadKey::A, 4);
                }
                _ => {
                    self.menu_downs += 1;
                    self.press(gb, KeypadKey::Down, 6);
                }
            }
            return;
        }

        match self.stage {
            Stage::Continue => {
                let menu = text.contains("CONTINUE")
                    || text.contains("NEW GAME")
                    || text.contains("OPTION")
                    || text.contains("BADGES");
                let loaded = gb.peek(W_PARTY_MON_NICKS) != 0
                    && (1..=6).contains(&gb.peek(W_PARTY_COUNT));
                if menu || !loaded || !looks_like_a_map(gb) {
                    self.stuck = 0;
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                self.stuck += 1;
                if self.stuck > 6 {
                    set(self, Stage::EnterRoom);
                }
            }
            Stage::EnterRoom => {
                if map == MAP_VIRIDIAN_POKECENTER {
                    set(self, Stage::WalkToReceptionist);
                    return;
                }
                let wy = gb.peek(W_WARP_ENTRIES);
                let wx = gb.peek(W_WARP_ENTRIES + 1);
                self.stuck += 1;
                self.step_towards(gb, x, y, wx, wy);
            }
            Stage::WalkToReceptionist => {
                if map == MAP_COLOSSEUM || map == MAP_TRADE_CENTER {
                    set(self, Stage::InRoom);
                    return;
                }
                if link != 0 {
                    set(self, Stage::LinkMenu);
                    return;
                }
                if text.contains("CABLE CLUB") || text.contains("SAVE") {
                    set(self, Stage::TalkAndSave);
                    return;
                }
                if x != RECEPTIONIST_X || y != RECEPTIONIST_Y {
                    self.step_towards(gb, x, y, RECEPTIONIST_X, RECEPTIONIST_Y);
                } else {
                    self.stuck += 1;
                    if self.stuck % 2 == 1 {
                        self.press(gb, KeypadKey::Up, 4);
                    } else {
                        self.press(gb, KeypadKey::A, 4);
                    }
                }
            }
            Stage::TalkAndSave => {
                if link != 0 {
                    set(self, Stage::LinkMenu);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Stage::LinkMenu => {
                if map == MAP_COLOSSEUM || map == MAP_TRADE_CENTER {
                    set(self, Stage::InRoom);
                    return;
                }
                // The cable-club menu is TRADE CENTER / COLOSSEUM / CANCEL
                // (`CableClubOptionsText`, engine/menus/main_menu.asm). Move the
                // cursor down once, then press A: the byte on the wire becomes
                // $D0 | item | (button << 2) = $D5.
                if text.contains("COLOSSEUM") && !self.menu_moved {
                    self.menu_moved = true;
                    self.press(gb, KeypadKey::Down, 6);
                    return;
                }
                self.press(gb, KeypadKey::A, 4);
            }
            Stage::InRoom => {
                if link >= 0x02 {
                    set(self, Stage::AtConsole);
                    return;
                }
                let spawn = *self.spawn.get_or_insert((x, y));
                if (x, y) != spawn {
                    self.step_towards(gb, x, y, spawn.0, spawn.1);
                    return;
                }
                self.stuck += 1;
                if self.stuck % 2 == 0 {
                    self.press(gb, KeypadKey::Right, 4);
                } else {
                    self.press(gb, KeypadKey::A, 4);
                }
            }
            Stage::AtConsole => {
                self.press(gb, KeypadKey::A, 4);
            }
            Stage::Battle => {
                // The battle menu is FIGHT/PKMN | ITEM/RUN with the cursor on
                // FIGHT, and the move menu opens on move 1. Spamming A therefore
                // always picks move slot 0; --moves lets the player vary.
                let move_menu = text.contains("TYPE") || text.contains("PP");
                if move_menu {
                    if !self.in_move_menu {
                        self.in_move_menu = true;
                        self.move_cursor = 0;
                    }
                    let want = if self.player_moves.is_empty() {
                        0
                    } else {
                        self.player_moves[self.turn % self.player_moves.len()]
                    };
                    if self.move_cursor < want {
                        self.move_cursor += 1;
                        self.press(gb, KeypadKey::Down, 4);
                        return;
                    }
                    self.turn += 1;
                    self.in_move_menu = false;
                    self.press(gb, KeypadKey::A, 4);
                    return;
                }
                self.in_move_menu = false;
                self.press(gb, KeypadKey::A, 4);
            }
        }
    }
}

// --- reading the machine (copied from examples/gen1_trade.rs) ---------------

fn decode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .take_while(|&&b| b != 0x50)
        .filter_map(|&b| terminalgb_plugin_gen1_link::text::decode_char(b))
        .collect()
}

fn screen_text(gb: &mut Gameboy) -> String {
    let lcdc = gb.peek(0xFF40);
    let window_on = lcdc & 0x20 != 0 && gb.peek(0xFF4A) < 144;
    let (map, sy, sx) = if window_on {
        (if lcdc & 0x40 != 0 { 0x9C00u16 } else { 0x9800 }, 0, 0)
    } else {
        (
            if lcdc & 0x08 != 0 { 0x9C00u16 } else { 0x9800 },
            gb.peek(0xFF42) as usize / 8,
            gb.peek(0xFF43) as usize / 8,
        )
    };
    let mut out = String::new();
    for row in 0..18 {
        for col in 0..20 {
            let y = (sy + row) % 32;
            let x = (sx + col) % 32;
            let t = gb.peek(map + (y * 32 + x) as u16);
            out.push(terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '));
        }
        out.push('\n');
    }
    out
}

fn looks_like_a_map(gb: &mut Gameboy) -> bool {
    let lcdc = gb.peek(0xFF40);
    let window_on = lcdc & 0x20 != 0 && gb.peek(0xFF4A) < 144;
    let map = if window_on {
        if lcdc & 0x40 != 0 {
            0x9C00u16
        } else {
            0x9800
        }
    } else if lcdc & 0x08 != 0 {
        0x9C00
    } else {
        0x9800
    };
    let mut seen = [false; 256];
    let mut distinct = 0;
    for row in 0..18u16 {
        for t in gb.peek_range(map + row * 32, 20) {
            if !seen[t as usize] {
                seen[t as usize] = true;
                distinct += 1;
            }
        }
    }
    distinct > 6
}

fn indent(s: &str) -> String {
    s.lines()
        .map(|l| format!("    |{l}|"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Row of the menu cursor. Gen 1 uses `$ED` (filled) and `$EC` (hollow).
fn cursor_row(gb: &mut Gameboy) -> Option<usize> {
    let lcdc = gb.peek(0xFF40);
    let window_on = lcdc & 0x20 != 0 && gb.peek(0xFF4A) < 144;
    let map = if window_on {
        if lcdc & 0x40 != 0 {
            0x9C00u16
        } else {
            0x9800
        }
    } else if lcdc & 0x08 != 0 {
        0x9C00
    } else {
        0x9800
    };
    (0..18).find(|&row| {
        gb.peek_range(map + (row * 32) as u16, 20)
            .iter()
            .any(|&t| t == 0xED || t == 0xEC)
    })
}

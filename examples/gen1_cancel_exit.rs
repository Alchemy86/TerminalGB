//! Can a Gen 1 player leave the Cable Club? Measured, on two real cartridges.
//!
//! ```text
//! cargo run --release --features image,plugin-gen1-link,plugin-trainer \
//!     --example gen1_cancel_exit -- --rom ~/roms/blue.gb --save <fixture.sav>
//! ```
//!
//! # The report this exists to answer
//!
//! *"There is simply no leaving the damn room. Cancelling doesn't make you
//! leave. I'm sure if both players choose cancel you SHOULD leave, but Blue is
//! trapped in the void trade room. Gold has a door to walk out."*
//!
//! Three separate claims live inside that, and they have three different
//! answers. This runner measures each of them on the running cartridge rather
//! than arguing from the state diagram:
//!
//! 1. **One player cancelling does not let that player out.** Correct, and it
//!    is the cartridge: `.cancelMenuItem_APressed` sends `$F`, runs one
//!    `Serial_SyncAndExchangeNybble` round and only falls through to
//!    `ReturnToCableClubRoom` when the round comes back `$F` — the
//!    disassembly's own comment on that compare is *"did the other person
//!    choose Cancel too?"*
//!    ([`engine/link/cable_club.asm`](https://github.com/pret/pokered/tree/master/engine/link/cable_club.asm)).
//! 2. **Both players cancelling does let both out — of the trade *screen*.**
//!    Also correct, and it is what phase 2 below shows.
//! 3. **Leaving the trade screen is NOT leaving the room, and the room has no
//!    door.** `data/maps/objects/TradeCenter.asm` declares `def_warp_events`
//!    with no entries, so `wNumberOfWarps` reads 0 at run time and there is no
//!    tile to walk out through. `ReturnToCableClubRoom` reloads the *same*
//!    map. This is the half the maintainer actually hit, and it is why Gold —
//!    whose Time Capsule room is an ordinary Pokémon Center 2F with a
//!    staircase — walks out while Blue cannot.
//!
//! The way out that the cartridge does provide is **START → RESET**:
//! `StartMenu_SaveReset` (`engine/menus/start_sub_menus.asm`) jumps to `Init`
//! when `BIT_LINK_CONNECTED` is set, and `TradeCenter_Trade` saves the party
//! immediately after "Trade completed!" with pokered's own comment *"this
//! allows reset into Pokecenter"*. Phase 3 drives that and checks the player
//! comes back standing in the Pokémon Center with the trade intact.
//!
//! # Two Gen 1 consoles, deliberately
//!
//! The cancel round is symmetric — Gen 2 in Time Capsule mode sends
//! `SERIAL_TIMECAPSULE | $F` = `$6F` from `LinkTradeOTPartymonMenuCheckCancel`
//! and requires `$F` back exactly as Gen 1 does — so nothing about the
//! question needs a Gold cartridge and a crafted Time Capsule save. Blue
//! against Blue is the same code path on the side under test, runs in seconds,
//! and needs only the one fixture. `--rom-b` takes a second cartridge if you
//! want to watch it against something else.
//!
//! Nothing here opens a path in the player's ROM directory: both consoles get
//! a copy of the cartridge and a copy of the save under `target/`.

use std::path::PathBuf;

use terminalgb::gameboy::Gameboy;
use terminalgb::link::LinkCable;
use terminalgb::KeypadKey;

#[path = "support/gen1_cable_club.rs"]
mod cable_club;

use cable_club::*;

// --- Addresses read out of the cartridge, not looked up ---------------------
//
// `.cancelMenuItem_APressed` is `ld a,'▷' / ldcoord_a 1,16 / ld a,$F /
// ld [wSerialExchangeNybbleSendData],a / call …`, i.e. the byte string
// `EA E1 C4 3E 0F EA lo hi`, which occurs exactly once in the cartridge. The
// operand of that `ld` is the send register; the `ld a,[nn] / cp $F` right
// after the call is the receive register.

/// `wSerialExchangeNybbleSendData` — the nybble this console is offering.
/// `$F` once the player has pressed A on CANCEL, and it is a **latch**: every
/// later sync round keeps sending it until `TradeCenter_SelectMon` resets it.
const W_SERIAL_SEND: u16 = 0xCC42;
/// `wSerialSyncAndExchangeNybbleReceiveData` — what came back. `$F` means the
/// other player cancelled too, and is the only value that opens the door.
const W_SERIAL_RECV: u16 = 0xCC3D;
/// `wTileMap` coord (1, 16) — the cancel cursor cell. `$ED` (`▶`) is
/// "parked on CANCEL waiting for a press"; `$EC` (`▷`) is "A pressed, inside
/// the sync round". Watching this byte is how "the player is pressing A on
/// CANCEL for ever" is *observed* rather than inferred.
const W_CANCEL_CURSOR: u16 = 0xC4E1;
const CURSOR_PARKED: u8 = 0xED;
const CURSOR_PRESSED: u8 = 0xEC;

/// `LINK_STATE_IN_CABLE_CLUB` / `LINK_STATE_TRADING` (`serial_constants.asm`).
const LINK_STATE_IN_CABLE_CLUB: u8 = 0x01;
const LINK_STATE_TRADING: u8 = 0x32;

fn main() {
    let mut rom_a: Option<PathBuf> = None;
    let mut rom_b: Option<PathBuf> = None;
    let mut save_a: Option<PathBuf> = None;
    let mut save_b: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut frames: u32 = 60 * 60 * 10;
    let mut verbose = false;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--rom" => rom_a = args.next().map(PathBuf::from),
            "--rom-b" => rom_b = args.next().map(PathBuf::from),
            "--save" => save_a = args.next().map(PathBuf::from),
            "--save-b" => save_b = args.next().map(PathBuf::from),
            "--shots" => shots = args.next().map(PathBuf::from),
            "--frames" => {
                frames = args.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--verbose" => verbose = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(rom_a) = rom_a else {
        eprintln!(
            "usage: gen1_cancel_exit --rom <blue.gb> --save <cableclub.sav> \
             [--rom-b <other.gb>] [--save-b <sav>] [--shots <dir>] \
             [--frames N] [--verbose]"
        );
        std::process::exit(2);
    };
    let rom_b = rom_b.unwrap_or_else(|| rom_a.clone());
    let save_b = save_b.or_else(|| save_a.clone());

    let scratch = PathBuf::from("target/gen1-cancel-exit");
    let a = stage("a", &rom_a, save_a.as_deref(), &scratch);
    let b = stage("b", &rom_b, save_b.as_deref(), &scratch);
    // Distinct parties so the two selection screens are visibly different and
    // a cursor that has walked onto a Pokémon rather than CANCEL is obvious.
    patch_party(
        &a,
        "trainer = RED\n\
         species = Abra\nlevel = 10\nnickname = GLIMMER\not_name = RED\not_id = 4242\n\
         moves = Teleport\n",
    );
    patch_party(
        &b,
        "trainer = BLUE\n\
         species = Machop\nlevel = 11\nnickname = ROCKY\not_name = BLUE\not_id = 1717\n\
         moves = Karate Chop, Low Kick\n",
    );

    let mut gb_a =
        Gameboy::new(std::fs::read(&a.rom).expect("rom a"), Some(a.rom.clone()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb_a.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    let mut gb_b =
        Gameboy::new(std::fs::read(&b.rom).expect("rom b"), Some(b.rom.clone()));
    // Same rule as above: a machine with no APU is not a quiet Game Boy.
    gb_b.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    println!("console A: {}", gb_a.rom_title());
    println!("console B: {}", gb_b.rom_title());

    if let Some(dir) = &shots {
        std::fs::create_dir_all(dir).expect("shots dir");
    }

    // Two cores stepped by one loop never drift, so their handshakes never
    // interleave — see `docs/link-cable.md`.
    for _ in 0..3 {
        gb_b.frame();
    }

    let mut cable = LinkCable::new();
    let mut bot_a = TradeBot::default();
    let mut bot_b = TradeBot::default();
    let mut ex = Experiment::default();
    let mut last = String::new();
    let mut shot_n = 0usize;
    let mut verdict = Verdict::default();

    for frame in 0..frames {
        // The shared bot drives both consoles as far as the trade selection
        // screen and is then taken off the controls — everything the
        // experiment measures happens after that.
        if bot_a.stage() != Stage::AtMachine {
            bot_a.act(&mut gb_a, verbose, "A");
        }
        if bot_b.stage() != Stage::AtMachine {
            bot_b.act(&mut gb_b, verbose, "B");
        }
        if bot_a.stage() == Stage::AtMachine && bot_b.stage() == Stage::AtMachine {
            ex.act(&mut gb_a, &mut gb_b, &mut verdict, frame);
        }

        cable.run_frame(&mut gb_a, &mut gb_b);

        if cable.desynced() {
            eprintln!(
                "\nLINK DESYNCHRONISED at frame {frame}\n  A: {:?}\n  B: {:?}",
                gb_a.fault(),
                gb_b.fault()
            );
            std::process::exit(1);
        }

        let label = format!(
            "{}/{}/{:?}",
            bot_a.stage_label(),
            bot_b.stage_label(),
            ex.phase
        );
        if label != last {
            println!(
                "[{frame:6}] {label:<40} A link={:#04x} send={:#04x} recv={:#04x} \
                 cursor={:#04x}",
                gb_a.peek(W_LINK_STATE),
                gb_a.peek(W_SERIAL_SEND),
                gb_a.peek(W_SERIAL_RECV),
                gb_a.peek(W_CANCEL_CURSOR),
            );
            if let Some(dir) = &shots {
                shot_n += 1;
                let name = label.replace('/', "-");
                write_pair(
                    &dir.join(format!("{shot_n:02}-{name}.png")),
                    &mut gb_a,
                    &mut gb_b,
                );
            }
            if verbose {
                println!("{}", side_by_side(&mut gb_a, &mut gb_b));
            }
            last = label;
        }
        if ex.phase == Phase::Done {
            break;
        }
    }

    if let Some(dir) = &shots {
        write_pair(&dir.join("99-final.png"), &mut gb_a, &mut gb_b);
    }
    let stats = cable.stats();
    println!(
        "\ncable: {} transfers, {} unanswered, {} torn",
        stats.transfers, stats.unanswered, stats.torn
    );
    verdict.report(&mut gb_a, &mut gb_b, ex.phase);
}

// ---------------------------------------------------------------------------
// The experiment.
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Both consoles are on the trade selection screen; walk A's cursor down
    /// onto CANCEL without pressing anything else.
    #[default]
    ToCancelA,
    /// A presses A on CANCEL, over and over, while B does nothing. This is the
    /// maintainer's screen.
    AloneOnCancel,
    /// Now B walks onto CANCEL too.
    ToCancelB,
    /// B presses A on CANCEL. Both should return to the Cable Club room.
    BothOnCancel,
    /// Standing in the room: is there a door?
    InRoom,
    /// START menu → RESET, and back in through CONTINUE.
    Resetting,
    Done,
}

#[derive(Default)]
struct Verdict {
    a_presses_alone: u32,
    a_left_alone: bool,
    /// Was the stuck state the one the report describes, byte for byte?
    stuck_signature: bool,
    a_left_together: bool,
    warps_in_room: Option<u8>,
    start_menu: String,
    reset_reached_title: bool,
    reset_map: Option<u8>,
    reset_party: Vec<String>,
}

#[derive(Default)]
struct Experiment {
    phase: Phase,
    hold: Option<(bool, KeypadKey, u32)>, // (which console, key, frames left)
    cooldown: u32,
    steps: u32,
    waited: u32,
    entered: u32,
}

impl Experiment {
    fn press(&mut self, gb: &mut Gameboy, side_a: bool, key: KeypadKey, hold: u32) {
        gb.keydown(key);
        self.hold = Some((side_a, key, hold));
    }

    fn to(&mut self, p: Phase, frame: u32) {
        println!("  [experiment] {:?} -> {p:?} at frame {frame}", self.phase);
        self.phase = p;
        self.steps = 0;
        self.waited = 0;
    }

    fn act(&mut self, a: &mut Gameboy, b: &mut Gameboy, v: &mut Verdict, frame: u32) {
        if self.entered == 0 {
            self.entered = frame;
        }
        // Give the selection screen a moment to be drawn before touching it.
        if frame - self.entered < 120 {
            return;
        }
        if let Some((side_a, key, left)) = self.hold {
            if left <= 1 {
                if side_a {
                    a.keyup(key)
                } else {
                    b.keyup(key)
                }
                self.hold = None;
                self.cooldown = 8;
            } else {
                self.hold = Some((side_a, key, left - 1));
            }
            return;
        }
        if self.cooldown > 0 {
            self.cooldown -= 1;
            return;
        }

        match self.phase {
            Phase::ToCancelA => {
                if a.peek(W_CANCEL_CURSOR) == CURSOR_PARKED {
                    self.to(Phase::AloneOnCancel, frame);
                    return;
                }
                self.steps += 1;
                if self.steps > 30 {
                    eprintln!("console A never reached the CANCEL item");
                    std::process::exit(1);
                }
                self.press(a, true, KeypadKey::Down, 4);
            }
            Phase::AloneOnCancel => {
                // `.cancelMenuItem_APressed` writes `▷` and then blocks in the
                // sync round; if the round ever comes back `$F` the game falls
                // through to `ReturnToCableClubRoom` and `wLinkState` drops to
                // `LINK_STATE_IN_CABLE_CLUB`.
                if a.peek(W_LINK_STATE) == LINK_STATE_IN_CABLE_CLUB {
                    v.a_left_alone = true;
                    self.to(Phase::InRoom, frame);
                    return;
                }
                if v.a_presses_alone >= 60 {
                    let (link, send, recv, cursor) = (
                        a.peek(W_LINK_STATE),
                        a.peek(W_SERIAL_SEND),
                        a.peek(W_SERIAL_RECV),
                        a.peek(W_CANCEL_CURSOR),
                    );
                    println!(
                        "  [experiment] console A has pressed A on CANCEL {} times \
                         with B still browsing: link={link:#04x} send={send:#04x} \
                         recv={recv:#04x} cursor={cursor:#04x}",
                        v.a_presses_alone,
                    );
                    // The signature of the reported symptom, spelled out: the
                    // trade screen still up, the cancel nybble latched, the
                    // hollow cursor showing A was taken and the round is
                    // running, and nothing but `$FE` coming back.
                    v.stuck_signature = link == LINK_STATE_TRADING
                        && send == 0x0F
                        && recv != 0x0F
                        && cursor == CURSOR_PRESSED;
                    self.to(Phase::ToCancelB, frame);
                    return;
                }
                v.a_presses_alone += 1;
                self.press(a, true, KeypadKey::A, 4);
            }
            Phase::ToCancelB => {
                if b.peek(W_CANCEL_CURSOR) == CURSOR_PARKED {
                    self.to(Phase::BothOnCancel, frame);
                    return;
                }
                self.steps += 1;
                if self.steps > 30 {
                    eprintln!("console B never reached the CANCEL item");
                    std::process::exit(1);
                }
                self.press(b, false, KeypadKey::Down, 4);
            }
            Phase::BothOnCancel => {
                if a.peek(W_LINK_STATE) == LINK_STATE_IN_CABLE_CLUB {
                    v.a_left_together = true;
                    self.to(Phase::InRoom, frame);
                    return;
                }
                self.steps += 1;
                if self.steps > 200 {
                    eprintln!("both consoles cancelled and A never left the screen");
                    self.to(Phase::Done, frame);
                    return;
                }
                self.press(b, false, KeypadKey::A, 4);
            }
            Phase::InRoom => {
                // Back in the Trade Center. Give the map a moment to load, then
                // ask the running game how many ways out it has.
                self.waited += 1;
                if self.waited < 60 {
                    return;
                }
                v.warps_in_room = Some(a.peek(W_NUMBER_OF_WARPS));
                self.to(Phase::Resetting, frame);
            }
            Phase::Resetting => {
                self.steps += 1;
                if self.steps > 3000 {
                    self.to(Phase::Done, frame);
                    return;
                }
                // `StartMenu_SaveReset` jumps straight to `Init`, so the
                // console comes back at the copyright screen with `wLinkState`
                // cleared. That is the moment RESET was taken.
                if !v.reset_reached_title && a.peek(W_LINK_STATE) == 0 {
                    v.reset_reached_title = true;
                    println!(
                        "  [experiment] RESET taken at frame {frame}: wLinkState \
                         is back to LINK_STATE_NONE"
                    );
                }
                if v.reset_reached_title {
                    // Same job the shared bot's `Continue` stage does: mash A
                    // through the title and CONTINUE until a map is up.
                    let map = a.peek(W_CUR_MAP);
                    let loaded = a.peek(W_PARTY_MON_NICKS) != 0
                        && (1..=6).contains(&a.peek(W_PARTY_COUNT));
                    if loaded && looks_like_a_map(a) && map != MAP_TRADE_CENTER {
                        self.waited += 1;
                        if self.waited > 8 {
                            v.reset_map = Some(map);
                            v.reset_party =
                                read_party(a).iter().map(|m| m.describe()).collect();
                            self.to(Phase::Done, frame);
                            return;
                        }
                    } else {
                        self.waited = 0;
                    }
                    self.press(a, true, KeypadKey::A, 4);
                    return;
                }
                // Open the START menu, then steer by what is on the screen
                // rather than by counting presses: the cursor is a `▶` tile in
                // the tilemap, so where it is and where RESET is are both
                // readable.
                let text = screen_text(a);
                let Some(reset_row) = text.lines().position(|l| l.contains("RESET"))
                else {
                    self.press(a, true, KeypadKey::Start, 4);
                    return;
                };
                v.start_menu = text;
                let Some(cursor_row) = menu_cursor_row(a) else {
                    self.press(a, true, KeypadKey::Start, 4);
                    return;
                };
                match cursor_row.cmp(&reset_row) {
                    std::cmp::Ordering::Less => self.press(a, true, KeypadKey::Down, 4),
                    std::cmp::Ordering::Greater => self.press(a, true, KeypadKey::Up, 4),
                    std::cmp::Ordering::Equal => {
                        println!(
                            "  [experiment] START menu cursor is on RESET (row \
                             {reset_row}) — pressing A"
                        );
                        self.press(a, true, KeypadKey::A, 4);
                    }
                }
            }
            Phase::Done => {}
        }
    }
}

impl Verdict {
    fn report(&self, a: &mut Gameboy, b: &mut Gameboy, phase: Phase) {
        println!("\n=== what the cartridges actually did =====================");
        println!(
            "1. one player cancelling alone: {} \
             (console A pressed A on CANCEL {} times{})",
            if self.a_left_alone {
                "LEFT THE SCREEN — unexpected"
            } else {
                "STUCK, as pokered says it should be"
            },
            self.a_presses_alone,
            if self.stuck_signature {
                ", and the stuck state is the reported one byte for byte"
            } else {
                ""
            },
        );
        println!(
            "2. both players cancelling:     {}",
            if self.a_left_together {
                "LEFT THE TRADE SCREEN"
            } else {
                "did not leave"
            }
        );
        match self.warps_in_room {
            Some(0) => println!(
                "3. the room itself:             wNumberOfWarps = 0 — the Trade \
                 Center has NO DOOR, and being back in it is not leaving it"
            ),
            Some(n) => println!("3. the room itself:             wNumberOfWarps = {n}"),
            None => println!("3. the room itself:             not reached"),
        }
        if !self.start_menu.is_empty() {
            let reset = self.start_menu.contains("RESET");
            println!(
                "4. START menu in the room:      {}",
                if reset {
                    "offers RESET in place of SAVE — the cartridge's own way out"
                } else {
                    "no RESET item found (unexpected)"
                }
            );
            for line in self.start_menu.lines().filter(|l| !l.trim().is_empty()) {
                println!("     | {line}");
            }
        }
        match self.reset_map {
            Some(m) => {
                println!(
                    "5. after RESET → CONTINUE:      out of the Cable Club, standing \
                     on map {m:#04x} ({})",
                    if m == MAP_VIRIDIAN_POKECENTER {
                        "the Pokémon Center it saved in"
                    } else {
                        "somewhere else"
                    }
                );
                for p in &self.reset_party {
                    println!("     party: {p}");
                }
            }
            None if phase == Phase::Done => {
                println!("5. after RESET → CONTINUE:      never got back into the game")
            }
            None => println!("5. after RESET → CONTINUE:      not reached"),
        }
        println!("\nfaults: A {:?} B {:?}", a.fault(), b.fault());
        println!(
            "A link={:#04x} B link={:#04x}",
            a.peek(W_LINK_STATE),
            b.peek(W_LINK_STATE),
        );
    }
}

/// Which tilemap row the menu cursor is on, if it is on screen.
///
/// Gen 1 draws the menu cursor as the character `▶` (`$ED`) straight into
/// `wTileMap`, so "where is the cursor" is a scan of the screen rather than a
/// WRAM address that would have to be located first.
fn menu_cursor_row(gb: &mut Gameboy) -> Option<usize> {
    let map = gb.peek_range(W_TILE_MAP, 20 * 18);
    map.iter().position(|&t| t == CURSOR_PARKED).map(|i| i / 20)
}

/// `wTileMap`.
const W_TILE_MAP: u16 = 0xC3A0;

//! # The World Trade Centre — a proof of concept
//!
//! Four independent emulator processes, each with its own staged cartridge, its
//! own save file and its own input, connected to one relay, walking about one
//! shared Cable Club room and seeing each other do it.
//!
//! **These are four emulator instances, not four humans.** That is stated here
//! and in `docs/worldlobby.md` because it is the honest description of what was
//! run: nobody can summon four people on demand, and the thing that actually
//! needs proving is that four *independent* consoles — separate processes,
//! separate memory, separate saves, separate walking — can share a room. The
//! code cannot tell the difference between a walk driven by this file's
//! `Walker` and a walk driven by somebody's thumb, because both arrive as
//! `Gameboy::keydown`.
//!
//! ## Running it
//!
//! ```sh
//! cargo run --release --features image,plugin-gen1-lobby,plugin-gen1-link,plugin-trainer \
//!     --example worldlobby -- \
//!     --rom ~/roms/blue.gb --save ~/roms/blue.sav --shots target/wl-shots
//! ```
//!
//! That is `demo` mode: it starts the relay in this process and spawns one
//! child process per player, each of which runs the `play` subcommand against
//! its own staged copy of the cartridge. `--players N` sizes the crowd (an even
//! number: the consoles pair up two at a time over the link cable, which is how
//! they get into the Cable Club at all).
//!
//! ## The shape of the thing
//!
//! ```text
//!   relay (this process, a thread)          8 Hz snapshots
//!        |          |          |          |
//!     player 1   player 2   player 3   player 4      <- separate PROCESSES
//!        \________/            \________/
//!         link cable            link cable           <- unix-socket peer pairs
//! ```
//!
//! Two layers, deliberately separate, and §4 of the scout report is why: the
//! **relay** carries presence — position, facing, a phrase — for everybody, and
//! never touches a game byte. The **cable** carries a trade or a battle, is
//! strictly 1:1, and is the cartridge's own protocol unchanged. A Game Boy has
//! one shift register and no addressing; there is nothing to make a hub out of,
//! so the crowd lives entirely outside the machine.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminalgb::gameboy::Gameboy;
use terminalgb::link::peer::LinkPeer;
use terminalgb::KeypadKey;

use terminalgb_plugin_gen1_lobby::avatar::{Peer, Presence};
use terminalgb_plugin_gen1_lobby::bubble::PHRASES;
use terminalgb_plugin_gen1_lobby::gen1::{Facing, MAP_COLOSSEUM, MAP_TRADE_CENTER};
use terminalgb_plugin_gen1_lobby::room::{walkable_square, Room, DEFAULT_ROOM};
use terminalgb_plugin_gen1_lobby::{LobbyPlugin, State};

#[path = "support/gen1_cable_club.rs"]
mod cable_club;
use cable_club::*;

/// The relay's tick rate. A Gen 1 walk step is 16 frames ≈ 268 ms, so 8 Hz is
/// twice what "smooth" needs and a late packet costs one stale frame of
/// somebody else's avatar, never a desync.
const TICK_HZ: u64 = 8;
const FRAMES_PER_TICK: u32 = 60 / TICK_HZ as u32;

// ---------------------------------------------------------------------------
// The wire. Tab-separated lines, no dependency, no escaping problem: every name
// and phrase has already been filtered to Gen 1's charmap, which has no tab.
// ---------------------------------------------------------------------------

/// What phase of the demonstration the room is in. The **relay** owns this so
/// that four processes act together without any of them being in charge, and so
/// that a screenshot taken at a given tick really is the same instant on every
/// screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Waiting,
    Gather,
    Mingle,
    Talk,
    Business,
    Done,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Gather => "gather",
            Phase::Mingle => "mingle",
            Phase::Talk => "talk",
            Phase::Business => "business",
            Phase::Done => "done",
        }
    }

    fn parse(s: &str) -> Phase {
        match s {
            "gather" => Phase::Gather,
            "mingle" => Phase::Mingle,
            "talk" => Phase::Talk,
            "business" => Phase::Business,
            "done" => Phase::Done,
            _ => Phase::Waiting,
        }
    }

    /// The schedule, in relay ticks after everybody was in the room.
    fn at(tick: u64) -> Phase {
        match tick {
            0..=47 => Phase::Gather,
            48..=127 => Phase::Mingle,
            128..=175 => Phase::Talk,
            176..=760 => Phase::Business,
            _ => Phase::Done,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Body {
    name: String,
    x: u8,
    y: u8,
    facing: u8,
    step: u8,
    says: String,
    ready: bool,
    /// Set once the client has left the room for a trade or a battle, so the
    /// relay stops reporting a stale position for them.
    away: bool,
}

#[derive(Default)]
struct Lobby {
    bodies: BTreeMap<u32, Body>,
    tick: u64,
    started: bool,
}

fn serve(listener: TcpListener, expect: usize, stop: Arc<AtomicBool>) {
    let lobby = Arc::new(Mutex::new(Lobby::default()));
    let clients: Arc<Mutex<Vec<(u32, TcpStream)>>> = Arc::new(Mutex::new(Vec::new()));

    // The broadcaster: one snapshot per tick to everybody, rather than a
    // fan-out per message. At four players that is indistinguishable; it is the
    // shape that stays affordable at a hundred (report §7.1 point 4).
    {
        let lobby = Arc::clone(&lobby);
        let clients = Arc::clone(&clients);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let period = Duration::from_millis(1000 / TICK_HZ);
            let mut next = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                next += period;
                std::thread::sleep(next.saturating_duration_since(Instant::now()));
                let mut line = String::new();
                let phase;
                {
                    let mut l = lobby.lock().unwrap();
                    let all_ready = l.bodies.len() >= expect
                        && l.bodies.values().all(|b| b.ready);
                    if all_ready {
                        l.started = true;
                    }
                    if l.started {
                        l.tick += 1;
                    }
                    phase = if l.started {
                        Phase::at(l.tick)
                    } else {
                        Phase::Waiting
                    };
                    line.push_str(&format!(
                        "snap\t{}\t{}\t{}\n",
                        l.tick,
                        phase.name(),
                        l.bodies.len()
                    ));
                    for (id, b) in &l.bodies {
                        if b.away {
                            continue;
                        }
                        line.push_str(&format!(
                            "peer\t{id}\t{}\t{}\t{}\t{}\t{}\t{}\n",
                            b.name, b.x, b.y, b.facing, b.step, b.says
                        ));
                    }
                    line.push_str("end\n");
                }
                let mut cs = clients.lock().unwrap();
                cs.retain_mut(|(_, s)| s.write_all(line.as_bytes()).is_ok());
                if phase == Phase::Done {
                    stop.store(true, Ordering::Relaxed);
                }
            }
        });
    }

    listener
        .set_nonblocking(false)
        .expect("blocking listener");
    let mut next_id = 1u32;
    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(mut stream) = stream else { continue };
        let id = next_id;
        next_id += 1;
        stream
            .write_all(format!("you\t{id}\n").as_bytes())
            .expect("greet");
        let tx = stream.try_clone().expect("clone");
        clients.lock().unwrap().push((id, tx));
        let lobby = Arc::clone(&lobby);
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let f: Vec<&str> = line.split('\t').collect();
                let mut l = lobby.lock().unwrap();
                let b = l.bodies.entry(id).or_default();
                match f.first().copied() {
                    Some("join") => b.name = f.get(1).unwrap_or(&"?").to_string(),
                    Some("ready") => b.ready = true,
                    Some("away") => b.away = f.get(1) == Some(&"1"),
                    Some("pos") => {
                        let g = |i: usize| f.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
                        b.x = g(1);
                        b.y = g(2);
                        b.facing = g(3);
                        b.step = g(4);
                        b.says = f.get(5).unwrap_or(&"").to_string();
                    }
                    _ => {}
                }
            }
            lobby.lock().unwrap().bodies.remove(&id);
            let _ = &stop;
        });
        if next_id as usize > expect + 4 {
            break;
        }
    }
}

// ---------------------------------------------------------------------------
// One player: one process, one console, one save, one pair of hands.
// ---------------------------------------------------------------------------

/// Walks the player between waypoints by pressing the keys a person would.
///
/// A press held for one frame does nothing useful — Gen 1 commits to a step
/// over 16 frames and a press in a direction the player is not facing spends
/// the first few of them *turning*. So the walker holds a direction, watches
/// `wXCoord`/`wYCoord`, and re-decides once the coordinate has moved or the
/// hold has run out. That is exactly the loop a player's thumb runs.
struct Walker {
    hold: Option<(KeypadKey, u32, (u8, u8))>,
    last: (u8, u8),
    blocked: u32,
    seed: u64,
}

impl Walker {
    fn new(seed: u64) -> Walker {
        Walker {
            hold: None,
            last: (255, 255),
            blocked: 0,
            seed: seed.wrapping_mul(6364136223846793005).wrapping_add(1),
        }
    }

    fn rand(&mut self) -> u64 {
        self.seed = self
            .seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.seed >> 33
    }

    /// Let go of whatever is held. **`Gameboy::clear_input` is not this**: it
    /// resets the scheduled-press queue and releases what *that* held, and the
    /// walker presses keys directly, the way a person's hand does. Handing over
    /// to another driver without calling this leaves a direction physically
    /// down for the rest of the run, and the symptom is a player pinned against
    /// a wall that reads exactly like a broken route.
    fn release(&mut self, gb: &mut Gameboy) {
        if let Some((k, _, _)) = self.hold.take() {
            gb.keyup(k);
        }
    }

    /// One frame of walking towards `(tx, ty)`. Returns true once arrived.
    ///
    /// **The press is released the instant the coordinate moves, not after a
    /// fixed number of frames.** A Gen 1 step is 16 frames and the game commits
    /// to a step once it has begun, so any hold long enough to be safe against
    /// the turn-then-walk case is also long enough to start a *second* step —
    /// and a walker aiming at an exact square then overshoots it and oscillates
    /// either side for ever. Watching the coordinate is what a person does, and
    /// it is the only version of this that converges.
    fn towards(&mut self, gb: &mut Gameboy, tx: u8, ty: u8) -> bool {
        let here = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
        if let Some((k, left, start)) = self.hold {
            // Moved, or out of patience (a wall, or somebody standing there).
            if here != start || left <= 1 {
                gb.keyup(k);
                self.hold = None;
            } else {
                self.hold = Some((k, left - 1, start));
            }
            return false;
        }
        let (x, y) = here;
        if (x, y) == (tx, ty) {
            return true;
        }
        if (x, y) == self.last {
            self.blocked += 1;
        } else {
            self.blocked = 0;
        }
        self.last = (x, y);

        // Prefer the longer axis, but alternate when blocked — a wall, or
        // another player standing in the way, has to be walked around.
        let want_x = (x != tx) as u32;
        let want_y = (y != ty) as u32;
        let prefer_x = if want_x + want_y == 2 {
            (self.blocked / 2) % 2 == 0
        } else {
            want_x == 1
        };
        let dx = if x < tx {
            Some(KeypadKey::Right)
        } else if x > tx {
            Some(KeypadKey::Left)
        } else {
            None
        };
        let dy = if y < ty {
            Some(KeypadKey::Down)
        } else if y > ty {
            Some(KeypadKey::Up)
        } else {
            None
        };
        let k = if prefer_x { dx.or(dy) } else { dy.or(dx) };
        if let Some(k) = k {
            gb.keydown(k);
            // Long enough for a turn *and* a step — a press in a direction the
            // player is not facing spends its first frames turning, and a hold
            // too short for both leaves a walker that only ever pirouettes.
            self.hold = Some((k, 40, (x, y)));
        }
        false
    }

    /// A random walkable square, for the mingle phase.
    fn wander_target(&mut self, room: &Room) -> (u8, u8) {
        let (w, h) = room.walk_size();
        loop {
            let x = (self.rand() % w as u64) as u8;
            let y = (self.rand() % h as u64) as u8;
            if walkable_square(room, x, y) {
                return (x, y);
            }
        }
    }
}

struct PlayOpts {
    name: String,
    rom: PathBuf,
    save: Option<PathBuf>,
    shots: Option<PathBuf>,
    port: u16,
    pair_dir: PathBuf,
    gather: (u8, u8),
    phrase: &'static str,
    business: Business,
    seed: u64,
    frames: u32,
    speed: f64,
    verbose: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Business {
    /// Walk back to the console and trade with the console next to you.
    Trade,
    /// Flip `wCurMap` to `COLOSSEUM` and start a link battle instead. One byte
    /// (`engine/pokemon/bills_pc.asm`) — but see `docs/worldlobby.md` §6 for
    /// what actually happened.
    Battle,
    /// Keep walking and watch.
    Watch,
}

impl Business {
    fn parse(s: &str) -> Business {
        match s {
            "trade" => Business::Trade,
            "battle" => Business::Battle,
            _ => Business::Watch,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Business::Trade => "trade",
            Business::Battle => "battle",
            Business::Watch => "watch",
        }
    }
}

/// Everything the relay last told us.
#[derive(Default)]
struct Snapshot {
    tick: u64,
    phase: Phase,
    peers: Vec<Peer>,
}

impl Default for Phase {
    fn default() -> Phase {
        Phase::Waiting
    }
}

fn reader_thread(stream: TcpStream, shared: Arc<Mutex<Snapshot>>, my_id: Arc<Mutex<u32>>) {
    let reader = BufReader::new(stream);
    let mut building: Option<Snapshot> = None;
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some("you") => {
                if let Some(id) = f.get(1).and_then(|s| s.parse().ok()) {
                    *my_id.lock().unwrap() = id;
                }
            }
            Some("snap") => {
                building = Some(Snapshot {
                    tick: f.get(1).and_then(|s| s.parse().ok()).unwrap_or(0),
                    phase: Phase::parse(f.get(2).copied().unwrap_or("")),
                    peers: Vec::new(),
                });
            }
            Some("peer") => {
                if let Some(s) = &mut building {
                    let g = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0u8);
                    let id: u32 = f.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                    let says = f.get(7).copied().unwrap_or("");
                    s.peers.push(Peer {
                        id,
                        name: f.get(2).unwrap_or(&"?").to_string(),
                        x: g(3),
                        y: g(4),
                        facing: Facing::from_byte(g(5)),
                        step: g(6),
                        says: (!says.is_empty()).then(|| says.to_string()),
                    });
                }
            }
            Some("end") => {
                if let Some(s) = building.take() {
                    *shared.lock().unwrap() = s;
                }
            }
            _ => {}
        }
    }
}

fn play(o: PlayOpts) {
    let who = o.name.clone();
    let say = |m: &str| println!("[{who:<5}] {m}");

    // Never near `~/roms`: the MBC writes a `.sav` beside the ROM it was given.
    let scratch = PathBuf::from("target/worldlobby");
    let staged = stage(&o.name.to_lowercase(), &o.rom, o.save.as_deref(), &scratch);
    // Distinct parties, so "a trade happened" is provable rather than plausible.
    patch_party(&staged, &party_spec(&o.name));

    let mut gb = Gameboy::new(
        std::fs::read(&staged.rom).expect("rom"),
        Some(staged.rom.clone()),
    );
    let title = gb.rom_title();
    let sav = staged.rom.with_extension("sav");

    // Each pair of consoles gets its own rendezvous directory, so who pairs
    // with whom is decided by us rather than by who happened to start first.
    std::fs::create_dir_all(&o.pair_dir).expect("pair dir");
    std::env::set_var("GB_LINK_DIR", &o.pair_dir);
    let mut peer = LinkPeer::discover(&title, Some(&sav));
    say(&format!("cable: {:?}", peer.state()));

    if let Some(d) = &o.shots {
        std::fs::create_dir_all(d).expect("shots dir");
    }

    // --- phase 0: get this console into the Cable Club, the shipped way ----
    let mut bot = TradeBot::default();
    let mut arrived = None;
    for f in 0..60 * 60 * 6 {
        bot.act(&mut gb, false, &o.name);
        peer.run_frame(&mut gb);
        if gb.peek(W_CUR_MAP) == MAP_TRADE_CENTER && gb.peek(W_LINK_STATE) == 0x01 {
            for _ in 0..60 {
                peer.run_frame(&mut gb);
            }
            arrived = Some(f);
            break;
        }
        if let Some(fault) = gb.fault() {
            eprintln!("[{who}] FAULT before the room: {fault}");
            std::process::exit(1);
        }
    }
    let Some(f) = arrived else {
        eprintln!("[{who}] never reached the Trade Center");
        std::process::exit(1);
    };
    // **Which seat is not a choice.** `CableClubLeftGameboy` returns
    // immediately unless this console took the internal clock and the player
    // faces right; `CableClubRightGameboy` is the mirror image for the external
    // clock (`engine/pokemon/bills_pc.asm`). The game has already warped the
    // player next to the correct console, so the seat is simply where we woke
    // up — reading it beats deciding it, and a caller that decides it gets a
    // player standing at the wrong machine pressing A at nothing.
    let seat = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
    let clock = gb.peek(H_SERIAL_CONNECTION_STATUS);
    say(&format!(
        "in the Cable Club at frame {f}, seat {seat:?}, clock {clock:#04x} ({})",
        match clock {
            0x02 => "internal - left console, face right",
            0x01 => "external - right console, face left",
            _ => "unsettled",
        }
    ));

    // --- the room -----------------------------------------------------------
    let lobby = LobbyPlugin::new(&o.name);
    let room = lobby.room();
    gb.add_plugin(Box::new(lobby)).expect("attach lobby plugin");
    // One frame installs it; the shot after that is the empty room.
    for _ in 0..8 {
        peer.run_frame(&mut gb);
    }
    if let Some(d) = &o.shots {
        write_one(&d.join(format!("01-room-{}.png", o.name.to_lowercase())), &mut gb);
    }
    say(&format!(
        "room installed: {}x{} blocks = {:?} walk tiles; {}",
        room.w,
        room.h,
        room.walk_size(),
        gb.plugin_status_line().unwrap_or_default()
    ));
    if let Some(p) = gb.plugin::<LobbyPlugin>() {
        // The calibration, printed rather than assumed: it is the plugin
        // proving it can reproduce the engine's own screen before it paints
        // one. `anchor None` means it declined to paint at all.
        say(&format!(
            "screen calibration: anchor {:?}, candidates {:?}",
            p.anchor(),
            p.calibration
        ));
    }

    // --- the relay ----------------------------------------------------------
    let mut sock = TcpStream::connect(("127.0.0.1", o.port)).expect("connect to relay");
    sock.set_nodelay(true).ok();
    writeln!(sock, "join\t{}", o.name).expect("join");
    let shared = Arc::new(Mutex::new(Snapshot::default()));
    let my_id = Arc::new(Mutex::new(0u32));
    {
        let s = sock.try_clone().expect("clone");
        let shared = Arc::clone(&shared);
        let my_id = Arc::clone(&my_id);
        std::thread::spawn(move || reader_thread(s, shared, my_id));
    }
    writeln!(sock, "ready").expect("ready");

    // --- the loop -----------------------------------------------------------
    let mut walker = Walker::new(o.seed);
    let mut target = o.gather;
    let mut last_phase = Phase::Waiting;
    let mut shot_ticks: Vec<u64> = vec![48, 72, 96, 120, 134];
    let mut trade_bot: Option<TradeBot> = None;
    let mut done = false;
    let mut said = false;
    let mut flipped = false;
    let mut battle_seen = false;
    let mut business_started = false;

    // **Pace to the real console.** 70224 cycles / 4.194304 MHz = 16.7427 ms.
    // Without this the emulator would outrun the relay's wall-clock tick by two
    // orders of magnitude and every "phase" would be over before a single frame
    // of it had been walked — and, more to the point, a demonstration of what
    // four people would see has to run at the speed four people would see it.
    let period = Duration::from_micros((16_743.0 / o.speed) as u64);
    let mut next_frame = Instant::now();

    for frame in 0..o.frames {
        // **The 1:1 exchange runs unpaced, and that is not an optimisation.**
        // Real-time pacing is what makes the *room* a demonstration of what
        // four people would see. A trade is 1,800 transfers of lockstep cable
        // traffic between two processes, with the console stalled on every
        // byte; pacing both ends to 59.7275 Hz and putting four emulators plus
        // a relay on one box adds enough jitter that a run occasionally loses a
        // byte, which arrives as `$FF` inside a name and decodes into something
        // plausible. `examples/link_two.rs --peer` — the shipped, proven
        // cross-process trade — runs its loop flat out, and so do we from the
        // moment the player has left the room.
        if trade_bot.is_none() {
            next_frame += period;
            std::thread::sleep(next_frame.saturating_duration_since(Instant::now()));
        } else {
            next_frame = Instant::now();
        }
        // The emulator drives itself; the relay is read, never waited on.
        let (tick, phase, peers) = {
            let s = shared.lock().unwrap();
            (s.tick, s.phase, s.peers.clone())
        };
        let id = *my_id.lock().unwrap();

        if let Some(p) = gb.plugin_mut::<LobbyPlugin>() {
            p.set_peers(peers.iter().filter(|p| p.id != id).cloned().collect());
        }

        if phase != last_phase {
            say(&format!("[{frame:6}] phase {} at tick {tick}", phase.name()));
            last_phase = phase;
            match phase {
                Phase::Mingle => target = walker.wander_target(&room),
                Phase::Talk => {
                    if let Some(p) = gb.plugin_mut::<LobbyPlugin>() {
                        p.say(o.phrase);
                    }
                    said = true;
                }
                _ => {}
            }
        }

        // Walking: ours in the room, the bot's once business starts.
        match phase {
            Phase::Waiting | Phase::Gather => {
                walker.towards(&mut gb, target.0, target.1);
            }
            Phase::Mingle => {
                if walker.towards(&mut gb, target.0, target.1) {
                    target = walker.wander_target(&room);
                }
            }
            Phase::Talk => {
                // Stand still. A bubble is charmap bytes in `wTileMap`, and
                // `RedrawRowOrColumn` owns that buffer the moment the view
                // scrolls - walking while speaking leaves two legible tiles of
                // a five-letter word. Standing still is also what a person does
                // when they are talking to somebody.
                walker.release(&mut gb);
                gb.clear_input();
                // Keep saying it for the whole phase. A phrase is up for
                // `BUBBLE_FRAMES` and the point of this phase is that four
                // screens can be photographed with four bubbles on them.
                if frame % 90 == 0 {
                    if let Some(p) = gb.plugin_mut::<LobbyPlugin>() {
                        p.say(o.phrase);
                    }
                }
            }
            Phase::Business => {
                if !business_started {
                    business_started = true;
                    say(&format!("business: {}", o.business.name()));
                    // **Leaving the shared room is part of starting a trade,
                    // and this is a finding rather than a convenience.** Remote
                    // avatars are solid to the player — `CollisionCheckOnLand`
                    // calls `IsSpriteInFrontOfPlayer` before it ever looks at
                    // the tile — so with four people converging on two consoles
                    // they simply block each other, exactly as four people would
                    // at two machines. A 1:1 session is 1:1: telling the relay
                    // we are away removes us from everyone's crowd, and removes
                    // everyone else from ours.
                    if o.business != Business::Watch {
                        walker.release(&mut gb);
                        gb.clear_input();
                        writeln!(sock, "away\t1").ok();
                    }
                    if o.business == Business::Battle && !flipped {
                        // ONE BYTE. `CableClubLeftGameboy` reads `wCurMap` and
                        // picks `LINK_STATE_START_TRADE` for the Trade Center,
                        // `LINK_STATE_START_BATTLE` for anything else — and the
                        // Colosseum is its twin: same size, same tileset, same
                        // two hidden events at (4,4) and (5,4).
                        gb.debug_write(W_CUR_MAP, MAP_COLOSSEUM);
                        flipped = true;
                        say("flipped wCurMap to COLOSSEUM ($F0)");
                    }
                }
                match o.business {
                    Business::Watch => {
                        if walker.towards(&mut gb, target.0, target.1) {
                            target = walker.wander_target(&room);
                        }
                    }
                    _ => {
                        // Walk back to the seat, then hand over to the shipped
                        // Cable Club bot, which knows the console dance.
                        let (x, y) = (gb.peek(W_X_COORD), gb.peek(W_Y_COORD));
                        if trade_bot.is_none() {
                            if (x, y) == seat {
                                // Let go of whatever the walker was holding
                                // before somebody else starts pressing keys - a
                                // direction left down is a direction held for
                                // ever, and it looks exactly like a bot that has
                                // stopped working.
                                walker.release(&mut gb);
                                gb.clear_input();
                                trade_bot = Some(TradeBot::in_trade_room(seat));
                                say("at my seat; pressing A at the console");
                            } else {
                                if frame % 90 == 0 {
                                    say(&format!(
                                        "[{frame:6}] back: ({x},{y}) -> {seat:?}"
                                    ));
                                }
                                // Row 4 is split by the two consoles, so route
                                // via row 6 rather than walking into them.
                                let via = if y != 6 && x != seat.0 {
                                    (x, 6)
                                } else if x != seat.0 {
                                    (seat.0, 6)
                                } else {
                                    seat
                                };
                                walker.towards(&mut gb, via.0, via.1);
                            }
                        }
                        if let Some(b) = &mut trade_bot {
                            b.act(&mut gb, o.verbose, &o.name);
                        }
                    }
                }
            }
            Phase::Done => done = true,
        }

        peer.run_frame(&mut gb);

        // Report position at the relay's rate, not the frame's.
        if frame % FRAMES_PER_TICK == 0 {
            let (me, saying, state) = match gb.plugin::<LobbyPlugin>() {
                Some(p) => (p.me(), p.saying().unwrap_or("").to_string(), p.state()),
                None => (Presence::default(), String::new(), State::Outside),
            };
            let step = ((frame / 8) % 4) as u8;
            let _ = writeln!(
                sock,
                "pos\t{}\t{}\t{}\t{}\t{}",
                me.x,
                me.y,
                me.facing.as_byte(),
                if state == State::InRoom { step } else { 0 },
                saying
            );
        }

        // Screenshots, keyed to the *relay's* tick — so a shot at tick 48 on
        // four screens really is one instant, not four nearby ones.
        if let Some(d) = &o.shots {
            if shot_ticks.first().is_some_and(|t| *t <= tick) {
                let at = shot_ticks.remove(0);
                let n = match at {
                    48 => "02-arrived",
                    72 => "03-walking",
                    96 => "04-passing",
                    120 => "05-crowd",
                    _ => "06-talking",
                };
                write_one(&d.join(format!("{n}-{}.png", o.name.to_lowercase())), &mut gb);
            }
        }

        if o.business == Business::Battle
            && !battle_seen
            && gb.peek(W_LINK_STATE) >= 0x03
        {
            battle_seen = true;
            say(&format!(
                "wLinkState reached {:#04x} — LINK_STATE_START_BATTLE",
                gb.peek(W_LINK_STATE)
            ));
            if let Some(d) = &o.shots {
                write_one(
                    &d.join(format!("07-battle-{}.png", o.name.to_lowercase())),
                    &mut gb,
                );
            }
        }
        if let Some(b) = &trade_bot {
            if b.traded() && !said {
                said = true;
            }
        }
        if let Some(fault) = gb.fault() {
            eprintln!("[{who}] FAULT: {fault}");
            break;
        }
        if done {
            break;
        }
    }

    // --- what happened ------------------------------------------------------
    let st = peer.stats();
    if let Some(d) = &o.shots {
        write_one(&d.join(format!("08-end-{}.png", o.name.to_lowercase())), &mut gb);
    }
    let (installs, art, ticks, touched) = match gb.plugin::<LobbyPlugin>() {
        Some(p) => (p.installs, p.art_installs, p.ticks, p.touched_saved_region()),
        None => (0, 0, 0, false),
    };
    say(&format!(
        "room installs {installs}, art blits {art}, lobby ticks {ticks}, \
         saved-region writes gated: {touched}"
    ));
    say(&format!(
        "cable: {} transfers, {} unanswered, {} torn; final map {:#04x} link {:#04x}",
        st.transfers,
        st.unanswered,
        st.torn,
        gb.peek(W_CUR_MAP),
        gb.peek(W_LINK_STATE),
    ));
    if o.business == Business::Battle {
        say(&format!(
            "battle attempt: LINK_STATE_START_BATTLE seen = {battle_seen}"
        ));
    }
    if let Some(b) = &trade_bot {
        say(&format!("trade bot: {} traded={}", b.stage_label(), b.traded()));
    }
    say("party:");
    let party = read_party(&mut gb);
    for m in &party {
        say(&format!("  {}", m.describe()));
    }
    if o.business == Business::Trade {
        // **Check the bytes, not that something arrived.** A byte lost on the
        // wire turns up as `$FF` inside a name, `$FF` is not a character, and a
        // decoder that skips what it does not know renders a corrupt "ROCKY"
        // as a perfectly plausible "Y". So the verdict is every field of the
        // arriving Pokémon, not the presence of a stranger in the party.
        let ok = party.iter().any(|m| {
            m.ot_name != o.name
                && !m.ot_name.is_empty()
                && (1..=100).contains(&m.level)
                && m.hp <= m.max_hp
                && m.max_hp > 0
                && m.nickname.chars().all(|c| c.is_ascii_uppercase())
                && !m.nickname.is_empty()
        });
        say(&format!(
            "TRADE VERDICT: {}",
            if ok {
                "a well-formed Pokemon whose original trainer is the other player"
            } else {
                "NOT a clean trade - see the party above"
            }
        ));
    }
    gb.flush_battery();
    peer.close();
    let _ = sock.shutdown(std::net::Shutdown::Both);
}

/// The 20x18 screen buffer with its walkability marked — the fastest way to
/// see what the engine thinks the room is, because `wTileMap` is exactly what
/// the collision test reads.
fn dump_screen(gb: &mut Gameboy, who: &str) {
    use terminalgb_plugin_gen1_lobby::gen1::{CLUB_WALKABLE, SCREEN_H, SCREEN_W, W_TILE_MAP};
    for row in 0..SCREEN_H {
        let mut line = String::new();
        for col in 0..SCREEN_W {
            let t = gb.peek(W_TILE_MAP + (row * SCREEN_W + col) as u16);
            line.push(if CLUB_WALKABLE.contains(&t) { '.' } else { '#' });
        }
        println!("[{who:<5}]   {line}");
    }
}

/// Distinct parties per player, so a trade is provable by whose original
/// trainer is on the arriving Pokémon rather than by something having moved.
fn party_spec(name: &str) -> String {
    let (species, nick, level, id) = match name {
        "RED" => ("Abra", "GLIMMER", 10, 4242),
        "BLUE" => ("Machop", "ROCKY", 11, 1717),
        "GREEN" => ("Pidgey", "FLAP", 9, 2323),
        "YELLOW" => ("Rattata", "RATTY", 8, 3131),
        _ => ("Nidoran M", "NIDO", 7, 5151),
    };
    format!(
        "trainer = {name}\n\
         species = {species}\nlevel = {level}\nnickname = {nick}\n\
         ot_name = {name}\not_id = {id}\n\
         moves = Tackle\n\
         species = Pikachu\nlevel = {level}\nnickname = SPARK\n\
         ot_name = {name}\not_id = {id}\n\
         moves = Thundershock, Growl\n"
    )
}

// ---------------------------------------------------------------------------
// demo: the relay here, one child process per player.
// ---------------------------------------------------------------------------

fn demo(
    rom: &Path,
    save: Option<&Path>,
    shots: Option<&Path>,
    players: usize,
    frames: u32,
    speed: f64,
    verbose: bool,
) {
    assert!(
        players >= 3 && players % 2 == 0,
        "players must be even and at least 3 — the consoles pair two at a time \
         over the link cable, which is how each of them gets into the Cable \
         Club at all (four is the smallest even number above three)"
    );
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind relay");
    let port = listener.local_addr().unwrap().port();
    println!("relay on 127.0.0.1:{port}, expecting {players} players");
    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = Arc::clone(&stop);
        std::thread::spawn(move || serve(listener, players, stop));
    }

    let exe = std::env::current_exe().expect("current exe");
    let names = ["RED", "BLUE", "GREEN", "YELLOW", "PINK", "GREY"];
    let mut kids = Vec::new();
    for i in 0..players {
        let name = names[i % names.len()];
        let pair = format!("target/worldlobby/cable-{}", i / 2);
        // Gather points: a loose ring in the middle of the hall, far enough
        // apart that nobody spawns inside somebody else.
        // A tight ring in the middle of the hall. The engine's own visibility
        // window is four tiles up, four down, four left and five right, so
        // these are chosen to put every player inside every other player's -
        // which is what makes four screenshots of one instant mean something.
        let gather = [
            (15u8, 13u8),
            (17, 13),
            (15, 15),
            (17, 15),
            (16, 14),
            (14, 14),
        ][i % 6];
        // Pair 0 trades with each other; pair 1 tries a battle; anybody else
        // keeps walking so the room is not empty while they do it.
        let business = match i / 2 {
            0 => "trade",
            1 => "battle",
            _ => "watch",
        };
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("play")
            .arg("--name")
            .arg(name)
            .arg("--rom")
            .arg(rom)
            .arg("--port")
            .arg(port.to_string())
            .arg("--pair-dir")
            .arg(&pair)
            .arg("--gather")
            .arg(format!("{},{}", gather.0, gather.1))
            .arg("--phrase")
            .arg(PHRASES[i % PHRASES.len()])
            .arg("--business")
            .arg(business)
            .arg("--seed")
            .arg((i as u64 + 1).to_string())
            .arg("--frames")
            .arg(frames.to_string())
            .arg("--speed")
            .arg(speed.to_string());
        if let Some(s) = save {
            cmd.arg("--save").arg(s);
        }
        if let Some(s) = shots {
            cmd.arg("--shots").arg(s);
        }
        if verbose {
            cmd.arg("--verbose");
        }
        kids.push(cmd.spawn().expect("spawn player"));
        // Two consoles started in lockstep never drift, so their Gen 1
        // handshakes never interleave and the cable never comes up
        // (`docs/link-cable.md`). Separate processes on a loaded box drift on
        // their own, but a deliberate stagger makes it reproducible.
        std::thread::sleep(Duration::from_millis(350));
    }
    for mut k in kids {
        let _ = k.wait();
    }
    stop.store(true, Ordering::Relaxed);
    println!("all players finished");
}

// ---------------------------------------------------------------------------

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mode = if args.first().map(|s| s.as_str()) == Some("play") {
        args.remove(0);
        "play"
    } else {
        "demo"
    };

    let mut rom: Option<PathBuf> = None;
    let mut save: Option<PathBuf> = None;
    let mut shots: Option<PathBuf> = None;
    let mut name = "RED".to_string();
    let mut port = 0u16;
    let mut pair_dir = PathBuf::from("target/worldlobby/cable-0");
    let mut gather = (16u8, 12u8);
    let mut phrase = PHRASES[0];
    let mut business = Business::Watch;
    let mut seed = 1u64;
    let mut players = 4usize;
    let mut frames = 60 * 60 * 12;
    let mut speed = 1.0f64;
    let mut verbose = false;

    let pair = |s: &str| -> (u8, u8) {
        let mut it = s.split(',').map(|v| v.trim().parse::<u8>().unwrap_or(0));
        (it.next().unwrap_or(0), it.next().unwrap_or(0))
    };

    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--rom" => rom = it.next().map(PathBuf::from),
            "--save" => save = it.next().map(PathBuf::from),
            "--shots" => shots = it.next().map(PathBuf::from),
            "--name" => name = it.next().unwrap_or(name),
            "--port" => port = it.next().and_then(|s| s.parse().ok()).unwrap_or(0),
            "--pair-dir" => pair_dir = it.next().map(PathBuf::from).unwrap_or(pair_dir),
            "--gather" => gather = it.next().map(|s| pair(&s)).unwrap_or(gather),
            "--phrase" => {
                let p = it.next().unwrap_or_default();
                phrase = PHRASES
                    .iter()
                    .find(|q| **q == p)
                    .copied()
                    .unwrap_or(PHRASES[0]);
            }
            "--business" => {
                business = Business::parse(&it.next().unwrap_or_default())
            }
            "--seed" => seed = it.next().and_then(|s| s.parse().ok()).unwrap_or(seed),
            "--players" => {
                players = it.next().and_then(|s| s.parse().ok()).unwrap_or(players)
            }
            "--frames" => {
                frames = it.next().and_then(|s| s.parse().ok()).unwrap_or(frames)
            }
            "--speed" => speed = it.next().and_then(|s| s.parse().ok()).unwrap_or(speed),
            "--verbose" => verbose = true,
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
    }

    let Some(rom) = rom else {
        eprintln!(
            "usage: worldlobby --rom <blue.gb> --save <cableclub.sav> \
             [--shots <dir>] [--players 4] [--frames N] [--verbose]\n\
             \n\
             Starts a lobby relay and one CHILD PROCESS per player, each with \
             its own staged cartridge and its own walking. Four independent \
             emulator instances, not four humans — see docs/worldlobby.md.\n\
             \n\
             `worldlobby play --name RED --port N …` is one player; the demo \
             spawns those for you."
        );
        std::process::exit(2);
    };

    // The default room is what every player installs; refuse before anything
    // starts if it could not fit, rather than half way through four processes.
    if let Err(e) = DEFAULT_ROOM.check() {
        eprintln!("room refused: {e:?}");
        std::process::exit(1);
    }

    match mode {
        "play" => play(PlayOpts {
            name,
            rom,
            save,
            shots,
            port,
            pair_dir,
            gather,
            phrase,
            business,
            seed,
            frames,
            speed,
            verbose,
        }),
        _ => demo(
            &rom,
            save.as_deref(),
            shots.as_deref(),
            players,
            frames,
            speed,
            verbose,
        ),
    }
}

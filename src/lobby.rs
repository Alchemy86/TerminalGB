//! The World Trade Centre's relay client — the network half of the shared
//! room, which deliberately does **not** live in the plugin.
//!
//! `terminalgb_plugin_gen1_lobby` is pure memory work: it installs the room,
//! writes remote avatars into sprite slots and draws what people say, and it
//! has no dependency but the plugin API. A [`Plugin`] is handed a machine
//! *during a frame the emulator has already decided to run*, and a socket is
//! not that — a plugin that blocked on one would be the audio hand-off bug
//! again (`docs/audio.md` §4c). So the socket is here, in the frontend, and it
//! never blocks the frame: a reader thread keeps the latest snapshot in a
//! mutex and the frame loop does a `try_lock`.
//!
//! The wire is tab-separated lines, no dependency and no escaping problem —
//! every name and phrase has already been filtered to Gen 1's charmap, which
//! has no tab. It is the same protocol `examples/worldlobby.rs` speaks, so the
//! captain's own terminal and a scripted walker are the same kind of client.
//!
//! [`Plugin`]: terminalgb_plugin_gen1_lobby::LobbyPlugin

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminalgb_plugin_gen1_lobby::avatar::{Peer, Presence};
use terminalgb_plugin_gen1_lobby::gen1::Facing;

/// How many emulated frames between position reports. 8 Hz: a Gen 1 walk step
/// is 16 frames ≈ 268 ms, so this is twice what "smooth" needs, and a late
/// packet costs one stale frame of somebody else's avatar, never a desync.
pub const FRAMES_PER_REPORT: u32 = 8;

/// A connection to a lobby relay.
pub struct LobbyClient {
    sock: TcpStream,
    peers: Arc<Mutex<Vec<Peer>>>,
    id: Arc<Mutex<u32>>,
    alive: Arc<AtomicBool>,
    frames: u32,
}

impl LobbyClient {
    /// Join a relay. `addr` is `host:port`; the name is what other players see
    /// over your head, and is filtered to what the cartridge can draw.
    pub fn join(addr: &str, name: &str) -> std::io::Result<LobbyClient> {
        let mut sock = TcpStream::connect(addr)?;
        sock.set_nodelay(true).ok();
        writeln!(
            sock,
            "join\t{}",
            terminalgb_plugin_gen1_lobby::gen1::filter_to_charmap(name)
        )?;
        writeln!(sock, "ready")?;
        let peers: Arc<Mutex<Vec<Peer>>> = Arc::new(Mutex::new(Vec::new()));
        let id = Arc::new(Mutex::new(0u32));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let rx = sock.try_clone()?;
            let peers = Arc::clone(&peers);
            let id = Arc::clone(&id);
            let alive = Arc::clone(&alive);
            std::thread::spawn(move || read_loop(rx, peers, id, alive));
        }
        Ok(LobbyClient {
            sock,
            peers,
            id,
            alive,
            frames: 0,
        })
    }

    pub fn connected(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// Everybody else in the room, as of the last snapshot.
    pub fn peers(&self) -> Vec<Peer> {
        let me = *self.id.lock().unwrap();
        self.peers
            .lock()
            .map(|p| p.iter().filter(|q| q.id != me).cloned().collect())
            .unwrap_or_default()
    }

    /// Report where we are, at [`FRAMES_PER_REPORT`] granularity. Called every
    /// frame; sends on one in eight, and a failed send is a lost cable, not a
    /// stalled emulator.
    pub fn publish(&mut self, me: Presence, step: u8, says: &str) {
        self.frames = self.frames.wrapping_add(1);
        if self.frames % FRAMES_PER_REPORT != 0 {
            return;
        }
        let line = format!(
            "pos\t{}\t{}\t{}\t{}\t{}\n",
            me.x,
            me.y,
            me.facing.as_byte(),
            step,
            says
        );
        if self.sock.write_all(line.as_bytes()).is_err() {
            self.alive.store(false, Ordering::Relaxed);
        }
    }

    /// Stop appearing in the room — for a trade or a battle, which is a
    /// private 1:1 session and not part of the crowd.
    pub fn set_away(&mut self, away: bool) {
        let _ = writeln!(self.sock, "away\t{}", u8::from(away));
    }
}

fn read_loop(
    rx: TcpStream,
    peers: Arc<Mutex<Vec<Peer>>>,
    id: Arc<Mutex<u32>>,
    alive: Arc<AtomicBool>,
) {
    let reader = BufReader::new(rx);
    let mut building: Vec<Peer> = Vec::new();
    for line in reader.lines() {
        let Ok(line) = line else { break };
        let f: Vec<&str> = line.split('\t').collect();
        match f.first().copied() {
            Some("you") => {
                if let Some(v) = f.get(1).and_then(|s| s.parse().ok()) {
                    *id.lock().unwrap() = v;
                }
            }
            Some("snap") => building.clear(),
            Some("peer") => {
                let g = |i: usize| f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0u8);
                let says = f.get(7).copied().unwrap_or("");
                building.push(Peer {
                    id: f.get(1).and_then(|v| v.parse().ok()).unwrap_or(0),
                    name: f.get(2).unwrap_or(&"?").to_string(),
                    x: g(3),
                    y: g(4),
                    facing: Facing::from_byte(g(5)),
                    step: g(6),
                    says: (!says.is_empty()).then(|| says.to_string()),
                });
            }
            Some("end") => {
                if let Ok(mut p) = peers.lock() {
                    *p = std::mem::take(&mut building);
                }
            }
            _ => {}
        }
    }
    alive.store(false, Ordering::Relaxed);
}

// ---------------------------------------------------------------------------
// The relay itself — hosting a room rather than joining one.
// ---------------------------------------------------------------------------

/// The relay's tick rate. A Gen 1 walk step is 16 frames ≈ 268 ms, so 8 Hz is
/// twice what "smooth" needs and a late packet costs one stale frame of
/// somebody else's avatar, never a desync.
pub const TICK_HZ: u64 = 8;

/// What the room is doing, as the **relay** sees it.
///
/// The relay owns this rather than any client, so that several processes act
/// together without any one of them being in charge, and so that a screenshot
/// taken at a given tick really is the same instant on every screen.
///
/// It is part of the wire, which is why it lives here beside the relay and not
/// in `examples/worldlobby.rs`: the demo's driven players and the shipped
/// frontend's driven partner are the same program reading the same field, so
/// there must be exactly one definition of it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// Not everybody expected has arrived yet.
    #[default]
    Waiting,
    /// Walk to your gathering point.
    Gather,
    /// Wander.
    Mingle,
    /// Stand still and say something.
    Talk,
    /// Go and do the thing you came for — a trade, a battle, or nothing.
    Business,
    /// The demonstration is over; the relay shuts down.
    Done,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Phase::Waiting => "waiting",
            Phase::Gather => "gather",
            Phase::Mingle => "mingle",
            Phase::Talk => "talk",
            Phase::Business => "business",
            Phase::Done => "done",
        }
    }

    pub fn parse(s: &str) -> Phase {
        match s {
            "gather" => Phase::Gather,
            "mingle" => Phase::Mingle,
            "talk" => Phase::Talk,
            "business" => Phase::Business,
            "done" => Phase::Done,
            _ => Phase::Waiting,
        }
    }

    /// The demonstration's schedule, in relay ticks after everybody was in the
    /// room. Ends: this is a capture run with a beginning and an end.
    pub fn at(tick: u64) -> Phase {
        match tick {
            0..=47 => Phase::Gather,
            48..=127 => Phase::Mingle,
            128..=175 => Phase::Talk,
            176..=760 => Phase::Business,
            _ => Phase::Done,
        }
    }

    /// The same schedule with **no end** — what a room a person is sitting in
    /// wants.
    ///
    /// [`Phase::at`] reaches `Done` at tick 760, i.e. **95 seconds** at
    /// [`TICK_HZ`], and `Done` shuts the relay down. That is right for a
    /// capture run and wrong for somebody playing: their room would go quiet
    /// after a minute and a half with nothing said. Clamping the tick inside
    /// the `Business` band is the whole difference, so a driven partner
    /// sequences identically either way.
    pub fn open(tick: u64) -> Phase {
        Phase::at(tick.min(700))
    }
}

/// One connected client, as the relay holds it.
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
    /// Set once the client is standing at its console, waiting to press A.
    seated: bool,
}

#[derive(Default)]
struct Room {
    bodies: BTreeMap<u32, Body>,
    tick: u64,
    started: bool,
}

/// How a hosted room behaves.
pub struct RelayOptions {
    /// How many clients must be connected **and** `ready` before the clock
    /// starts. Until then the phase is `waiting`, which is the signal both ends
    /// of a cable start on (`docs/worldlobby.md` §6.4).
    pub expect: usize,
    /// Names the phase for a tick once the room has started. Defaults to
    /// [`Phase::open`] — the demonstration's sequence, with no end.
    pub schedule: Box<dyn Fn(u64) -> Phase + Send>,
    /// Stop accepting connections past this many. `None` is an open room.
    pub max_clients: Option<usize>,
}

impl Default for RelayOptions {
    fn default() -> RelayOptions {
        RelayOptions {
            expect: 1,
            schedule: Box::new(Phase::open),
            max_clients: None,
        }
    }
}

/// A room this process is hosting.
pub struct Hosted {
    /// What to put in `GB_LOBBY`.
    pub addr: String,
    pub port: u16,
    /// Set it to stop the relay. Also set by the relay itself when the
    /// schedule reaches [`Phase::Done`].
    pub stop: Arc<AtomicBool>,
}

/// Bind a relay and serve it on background threads.
///
/// `bind` is anything `TcpListener::bind` takes; `127.0.0.1:0` asks the kernel
/// for a free port and [`Hosted::addr`] reports which.
pub fn host(bind: &str, opts: RelayOptions) -> std::io::Result<Hosted> {
    let listener = TcpListener::bind(bind)?;
    let local = listener.local_addr()?;
    let stop = Arc::new(AtomicBool::new(false));
    let hosted = Hosted {
        addr: format!("127.0.0.1:{}", local.port()),
        port: local.port(),
        stop: Arc::clone(&stop),
    };
    std::thread::spawn(move || serve(listener, opts, stop));
    Ok(hosted)
}

/// Run a relay on `listener` until `stop`.
///
/// One snapshot per tick to everybody, rather than a fan-out per message. At
/// four players that is indistinguishable; it is the shape that stays
/// affordable at a hundred.
pub fn serve(listener: TcpListener, opts: RelayOptions, stop: Arc<AtomicBool>) {
    let RelayOptions {
        expect,
        schedule,
        max_clients,
    } = opts;
    let room = Arc::new(Mutex::new(Room::default()));
    let clients: Arc<Mutex<Vec<(u32, TcpStream)>>> = Arc::new(Mutex::new(Vec::new()));

    {
        let room = Arc::clone(&room);
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
                    let mut l = room.lock().unwrap();
                    let all_ready =
                        l.bodies.len() >= expect && l.bodies.values().all(|b| b.ready);
                    if all_ready {
                        l.started = true;
                    }
                    if l.started {
                        l.tick += 1;
                    }
                    phase = if l.started {
                        schedule(l.tick)
                    } else {
                        Phase::Waiting
                    };
                    let seated = l.bodies.values().filter(|b| b.seated).count();
                    line.push_str(&format!(
                        "snap\t{}\t{}\t{}\t{}\n",
                        l.tick,
                        phase.name(),
                        l.bodies.len(),
                        seated
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

    listener.set_nonblocking(false).expect("blocking listener");
    let mut next_id = 1u32;
    for stream in listener.incoming() {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let Ok(mut stream) = stream else { continue };
        let id = next_id;
        next_id += 1;
        if stream.write_all(format!("you\t{id}\n").as_bytes()).is_err() {
            continue;
        }
        let Ok(tx) = stream.try_clone() else { continue };
        clients.lock().unwrap().push((id, tx));
        let room = Arc::clone(&room);
        std::thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let f: Vec<&str> = line.split('\t').collect();
                let mut l = room.lock().unwrap();
                let b = l.bodies.entry(id).or_default();
                match f.first().copied() {
                    Some("join") => b.name = f.get(1).unwrap_or(&"?").to_string(),
                    Some("ready") => b.ready = true,
                    Some("away") => b.away = f.get(1) == Some(&"1"),
                    Some("seated") => b.seated = true,
                    Some("pos") => {
                        let g =
                            |i: usize| f.get(i).and_then(|s| s.parse().ok()).unwrap_or(0);
                        b.x = g(1);
                        b.y = g(2);
                        b.facing = g(3);
                        b.step = g(4);
                        b.says = f.get(5).unwrap_or(&"").to_string();
                    }
                    _ => {}
                }
            }
            room.lock().unwrap().bodies.remove(&id);
        });
        if max_clients.is_some_and(|m| next_id as usize > m) {
            break;
        }
    }
}

/// Where to join, from the environment. `GB_LOBBY=host:port`.
///
/// An environment variable rather than a config key on purpose: a lobby
/// address is per-session (which room, whose relay), not a property of the
/// installation, and the demo hands it to a child process.
pub fn from_env(name: &str) -> Option<(LobbyClient, String)> {
    let addr = std::env::var("GB_LOBBY").ok()?;
    let addr = addr.trim().to_string();
    if addr.is_empty() {
        return None;
    }
    match LobbyClient::join(&addr, name) {
        Ok(c) => Some((c, format!("lobby: joined {addr} as {name}"))),
        Err(e) => {
            // An unreachable relay costs the room and nothing else — the
            // emulator still runs the cartridge. Same rule as an unplugged
            // cable.
            eprintln!("terminalgb: lobby {addr}: {e}");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Taking a seat: one call that hosts (or joins) a room, optionally boots a
// driven partner, and hands the terminal to the person at the keyboard.
// ---------------------------------------------------------------------------

/// The driven console the human meets in the room.
///
/// **This is `examples/worldlobby.rs`'s `play` subcommand, spawned as a child
/// process — not a second implementation of it.** A driven player is a walker,
/// a phase machine, a screenshot writer and the Cable Club bot; none of that
/// belongs in the shipped emulator, and having two of it would mean the
/// captain's partner and the demonstration's players could drift apart. So the
/// frontend's job is only to *find* that binary and hand it the room.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub struct PartnerOptions {
    /// The `worldlobby` binary. `None` searches — see [`find_partner_bin`].
    pub bin: Option<std::path::PathBuf>,
    /// The name over its head.
    pub name: String,
    /// Its cartridge. Always staged into `target/` before it is opened, so the
    /// driven console can never reach the player's own save.
    pub rom: std::path::PathBuf,
    pub save: Option<std::path::PathBuf>,
    /// How long it plays for before it stops.
    pub frames: u32,
    pub verbose: bool,
}

/// Look for the driven-partner binary beside this executable.
///
/// `$GB_LOBBY_PARTNER` overrides. Otherwise: the same directory (which is where
/// it is when the frontend is itself `target/release/examples/terminal`), then
/// an `examples/` subdirectory (where it is when the frontend is the installed
/// `terminalgb` binary in `target/release`).
///
/// Returning `None` is not a failure. A room with nobody in it is still a room,
/// and the second seat can be filled by another person running the same
/// command — see [`take_seat`], which says so rather than refusing to start.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub fn find_partner_bin() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("GB_LOBBY_PARTNER") {
        let p = std::path::PathBuf::from(p);
        return p.is_file().then_some(p);
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [
        dir.join("worldlobby"),
        dir.join("examples").join("worldlobby"),
    ]
    .into_iter()
    .find(|c| c.is_file())
}

/// What a person's seat in the room needs.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub struct SeatOptions {
    /// The cartridge the person plays.
    pub rom: std::path::PathBuf,
    /// Their save. `None` means whatever sits beside the ROM, as usual.
    pub save: Option<std::path::PathBuf>,
    /// The name over their head.
    pub name: String,
    /// `Some(addr)` joins somebody else's room; `None` hosts one here.
    pub join: Option<String>,
    /// Boot a driven console to meet.
    pub partner: Option<PartnerOptions>,
    /// Play a **copy** of the cartridge and save out of `target/`, rather than
    /// the real ones.
    ///
    /// `examples/worldlobby.rs` sets this: a capture run must not be able to
    /// write to the captain's cartridge at all. The shipped `--lobby` does
    /// **not**, because it is an ordinary launch of the person's own game —
    /// silently playing a throwaway copy would lose whatever they did after
    /// they left the Cable Club. Neither choice risks the save inside the room
    /// itself: `wLinkState == LINK_STATE_IN_CABLE_CLUB` is what makes
    /// `DrawStartMenu` print RESET where SAVE would be (`docs/worldlobby.md`
    /// §5).
    pub stage_self: bool,
}

/// Where staged copies go. One directory, so a second run overwrites rather
/// than accumulating cartridges.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
const SCRATCH: &str = "target/worldlobby";

/// Copy a cartridge and its save into `target/`, the way
/// `examples/support/gen1_cable_club.rs` does.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
fn stage(
    which: &str,
    rom: &std::path::Path,
    save: Option<&std::path::Path>,
) -> std::io::Result<std::path::PathBuf> {
    let dir = std::path::Path::new(SCRATCH).join(which);
    std::fs::create_dir_all(&dir)?;
    let name = rom
        .file_name()
        .ok_or_else(|| std::io::Error::other("cartridge path has no filename"))?;
    let dest = dir.join(name);
    std::fs::copy(rom, &dest)?;
    // `Battery` writes `<stem>.sav`: the extension is REPLACED, not appended.
    let sav = dest.with_extension("sav");
    let _ = std::fs::remove_file(&sav);
    let beside = save
        .map(|s| s.to_path_buf())
        .unwrap_or_else(|| rom.with_extension("sav"));
    if beside.is_file() {
        std::fs::copy(&beside, &sav)?;
    }
    Ok(dest)
}

/// Spawn the driven partner against a room, with its output in a log file.
///
/// Its stdout would fight the alternate screen for the terminal, so it goes to
/// `target/worldlobby/partner.log`, which is returned.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub fn spawn_partner(
    port: u16,
    pair_dir: &std::path::Path,
    o: &PartnerOptions,
) -> std::io::Result<(std::process::Child, std::path::PathBuf)> {
    let bin = match o.bin.clone().or_else(find_partner_bin) {
        Some(b) => b,
        None => {
            return Err(std::io::Error::other(
                "no `worldlobby` binary beside this one — build it with \
                 `cargo build --release --example worldlobby`, or set \
                 $GB_LOBBY_PARTNER",
            ))
        }
    };
    // NOT staged here: `worldlobby play` stages its own copy (and patches a
    // distinct party into it, so "a trade happened" is provable). Staging it
    // twice hands it a cartridge whose `.sav` it then cannot find — measured,
    // as a panic in `patch_party`.
    std::fs::create_dir_all(pair_dir)?;
    let log = std::path::Path::new(SCRATCH).join("partner.log");
    let out = std::fs::File::create(&log)?;

    let mut cmd = std::process::Command::new(&bin);
    cmd.arg("play")
        .arg("--name")
        .arg(&o.name)
        .arg("--rom")
        .arg(&o.rom)
        .arg("--port")
        .arg(port.to_string())
        .arg("--pair-dir")
        .arg(pair_dir)
        .arg("--gather")
        .arg("14,12")
        .arg("--phrase")
        .arg("HI!")
        // `await` is "wander, then stand at your console and keep offering a
        // trade" — what a friend waiting at a machine looks like, and the only
        // business that does not need a second scripted console to agree with.
        .arg("--business")
        .arg("await")
        .arg("--seed")
        .arg("7")
        .arg("--frames")
        .arg(o.frames.to_string())
        .arg("--speed")
        .arg("1");
    if let Some(s) = &o.save {
        cmd.arg("--save").arg(s);
    }
    if o.verbose {
        cmd.arg("--verbose");
    }
    cmd.stdout(out.try_clone()?).stderr(out);
    Ok((cmd.spawn()?, log))
}

/// Host or join a room, optionally boot a partner, and run the terminal
/// frontend in it.
///
/// **Nothing here is a special path for the person.** The frontend attaches the
/// same `gen1-lobby` plugin a scripted player does, joins the same relay over
/// the same tab-separated wire, and pairs on the cable through the same
/// auto-discovered peer (`link = auto`, the default). The only difference is
/// where the keypresses come from, and the emulator cannot tell — both arrive
/// as `Gameboy::keydown`.
///
/// The two things it *does* arrange are the two that are not the frontend's by
/// default: the cable must belong to the real second console (so the built-in
/// virtual trade partner must not claim it), and `link = auto` must find that
/// console rather than a stranger's, which is what the private pair directory
/// is for.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
pub fn take_seat(o: SeatOptions) -> std::io::Result<()> {
    use terminalgb_plugin_gen1_lobby::room::DEFAULT_ROOM;

    // Refuse before anything starts if the room could not fit, rather than
    // half way through spawning a child process.
    if let Err(e) = DEFAULT_ROOM.check() {
        return Err(std::io::Error::other(format!("room refused: {e:?}")));
    }

    let mine = if o.stage_self {
        stage("you", &o.rom, o.save.as_deref())?
    } else {
        if let Some(s) = &o.save {
            // Every other launch takes the save from beside the cartridge, so
            // an explicit one is only meaningful with `stage_self`. Say so
            // rather than accepting it and ignoring it.
            eprintln!(
                "terminalgb: lobby: ignoring --save {} — an unstaged seat reads \
                 the save beside the cartridge, as every other launch does",
                s.display()
            );
        }
        o.rom.clone()
    };

    // --- the room ---------------------------------------------------------
    let (addr, stop) = match &o.join {
        Some(a) => (a.clone(), None),
        None => {
            // Two seats when a partner is coming, one when it is not: the
            // relay's clock starts when everybody *expected* is `ready`, and
            // that signal is what stops one console racing ahead of the other
            // (`docs/worldlobby.md` §6.4).
            let expect = if o.partner.is_some() { 2 } else { 1 };
            let h = host(
                "127.0.0.1:0",
                RelayOptions {
                    expect,
                    ..RelayOptions::default()
                },
            )?;
            println!("lobby: hosting a room on {}", h.addr);
            (h.addr, Some(h.stop))
        }
    };

    // --- the partner ------------------------------------------------------
    // A private pair directory when we booted the partner, so `link = auto`
    // finds exactly that console and not a stranger's emulator on the same
    // box. Without a partner the ordinary discovery directory is right: that
    // is how a second *person* running the same command pairs with you.
    let port: u16 = addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    let mut child = None;
    if let Some(p) = &o.partner {
        let pair = std::path::Path::new(SCRATCH).join("cable-human");
        match spawn_partner(port, &pair, p) {
            Ok((kid, log)) => {
                println!("lobby: {} is walking in; log: {}", p.name, log.display());
                std::env::set_var("GB_LINK_DIR", &pair);
                child = Some(kid);
            }
            // Fail-soft, and the same rule as an unreachable relay: a missing
            // partner costs the partner and nothing else. The room still
            // exists, and a second person running the same command fills it.
            Err(e) => eprintln!(
                "terminalgb: lobby: no partner ({e}). You can still be joined by \
                 somebody running `terminalgb --lobby={addr}`."
            ),
        }
    }

    // --- what the frontend needs told -------------------------------------
    std::env::set_var("GB_LOBBY", &addr);
    std::env::set_var("GB_LOBBY_NAME", &o.name);
    std::env::set_var("GB_PLUGINS", plugin_list_for_lobby());
    // The room is a Cable Club feature and the other end of the cable is a
    // real second console, so the built-in *virtual* trade partner must not
    // also claim it. A cable has two ends and `gen1-link` is one of them.
    std::env::set_var("GB_GEN1_TRADE", "");

    println!();
    println!(
        "  You are {}. Walk to the Cable Club receptionist, say yes, and",
        o.name
    );
    println!("  choose TRADE CENTER. Once both consoles are in, the room becomes");
    println!("  40x28 with our own floor and you will see each other walk about.");
    println!();

    crate::gameboy::run_terminal(Some(mine));

    if let Some(s) = stop {
        s.store(true, Ordering::Relaxed);
    }
    if let Some(mut k) = child {
        let _ = k.kill();
        let _ = k.wait();
    }
    Ok(())
}

/// The plugin list to run the room with: whatever the person configured, minus
/// anything that would claim the link port, plus `gen1-lobby`.
///
/// It is spelled through `GB_PLUGINS` because that is the channel that already
/// wins over `config.toml`. Subtracting rather than replacing matters: the
/// captain's own `plugins = gen1-link, trainer` must keep its trainer panel in
/// the room, and must lose only the one plugin that cannot coexist with a real
/// second console on the cable.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
fn plugin_list_for_lobby() -> String {
    let configured = match std::env::var("GB_PLUGINS") {
        Ok(v) => v,
        Err(_) => crate::screen::config::Config::load().plugins.join(","),
    };
    plugin_list_from(&configured)
}

/// The pure half of [`plugin_list_for_lobby`], so the rule can be tested
/// without reaching for a process-wide environment variable other tests in
/// this crate also write.
#[cfg(all(not(target_arch = "wasm32"), feature = "tui"))]
fn plugin_list_from(configured: &str) -> String {
    const LOBBY: &str = "gen1-lobby";
    /// Plugins that hold `SERIAL_LINK`. At most one thing may answer the
    /// cable, and in the room that thing is the other console.
    const CABLE: &[&str] = &["gen1-link", "gen1-battle"];

    let mut names: Vec<String> = configured
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !CABLE.contains(&s.as_str()))
        .collect();
    if !names.iter().any(|n| n == LOBBY) {
        names.push(LOBBY.to_string());
    }
    names.join(",")
}

#[cfg(all(test, not(target_arch = "wasm32"), feature = "tui"))]
mod seat_tests {
    /// The room never ends, and the demonstration always does. Both halves
    /// matter: a capture run that ran for ever would never write its last
    /// screenshot, and a person's room that stopped after 95 seconds would go
    /// quiet with nothing said.
    #[test]
    fn an_open_room_never_reaches_done() {
        use super::Phase;
        assert_eq!(Phase::at(1000), Phase::Done);
        for t in [0u64, 47, 176, 760, 761, 10_000, u64::MAX] {
            assert_ne!(Phase::open(t), Phase::Done, "tick {t}");
        }
        // ...and it is the same sequence up to that point, so a driven partner
        // behaves identically in a demonstration and in somebody's room.
        for t in 0..=700u64 {
            assert_eq!(Phase::open(t), Phase::at(t), "tick {t}");
        }
    }

    /// The one thing the lobby must take away from the configuration, and the
    /// one thing it must add. Anything else the person asked for is theirs.
    #[test]
    fn the_lobby_frees_the_cable_and_keeps_everything_else() {
        // The captain's own configuration, plus one more.
        let list = super::plugin_list_from("gen1-link, trainer, agent");
        let names: Vec<&str> = list.split(',').collect();
        assert!(!names.contains(&"gen1-link"), "{list}");
        assert!(names.contains(&"trainer"), "{list}");
        assert!(names.contains(&"agent"), "{list}");
        assert!(names.contains(&"gen1-lobby"), "{list}");
    }

    /// Naming it twice must not attach it twice.
    #[test]
    fn naming_the_lobby_yourself_is_not_an_error() {
        assert_eq!(super::plugin_list_from("gen1-lobby"), "gen1-lobby");
        assert_eq!(super::plugin_list_from(""), "gen1-lobby");
    }
}

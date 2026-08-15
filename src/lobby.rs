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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use terminalgb_plugin_gen1_lobby::avatar::{Peer, Presence};
use terminalgb_plugin_gen1_lobby::gen1::Facing;

/// How many emulated frames between position reports. 8 Hz: a Gen 1 walk step
/// is 16 frames ≈ 268 ms, so this is twice what "smooth" needs, and a late
/// packet costs one stale frame of somebody else's avatar, never a desync.
pub const FRAMES_PER_REPORT: u32 = 8;

/// How long to wait for a relay to say hello before deciding whatever is on
/// that port is not one. Loopback, so this is generous by three orders of
/// magnitude; it exists so a *foreign* program holding the well-known port
/// cannot wedge the emulator's startup (the same rule as every other sink here:
/// nothing may block the frame loop, and nothing may block getting to it).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1500);

/// A connection to a lobby relay.
pub struct LobbyClient {
    sock: TcpStream,
    peers: Arc<Mutex<Vec<Peer>>>,
    id: Arc<Mutex<u32>>,
    alive: Arc<AtomicBool>,
    frames: u32,
    /// The name the **relay** gave us, which is not always the one we asked
    /// for — see [`unique_name`].
    name: String,
}

impl LobbyClient {
    /// Join a relay. `addr` is `host:port`; the name is what other players see
    /// over your head, and is filtered to what the cartridge can draw.
    ///
    /// The relay's `you` line is read **before** this returns, so a port that
    /// belongs to something else is an `Err` here rather than a room that never
    /// says anything. The name is read too: two people who both asked to be RED
    /// cannot both be RED, and the relay is the only party that can see that
    /// (see [`unique_name`]).
    pub fn join(addr: &str, name: &str) -> std::io::Result<LobbyClient> {
        let mut sock = TcpStream::connect(addr)?;
        sock.set_nodelay(true).ok();
        sock.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).ok();
        let asked = terminalgb_plugin_gen1_lobby::gen1::filter_to_charmap(name);
        writeln!(sock, "join\t{asked}")?;
        writeln!(sock, "ready")?;

        // The buffered reader is built here and *handed to the thread*: a
        // snapshot can arrive interleaved with the handshake, and a second
        // reader over the same socket would drop whatever the first had already
        // pulled into its buffer.
        let mut reader = BufReader::new(sock.try_clone()?);
        let mut line = String::new();
        let mut id = 0u32;
        let mut given: Option<String> = None;
        let mut saw_you = false;
        // `you` comes on accept and `name` comes back from our `join`, so both
        // are due at once; anything else in between is a snapshot, which is
        // resent at TICK_HZ and can be dropped without consequence.
        while given.is_none() {
            line.clear();
            if reader.read_line(&mut line)? == 0 {
                return Err(std::io::Error::other("the relay hung up"));
            }
            let f: Vec<&str> = line.trim_end().split('\t').collect();
            match f.first().copied() {
                Some("you") => {
                    saw_you = true;
                    id = f.get(1).and_then(|v| v.parse().ok()).unwrap_or(0);
                }
                Some("name") => given = f.get(1).map(|v| v.to_string()),
                // Not a relay at all: something else owns this port.
                _ if !saw_you => {
                    return Err(std::io::Error::other(
                        "that port is not a TerminalGB lobby",
                    ))
                }
                _ => {}
            }
        }
        sock.set_read_timeout(None).ok();
        reader.get_mut().set_read_timeout(None).ok();

        let peers: Arc<Mutex<Vec<Peer>>> = Arc::new(Mutex::new(Vec::new()));
        let id = Arc::new(Mutex::new(id));
        let alive = Arc::new(AtomicBool::new(true));
        {
            let peers = Arc::clone(&peers);
            let id = Arc::clone(&id);
            let alive = Arc::clone(&alive);
            std::thread::spawn(move || read_loop(reader, peers, id, alive));
        }
        Ok(LobbyClient {
            sock,
            peers,
            id,
            alive,
            frames: 0,
            name: given.unwrap_or(asked),
        })
    }

    pub fn connected(&self) -> bool {
        self.alive.load(Ordering::Relaxed)
    }

    /// The name the relay gave us. Show this, not the one that was asked for.
    pub fn name(&self) -> &str {
        &self.name
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
    reader: BufReader<TcpStream>,
    peers: Arc<Mutex<Vec<Peer>>>,
    id: Arc<Mutex<u32>>,
    alive: Arc<AtomicBool>,
) {
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

/// Names to fall back on when two people ask to be the same one. Gen 1's own
/// cast, so a second console reads as a second *player* rather than as "RED
/// (2)" — and every one of them survives [`filter_to_charmap`].
pub const NAME_PALETTE: &[&str] = &[
    "RED", "BLUE", "GREEN", "YELLOW", "GOLD", "SILVER", "CRYSTAL", "IVY",
];

/// A name nobody else in the room has.
///
/// **The relay decides this, because the relay is the only party that can see
/// everybody.** Two people running the shipped command both ask to be `RED` —
/// the default, since neither typed a name — and a room with two REDs in it is
/// a room you cannot read: the avatar labels are the only thing telling you
/// which walking figure is the other person.
pub fn unique_name(want: &str, taken: &[String]) -> String {
    let want = want.trim();
    let want = if want.is_empty() { "RED" } else { want };
    let free = |n: &str| !taken.iter().any(|t| t.eq_ignore_ascii_case(n));
    if free(want) {
        return want.to_string();
    }
    for n in NAME_PALETTE {
        if free(n) {
            return (*n).to_string();
        }
    }
    for i in 2..=99u32 {
        let c = format!("{want}{i}");
        if free(&c) {
            return c;
        }
    }
    want.to_string()
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
    host_on(TcpListener::bind(bind)?, opts)
}

/// Serve a room on a listener somebody else bound.
///
/// Separate from [`host`] because the well-known rendezvous ([`rendezvous`])
/// has to *bind first* and host second: the bind is what decides who hosts, and
/// it has to be the same syscall that answers the question, not a second one
/// taken after a lookup.
pub fn host_on(listener: TcpListener, opts: RelayOptions) -> std::io::Result<Hosted> {
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
        let Ok(mut reply) = stream.try_clone() else {
            continue;
        };
        clients.lock().unwrap().push((id, tx));
        let room = Arc::clone(&room);
        std::thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let f: Vec<&str> = line.split('\t').collect();
                let mut l = room.lock().unwrap();
                if f.first().copied() == Some("join") {
                    // **Decided under the room lock, answered after it.** Two
                    // people joining in the same tick must not both race
                    // against an empty `taken` and be handed the same name; but
                    // a socket write must not happen with the lock held either,
                    // or a client that has stopped reading stalls the relay's
                    // own tick thread.
                    let taken: Vec<String> = l
                        .bodies
                        .iter()
                        .filter(|(k, _)| **k != id)
                        .map(|(_, b)| b.name.clone())
                        .collect();
                    let given = unique_name(f.get(1).unwrap_or(&"?"), &taken);
                    l.bodies.entry(id).or_default().name = given.clone();
                    drop(l);
                    let _ = writeln!(reply, "name\t{given}");
                    continue;
                }
                let b = l.bodies.entry(id).or_default();
                match f.first().copied() {
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

// ---------------------------------------------------------------------------
// The rendezvous: one room per user, decided by a bind rather than by timing.
// ---------------------------------------------------------------------------

/// Base of the per-user rendezvous port. Below the ephemeral range
/// (32768-60999 on Linux), so an outgoing connection's local port can never
/// land on it and make the room disappear for one launch in a thousand.
pub const LOBBY_PORT_BASE: u16 = 24000;

/// The port this user's room lives on. `$GB_LOBBY_PORT` overrides.
///
/// Keyed by uid so two people sharing a machine get two rooms — the same reason
/// [`crate::link::peer::rendezvous_dir`] keys its fallback directory by uid.
pub fn rendezvous_port() -> u16 {
    if let Ok(p) = std::env::var("GB_LOBBY_PORT") {
        if let Ok(p) = p.trim().parse::<u16>() {
            return p;
        }
        eprintln!("terminalgb: GB_LOBBY_PORT={p:?} is not a port number — ignoring");
    }
    LOBBY_PORT_BASE + (uid() % 1000) as u16
}

fn uid() -> u32 {
    // No `libc` in the default build, and this is not worth a feature: the
    // runtime directory is already `/run/user/<uid>`, and `$UID` is exported by
    // every interactive shell. A wrong answer costs a shared room, not
    // correctness.
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        if let Some(n) = std::path::Path::new(&d)
            .file_name()
            .and_then(|s| s.to_str())
            .and_then(|s| s.parse::<u32>().ok())
        {
            return n;
        }
    }
    std::env::var("UID")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// The room's own cable rendezvous directory — where the consoles *in this
/// room* find each other, and nowhere a stray emulator is looking.
///
/// `$GB_LOBBY_DIR` overrides.
pub fn lobby_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("GB_LOBBY_DIR") {
        return PathBuf::from(d);
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return PathBuf::from(d).join("terminalgb/lobby");
    }
    std::env::temp_dir().join(format!("terminalgb-lobby-{}", uid()))
}

/// The result of [`rendezvous`]: the room's address, and the handle that stops
/// it if this process is the one hosting.
pub struct Rendezvous {
    pub addr: String,
    /// `Some` when we bound the port and are serving the room.
    pub stop: Option<Arc<AtomicBool>>,
    pub hosting: bool,
    /// Set when the well-known port was unusable and we had to fall back to an
    /// ephemeral one — which means a second instance will **not** find us, so
    /// it must be said out loud rather than discovered by a lonely player.
    pub private: bool,
}

/// Host the user's room, or join it if it already exists.
///
/// **The decision is the `bind`, not a lookup.** A rendezvous made of a file
/// holding a port number has a window between reading it and acting on it, and
/// two people starting the emulator at the same moment fall into that window
/// and end up in two rooms of one person each — which is exactly the bug this
/// replaces. `TcpListener::bind` on a fixed port is decided by the kernel:
/// precisely one process can hold it, everybody else gets `AddrInUse`, and
/// there is no stale state to sweep because a port is released when the process
/// that held it dies.
///
/// It is the same shape as the cable's own auto-discovery
/// ([`crate::link::peer::LinkPeer::discover`]) — first one there waits, the
/// rest attach — with the sweep-the-corpses step replaced by something the
/// kernel does for us.
pub fn rendezvous(opts: RelayOptions) -> std::io::Result<Rendezvous> {
    let port = rendezvous_port();
    match decide(port) {
        Decision::Host(l) => {
            let h = host_on(l, opts)?;
            Ok(Rendezvous {
                addr: h.addr,
                stop: Some(h.stop),
                hosting: true,
                private: false,
            })
        }
        Decision::Join(addr) => Ok(Rendezvous {
            addr,
            stop: None,
            hosting: false,
            private: false,
        }),
        // Something else owns the fixed port, or this machine will not let us
        // have it (a sandbox, a policy). A room only this process can see still
        // beats no room, but it is a different promise and says so out loud —
        // a second instance will not find us.
        Decision::Private(why) => {
            eprintln!(
                "terminalgb: lobby: the shared room port {port} is not usable \
                 ({why}) — hosting a room only this window can see. Set \
                 GB_LOBBY_PORT to a free port to share again."
            );
            let h = host("127.0.0.1:0", opts)?;
            Ok(Rendezvous {
                addr: h.addr,
                stop: Some(h.stop),
                hosting: true,
                private: true,
            })
        }
    }
}

enum Decision {
    Host(TcpListener),
    Join(String),
    Private(String),
}

/// Bind, or prove somebody else's room is really there.
///
/// The retry is not defensive padding: the holder of the port may have quit in
/// the microsecond between our failed bind and our connect, and a person whose
/// second window opened exactly then would otherwise get no room at all with no
/// way to tell why.
fn decide(port: u16) -> Decision {
    let addr = format!("127.0.0.1:{port}");
    let mut why = String::new();
    for _ in 0..3 {
        match TcpListener::bind(("127.0.0.1", port)) {
            Ok(l) => return Decision::Host(l),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => match probe(&addr) {
                Ok(()) => return Decision::Join(addr),
                Err(e) => why = e.to_string(),
            },
            Err(e) => return Decision::Private(e.to_string()),
        }
    }
    Decision::Private(why)
}

/// Is there a lobby on `addr`? Connects, reads the relay's own `you` greeting
/// and hangs up **without saying anything**, so the room never sees a body for
/// a connection that was only asking.
fn probe(addr: &str) -> std::io::Result<()> {
    let sock = TcpStream::connect(addr)?;
    sock.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).ok();
    let mut line = String::new();
    BufReader::new(sock).read_line(&mut line)?;
    if line.starts_with("you\t") {
        Ok(())
    } else {
        Err(std::io::Error::other("that port is not a TerminalGB lobby"))
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
        Ok(c) => {
            let given = c.name().to_string();
            let note = if given == name {
                format!("lobby: joined {addr} as {given}")
            } else {
                // Never silent: the player asked to be one thing and is another,
                // and the label over their head is how they find themselves in
                // a room full of identical sprites.
                format!("lobby: joined {addr} as {given} ({name} was taken)")
            };
            Some((c, note))
        }
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
    // **The log's own directory, explicitly.** It used to be created as a side
    // effect of the pair directory living under `target/worldlobby/` too; the
    // cable rendezvous moved out to the runtime directory and this quietly
    // became "no partner (No such file or directory)" — an error message that
    // names the log file nowhere and reads like a missing binary.
    std::fs::create_dir_all(SCRATCH)?;
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
/// The three things it *does* arrange are the three that are not the
/// frontend's by default: which room this is ([`rendezvous`] — one per user,
/// decided by a bind so two windows cannot each open their own), which cable
/// rendezvous the console uses (the room's own, so `link = auto` finds the
/// person opposite rather than whatever else is running), and that the built-in
/// *virtual* trade partner does not claim the port the real second console
/// needs.
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
    // Two seats when a partner is coming, one when it is not: the relay's clock
    // starts when everybody *expected* is `ready`, and that signal is what stops
    // one console racing ahead of the other (`docs/worldlobby.md` §6.4). A room
    // people walk into on their own schedule must start for the first of them;
    // waiting for a second person who may never come would leave the first in
    // `Waiting` for ever.
    let expect = if o.partner.is_some() { 2 } else { 1 };
    let opts = RelayOptions {
        expect,
        ..RelayOptions::default()
    };
    let (addr, stop, hosting) = match &o.join {
        Some(a) => (a.clone(), None, false),
        None => {
            let r = rendezvous(opts)?;
            if r.hosting {
                eprintln!("lobby: hosting the room on {}", r.addr);
            } else {
                eprintln!("lobby: another window already has a room open — joining it");
            }
            (r.addr, r.stop, r.hosting)
        }
    };

    // --- the cable --------------------------------------------------------
    // **The room decides who is on the cable, so put the rendezvous inside the
    // room.** `link = auto`'s ordinary directory is shared with every other
    // emulator this user is running, and it pairs in arrival order — so an
    // unrelated window could take the seat the person opposite you was walking
    // towards. A directory belonging to *this room* narrows the candidates to
    // the consoles that are in it.
    //
    // The driven partner gets a directory of its own, keyed by **our** pid: it
    // is a private two-console session and must not be reachable by anybody
    // else's run. That is the whole of the defect the captain hit — the old
    // path used one fixed `cable-human` for every launch, so a second `--lobby`
    // put four consoles in one directory and the wrong two paired.
    //
    // **A partner is only ever booted into an empty room.** The cable has two
    // ends: if somebody else is already here, the seat opposite is theirs, and
    // a driven console taking it is precisely the crowd that made two `--lobby`
    // runs race. Joining a room is proof somebody else is here, so the partner
    // is dropped without ceremony.
    let want_partner = o.partner.is_some() && hosting;
    let mut child = None;
    let pair = if want_partner {
        lobby_dir().join(format!("cable-{}", std::process::id()))
    } else {
        lobby_dir().join("cable")
    };
    let _ = std::fs::create_dir_all(&pair);
    std::env::set_var("GB_LINK_DIR", &pair);

    // --- the partner ------------------------------------------------------
    let port: u16 = addr
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(0);
    if let Some(p) = o.partner.as_ref().filter(|_| want_partner) {
        match spawn_partner(port, &pair, p) {
            Ok((kid, log)) => {
                eprintln!("lobby: {} is walking in; log: {}", p.name, log.display());
                child = Some(kid);
            }
            // Fail-soft, and the same rule as an unreachable relay: a missing
            // partner costs the partner and nothing else. The room still
            // exists, and a second person running the same command fills it.
            Err(e) => eprintln!(
                "terminalgb: lobby: no partner ({e}). You can still be joined by \
                 somebody running `terminalgb --lobby`."
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

    // **Stderr, not stdout.** `ratatui` owns stdout the moment `run_terminal`
    // starts, and it is about to. A line printed to stdout here is cleared by
    // the alternate screen a few milliseconds later, so the one place these
    // instructions were guaranteed *not* to be readable was the terminal they
    // were printed to.
    eprintln!();
    eprintln!("  Walk to the Cable Club receptionist, say yes, and choose TRADE");
    eprintln!("  CENTER. Once both consoles are in, the room becomes 40x28 with our");
    eprintln!("  own floor and you will see each other walk about.");
    if child.is_some() {
        eprintln!("  A driven console is walking in to meet you — the seat opposite");
        eprintln!("  is taken, so a second person cannot join this room.");
    } else {
        // **A cable has two ends and the receptionist checks the wire**, so a
        // room with nobody else in it is a room you cannot get into. Say which
        // of the two ways to fill the other seat you want, rather than leaving
        // somebody standing at a desk wondering.
        eprintln!("  Run `terminalgb --lobby` again in another window to be the");
        eprintln!("  second console — it finds this room by itself. On your own,");
        eprintln!("  `terminalgb --lobby-partner` boots a driven console instead.");
    }
    eprintln!();

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

#[cfg(test)]
mod rendezvous_tests {
    use super::*;

    /// Two people running the shipped command get **one** room, and the second
    /// one joins rather than opening its own. This is the whole defect: the old
    /// path bound `127.0.0.1:0` twice and the two windows never saw each other.
    #[test]
    fn the_second_instance_joins_the_first_instead_of_hosting() {
        // A port of our own, so a developer running the emulator beside the
        // tests does not decide the outcome.
        let port = 24700 + (std::process::id() % 200) as u16;
        std::env::set_var("GB_LOBBY_PORT", port.to_string());
        let first = rendezvous(RelayOptions::default()).expect("first");
        assert!(first.hosting, "the first instance hosts");
        assert!(!first.private, "and on the shared port: {}", first.addr);

        let second = rendezvous(RelayOptions::default()).expect("second");
        assert!(!second.hosting, "the second instance must NOT host its own");
        assert_eq!(second.addr, first.addr, "and must land in the same room");

        // ...and a real client can reach it and is told who it is.
        let a = LobbyClient::join(&second.addr, "RED").expect("join");
        let b = LobbyClient::join(&second.addr, "RED").expect("join");
        assert_eq!(a.name(), "RED");
        assert_ne!(
            b.name(),
            "RED",
            "two REDs in one room is a room you cannot read"
        );
        assert_eq!(b.name(), "BLUE", "and the second one is Gen 1's second one");

        first.stop.unwrap().store(true, Ordering::Relaxed);
        std::env::remove_var("GB_LOBBY_PORT");
    }

    /// A port that belongs to something else must not be mistaken for a room:
    /// the emulator has to notice and say so, not hang waiting for a greeting
    /// that is never coming.
    #[test]
    fn a_foreign_program_on_the_port_is_not_a_room() {
        let l = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = format!("127.0.0.1:{}", l.local_addr().unwrap().port());
        // Accept and say nothing at all, as an unrelated service would.
        std::thread::spawn(move || {
            let _keep: Vec<_> = l.incoming().flatten().collect();
        });
        assert!(probe(&addr).is_err());
        assert!(LobbyClient::join(&addr, "RED").is_err());
    }

    /// The rule the relay applies, on its own, over every shape of collision.
    #[test]
    fn nobody_is_given_a_name_somebody_else_already_has() {
        assert_eq!(unique_name("RED", &[]), "RED");
        assert_eq!(unique_name("RED", &["RED".into()]), "BLUE");
        assert_eq!(unique_name("RED", &["RED".into(), "BLUE".into()]), "GREEN");
        // A name of one's own is kept even when the palette is untouched.
        assert_eq!(unique_name("ASH", &["RED".into()]), "ASH");
        // Case is not a difference anybody can see on a Game Boy screen.
        assert_eq!(unique_name("red", &["RED".into()]), "BLUE");
        // Past the palette, still unique.
        let full: Vec<String> = NAME_PALETTE.iter().map(|s| s.to_string()).collect();
        let n = unique_name("RED", &full);
        assert!(!full.contains(&n), "{n}");
    }
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

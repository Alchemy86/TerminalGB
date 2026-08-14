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

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

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

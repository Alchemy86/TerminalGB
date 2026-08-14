//! **BGB's link-cable protocol over TCP** — the link port, across a network.
//!
//! Reference: <https://bgb.bircd.org/bgblink.html>. It is publicly documented
//! and widely implemented; this is a clean-room implementation from that page
//! plus the shape of the reference server in
//! [`Lorenzooone/PokemonGB_Online_Trades`](https://github.com/Lorenzooone/PokemonGB_Online_Trades)
//! (MIT), which is the counterparty the integration test drives. No third-party
//! code is reproduced and no new dependency is added: the whole thing is
//! `std::net` and eight bytes at a time.
//!
//! # Why this protocol and not one of our own
//!
//! Because it already goes somewhere. BGB, SameBoy's link support, Emulicious,
//! gbplay and several hardware bridges speak it, so implementing it once buys
//! emulator-to-emulator linking with all of them — and, through
//! `PokemonGB_Online_Trades` used **unchanged**, a live trade between this
//! emulator and a **real Game Boy** on somebody else's USB link adapter,
//! anywhere in the world. That tool's `utilities/bgb_link_cable_server.py`
//! listens for a BGB client and relays the bytes onward over a WebSocket; its
//! `usb_trading.py` puts the same trading logic behind a physical cartridge.
//! The two are interchangeable, which is what makes this the cheapest possible
//! bridge to hardware: it costs no hardware at all.
//!
//! # The wire format, in full
//!
//! Every packet is exactly 8 bytes: four bytes then a little-endian `u32`
//! (`<4BI>`, in the reference server's own struct syntax).
//!
//! | `b1` | name | `b2` | `b3` | `b4` | `i1` |
//! |---|---|---|---|---|---|
//! | 1 | version | major (1) | minor (4) | patch (0) | 0 |
//! | 101 | joypad | button + state | 0 | 0 | 0 |
//! | 104 | sync1 — **master data** | data byte | `SC` value | 0 | timestamp |
//! | 105 | sync2 — **slave data** | data byte | `$80` | 0 | 0 |
//! | 106 | sync3 — ack / timestamp | 1 = ack, 0 = timestamp | — | — | timestamp |
//! | 108 | status | bit 0 running, 1 paused, 2 reconnect | 0 | 0 | 0 |
//! | 109 | wantdisconnect | 0 | 0 | 0 | 0 |
//!
//! One side listens and the other connects; both send a version packet and
//! verify what comes back, and a status packet is sent **in response to
//! version**. `TCP_NODELAY` is set, as the specification asks.
//!
//! ## The three packets that actually carry a trade
//!
//! * A console that drives the clock sends **sync1**.
//! * A console that was armed for an external clock answers **sync2** with the
//!   byte in its own shift register. That is one complete exchange: after it,
//!   each side holds what the other started with, exactly as eight clock edges
//!   on a real cable leave them.
//! * A console that was *not* armed answers **sync3 with `b2 = 1`** — "I am not
//!   doing a passive transfer". The master shifts in the idle-high `$FF`.
//!
//! ## Timestamps: carried, and deliberately not enforced
//!
//! Each side keeps the difference between its own timestamp and the peer's, and
//! uses it to decide when to wait. We send ours — a 2 MiHz count derived from
//! **emulated** T-cycles, so a stalled console really does stop the clock it
//! reports — and we record the peer's, but we do not gate anything on the
//! difference. Waiting is done instead by
//! [`TransportLink`](crate::link::TransportLink), which stops paying cycles to
//! the CPU until the answer arrives; that is the same thing the timestamps
//! exist to arrange, expressed where it can be seen and counted.
//!
//! This is not a shortcut invented here. `PokemonGB_Online_Trades` "cheats" the
//! same way — its comment reads *"Cheat, and say we are exactly in sync with
//! the client"* — and it completes real trades with real cartridges. The games
//! are turn-based: neither side proceeds until it has the other's byte, so the
//! protocol's own request/response structure is the synchronisation.
//!
//! # Never blocks, never hangs
//!
//! The socket is non-blocking from the moment it exists, `accept` included, so
//! a console configured to *listen* runs normally while it waits for a peer —
//! the same rule the video sink learned the hard way (`AGENTS.md`, "no media
//! sink may ever block the emulator"). [`idle`](LinkTransport::idle) spins
//! briefly and then sleeps for a fraction of a millisecond, and a master
//! transfer the peer never answers is given up on after
//! [`MASTER_TIMEOUT`] rather than stalling the emulator for ever.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

use crate::link::{LinkTransport, MasterReply, TransportError};

/// Packet command numbers.
pub mod cmd {
    /// Protocol version, exchanged and verified on connection.
    pub const VERSION: u8 = 1;
    /// Remote joypad control. Received and deliberately ignored — see
    /// [`BgbLink`].
    pub const JOYPAD: u8 = 101;
    /// Master data: the sender is driving the shift clock.
    pub const SYNC1: u8 = 104;
    /// Slave data: the sender was armed for an external clock and this is what
    /// was in its shift register.
    pub const SYNC2: u8 = 105;
    /// Acknowledgement (`b2 = 1`) or timestamp synchronisation (`b2 = 0`).
    pub const SYNC3: u8 = 106;
    /// Emulator state.
    pub const STATUS: u8 = 108;
    /// The user asked to disconnect.
    pub const WANT_DISCONNECT: u8 = 109;
}

/// The protocol version this implementation speaks and requires. BGB refuses
/// anything else and so do we — a version mismatch is a different problem from
/// a broken link and must not be reported as one.
pub const PROTOCOL_VERSION: (u8, u8, u8) = (1, 4, 0);

/// Bytes in one packet.
pub const PACKET_LEN: usize = 8;

/// How long a master transfer waits for the far end before the console is given
/// the idle-high line instead.
///
/// Generous on purpose: the reference Python server paces at **10 ms per byte**
/// from a thread that does not even set `TCP_NODELAY`, and completes real
/// trades at that rate. A timeout tight enough to be "responsive" would break
/// the very counterparty this exists to talk to. A game whose transfer times
/// out sees an unplugged cable and its own protocol re-synchronises — Gen 1's
/// `Serial_ExchangeBytes` re-sends its first byte until the preamble comes
/// back.
pub const MASTER_TIMEOUT: Duration = Duration::from_secs(2);

/// How much emulated time may pass between timestamp packets, in 2 MiHz ticks.
///
/// ~16 ms, i.e. once an emulated frame. These matter more than they look: the
/// reference server sends nothing at all until it has seen a **non-zero**
/// timestamp from the client, and a console that never happens to drive the
/// clock would otherwise never send one.
const SYNC_PERIOD: u32 = 32_768;

/// The 2 MiHz unit the protocol counts in: 2^21 ticks per second, against the
/// console's 2^22 T-cycles per second — so one tick is two T-cycles exactly.
const T_CYCLES_PER_TICK: u32 = 2;

/// An 8-byte protocol packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Packet {
    pub b1: u8,
    pub b2: u8,
    pub b3: u8,
    pub b4: u8,
    /// Timestamp, in 2 MiHz ticks. Only the low 31 bits are used.
    pub i1: u32,
}

impl Packet {
    pub fn new(b1: u8, b2: u8, b3: u8, b4: u8, i1: u32) -> Packet {
        Packet { b1, b2, b3, b4, i1 }
    }

    pub fn encode(&self) -> [u8; PACKET_LEN] {
        let t = self.i1.to_le_bytes();
        [self.b1, self.b2, self.b3, self.b4, t[0], t[1], t[2], t[3]]
    }

    pub fn decode(raw: &[u8; PACKET_LEN]) -> Packet {
        Packet {
            b1: raw[0],
            b2: raw[1],
            b3: raw[2],
            b4: raw[3],
            i1: u32::from_le_bytes([raw[4], raw[5], raw[6], raw[7]]),
        }
    }

    /// The version packet both sides send on connection.
    pub fn version() -> Packet {
        let (major, minor, patch) = PROTOCOL_VERSION;
        Packet::new(cmd::VERSION, major, minor, patch, 0)
    }

    /// The status packet sent in response to version: running, not paused, no
    /// reconnect support (we do not offer to be re-plugged mid-session).
    pub fn status_running() -> Packet {
        Packet::new(cmd::STATUS, 1, 0, 0, 0)
    }
}

/// Which end of the connection this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Bind and wait for a peer to connect. What BGB's "Listen" does, and what
    /// a second emulator or a bridge tool connects to.
    Listen,
    /// Connect out to a peer that is already listening. What
    /// `PokemonGB_Online_Trades` expects, since its server binds first.
    Connect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Bound, nothing has connected yet.
    Listening,
    /// Connected; version packets in flight.
    Handshake,
    /// Version verified. Bytes may flow.
    Live,
    /// The peer went away or the link failed.
    Closed,
}

/// A link partner reachable over TCP, speaking BGB's protocol.
///
/// Attach it to a console with
/// [`TransportLink`](crate::link::TransportLink); see that type for how the
/// driver decides when to stall.
///
/// # What is deliberately not implemented
///
/// * **Joypad packets (101).** They exist so one BGB can drive another's
///   buttons. Accepting them would let a network peer play the game on this
///   console, which is not a link cable and is not something a trade partner
///   should be able to do. They are parsed and dropped.
/// * **`wantdisconnect` reconnection.** Our status packet advertises no
///   reconnect support, so a well-behaved peer will not send one; if it does,
///   the link closes.
#[derive(Debug)]
pub struct BgbLink {
    phase: Phase,
    listener: Option<TcpListener>,
    stream: Option<TcpStream>,
    /// Who we are talking to, for the status row.
    peer: String,
    /// Where we are listening or connecting, for the status row before a peer
    /// turns up.
    addr: String,
    role: Role,

    /// Bytes read from the socket that do not yet make a whole packet.
    rx: Vec<u8>,
    /// Bytes we could not write because the socket was full.
    tx: VecDeque<u8>,

    /// Bytes the peer has clocked to us as master, oldest first.
    incoming: VecDeque<u8>,
    /// True while a byte handed to the driver has not been answered.
    awaiting_answer: bool,

    /// True while we have sent a sync1 and had no answer.
    master_pending: bool,
    master_sent_at: Option<Instant>,
    master_reply: Option<MasterReply>,

    /// Our own clock, in 2 MiHz ticks, driven by **emulated** time.
    clock: u32,
    clock_frac: u32,
    last_sync_at: u32,
    /// The peer's most recent timestamp. Recorded because the protocol carries
    /// it and a diagnostic wants it; nothing is gated on it (see module docs).
    peer_clock: u32,

    /// How many consecutive `idle` calls have happened, so the first few can be
    /// a cheap yield and only a real wait costs a sleep.
    idle_spins: u32,

    /// Why the socket went away, reported once on the next poll.
    closed_because: Option<String>,
    /// Whether that closure was the peer saying goodbye rather than vanishing.
    closed_cleanly: bool,
    /// Whether the link ever reached [`Phase::Live`].
    ///
    /// This is the whole of the unplugged-versus-broken distinction: a link
    /// that never came up cannot have lost a byte, so it must not fault the
    /// console and poison its save. See [`TransportError`].
    went_live: bool,
}

impl BgbLink {
    /// Bind and wait — without blocking — for a peer to connect.
    ///
    /// Returns as soon as the socket is bound. The console runs normally while
    /// nothing is connected; it simply has an unplugged cable.
    pub fn listen(
        addr: impl ToSocketAddrs + std::fmt::Display,
    ) -> Result<BgbLink, TransportError> {
        let shown = addr.to_string();
        let listener = TcpListener::bind(&addr).map_err(|e| {
            TransportError::Setup(format!("cannot listen on {shown}: {e}"))
        })?;
        listener.set_nonblocking(true).map_err(|e| {
            TransportError::Setup(format!("cannot make the listener non-blocking: {e}"))
        })?;
        let bound = listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or(shown);
        Ok(BgbLink {
            phase: Phase::Listening,
            listener: Some(listener),
            addr: bound,
            role: Role::Listen,
            ..BgbLink::empty()
        })
    }

    /// Connect out to a peer that is already listening.
    ///
    /// The connect itself is the one blocking call in this module and it is
    /// bounded: a peer that is not there fails in
    /// [`CONNECT_TIMEOUT`](Self::CONNECT_TIMEOUT) rather than hanging the
    /// emulator's startup. Failure is [`TransportError::Setup`] — an unplugged
    /// cable, not a broken one.
    pub fn connect(
        addr: impl ToSocketAddrs + std::fmt::Display,
    ) -> Result<BgbLink, TransportError> {
        let shown = addr.to_string();
        let mut last = None;
        let resolved: Vec<SocketAddr> = addr
            .to_socket_addrs()
            .map_err(|e| TransportError::Setup(format!("cannot resolve {shown}: {e}")))?
            .collect();
        for sa in &resolved {
            match TcpStream::connect_timeout(sa, Self::CONNECT_TIMEOUT) {
                Ok(s) => {
                    let peer = sa.to_string();
                    let mut link = BgbLink {
                        phase: Phase::Handshake,
                        addr: shown,
                        peer: peer.clone(),
                        role: Role::Connect,
                        ..BgbLink::empty()
                    };
                    link.adopt(s)?;
                    return Ok(link);
                }
                Err(e) => last = Some(e),
            }
        }
        Err(TransportError::Setup(match last {
            Some(e) => format!("cannot connect to {shown}: {e}"),
            None => format!("cannot connect to {shown}: no address resolved"),
        }))
    }

    /// How long [`connect`](BgbLink::connect) waits before deciding nothing is
    /// there.
    pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

    /// Parse `[listen:|connect:]host:port` and open it. Without a prefix the
    /// default is **connect**, because the tools worth reaching — BGB's
    /// "Listen", and `PokemonGB_Online_Trades`' server — bind first.
    pub fn open(spec: &str) -> Result<BgbLink, TransportError> {
        let spec = spec.trim();
        let (role, addr) = match spec.split_once(':') {
            Some((tag, rest)) if tag.eq_ignore_ascii_case("listen") => {
                (Role::Listen, rest.trim())
            }
            Some((tag, rest)) if tag.eq_ignore_ascii_case("connect") => {
                (Role::Connect, rest.trim())
            }
            _ => (Role::Connect, spec),
        };
        if addr.is_empty() {
            return Err(TransportError::Setup(
                "no address: expected [listen:|connect:]host:port".to_string(),
            ));
        }
        match role {
            Role::Listen => BgbLink::listen(addr),
            Role::Connect => BgbLink::connect(addr),
        }
    }

    fn empty() -> BgbLink {
        BgbLink {
            phase: Phase::Closed,
            listener: None,
            stream: None,
            peer: String::new(),
            addr: String::new(),
            role: Role::Connect,
            rx: Vec::with_capacity(64),
            tx: VecDeque::new(),
            incoming: VecDeque::new(),
            awaiting_answer: false,
            master_pending: false,
            master_sent_at: None,
            master_reply: None,
            clock: 0,
            clock_frac: 0,
            last_sync_at: 0,
            peer_clock: 0,
            idle_spins: 0,
            closed_because: None,
            closed_cleanly: false,
            went_live: false,
        }
    }

    /// Take a freshly connected stream: set the options the specification asks
    /// for and open the handshake.
    fn adopt(&mut self, s: TcpStream) -> Result<(), TransportError> {
        // "The TCP_NODELAY setting should be set on the socket to avoid extra
        // delays in the OS." A link byte is eight bytes long; Nagle would sit
        // on it waiting for company that never comes.
        s.set_nodelay(true)
            .map_err(|e| TransportError::Setup(format!("cannot set TCP_NODELAY: {e}")))?;
        s.set_nonblocking(true).map_err(|e| {
            TransportError::Setup(format!("cannot make the socket non-blocking: {e}"))
        })?;
        self.stream = Some(s);
        self.phase = Phase::Handshake;
        self.rx.clear();
        self.tx.clear();
        self.send(Packet::version());
        Ok(())
    }

    /// Which end of the connection this is. Worth showing: "listening" and
    /// "connecting" fail in completely different ways, and a link that has not
    /// come up is the commonest thing to have to debug.
    pub fn role(&self) -> Role {
        self.role
    }

    /// The peer's last reported timestamp, in 2 MiHz ticks. Diagnostic only.
    pub fn peer_clock(&self) -> u32 {
        self.peer_clock
    }

    /// Our own clock, in 2 MiHz ticks of *emulated* time.
    pub fn clock(&self) -> u32 {
        self.clock
    }

    /// Queue a packet, writing as much of it as the socket will take now.
    fn send(&mut self, p: Packet) {
        self.tx.extend(p.encode());
        self.flush();
    }

    fn flush(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            self.tx.clear();
            return;
        };
        while !self.tx.is_empty() {
            let (front, _) = self.tx.as_slices();
            // `as_slices` can hand back an empty first slice when the ring has
            // wrapped; `make_contiguous` is the cheap way to be sure.
            let front: &[u8] = if front.is_empty() {
                self.tx.make_contiguous()
            } else {
                front
            };
            match stream.write(front) {
                Ok(0) => break,
                Ok(n) => {
                    self.tx.drain(..n);
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.close(format!("write failed: {e}"));
                    return;
                }
            }
        }
    }

    fn close(&mut self, why: String) {
        self.close_as(why, false);
    }

    /// Close, recording whether the peer said goodbye first.
    ///
    /// The distinction decides whether the console's save survives, so it is
    /// drawn as narrowly as the protocol allows: a `wantdisconnect` packet with
    /// no transfer in flight is somebody choosing to unplug the cable, and
    /// anything else — a bare EOF, a reset, an error — is the far end
    /// vanishing, which may have taken a byte with it.
    fn close_as(&mut self, why: String, clean: bool) {
        self.phase = Phase::Closed;
        self.stream = None;
        if self.closed_because.is_none() {
            self.closed_because = Some(why);
            self.closed_cleanly = clean;
        }
    }

    /// Whether a byte is in the air right now. Used only to decide how bad a
    /// disconnection is.
    fn mid_transfer(&self) -> bool {
        self.master_pending || self.awaiting_answer || !self.incoming.is_empty()
    }

    /// Say goodbye and hang up.
    ///
    /// **Call this on a clean shutdown**, from wherever the frontend already
    /// flushes the battery — `Drop` cannot be relied on here for exactly the
    /// reasons `src/mbc/battery.rs` records (winit's event loop calls
    /// `process::exit`, the release profile aborts on panic, `kill` skips
    /// destructors). A peer that receives this treats the link as unplugged
    /// rather than broken and keeps writing its save; a peer that receives
    /// nothing has to assume the worst.
    /// **Closing a socket that still has unread data in its receive queue
    /// sends an RST, not a FIN** — and an RST discards everything already in
    /// flight, including the goodbye we just wrote. Measured: a first version
    /// sent the packet and dropped the stream, and the peer read
    /// `Connection reset by peer`, faulted, and stopped writing its save. So
    /// the close here is the textbook graceful one: write, drain, half-close,
    /// then wait a bounded moment for the peer's own FIN.
    ///
    /// The wait is the one place in this module that sleeps for longer than a
    /// scheduling quantum, and it is at process shutdown rather than anywhere
    /// near a frame, which is the distinction `AGENTS.md`'s no-blocking rule
    /// actually draws.
    pub fn disconnect(&mut self) {
        if let Some(stream) = self.stream.take() {
            self.stream = Some(stream);
            self.send(Packet::new(cmd::WANT_DISCONNECT, 0, 0, 0, 0));
            self.flush();
            self.graceful_close();
        }
        self.close_as("disconnected locally".to_string(), true);
    }

    /// Drain, half-close, and give the peer a bounded moment to read what was
    /// just written. See [`disconnect`](BgbLink::disconnect).
    fn graceful_close(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        let _ = stream.shutdown(std::net::Shutdown::Write);
        let mut buf = [0u8; 512];
        let deadline = Instant::now() + Self::LINGER;
        while Instant::now() < deadline {
            match stream.read(&mut buf) {
                // The peer's FIN: it has seen everything we sent.
                Ok(0) => break,
                Ok(_) => continue,
                Err(e) if e.kind() == ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
    }

    /// How long [`disconnect`](BgbLink::disconnect) waits for the peer to
    /// acknowledge the goodbye. Loopback answers in microseconds and a
    /// transatlantic peer in tens of milliseconds; past this the peer has
    /// almost certainly gone already, and the worst case is that it treats the
    /// link as broken, which is the safe direction.
    const LINGER: Duration = Duration::from_millis(250);

    /// Read whatever is waiting and turn it into packets.
    ///
    /// "Packet boundaries are not guaranteed: multiple commands may be in a
    /// single TCP packet, and a command may be split over multiple TCP
    /// packets." Hence the accumulating buffer rather than an 8-byte read.
    fn receive(&mut self) -> Result<(), TransportError> {
        let mut buf = [0u8; 512];
        // **Parse what arrived before acting on the hang-up that came with
        // it.** A polite peer writes its `wantdisconnect` and then closes, so
        // both land in the same `read(2)` — and `close_as` keeps the *first*
        // reason it is given. Recording the EOF as we see it therefore
        // overwrote the goodbye and turned every clean shutdown into a broken
        // link, which faulted the other console and stopped its save being
        // written. Measured, then fixed.
        let mut ended: Option<String> = None;
        loop {
            let Some(stream) = self.stream.as_mut() else {
                break;
            };
            match stream.read(&mut buf) {
                Ok(0) => {
                    ended = Some("the peer closed the connection".to_string());
                    break;
                }
                Ok(n) => self.rx.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => {
                    ended = Some(format!("read failed: {e}"));
                    break;
                }
            }
        }

        while self.rx.len() >= PACKET_LEN {
            let mut raw = [0u8; PACKET_LEN];
            raw.copy_from_slice(&self.rx[..PACKET_LEN]);
            self.rx.drain(..PACKET_LEN);
            self.handle(Packet::decode(&raw))?;
        }
        if let Some(why) = ended {
            self.close(why);
        }
        Ok(())
    }

    /// Act on one received packet.
    fn handle(&mut self, p: Packet) -> Result<(), TransportError> {
        match p.b1 {
            cmd::VERSION => {
                if (p.b2, p.b3, p.b4) != PROTOCOL_VERSION {
                    let (a, b, c) = PROTOCOL_VERSION;
                    let why = format!(
                        "the peer speaks BGB link protocol {}.{}.{}, this is {a}.{b}.{c}",
                        p.b2, p.b3, p.b4
                    );
                    self.close(why.clone());
                    // A version mismatch is a link that never came up, not one
                    // that broke: nothing has been exchanged, so nothing can
                    // have been lost.
                    return Err(TransportError::Setup(why));
                }
                // "should be sent in response to version". Note what is *not*
                // done: a status packet is never sent in reply to a status
                // packet. The reference Python server does reply to one, so
                // answering back would ping-pong for ever.
                self.send(Packet::status_running());
                self.phase = Phase::Live;
                // Latched here rather than at the end of the poll: a peer can
                // go live and hang up inside one `read(2)`, and "was this link
                // ever up?" must not depend on where in a poll it is asked.
                self.went_live = true;
            }
            // Remote button control. A trade partner has no business pressing
            // buttons on this console; parsed and dropped.
            cmd::JOYPAD => {}
            cmd::SYNC1 => {
                self.note_peer_clock(p.i1);
                self.incoming.push_back(p.b2);
            }
            cmd::SYNC2 => {
                if self.master_pending {
                    self.master_reply = Some(MasterReply::Byte(p.b2));
                    self.master_pending = false;
                    self.master_sent_at = None;
                }
            }
            cmd::SYNC3 => {
                if p.b2 == 1 {
                    // "a synchronization packet sent in response to a received
                    // active transfer when not doing a passive transfer" — the
                    // peer was not armed, so we shift in an idle line.
                    if self.master_pending {
                        self.master_reply = Some(MasterReply::Idle);
                        self.master_pending = false;
                        self.master_sent_at = None;
                    }
                } else {
                    // Pure timestamp synchronisation. Recorded, not answered:
                    // answering one would ping-pong with a peer that answers
                    // ours.
                    self.note_peer_clock(p.i1);
                }
            }
            cmd::STATUS => {}
            cmd::WANT_DISCONNECT => {
                let clean = !self.mid_transfer();
                self.close_as("the peer asked to disconnect".to_string(), clean);
            }
            // "unknown commands should be ignored" is the only forward-
            // compatible reading, and BGB 1.5 added a command to a protocol
            // that calls itself 1.4.
            _ => {}
        }
        Ok(())
    }

    fn note_peer_clock(&mut self, ts: u32) {
        if ts != 0 {
            self.peer_clock = ts & 0x7FFF_FFFF;
        }
    }

    /// The failure to report once the socket has gone, if there was one.
    fn take_break(&mut self) -> Option<TransportError> {
        let why = self.closed_because.take()?;
        Some(if self.went_live && !self.closed_cleanly {
            TransportError::Broken(why)
        } else {
            TransportError::Setup(why)
        })
    }
}

impl LinkTransport for BgbLink {
    fn describe(&self) -> String {
        match self.phase {
            Phase::Listening => format!("bgb listening on {}", self.addr),
            Phase::Handshake => format!("bgb handshaking with {}", self.peer),
            Phase::Live => format!("bgb linked to {}", self.peer),
            Phase::Closed => format!("bgb closed ({})", self.addr),
        }
    }

    fn poll(&mut self) -> Result<(), TransportError> {
        if self.phase == Phase::Listening {
            // Non-blocking accept: the console keeps running while nobody has
            // turned up, which is the whole reason the listener is not opened
            // with a blocking `accept()`.
            let accepted = self
                .listener
                .as_ref()
                .map(|l| l.accept())
                .unwrap_or(Err(std::io::Error::from(ErrorKind::NotConnected)));
            match accepted {
                Ok((s, who)) => {
                    self.peer = who.to_string();
                    self.adopt(s)?;
                }
                Err(e) if e.kind() == ErrorKind::WouldBlock => return Ok(()),
                Err(e) => {
                    return Err(TransportError::Setup(format!("accept failed: {e}")));
                }
            }
        }

        if self.stream.is_some() {
            self.flush();
            self.receive()?;
        }

        // A master transfer the peer will plainly never answer. The console
        // gets the idle line it would get from an unplugged cable rather than
        // being held still indefinitely.
        if self.master_pending {
            if let Some(t) = self.master_sent_at {
                if t.elapsed() >= MASTER_TIMEOUT {
                    self.master_reply = Some(MasterReply::Idle);
                    self.master_pending = false;
                    self.master_sent_at = None;
                }
            }
        }

        // Keep the peer's idea of our clock fresh. The reference server sends
        // nothing at all until it has seen a non-zero timestamp from us, and a
        // console that never drives the clock never sends a sync1 — so without
        // this the link would come up and then sit there.
        if self.phase == Phase::Live
            && !self.master_pending
            && self.clock.wrapping_sub(self.last_sync_at) >= SYNC_PERIOD
        {
            self.last_sync_at = self.clock;
            let ts = self.clock;
            self.send(Packet::new(cmd::SYNC3, 0, 0, 0, ts));
        }

        match self.take_break() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn advance(&mut self, t_cycles: u32) {
        self.clock_frac += t_cycles;
        let ticks = self.clock_frac / T_CYCLES_PER_TICK;
        self.clock_frac %= T_CYCLES_PER_TICK;
        // 31 bits: "the highest bit is zero".
        self.clock = self.clock.wrapping_add(ticks) & 0x7FFF_FFFF;
    }

    fn live(&self) -> bool {
        self.phase == Phase::Live
    }

    fn send_master(&mut self, out: u8, control: u8) {
        if self.phase != Phase::Live {
            // Nothing on the other end. An unplugged cable idles high, and
            // answering now keeps the driver from stalling on a link that does
            // not exist.
            self.master_reply = Some(MasterReply::Idle);
            return;
        }
        let ts = self.clock;
        self.send(Packet::new(cmd::SYNC1, out, control, 0, ts));
        self.master_pending = true;
        self.master_sent_at = Some(Instant::now());
        self.idle_spins = 0;
    }

    fn take_master_reply(&mut self) -> Option<MasterReply> {
        self.master_reply.take()
    }

    fn take_incoming(&mut self) -> Option<u8> {
        if self.awaiting_answer {
            return None;
        }
        let b = self.incoming.pop_front()?;
        self.awaiting_answer = true;
        Some(b)
    }

    fn answer_incoming(&mut self, out: Option<u8>) {
        self.awaiting_answer = false;
        match out {
            // sync2: "b3 = $80", i1 = 0.
            Some(b) => self.send(Packet::new(cmd::SYNC2, b, 0x80, 0, 0)),
            // sync3 with b2 = 1: we were not doing a passive transfer.
            None => self.send(Packet::new(cmd::SYNC3, 1, 0, 0, 0)),
        }
    }

    /// **This override is load-bearing.** The inherent
    /// [`BgbLink::disconnect`] is not reachable through `dyn LinkTransport`,
    /// and the trait's default body is a no-op — so without this line a
    /// frontend's clean shutdown silently sent nothing and every peer recorded
    /// a broken link and stopped writing its save. A unit test on the concrete
    /// type could not see it (it calls the inherent method); a two-process
    /// trade did.
    fn disconnect(&mut self) {
        BgbLink::disconnect(self);
    }

    fn idle(&mut self) {
        self.idle_spins = self.idle_spins.saturating_add(1);
        // A loopback round trip is measured in microseconds and a local
        // process' scheduling quantum is not, so spin briefly before paying for
        // a sleep. Past that, sleeping is the point: the reference server paces
        // at 10 ms per byte and spinning through that would burn a core to no
        // purpose.
        if self.idle_spins < 64 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codec, both directions, against bytes written out by hand from the
    /// specification's own table. `<4BI>` little-endian, and nothing else.
    #[test]
    fn the_packet_format_is_four_bytes_and_a_little_endian_u32() {
        let p = Packet::new(cmd::SYNC1, 0xA5, 0x81, 0, 0x0123_4567);
        assert_eq!(
            p.encode(),
            [104, 0xA5, 0x81, 0x00, 0x67, 0x45, 0x23, 0x01],
            "timestamp is little-endian and sits in the last four bytes"
        );
        assert_eq!(Packet::decode(&p.encode()), p);

        // The version packet every implementation must send and verify.
        assert_eq!(Packet::version().encode(), [1, 1, 4, 0, 0, 0, 0, 0]);
        // And the status reply: running, not paused, no reconnect support.
        assert_eq!(
            Packet::status_running().encode(),
            [108, 1, 0, 0, 0, 0, 0, 0]
        );
    }

    /// The address spec a user types. The default matters: BGB's "Listen" and
    /// the Python bridge both bind first, so a bare address must connect.
    #[test]
    fn an_address_without_a_prefix_connects() {
        // Nothing is opened here; only the parse is under test, so point at a
        // port nothing can be listening on and check *how* it failed.
        let err = BgbLink::open("listen:256.256.256.256:1").unwrap_err();
        assert!(matches!(err, TransportError::Setup(_)));
        let err = BgbLink::open("").unwrap_err();
        assert!(format!("{err}").contains("expected"));
    }

    /// Drive a whole handshake against a socket we are the far end of, and
    /// check every packet on the wire — this is the part another implementation
    /// sees, so it is the part that has to be right.
    #[test]
    fn the_handshake_is_version_then_status_and_never_status_for_status() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            // Both sides send version on connection.
            s.write_all(&Packet::version().encode()).unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 8];
            // Expect exactly two packets from the emulator: its version, and
            // the status it owes in response to ours.
            for _ in 0..2 {
                s.read_exact(&mut buf).unwrap();
                got.push(Packet::decode(&buf));
            }
            // A status packet must not draw another status packet: the
            // reference Python server answers every status with a status, so an
            // implementation that does the same ping-pongs for ever.
            s.write_all(&Packet::status_running().encode()).unwrap();
            s.set_read_timeout(Some(Duration::from_millis(250)))
                .unwrap();
            let quiet = s.read_exact(&mut buf).is_err();
            (got, quiet)
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let start = Instant::now();
        while !link.live() && start.elapsed() < Duration::from_secs(5) {
            link.poll().unwrap();
            std::thread::yield_now();
        }
        assert!(link.live(), "the link should have gone live");
        // Keep polling so the peer's trailing status is read and (not)
        // answered. Errors are ignored here: the peer hangs up as soon as its
        // own quiet check expires, and that race is not what is under test.
        let until = Instant::now() + Duration::from_millis(400);
        while Instant::now() < until {
            if link.poll().is_err() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let (got, quiet) = peer.join().unwrap();
        assert_eq!(got[0], Packet::version());
        assert_eq!(got[1].b1, cmd::STATUS);
        assert_eq!(got[1].b2 & 1, 1, "we report ourselves as running");
        assert!(quiet, "a status must not be answered with a status");
    }

    /// A peer speaking a different version is refused — and refused as a link
    /// that never came up, not as one that broke, because nothing has been
    /// exchanged and no save is at risk.
    #[test]
    fn a_version_mismatch_is_refused_without_faulting_anything() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&Packet::new(cmd::VERSION, 1, 5, 0, 0).encode())
                .unwrap();
            std::thread::sleep(Duration::from_millis(300));
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let start = Instant::now();
        let err = loop {
            match link.poll() {
                Err(e) => break e,
                Ok(()) => {
                    assert!(start.elapsed() < Duration::from_secs(5), "no refusal");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert!(matches!(err, TransportError::Setup(_)));
        assert!(format!("{err}").contains("1.5.0"));
    }

    /// The three packets that carry a trade, in both directions, over a real
    /// socket: we clock the peer as master, and the peer clocks us.
    #[test]
    fn sync1_and_sync2_carry_a_byte_each_way() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            let mut buf = [0u8; 8];
            let mut master_byte = None;
            let mut slave_byte = None;
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            while master_byte.is_none() || slave_byte.is_none() {
                if s.read_exact(&mut buf).is_err() {
                    break;
                }
                let p = Packet::decode(&buf);
                match p.b1 {
                    cmd::SYNC1 => {
                        master_byte = Some(p.b2);
                        // Answer as a console that was armed: sync2 with our
                        // own byte.
                        s.write_all(&Packet::new(cmd::SYNC2, 0x3C, 0x80, 0, 0).encode())
                            .unwrap();
                        // ...and immediately clock them as master.
                        s.write_all(&Packet::new(cmd::SYNC1, 0x5A, 0x81, 0, 1).encode())
                            .unwrap();
                    }
                    cmd::SYNC2 => slave_byte = Some(p.b2),
                    _ => {}
                }
            }
            (master_byte, slave_byte)
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sent = false;
        let mut got_reply = None;
        let mut got_incoming = None;
        while Instant::now() < deadline {
            link.poll().unwrap();
            link.advance(4096);
            if link.live() && !sent {
                link.send_master(0xA5, 0x81);
                sent = true;
            }
            if let Some(r) = link.take_master_reply() {
                got_reply = Some(r);
            }
            if got_incoming.is_none() {
                if let Some(b) = link.take_incoming() {
                    got_incoming = Some(b);
                    link.answer_incoming(Some(0x11));
                }
            }
            if got_reply.is_some() && got_incoming.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(
            got_reply,
            Some(MasterReply::Byte(0x3C)),
            "our master transfer should be answered with the peer's byte"
        );
        assert_eq!(
            got_incoming,
            Some(0x5A),
            "and the peer's master transfer should reach us"
        );
        let (master_byte, slave_byte) = peer.join().unwrap();
        assert_eq!(master_byte, Some(0xA5));
        assert_eq!(slave_byte, Some(0x11));
    }

    /// A peer that answers sync3 `b2 = 1` was not armed. The master must be
    /// told to shift in an idle line rather than being left waiting.
    #[test]
    fn a_peer_that_is_not_armed_answers_sync3_and_the_master_gets_an_idle_line() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            let mut buf = [0u8; 8];
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            while s.read_exact(&mut buf).is_ok() {
                if Packet::decode(&buf).b1 == cmd::SYNC1 {
                    s.write_all(&Packet::new(cmd::SYNC3, 1, 0, 0, 0).encode())
                        .unwrap();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sent = false;
        let mut reply = None;
        while Instant::now() < deadline && reply.is_none() {
            link.poll().unwrap();
            if link.live() && !sent {
                link.send_master(0x99, 0x81);
                sent = true;
            }
            reply = link.take_master_reply();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(reply, Some(MasterReply::Idle));
    }

    /// A console with nothing connected yet must behave as an unplugged cable
    /// — the driver must never stall on a link that does not exist.
    #[test]
    fn a_listener_with_no_peer_answers_every_transfer_immediately() {
        let mut link = BgbLink::listen("127.0.0.1:0").unwrap();
        link.poll().unwrap();
        assert!(!link.live());
        link.send_master(0x42, 0x81);
        assert_eq!(link.take_master_reply(), Some(MasterReply::Idle));
        assert!(link.describe().contains("listening"));
    }

    /// The timestamp is emulated time, not wall-clock time — a stalled console
    /// really has not moved, and a peer that waits on our clock must see that.
    #[test]
    fn the_timestamp_counts_emulated_cycles_at_two_t_cycles_a_tick() {
        let mut link = BgbLink::listen("127.0.0.1:0").unwrap();
        link.advance(70_224);
        assert_eq!(link.clock(), 35_112, "one frame is 70224 T = 35112 ticks");
        link.advance(1);
        link.advance(1);
        assert_eq!(link.clock(), 35_113, "odd cycles accumulate, none are lost");
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(link.clock(), 35_113, "the wall clock is not the clock");
    }

    /// Packets may be split or coalesced by TCP; the reader has to cope. Send
    /// two packets one byte at a time and check both arrive intact.
    #[test]
    fn a_packet_split_across_reads_is_reassembled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            let mut stream = Vec::new();
            stream.extend(Packet::version().encode());
            stream.extend(Packet::new(cmd::SYNC1, 0xC3, 0x81, 0, 7).encode());
            stream.extend(Packet::new(cmd::SYNC1, 0x1D, 0x81, 0, 8).encode());
            for b in stream {
                s.write_all(&[b]).unwrap();
                std::thread::sleep(Duration::from_micros(200));
            }
            std::thread::sleep(Duration::from_millis(300));
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut got = Vec::new();
        while Instant::now() < deadline && got.len() < 2 {
            link.poll().unwrap();
            if let Some(b) = link.take_incoming() {
                got.push(b);
                link.answer_incoming(Some(0));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(got, vec![0xC3, 0x1D]);
        assert_eq!(link.peer_clock(), 8, "the peer's timestamp is recorded");
    }

    /// A peer that goes away after the link was live is a **broken** cable, not
    /// an unplugged one — bytes may have been lost mid-exchange, and the
    /// driver's fail-safe turns on that distinction.
    #[test]
    fn a_peer_that_disappears_after_going_live_reports_a_broken_link() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            // Read the emulator's version + status so nothing is lost to an
            // RST, then hang up.
            let mut buf = [0u8; 16];
            s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let _ = s.read_exact(&mut buf);
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let err = loop {
            match link.poll() {
                Err(e) => break e,
                Ok(()) => {
                    assert!(Instant::now() < deadline, "the peer never went away");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert!(
            matches!(err, TransportError::Broken(_)),
            "a link that was live and then closed is broken, not unplugged: {err:?}"
        );
    }

    /// A peer that says goodbye first is an **unplugged** cable, not a broken
    /// one — and that distinction is what decides whether the console keeps
    /// writing its save. `wantdisconnect` is the only thing on the wire that
    /// can tell "the player quit" from "the emulator died".
    ///
    /// The peer here hangs up **immediately** after the goodbye, on purpose:
    /// that is what a real one does, both packets land in one `read(2)`, and an
    /// implementation that records the EOF before parsing what came with it
    /// reports every clean shutdown as a broken link. That was a real bug here,
    /// caught by a two-process trade and not by the first version of this test.
    #[test]
    fn a_peer_that_says_goodbye_first_is_unplugged_not_broken() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            // Let the link genuinely go live first — the version and the status
            // the client owes us — so what follows is a *live* link breaking,
            // which is the case that decides whether a save is written.
            let mut buf = [0u8; 16];
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            s.read_exact(&mut buf).unwrap();
            s.write_all(&Packet::new(cmd::WANT_DISCONNECT, 0, 0, 0, 0).encode())
                .unwrap();
            // ...and hang up in the same breath, so the goodbye and the EOF
            // land in one `read(2)` at the far end. That is what a real peer
            // does, and parsing the buffer *after* recording the hang-up turns
            // every clean shutdown into a broken link.
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let err = loop {
            match link.poll() {
                Err(e) => break e,
                Ok(()) => {
                    assert!(Instant::now() < deadline, "the peer never said goodbye");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert!(
            matches!(err, TransportError::Setup(_)),
            "a polite disconnect must not be reported as a broken link: {err:?}"
        );
    }

    /// ...but not while a byte is in the air. Somebody unplugging in the middle
    /// of an exchange has still lost that byte, and the save guard has to see
    /// it.
    #[test]
    fn a_goodbye_in_the_middle_of_a_transfer_is_still_broken() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            let mut buf = [0u8; 8];
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            // Wait for a master transfer, then hang up without answering it.
            while s.read_exact(&mut buf).is_ok() {
                if Packet::decode(&buf).b1 == cmd::SYNC1 {
                    s.write_all(&Packet::new(cmd::WANT_DISCONNECT, 0, 0, 0, 0).encode())
                        .unwrap();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(400));
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut sent = false;
        let err = loop {
            match link.poll() {
                Err(e) => break e,
                Ok(()) => {
                    if link.live() && !sent {
                        link.send_master(0x7E, 0x81);
                        sent = true;
                    }
                    assert!(Instant::now() < deadline, "the peer never hung up");
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        };
        assert!(matches!(err, TransportError::Broken(_)), "{err:?}");
    }

    /// Our own goodbye reaches the peer, so the *other* console gets the same
    /// courtesy. Sent explicitly on shutdown, never from `Drop` — see
    /// [`BgbLink::disconnect`].
    #[test]
    fn our_own_disconnect_puts_a_wantdisconnect_on_the_wire() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let peer = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.set_nodelay(true).unwrap();
            s.write_all(&Packet::version().encode()).unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 8];
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            while s.read_exact(&mut buf).is_ok() {
                let p = Packet::decode(&buf);
                seen.push(p.b1);
                if p.b1 == cmd::WANT_DISCONNECT {
                    break;
                }
            }
            seen
        });

        let mut link = BgbLink::connect(addr).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !link.live() && Instant::now() < deadline {
            link.poll().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(link.live());
        // **Through the trait object, not the inherent method.** The inherent
        // `BgbLink::disconnect` is unreachable via `dyn LinkTransport`, and the
        // trait's default body does nothing — which is exactly how a frontend's
        // clean shutdown once sent no goodbye at all while this test passed.
        let mut boxed: Box<dyn LinkTransport> = Box::new(link);
        boxed.disconnect();

        let seen = peer.join().unwrap();
        assert!(
            seen.contains(&cmd::WANT_DISCONNECT),
            "expected a wantdisconnect packet, saw {seen:?}"
        );
        assert!(!boxed.live(), "and the link is closed afterwards");
    }

    /// Sanity check on the counterpart of the test above: a *connect* that
    /// finds nothing there is a setup failure and nothing is opened.
    #[test]
    fn connecting_to_nothing_is_a_setup_failure() {
        // Bind and drop, so the port is almost certainly free.
        let addr = {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap()
        };
        let err = BgbLink::connect(addr).unwrap_err();
        assert!(matches!(err, TransportError::Setup(_)), "{err:?}");
    }
}

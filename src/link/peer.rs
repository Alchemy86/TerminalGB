//! **Plug the cable in by starting a second emulator.**
//!
//! [`LinkCable`](super::LinkCable) joins two cores inside one process. This
//! joins two *processes*: start `terminalgb blue.gb` in one terminal and
//! `terminalgb gold.gbc` in another and they find each other, pair, and present
//! as a plugged-in cable to both games — with nothing to arm, configure or
//! press.
//!
//! That is not a convenience, it is a correctness requirement. Gen 1's Cable
//! Club attendant *checks the link before she lets you through*
//! (`CableClubNPC`, pokered `engine/link/cable_club_npc.asm`: 90 frames of
//! handshake, then "This area is reserved for 2 friends who are linked by
//! cable"). For her to see a partner, the serial port has to be genuinely live
//! at the hardware level while the player is standing at her desk — an
//! on-request link that only wakes up once somebody asks for it is exactly the
//! link she turns away.
//!
//! # Discovery, and what happens with three
//!
//! Each instance looks in a rendezvous directory (`$XDG_RUNTIME_DIR/terminalgb/
//! link`, else `/tmp/terminalgb-link-<uid>`; `$GB_LINK_DIR` overrides). It
//! tries to connect to every socket it finds. The first one that answers is its
//! partner. If none does, it creates its own socket and waits.
//!
//! **A cable has two ends, so instances pair up two at a time, in the order
//! they start.** A third instance finds the first pair's socket already
//! answering [`Kind::Busy`], creates its own, and waits — so it links to the
//! *fourth* instance, not to either of the first two. That is the honest
//! hardware analogy: a second cable, not a four-player adapter, which this
//! emulator does not model. The consequence to know is that pairing is by
//! **arrival order, not by cartridge** — start Blue, then Blue, then Gold and
//! the two Blues share the cable. Set `link = off` on the one that should not
//! take part.
//!
//! # It must never block, and it must never cost the frame
//!
//! Everything here is non-blocking, and this module owes its shape to two
//! lessons already paid for in this repository:
//!
//! * **Nothing on the emulator's clock may wait on something else's**
//!   (`docs/audio.md` §4c, where a blocking audio hand-off cost 17.9 ms of a
//!   16.7 ms frame). A peer that stops reading loses bytes; it never costs
//!   frame time.
//! * **A partial write must be finished before the next message**
//!   (`src/video.rs`, where a half-written frame desynchronised the consumer
//!   for the rest of the run). The outgoing buffer is drained in order.
//!
//! The socket is polled every [`SERVICE_TICKS`] T-cycles rather than every
//! instruction: a `read` per instruction would be ~1 M syscalls a second for
//! nothing. The *role* check is a couple of field reads and does happen every
//! instruction, so the hundred-cycle window Gen 1's handshake turns on is never
//! missed — see [`LinkPeer::service`].
//!
//! # Failing safe
//!
//! A peer that goes away **while the cable is carrying bytes** faults both
//! consoles (`Gameboy::report_fault`, so the battery save is not written
//! again) — half a trade leaves the survivor holding the dangerous file. A peer
//! that says goodbye, or that vanishes while the cable has been idle, just
//! unplugs: no fault, and the game carries on exactly as it would with no cable
//! at all.

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use crate::gameboy::{Gameboy, FRAME_TICKS};
use crate::mmu::serial::LinkRole;

use super::LinkStats;

/// How often the socket is actually read and written, in T-cycles.
///
/// A byte on the wire takes 4096 T-cycles, so this is sixteen chances to get a
/// message across during one transfer — while costing ~274 polls per frame
/// instead of the ~17,500 that per-instruction polling would.
pub const SERVICE_TICKS: u32 = 256;

/// How long a master transfer's offer is held open on the receiving side.
///
/// One transfer's worth of T-cycles, which mirrors what the in-process cable
/// does by re-checking: a console that arms its port *after* the master started
/// is still in time, because the master is still shifting.
const OFFER_TICKS: u32 = 4096;

/// A peer vanishing within this much emulated time of the last byte is treated
/// as a link that was in use, and faults both consoles. One second.
const ACTIVE_WINDOW_TICKS: u64 = 4_194_304;

/// How long a held master transfer waits for the far console before giving up
/// and shifting in the idle line. 250 ms of emulated time — far longer than two
/// frame-paced processes can stay out of step, and short enough that a peer
/// which has wedged does not hang the game.
const HOLD_TIMEOUT_TICKS: u32 = 1_048_576;

/// Cap on unsent bytes before the peer is declared stalled. A transfer is 32
/// bytes each way; this is hundreds of them.
const OUTBUF_CAP: usize = 64 * 1024;

const FRAME_LEN: usize = 32;
const PROTOCOL_VERSION: u32 = 1;

/// What a frame on the wire means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Kind {
    /// Cartridge title, save-file key, protocol version.
    Hello = 1,
    /// "I have started an internally clocked transfer; here is my byte."
    Master = 2,
    /// The answer: the byte shifted back, and whether anything was listening.
    Slave = 3,
    /// "This cable already has two ends." Sent to a third instance.
    Busy = 4,
    /// A human-readable label for the far end — the frontend fills it in (the
    /// player's in-game name, when something can read one).
    Label = 5,
    /// A clean goodbye, so a quit is not mistaken for a crash.
    Bye = 6,
    /// "Forget the transfer I told you about; I have given up on it." Without
    /// this, a far console that arms its port *after* we gave up answers a
    /// transfer nobody is listening for any more — and spends one of **its**
    /// bytes doing it, which slides its whole stream by one.
    Abort = 7,
}

impl Kind {
    fn from_u8(v: u8) -> Option<Kind> {
        Some(match v {
            1 => Kind::Hello,
            2 => Kind::Master,
            3 => Kind::Slave,
            4 => Kind::Busy,
            5 => Kind::Label,
            6 => Kind::Bye,
            7 => Kind::Abort,
            _ => return None,
        })
    }
}

/// One fixed-size message. Fixed size because framing a stream socket is the
/// classic place to introduce a bug for no benefit, and 32 bytes twice per
/// transfer is ~1 KB per frame at the busiest.
#[derive(Debug, Clone, Copy, Default)]
struct Frame {
    kind: u8,
    byte: u8,
    flags: u8,
    seq: u32,
    save_key: u64,
    text: [u8; 16],
}

impl Frame {
    fn encode(&self) -> [u8; FRAME_LEN] {
        let mut b = [0u8; FRAME_LEN];
        b[0] = self.kind;
        b[1] = self.byte;
        b[2] = self.flags;
        b[4..8].copy_from_slice(&self.seq.to_le_bytes());
        b[8..16].copy_from_slice(&self.save_key.to_le_bytes());
        b[16..32].copy_from_slice(&self.text);
        b
    }

    fn decode(b: &[u8]) -> Frame {
        let mut text = [0u8; 16];
        text.copy_from_slice(&b[16..32]);
        Frame {
            kind: b[0],
            byte: b[1],
            flags: b[2],
            seq: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            save_key: u64::from_le_bytes([
                b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15],
            ]),
            text,
        }
    }

    fn of(kind: Kind) -> Frame {
        Frame {
            kind: kind as u8,
            ..Frame::default()
        }
    }

    fn with_text(mut self, s: &str) -> Frame {
        for (dst, src) in self.text.iter_mut().zip(s.bytes()) {
            *dst = src;
        }
        self
    }

    fn text_str(&self) -> String {
        let end = self.text.iter().position(|&b| b == 0).unwrap_or(16);
        String::from_utf8_lossy(&self.text[..end])
            .trim()
            .to_string()
    }
}

/// What the cable is doing, for the UI. These are three different things to
/// somebody deciding whether to walk into a Cable Club.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PeerState {
    /// Turned off by configuration.
    Off,
    /// Listening, nobody there yet. The game sees an unplugged cable.
    Waiting,
    /// A second console is on the other end.
    Paired {
        /// Its cartridge title, from the handshake.
        title: String,
        /// A label the far frontend supplied — the player's in-game name, when
        /// anything over there can read one. Empty otherwise.
        label: String,
        /// Both consoles are running the same cartridge **and would write the
        /// same save file**. See [`LinkPeer::same_save_file`].
        same_save: bool,
    },
    /// There was a peer and it has gone.
    Unplugged,
}

impl PeerState {
    pub fn is_paired(&self) -> bool {
        matches!(self, PeerState::Paired { .. })
    }
}

/// Our master transfer, waiting for the far console's answer.
#[derive(Debug, Clone, Copy)]
struct MasterXfer {
    seq: u32,
    answered: bool,
    /// Emulated T-cycles this transfer has been held waiting for an answer.
    held: u32,
}

/// The far console's master transfer, waiting for us to arm a slave.
#[derive(Debug, Clone, Copy)]
struct Offer {
    seq: u32,
    byte: u8,
    age: u32,
}

/// One end of an automatically discovered link cable.
pub struct LinkPeer {
    listener: Option<UnixListener>,
    /// Our own socket path, unlinked on `close`.
    sock_path: Option<PathBuf>,
    stream: Option<UnixStream>,
    inbuf: Vec<u8>,
    outbuf: VecDeque<u8>,
    state: PeerState,
    local_title: String,
    local_label: String,
    save_key: u64,
    stats: LinkStats,
    mine: Option<MasterXfer>,
    offer: Option<Offer>,
    since_poll: u32,
    idle_ticks: u64,
    /// Set once, so a flapping cable cannot fault twice.
    faulted: bool,
    /// Set while a `Label` needs sending.
    label_dirty: bool,
    /// The far end said goodbye, so its disappearance is not a crash.
    graceful: bool,
    /// We have had a `Hello` from this connection. **Nothing may fault over a
    /// peer we never actually spoke to** — a connection that dies before the
    /// handshake is a crowded rendezvous, not a crashed partner.
    handshaked: bool,
    /// Bumped whenever a *different* connection becomes ours. End-of-stream is
    /// noticed before the frames that arrived with it are handled, and one of
    /// those frames can replace the connection (`Busy` restarts discovery, and
    /// may pair us with somebody else in the same call). Without this, that
    /// fresh peer would be torn down by the previous one's end-of-stream.
    conn_gen: u64,
    /// Kept so a `Busy` answer can start discovery again from scratch.
    save_path: Option<PathBuf>,
}

impl LinkPeer {
    /// A cable that is switched off. Costs one `matches!` per service call.
    pub fn off() -> LinkPeer {
        LinkPeer::empty(PeerState::Off)
    }

    fn empty(state: PeerState) -> LinkPeer {
        LinkPeer {
            listener: None,
            sock_path: None,
            stream: None,
            inbuf: Vec::new(),
            outbuf: VecDeque::new(),
            state,
            local_title: String::new(),
            local_label: String::new(),
            save_key: 0,
            stats: LinkStats::default(),
            mine: None,
            offer: None,
            since_poll: 0,
            idle_ticks: u64::MAX,
            faulted: false,
            label_dirty: false,
            graceful: false,
            handshaked: false,
            conn_gen: 0,
            save_path: None,
        }
    }

    /// Find a peer, or become one. **Never blocks and never fails**: with no
    /// rendezvous directory, no permission or no peer, the result is a cable
    /// with nothing on the end of it, which behaves exactly as no cable at all.
    ///
    /// `title` is this cartridge's name and `save_path` is the file this
    /// console will write, used only to warn when both ends would write the
    /// same one.
    pub fn discover(title: &str, save_path: Option<&Path>) -> LinkPeer {
        let mut peer = LinkPeer::empty(PeerState::Waiting);
        peer.local_title = title.to_string();
        peer.save_key = save_path.map(fnv64).unwrap_or(0);
        peer.save_path = save_path.map(|p| p.to_path_buf());

        let Some(dir) = rendezvous_dir() else {
            peer.state = PeerState::Waiting;
            return peer;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return peer;
        }

        // Try every socket already there. The first that answers is our
        // partner; anything that refuses is a crashed instance's leftovers,
        // because `Drop` is not a shutdown hook in this codebase (see
        // `src/mbc/battery.rs` and `src/shmframe.rs` for the same sweep).
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for e in entries.flatten() {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "sock") {
                    continue;
                }
                match UnixStream::connect(&p) {
                    Ok(s) => {
                        if peer.adopt(s) {
                            return peer;
                        }
                    }
                    Err(_) => {
                        // Nothing listening: the owner is gone.
                        let _ = std::fs::remove_file(&p);
                    }
                }
            }
        }

        // Nobody home. Be the one who waits.
        peer.advertise();
        peer
    }

    /// Put our socket in the rendezvous directory so other instances can find
    /// us. Idempotent, and silent about failure — a cable nobody can find is
    /// the same as no cable.
    fn advertise(&mut self) {
        if self.listener.is_some() {
            return;
        }
        let Some(dir) = rendezvous_dir() else { return };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        if let Ok(l) = UnixListener::bind(&path) {
            let _ = l.set_nonblocking(true);
            self.listener = Some(l);
            self.sock_path = Some(path);
        }
    }

    /// Stop advertising. **Called the moment we pair**, and it is what makes
    /// three instances terminate: a socket that is still listed is a socket a
    /// third instance will keep connecting to, only to be told it is busy and
    /// to come back and try the same one again.
    fn retire(&mut self) {
        // Turn away anyone already queued *before* letting the listener go.
        // Dropping it resets the backlog, and a connection that dies without
        // ever having said anything is indistinguishable, from the far end,
        // from a partner that crashed — which is a fault, and a poisoned save,
        // for a console that was never linked at all.
        if let Some(l) = self.listener.as_ref() {
            while let Ok((mut s, _)) = l.accept() {
                let _ = s.write_all(&Frame::of(Kind::Busy).encode());
            }
        }
        self.listener = None;
        if let Some(p) = self.sock_path.take() {
            let _ = std::fs::remove_file(p);
        }
    }

    /// Take a freshly connected stream as our partner. Returns `false` if the
    /// far end said the cable already has two ends.
    fn adopt(&mut self, s: UnixStream) -> bool {
        if s.set_nonblocking(true).is_err() {
            return false;
        }
        self.stream = Some(s);
        self.conn_gen += 1;
        self.retire();
        self.push(Frame {
            save_key: self.save_key,
            seq: PROTOCOL_VERSION,
            ..Frame::of(Kind::Hello).with_text(&self.local_title)
        });
        self.flush();
        // The far end's HELLO (or BUSY) arrives in the ordinary read path, so
        // pairing completes in `service` rather than here — nothing blocks.
        self.state = PeerState::Paired {
            title: String::new(),
            label: String::new(),
            same_save: false,
        };
        true
    }

    /// What the cable is doing. Show this: a link nobody can see is a link
    /// nobody can debug, and here it is also the answer to "can I walk into the
    /// Cable Club yet?".
    pub fn state(&self) -> &PeerState {
        &self.state
    }

    pub fn stats(&self) -> LinkStats {
        self.stats
    }

    /// Both ends are writing the same `.sav`. Two instances of one cartridge on
    /// one machine is the *first* thing anybody tries, and it is exactly how a
    /// save gets destroyed — both consoles autosave over each other. The cable
    /// still works; the frontend must warn.
    pub fn same_save_file(&self) -> bool {
        matches!(
            self.state,
            PeerState::Paired {
                same_save: true,
                ..
            }
        )
    }

    /// Name this console's cartridge, and the save file it will write.
    ///
    /// Discovery happens **before** a ROM is chosen — the port has to be live
    /// from the first frame of the first game, which is the whole point — so
    /// the cartridge is not known yet. This fills it in afterwards, and
    /// re-introduces us if a partner is already on the line.
    pub fn set_identity(&mut self, title: &str, save_path: Option<&Path>) {
        let key = save_path.map(fnv64).unwrap_or(0);
        if self.local_title == title && self.save_key == key {
            return;
        }
        self.local_title = title.to_string();
        self.save_key = key;
        self.save_path = save_path.map(|p| p.to_path_buf());
        if self.stream.is_some() {
            let t = self.local_title.clone();
            self.push(Frame {
                save_key: key,
                seq: PROTOCOL_VERSION,
                ..Frame::of(Kind::Hello).with_text(&t)
            });
        }
    }

    /// Tell the far end who is playing here. The frontend supplies it (the
    /// trainer plugin can read a Gen 1 player's name); core stays game-agnostic.
    pub fn set_label(&mut self, label: &str) {
        if self.local_label != label {
            self.local_label = label.to_string();
            self.label_dirty = true;
        }
    }

    /// Say goodbye and take our socket out of the rendezvous directory, so a
    /// clean quit is not mistaken for a crash and does not fault the peer.
    ///
    /// Deliberately **not** a `Drop` impl. `Drop` is not a shutdown hook here —
    /// the release profile aborts on panic and the frontends exit without
    /// unwinding — which is the same lesson battery saves cost this project.
    pub fn close(&mut self) {
        if self.stream.is_some() {
            self.push(Frame::of(Kind::Bye));
            self.flush();
        }
        if let Some(p) = self.sock_path.take() {
            let _ = std::fs::remove_file(p);
        }
        self.stream = None;
        self.listener = None;
    }

    /// Advance one console by a frame's worth of cycles with the cable live.
    ///
    /// The mirror of [`LinkCable::run_frame`](super::LinkCable::run_frame) for
    /// the one-core case: it performs the same per-frame housekeeping
    /// `Gameboy::frame` does, and services the wire as it goes.
    pub fn run_frame(&mut self, gb: &mut Gameboy) {
        gb.begin_frame();
        let mut ticks = 0u32;
        while ticks < FRAME_TICKS {
            let t = gb.step_instruction();
            ticks += t;
            self.service(gb, t);
        }
        gb.end_frame();
    }

    /// Move the wire on. Call between instructions; `ticks` is what the last
    /// instruction cost.
    ///
    /// The **role** check happens every call and is a couple of field reads, so
    /// the hundred-cycle window Gen 1's handshake turns on is never missed. The
    /// **socket** is only touched every [`SERVICE_TICKS`] T-cycles, or at once
    /// when there is something to say — a syscall per instruction would be a
    /// million a second to learn nothing.
    pub fn service(&mut self, gb: &mut Gameboy, ticks: u32) {
        if matches!(self.state, PeerState::Off) {
            return;
        }
        self.since_poll = self.since_poll.saturating_add(ticks);
        self.idle_ticks = self.idle_ticks.saturating_add(ticks as u64);

        let role = gb.link_role();

        // Cheap, every instruction: has our own port started or finished a
        // transfer, and have we armed a slave the far end is waiting on?
        let mut urgent = false;

        match (role, self.mine) {
            (LinkRole::Master, None) => {
                let seq = gb.link_seq();
                self.mine = Some(MasterXfer {
                    seq,
                    answered: false,
                    held: 0,
                });
                // Do not let the register run out until the far console has
                // answered — see `Serial::link_hold`.
                gb.link_set_hold(self.state.is_paired());
                self.push(Frame {
                    byte: gb.link_out(),
                    seq,
                    ..Frame::of(Kind::Master)
                });
                urgent = true;
            }
            (LinkRole::Master, Some(m)) if gb.link_seq() != m.seq => {
                // A second transfer inside one poll window. At 4096 T-cycles
                // apiece this cannot happen, but losing a byte to an assumption
                // is not worth the line saved.
                if !m.answered {
                    self.stats.unanswered += 1;
                }
                let seq = gb.link_seq();
                self.mine = Some(MasterXfer {
                    seq,
                    answered: false,
                    held: 0,
                });
                gb.link_set_hold(self.state.is_paired());
                self.push(Frame {
                    byte: gb.link_out(),
                    seq,
                    ..Frame::of(Kind::Master)
                });
                urgent = true;
            }
            (LinkRole::Master, Some(mut m)) => {
                // Still shifting, still unanswered. Give up after a quarter of
                // a second of emulated time and let it finish the way an
                // unplugged cable would.
                if !m.answered {
                    m.held = m.held.saturating_add(ticks);
                    if m.held >= HOLD_TIMEOUT_TICKS {
                        gb.link_set_hold(false);
                        m.answered = true;
                        // Counted as `torn`, not `unanswered`: the far console
                        // never spoke, so a byte really was lost. `unanswered`
                        // is the *benign* case — it answered "nobody
                        // listening", which is what an idle console says and
                        // what Gen 1's handshake produces hundreds of.
                        self.stats.torn += 1;
                        self.push(Frame {
                            seq: m.seq,
                            ..Frame::of(Kind::Abort)
                        });
                        urgent = true;
                        // A quarter of a second of emulated time with no answer
                        // at all from a socket on the same machine means the
                        // far console is not executing — paused, wedged, or
                        // stopped in a debugger. The byte we just shifted in is
                        // `$FF` and the far end does not know it, so the two
                        // streams have diverged: this is the corrupt-trade case
                        // the save guard exists for.
                        self.lost_byte(gb);
                    }
                    self.mine = Some(m);
                }
            }
            (_, Some(m)) => {
                if !m.answered {
                    self.stats.unanswered += 1;
                }
                gb.link_set_hold(false);
                self.mine = None;
            }
            _ => {}
        }

        // An offer we are holding: answer it the instant this console arms its
        // port, and give up when the far end's transfer must have run out.
        if let Some(mut o) = self.offer {
            if role == LinkRole::Slave {
                let ours = gb.link_out();
                let landed = gb.link_clock_slave(o.byte);
                self.push(Frame {
                    byte: ours,
                    flags: u8::from(landed),
                    seq: o.seq,
                    ..Frame::of(Kind::Slave)
                });
                if landed {
                    self.stats.transfers += 1;
                    self.idle_ticks = 0;
                }
                self.offer = None;
                urgent = true;
            } else {
                o.age = o.age.saturating_add(ticks);
                if o.age >= OFFER_TICKS {
                    self.push(Frame {
                        seq: o.seq,
                        ..Frame::of(Kind::Slave)
                    });
                    self.offer = None;
                    urgent = true;
                } else {
                    self.offer = Some(o);
                }
            }
        }

        if !urgent && self.since_poll < SERVICE_TICKS && self.outbuf.is_empty() {
            return;
        }
        self.since_poll = 0;

        self.accept_if_waiting();
        self.flush();
        self.pump(gb);
    }

    /// Take a partner if one knocks, and turn away a third instance.
    fn accept_if_waiting(&mut self) {
        let Some(l) = self.listener.as_ref() else {
            return;
        };
        match l.accept() {
            Ok((s, _)) => {
                if self.stream.is_some() {
                    // A cable has two ends. Tell the newcomer plainly rather
                    // than dropping the connection, so it can report *why* it
                    // is not linked instead of guessing.
                    let mut s = s;
                    let _ = s.write_all(&Frame::of(Kind::Busy).encode());
                    return;
                }
                if s.set_nonblocking(true).is_ok() {
                    self.stream = Some(s);
                    self.conn_gen += 1;
                    self.retire();
                    self.push(Frame {
                        save_key: self.save_key,
                        seq: PROTOCOL_VERSION,
                        ..Frame::of(Kind::Hello).with_text(&self.local_title)
                    });
                    self.state = PeerState::Paired {
                        title: String::new(),
                        label: String::new(),
                        same_save: false,
                    };
                    self.label_dirty = !self.local_label.is_empty();
                }
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => {}
            Err(_) => {}
        }
    }

    fn push(&mut self, f: Frame) {
        if self.outbuf.len() > OUTBUF_CAP {
            return;
        }
        self.outbuf.extend(f.encode());
    }

    /// Drain what we owe the far end, in order. A partial write is kept and
    /// finished next time — a half-written frame would desynchronise the
    /// stream for the rest of the session.
    fn flush(&mut self) {
        let Some(s) = self.stream.as_mut() else {
            self.outbuf.clear();
            return;
        };
        let mut dead = false;
        while !self.outbuf.is_empty() {
            let n = {
                let buf = self.outbuf.make_contiguous();
                match s.write(buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(_) => {
                        dead = true;
                        break;
                    }
                }
            };
            self.outbuf.drain(..n);
        }
        if dead {
            self.outbuf.clear();
        }
        if self.outbuf.len() > OUTBUF_CAP {
            // The far end has stopped reading. That is a dead cable, not a
            // reason to grow a buffer without bound.
            self.outbuf.clear();
            self.drop_peer(None);
        }
    }

    /// Read whatever has arrived and act on it.
    fn pump(&mut self, gb: &mut Gameboy) {
        let mut gone = false;
        let gen = self.conn_gen;
        if let Some(s) = self.stream.as_mut() {
            let mut buf = [0u8; 1024];
            loop {
                match s.read(&mut buf) {
                    Ok(0) => {
                        gone = true;
                        break;
                    }
                    Ok(n) => self.inbuf.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                    Err(_) => {
                        gone = true;
                        break;
                    }
                }
            }
        }

        while self.inbuf.len() >= FRAME_LEN {
            let f = Frame::decode(&self.inbuf[..FRAME_LEN]);
            self.inbuf.drain(..FRAME_LEN);
            self.handle(f, gb);
        }

        // Only tear down the connection that actually ended. A `Busy` or `Bye`
        // in the frames just handled has already dealt with it — and may have
        // put a *new* peer in its place.
        if gone && self.conn_gen == gen && self.stream.is_some() {
            self.drop_peer(Some(gb));
        }
    }

    fn handle(&mut self, f: Frame, gb: &mut Gameboy) {
        let Some(kind) = Kind::from_u8(f.kind) else {
            return;
        };
        match kind {
            Kind::Hello => {
                self.handshaked = true;
                let same = self.save_key != 0 && f.save_key == self.save_key;
                let label = match &self.state {
                    PeerState::Paired { label, .. } => label.clone(),
                    _ => String::new(),
                };
                self.state = PeerState::Paired {
                    title: f.text_str(),
                    label,
                    same_save: same,
                };
                if !self.local_label.is_empty() {
                    self.label_dirty = true;
                }
                if self.label_dirty {
                    self.label_dirty = false;
                    let l = self.local_label.clone();
                    self.push(Frame::of(Kind::Label).with_text(&l));
                }
            }
            Kind::Label => {
                if let PeerState::Paired { label, .. } = &mut self.state {
                    *label = f.text_str();
                }
            }
            Kind::Busy => {
                // Somebody else already has both ends. Start discovery over
                // rather than pretending to be linked: that leaves us
                // advertising a socket of our own, so a *fourth* instance
                // pairs with us instead of finding nothing.
                let (title, save) = (self.local_title.clone(), self.save_path.clone());
                let label = std::mem::take(&mut self.local_label);
                *self = LinkPeer::discover(&title, save.as_deref());
                self.local_label = label;
            }
            Kind::Bye => {
                self.graceful = true;
                self.stream = None;
                gb.link_set_hold(false);
                self.state = PeerState::Unplugged;
            }
            Kind::Master => {
                // The far console has started a transfer. Answer at once if our
                // port is armed; otherwise hold the offer open — it has the
                // rest of the far end's shift window to arrive.
                if gb.link_role() == LinkRole::Slave {
                    let ours = gb.link_out();
                    let landed = gb.link_clock_slave(f.byte);
                    self.push(Frame {
                        byte: ours,
                        flags: u8::from(landed),
                        seq: f.seq,
                        ..Frame::of(Kind::Slave)
                    });
                    if landed {
                        self.stats.transfers += 1;
                        self.idle_ticks = 0;
                    }
                } else if gb.link_role() == LinkRole::Master {
                    // Both consoles are driving the clock. Neither can serve
                    // the other and both are holding their shift registers, so
                    // holding this offer as well is a deadlock that only the
                    // timeout breaks — and every timeout is a lost byte. Say
                    // "nobody listening" at once instead, which is what two
                    // consoles fighting over the clock line see anyway.
                    //
                    // Gen 1's `.establishConnectionLoop` masters on *both*
                    // consoles every frame, so this is the ordinary case until
                    // the roles are settled, not an exotic one.
                    self.push(Frame {
                        seq: f.seq,
                        ..Frame::of(Kind::Slave)
                    });
                } else {
                    // Idle. Hold it open: the far console is still shifting, so
                    // arming our port within its window is still in time.
                    self.offer = Some(Offer {
                        seq: f.seq,
                        byte: f.byte,
                        age: 0,
                    });
                }
            }
            Kind::Abort => {
                if self.offer.is_some_and(|o| o.seq == f.seq) {
                    self.offer = None;
                }
            }
            Kind::Slave => {
                let Some(mut m) = self.mine else { return };
                if m.seq != f.seq || m.answered {
                    return;
                }
                if f.flags & 1 != 0 {
                    gb.link_answer_master(f.byte);
                    self.stats.transfers += 1;
                    self.idle_ticks = 0;
                } else {
                    // Nothing was listening. Leave the idle-high `$FF` the
                    // transfer started with, which is what an unplugged line
                    // reads as.
                    self.stats.unanswered += 1;
                }
                // Either way the far console has spoken, so let the shift
                // register finish on its own next edge.
                gb.link_set_hold(false);
                m.answered = true;
                self.mine = Some(m);
            }
        }
    }

    /// A byte went missing on a cable that is up. Take the save away, once.
    ///
    /// Unlike a peer *disappearing*, this is not ambiguous: the connection is
    /// alive, so the two consoles are still exchanging — they have simply
    /// stopped agreeing about what. Pausing one emulator in the middle of a
    /// trade lands here, and that is correct: the trade really is ruined.
    fn lost_byte(&mut self, gb: &mut Gameboy) {
        if self.faulted || !self.handshaked {
            return;
        }
        self.faulted = true;
        gb.report_fault(
            "the console on the other end of the link cable stopped answering \
             mid-exchange, so a byte was lost",
        );
    }

    /// The far end has gone. Whether that costs both saves depends on whether
    /// the cable was carrying anything.
    fn drop_peer(&mut self, gb: Option<&mut Gameboy>) {
        self.stream = None;
        let mut gb = gb;
        if let Some(gb) = gb.as_deref_mut() {
            // Never leave a transfer parked against a console that is gone.
            gb.link_set_hold(false);
        }
        self.inbuf.clear();
        self.outbuf.clear();
        let was_active = !self.graceful
            && self.handshaked
            && (self.mine.is_some_and(|m| !m.answered)
                || self.offer.is_some()
                || self.idle_ticks < ACTIVE_WINDOW_TICKS);
        self.mine = None;
        self.offer = None;
        self.state = PeerState::Unplugged;
        // Go back on the air. Somebody relaunching the console they just closed
        // is the most likely next event, and they should not have to restart
        // this one as well.
        self.graceful = false;
        self.handshaked = false;
        self.advertise();
        if was_active && !self.faulted {
            self.faulted = true;
            if let Some(gb) = gb {
                gb.report_fault(
                    "the console on the other end of the link cable disappeared \
                     mid-exchange",
                );
            }
        }
    }
}

/// Where instances look for each other. `$GB_LINK_DIR` overrides, which is how
/// tests and examples get a private rendezvous that cannot reach a real
/// session.
pub fn rendezvous_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("GB_LINK_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(d) = std::env::var_os("XDG_RUNTIME_DIR") {
        return Some(PathBuf::from(d).join("terminalgb/link"));
    }
    // No runtime dir (a bare cron job, a container). `/tmp` keyed by uid keeps
    // two users' cables apart.
    let uid = std::env::var("UID").unwrap_or_else(|_| "0".into());
    Some(std::env::temp_dir().join(format!("terminalgb-link-{uid}")))
}

fn fnv64(p: &Path) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in p.as_os_str().as_encoded_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `GB_LINK_DIR` is process-global and `cargo test` runs in parallel, so a
    /// test that sets it must hold this or two tests share a rendezvous
    /// directory and pair with each other. Same reason
    /// `sound::output_volume::test_lock` exists.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "terminalgb-peer-test/{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The same ROM both ends of the in-process cable's tests use: exchange a
    /// byte for ever, parking each one received at `$C000`.
    fn link_loop_rom(send: u8, clock: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x119].copy_from_slice(&[
            0x3E, send, 0xEA, 0x01, 0xFF, 0x3E, clock, 0xEA, 0x02, 0xFF, 0xFA, 0x02,
            0xFF, 0xCB, 0x7F, 0x20, 0xF9, 0xFA, 0x01, 0xFF, 0xEA, 0x00, 0xC0, 0x18, 0xE5,
        ]);
        rom
    }

    /// Two `LinkPeer`s in one test process, joined the way two *processes*
    /// would be — through the rendezvous directory and a real socket. The only
    /// thing this cannot show is separate address spaces, which
    /// `testharness/linkpeer.rs` covers by spawning actual processes.
    fn pair(dir: &Path) -> (LinkPeer, LinkPeer) {
        std::env::set_var("GB_LINK_DIR", dir);
        let a = LinkPeer::discover("CONSOLE A", Some(Path::new("/tmp/a.sav")));
        let b = LinkPeer::discover("CONSOLE B", Some(Path::new("/tmp/b.sav")));
        (a, b)
    }

    /// Drive both ends until they have exchanged `want` bytes or the budget
    /// runs out.
    fn run(
        a: (&mut LinkPeer, &mut Gameboy),
        b: (&mut LinkPeer, &mut Gameboy),
        frames: usize,
    ) {
        let (pa, ga) = a;
        let (pb, gb) = b;
        for _ in 0..frames {
            pa.run_frame(ga);
            pb.run_frame(gb);
        }
    }

    #[test]
    fn two_instances_find_each_other_and_exchange_a_byte() {
        let _guard = env_lock();
        let dir = scratch("exchange");
        let (mut pa, mut pb) = pair(&dir);
        let mut ga = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x3C, 0x80), None);

        run((&mut pa, &mut ga), (&mut pb, &mut gb), 6);

        assert!(
            pa.state().is_paired(),
            "A should have found B: {:?}",
            pa.state()
        );
        assert!(pb.state().is_paired());
        assert_eq!(
            ga.peek(0xC000),
            0x3C,
            "the master should shift in what the slave was holding"
        );
        assert_eq!(gb.peek(0xC000), 0xA5);
        assert!(pa.stats().transfers > 0 && pb.stats().transfers > 0);
    }

    /// The handshake the far end must see: each learns the other's cartridge.
    #[test]
    fn the_handshake_names_the_other_cartridge() {
        let _guard = env_lock();
        let dir = scratch("hello");
        let (mut pa, mut pb) = pair(&dir);
        let mut ga = Gameboy::new(link_loop_rom(0x11, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x22, 0x80), None);
        run((&mut pa, &mut ga), (&mut pb, &mut gb), 3);
        match pa.state() {
            PeerState::Paired { title, .. } => assert_eq!(title, "CONSOLE B"),
            s => panic!("A is not paired: {s:?}"),
        }
        match pb.state() {
            PeerState::Paired { title, .. } => assert_eq!(title, "CONSOLE A"),
            s => panic!("B is not paired: {s:?}"),
        }
    }

    /// **A single instance must behave exactly as it does with no cable.**
    #[test]
    fn one_instance_alone_sees_an_unplugged_cable() {
        let _guard = env_lock();
        let dir = scratch("alone");
        std::env::set_var("GB_LINK_DIR", &dir);
        let mut p = LinkPeer::discover("LONELY", None);
        let mut gb = Gameboy::new(link_loop_rom(0x5A, 0x81), None);
        for _ in 0..4 {
            p.run_frame(&mut gb);
        }
        assert_eq!(*p.state(), PeerState::Waiting);
        assert_eq!(
            gb.peek(0xC000),
            0xFF,
            "an unanswered master transfer shifts in an idle line"
        );
        assert!(gb.fault().is_none());
    }

    /// A cable has two ends. The third instance must know *why* it is not
    /// linked, and must go on to be available for a fourth.
    #[test]
    fn a_third_instance_is_told_the_cable_is_busy_and_waits() {
        let _guard = env_lock();
        let dir = scratch("three");
        let (mut pa, mut pb) = pair(&dir);
        std::env::set_var("GB_LINK_DIR", &dir);
        let mut pc = LinkPeer::discover("CONSOLE C", Some(Path::new("/tmp/c.sav")));
        let mut ga = Gameboy::new(link_loop_rom(0x11, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x22, 0x80), None);
        let mut gc = Gameboy::new(link_loop_rom(0x33, 0x81), None);
        for _ in 0..4 {
            pa.run_frame(&mut ga);
            pb.run_frame(&mut gb);
            pc.run_frame(&mut gc);
        }
        assert!(pa.state().is_paired());
        assert!(pb.state().is_paired());
        assert_eq!(
            *pc.state(),
            PeerState::Waiting,
            "the third console must wait, not join a cable that has two ends"
        );

        // ...and a fourth pairs with it.
        std::env::set_var("GB_LINK_DIR", &dir);
        let mut pd = LinkPeer::discover("CONSOLE D", Some(Path::new("/tmp/d.sav")));
        let mut gd = Gameboy::new(link_loop_rom(0x44, 0x80), None);
        for _ in 0..4 {
            pc.run_frame(&mut gc);
            pd.run_frame(&mut gd);
        }
        assert!(
            pc.state().is_paired(),
            "third and fourth should pair: {:?}",
            pc.state()
        );
        assert!(pd.state().is_paired());
    }

    /// Two instances of one cartridge write the same `.sav`. The cable works;
    /// the player has to be told, because both consoles autosaving over each
    /// other is how a real save gets destroyed.
    #[test]
    fn two_instances_of_the_same_cartridge_are_flagged() {
        let _guard = env_lock();
        let dir = scratch("samesave");
        std::env::set_var("GB_LINK_DIR", &dir);
        let same = Path::new("/tmp/blue.sav");
        let mut pa = LinkPeer::discover("POKEMON BLUE", Some(same));
        let mut pb = LinkPeer::discover("POKEMON BLUE", Some(same));
        let mut ga = Gameboy::new(link_loop_rom(0x11, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x22, 0x80), None);
        run((&mut pa, &mut ga), (&mut pb, &mut gb), 3);
        assert!(pa.same_save_file());
        assert!(pb.same_save_file());
    }

    /// A peer that says goodbye is not a crash: unplug, do not poison the save.
    #[test]
    fn a_clean_goodbye_unplugs_without_faulting() {
        let _guard = env_lock();
        let dir = scratch("bye");
        let (mut pa, mut pb) = pair(&dir);
        let mut ga = Gameboy::new(link_loop_rom(0x11, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x22, 0x80), None);
        run((&mut pa, &mut ga), (&mut pb, &mut gb), 3);
        assert!(pa.state().is_paired());

        pb.close();
        for _ in 0..3 {
            pa.run_frame(&mut ga);
        }
        assert_eq!(*pa.state(), PeerState::Unplugged);
        assert!(
            ga.fault().is_none(),
            "a peer quitting cleanly must not cost the survivor its save"
        );
    }

    /// A peer that vanishes while the cable is carrying bytes is a crash, and
    /// the survivor is the one holding half a trade.
    #[test]
    fn a_peer_that_vanishes_mid_exchange_faults_the_survivor() {
        let _guard = env_lock();
        let dir = scratch("vanish");
        let (mut pa, mut pb) = pair(&dir);
        let mut ga = Gameboy::new(link_loop_rom(0x11, 0x81), None);
        let mut gb = Gameboy::new(link_loop_rom(0x22, 0x80), None);
        run((&mut pa, &mut ga), (&mut pb, &mut gb), 3);
        assert!(pa.stats().transfers > 0, "the cable must have been in use");

        // No `close`: the socket just dies, as it would if the process were
        // killed. `Drop` on `UnixStream` closes the fd, which the survivor sees
        // as end-of-stream — the same thing a `kill -9` produces.
        drop(pb);
        for _ in 0..3 {
            pa.run_frame(&mut ga);
        }
        assert_eq!(*pa.state(), PeerState::Unplugged);
        assert!(
            ga.fault().is_some(),
            "an active cable disappearing must stop the survivor writing its save"
        );
    }

    /// Switched off is switched off: no socket, no rendezvous entry, no cost.
    #[test]
    fn off_is_off() {
        let _guard = env_lock();
        let dir = scratch("off");
        std::env::set_var("GB_LINK_DIR", &dir);
        let mut p = LinkPeer::off();
        let mut gb = Gameboy::new(link_loop_rom(0x5A, 0x81), None);
        for _ in 0..2 {
            p.run_frame(&mut gb);
        }
        assert_eq!(*p.state(), PeerState::Off);
        assert_eq!(p.stats(), LinkStats::default());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "a switched-off cable must not advertise itself"
        );
    }

    /// A socket left behind by a killed instance must not make the next one
    /// think it has a partner. `Drop` is not a shutdown hook in this codebase.
    #[test]
    fn a_stale_socket_is_swept_rather_than_believed() {
        let _guard = env_lock();
        let dir = scratch("stale");
        std::env::set_var("GB_LINK_DIR", &dir);
        let stale = dir.join("999999.sock");
        {
            let l = UnixListener::bind(&stale).unwrap();
            drop(l);
        }
        assert!(stale.exists());
        let p = LinkPeer::discover("SURVIVOR", None);
        assert_eq!(*p.state(), PeerState::Waiting);
        assert!(!stale.exists(), "the dead instance's socket should be gone");
    }
}

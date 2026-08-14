//! The link port when the far end is **not** another core in this process.
//!
//! [`LinkCable`](super::LinkCable) joins two [`Gameboy`]s that share a loop and
//! a clock, so it can never drop a byte and never has to wait. Everything else
//! that could be on the other end of the wire — a socket to another emulator, a
//! USB adapter with a real console behind it — has the opposite problem: the far
//! end runs on its own clock, answers when it feels like it, and can go away.
//!
//! This module is the seam for those. [`LinkTransport`] is what such a far end
//! has to be able to do; [`TransportLink`] is the driver that turns it into
//! bytes on an emulated console's link port, and it is the only thing that
//! touches [`Gameboy`]'s wire.
//!
//! # The one rule a transport must obey: never block
//!
//! Every method here returns immediately. This is not politeness — it is the
//! repo's most expensive lesson, learned twice. The cpal audio player once
//! waited for the sound card from inside `Cpu::do_cycle` and cost **17.9 ms of
//! a 16.74 ms frame** (`docs/audio.md` §4c); a configured video FIFO once
//! blocked in `open(2)` before the terminal UI existed and hung the emulator at
//! startup (`AGENTS.md`, "no media sink may ever block the emulator"). A link
//! partner has exactly the same shape and would make exactly the same mistake.
//!
//! So the *driver* is what waits, not the transport and not the core. When a
//! master transfer is outstanding and the far end has not answered,
//! [`TransportLink`] simply stops paying cycles to the CPU — visible, bounded,
//! counted in [`TransportStats::stalls`], and giving the transport an
//! [`idle`](LinkTransport::idle) call it may spend on its own I/O.
//!
//! # Why the console has to be stalled at all
//!
//! A master's transfer completes on its **own** shift clock in about a
//! millisecond of emulated time whether or not anything answers (Pan Docs,
//! *Serial Data Transfer*). A far end across a socket cannot reliably answer
//! inside that window, so a driver that let the console run would hand it the
//! idle-high `$FF` every single time and no link would ever carry a byte.
//!
//! Stalling is also what BGB's own protocol does — its timestamps exist "so
//! each side can, at the right times, wait for the remote side"
//! (<https://bgb.bircd.org/bgblink.html>). Stopping the CPU is the honest
//! implementation of that: emulated time really has not passed.
//!
//! # And why an *incoming* byte is held rather than refused
//!
//! When the far end clocks us, our console may not have armed `SC = $80` yet —
//! it is a whole console running its own program, and the far end has no idea
//! which instruction it is on. Answering "not listening" immediately would drop
//! that byte, and a dropped byte in the middle of a trade is the dangerous
//! state. So an incoming byte is **held** while the console keeps running, for
//! up to [`SLAVE_ARM_WINDOW`] T-cycles, and only refused when the console
//! plainly is not going to take it.

use super::{Gameboy, LinkRole, FRAME_TICKS};

/// Why a transport stopped working.
///
/// The distinction is load-bearing, and it is the same one the rest of this
/// repo draws for an absent capability: **an unplugged cable is not a broken
/// one**. A link that never came up costs the player the link and nothing else;
/// a link that came up and then broke may have lost a byte mid-exchange, which
/// is exactly when the surviving console's save is the dangerous one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    /// The link could never be established — nothing listening, a refused
    /// connection, a bad address. The console is untouched.
    Setup(String),
    /// The link was live and has broken. Bytes may have gone missing in the
    /// middle of an exchange, so the console is faulted: its battery save is
    /// poisoned for the rest of the session, exactly as
    /// [`LinkCable`](super::LinkCable) does when one of two local consoles
    /// crashes.
    Broken(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TransportError::Setup(m) => write!(f, "{m}"),
            TransportError::Broken(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for TransportError {}

/// What the far end shifted back for a transfer our console clocked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MasterReply {
    /// It was armed, and this is the byte that was in its shift register.
    Byte(u8),
    /// It was not armed. The master shifts in `$FF` — an idle-high data line —
    /// which is what a real cable with nothing listening on it gives.
    Idle,
}

/// A far end of the link port that runs on its own clock.
///
/// Implementors: [`crate::bgblink::BgbLink`] (BGB's TCP link protocol, and
/// through it every emulator that speaks it). Everything here must return
/// immediately — see the module docs.
pub trait LinkTransport {
    /// One short line for a status row: what this is and who it is talking to.
    fn describe(&self) -> String;

    /// Pump I/O. Called at least once per driver tick, including while the
    /// driver is stalled. Must not block.
    fn poll(&mut self) -> Result<(), TransportError>;

    /// Emulated time has advanced by `t_cycles`. A transport that carries
    /// timestamps derives them from this rather than from the wall clock, so a
    /// stalled console really does stop the clock it reports.
    fn advance(&mut self, t_cycles: u32) {
        let _ = t_cycles;
    }

    /// Whether the far end is connected and past its handshake. Before this is
    /// true the driver behaves as an unplugged cable.
    fn live(&self) -> bool;

    /// Our console has begun an internally-clocked transfer, putting `out` on
    /// the wire with `control` in `SC`. Called exactly once per transfer.
    fn send_master(&mut self, out: u8, control: u8);

    /// The far end's answer to the outstanding master transfer, when it
    /// arrives. A transport that decides the far end is never going to answer
    /// returns [`MasterReply::Idle`] rather than leaving the driver stalled.
    fn take_master_reply(&mut self) -> Option<MasterReply>;

    /// A byte the far end has clocked to us as master, if one is waiting.
    ///
    /// At most one byte may be outstanding: the driver answers each with
    /// [`answer_incoming`](LinkTransport::answer_incoming) before asking for
    /// the next, so the two byte streams cannot get out of order.
    fn take_incoming(&mut self) -> Option<u8>;

    /// Answer the byte handed over by [`take_incoming`](LinkTransport::take_incoming):
    /// `Some(b)` if our console was armed as a slave and shifted `b` back,
    /// `None` if it was not armed at all.
    fn answer_incoming(&mut self, out: Option<u8>);

    /// The driver is holding the console still and has nothing else to do. A
    /// transport may spend a *short, bounded* moment here on its own I/O — a
    /// yield, a sub-millisecond sleep — and must return.
    fn idle(&mut self) {}

    /// Say goodbye to the far end, so it can tell an orderly shutdown from a
    /// crash. Called from wherever the frontend already flushes the battery —
    /// **not** from `Drop`, which this repo has twice been bitten by relying on
    /// (`src/mbc/battery.rs`).
    ///
    /// It matters because the two cases have opposite consequences for the
    /// *other* console's save: an unplugged cable leaves it writing normally, a
    /// broken one stops it.
    fn disconnect(&mut self) {}
}

/// How long an incoming byte is held while the console is given a chance to arm
/// `SC = $80`, in T-cycles. Two frames: Gen 1's link handler arms and re-arms
/// several times a frame, so anything that is going to listen has listened
/// long before this expires, and a console that is busy elsewhere is not made
/// to wait for ever.
pub const SLAVE_ARM_WINDOW: u32 = 2 * FRAME_TICKS;

/// A backstop on the stall loop so a transport that never answers and never
/// times out cannot hang the emulator. Each iteration costs one
/// [`LinkTransport::idle`] call, so this is a count of polls, not of seconds —
/// deliberately, because a stalled console's clock has stopped and there is no
/// emulated time to measure with.
const MAX_STALL_POLLS: u32 = 20_000;

/// Counts describing what a transport-backed wire has carried.
///
/// Deliberately a different set from [`LinkStats`](super::LinkStats): a cable
/// between two local cores cannot time out, cannot miss a byte because it was
/// not ready, and never has to stop the CPU. Reporting those as zero would
/// suggest they had been measured.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportStats {
    /// Byte exchanges both ends agreed on.
    pub transfers: u64,
    /// Transfers where our console was master and the far end was not armed,
    /// so `$FF` was shifted in. **Normal** — it is what a real cable does
    /// whenever the other side is not listening, and Gen 1's handshake spends
    /// its whole idle time here.
    pub unanswered: u64,
    /// Bytes the far end clocked to us that our console never took, because it
    /// was not armed as a slave within [`SLAVE_ARM_WINDOW`]. A steady climb
    /// here means the two ends have settled into the same role and the link
    /// will never move — the single most useful diagnostic this driver has.
    pub missed: u64,
    /// Master transfers the far end never answered at all. Distinguished from
    /// `unanswered` because an explicit "I am not listening" and a silence are
    /// very different things to debug.
    pub timeouts: u64,
    /// Times the driver stopped paying cycles to the CPU to wait for the far
    /// end. Expect one per transferred byte; that is the design, not a fault.
    pub stalls: u64,
}

/// A console's link port, wired to a [`LinkTransport`].
///
/// The driver does not own the console — it is handed it on every call, the
/// same arrangement [`LinkCable`](super::LinkCable) uses — so a frontend keeps
/// whatever structure suits it.
///
/// ```no_run
/// use terminalgb::gameboy::Gameboy;
/// use terminalgb::link::TransportLink;
/// use terminalgb::bgblink::BgbLink;
///
/// let mut gb = Gameboy::new(std::fs::read("blue.gb").unwrap(), None);
/// let mut link = TransportLink::new(Box::new(BgbLink::connect("127.0.0.1:8765").unwrap()));
/// loop {
///     link.run_frame(&mut gb);
/// }
/// ```
pub struct TransportLink {
    transport: Box<dyn LinkTransport>,
    /// The master transfer we have offered to the far end and not yet had an
    /// answer for.
    outstanding: bool,
    /// The last `link_seq` we offered, so a transfer is never offered twice and
    /// a second identical-looking transfer is never mistaken for it.
    last_seq: Option<u32>,
    /// A byte the far end clocked to us, waiting for our console to arm.
    incoming: Option<Incoming>,
    stats: TransportStats,
    failure: Option<TransportError>,
}

#[derive(Debug, Clone, Copy)]
struct Incoming {
    byte: u8,
    /// T-cycles the console has been given since this byte arrived.
    waited: u32,
}

impl TransportLink {
    pub fn new(transport: Box<dyn LinkTransport>) -> TransportLink {
        TransportLink {
            transport,
            outstanding: false,
            last_seq: None,
            incoming: None,
            stats: TransportStats::default(),
            failure: None,
        }
    }

    /// What the wire has carried so far.
    pub fn stats(&self) -> TransportStats {
        self.stats
    }

    /// One short line describing the far end, for a status row.
    pub fn describe(&self) -> String {
        self.transport.describe()
    }

    /// Whether the far end is connected and past its handshake.
    pub fn live(&self) -> bool {
        self.transport.live()
    }

    /// Why the link stopped working, if it has. A [`TransportError::Broken`]
    /// has also faulted the console.
    pub fn failure(&self) -> Option<&TransportError> {
        self.failure.as_ref()
    }

    /// Unplug the cable politely.
    ///
    /// Call it wherever the frontend flushes the battery on a clean shutdown.
    /// The far end can then tell "the player quit" from "the emulator died",
    /// and only the second of those needs to cost it its save.
    pub fn disconnect(&mut self) {
        self.transport.disconnect();
    }

    /// Advance the console by one frame's worth of cycles, servicing the wire
    /// between every instruction and stalling whenever the far end owes us an
    /// answer.
    ///
    /// Returns the T-cycles actually run, which is less than [`FRAME_TICKS`]
    /// when the far end held us up — a frontend can use that to tell the
    /// player the link, not the emulator, is what is slow.
    ///
    /// Exactly like [`LinkCable::run_frame`](super::LinkCable::run_frame), a
    /// fixed cycle budget is paid rather than running to the PPU's own frame
    /// boundary: a console that stops on its own boundary is a console standing
    /// still, and standing still is precisely when a link drops a byte.
    pub fn run_frame(&mut self, gb: &mut Gameboy) -> u32 {
        gb.begin_frame();

        let mut ticks = 0u32;
        let mut stall_polls = 0u32;
        let mut stalling = false;
        while ticks < FRAME_TICKS {
            self.pump(gb, 0);

            if self.outstanding && self.failure.is_none() {
                if !stalling {
                    stalling = true;
                    self.stats.stalls += 1;
                }
                stall_polls += 1;
                if stall_polls >= MAX_STALL_POLLS {
                    // A transport that neither answers nor gives up. Hand the
                    // console the idle line hardware would and carry on: a
                    // hung emulator is a worse failure than a lost byte.
                    self.give_up_on_master(gb);
                    stall_polls = 0;
                    stalling = false;
                    continue;
                }
                self.transport.idle();
                continue;
            }
            stall_polls = 0;
            stalling = false;

            let t = gb.step_instruction();
            ticks += t;
            self.transport.advance(t);
            self.pump(gb, t);
        }

        gb.end_frame();
        ticks
    }

    /// Look at the console's link port and at the transport, and move the wire
    /// on by whatever it owes. `elapsed` is the T-cycles the console has been
    /// given since the last call.
    ///
    /// Cheap when nothing is happening, which is almost always: a couple of
    /// field reads and one transport poll.
    pub fn pump(&mut self, gb: &mut Gameboy, elapsed: u32) {
        if self.failure.is_some() {
            return;
        }
        if let Err(e) = self.transport.poll() {
            self.fail(gb, e);
            return;
        }

        self.service_incoming(gb, elapsed);
        self.service_master(gb);

        // Only ever one byte in the air: the next is not taken until this one
        // has been answered, so the two streams cannot get out of order.
        if self.incoming.is_none() {
            if let Some(byte) = self.transport.take_incoming() {
                self.incoming = Some(Incoming { byte, waited: 0 });
            }
        }
    }

    /// A byte the far end clocked to us: give it to the console if it is
    /// listening, hold it a little while if it might be about to, refuse it if
    /// it plainly is not.
    fn service_incoming(&mut self, gb: &mut Gameboy, elapsed: u32) {
        let Some(inc) = self.incoming.as_mut() else {
            return;
        };
        inc.waited = inc.waited.saturating_add(elapsed);
        let byte = inc.byte;
        let waited = inc.waited;

        match gb.link_role() {
            LinkRole::Slave => {
                // `link_out` first: clocking the slave replaces `SB` with the
                // byte that just arrived, and what the far end is owed is what
                // was in the register before that.
                let ours = gb.link_out();
                gb.link_clock_slave(byte);
                self.transport.answer_incoming(Some(ours));
                self.stats.transfers += 1;
                self.incoming = None;
            }
            // We are driving a transfer of our own. Two masters at once is a
            // real hardware situation and both sides shift in an idle line;
            // BGB spells that "not doing a passive transfer".
            LinkRole::Master => {
                self.transport.answer_incoming(None);
                self.stats.missed += 1;
                self.incoming = None;
            }
            LinkRole::Idle if waited >= SLAVE_ARM_WINDOW => {
                self.transport.answer_incoming(None);
                self.stats.missed += 1;
                self.incoming = None;
            }
            // Idle, but not for long enough to give up on. Keep running the
            // console; it may be one instruction away from arming.
            LinkRole::Idle => {}
        }
    }

    /// Our console's own master transfers: offer each one exactly once, and
    /// land the answer when it comes.
    fn service_master(&mut self, gb: &mut Gameboy) {
        if self.outstanding {
            match self.transport.take_master_reply() {
                Some(MasterReply::Byte(b)) => {
                    gb.link_answer_master(b);
                    self.stats.transfers += 1;
                    self.outstanding = false;
                }
                Some(MasterReply::Idle) => {
                    gb.link_answer_master(0xFF);
                    self.stats.unanswered += 1;
                    self.outstanding = false;
                }
                None => return,
            }
        }

        if gb.link_role() == LinkRole::Master {
            let seq = gb.link_seq();
            if self.last_seq != Some(seq) {
                self.last_seq = Some(seq);
                let byte = gb.link_out();
                // `SC = $81`: transfer started, internal clock. The BGB
                // protocol carries the register value itself.
                self.transport.send_master(byte, 0x81);
                self.outstanding = true;
            }
        }
    }

    fn give_up_on_master(&mut self, gb: &mut Gameboy) {
        gb.link_answer_master(0xFF);
        self.stats.timeouts += 1;
        self.outstanding = false;
    }

    /// Record a transport failure, and — for a link that had gone live — take
    /// the console's right to write its battery save away.
    ///
    /// The reasoning is [`LinkCable::propagate_fault`](super::LinkCable)'s: a
    /// Gen 1 trade removes a Pokémon from one party before adding it to the
    /// other, so a link that breaks between those two writes leaves whichever
    /// side is still running holding a save the player would rather not keep.
    /// We cannot reach the far end's save; we can refuse to write our own.
    fn fail(&mut self, gb: &mut Gameboy, e: TransportError) {
        if let TransportError::Broken(why) = &e {
            gb.report_fault(&format!("the link partner went away mid-exchange: {why}"));
        }
        // An unanswered transfer must not be left hanging: the console is
        // about to run on without a cable, and hardware would give it an idle
        // line.
        if self.outstanding {
            gb.link_answer_master(0xFF);
            self.stats.unanswered += 1;
            self.outstanding = false;
        }
        self.incoming = None;
        self.failure = Some(e);
    }
}

impl std::fmt::Debug for TransportLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TransportLink")
            .field("transport", &self.transport.describe())
            .field("live", &self.transport.live())
            .field("stats", &self.stats)
            .field("failure", &self.failure)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// A transport whose far end is a script: it answers each master transfer
    /// with the next byte of `answers`, and clocks us with each byte of
    /// `clocks` as soon as the previous one has been answered.
    ///
    /// Deliberately *not* a socket. Everything the driver decides — when to
    /// stall, when to hold an incoming byte, when to give up on one — is
    /// decided here and has to be testable without a network.
    #[derive(Default)]
    struct Script {
        live: bool,
        /// Replies to our master transfers, in order.
        answers: Vec<MasterReply>,
        /// What the far end sent us as master, and what we answered with.
        clocks: Vec<u8>,
        sent: Vec<u8>,
        answered: Vec<Option<u8>>,
        /// Set when a master transfer is outstanding; cleared when answered.
        pending_master: bool,
        /// `Some` while a byte we handed over has not been answered.
        out_for_answer: bool,
        /// How many `idle` calls the driver has made.
        idles: u32,
        /// Fail on the next poll.
        fail: Option<TransportError>,
        /// Never answer a master transfer.
        deaf: bool,
    }

    type Shared = Rc<RefCell<Script>>;

    struct Handle(Shared);

    impl LinkTransport for Handle {
        fn describe(&self) -> String {
            "script".to_string()
        }
        fn poll(&mut self) -> Result<(), TransportError> {
            match self.0.borrow_mut().fail.take() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }
        fn live(&self) -> bool {
            self.0.borrow().live
        }
        fn send_master(&mut self, out: u8, _control: u8) {
            let mut s = self.0.borrow_mut();
            s.sent.push(out);
            s.pending_master = true;
        }
        fn take_master_reply(&mut self) -> Option<MasterReply> {
            let mut s = self.0.borrow_mut();
            if !s.pending_master || s.deaf {
                return None;
            }
            let r = if s.answers.is_empty() {
                MasterReply::Idle
            } else {
                s.answers.remove(0)
            };
            s.pending_master = false;
            Some(r)
        }
        fn take_incoming(&mut self) -> Option<u8> {
            let mut s = self.0.borrow_mut();
            if s.out_for_answer || s.clocks.is_empty() {
                return None;
            }
            s.out_for_answer = true;
            Some(s.clocks.remove(0))
        }
        fn answer_incoming(&mut self, out: Option<u8>) {
            let mut s = self.0.borrow_mut();
            s.answered.push(out);
            s.out_for_answer = false;
        }
        fn idle(&mut self) {
            self.0.borrow_mut().idles += 1;
        }
    }

    /// The same shape of link loop `super::tests` uses: load `SB`, arm `SC`,
    /// wait for the start flag to clear, park the received byte at `$C000`.
    fn link_loop_rom(send: u8, clock: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x119].copy_from_slice(&[
            0x3E, send, 0xEA, 0x01, 0xFF, 0x3E, clock, 0xEA, 0x02, 0xFF, 0xFA, 0x02,
            0xFF, 0xCB, 0x7F, 0x20, 0xF9, 0xFA, 0x01, 0xFF, 0xEA, 0x00, 0xC0, 0x18, 0xE5,
        ]);
        rom
    }

    fn script(s: Script) -> (Shared, TransportLink) {
        let shared = Rc::new(RefCell::new(s));
        let link = TransportLink::new(Box::new(Handle(shared.clone())));
        (shared, link)
    }

    /// The wiring, in one claim: a console driving the clock gets its byte to
    /// the transport and the transport's answer back into `SB`.
    #[test]
    fn a_master_transfer_reaches_the_transport_and_the_answer_lands() {
        let (shared, mut link) = script(Script {
            live: true,
            answers: vec![MasterReply::Byte(0x3C); 64],
            ..Default::default()
        });
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);

        link.run_frame(&mut gb);

        assert_eq!(
            shared.borrow().sent.first().copied(),
            Some(0xA5),
            "the byte the console put on the wire should reach the transport"
        );
        assert_eq!(
            gb.peek(0xC000),
            0x3C,
            "and the transport's answer should land in the console's SB"
        );
        assert!(link.stats().transfers > 0);
        assert_eq!(link.stats().unanswered, 0);
    }

    /// The whole reason the driver exists: a far end that answers slowly must
    /// change the wall clock and **nothing else**. A console left running would
    /// have shifted in `$FF` long before the answer arrived.
    #[test]
    fn a_slow_far_end_costs_time_and_never_a_byte() {
        // Answer only after the driver has idled ten times — i.e. long after
        // the console's own ~4096-cycle shift clock would have run out and
        // handed the game `$FF`, had the driver let it run.
        struct Slow {
            sent: Vec<u8>,
            idles: u32,
        }
        impl LinkTransport for Slow {
            fn describe(&self) -> String {
                "slow".into()
            }
            fn poll(&mut self) -> Result<(), TransportError> {
                Ok(())
            }
            fn live(&self) -> bool {
                true
            }
            fn send_master(&mut self, out: u8, _c: u8) {
                self.sent.push(out);
                self.idles = 0;
            }
            fn take_master_reply(&mut self) -> Option<MasterReply> {
                (self.idles >= 10).then_some(MasterReply::Byte(0x77))
            }
            fn take_incoming(&mut self) -> Option<u8> {
                None
            }
            fn answer_incoming(&mut self, _out: Option<u8>) {}
            fn idle(&mut self) {
                self.idles += 1;
            }
        }
        let mut slow = TransportLink::new(Box::new(Slow {
            sent: Vec::new(),
            idles: 0,
        }));
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        let ticks = slow.run_frame(&mut gb);

        assert_eq!(gb.peek(0xC000), 0x77);
        assert!(
            slow.stats().stalls > 0,
            "the driver should have held the console still"
        );
        assert_eq!(slow.stats().unanswered, 0);
        assert!(
            ticks >= FRAME_TICKS,
            "the frame's cycles are still all paid — only the wall clock moved"
        );
    }

    /// A far end that is not armed is not a failure: hardware gives the master
    /// an idle-high line, and Gen 1's handshake spends its whole idle time
    /// there. Nothing may be faulted.
    #[test]
    fn a_far_end_that_is_not_armed_gives_the_console_an_idle_line() {
        let (_shared, mut link) = script(Script {
            live: true,
            answers: vec![MasterReply::Idle; 8],
            ..Default::default()
        });
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        link.run_frame(&mut gb);

        assert_eq!(gb.peek(0xC000), 0xFF);
        assert!(link.stats().unanswered > 0);
        assert_eq!(link.stats().transfers, 0);
        assert!(
            gb.fault().is_none(),
            "an unanswered transfer is not a fault"
        );
    }

    /// The slave half, which is the half a lone emulator never needed: the far
    /// end drives the clock and our console shifts its own byte back.
    #[test]
    fn the_far_end_can_clock_our_console_as_a_slave() {
        let (shared, mut link) = script(Script {
            live: true,
            clocks: vec![0x11, 0x22, 0x33],
            ..Default::default()
        });
        let mut gb = Gameboy::new(link_loop_rom(0x5A, 0x80), None);
        link.run_frame(&mut gb);

        assert_eq!(
            gb.peek(0xC000),
            0x33,
            "the console's loop should have taken all three, most recent last"
        );
        assert_eq!(
            shared.borrow().answered,
            vec![Some(0x5A), Some(0x5A), Some(0x5A)],
            "and its own byte should have gone back for each of them"
        );
        assert_eq!(link.stats().transfers, 3);
        assert_eq!(link.stats().missed, 0);
    }

    /// An incoming byte is **held** while the console might still arm. Prove
    /// the hold really happens, rather than the byte merely being lucky: a
    /// console that spends its first instructions setting up must still get it.
    #[test]
    fn an_incoming_byte_is_held_until_the_console_arms() {
        let (shared, mut link) = script(Script {
            live: true,
            clocks: vec![0x99],
            ..Default::default()
        });
        // The ROM arms `SC = $80` several instructions in; the byte is offered
        // to the driver on the very first pump, before that has happened.
        let mut gb = Gameboy::new(link_loop_rom(0x42, 0x80), None);
        link.pump(&mut gb, 0);
        link.pump(&mut gb, 0);
        assert!(
            shared.borrow().answered.is_empty(),
            "the byte must not be refused while the console is still starting up"
        );

        link.run_frame(&mut gb);
        assert_eq!(gb.peek(0xC000), 0x99);
        assert_eq!(link.stats().missed, 0);
    }

    /// ...and it is not held for ever. A console that is never going to listen
    /// must be told so, and the refusal counted where somebody can see it.
    #[test]
    fn a_console_that_never_arms_is_reported_not_hidden() {
        let (shared, mut link) = script(Script {
            live: true,
            clocks: vec![0x99],
            ..Default::default()
        });
        // A ROM that only ever spins: `JR -2` at $0100.
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x102].copy_from_slice(&[0x18, 0xFE]);
        let mut gb = Gameboy::new(rom, None);

        // Two frames is exactly `SLAVE_ARM_WINDOW`; run three to be past it.
        for _ in 0..3 {
            link.run_frame(&mut gb);
        }
        assert_eq!(
            shared.borrow().answered.first().copied(),
            Some(None),
            "the far end must be told we were not listening"
        );
        assert_eq!(link.stats().missed, 1);
        assert!(gb.fault().is_none(), "not listening is not a crash");
    }

    /// The fail-safe, proven rather than asserted: a link that breaks after it
    /// was live stops the save being written.
    #[test]
    fn a_broken_link_stops_the_save_being_written() {
        let (shared, mut link) = script(Script {
            live: true,
            ..Default::default()
        });
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        shared.borrow_mut().fail = Some(TransportError::Broken("peer closed".into()));

        link.run_frame(&mut gb);

        assert!(gb.fault().is_some(), "a broken link must fault the console");
        assert!(matches!(link.failure(), Some(TransportError::Broken(_))));
    }

    /// ...and a link that never came up does **not**. An unplugged cable costs
    /// you the cable, the same rule every other absent capability follows here.
    #[test]
    fn a_link_that_never_connected_is_not_a_fault() {
        let (shared, mut link) = script(Script::default());
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        shared.borrow_mut().fail =
            Some(TransportError::Setup("connection refused".into()));

        link.run_frame(&mut gb);

        assert!(gb.fault().is_none());
        assert!(matches!(link.failure(), Some(TransportError::Setup(_))));
    }

    /// A transport that answers nothing and never gives up must not hang the
    /// emulator. The console gets the idle line hardware would give it and the
    /// give-up is counted separately from an honest "not listening".
    #[test]
    fn a_transport_that_never_answers_cannot_hang_the_emulator() {
        let (shared, mut link) = script(Script {
            live: true,
            deaf: true,
            ..Default::default()
        });
        let mut gb = Gameboy::new(link_loop_rom(0xA5, 0x81), None);
        link.run_frame(&mut gb);

        assert!(link.stats().timeouts > 0);
        assert_eq!(gb.peek(0xC000), 0xFF);
        assert!(
            shared.borrow().idles > 0,
            "the transport should have been given its idle time, not spun on"
        );
    }
}

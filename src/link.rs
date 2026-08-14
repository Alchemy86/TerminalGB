//! The link cable: **two whole emulated consoles joined by one wire**.
//!
//! This is the other way to be the far end of a Game Boy's link port. The first
//! is [`plugins/gen1-link`](https://github.com/Alchemy86/gameboy/tree/main/plugins/gen1-link),
//! which *impersonates* a second console by speaking Pokémon Red/Blue's trade
//! protocol from Rust. It works, and it is the only option when you have one
//! cartridge. But it can only ever know the protocols someone wrote down.
//!
//! A cable between two real cores knows none of them and needs none of them.
//! It relays bytes. Whatever the two games agree on — a Gen 1 trade, a Gen 1
//! link battle, the Gen 1 ↔ Gen 2 Time Capsule, Tetris, a game nobody has
//! looked at — works because both ends are the genuine article.
//!
//! # The hardware being modelled
//!
//! From Pan Docs, *Serial Data Transfer (Link Cable)*, and the register decode
//! in [`crate::mmu::serial`]:
//!
//! * `SB` (`$FF01`) is an **8-bit shift register**, not a mailbox. Both
//!   consoles present their `SB` to the wire simultaneously; after eight clock
//!   edges each holds what the other started with. There is no direction: a
//!   transfer is always an *exchange*.
//! * `SC` (`$FF02`) bit 7 is the transfer-start flag and bit 0 selects the
//!   shift clock — **1 = internal** (this console drives it, "master") and
//!   **0 = external** (the other console does, "slave").
//! * A master's transfer runs on its own clock — 8192 Hz on a DMG, so eight
//!   edges is ~1 ms — and **completes whether or not anything answers**, with
//!   `$FF` shifted in, because an unplugged cable's data line idles high.
//! * A slave's transfer **never** completes on its own. With no cable it waits
//!   for ever. That is why a single-console emulator has never needed to model
//!   the slave side at all, and why adding it was the substantive part of this
//!   work.
//! * When the eighth edge lands, *both* consoles clear `SC` bit 7 and request
//!   the serial interrupt (`IF` bit 3). Agreeing on that instant is the whole
//!   of keeping a link in sync.
//!
//! # How this cable stays in sync
//!
//! **One process, one loop, two cores stepped alternately by cycle count.**
//! [`LinkCable::run_frame`] pays each core one instruction at a time, always
//! feeding whichever is behind, so the two never differ by more than a single
//! instruction (~24 T-cycles) — against 4096 T-cycles per byte transferred.
//! There is no IPC, no wall clock and no negotiation, so there is nothing to
//! drift.
//!
//! The wire is serviced between every instruction:
//!
//! 1. One console starts a master transfer (`SC = $81`). The cable sees
//!    [`LinkRole::Master`] and looks at the other end.
//! 2. If the other end is [`LinkRole::Slave`], the two are **paired**: each
//!    console's `SB` is latched, and the master is told what it will shift in.
//!    The master still runs its full eight edges, so the *game* sees hardware
//!    timing rather than an instantaneous exchange.
//! 3. When the master's shift register runs out, the cable clocks the slave —
//!    landing the master's byte in the slave's `SB`, clearing its start flag
//!    and requesting its serial interrupt. Both consoles now agree.
//!
//! Cases the cable deliberately does **not** paper over, because hardware does
//! not either: two masters at once (both shift in `$FF`, as two consoles
//! fighting over the clock line would), and two slaves at once (nothing
//! happens, for ever — every link protocol's handshake exists to resolve
//! exactly this).
//!
//! # Failing safe
//!
//! The guard here is **fault propagation**: if one console faults, the other is
//! faulted too, which poisons both batteries so neither save is written again
//! this session ([`Gameboy::report_fault`], the same mechanism the crash
//! detector uses). Half an exchange is exactly the situation where the
//! *surviving* console's save is the dangerous one — a Gen 1 trade removes a
//! Pokémon from one party and adds it to the other, and a crash between those
//! two writes leaves the other side holding a save the player would rather not
//! keep.
//!
//! ## A torn byte keeps its data
//!
//! When the other console arms `SC = $80` and then rewrites `SC` before the
//! eighth edge, the exchange is *torn*. The master keeps the byte that was
//! latched when the two were paired; only the slave half is abandoned, which is
//! what the slave asked for by rewriting the register.
//!
//! That is a deliberate reading of an ambiguous case, and it is the reading Gen
//! 1 needs. `.establishConnectionLoop` offers `SB = $02` behind `SC = $80` and
//! replaces it with `SB = $01` / `SC = $81` about a hundred T-cycles later. The
//! **only** way two consoles agree on who drives the clock is for one of them
//! to read `$02` out of that window while the other reads back the `$01` it
//! sent — so a cable that voids a torn byte can never connect two Game Boys at
//! all. Taking the byte back was tried here and did exactly that.
//!
//! What the cable deliberately does **not** do is treat a lost byte as
//! corruption, and that was a real bug on the way here. The first version
//! faulted both machines the moment a slave disarmed mid-transfer — and then
//! faulted them nine hundred frames into a Blue-to-Blue trade, before the two
//! consoles had finished saying hello. Gen 1's `.establishConnectionLoop`
//! (`engine/link/cable_club_npc.asm`) arms `SC = $80` and overwrites it with
//! `SC = $81` six instructions later, **every frame**, for as long as it takes
//! to find a partner: tearing a byte is how that handshake *works*. So a torn
//! byte gives the master `$FF` — a data line nobody is driving — and is
//! counted, not punished. The games' own protocols are the byte-level
//! desync detectors, and Gen 1's is a good one (`Serial_ExchangeBytes` re-sends
//! its first byte until it sees the `$FD` preamble come back).

/// Two *processes* on one machine, joined automatically — see [`peer`].
#[cfg(unix)]
pub mod peer;

use crate::gameboy::{Gameboy, FRAME_TICKS};
use crate::mmu::serial::LinkRole;

/// What the other end was doing while a master transfer ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Partner {
    /// Nothing listening (yet). The master will shift in the idle-high `$FF` it
    /// started with.
    Absent,
    /// Listening, and its byte has been latched into the master.
    Armed,
    /// It *was* listening and stopped before the eighth edge. The master has
    /// been given `$FF` back, because that is what a data line nobody is
    /// driving reads as.
    Torn,
}

/// A master-side transfer the cable is watching from its start to its eighth
/// clock edge.
#[derive(Debug, Clone, Copy)]
struct InFlight {
    /// Which console is driving the clock: `true` for the first of the pair.
    master_is_a: bool,
    /// The master's transfer counter at the moment it was noticed, so a second
    /// transfer that looks identical is never mistaken for this one.
    seq: u32,
    /// The byte the master put on the wire, owed to the slave when the clock
    /// runs out.
    master_out: u8,
    partner: Partner,
}

/// Counts describing what the wire has carried. Cheap to read; a frontend or a
/// harness shows these so a link that is quietly doing nothing is visible.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LinkStats {
    /// Byte exchanges completed with both consoles agreeing.
    pub transfers: u64,
    /// Transfers where a master shifted in `$FF` because the other console was
    /// not armed. This is **normal**: it is what a real cable does whenever one
    /// side is not listening, and Gen 1's handshake spends its whole idle time
    /// here — a completed Blue-to-Blue trade over
    /// [`peer`] logs around 450 of them.
    pub unanswered: u64,
    /// Transfers where the other console armed its port and then disarmed it
    /// before the eighth edge. The master **keeps** the byte that was latched;
    /// only the slave side of the exchange is abandoned, which is what the
    /// slave itself asked for by rewriting `SC`.
    ///
    /// **This is normal too**, and expecting otherwise was a real bug here: Gen
    /// 1's own `.establishConnectionLoop` (`engine/link/cable_club_npc.asm`)
    /// arms `SC = $80` and then overwrites it with `SC = $81` six instructions
    /// later, every frame, for as long as it takes to find a partner. A cable
    /// that treats a torn byte as corruption faults both consoles before they
    /// have finished saying hello.
    pub torn: u64,
}

/// The wire joining two [`Gameboy`]s.
///
/// The cable does not own the consoles — it is handed both on every call — so a
/// frontend, an example or a test keeps whatever arrangement suits it (two
/// panes, one visible and one headless, or neither).
///
/// ```no_run
/// use terminalgb::gameboy::Gameboy;
/// use terminalgb::link::LinkCable;
///
/// let mut a = Gameboy::new(std::fs::read("blue.gb").unwrap(), None);
/// let mut b = Gameboy::new(std::fs::read("blue.gb").unwrap(), None);
/// let mut cable = LinkCable::new();
/// loop {
///     cable.run_frame(&mut a, &mut b);
/// }
/// ```
#[derive(Debug, Default)]
pub struct LinkCable {
    in_flight: Option<InFlight>,
    stats: LinkStats,
    faulted: bool,
}

impl LinkCable {
    pub fn new() -> LinkCable {
        LinkCable::default()
    }

    /// What the wire has carried so far.
    pub fn stats(&self) -> LinkStats {
        self.stats
    }

    /// Tell the cable that the caller is about to hand it its two consoles the
    /// other way round.
    ///
    /// A pairing in flight records *which side* is driving the clock
    /// positionally, so a frontend that swaps the two — the terminal
    /// frontend's `L` key does, because focusing a console is implemented as a
    /// swap so nothing downstream needs to know a second one exists — has to
    /// say so, or a byte gets delivered to the wrong machine.
    pub fn swap_ends(&mut self) {
        if let Some(f) = self.in_flight.as_mut() {
            f.master_is_a = !f.master_is_a;
        }
    }

    /// Whether the cable has seen a byte go missing. Once true, both consoles
    /// have been faulted and neither will write its battery save again.
    pub fn desynced(&self) -> bool {
        self.faulted
    }

    /// Advance both consoles by one frame's worth of cycles, keeping them in
    /// lockstep and servicing the wire between every instruction.
    ///
    /// Exactly [`FRAME_TICKS`] T-cycles are paid to each core, rather than
    /// running each to its own PPU frame boundary as [`Gameboy::frame`] does.
    /// Two consoles cannot both stop on their own boundary without one of them
    /// standing still while the other catches up — and standing still is
    /// precisely when a link drops a byte. Paying a fixed budget keeps them
    /// aligned to the cycle and gives the same 59.7275 Hz pacing.
    pub fn run_frame(&mut self, a: &mut Gameboy, b: &mut Gameboy) {
        a.begin_frame();
        b.begin_frame();

        let (mut ticks_a, mut ticks_b) = (0u32, 0u32);
        while ticks_a < FRAME_TICKS || ticks_b < FRAME_TICKS {
            // Always feed the console that is behind, so the gap between them
            // stays under one instruction.
            if ticks_b >= FRAME_TICKS || (ticks_a <= ticks_b && ticks_a < FRAME_TICKS) {
                ticks_a += a.step_instruction();
            } else {
                ticks_b += b.step_instruction();
            }
            self.service(a, b);
        }

        a.end_frame();
        b.end_frame();
    }

    /// Look at both link ports and move the wire on by whatever it owes.
    ///
    /// Safe to call as often as you like — it is a couple of field reads when
    /// nothing is happening, which is almost always. Call it at least once per
    /// instruction: a transfer lasts 4096 T-cycles, so anything finer than that
    /// is timely, but a caller that only looks once per *frame* would miss up
    /// to sixteen transfers.
    pub fn service(&mut self, a: &mut Gameboy, b: &mut Gameboy) {
        // A console that has crashed must not take its partner's save down with
        // it: half a trade is worse on the survivor than no trade at all.
        self.propagate_fault(a, b);

        if let Some(mut f) = self.in_flight {
            let (master, slave) = split(a, b, f.master_is_a);
            let running =
                master.link_role() == LinkRole::Master && master.link_seq() == f.seq;
            let listening = slave.link_role() == LinkRole::Slave;

            if running {
                match (f.partner, listening) {
                    // Somebody started listening. It may have armed after the
                    // master did — it still has the rest of the ~1 ms window —
                    // so latch the exchange now.
                    (Partner::Absent, true) => {
                        let slave_out = slave.link_out();
                        master.link_answer_master(slave_out);
                        f.partner = Partner::Armed;
                    }
                    // It stopped listening mid-transfer. The master keeps what
                    // was latched — see the module docs, "A torn byte keeps
                    // its data": taking it back breaks Gen 1's handshake,
                    // whose whole mechanism is a hundred-cycle window.
                    (Partner::Armed, false) => {
                        f.partner = Partner::Torn;
                        self.stats.torn += 1;
                    }
                    _ => {}
                }
                self.in_flight = Some(f);
                return;
            }

            // The eighth edge has landed. Both consoles must agree *now*.
            match f.partner {
                Partner::Armed if listening => {
                    slave.link_clock_slave(f.master_out);
                    self.stats.transfers += 1;
                }
                // It disarmed in the very last instruction, too late for the
                // master to be told. One instruction in 4096 T-cycles.
                Partner::Armed => self.stats.torn += 1,
                Partner::Absent => self.stats.unanswered += 1,
                Partner::Torn => {}
            }
            self.in_flight = None;
        }

        // Notice a fresh master transfer, and pair it straight away if the
        // other console is already listening.
        let (role_a, role_b) = (a.link_role(), b.link_role());
        let master_is_a = match (role_a, role_b) {
            (LinkRole::Master, _) => true,
            (_, LinkRole::Master) => false,
            // Nothing armed, or both listening — the deadlock every link
            // protocol's handshake exists to break. A real cable does nothing
            // about either.
            _ => return,
        };
        let (master, slave) = split(a, b, master_is_a);
        let mut f = InFlight {
            master_is_a,
            seq: master.link_seq(),
            master_out: master.link_out(),
            partner: Partner::Absent,
        };
        if slave.link_role() == LinkRole::Slave {
            let slave_out = slave.link_out();
            master.link_answer_master(slave_out);
            f.partner = Partner::Armed;
        }
        self.in_flight = Some(f);
    }

    /// If one console has faulted, fault the other.
    ///
    /// A fault takes away the right to write the battery save
    /// (`MemoryManagementUnit::report_fault`). Half a link exchange is exactly
    /// the situation where the *surviving* console's save is the dangerous one:
    /// a Gen 1 trade removes a Pokémon from one party and adds it to the other,
    /// and a crash between those two writes leaves whichever side is still
    /// running holding a save the player would rather not keep.
    fn propagate_fault(&mut self, a: &mut Gameboy, b: &mut Gameboy) {
        if self.faulted {
            return;
        }
        let (fa, fb) = (a.fault().is_some(), b.fault().is_some());
        if fa == fb {
            return;
        }
        self.faulted = true;
        const WHY: &str = "the console on the other end of the link cable faulted \
                           mid-exchange";
        if fa {
            b.report_fault(WHY);
        } else {
            a.report_fault(WHY);
        }
    }
}

/// Borrow the pair as (master, slave) without upsetting the borrow checker.
fn split<'g>(
    a: &'g mut Gameboy,
    b: &'g mut Gameboy,
    master_is_a: bool,
) -> (&'g mut Gameboy, &'g mut Gameboy) {
    if master_is_a {
        (a, b)
    } else {
        (b, a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM that exchanges `send` for ever, parking each byte it receives at
    /// `$C000`. `clock` is the `SC` value: `$81` drives the shift clock
    /// (master), `$80` follows the other console's (slave). Both sides of a
    /// real link run the same shape of loop — load `SB`, arm `SC`, wait for the
    /// start flag to clear, read `SB` — which is why one routine serves for
    /// both.
    fn link_loop_rom(send: u8, clock: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x119].copy_from_slice(&[
            0x3E, send, // LD A, send
            0xEA, 0x01, 0xFF, // LD ($FF01), A  ; SB = the byte we put on the wire
            0x3E, clock, // LD A, clock
            0xEA, 0x02, 0xFF, // LD ($FF02), A  ; SC = start + clock source
            0xFA, 0x02, 0xFF, // LD A, ($FF02)  ; poll the transfer-start flag
            0xCB, 0x7F, // BIT 7, A
            0x20, 0xF9, // JR NZ, -7      ; ...until the transfer finishes
            0xFA, 0x01, 0xFF, // LD A, ($FF01)  ; the byte the other console sent
            0xEA, 0x00, 0xC0, // LD ($C000), A
            0x18, 0xE5, // JR -27         ; and round again
        ]);
        rom
    }

    fn master_rom(send: u8) -> Vec<u8> {
        link_loop_rom(send, 0x81)
    }

    fn slave_rom(send: u8) -> Vec<u8> {
        link_loop_rom(send, 0x80)
    }

    fn boot(rom: Vec<u8>) -> Gameboy {
        // `Cpu::new` hands over at `PC = $0100` exactly as a boot ROM does, so
        // these programs start running on their own.
        Gameboy::new(rom, None)
    }

    /// The claim the whole feature rests on: two cores, one wire, and each ends
    /// up holding what the other started with. Not a mock — two real
    /// `Gameboy`s, each running its own program, joined only by the cable.
    #[test]
    fn two_cores_exchange_a_byte_through_the_cable() {
        let mut a = boot(master_rom(0xA5));
        let mut b = boot(slave_rom(0x3C));
        let mut cable = LinkCable::new();

        cable.run_frame(&mut a, &mut b);

        assert_eq!(
            a.peek(0xC000),
            0x3C,
            "the master should shift in what the slave was holding"
        );
        assert_eq!(
            b.peek(0xC000),
            0xA5,
            "and the slave should shift in what the master sent"
        );
        assert!(cable.stats().transfers > 0);
        assert_eq!(cable.stats().torn, 0);
        assert!(!cable.desynced());
    }

    /// The slave side is the part that did not exist before. Prove it is the
    /// cable doing the work and not some accident of the master's own timing:
    /// with no cable serviced, a slave waits for ever.
    #[test]
    fn a_slave_with_no_cable_waits_for_ever() {
        let mut b = boot(slave_rom(0x3C));
        for _ in 0..4 {
            b.frame();
        }
        assert_eq!(
            b.peek(0xFF01),
            0x3C,
            "SB still holds the byte it loaded — nothing has clocked it"
        );
        assert_eq!(
            b.peek(0xFF02) & 0x80,
            0x80,
            "and the transfer-start flag is still set: the transfer never ran"
        );
        assert_ne!(
            b.peek(0xC000),
            0x3C,
            "the program is still spinning in its poll loop, four frames later"
        );
    }

    /// Both consoles must see the transfer *finish*, which means the serial
    /// interrupt is requested on both sides — that is the event every link
    /// protocol's handler hangs off.
    #[test]
    fn completing_a_transfer_interrupts_both_consoles() {
        let mut a = boot(master_rom(0x11));
        let mut b = boot(slave_rom(0x22));
        // The test ROMs never enable interrupts, so `IF` accumulates and can be
        // read after the fact.
        let mut cable = LinkCable::new();
        cable.run_frame(&mut a, &mut b);
        assert_ne!(
            a.peek(0xFF0F) & 0x08,
            0,
            "master requests the serial interrupt"
        );
        assert_ne!(b.peek(0xFF0F) & 0x08, 0, "slave requests it too");
    }

    /// Two consoles both driving the clock is a real hardware situation and the
    /// cable must not invent a byte for it: each sees an idle-high line.
    #[test]
    fn two_masters_shift_in_an_idle_line() {
        let mut a = boot(master_rom(0x11));
        let mut b = boot(master_rom(0x22));
        let mut cable = LinkCable::new();
        cable.run_frame(&mut a, &mut b);
        assert_eq!(a.peek(0xC000), 0xFF);
        assert_eq!(b.peek(0xC000), 0xFF);
        assert_eq!(cable.stats().transfers, 0);
        assert!(!cable.desynced(), "an unanswered transfer is not a desync");
    }

    /// Two consoles both listening is the deadlock every handshake exists to
    /// break. Nothing should happen, and nothing should be reported as wrong.
    #[test]
    fn two_slaves_deadlock_quietly() {
        let mut a = boot(slave_rom(0x11));
        let mut b = boot(slave_rom(0x22));
        let mut cable = LinkCable::new();
        cable.run_frame(&mut a, &mut b);
        assert_eq!(a.peek(0xFF01), 0x11, "neither console was clocked");
        assert_eq!(b.peek(0xFF01), 0x22);
        assert_eq!(
            a.peek(0xFF02) & 0x80,
            0x80,
            "both still waiting to be clocked"
        );
        assert_eq!(b.peek(0xFF02) & 0x80, 0x80);
        assert_eq!(cable.stats(), LinkStats::default());
        assert!(!cable.desynced());
    }

    /// The stream has to stay in step over many transfers, not just one — that
    /// is the difference between a cable and a lucky first byte. A Gen 1 trade
    /// is 644 transfers, so run comfortably past that.
    #[test]
    fn a_long_run_stays_in_step() {
        let mut a = boot(master_rom(0x5A));
        let mut b = boot(slave_rom(0xC3));
        let mut cable = LinkCable::new();
        for _ in 0..60 {
            cable.run_frame(&mut a, &mut b);
            assert_eq!(a.peek(0xC000), 0xC3);
            assert_eq!(b.peek(0xC000), 0x5A);
        }
        assert!(
            cable.stats().transfers > 644,
            "a Gen 1 trade is 644 transfers; this run carried only {}",
            cable.stats().transfers
        );
        assert_eq!(cable.stats().torn, 0);
        assert!(!cable.desynced());
    }

    /// A slave that disarms mid-transfer is **not** corruption — it is what
    /// Gen 1's own handshake does every frame. The master must be handed the
    /// idle line hardware would give it, and nothing may be faulted.
    #[test]
    fn a_slave_that_disarms_mid_transfer_leaves_the_master_an_idle_line() {
        let mut a = boot(master_rom(0x11));
        let mut b = boot(slave_rom(0x22));
        let mut cable = LinkCable::new();

        // Run until the two are paired, then disarm the slave before the
        // master's shift register has run out.
        for _ in 0..80 {
            a.step_instruction();
            b.step_instruction();
            cable.service(&mut a, &mut b);
            if cable.in_flight.is_some_and(|f| f.partner == Partner::Armed) {
                break;
            }
        }
        assert!(
            cable.in_flight.is_some_and(|f| f.partner == Partner::Armed),
            "the two should have paired"
        );
        b.debug_write(0xFF02, 0x00);
        for _ in 0..4000 {
            a.step_instruction();
            cable.service(&mut a, &mut b);
        }

        assert_eq!(cable.stats().torn, 1);
        assert_eq!(cable.stats().transfers, 0);
        assert_eq!(
            a.peek(0xC000),
            0xFF,
            "the master shifts in an idle line, not the byte it had been promised"
        );
        assert!(!cable.desynced(), "a torn byte is normal, not a desync");
        assert!(a.fault().is_none());
        assert!(b.fault().is_none());
    }

    /// A frontend that swaps the two consoles — the terminal frontend focuses
    /// one by swapping, so nothing downstream needs to know a second exists —
    /// must not lose the byte in flight to the wrong machine.
    #[test]
    fn swapping_the_ends_mid_transfer_still_delivers_to_the_right_console() {
        let mut a = boot(master_rom(0xA5));
        let mut b = boot(slave_rom(0x3C));
        let mut cable = LinkCable::new();

        // Get a pairing in flight, then swap the pair over as the frontend
        // does and keep servicing with the arguments the other way round.
        for _ in 0..80 {
            a.step_instruction();
            b.step_instruction();
            cable.service(&mut a, &mut b);
            if cable.in_flight.is_some_and(|f| f.partner == Partner::Armed) {
                break;
            }
        }
        assert!(cable.in_flight.is_some_and(|f| f.partner == Partner::Armed));
        std::mem::swap(&mut a, &mut b);
        cable.swap_ends();
        // Stop the instant the transfer that was in flight across the swap
        // completes, so the assertions below are about that byte and no other.
        for _ in 0..8000 {
            a.step_instruction();
            b.step_instruction();
            cable.service(&mut a, &mut b);
            if cable.stats().transfers == 1 {
                break;
            }
        }

        // `a` and `b` are the other way round now, so the *slave* program is in
        // `a`. Each must still be holding what the other sent.
        assert_eq!(cable.stats().transfers, 1);
        assert_eq!(cable.stats().torn, 0);
        assert_eq!(a.peek(0xFF01), 0xA5, "the slave received the master's byte");
        // `SB` itself, not the program's copy: the master is still inside its
        // poll loop and has not stored the byte anywhere yet.
        assert_eq!(b.peek(0xFF01), 0x3C, "and the master received the slave's");
    }

    /// The fail-safe that is real: a console that crashes must take its partner
    /// off the air, because the survivor is the one holding half a trade.
    #[test]
    fn a_console_that_faults_takes_its_partner_off_the_air() {
        let mut a = boot(master_rom(0x11));
        let mut b = boot(slave_rom(0x22));
        let mut cable = LinkCable::new();
        cable.run_frame(&mut a, &mut b);
        assert!(b.fault().is_none());

        a.report_fault("the game has left the rails");
        cable.service(&mut a, &mut b);

        assert!(cable.desynced());
        assert!(
            b.fault().is_some(),
            "the surviving console must stop writing its save too"
        );
    }
}

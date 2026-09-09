//! DMG/CGB timer — modelled as the hardware circuit, not as two independent
//! counters.
//!
//! There is exactly **one** counter on the die: a 16-bit "system counter"
//! clocked at 4 MiHz. `DIV` ($FF04) is its upper 8 bits; writing *any* value to
//! `DIV` clears the whole 16-bit counter, not just the visible byte. `TIMA`
//! ($FF05) is incremented by a **falling edge** on `system_counter[tap] AND
//! TAC.enable`, where `tap` is selected by `TAC & 3`:
//!
//! | `TAC & 3` | frequency | tap bit | period |
//! |---|---|---|---|
//! | 0 | 4096 Hz | 9 | 1024 T |
//! | 1 | 262144 Hz | 3 | 16 T |
//! | 2 | 65536 Hz | 5 | 64 T |
//! | 3 | 16384 Hz | 7 | 256 T |
//!
//! Because the edge is taken from the *shared* counter, three things follow
//! that a "reset a private divisor every N cycles" model gets wrong, and each
//! has a Mooneye acceptance test:
//!
//! * Resetting `DIV` also resets the timer's phase — and if the tap bit was
//!   high at the time, clearing it **is** a falling edge, so `TIMA` increments
//!   (`timer/div_write`, `timer/rapid_toggle`).
//! * Starting or stopping the timer via `TAC` does not reset anything; it only
//!   gates the tap. Clearing the enable bit while the tap bit is high is again
//!   a falling edge (`timer/rapid_toggle`, "the timer circuit design causes
//!   some unexpected timer increases").
//! * The increment phase is absolute, so `TIMA` steps 1024 T after a `DIV`
//!   write, not 1024 T after the write to `TAC` (`timer/tim00`, `tim01`,
//!   `tim10`, `tim11` and the `*_div_trigger` variants).
//!
//! `TIMA` overflow is **not** instantaneous either. On overflow `TIMA` reads
//! `$00` for 4 T-cycles; only then is `TMA` copied in and the timer interrupt
//! requested (`timer/tima_reload`). A write to `TIMA` inside that window
//! cancels the reload *and* the interrupt, while a write during the reload
//! cycle itself is ignored (`timer/tima_write_reloading`), and a write to `TMA`
//! during the reload cycle is picked up by that very reload
//! (`timer/tma_write_reloading`).
//!
//! Reference: Pan Docs, "Timer and Divider Registers" / "Timer Obscure
//! Behaviour" <https://gbdev.io/pandocs/Timer_and_Divider_Registers.html>, and
//! Gekkio, *Game Boy: Complete Technical Reference* §"Timer".

#[derive(Default)]
pub struct Timer {
    /// The 16-bit system counter. `DIV` is `(div >> 8) as u8`.
    div: u16,
    tima: u8,
    tma: u8,
    /// Raw `TAC`; only the low 3 bits exist.
    tac: u8,
    /// `TAC`'s multiplexer output as a bit mask of the system counter, or 0
    /// when the timer is disabled — so [`Timer::signal`] is one AND instead of
    /// a table lookup, a shift and two branches. Derived from `tac` alone and
    /// rebuilt wherever `tac` is set; **never serialise it**, it is not state.
    tap_mask: u16,
    /// The system counter as it stood at the *start* of the current M-cycle.
    /// A CPU register write reaches this core after the M-cycle's tick, but
    /// hardware latches `TAC` before the counter advances — see [`Timer::wb`].
    prev_div: u16,
    /// Falling edges of the serial shift clock (system-counter bit 8) that have
    /// happened since the owner last drained them.
    ///
    /// The link port's shift register is divided from the *same* counter as
    /// `TIMA`, so it belongs here rather than in [`crate::mmu::serial`] — that
    /// is exactly what `acceptance/serial/boot_sclk_align` measures: edges
    /// align to the counter's reset, not to the moment `SC` was written. The
    /// owner drains this in the same breath as advancing the counter (see
    /// `MemoryManagementUnit::bus_tick`), so it never holds a value across a
    /// save point and is deliberately not serialised.
    serial_edges: u8,
    /// M-cycles the counter has been promised but not yet advanced by.
    ///
    /// The counter is the emulator's most-stepped piece of state — one M-cycle
    /// per memory access of every instruction — and essentially nothing looks
    /// at it: Pokémon Blue reads a timer register **twice a frame** and writes
    /// one five times in a 7,168-frame run, while `do_mcycle` is asked 5,943
    /// times a frame. So the steps are counted here and applied in closed form
    /// ([`Timer::batch_mcycles`]) when something could actually tell, which is
    /// the same trade [`crate::gpu::Gpu::defer_dots`] makes for the PPU.
    ///
    /// **Never serialised**: a save state is an observation and
    /// [`MemoryManagementUnit::flush_for_observer`] empties this first, so it
    /// is always zero at a save point.
    ///
    /// [`MemoryManagementUnit::flush_for_observer`]: crate::mmu::MemoryManagementUnit
    pending: u32,
    /// How far [`Timer::pending`] may run before the counter does something a
    /// program can observe *without asking* — a `TIMA` overflow, which raises
    /// an interrupt, or a serial shift while the link port is holding one.
    ///
    /// Everything else it does is observable only through a register read or
    /// an `IF` read, and both of those sync first. Derived from `tac`, `tima`
    /// and `div` by [`Timer::idle_mcycles`]; **not state**, and rebuilt at
    /// every flush.
    horizon: u32,
    /// The link port is mid-transfer, so a shift edge is observable on the
    /// M-cycle it happens and the counter may not run ahead of one.
    ///
    /// With no transfer in flight [`crate::mmu::serial::Serial::shift_clock`]
    /// returns on its first line, so the edges are inert and the counter is
    /// free to run to the next `TIMA` event.
    serial_hold: bool,
    /// Set on the M-cycle `TIMA` overflowed: `TMA` is copied in on the next one.
    reload_pending: bool,
    /// True for exactly the M-cycle in which `TMA` was copied into `TIMA`.
    reloading: bool,
    pub interrupt: u8,
}

/// Which bit of the system counter each `TAC & 3` mode taps.
const TAP: [u8; 4] = [9, 3, 5, 7];

impl Timer {
    /// The multiplexer output that `TIMA` counts falling edges of.
    #[inline]
    fn signal(&self) -> bool {
        self.div & self.tap_mask != 0
    }

    /// Rebuild [`Timer::tap_mask`] from `tac`. Every path that assigns `tac`
    /// must call this — there are three, and they are all in this file.
    #[inline]
    fn set_tac(&mut self, tac: u8) {
        self.tac = tac;
        self.tap_mask = if tac & 0x04 != 0 {
            1 << TAP[(tac & 0x03) as usize]
        } else {
            0
        };
    }

    /// Apply a new system-counter value, incrementing `TIMA` on a falling edge
    /// of the tap. Every path that can change the edge input — the clock, a
    /// `DIV` write, a `TAC` write — goes through here.
    #[inline]
    fn set_div(&mut self, div: u16) {
        let was = self.signal();
        self.div = div;
        if was && !self.signal() {
            self.inc_tima();
        }
    }

    #[inline]
    fn inc_tima(&mut self) {
        self.tima = self.tima.wrapping_add(1);
        if self.tima == 0 {
            // Reads $00 for the next 4 T-cycles; TMA lands then.
            self.reload_pending = true;
        }
    }

    /// Seed the 16-bit system counter with the value the boot ROM leaves
    /// behind at `PC=$0100`.
    ///
    /// **The value passed here is the counter as a CPU read observes it on the
    /// machine's first M-cycle**, which is one M-cycle ahead of what this core
    /// stores: [`MemoryManagementUnit::bus_tick`] advances the counter *before*
    /// the access it belongs to, so a read on M-cycle *n* sees `div + 4n`. The
    /// documented hardware value is therefore stored 4 T-cycles behind — see
    /// `docs/components/timer.md` and the `boot_div` note in `AGENTS.md`.
    ///
    /// [`MemoryManagementUnit::bus_tick`]: crate::mmu::MemoryManagementUnit
    pub fn set_boot_counter(&mut self, observed: u16) {
        self.div = observed.wrapping_sub(4);
        self.prev_div = self.div;
        self.refresh_horizon();
    }

    /// The system counter **as the current M-cycle's bus access observes it**.
    ///
    /// Peripherals whose divider is tapped off the same counter but whose edge
    /// is not a CPU-visible read (the serial shift clock) need this, not
    /// [`Self::div`]: the raw field is one M-cycle behind what a read returns,
    /// and `acceptance/serial/boot_sclk_align` measures the difference.
    #[inline]
    pub fn observed_counter(&self) -> u16 {
        self.div.wrapping_add(4)
    }

    pub fn rb(&mut self, a: u16) -> u8 {
        // The counter may be running behind — see [`Timer::pending`]. Nothing
        // may read a register off a stale one, and this is cheap: Pokémon Blue
        // reads a timer register twice a frame.
        self.sync();
        match a {
            0xFF04 => (self.div >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => 0xF8 | (self.tac & 0x07),
            _ => panic!("Timer does not handler read {:4X}", a),
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
        // `TAC`'s latch reads `prev_div`, and every one of these registers
        // moves the horizon, so the counter lands before the write and the
        // horizon is re-derived after it.
        self.sync();
        self.wb_inner(a, v);
        self.refresh_horizon();
    }

    fn wb_inner(&mut self, a: u16, v: u8) {
        match a {
            // Writing DIV clears the whole 16-bit counter. If the tap bit was
            // high this is a falling edge and TIMA increments — that is the
            // behaviour `timer/div_write` and `timer/rapid_toggle` check.
            //
            // Clearing *all sixteen* bits also matters outside the timer: the
            // APU's frame sequencer is clocked off the same counter, so a
            // residual sub-DIV value here desynchronises the DIV-APU event
            // (SameSuite `div_write_trigger` waits for `DIV == $10` and then
            // resets DIV to force one). See `Sound::div_write`.
            0xFF04 => self.set_div(0),
            0xFF05 => {
                if self.reloading {
                    // The reload wins over a write landing on the same cycle.
                } else {
                    self.tima = v;
                    // A write inside the 4-cycle overflow window cancels both
                    // the TMA reload and the interrupt.
                    self.reload_pending = false;
                }
            }
            0xFF06 => {
                self.tma = v;
                if self.reloading {
                    // Written on the reload cycle: this value is what gets
                    // loaded (`timer/tma_write_reloading`).
                    self.tima = v;
                }
            }
            0xFF07 => {
                let was = self.signal();
                self.set_tac(v & 0x07);
                if was && !self.signal() {
                    // The ordinary TAC glitch: the gate was open and this write
                    // closed it, which the edge detector sees as a fall.
                    self.inc_tima();
                } else {
                    // `TAC` is latched *before* the system counter's increment
                    // for its M-cycle, whereas a CPU write reaches this core
                    // after the tick (see `MemoryManagementUnit::bus_tick`). So
                    // also run the edge across that increment as the newly
                    // latched `TAC` would have seen it: enabling the timer on
                    // the very M-cycle the tap falls still counts. Without
                    // this, `acceptance/timer/rapid_toggle` misses exactly one
                    // increment — the one where its `TAC` enable lands on the
                    // counter's own 1→0 boundary — and takes one extra loop
                    // iteration to overflow (BC = $FFD8 rather than $FFD9).
                    let now = self.div;
                    self.div = self.prev_div;
                    let early = self.signal();
                    self.div = now;
                    if early && !self.signal() {
                        self.inc_tima();
                    }
                }
            }
            _ => panic!("Timer does not handler write {:4X}", a),
        };
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u16(self.div);
        w.u8(self.tima);
        w.u8(self.tma);
        w.u8(self.tac);
        w.u16(self.prev_div);
        w.bool(self.reload_pending);
        w.bool(self.reloading);
        w.u8(self.interrupt);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.div = r.u16()?;
        self.tima = r.u8()?;
        self.tma = r.u8()?;
        let tac = r.u8()?;
        self.set_tac(tac);
        self.prev_div = r.u16()?;
        self.reload_pending = r.bool()?;
        self.reloading = r.bool()?;
        self.interrupt = r.u8()?;
        // Neither is state: a save point is always synced, and the horizon is
        // derived from the three registers just restored.
        self.pending = 0;
        self.refresh_horizon();
        Ok(())
    }

    /// Advance one M-cycle (4 T-cycles), the finest granularity anything in
    /// this core can observe. Every tap is at bit 3 or above (period ≥ 16 T),
    /// so a 4 T step can never skip an edge.
    /// Advance exactly one M-cycle — what [`MemoryManagementUnit::bus_tick`]
    /// wants, without [`Timer::do_cycle`]'s loop and remainder around it. It is
    /// asked ~17,000 times a frame in a busy game.
    ///
    /// [`MemoryManagementUnit::bus_tick`]: crate::mmu::MemoryManagementUnit
    /// The counter owes an M-cycle. Returns **true** when that landed the
    /// counter on something the caller has to look at — an interrupt, or a
    /// serial edge — and false, which is almost always, when it did not.
    ///
    /// This is the per-access path and it is two instructions in the common
    /// case. See [`Timer::pending`] for why the counter is allowed to lag.
    #[inline]
    pub(crate) fn tick_mcycle(&mut self) -> bool {
        #[cfg(feature = "profile")]
        if crate::profile::timer_skipped() {
            return false;
        }
        self.pending += 1;
        if self.pending >= self.horizon {
            self.sync_cold();
            return true;
        }
        false
    }

    /// [`Timer::tick_mcycle`] for a whole instruction's remaining T-cycles.
    ///
    /// A span that is not a whole number of M-cycles cannot be counted, so it
    /// takes the slow path — nothing in this core produces one, but
    /// `do_cycle`'s partial-cycle arm exists and this must not silently drop it.
    #[inline]
    pub(crate) fn tick_ticks(&mut self, ticks: u32) -> bool {
        #[cfg(feature = "profile")]
        if crate::profile::timer_skipped() {
            return false;
        }
        if ticks & 3 != 0 {
            self.sync();
            self.do_cycle_cold(ticks);
            self.refresh_horizon();
            return true;
        }
        self.pending += ticks >> 2;
        if self.pending >= self.horizon {
            self.sync_cold();
            return true;
        }
        false
    }

    /// Is the counter behind where the machine has actually reached?
    #[inline]
    pub(crate) fn needs_sync(&self) -> bool {
        self.pending != 0
    }

    /// Bring the counter up to the machine's real position.
    ///
    /// **Every path that can observe the timer calls this first**: a read or a
    /// write of `$FF04-$FF07`, a read or a write of `IF`, the link port's
    /// registers, the halt horizon, and a save state.
    #[inline]
    pub(crate) fn sync(&mut self) {
        if self.pending != 0 {
            self.sync_cold();
        }
    }

    /// [`Timer::sync`]'s body, kept out of line so the per-access path is an
    /// add, a compare and a predicted branch.
    #[cold]
    #[inline(never)]
    fn sync_cold(&mut self) {
        let n = std::mem::take(&mut self.pending);
        self.do_cycle_inner(4 * n);
        self.refresh_horizon();
    }

    /// The link port has started or finished shifting; the counter may or may
    /// not run ahead of a shift edge from here.
    pub(crate) fn set_serial_hold(&mut self, held: bool) {
        if self.serial_hold != held {
            self.sync();
            self.serial_hold = held;
            self.refresh_horizon();
        }
    }

    /// Recompute how far the counter may run unobserved.
    ///
    /// Capped so `pending` cannot grow without bound on a cartridge that never
    /// enables the timer — `batch_mcycles` counts in `u32` and a bounded span
    /// keeps that argument trivially true.
    /// **Why an unbounded horizon is a hardware guarantee and not a lucky
    /// property of one cartridge.** `TAC` bit 2 is documented as the timer's
    /// stop/start control, and `TIMA` counts only on the input clock `TAC`
    /// selects — so with that bit clear the counter cannot reach `TIMA` at all,
    /// on any cartridge (Nintendo, *Game Boy Programming Manual* v1.1, §2.4.3
    /// "Timer Registers"; Pan Docs, ["Timer and Divider
    /// Registers"](https://gbdev.io/pandocs/Timer_and_Divider_Registers.html)).
    /// `DIV` keeps counting regardless — it is the top byte of the free-running
    /// 16-bit counter (§2.4.2) — which is why a `DIV` read still syncs.
    ///
    /// That distinction is the whole safety argument: Pokémon Blue never
    /// setting the bit is what makes the win *large*, and the contract above is
    /// what makes it *correct*. When a cartridge does set it, `idle_mcycles`
    /// returns a finite horizon and the counter is applied on the M-cycle the
    /// overflow lands — which is what Mooneye's thirteen `acceptance/timer/*`
    /// ROMs measure, and they are green.
    fn refresh_horizon(&mut self) {
        const HORIZON_CAP: u32 = 0x1_0000;
        self.horizon = if self.serial_hold {
            1
        } else {
            self.idle_mcycles_raw().min(HORIZON_CAP)
        };
    }

    fn do_mcycle(&mut self) {
        // The overflow reload lands 4 T after the overflow itself, i.e. at the
        // top of the following M-cycle — before any bus access in it.
        self.reloading = self.reload_pending;
        if self.reload_pending {
            self.reload_pending = false;
            self.tima = self.tma;
            self.interrupt |= 0x04;
        }
        self.prev_div = self.div;
        self.set_div(self.div.wrapping_add(4));
        // The internal serial clock is 8192 Hz, i.e. one shift per 512 T — a
        // falling edge of counter bit 8. It is taken from the counter as a bus
        // access observes it (see `observed_counter`), one M-cycle ahead of the
        // raw field; `acceptance/serial/boot_sclk_align` fails by exactly one
        // instruction if it is taken from `div` directly.
        if self.observed_counter() & 0x01FF == 0 {
            self.serial_edges = self.serial_edges.saturating_add(1);
        }
    }

    /// Is there a serial shift-clock edge waiting to be drained?
    ///
    /// [`Timer::take_serial_edges`] writes the field whether or not there was
    /// one, so a per-M-cycle caller that drains unconditionally dirties a cache
    /// line ~17,000 times a frame for an event that happens 137 times.
    #[inline]
    pub fn serial_edge_pending(&self) -> bool {
        self.serial_edges != 0
    }

    /// Take the serial shift-clock edges accumulated since the last call.
    /// Always drain this in the same step that advanced the counter.
    #[inline]
    pub fn take_serial_edges(&mut self) -> u8 {
        std::mem::take(&mut self.serial_edges)
    }

    /// How many **M-cycles** the timer can be advanced in one step before it
    /// could raise its interrupt — the timer's half of the horizon a halted CPU
    /// may be batched over. `u32::MAX` means "never, from here".
    ///
    /// `TIMA` steps on a falling edge of the tap, which is the M-cycle the
    /// system counter reaches a multiple of the tap's period, so the distance
    /// to the next edge is exact arithmetic; overflow needs `0x100 - TIMA` of
    /// them, and the interrupt lands one M-cycle after the last. A reload
    /// already pending *is* that next M-cycle.
    pub(crate) fn idle_mcycles(&self) -> u32 {
        // Answer for where the counter *will* be once `pending` lands, so a
        // caller never has to know whether a sync is outstanding.
        self.idle_mcycles_raw().saturating_sub(self.pending)
    }

    /// [`Timer::idle_mcycles`] measured from the counter as it currently
    /// stands, i.e. ignoring [`Timer::pending`]. Only the horizon wants this.
    fn idle_mcycles_raw(&self) -> u32 {
        if self.reload_pending || self.reloading {
            return 1;
        }
        if self.tac & 0x04 == 0 {
            return u32::MAX;
        }
        let period: u32 = 1 << (TAP[(self.tac & 0x03) as usize] + 1);
        let to_edge = period - (self.div as u32 & (period - 1));
        let edges = 0x100u32 - self.tima as u32;
        let dots = to_edge + (edges - 1) * period;
        dots.div_ceil(4)
    }

    /// Advance `n` whole M-cycles in **closed form**, or decline.
    ///
    /// [`Timer::do_cycle`]'s loop is the emulator's most-called piece of
    /// arithmetic — a halted CPU is batched up to `IDLE_BATCH_MAX` M-cycles at
    /// a time and Pokémon Blue is halted for about four fifths of its frame, so
    /// three quarters of every `do_mcycle` in a run is a loop iteration whose
    /// only job is to add four to a counter. Nothing in that span is *observed*
    /// per M-cycle: the two things a step can produce are a `TIMA` increment and
    /// a serial shift edge, and both are counted by arithmetic.
    ///
    /// The counting rests on one fact about the circuit and one about the step.
    /// `TIMA` counts falling edges of system-counter bit `TAP[tac & 3]`, whose
    /// period `P` is at least 16 T; the step is exactly 4 T; so the bit can only
    /// fall on the M-cycle that carries the counter across a multiple of `P`,
    /// and the number of those in a span telescopes to a difference of two
    /// divisions. `P` divides 65536, so the counter's own wrap is one of them
    /// and 32-bit arithmetic counts it correctly. The serial clock is the same
    /// argument with a period of 512 T, one M-cycle ahead
    /// ([`Timer::observed_counter`]).
    ///
    /// It **declines** — leaving the caller to run the loop — whenever a step
    /// inside the span would do more than that: a reload already in flight, or
    /// a `TIMA` overflow anywhere in the span, both of which are stateful. Those
    /// are rare (a few M-cycles a frame), so the loop still exists and is still
    /// the definition; this only skips it when it provably has nothing to say.
    /// `timer::tests::the_batch_matches_the_loop_everywhere_it_is_taken`
    /// exhausts that claim against the loop itself.
    #[inline]
    fn batch_mcycles(&mut self, n: u32) -> bool {
        // A reload is a two-M-cycle state machine; leave it to the loop. The
        // counter must also be on the 4 T grid the step assumes — it always is,
        // but `do_cycle`'s partial-cycle path could in principle leave it off.
        if n < 2 || self.reload_pending || self.reloading || self.div & 3 != 0 {
            return false;
        }
        let start = self.div as u32;
        let span = 4 * n;

        let edges = if self.tap_mask == 0 {
            0
        } else {
            // The tap's full period: the bit is high for the top half of it and
            // falls when the counter crosses a multiple.
            let p = (self.tap_mask as u32) << 1;
            (start + span) / p - start / p
        };
        // An overflow is stateful (`TIMA` reads $00 for one M-cycle, then `TMA`
        // lands and the interrupt is raised): decline and let the loop do it.
        if edges + self.tima as u32 > 0xFF {
            return false;
        }
        self.tima += edges as u8;

        // The serial clock is bit 8 of the counter **as a bus access observes
        // it**, which is one M-cycle ahead of the raw field.
        let serial = (start + 4 + span) / 512 - (start + 4) / 512;
        if serial != 0 {
            self.serial_edges = self.serial_edges.saturating_add(serial.min(255) as u8);
        }

        self.prev_div = (start + span - 4) as u16;
        self.div = (start + span) as u16;
        true
    }

    /// Advance the timer by `ticks` T-cycles.
    ///
    /// **One call site, from `MemoryManagementUnit::do_cycle`**, so that this
    /// stays inlined into `Gameboy::frame` where it has always been. The
    /// horizon batch's flush path calls [`Timer::do_cycle_cold`] instead;
    /// giving this one a second caller takes it out of `frame` and costs 3 %
    /// of the accurate frame, measured — the one-call-site rule in
    /// `docs/measured/speed-ledger.md` §3a, one level down.
    #[inline]
    pub fn do_cycle(&mut self, ticks: u32) {
        self.sync();
        self.do_cycle_inner(ticks);
        self.refresh_horizon();
    }

    /// [`Timer::do_cycle`] behind a call, for the horizon batch's flush.
    #[cold]
    #[inline(never)]
    pub(crate) fn do_cycle_cold(&mut self, ticks: u32) {
        self.do_cycle_inner(ticks);
    }

    #[inline(always)]
    fn do_cycle_inner(&mut self, ticks: u32) {
        // Measurement only, compiled out of every shipping build.
        #[cfg(feature = "profile")]
        if crate::profile::timer_skipped() {
            return;
        }
        let mut left = ticks;
        // Measurement only, compiled out of every shipping build: the honest
        // "before" for the closed form is the loop underneath it.
        #[cfg(feature = "profile")]
        let batch = !crate::profile::timer_batch_skipped();
        #[cfg(not(feature = "profile"))]
        let batch = true;
        if batch && self.batch_mcycles(left / 4) {
            left &= 3;
        }
        while left >= 4 {
            self.do_mcycle();
            left -= 4;
        }
        // Nothing in this core ever asks for a partial M-cycle, but keep the
        // counter honest if something ever does.
        if left > 0 {
            self.prev_div = self.div;
            self.set_div(self.div.wrapping_add(left as u16));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DIV` is the top byte of the shared counter, and a write clears all 16
    /// bits — not just the visible byte.
    #[test]
    fn div_write_clears_the_whole_system_counter() {
        let mut t = Timer::default();
        t.do_cycle(4 * 300);
        assert_eq!(t.rb(0xFF04), (1200u16 >> 8) as u8);
        t.wb(0xFF04, 0xAB);
        assert_eq!(t.rb(0xFF04), 0);
        // Phase is reset too: TIMA (4096 Hz) steps 1024 T later, not sooner.
        t.wb(0xFF07, 0x04);
        t.wb(0xFF05, 0);
        t.do_cycle(1020);
        assert_eq!(t.rb(0xFF05), 0);
        t.do_cycle(4);
        assert_eq!(t.rb(0xFF05), 1);
    }

    /// Clearing `DIV` while the tap bit is high is a falling edge, so `TIMA`
    /// increments — the defect `acceptance/timer/div_write` detects.
    #[test]
    fn a_div_write_with_the_tap_bit_high_increments_tima() {
        let mut t = Timer::default();
        t.wb(0xFF07, 0x04); // 4096 Hz, tap = bit 9
        t.do_cycle(512); // bit 9 now high
        assert_eq!(t.rb(0xFF05), 0);
        t.wb(0xFF04, 0);
        assert_eq!(t.rb(0xFF05), 1);
    }

    /// Stopping the timer while the tap bit is high is also a falling edge —
    /// `acceptance/timer/rapid_toggle`'s "unexpected timer increases".
    #[test]
    fn stopping_the_timer_with_the_tap_bit_high_increments_tima() {
        let mut t = Timer::default();
        t.wb(0xFF07, 0x04);
        t.do_cycle(512);
        t.wb(0xFF07, 0x00); // stop
        assert_eq!(t.rb(0xFF05), 1);
        // ...and stopping does not reset the counter's phase.
        t.wb(0xFF07, 0x04);
        t.do_cycle(512);
        assert_eq!(t.rb(0xFF05), 2);
    }

    /// On overflow `TIMA` reads `$00` for 4 T-cycles before `TMA` arrives, and
    /// the interrupt is requested with the reload, not with the overflow.
    #[test]
    fn tima_reads_zero_for_four_cycles_before_tma_is_loaded() {
        let mut t = Timer::default();
        t.wb(0xFF06, 0x7E); // TMA
        t.wb(0xFF05, 0xFF); // TIMA
        t.wb(0xFF07, 0x05); // enable, 262144 Hz (tap bit 3, period 16 T)
        t.do_cycle(16);
        assert_eq!(t.rb(0xFF05), 0x00, "overflow window reads 00");
        assert_eq!(
            t.interrupt, 0,
            "interrupt fires with the reload, not the overflow"
        );
        t.do_cycle(4);
        assert_eq!(t.rb(0xFF05), 0x7E);
        assert_eq!(t.interrupt, 0x04);
    }

    /// A `TIMA` write inside the overflow window cancels the reload and the
    /// interrupt (`acceptance/timer/tima_write_reloading`).
    #[test]
    fn a_tima_write_in_the_overflow_window_cancels_the_reload() {
        let mut t = Timer::default();
        t.wb(0xFF06, 0x7E);
        t.wb(0xFF05, 0xFF);
        t.wb(0xFF07, 0x05);
        t.do_cycle(16);
        t.wb(0xFF05, 0x33);
        t.do_cycle(4);
        assert_eq!(t.rb(0xFF05), 0x33);
        assert_eq!(t.interrupt, 0, "cancelled: no timer interrupt");
    }

    /// A `TIMA` write *on* the reload cycle loses to `TMA`; a `TMA` write on
    /// that cycle wins (`acceptance/timer/tma_write_reloading`).
    #[test]
    fn the_reload_cycle_beats_a_tima_write_and_honours_a_tma_write() {
        let mut t = Timer::default();
        t.wb(0xFF06, 0x7E);
        t.wb(0xFF05, 0xFF);
        t.wb(0xFF07, 0x05);
        t.do_cycle(16); // overflow
        t.do_cycle(4); // reload cycle
        t.wb(0xFF05, 0x33);
        assert_eq!(
            t.rb(0xFF05),
            0x7E,
            "TIMA write on the reload cycle is ignored"
        );

        let mut t = Timer::default();
        t.wb(0xFF06, 0x7E);
        t.wb(0xFF05, 0xFF);
        t.wb(0xFF07, 0x05);
        t.do_cycle(16);
        t.do_cycle(4);
        t.wb(0xFF06, 0x11);
        assert_eq!(
            t.rb(0xFF05),
            0x11,
            "TMA write on the reload cycle is picked up"
        );
    }

    /// Enabling the timer on the very M-cycle the tap falls still produces an
    /// increment, because `TAC` latches before the counter advances. This is
    /// the last of `acceptance/timer/rapid_toggle`'s "unexpected timer
    /// increases"; without it that test overflows one loop iteration late.
    #[test]
    fn enabling_on_the_taps_own_falling_edge_still_increments() {
        let mut t = Timer::default();
        // Park the counter 4 T short of a bit-9 fall with the timer stopped.
        t.do_cycle(1020);
        assert_eq!(t.rb(0xFF05), 0);
        // One more M-cycle carries the counter across 1024; the enable write
        // lands in that same M-cycle, after our tick but before hardware's.
        t.do_cycle(4);
        t.wb(0xFF07, 0x04);
        assert_eq!(t.rb(0xFF05), 1);
    }

    /// Pins `crate::mmu::CGB_BOOT_SYSTEM_COUNTER` — the boot-time system
    /// counter for a **genuine** CGB-flagged cartridge (`GbMode::Color`,
    /// as distinct from a DMG cartridge's compatibility path) — against the
    /// exact M-cycle count gambatte's own `div/start_inc_{1,2}_cgb04c.asm`
    /// measures it at.
    ///
    /// Each ROM is `jp; jp; NOP × N; LDH A,($04)` from `$0100`; N = 13 wants
    /// `DIV = $1E`, N = 14 wants `$1F`. Counted in real bus accesses (each
    /// `NOP` is one M-cycle, matching [`Timer::tick_mcycle`] one for one)
    /// the read lands on M-cycle 24 for N = 13 and 25 for N = 14: the two
    /// `jp`s cost 4 M-cycles each *before* the first `NOP`, and the `LDH`
    /// costs 3 more *after* the last one — 8 + N + 3.
    /// See the constant's own doc comment for the derivation and the
    /// independent BullyGB corroboration.
    #[test]
    fn a_genuine_cgb_cartridge_boots_div_where_gambatte_measures_it() {
        let mut t = Timer::default();
        t.set_boot_counter(crate::mmu::CGB_BOOT_SYSTEM_COUNTER);
        // `tick_mcycle` is the per-access path, so this also pins that the
        // deferred counter reads back exactly where the stepped one did:
        // `rb` syncs, and the answer is the same M-cycle for M-cycle.
        for _ in 0..24 {
            t.tick_mcycle();
        }
        assert_eq!(t.rb(0xFF04), 0x1E, "13 NOPs (M-cycle 24)");
        t.tick_mcycle();
        assert_eq!(t.rb(0xFF04), 0x1F, "14 NOPs (M-cycle 25)");
    }

    /// Everything the closed-form batch claims, checked against the loop it
    /// replaces — the loop stays the definition.
    ///
    /// One M-cycle at a time always declines the batch (`n < 2`), so the
    /// right-hand machine below is the old code exactly. 133,120 starting
    /// states cover every phase of the longest tap (1024 T), every `TAC`
    /// including the disabled ones, the `TIMA` values either side of an
    /// overflow, and the counter's own 16-bit wrap. Teeth: drop the `& 3` grid
    /// guard, count crossings of `P/2` instead of `P`, or take the serial edge
    /// from `div` rather than `observed_counter`, and this goes red.
    #[test]
    fn the_batch_matches_the_loop_everywhere_it_is_taken() {
        fn state(t: &Timer) -> (u16, u8, u8, u8, u16, u16, u8, bool, bool, u8) {
            (
                t.div,
                t.tima,
                t.tma,
                t.tac,
                t.tap_mask,
                t.prev_div,
                t.serial_edges,
                t.reload_pending,
                t.reloading,
                t.interrupt,
            )
        }
        fn seed(start: u16, tac: u8, tima: u8) -> Timer {
            let mut t = Timer::default();
            t.wb(0xFF06, 0x33);
            t.wb(0xFF07, tac);
            t.wb(0xFF05, tima);
            t.div = start;
            t.prev_div = start;
            t
        }

        let mut batched = 0u32;
        for phase in 0..256u16 {
            for &wrap in &[0u16, 0xFC00] {
                let start = wrap.wrapping_add(phase * 4);
                for tac in 0..8u8 {
                    for &tima in &[0u8, 1, 0xFD, 0xFE, 0xFF] {
                        for &n in &[2u32, 3, 4, 5, 7, 8, 15, 16, 17, 31, 32, 63, 64] {
                            let mut whole = seed(start, tac, tima);
                            let mut step = seed(start, tac, tima);
                            if whole.batch_mcycles(n) {
                                batched += 1;
                            } else {
                                for _ in 0..n {
                                    whole.do_mcycle();
                                }
                            }
                            for _ in 0..n {
                                step.do_cycle(4);
                            }
                            assert_eq!(
                                state(&whole),
                                state(&step),
                                "start {start:#06x} tac {tac:#04x} \
                                 tima {tima:#04x} n {n}"
                            );
                        }
                    }
                }
            }
        }
        // The batch must actually be *taken* on the overwhelming majority of
        // those states, or the test is only checking the loop against itself.
        assert!(batched > 100_000, "batch taken only {batched} times");
    }

    /// `TAC` reads back with the unused top five bits set.
    #[test]
    fn tac_reads_back_with_unused_bits_high() {
        let mut t = Timer::default();
        t.wb(0xFF07, 0x05);
        assert_eq!(t.rb(0xFF07), 0xFD);
        t.wb(0xFF07, 0xFF);
        assert_eq!(t.rb(0xFF07), 0xFF);
    }
}

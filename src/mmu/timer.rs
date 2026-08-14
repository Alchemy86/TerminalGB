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
        self.tac & 0x04 != 0 && self.div & (1 << TAP[(self.tac & 0x03) as usize]) != 0
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

    pub fn rb(&self, a: u16) -> u8 {
        match a {
            0xFF04 => (self.div >> 8) as u8,
            0xFF05 => self.tima,
            0xFF06 => self.tma,
            0xFF07 => 0xF8 | (self.tac & 0x07),
            _ => panic!("Timer does not handler read {:4X}", a),
        }
    }

    pub fn wb(&mut self, a: u16, v: u8) {
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
                self.tac = v & 0x07;
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
        self.tac = r.u8()?;
        self.prev_div = r.u16()?;
        self.reload_pending = r.bool()?;
        self.reloading = r.bool()?;
        self.interrupt = r.u8()?;
        Ok(())
    }

    /// Advance one M-cycle (4 T-cycles), the finest granularity anything in
    /// this core can observe. Every tap is at bit 3 or above (period ≥ 16 T),
    /// so a 4 T step can never skip an edge.
    #[inline]
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

    /// Take the serial shift-clock edges accumulated since the last call.
    /// Always drain this in the same step that advanced the counter.
    #[inline]
    pub fn take_serial_edges(&mut self) -> u8 {
        std::mem::take(&mut self.serial_edges)
    }

    pub fn do_cycle(&mut self, ticks: u32) {
        let mut left = ticks;
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

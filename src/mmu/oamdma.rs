//! OAM DMA ($FF46) as a **timed** transfer.
//!
//! Writing a page number to `$FF46` does not copy 160 bytes instantly. Hardware
//! copies one byte per M-cycle for 160 M-cycles, and it does not begin
//! immediately — Mooneye's `acceptance/oam_dma_start` states the schedule
//! exactly:
//!
//! ```text
//! M = 0: write to $FF46 happens
//! M = 1: nothing (OAM still accessible)
//! M = 2: new DMA starts, OAM reads will return $FF
//! ```
//!
//! and, for a DMA restarted while one is already running, the *previous*
//! transfer keeps running through M = 1 and only stops when the new one starts
//! at M = 2.
//!
//! While a transfer runs the CPU cannot see OAM: reads return `$FF` and writes
//! are dropped. That is not a detail — it is how Mooneye measures instruction
//! timing at all. `acceptance/rst_timing`, `call_timing`, `push_timing` and
//! friends fill VRAM with a known byte, DMA it into OAM, point `SP` at OAM and
//! then execute the instruction under test, so which of its pushes survive
//! reveals the exact M-cycle each memory access lands on. With an instantaneous
//! DMA every one of those tests is unmeasurable.
//!
//! Reference: Pan Docs, "OAM DMA Transfer"
//! <https://gbdev.io/pandocs/OAM_DMA_Transfer.html>; Gekkio, *Game Boy:
//! Complete Technical Reference* §"OAM DMA".

/// Bytes in a transfer — also the "idle" value of [`OamDma::pos`].
const LEN: u8 = 0xA0;

/// M-cycles between the write to `$FF46` and the first byte being copied.
const START_DELAY: u8 = 2;

#[derive(Default)]
pub struct OamDma {
    /// Last value written to `$FF46`. Reads return it whatever the transfer
    /// state is (`acceptance/oam_dma/reg_read`).
    reg: u8,
    /// M-cycles left before the requested transfer starts; 0 = none requested.
    delay: u8,
    /// Source page latched by the write, waiting for `delay` to expire.
    pending_src: u8,
    /// Source page of the running transfer.
    src: u8,
    /// Index of the byte being copied in the *current* M-cycle; `LEN` means
    /// nothing is running. It is advanced at the top of [`OamDma::step`] rather
    /// than after the copy, so [`OamDma::active`] is still true for the CPU
    /// access that shares the M-cycle with the transfer's last byte — the
    /// single cycle `acceptance/oam_dma_timing` measures.
    pos: u8,
}

impl OamDma {
    pub fn new() -> Self {
        OamDma {
            reg: 0,
            delay: 0,
            pending_src: 0,
            src: 0,
            pos: LEN,
        }
    }

    /// A write to `$FF46`: arm a transfer to start two M-cycles from now. Any
    /// transfer already running keeps going until then.
    pub fn request(&mut self, value: u8) {
        self.reg = value;
        self.pending_src = value;
        self.delay = START_DELAY;
    }

    pub fn read_reg(&self) -> u8 {
        self.reg
    }

    /// True while OAM belongs to the DMA rather than to the CPU.
    #[inline]
    pub fn active(&self) -> bool {
        self.pos < LEN
    }

    /// Nothing pending and nothing running — the common case, checked once per
    /// M-cycle, so keep it a single test.
    #[inline]
    pub fn idle(&self) -> bool {
        self.delay == 0 && self.pos >= LEN
    }

    /// Advance one M-cycle. Returns `Some((source_page, index))` when a byte
    /// must be copied in this cycle.
    #[inline]
    pub fn step(&mut self) -> Option<(u8, u8)> {
        if self.pos < LEN {
            // Retire the byte copied in the previous M-cycle. Reaching LEN here
            // is what ends the transfer.
            self.pos += 1;
        }
        if self.delay > 0 {
            self.delay -= 1;
            if self.delay == 0 {
                // The previous transfer, if any, is abandoned here — not when
                // the write happened (`acceptance/oam_dma_restart`).
                self.src = self.pending_src;
                self.pos = 0;
            }
        }
        if self.pos < LEN {
            Some((self.src, self.pos))
        } else {
            None
        }
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.reg);
        w.u8(self.delay);
        w.u8(self.pending_src);
        w.u8(self.src);
        w.u8(self.pos);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.reg = r.u8()?;
        self.delay = r.u8()?;
        self.pending_src = r.u8()?;
        self.src = r.u8()?;
        self.pos = r.u8()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schedule from `acceptance/oam_dma_start`: the write is M=0, M=1 is
    /// idle and OAM is still the CPU's, the first byte moves at M=2.
    #[test]
    fn a_transfer_starts_two_mcycles_after_the_write() {
        let mut d = OamDma::new();
        d.request(0x80);
        assert!(!d.active(), "M=0: the write itself does not start it");
        assert_eq!(d.step(), None, "M=1: still nothing");
        assert!(!d.active(), "M=1: OAM is still accessible");
        assert_eq!(d.step(), Some((0x80, 0)), "M=2: first byte");
        assert!(d.active(), "M=2: OAM now belongs to the DMA");
    }

    /// 160 bytes, one per M-cycle, and OAM comes back exactly one M-cycle after
    /// the last one (`acceptance/oam_dma_timing`).
    #[test]
    fn a_transfer_is_160_mcycles_long() {
        let mut d = OamDma::new();
        d.request(0x80);
        d.step();
        for i in 0..0xA0u8 {
            assert_eq!(d.step(), Some((0x80, i)));
            assert!(
                d.active(),
                "OAM is still the DMA's on the cycle it copies byte {i}"
            );
        }
        assert_eq!(d.step(), None);
        assert!(!d.active(), "and the CPU's again on the very next M-cycle");
    }

    /// Restarting mid-transfer does not stop the old one at the write — it runs
    /// for one more M-cycle (`acceptance/oam_dma_restart`).
    #[test]
    fn a_restart_lets_the_previous_transfer_run_one_more_cycle() {
        let mut d = OamDma::new();
        d.request(0x80);
        d.step();
        for _ in 0..10 {
            d.step();
        }
        d.request(0xC0);
        assert_eq!(
            d.step(),
            Some((0x80, 10)),
            "M=1: the old transfer still owns the bus"
        );
        assert_eq!(
            d.step(),
            Some((0xC0, 0)),
            "M=2: the new one takes over from byte 0"
        );
        assert_eq!(d.step(), Some((0xC0, 1)));
    }

    /// `$FF46` reads back the last value written, running or not.
    #[test]
    fn the_register_reads_back_what_was_written() {
        let mut d = OamDma::new();
        d.request(0xC1);
        assert_eq!(d.read_reg(), 0xC1);
        d.step();
        d.step();
        assert_eq!(d.read_reg(), 0xC1);
    }
}

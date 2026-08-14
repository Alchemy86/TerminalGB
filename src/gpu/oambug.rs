//! The DMG **OAM corruption bug** — the hardware defect, modelled.
//!
//! On a DMG/MGB/SGB (but **not** a CGB, even in DMG-compat mode) the CPU
//! putting an address in `$FE00-$FEFF` on the bus while the PPU is scanning
//! OAM (mode 2) corrupts OAM. The corruption is not random: it is the sprite
//! FIFO's row latch being clobbered, so whole 8-byte rows are copied and one
//! word is mangled by a fixed bitwise expression.
//!
//! Reference: Pan Docs, ["OAM Corruption
//! Bug"](https://gbdev.io/pandocs/OAM_Corruption_Bug.html) — the three
//! corruption patterns below are its "Corruption Patterns" section verbatim in
//! behaviour, and the trigger list is its "Affected Operations".
//!
//! OAM is 20 rows of 8 bytes (two objects each). During mode 2 the PPU reads
//! one row per M-cycle, so *which* row is corrupted is a pure function of the
//! dot the offending bus cycle lands on. Row 0 — objects 0 and 1 — is never
//! corrupted, which is why the corruption window is 19 M-cycles wide and not
//! 20 (Blargg `oam_bug/4-scanline_timing`).
//!
//! Everything here is bit-parallel, so the "16-bit word" the hardware
//! documentation talks about is applied byte by byte: `((a^c)&(b^c))^c` and
//! friends commute with byte splitting, and OAM's byte order never enters into
//! it.

/// Bytes per OAM row — two objects.
pub(crate) const ROW: usize = 8;
/// Rows of OAM the PPU walks during mode 2, one per M-cycle.
pub(crate) const ROWS: usize = 20;
/// Byte offset of the third word within a row.
const THIRD_WORD: usize = 4;

/// What the CPU put on the OAM bus during a mode-2 M-cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OamBug {
    /// A write — or the "glitched write" the 16-bit increment/decrement unit
    /// performs just by putting its operand on the address bus.
    Write,
    /// A read.
    Read,
    /// A read *and* an IDU write in the same M-cycle: `ld a,[hli]`,
    /// `ld a,[hld]`, and the first read of a `pop`.
    ReadWrite,
}

/// Apply one corruption to `oam` at the row the PPU is currently scanning.
pub(crate) fn corrupt(oam: &mut [u8], row: usize, kind: OamBug) {
    debug_assert!(oam.len() >= ROWS * ROW);
    if row >= ROWS {
        return;
    }
    match kind {
        OamBug::Write => write_corrupt(oam, row),
        OamBug::Read => read_corrupt(oam, row),
        OamBug::ReadWrite => read_write_corrupt(oam, row),
    }
}

/// The first word of the row, replaced; the last three copied from the row
/// before. `f` is the bitwise expression that distinguishes a read from a
/// write.
fn corrupt_row(oam: &mut [u8], row: usize, f: impl Fn(u8, u8, u8) -> u8) {
    // Objects 0 and 1 are not affected: there is no preceding row to copy.
    if row == 0 {
        return;
    }
    let prev = (row - 1) * ROW;
    let cur = row * ROW;
    for i in 0..2 {
        let a = oam[cur + i];
        let b = oam[prev + i];
        let c = oam[prev + THIRD_WORD + i];
        oam[cur + i] = f(a, b, c);
    }
    for i in 2..ROW {
        oam[cur + i] = oam[prev + i];
    }
}

/// `((a ^ c) & (b ^ c)) ^ c` — a majority vote of the three words, bit by bit.
fn write_corrupt(oam: &mut [u8], row: usize) {
    corrupt_row(oam, row, |a, b, c| ((a ^ c) & (b ^ c)) ^ c);
}

/// `b | (a & c)`.
fn read_corrupt(oam: &mut [u8], row: usize) {
    corrupt_row(oam, row, |a, b, c| b | (a & c));
}

/// A read and a write in the same M-cycle. The extra pattern reaches *back* a
/// row and forward again; it does not happen in the first four rows, nor in
/// the last. A plain read corruption is then applied on top, always.
fn read_write_corrupt(oam: &mut [u8], row: usize) {
    if (4..ROWS - 1).contains(&row) {
        let two_back = (row - 2) * ROW;
        let prev = (row - 1) * ROW;
        let cur = row * ROW;
        for i in 0..2 {
            let a = oam[two_back + i];
            let b = oam[prev + i];
            let c = oam[cur + i];
            let d = oam[prev + THIRD_WORD + i];
            oam[prev + i] = (b & (a | c | d)) | (a & c & d);
        }
        for i in 0..ROW {
            let v = oam[prev + i];
            oam[cur + i] = v;
            oam[two_back + i] = v;
        }
    }
    read_corrupt(oam, row);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> Vec<u8> {
        (0..(ROWS * ROW) as u8).collect()
    }

    #[test]
    fn row_zero_is_never_corrupted() {
        for kind in [OamBug::Write, OamBug::Read, OamBug::ReadWrite] {
            let mut oam = filled();
            corrupt(&mut oam, 0, kind);
            assert_eq!(oam, filled(), "{kind:?} touched row 0");
        }
    }

    #[test]
    fn write_corruption_copies_the_last_three_words() {
        let mut oam = filled();
        write_corrupt(&mut oam, 3);
        assert_eq!(&oam[3 * ROW + 2..4 * ROW], &filled()[2 * ROW + 2..3 * ROW]);
    }

    #[test]
    fn the_write_expression_is_a_majority_vote() {
        // ((a ^ c) & (b ^ c)) ^ c picks c when a and b disagree with it, and
        // the common value of a and b when they agree.
        let mut oam = vec![0u8; ROWS * ROW];
        oam[ROW] = 0b1010_1010; // a: current row, first word
        oam[0] = 0b1100_1100; // b: preceding row, first word
        oam[THIRD_WORD] = 0b1111_0000; // c: preceding row, third word
        write_corrupt(&mut oam, 1);
        assert_eq!(oam[ROW], 0b1110_1000);
    }

    #[test]
    fn the_read_expression_is_b_or_a_and_c() {
        let mut oam = vec![0u8; ROWS * ROW];
        oam[ROW] = 0b1010_1010;
        oam[0] = 0b0000_1111;
        oam[THIRD_WORD] = 0b1111_0000;
        read_corrupt(&mut oam, 1);
        assert_eq!(oam[ROW], 0b1010_1111);
    }

    #[test]
    fn read_write_leaves_the_first_four_and_the_last_row_to_a_plain_read() {
        for row in [1usize, 2, 3, ROWS - 1] {
            let mut rw = filled();
            let mut plain = filled();
            corrupt(&mut rw, row, OamBug::ReadWrite);
            read_corrupt(&mut plain, row);
            assert_eq!(rw, plain, "row {row} took the extra pattern");
        }
    }

    #[test]
    fn read_write_copies_the_preceding_row_three_ways() {
        let mut oam = filled();
        corrupt(&mut oam, 6, OamBug::ReadWrite);
        // Rows 4, 5 and 6 all end up holding the (corrupted) row 5, except
        // for the first word of row 6 which the trailing read corruption
        // rewrites.
        assert_eq!(&oam[4 * ROW..5 * ROW], &oam[5 * ROW..6 * ROW]);
        assert_eq!(&oam[6 * ROW + 2..7 * ROW], &oam[5 * ROW + 2..6 * ROW]);
    }

    #[test]
    fn a_row_past_the_end_is_ignored() {
        let mut oam = filled();
        corrupt(&mut oam, ROWS, OamBug::Write);
        assert_eq!(oam, filled());
    }
}

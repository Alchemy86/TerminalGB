//! Binary-coded decimal, the way the Game Boy does it.
//!
//! HP, the radness score and the total score are all two-byte little-endian
//! BCD in the original, manipulated with `add`/`sub` followed by `daa`. The
//! scores are *displayed* by splitting each byte into two nibbles and adding
//! the tile id of `0`, so the packing is not an implementation detail we are
//! free to change: `$60,$00` is the number 6000 and nothing else.
//!
//! [`daa`] is the real instruction rather than decimal arithmetic on `u8`,
//! because one caller depends on its exact flag behaviour:
//! `SurfingMinigame_AddRadnessToTotal` does `sub $1` / `daa` / `sbc $0` /
//! `daa`, and the borrow has to survive the first `daa` to reach the `sbc`.

/// The Game Boy's `DAA`, given the flags its operand-producing instruction
/// left behind. Returns the adjusted accumulator and the new carry.
///
/// Both conditions are tested against the *pre-adjustment* accumulator and a
/// single correction is applied, which is what the hardware does; applying the
/// `$60` correction first and then re-testing the low nibble would differ on
/// operands like `$9a`.
#[inline]
pub fn daa(a: u8, n: bool, h: bool, c: bool) -> (u8, bool) {
    let mut adjust: u8 = 0;
    let mut carry = c;
    if n {
        // After a subtraction the correction only ever subtracts, and `daa`
        // never *sets* carry here — it leaves whatever the `sub`/`sbc` left.
        if c {
            adjust |= 0x60;
        }
        if h {
            adjust |= 0x06;
        }
        (a.wrapping_sub(adjust), carry)
    } else {
        if c || a > 0x99 {
            adjust |= 0x60;
            carry = true;
        }
        if h || (a & 0x0f) > 0x09 {
            adjust |= 0x06;
        }
        (a.wrapping_add(adjust), carry)
    }
}

/// `adc` then `daa`: decimal addition of two BCD bytes. Returns the sum and
/// the carry out, which is the next byte's carry in.
#[inline]
pub fn add(a: u8, b: u8, carry_in: bool) -> (u8, bool) {
    let cin = carry_in as u16;
    let sum = a as u16 + b as u16 + cin;
    let h = (a & 0x0f) + (b & 0x0f) + cin as u8 > 0x0f;
    daa(sum as u8, false, h, sum > 0xff)
}

/// `sbc` then `daa`: decimal subtraction. Returns the difference and the
/// borrow out.
#[inline]
pub fn sub(a: u8, b: u8, borrow_in: bool) -> (u8, bool) {
    let bin = borrow_in as u16;
    let diff = (a as u16).wrapping_sub(b as u16).wrapping_sub(bin);
    let h = (a & 0x0f) < (b & 0x0f) + bin as u8;
    daa(diff as u8, true, h, (a as u16) < b as u16 + bin)
}

/// A two-byte little-endian BCD counter — `$60,$00` reads as 6000.
///
/// Layout matches the original exactly: `[0]` is the low pair of digits,
/// `[1]` the high pair, and the printer emits `[1]` then `[0]`.
pub type Bcd16 = [u8; 2];

/// Add `amount` (itself BCD) to a counter, saturating at 9999 the way
/// `SurfingMinigame_AddRadness` and `_AddPointsToTotal` both do — on carry out
/// of the high byte they overwrite *both* bytes with `$99`.
pub fn add_saturating(counter: &mut Bcd16, amount: u8) {
    let (lo, carry) = add(counter[0], amount, false);
    counter[0] = lo;
    let (hi, carry) = add(counter[1], 0, carry);
    counter[1] = hi;
    if carry {
        counter[0] = 0x99;
        counter[1] = 0x99;
    }
}

/// Subtract one, as `SurfingMinigame_AddRadnessToTotal` does. Returns `false`
/// when the counter was already zero and nothing was subtracted.
pub fn dec(counter: &mut Bcd16) -> bool {
    if counter[0] == 0 && counter[1] == 0 {
        return false;
    }
    let (lo, borrow) = sub(counter[0], 1, false);
    let (hi, _) = sub(counter[1], 0, borrow);
    counter[0] = lo;
    counter[1] = hi;
    true
}

/// The numeric value of a counter, for tests and the status line. The game
/// itself never needs this — it prints nibbles.
pub fn value(counter: &Bcd16) -> u16 {
    let digit = |b: u8, shift: u32| ((b >> shift) & 0x0f) as u16;
    digit(counter[1], 4) * 1000
        + digit(counter[1], 0) * 100
        + digit(counter[0], 4) * 10
        + digit(counter[0], 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decimal arithmetic on valid BCD is what `daa` is *for*, so the two must
    /// agree across the whole valid domain. Where they disagree the hardware
    /// is right and this test is the record of it.
    #[test]
    fn daa_matches_decimal_over_valid_bcd() {
        for a in 0u8..100 {
            for b in 0u8..100 {
                let (av, bv) = (((a / 10) << 4) | (a % 10), ((b / 10) << 4) | (b % 10));
                let (sum, carry) = add(av, bv, false);
                let expect = a as u16 + b as u16;
                assert_eq!(
                    (((sum >> 4) & 0xf) * 10 + (sum & 0xf)) as u16
                        + if carry { 100 } else { 0 },
                    expect,
                    "{a} + {b}"
                );
                let (diff, borrow) = sub(av, bv, false);
                let expect = (a as i16 - b as i16).rem_euclid(100) as u16;
                assert_eq!(
                    (((diff >> 4) & 0xf) * 10 + (diff & 0xf)) as u16,
                    expect,
                    "{a} - {b}"
                );
                assert_eq!(borrow, b > a, "{a} - {b} borrow");
            }
        }
    }

    #[test]
    fn hp_starts_at_6000_and_ticks_down() {
        let mut hp: Bcd16 = [0x00, 0x60];
        assert_eq!(value(&hp), 6000);
        assert!(dec(&mut hp));
        assert_eq!(hp, [0x99, 0x59]);
        assert_eq!(value(&hp), 5999);
    }

    #[test]
    fn score_saturates_at_9999_rather_than_wrapping() {
        let mut total: Bcd16 = [0x50, 0x99];
        add_saturating(&mut total, 0x50);
        assert_eq!(value(&total), 9999);
        add_saturating(&mut total, 0x50);
        assert_eq!(value(&total), 9999);
    }

    #[test]
    fn dec_reports_empty_rather_than_underflowing() {
        let mut c: Bcd16 = [0x01, 0x00];
        assert!(dec(&mut c));
        assert_eq!(value(&c), 0);
        assert!(!dec(&mut c));
        assert_eq!(value(&c), 0);
    }
}

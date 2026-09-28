//! 8.8 fixed point — the only numeric format the original uses for motion.
//!
//! The disassembly writes speeds and scroll rates as RGBDS fixed-point
//! literals (`ld de, 1.5`, `ld hl, -0.5`, `ld a, 0.25`). RGBDS was assembling
//! them at `OPT Q.8`, which is provable from the code rather than assumed:
//! `SurfingPikachuMinigame_LoadGFXAndLayout` writes `0.25` into the **low**
//! byte of the 16-bit `wSurfingMinigamePikachuSpeed` and zero into the high
//! byte, and `SurfingMinigame_SpeedUpPikachu` adds a literal commented
//! "increase speed by 1/128 per frame". Both only hold if one unit is 1/256.
//!
//! So every fixed-point literal below is `value * 256` in a `u16`, and every
//! operation wraps exactly like the `add hl, de` it replaces.

/// A 16-bit 8.8 fixed-point quantity: high byte whole, low byte 1/256ths.
pub type Fx = u16;

/// `0.25` — the starting speed, and the speed a wipeout resets you to.
pub const START_SPEED: Fx = 0x0040;
/// `0.0078125` = 1/128 — the per-frame acceleration while riding.
pub const ACCEL: Fx = 0x0002;
/// `0.25` — the floor `ReduceSpeedBy64` refuses to underflow past.
pub const SPEED_STEP_64: Fx = 0x0040;
/// `0.5` — the floor `ReduceSpeedBy128` refuses to underflow past.
pub const SPEED_STEP_128: Fx = 0x0080;
/// `1.5` pixels per frame — the wave's scroll rate. Note it is **constant**:
/// Pikachu's speed does not scroll the wave faster, it only covers the course
/// faster. See [`crate::game`].
pub const SCROLL_PER_FRAME: Fx = 0x0180;
/// `9.0` pixels per frame — the coast after the goal.
pub const COAST_PER_FRAME: Fx = 0x0900;
/// `0.5` — the per-frame change in jump velocity, up and down.
pub const JUMP_GRAVITY: Fx = 0x0080;

/// Whole part — the `ld a, h` half of every `add hl, de` pair.
#[inline]
pub const fn whole(v: Fx) -> u8 {
    (v >> 8) as u8
}

/// Fractional part — the `ld a, l` half.
#[inline]
pub const fn frac(v: Fx) -> u8 {
    v as u8
}

/// Rebuild an 8.8 value from the two bytes the original keeps apart.
#[inline]
pub const fn from_parts(whole: u8, frac: u8) -> Fx {
    ((whole as u16) << 8) | frac as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literals_are_q8_8() {
        // These are the assertions that pin the format: if RGBDS had been at
        // its 16.16 default every one of them would be wrong by 256x.
        assert_eq!(START_SPEED, 256 / 4);
        assert_eq!(ACCEL, 256 / 128);
        assert_eq!(SCROLL_PER_FRAME, 256 + 128);
        assert_eq!(COAST_PER_FRAME, 9 * 256);
        assert_eq!(JUMP_GRAVITY, 256 / 2);
    }

    #[test]
    fn parts_round_trip() {
        for v in [0u16, 1, 0x0040, 0x0180, 0x1234, 0xffff] {
            assert_eq!(from_parts(whole(v), frac(v)), v);
        }
    }
}

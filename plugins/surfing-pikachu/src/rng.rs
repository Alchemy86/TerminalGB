//! The wave chooser's random byte.
//!
//! **This is the one behaviour deliberately substituted rather than ported.**
//! `SurfingMinigame_RunGame` calls pokered's `Random`, which folds `rDIV` —
//! the hardware divider register, i.e. how many cycles the console has been
//! running — into a pair of running bytes. Off a real Game Boy there is
//! nothing to fold, and a course that cannot be replayed cannot be tested.
//!
//! So the port takes a seed and produces the same course every time from it.
//! Everything the byte then feeds is ported exactly: it is consumed by
//! `SurfingMinigame_ChooseNextWaveSequence` as `(random - 1) & 7`, with zero
//! meaning "stay on open water", and that mapping is unchanged.

/// A seeded xorshift32. Small, reproducible, and good enough for a chooser
/// that only uses three bits of it.
#[derive(Clone, Debug)]
pub struct Rng(u32);

impl Rng {
    /// Seed the generator. Zero is remapped, because xorshift is stuck there.
    pub fn new(seed: u32) -> Self {
        Rng(if seed == 0 { 0x9e37_79b9 } else { seed })
    }

    /// The next byte, standing in for `Random`'s return value.
    pub fn next_u8(&mut self) -> u8 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x >> 24) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let a: Vec<u8> = (0..64).map(|_| Rng::new(7).next_u8()).collect();
        let mut r = Rng::new(7);
        let b: Vec<u8> = (0..64).map(|_| r.next_u8()).collect();
        assert_eq!(a[0], b[0]);
        let mut r2 = Rng::new(7);
        let c: Vec<u8> = (0..64).map(|_| r2.next_u8()).collect();
        assert_eq!(b, c);
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        let differ = (0..32).filter(|_| a.next_u8() != b.next_u8()).count();
        assert!(differ > 20, "streams barely differ: {differ}/32");
    }

    #[test]
    fn all_eight_sequences_are_reachable() {
        // The chooser only ever looks at `(byte - 1) & 7`, so a generator
        // whose low bits were poor would silently shrink the course to one or
        // two wave shapes.
        let mut r = Rng::new(0xc0ffee);
        let mut seen = [false; 8];
        for _ in 0..4096 {
            let b = r.next_u8();
            if b != 0 {
                seen[((b - 1) & 7) as usize] = true;
            }
        }
        assert!(seen.iter().all(|s| *s), "unreachable sequences: {seen:?}");
    }
}

//! A tiny seeded PRNG, so noise (and anything else that wants randomness) is
//! reproducible from a spec — the same argument [`crate::wav`] and every other
//! deterministic-render module in this crate makes.
//!
//! SplitMix64 (Steele, Lea & Flood, "Fast Splittable Pseudorandom Number
//! Generators", OOPSLA 2014) — a public, widely reimplemented 64-bit
//! generator, good enough for audio texture and small enough to read in one
//! screen. Not cryptographic; nothing here needs that.

/// A SplitMix64 generator.
#[derive(Debug, Clone)]
pub struct Rng(u64);

const GOLDEN_GAMMA: u64 = 0x9E37_79B9_7F4A_7C15;

impl Rng {
    /// Seed of `0` is not a degenerate all-zero state — it is folded through
    /// the golden-ratio constant once before the first draw, the same
    /// precaution every SplitMix64 port takes.
    pub fn new(seed: u64) -> Self {
        Rng(seed ^ GOLDEN_GAMMA)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(GOLDEN_GAMMA);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A uniform sample in `[-1.0, 1.0)` — white noise's amplitude at one
    /// instant.
    pub fn uniform_bipolar(&mut self) -> f64 {
        let u = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64; // [0,1)
        u * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        for _ in 0..64 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::new(1);
        let mut b = Rng::new(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn uniform_bipolar_stays_in_range() {
        let mut r = Rng::new(7);
        for _ in 0..10_000 {
            let v = r.uniform_bipolar();
            assert!((-1.0..1.0).contains(&v), "{v} out of range");
        }
    }

    #[test]
    fn zero_seed_is_not_degenerate() {
        let mut r = Rng::new(0);
        let first = r.next_u64();
        assert_ne!(first, 0);
    }
}

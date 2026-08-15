//! The dice, and the honesty rules around them.
//!
//! # What the cartridge does
//!
//! `Random_` (`engine/math/random.asm`) is four instructions:
//!
//! ```text
//! ldh a, [rDIV]      ld b, a
//! ldh a, [hRandomAdd]   adc b     ldh [hRandomAdd], a
//! ldh a, [rDIV]      ld b, a
//! ldh a, [hRandomSub]   sbc b     ldh [hRandomSub], a
//! ```
//!
//! and `Random` (`home/random.asm`) returns `hRandomAdd`. So the sequence is
//! **not** a self-contained PRNG: each call folds in the *hardware divider*,
//! which advances at 16384 Hz and is therefore a function of exactly when the
//! call happened. That is what makes Gen 1's RNG manipulable, and it is why a
//! model of the battle cannot predict the cartridge's rolls without also
//! modelling the emulated clock.
//!
//! Two consequences this module exists to keep straight.
//!
//! **`hRandomSub` moves on every call even though only `hRandomAdd` is
//! returned.** It is read by `AddPartyMon` and others, so the pair is the state,
//! not the single byte.
//!
//! **Both bytes come from one `rDIV` sample.** The two `ldh a, [rDIV]` reads are
//! four cycles apart and `rDIV` ticks every 256, so in practice they read the
//! same value: `add = add + d + c`, `sub = sub - d - c'`. That single-sample
//! fact is what the shiny hunters exploit and is written up in
//! `docs/shiny-hunt.md`.
//!
//! # The honesty rules
//!
//! An agent that wins by steering the dice has produced an interesting result
//! and a dishonest score. So:
//!
//! * every [`Rng`] implementation reports [`Rng::provenance`], and an evaluation
//!   report prints it;
//! * [`Rng::rolls`] counts draws, so two runs that were supposed to be identical
//!   can be shown to have consumed the same dice;
//! * the evaluation protocol ([`crate::eval`]) fixes the seed **per episode**
//!   from the episode index, so an agent cannot be re-rolled until it wins and
//!   the same episode is reproducible by anyone;
//! * an agent that is handed the RNG at all is a *research* configuration, and
//!   [`Provenance::Foreknowledge`] marks it as one. It is never the default.

use core::fmt;

/// Where a stream of battle random bytes came from — printed with every result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// A deterministic stand-in, seeded by the evaluation protocol. Reproducible
    /// by anyone with the seed, and explicitly **not** a claim to reproduce the
    /// cartridge's own sequence.
    Seeded { seed: u64 },
    /// Bytes recorded from a real cartridge, replayed. This is the only
    /// provenance under which a modelled battle can be compared to a real one
    /// turn for turn.
    Recorded { source: String, len: usize },
    /// The cartridge's own `Random_` chain, driven by a supplied `rDIV`
    /// sequence. Faithful to the algorithm; only as faithful to a real run as
    /// the divider samples it is given.
    Modelled { add: u8, sub: u8 },
    /// Fixed bytes chosen by hand — for tests that pin one branch.
    Fixed,
    /// The agent can see the stream. A research configuration for measuring how
    /// much an oracle is worth; **never** a valid competitive score.
    Foreknowledge,
}

impl fmt::Display for Provenance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Provenance::Seeded { seed } => write!(f, "seeded({seed})"),
            Provenance::Recorded { source, len } => {
                write!(f, "recorded({source}, {len} bytes)")
            }
            Provenance::Modelled { add, sub } => {
                write!(f, "modelled(add=${add:02X},sub=${sub:02X})")
            }
            Provenance::Fixed => write!(f, "fixed"),
            Provenance::Foreknowledge => {
                write!(f, "FOREKNOWLEDGE — not a competitive score")
            }
        }
    }
}

/// A source of `BattleRandom` bytes.
///
/// Every draw in the engine goes through this, and every draw is counted, so a
/// turn's dice consumption is observable. That matters more than it sounds: two
/// implementations of the same rule that draw a *different number of times* will
/// diverge from the cartridge on the following turn even when both got this turn
/// right, and the count is what catches it.
pub trait Rng {
    /// One byte, as `BattleRandom` returns.
    fn next_byte(&mut self) -> u8;
    /// How many bytes have been drawn.
    fn rolls(&self) -> u64;
    fn provenance(&self) -> Provenance;
}

/// A deterministic stand-in for evaluation.
///
/// A 64-bit xorshift, folded to a byte. It is **not** the cartridge's sequence
/// and does not pretend to be — its whole job is to be reproducible from a seed
/// so that two agents can be compared on identical dice. Where fidelity to the
/// cartridge matters, use [`RecordedRng`].
#[derive(Clone, Debug)]
pub struct SeededRng {
    state: u64,
    seed: u64,
    rolls: u64,
}

impl SeededRng {
    pub fn new(seed: u64) -> Self {
        // Avoid the xorshift fixed point at zero.
        let state = if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        };
        SeededRng {
            state,
            seed,
            rolls: 0,
        }
    }
}

impl Rng for SeededRng {
    fn next_byte(&mut self) -> u8 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        self.rolls += 1;
        // The high bits of a xorshift are the better-mixed ones.
        (x >> 56) as u8
    }
    fn rolls(&self) -> u64 {
        self.rolls
    }
    fn provenance(&self) -> Provenance {
        Provenance::Seeded { seed: self.seed }
    }
}

/// Replays bytes recorded from a real cartridge.
///
/// This is what makes the differential harness a real comparison: the model is
/// handed the same dice the cartridge drew, so any divergence in the outcome is
/// a divergence in the *rules*, which is the thing being tested.
///
/// Running off the end is not silently wrapped — it returns `0` and sets
/// [`RecordedRng::exhausted`], because a model that drew more dice than the
/// cartridge did has already disagreed and the report must say so.
#[derive(Clone, Debug)]
pub struct RecordedRng {
    bytes: Vec<u8>,
    at: usize,
    source: String,
    exhausted: bool,
}

impl RecordedRng {
    pub fn new(source: impl Into<String>, bytes: Vec<u8>) -> Self {
        RecordedRng {
            bytes,
            at: 0,
            source: source.into(),
            exhausted: false,
        }
    }
    /// True once a draw ran past the end of the recording.
    pub fn exhausted(&self) -> bool {
        self.exhausted
    }
    /// Bytes not yet consumed — a non-zero remainder at the end of a battle
    /// means the model drew *fewer* dice than the cartridge.
    pub fn remaining(&self) -> usize {
        self.bytes.len().saturating_sub(self.at)
    }
}

impl Rng for RecordedRng {
    fn next_byte(&mut self) -> u8 {
        match self.bytes.get(self.at) {
            Some(&b) => {
                self.at += 1;
                b
            }
            None => {
                self.exhausted = true;
                self.at += 1;
                0
            }
        }
    }
    fn rolls(&self) -> u64 {
        self.at as u64
    }
    fn provenance(&self) -> Provenance {
        Provenance::Recorded {
            source: self.source.clone(),
            len: self.bytes.len(),
        }
    }
}

/// The cartridge's own `Random_` chain.
///
/// Faithful to `engine/math/random.asm` including the carry between the add and
/// the subtract; the `rDIV` samples come from `divider`, which the caller
/// supplies (a real emulator can record them; a study of manipulation can invent
/// them). With an empty `divider` it degenerates to `+0`/`-0` per call, which is
/// exactly what the hardware would do with a stopped divider and is the honest
/// answer rather than a substituted pseudo-random one.
#[derive(Clone, Debug)]
pub struct ModelledRng {
    add: u8,
    sub: u8,
    carry: bool,
    divider: Vec<u8>,
    at: usize,
    rolls: u64,
}

impl ModelledRng {
    pub fn new(add: u8, sub: u8, divider: Vec<u8>) -> Self {
        ModelledRng {
            add,
            sub,
            carry: false,
            divider,
            at: 0,
            rolls: 0,
        }
    }

    /// `hRandomSub`, which moves on every call even though only `hRandomAdd` is
    /// returned. Several routines outside battle read it.
    pub fn sub(&self) -> u8 {
        self.sub
    }
}

impl Rng for ModelledRng {
    fn next_byte(&mut self) -> u8 {
        let d = if self.divider.is_empty() {
            0
        } else {
            let v = self.divider[self.at % self.divider.len()];
            self.at += 1;
            v
        };
        // adc b — the carry flowing in is whatever the previous arithmetic left.
        let (a1, c1) = self.add.overflowing_add(d);
        let (a2, c2) = a1.overflowing_add(u8::from(self.carry));
        self.add = a2;
        let carry_out = c1 || c2;
        // sbc b, with the carry from the add. Both bytes read the same rDIV: the
        // two `ldh a, [rDIV]` are four cycles apart and rDIV ticks every 256.
        let (s1, b1) = self.sub.overflowing_sub(d);
        let (s2, b2) = s1.overflowing_sub(u8::from(carry_out));
        self.sub = s2;
        self.carry = b1 || b2;
        self.rolls += 1;
        self.add
    }
    fn rolls(&self) -> u64 {
        self.rolls
    }
    fn provenance(&self) -> Provenance {
        Provenance::Modelled {
            add: self.add,
            sub: self.sub,
        }
    }
}

/// A fixed cycle of bytes, for tests that pin one branch.
#[derive(Clone, Debug)]
pub struct FixedRng {
    bytes: Vec<u8>,
    at: usize,
}

impl FixedRng {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        let bytes = bytes.into();
        assert!(!bytes.is_empty(), "FixedRng needs at least one byte");
        FixedRng { bytes, at: 0 }
    }
    /// Every draw returns the same byte.
    pub fn constant(b: u8) -> Self {
        FixedRng::new(vec![b])
    }
}

impl Rng for FixedRng {
    fn next_byte(&mut self) -> u8 {
        let b = self.bytes[self.at % self.bytes.len()];
        self.at += 1;
        b
    }
    fn rolls(&self) -> u64 {
        self.at as u64
    }
    fn provenance(&self) -> Provenance {
        Provenance::Fixed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_stream_is_reproducible_and_a_different_seed_is_not() {
        let a: Vec<u8> = (0..64).map(|_| SeededRng::new(7).next_byte()).collect();
        let mut r = SeededRng::new(7);
        let b: Vec<u8> = (0..64).map(|_| r.next_byte()).collect();
        assert_eq!(a[0], b[0]);
        let mut r2 = SeededRng::new(7);
        let s1: Vec<u8> = (0..64).map(|_| r2.next_byte()).collect();
        let mut r3 = SeededRng::new(8);
        let s2: Vec<u8> = (0..64).map(|_| r3.next_byte()).collect();
        assert_ne!(s1, s2);
        let mut r4 = SeededRng::new(7);
        let s3: Vec<u8> = (0..64).map(|_| r4.next_byte()).collect();
        assert_eq!(s1, s3);
    }

    /// Running past the end of a recording is a reportable disagreement, not a
    /// silent wrap.
    #[test]
    fn a_recording_that_runs_out_says_so() {
        let mut r = RecordedRng::new("t", vec![1, 2]);
        assert_eq!(r.next_byte(), 1);
        assert_eq!(r.next_byte(), 2);
        assert!(!r.exhausted());
        assert_eq!(r.next_byte(), 0);
        assert!(r.exhausted());
    }

    #[test]
    fn an_unconsumed_recording_remainder_is_visible() {
        let mut r = RecordedRng::new("t", vec![1, 2, 3, 4]);
        r.next_byte();
        assert_eq!(r.remaining(), 3);
    }

    /// `hRandomSub` advances on every call, even though `Random` returns only
    /// `hRandomAdd` — the pair is the state.
    #[test]
    fn the_modelled_chain_moves_both_bytes() {
        let mut r = ModelledRng::new(0x00, 0x00, vec![0x10]);
        let before = r.sub();
        let out = r.next_byte();
        assert_eq!(out, 0x10);
        assert_ne!(r.sub(), before);
        assert_eq!(r.sub(), 0xF0);
    }

    /// With no divider samples the chain stands still — the honest answer for a
    /// stopped clock, not a substituted pseudo-random byte.
    #[test]
    fn a_stopped_divider_stops_the_chain() {
        let mut r = ModelledRng::new(0x42, 0x00, vec![]);
        assert_eq!(r.next_byte(), 0x42);
        assert_eq!(r.next_byte(), 0x42);
    }

    #[test]
    fn every_provenance_prints_something_a_report_can_carry() {
        assert!(format!("{}", Provenance::Seeded { seed: 3 }).contains('3'));
        assert!(
            format!("{}", Provenance::Foreknowledge).contains("not a competitive score")
        );
    }
}

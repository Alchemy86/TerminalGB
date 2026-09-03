//! Oscillators — the sound source at the front of every voice.
//!
//! # Why square and saw are band-limited and triangle is not
//!
//! A phase accumulator's naive square or sawtooth has a genuine
//! discontinuity every cycle — a vertical jump from -1 to 1 in one sample.
//! Sampled at a finite rate, that jump's infinite bandwidth folds back
//! (aliases) into audible, inharmonic tones above a few hundred Hz, which is
//! the "harsh" or "buzzy" quality a cheap software oscillator has that a real
//! analog one does not. [`Waveform::Square`] and [`Waveform::Saw`] are
//! corrected with PolyBLEP (**Poly**nomial **B**and-**L**imited st**E**p;
//! Välimäki & Huovilainen, "Antialiasing Oscillators in Subtractive
//! Synthesis", IEEE Signal Processing Magazine 24(2), 2007): a small
//! polynomial is added in the one or two samples either side of each
//! discontinuity, replacing the vertical jump with a short, band-limited
//! ramp. [`Waveform::Triangle`] is left naive on purpose — a triangle's
//! harmonics already fall off at -12 dB/octave (against a square or saw's
//! -6 dB/octave), so at ordinary musical pitches its aliasing is well below
//! what a listener notices; correcting it would cost real code for no
//! audible gain.
//!
//! # Wavetable
//!
//! [`Waveform::Wavetable`] plays back an arbitrary single-cycle table with
//! linear interpolation between samples — the general case every fixed
//! waveform below is a special instance of. It is what lets a caller design
//! a timbre no named waveform expresses (a Game Boy-style stepped table, for
//! instance) without this crate growing a new `Waveform` variant for it.

use crate::rng::Rng;

/// What an [`Oscillator`] plays. Cheap to construct and cheap to hold in a
/// preset — none of these carry per-instance state, that lives in
/// [`Oscillator`] itself.
#[derive(Debug, Clone)]
pub enum Waveform {
    Sine,
    /// Band-limited sawtooth, ramping from -1 to 1 across each cycle.
    Saw,
    /// Band-limited pulse. `duty` is the fraction of the cycle spent at +1,
    /// in `(0.0, 1.0)` — `0.5` is a plain square wave, values away from that
    /// thin the tone the way a Game Boy's `NRx1` duty setting does.
    Square {
        duty: f64,
    },
    /// Naive (not band-limited — see the module docs) triangle wave.
    Triangle,
    /// White noise: [`Rng::uniform_bipolar`] resampled every call. Ignores
    /// `freq_hz` entirely — noise has no pitch.
    Noise,
    /// An arbitrary single-cycle table, linearly interpolated. Must have at
    /// least 2 samples; a table of one repeated value is silence, which is a
    /// legitimate (if useless) table rather than a panic.
    Wavetable(std::sync::Arc<[f32]>),
}

/// A phase accumulator paired with one [`Waveform`]. Reusable across notes —
/// [`Oscillator::reset_phase`] is what a fresh note trigger calls, not a new
/// oscillator.
#[derive(Debug, Clone)]
pub struct Oscillator {
    waveform: Waveform,
    /// Cycle position, `0.0..1.0`, wrapping.
    phase: f64,
}

impl Oscillator {
    pub fn new(waveform: Waveform) -> Self {
        Oscillator {
            waveform,
            phase: 0.0,
        }
    }

    /// Start the next note's cycle at a fixed point rather than wherever the
    /// last note left off — every voice in [`crate::voice`] does this on
    /// trigger, which is what makes a rendered piece deterministic sample for
    /// sample regardless of how many notes came before it.
    pub fn reset_phase(&mut self) {
        self.phase = 0.0;
    }

    /// Advance by one sample at `freq_hz` against `sample_rate_hz`, returning
    /// the waveform's value in (about) `[-1.0, 1.0]`. `rng` is only consulted
    /// for [`Waveform::Noise`].
    pub fn next(&mut self, freq_hz: f64, sample_rate_hz: f64, rng: &mut Rng) -> f64 {
        if matches!(self.waveform, Waveform::Noise) {
            return rng.uniform_bipolar();
        }
        let dt = (freq_hz / sample_rate_hz).max(0.0);
        let phase = self.phase;
        let out = match &self.waveform {
            Waveform::Sine => (2.0 * std::f64::consts::PI * phase).sin(),
            Waveform::Saw => {
                let naive = 2.0 * phase - 1.0;
                naive - poly_blep(phase, dt)
            }
            Waveform::Square { duty } => {
                let duty = duty.clamp(0.01, 0.99);
                let mut v = if phase < duty { 1.0 } else { -1.0 };
                v += poly_blep(phase, dt);
                v -= poly_blep((phase - duty).rem_euclid(1.0), dt);
                v
            }
            Waveform::Triangle => {
                if phase < 0.5 {
                    4.0 * phase - 1.0
                } else {
                    3.0 - 4.0 * phase
                }
            }
            Waveform::Wavetable(table) => sample_table(table, phase),
            Waveform::Noise => unreachable!("handled above"),
        };
        self.phase = (self.phase + dt).rem_euclid(1.0);
        out
    }
}

/// Välimäki & Huovilainen's polynomial correction. `t` is the phase distance
/// (in cycles) past the discontinuity, `dt` the phase increment for one
/// sample — so the correction is only nonzero in the one sample before and
/// the one sample after the jump.
fn poly_blep(t: f64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let t = t / dt;
        t + t - t * t - 1.0
    } else if t > 1.0 - dt {
        let t = (t - 1.0) / dt;
        t * t + t + t + 1.0
    } else {
        0.0
    }
}

/// Linear interpolation into a single-cycle table at cycle position `phase`.
fn sample_table(table: &[f32], phase: f64) -> f64 {
    if table.len() < 2 {
        return table.first().copied().unwrap_or(0.0) as f64;
    }
    let n = table.len();
    let pos = phase.rem_euclid(1.0) * n as f64;
    let i0 = pos.floor() as usize % n;
    let i1 = (i0 + 1) % n;
    let frac = pos - pos.floor();
    let a = table[i0] as f64;
    let b = table[i1] as f64;
    a + (b - a) * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peak(waveform: Waveform, freq_hz: f64, sr: f64, n: usize) -> f64 {
        let mut osc = Oscillator::new(waveform);
        let mut rng = Rng::new(1);
        (0..n)
            .map(|_| osc.next(freq_hz, sr, &mut rng).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn every_waveform_stays_within_a_reasonable_range() {
        // PolyBLEP's correction can overshoot by a small margin right at a
        // discontinuity (that overshoot IS the antialiasing — a Gibbs-like
        // ripple replacing an infinite-bandwidth jump), so the bound is
        // generous rather than exactly 1.0.
        for w in [
            Waveform::Sine,
            Waveform::Saw,
            Waveform::Square { duty: 0.5 },
            Waveform::Square { duty: 0.25 },
            Waveform::Triangle,
        ] {
            let p = peak(w, 440.0, 48_000.0, 4_800);
            assert!(p < 1.3, "peak {p} too large");
        }
    }

    #[test]
    fn sine_matches_the_closed_form() {
        let mut osc = Oscillator::new(Waveform::Sine);
        let mut rng = Rng::new(0);
        let sr = 48_000.0;
        let freq = 100.0;
        for i in 0..1000 {
            let got = osc.next(freq, sr, &mut rng);
            let want = (2.0 * std::f64::consts::PI * freq * i as f64 / sr).sin();
            assert!((got - want).abs() < 1e-9, "sample {i}: {got} vs {want}");
        }
    }

    #[test]
    fn a_50_percent_square_is_near_bipolar_and_alternates() {
        let mut osc = Oscillator::new(Waveform::Square { duty: 0.5 });
        let mut rng = Rng::new(0);
        // 4800 Hz at 48 kHz is a 10-sample period, so 200 samples covers 20
        // full cycles — enough to see both halves of the wave, unlike a
        // frequency whose period alone exceeds the sample window.
        let vals: Vec<f64> = (0..200)
            .map(|_| osc.next(4800.0, 48_000.0, &mut rng))
            .collect();
        let high = vals.iter().filter(|&&v| v > 0.5).count();
        let low = vals.iter().filter(|&&v| v < -0.5).count();
        assert!(high > 50 && low > 50, "high={high} low={low}");
    }

    #[test]
    fn noise_ignores_frequency_and_uses_the_rng() {
        let mut osc = Oscillator::new(Waveform::Noise);
        let mut rng = Rng::new(9);
        let mut rng_ref = Rng::new(9);
        for _ in 0..50 {
            assert_eq!(
                osc.next(1234.0, 48_000.0, &mut rng),
                rng_ref.uniform_bipolar()
            );
        }
    }

    #[test]
    fn wavetable_reproduces_a_hand_written_table() {
        let table: std::sync::Arc<[f32]> =
            std::sync::Arc::from(vec![0.0, 1.0, 0.0, -1.0]);
        let mut osc = Oscillator::new(Waveform::Wavetable(table));
        let mut rng = Rng::new(0);
        // One full cycle in exactly 4 samples: freq = sr/4.
        let sr = 48_000.0;
        let freq = sr / 4.0;
        let got: Vec<f64> = (0..4).map(|_| osc.next(freq, sr, &mut rng)).collect();
        assert!((got[0] - 0.0).abs() < 1e-9);
        assert!((got[1] - 1.0).abs() < 1e-9);
        assert!((got[2] - 0.0).abs() < 1e-9);
        assert!((got[3] - (-1.0)).abs() < 1e-9);
    }

    #[test]
    fn reset_phase_returns_to_the_start_of_the_cycle() {
        let mut osc = Oscillator::new(Waveform::Sine);
        let mut rng = Rng::new(0);
        for _ in 0..37 {
            osc.next(220.0, 48_000.0, &mut rng);
        }
        osc.reset_phase();
        let after_reset = osc.next(220.0, 48_000.0, &mut rng);
        let mut fresh = Oscillator::new(Waveform::Sine);
        let first = fresh.next(220.0, 48_000.0, &mut rng);
        assert_eq!(after_reset, first);
    }
}

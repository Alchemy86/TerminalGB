//! A resonant low-pass filter — the single primitive this crate exists to add
//! that the chip-idiom voices it can reproduce (see [`crate::presets`]) never
//! had: a Game Boy pulse channel has no filter stage at all, so its whole
//! timbral palette is "which duty cycle" and "how loud". A filter is what
//! turns "a square wave" into "a square wave with the top end rolled off",
//! which is most of the distance between "chiptune" and "produced".
//!
//! # The formula
//!
//! A textbook two-pole (biquad) IIR low-pass, direct form I, from Robert
//! Bristow-Johnson's "Audio EQ Cookbook" — the reference every subtractive
//! synth's low-pass ultimately traces back to, and the same normalized
//! transfer function *The Audio Programming Book* and essentially every
//! other DSP text present under "recursive/IIR filter design":
//!
//! ```text
//! w0 = 2*pi*f0/fs
//! alpha = sin(w0) / (2*Q)
//! b0 = (1 - cos(w0)) / 2
//! b1 =  1 - cos(w0)
//! b2 = (1 - cos(w0)) / 2
//! a0 =  1 + alpha
//! a1 = -2*cos(w0)
//! a2 =  1 - alpha
//! ```
//!
//! with every coefficient normalized by `a0` before use, and the recurrence
//! `y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]`.
//! `Q` (resonance) above about `0.707` (the maximally-flat/Butterworth
//! value) puts a peak at the cutoff — the "resonant" part of "resonant
//! low-pass".

/// A stateful biquad. Holds the two-sample history the recurrence needs, so
/// (unlike [`crate::osc::Oscillator`], which is stateless between calls to
/// [`crate::osc::Oscillator::next`] beyond its own phase) a filter must be
/// re-created — or explicitly [`Biquad::reset`] — at the start of each note,
/// or the previous note's tail leaks into the next one's attack.
#[derive(Debug, Clone, Copy)]
pub struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Biquad {
    /// A resonant low-pass at `cutoff_hz` against `sample_rate_hz`. `q` below
    /// `~0.5` over-damps (a gentler roll-off than one pole would give is not
    /// achievable this way — `q` has a practical floor), `0.707` is flat,
    /// higher values ring at the cutoff.
    pub fn lowpass(cutoff_hz: f64, q: f64, sample_rate_hz: f64) -> Self {
        let mut f = Biquad {
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        };
        f.set_lowpass(cutoff_hz, q, sample_rate_hz);
        f
    }

    /// Recompute this filter's coefficients for a new cutoff/Q, **without**
    /// touching its `x`/`y` history — the operation a modulated filter
    /// (an envelope or an LFO sweeping the cutoff) actually needs every
    /// sample. Calling [`Biquad::lowpass`] again instead would silently
    /// reset the filter's memory each time, which turns a sweep into
    /// something closer to noise gated on and off than a moving filter —
    /// this is the fix for exactly that bug, found by
    /// `tests/pieces.rs`/`presets::tests` while building
    /// [`crate::presets::analog_pad`].
    pub fn set_lowpass(&mut self, cutoff_hz: f64, q: f64, sample_rate_hz: f64) {
        let nyquist = sample_rate_hz * 0.5;
        // A cutoff at or past Nyquist has no meaning for this design (w0
        // would reach or pass pi); clamp well inside it rather than let the
        // trig blow up.
        let f0 = cutoff_hz.clamp(1.0, nyquist * 0.999);
        let q = q.max(0.1);
        let w0 = 2.0 * std::f64::consts::PI * f0 / sample_rate_hz;
        let (sin_w0, cos_w0) = w0.sin_cos();
        let alpha = sin_w0 / (2.0 * q);

        let b0 = (1.0 - cos_w0) / 2.0;
        let b1 = 1.0 - cos_w0;
        let b2 = (1.0 - cos_w0) / 2.0;
        let a0 = 1.0 + alpha;
        let a1 = -2.0 * cos_w0;
        let a2 = 1.0 - alpha;

        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = a1 / a0;
        self.a2 = a2 / a0;
    }

    /// Process one sample.
    pub fn process(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Clear the filter's memory (but keep its coefficients) — call this
    /// between notes rather than rebuilding coefficients you already have.
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive the filter with a steady sine at a frequency well below cutoff
    /// and one well above it; a low-pass should pass the first close to unity
    /// gain and attenuate the second substantially. This is the property
    /// that actually defines "low-pass" — checking it end to end is a
    /// stronger proof than checking any individual coefficient.
    fn steady_state_gain(cutoff_hz: f64, q: f64, sr: f64, tone_hz: f64) -> f64 {
        let mut f = Biquad::lowpass(cutoff_hz, q, sr);
        let n = (sr * 0.5) as usize; // half a second: settle, then measure
        let settle = n / 2;
        let mut peak_out = 0.0f64;
        let mut peak_in = 0.0f64;
        for i in 0..n {
            let x = (2.0 * std::f64::consts::PI * tone_hz * i as f64 / sr).sin();
            let y = f.process(x);
            if i >= settle {
                peak_out = peak_out.max(y.abs());
                peak_in = peak_in.max(x.abs());
            }
        }
        peak_out / peak_in
    }

    #[test]
    fn passes_low_frequencies_near_unity_gain() {
        let g =
            steady_state_gain(2000.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0, 200.0);
        assert!(
            (0.8..1.2).contains(&g),
            "gain {g} out of range for a passband tone"
        );
    }

    #[test]
    fn attenuates_frequencies_well_above_cutoff() {
        let g =
            steady_state_gain(500.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0, 8000.0);
        assert!(g < 0.15, "gain {g} too high for a stopband tone");
    }

    #[test]
    fn higher_q_boosts_the_response_right_at_cutoff() {
        let flat =
            steady_state_gain(1000.0, std::f64::consts::FRAC_1_SQRT_2, 48_000.0, 1000.0);
        let resonant = steady_state_gain(1000.0, 4.0, 48_000.0, 1000.0);
        assert!(
            resonant > flat,
            "resonant {resonant} should exceed flat {flat} at cutoff"
        );
    }

    #[test]
    fn reset_clears_history_without_needing_new_coefficients() {
        let mut f = Biquad::lowpass(1000.0, 0.9, 48_000.0);
        for _ in 0..100 {
            f.process(1.0);
        }
        assert_ne!(
            f.process(0.0),
            0.0,
            "filter should still be ringing before reset"
        );
        f.reset();
        let after_reset = f.process(0.0);
        assert_eq!(
            after_reset, 0.0,
            "a silent input into a freshly reset filter is silent"
        );
    }

    #[test]
    fn set_lowpass_changes_coefficients_but_keeps_history() {
        let mut f = Biquad::lowpass(1000.0, 0.9, 48_000.0);
        // Ring the filter up with some real signal so it has non-zero state.
        for i in 0..50 {
            f.process((i as f64 * 0.7).sin());
        }
        let before = (f.x1, f.x2, f.y1, f.y2);
        f.set_lowpass(3000.0, 0.9, 48_000.0);
        let after = (f.x1, f.x2, f.y1, f.y2);
        assert_eq!(before, after, "set_lowpass must not disturb x1/x2/y1/y2");
    }

    #[test]
    fn a_swept_cutoff_via_set_lowpass_has_no_reset_clicks() {
        // Sweep the cutoff every sample the way `voice::Instrument` does for
        // a filter envelope, and check the output never jumps by more than a
        // modest amount sample to sample — a per-sample `Biquad::lowpass`
        // rebuild (discarding history) produces exactly this kind of large
        // discontinuous jump; `set_lowpass` must not.
        let mut f = Biquad::lowpass(200.0, 2.0, 48_000.0);
        let mut prev = 0.0;
        let mut max_jump = 0.0f64;
        for i in 0..4800 {
            let cutoff = 200.0 + 3000.0 * (i as f64 / 4800.0);
            f.set_lowpass(cutoff, 2.0, 48_000.0);
            let x = ((i as f64) * 0.9).sin() * 0.5;
            let y = f.process(x);
            if i > 0 {
                max_jump = max_jump.max((y - prev).abs());
            }
            prev = y;
        }
        assert!(
            max_jump < 0.5,
            "unexpectedly large sample-to-sample jump: {max_jump}"
        );
    }

    #[test]
    fn never_produces_a_non_finite_output_for_reasonable_input() {
        let mut f = Biquad::lowpass(3000.0, 5.0, 44_100.0);
        for i in 0..10_000 {
            let x = if i % 7 == 0 { 1.0 } else { -0.3 };
            let y = f.process(x);
            assert!(y.is_finite(), "sample {i}: {y}");
        }
    }
}

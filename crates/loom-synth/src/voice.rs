//! [`Instrument`] — a named, reusable timbre. This is the abstraction the
//! captain's amendment asked for by name: something a piece can *ask for* by
//! preset rather than every voice being a bespoke function (the shape
//! [`crate::presets`] is deliberately moving away from for anything new,
//! while still reproducing the old one faithfully — see that module).
//!
//! An instrument is: one oscillator, an amplitude envelope, an optional
//! filter with its own envelope over the cutoff, optional vibrato, and a
//! gain. [`Instrument::render_note`] is the whole rendering contract: give it
//! a pitch and a duration, get back exactly the samples that note produces —
//! deterministic, with no global state, so a [`crate::sequence::Piece`] can
//! call it once per note in any order and get the same piece back every time.

use crate::envelope::Adsr;
use crate::filter::Biquad;
use crate::osc::{Oscillator, Waveform};
use crate::rng::Rng;

/// How a filter's cutoff moves over a note, layered on top of its base
/// cutoff: `base_cutoff_hz + env_amount_hz * envelope(t)`. `env_amount_hz`
/// may be negative (a filter that *closes* as the note holds).
#[derive(Debug, Clone)]
pub struct FilterSpec {
    pub base_cutoff_hz: f64,
    pub q: f64,
    pub env_amount_hz: f64,
    pub env: Adsr,
}

/// A reusable timbre: what to oscillate, how to shape it, and how (if at
/// all) to filter it.
#[derive(Debug, Clone)]
pub struct Instrument {
    pub waveform: Waveform,
    pub amp_env: Adsr,
    pub filter: Option<FilterSpec>,
    /// Vibrato rate in Hz; `0.0` disables it regardless of `vibrato_depth`.
    pub vibrato_hz: f64,
    /// Vibrato depth as a fraction of the note's own frequency — matching
    /// the `vib / freq` shaping ShowReel's reference chiptune voices use
    /// (see [`crate::presets`]), so a chiptune lead's wobble reproduces at
    /// the same audible depth however high or low the note is.
    pub vibrato_depth: f64,
    pub gain: f32,
}

impl Instrument {
    pub fn new(waveform: Waveform, amp_env: Adsr) -> Self {
        Instrument {
            waveform,
            amp_env,
            filter: None,
            vibrato_hz: 0.0,
            vibrato_depth: 0.0,
            gain: 1.0,
        }
    }

    pub fn with_filter(mut self, filter: FilterSpec) -> Self {
        self.filter = Some(filter);
        self
    }

    pub fn with_vibrato(mut self, hz: f64, depth: f64) -> Self {
        self.vibrato_hz = hz;
        self.vibrato_depth = depth;
        self
    }

    pub fn with_gain(mut self, gain: f32) -> Self {
        self.gain = gain;
        self
    }

    /// Render one note: `freq_hz` held for `gate_secs`, then released. The
    /// buffer is `gate_secs + amp_env.release_secs` samples — see
    /// [`Adsr::render`] for why the release tail is not optional.
    ///
    /// `rng` is threaded in rather than owned, so a whole [`crate::sequence::Piece`]
    /// shares one seeded stream across every noise-using note — the same
    /// determinism argument [`crate::rng::Rng`] documents.
    pub fn render_note(
        &self,
        freq_hz: f64,
        gate_secs: f64,
        sample_rate_hz: f64,
        rng: &mut Rng,
    ) -> Vec<f32> {
        let amp = self.amp_env.render(gate_secs, sample_rate_hz);
        let n = amp.len();
        let mut osc = Oscillator::new(self.waveform.clone());
        osc.reset_phase();

        let mut filter = self
            .filter
            .as_ref()
            .map(|f| Biquad::lowpass(f.base_cutoff_hz, f.q, sample_rate_hz));
        let filter_env = self
            .filter
            .as_ref()
            .map(|f| f.env.render(gate_secs, sample_rate_hz));

        let vib_hz = self.vibrato_hz;
        let vib_depth = self.vibrato_depth;

        let mut out = vec![0.0f32; n];
        for i in 0..n {
            let t = i as f64 / sample_rate_hz;
            let f = if vib_hz > 0.0 && vib_depth > 0.0 {
                freq_hz
                    + freq_hz
                        * vib_depth
                        * (2.0 * std::f64::consts::PI * vib_hz * t).sin()
            } else {
                freq_hz
            };
            let mut s = osc.next(f, sample_rate_hz, rng);

            if let (Some(filt), Some(fenv)) = (filter.as_mut(), filter_env.as_ref()) {
                let spec = self.filter.as_ref().unwrap();
                // The filter envelope's own release need not match the
                // amplitude envelope's — a note can go on ringing after its
                // filter envelope has finished. Once past the end of `fenv`,
                // hold at 0.0 (fully released), which settles the cutoff at
                // `base_cutoff_hz` rather than indexing out of bounds.
                let env_v = fenv.get(i).copied().unwrap_or(0.0) as f64;
                let cutoff = (spec.base_cutoff_hz + spec.env_amount_hz * env_v)
                    .clamp(20.0, sample_rate_hz * 0.45);
                // `set_lowpass`, not a fresh `Biquad::lowpass` — the latter
                // would reset the filter's x/y history every sample, which
                // very nearly shipped here (see `filter.rs`'s own tests for
                // the bug that caught it).
                filt.set_lowpass(cutoff, spec.q, sample_rate_hz);
                s = filt.process(s);
            }

            out[i] = (s * amp[i] as f64 * self.gain as f64) as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_note_length_matches_the_envelopes() {
        let inst = Instrument::new(Waveform::Sine, Adsr::linear(0.01, 0.01, 0.6, 0.05));
        let mut rng = Rng::new(1);
        let out = inst.render_note(440.0, 0.2, 48_000.0, &mut rng);
        let want = ((0.2 + 0.05) * 48_000.0f64).round() as usize;
        assert_eq!(out.len(), want);
    }

    #[test]
    fn silence_at_the_very_start_and_the_very_end() {
        let inst = Instrument::new(
            Waveform::Square { duty: 0.5 },
            Adsr::linear(0.005, 0.0, 1.0, 0.02),
        );
        let mut rng = Rng::new(1);
        let out = inst.render_note(220.0, 0.3, 48_000.0, &mut rng);
        assert!(
            out[0].abs() < 0.05,
            "first sample {} should be near silent",
            out[0]
        );
        assert!(
            out.last().copied().unwrap().abs() < 0.02,
            "last sample should have released"
        );
    }

    #[test]
    fn a_filtered_instrument_stays_bounded_and_finite() {
        let inst = Instrument::new(Waveform::Saw, Adsr::linear(0.02, 0.05, 0.7, 0.2))
            .with_filter(FilterSpec {
                base_cutoff_hz: 400.0,
                q: 3.0,
                env_amount_hz: 3000.0,
                env: Adsr::linear(0.01, 0.3, 0.2, 0.2),
            });
        let mut rng = Rng::new(2);
        let out = inst.render_note(110.0, 0.5, 48_000.0, &mut rng);
        for &s in &out {
            assert!(s.is_finite());
            assert!(s.abs() < 4.0, "unexpectedly large sample {s}"); // resonance can overshoot 1.0, but not by much
        }
    }

    #[test]
    fn determinism_same_inputs_same_output() {
        let inst = Instrument::new(Waveform::Noise, Adsr::pluck(0.0, 0.1)).with_gain(0.8);
        let mut r1 = Rng::new(99);
        let mut r2 = Rng::new(99);
        let a = inst.render_note(0.0, 0.1, 48_000.0, &mut r1);
        let b = inst.render_note(0.0, 0.1, 48_000.0, &mut r2);
        assert_eq!(a, b);
    }

    #[test]
    fn vibrato_makes_the_signal_differ_from_the_unmodulated_version() {
        let plain = Instrument::new(Waveform::Sine, Adsr::linear(0.0, 0.0, 1.0, 0.0));
        let vib = plain.clone().with_vibrato(6.0, 0.02);
        let mut r1 = Rng::new(1);
        let mut r2 = Rng::new(1);
        let a = plain.render_note(440.0, 0.5, 48_000.0, &mut r1);
        let b = vib.render_note(440.0, 0.5, 48_000.0, &mut r2);
        assert_ne!(a, b);
    }
}

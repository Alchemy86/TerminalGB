//! ADSR — the four-stage amplitude (or filter-cutoff) contour every
//! synthesised note is shaped by: how fast it arrives, how far it settles
//! back before the held level, how long it holds there, how it lets go.
//!
//! This is the standard shape used across essentially every subtractive
//! synthesiser and every audio-programming text that covers one (Boulanger &
//! Lazzarini's *The Audio Programming Book* included) — four line segments,
//! not a novel design. What is worth being precise about is what each stage
//! actually measures, because "attack" and "decay" get used loosely:
//!
//! - **Attack** (seconds): silence to peak (`1.0`).
//! - **Decay** (seconds): peak down to `sustain`.
//! - **Sustain** (level, `0.0..=1.0`): held for as long as the note's gate
//!   stays on — this is a *level*, the only stage of the four that is not a
//!   duration.
//! - **Release** (seconds): `sustain` (or wherever the envelope was, if the
//!   gate closes before decay finishes — see [`Adsr::render`]) down to `0.0`.

/// One ADSR contour. Reused across every note an [`crate::voice::Instrument`]
/// plays; it carries no per-note state itself, [`Adsr::render`] returns a
/// fresh sample buffer each time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adsr {
    pub attack_secs: f64,
    pub decay_secs: f64,
    pub sustain_level: f64,
    pub release_secs: f64,
    /// Segment shape: `1.0` is a straight line (the textbook default);
    /// higher values bow the curve concave making the early part of the
    /// stage move faster and the tail linger — the "exponential-feeling"
    /// envelope an analog capacitor-based design would produce, without
    /// modelling a capacitor. Implemented as `progress.powf(shape)` inside
    /// each segment, so `1.0` reproduces the plain linear ramp exactly.
    pub shape: f64,
}

impl Adsr {
    pub const fn linear(
        attack_secs: f64,
        decay_secs: f64,
        sustain_level: f64,
        release_secs: f64,
    ) -> Self {
        Adsr {
            attack_secs,
            decay_secs,
            sustain_level,
            release_secs,
            shape: 1.0,
        }
    }

    pub const fn shaped(
        attack_secs: f64,
        decay_secs: f64,
        sustain_level: f64,
        release_secs: f64,
        shape: f64,
    ) -> Self {
        Adsr {
            attack_secs,
            decay_secs,
            sustain_level,
            release_secs,
            shape,
        }
    }

    /// A short, percussive contour with no sustain — [`Adsr::linear`] with
    /// `decay_secs` doing the whole job and `sustain_level` at `0.0`, which
    /// is how a drum or a plucked chip note is shaped: it has decayed away
    /// to nothing by the time `decay_secs` is up, so `release_secs` is never
    /// actually reached while the gate is held for at least that long.
    pub const fn pluck(attack_secs: f64, decay_secs: f64) -> Self {
        // `f64::max` is not `const fn` on the toolchain this crate pins to
        // (see `rust-toolchain` at the workspace root), hence the manual
        // comparison rather than `decay_secs.max(0.001)`.
        let release_secs = if decay_secs > 0.001 {
            decay_secs
        } else {
            0.001
        };
        Adsr::linear(attack_secs, decay_secs, 0.0, release_secs)
    }

    /// Render the envelope for a note held for `gate_secs` then released.
    /// The returned buffer is `gate_secs + release_secs` long (in samples),
    /// which is the note's *audible* length — a caller renders the
    /// oscillator/filter for exactly this many samples, not for `gate_secs`
    /// alone, or the release tail is silently cut off.
    ///
    /// If `gate_secs` is shorter than attack+decay, the envelope releases
    /// from wherever it had reached rather than snapping to `sustain_level`
    /// first — a real envelope has no "wait for decay to finish" step, the
    /// gate can close at any instant.
    pub fn render(&self, gate_secs: f64, sample_rate_hz: f64) -> Vec<f32> {
        let gate_secs = gate_secs.max(0.0);
        let release_secs = self.release_secs.max(0.0);
        let total_n = ((gate_secs + release_secs) * sample_rate_hz)
            .round()
            .max(0.0) as usize;
        let gate_n = (gate_secs * sample_rate_hz).round() as usize;
        let attack_n = (self.attack_secs.max(0.0) * sample_rate_hz).round() as usize;
        let decay_n = (self.decay_secs.max(0.0) * sample_rate_hz).round() as usize;
        let release_n = total_n.saturating_sub(gate_n).max(1);
        let sustain = self.sustain_level.clamp(0.0, 1.0);
        let shape = self.shape.max(0.05);

        let mut out = vec![0.0f32; total_n];
        // Value the envelope has reached at the instant the gate closes —
        // needed so release starts from the real level, not from `sustain`.
        let mut level_at_gate_close = sustain;
        for (i, o) in out.iter_mut().enumerate().take(gate_n) {
            let v = if i < attack_n {
                (i as f64 / attack_n.max(1) as f64).powf(shape)
            } else if i < attack_n + decay_n {
                let p = (i - attack_n) as f64 / decay_n.max(1) as f64;
                1.0 - (1.0 - sustain) * p.powf(shape)
            } else {
                sustain
            };
            level_at_gate_close = v;
            *o = v as f32;
        }
        for (k, o) in out.iter_mut().enumerate().skip(gate_n) {
            let p = ((k - gate_n) as f64 / release_n as f64).powf(shape);
            *o = (level_at_gate_close * (1.0 - p)).max(0.0) as f32;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_envelope_reaches_full_scale_at_the_end_of_attack() {
        let env = Adsr::linear(0.1, 0.0, 1.0, 0.0);
        let sr = 1000.0;
        let out = env.render(0.1, sr);
        assert!((out.last().copied().unwrap() - 1.0).abs() < 0.01);
        assert!(out[0] < 0.1);
    }

    #[test]
    fn decay_settles_at_the_sustain_level_and_holds() {
        let env = Adsr::linear(0.0, 0.05, 0.4, 0.0);
        let sr = 1000.0;
        let out = env.render(0.2, sr);
        // Well past decay, still within the gate: should sit at ~0.4.
        let mid = out[150];
        assert!((mid - 0.4).abs() < 0.02, "{mid}");
    }

    #[test]
    fn release_fades_to_silence_by_the_end_of_the_buffer() {
        let env = Adsr::linear(0.0, 0.0, 0.8, 0.1);
        let sr = 1000.0;
        let out = env.render(0.05, sr);
        assert!((out.last().copied().unwrap()).abs() < 0.01);
    }

    #[test]
    fn releasing_before_decay_finishes_starts_from_the_real_level_not_sustain() {
        // Attack instant, decay very long, sustain low, gate closes mid-decay.
        let env = Adsr::linear(0.0, 1.0, 0.1, 0.05);
        let sr = 1000.0;
        // Gate closes at 0.1s, i.e. 10% through a 1s decay from 1.0 -> 0.1:
        // level at that instant is roughly 1.0 - 0.9*0.1 = 0.91, not 0.1.
        let out = env.render(0.1, sr);
        let at_gate_close = out[99];
        assert!(
            at_gate_close > 0.7,
            "expected still-high level, got {at_gate_close}"
        );
    }

    #[test]
    fn total_length_is_gate_plus_release() {
        let env = Adsr::linear(0.01, 0.01, 0.5, 0.2);
        let sr = 48_000.0;
        let out = env.render(0.5, sr);
        let want = ((0.5 + 0.2) * sr).round() as usize;
        assert_eq!(out.len(), want);
    }

    #[test]
    fn shape_one_is_a_straight_line() {
        let env = Adsr::linear(0.1, 0.0, 1.0, 0.0);
        let sr = 1000.0;
        let out = env.render(0.1, sr);
        // Roughly linear: the value halfway through attack is close to 0.5.
        let half = out[49];
        assert!((half - 0.5).abs() < 0.05, "{half}");
    }

    #[test]
    fn never_produces_nan_or_out_of_range_values() {
        for shape in [0.05, 1.0, 4.0] {
            let env = Adsr::shaped(0.02, 0.03, 0.6, 0.15, shape);
            let out = env.render(0.01, 44_100.0); // gate shorter than attack+decay
            for &v in &out {
                assert!(
                    v.is_finite() && (0.0..=1.01).contains(&v),
                    "{v} shape={shape}"
                );
            }
        }
    }
}

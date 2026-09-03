//! Compare two renders quantitatively — this is what
//! `loom-synth compare a.wav b.wav` runs, and it is how this crate's
//! chiptune preset (see [`crate::presets`]) is checked against a render
//! ShowReel's own, unmodified `music.rs` produced: not by asserting the
//! samples are identical (they cannot be — this is an independent
//! reimplementation through generalised primitives, not a port), but by
//! measuring whether the two pieces move together in loudness over time,
//! which is what "the same song, played through different machinery" should
//! look like.
//!
//! Two measurements, both windowed so a single loud instant cannot dominate:
//!
//! - **Envelope correlation** — the Pearson correlation of the two signals'
//!   windowed RMS loudness curves. High correlation means notes land and
//!   decay at the same moments in both renders — the strongest, simplest
//!   evidence that the same pattern is driving both.
//! - **Brightness correlation** — the same correlation, but of each window's
//!   *balance* between energy above and below 1 kHz (a cheap one-pole
//!   high-pass rather than a full spectral analysis — proportionate to what
//!   this check needs, see the module's own reasoning below). Two pieces
//!   with the same drum/bass/lead mix landing at the same times should
//!   brighten and darken together too.

/// RMS of `samples` inside non-overlapping windows of `window_len` samples
/// (the last, possibly short, window is still included).
pub fn windowed_rms(samples: &[f32], window_len: usize) -> Vec<f64> {
    if window_len == 0 || samples.is_empty() {
        return Vec::new();
    }
    samples
        .chunks(window_len)
        .map(|w| {
            let sum_sq: f64 = w.iter().map(|&s| (s as f64) * (s as f64)).sum();
            (sum_sq / w.len() as f64).sqrt()
        })
        .collect()
}

/// A cheap "how bright is this window" proxy: a one-pole high-pass at 1 kHz
/// run once over the whole buffer, then windowed RMS of *that*, divided by
/// the plain windowed RMS — i.e. what fraction of each window's energy sits
/// above roughly 1 kHz. A full spectral centroid would need an FFT this
/// crate has no other use for; this single-pole ratio is enough to tell a
/// bass-heavy window from a hat-heavy one, which is all the comparison needs.
pub fn windowed_brightness(samples: &[f32], window_len: usize) -> Vec<f64> {
    if samples.is_empty() {
        return Vec::new();
    }
    // One-pole high-pass: y[n] = a*(y[n-1] + x[n] - x[n-1]), a = RC/(RC+dt).
    // RC chosen for a ~1 kHz corner at 48 kHz (the sample rate every piece in
    // this crate renders at); a slightly wrong corner at another rate only
    // shifts *where* "bright" starts, which does not change whether two
    // curves rise and fall together.
    let dt = 1.0 / 48_000.0;
    let rc = 1.0 / (2.0 * std::f64::consts::PI * 1000.0);
    let a = rc / (rc + dt);
    let mut hp = vec![0.0f64; samples.len()];
    let mut prev_x = 0.0f64;
    let mut prev_y = 0.0f64;
    for (i, &s) in samples.iter().enumerate() {
        let x = s as f64;
        let y = a * (prev_y + x - prev_x);
        hp[i] = y;
        prev_x = x;
        prev_y = y;
    }
    let full = windowed_rms(samples, window_len);
    let bright = windowed_rms_f64(&hp, window_len);
    full.iter()
        .zip(bright.iter())
        .map(|(&f, &b)| if f > 1e-9 { (b / f).min(1.0) } else { 0.0 })
        .collect()
}

fn windowed_rms_f64(samples: &[f64], window_len: usize) -> Vec<f64> {
    if window_len == 0 || samples.is_empty() {
        return Vec::new();
    }
    samples
        .chunks(window_len)
        .map(|w| (w.iter().map(|&s| s * s).sum::<f64>() / w.len() as f64).sqrt())
        .collect()
}

/// Pearson correlation of two equal-length series, `NaN` if either has zero
/// variance (a flat line correlates with nothing meaningfully).
pub fn pearson_correlation(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    if n < 2 {
        return f64::NAN;
    }
    let mean = |s: &[f64]| s[..n].iter().sum::<f64>() / n as f64;
    let ma = mean(a);
    let mb = mean(b);
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for i in 0..n {
        let da = a[i] - ma;
        let db = b[i] - mb;
        cov += da * db;
        va += da * da;
        vb += db * db;
    }
    if va < 1e-12 || vb < 1e-12 {
        return f64::NAN;
    }
    cov / (va.sqrt() * vb.sqrt())
}

/// The full comparison report between two mono buffers at the same sample
/// rate.
#[derive(Debug, Clone)]
pub struct Report {
    pub duration_secs_a: f64,
    pub duration_secs_b: f64,
    pub peak_a: f32,
    pub peak_b: f32,
    pub rms_a: f32,
    pub rms_b: f32,
    pub envelope_correlation: f64,
    pub brightness_correlation: f64,
}

pub fn compare(a: &[f32], b: &[f32], sample_rate_hz: u32) -> Report {
    let window = (sample_rate_hz as f64 * 0.02).round().max(1.0) as usize; // 20ms windows
    let ra = windowed_rms(a, window);
    let rb = windowed_rms(b, window);
    let ba = windowed_brightness(a, window);
    let bb = windowed_brightness(b, window);
    let peak = |s: &[f32]| s.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    let rms = |s: &[f32]| {
        if s.is_empty() {
            0.0
        } else {
            ((s.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / s.len() as f64)
                .sqrt()) as f32
        }
    };
    Report {
        duration_secs_a: a.len() as f64 / sample_rate_hz as f64,
        duration_secs_b: b.len() as f64 / sample_rate_hz as f64,
        peak_a: peak(a),
        peak_b: peak(b),
        rms_a: rms(a),
        rms_b: rms(b),
        envelope_correlation: pearson_correlation(&ra, &rb),
        brightness_correlation: pearson_correlation(&ba, &bb),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signal_correlates_perfectly_with_itself() {
        let sr = 48_000;
        let n = sr as usize * 2;
        let sig: Vec<f32> = (0..n)
            .map(|i| {
                ((i as f64 / sr as f64 * 220.0 * 2.0 * std::f64::consts::PI).sin() * 0.5)
                    as f32
            })
            .collect();
        let r = compare(&sig, &sig, sr);
        assert!(
            (r.envelope_correlation - 1.0).abs() < 1e-9,
            "{}",
            r.envelope_correlation
        );
        assert!(
            (r.brightness_correlation - 1.0).abs() < 1e-6,
            "{}",
            r.brightness_correlation
        );
    }

    #[test]
    fn loudness_envelopes_that_move_together_correlate_highly() {
        let sr = 48_000u32;
        let window = (sr as f64 * 0.02).round() as usize;
        // Same amplitude-modulated tone, one scaled down uniformly — the
        // envelope shape is identical, only the level differs.
        let n = window * 50;
        let a: Vec<f32> = (0..n)
            .map(|i| {
                let env = if (i / window) % 5 < 2 { 1.0 } else { 0.1 };
                (env * (i as f64 * 0.3).sin()) as f32
            })
            .collect();
        let b: Vec<f32> = a.iter().map(|&v| v * 0.3).collect();
        let r = compare(&a, &b, sr);
        assert!(r.envelope_correlation > 0.99, "{}", r.envelope_correlation);
    }

    #[test]
    fn unrelated_envelopes_do_not_correlate_highly() {
        let sr = 48_000u32;
        let window = (sr as f64 * 0.02).round() as usize;
        let n = window * 50;
        // Loud in the first half then silent, versus the exact opposite.
        let a: Vec<f32> = (0..n).map(|i| if i < n / 2 { 0.8 } else { 0.0 }).collect();
        let b: Vec<f32> = (0..n).map(|i| if i < n / 2 { 0.0 } else { 0.8 }).collect();
        let r = compare(&a, &b, sr);
        assert!(r.envelope_correlation < 0.0, "{}", r.envelope_correlation);
    }

    #[test]
    fn brightness_tells_a_bass_tone_from_a_hihat() {
        let sr = 48_000u32;
        let n = sr as usize;
        let low: Vec<f32> = (0..n)
            .map(|i| {
                ((i as f64 / sr as f64 * 80.0 * 2.0 * std::f64::consts::PI).sin() * 0.6)
                    as f32
            })
            .collect();
        let mut rng = crate::rng::Rng::new(1);
        let high: Vec<f32> = (0..n).map(|_| rng.uniform_bipolar() as f32 * 0.6).collect();
        let window = (sr as f64 * 0.02).round() as usize;
        let low_b = windowed_brightness(&low, window);
        let high_b = windowed_brightness(&high, window);
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        assert!(
            mean(&high_b) > mean(&low_b),
            "noise should read brighter than an 80Hz tone"
        );
    }

    #[test]
    fn pearson_of_a_flat_series_is_nan_not_a_panic() {
        let flat = vec![1.0; 10];
        let varying = vec![1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0, 2.0, 3.0, 1.0];
        assert!(pearson_correlation(&flat, &varying).is_nan());
    }
}

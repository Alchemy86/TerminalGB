//! Mixing down to headroom.
//!
//! Additive synthesis (every voice just adds into one buffer, as
//! [`crate::sequence::Piece::render`] does) has an old, simple problem: the
//! peak of a sum is not bounded by the peaks of its parts, so a buffer with
//! four voices comfortably under `1.0` each can still clip once they land on
//! the same beat. The standard, simple fix — and the one this crate uses —
//! is a soft limiter: normalise to the buffer's own peak, push the result
//! through a saturating nonlinearity (`tanh`) so a value approaching the
//! ceiling compresses rather than clips flat, then back the whole thing off
//! to a fixed headroom so the final peak sits a little under full scale
//! rather than exactly at it.

/// Normalise `buf` to its own peak, apply a `tanh` soft knee, and scale to
/// `headroom` (a fraction of full scale, e.g. `0.9`). A silent buffer is left
/// silent rather than dividing by zero.
pub fn soft_limit(buf: &mut [f32], headroom: f32) {
    let peak = buf.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    if peak < 1e-9 {
        return;
    }
    // A little drive into the knee (1.3x) before the tanh, so quieter
    // passages are barely touched and only the loudest moments actually
    // compress — the same shape a limiter with a soft threshold has.
    for v in buf.iter_mut() {
        let normalised = (*v / peak) as f64;
        let limited = (normalised * 1.3).tanh();
        *v = (limited * headroom as f64) as f32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_lands_at_or_under_headroom() {
        let mut buf = vec![0.1, -3.0, 2.5, 0.0, 1.0];
        soft_limit(&mut buf, 0.9);
        let peak = buf.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(peak <= 0.9001, "{peak}");
    }

    #[test]
    fn silence_stays_silent() {
        let mut buf = vec![0.0f32; 100];
        soft_limit(&mut buf, 0.9);
        assert!(buf.iter().all(|&v| v == 0.0));
    }

    #[test]
    fn relative_ordering_of_loud_and_quiet_samples_is_preserved() {
        let mut buf = vec![0.2, 0.5, 1.0];
        soft_limit(&mut buf, 0.9);
        assert!(buf[0] < buf[1]);
        assert!(buf[1] < buf[2]);
    }

    #[test]
    fn no_nan_or_infinite_output() {
        let mut buf = vec![f32::MIN_POSITIVE, 1e6, -1e6, 0.0];
        soft_limit(&mut buf, 0.9);
        assert!(buf.iter().all(|v| v.is_finite()));
    }
}

//! Frame pacing for a browser frontend — a deadline accumulator over whatever
//! clock the page has.
//!
//! ## Why this is not `requestAnimationFrame`
//!
//! `requestAnimationFrame` fires at the **display's** refresh rate, which has
//! nothing to do with 59.7275 Hz. Stepping the machine once per callback — what
//! the web frontend did — therefore ran it at:
//!
//! | display | emulated rate | what a player gets |
//! |---|---|---|
//! | 59.94 Hz | 59.94 Hz | +0.36 % |
//! | 60 Hz | 60 Hz | +0.46 % |
//! | 120 Hz | **120 Hz** | the game at **double speed** |
//! | 144 Hz | **144 Hz** | 2.4× |
//!
//! A high-refresh laptop or phone is completely ordinary now, so this is not a
//! corner case. It is also what made web audio impossible to get right before:
//! the APU emits a fixed number of samples per *emulated* second, so a machine
//! running 2× fast produces audio twice as fast as any sound device can play
//! it, and the only outcomes are unbounded latency or a queue that sheds audio
//! in lumps.
//!
//! The fix is the same deadline accumulator the terminal frontend uses (see
//! `docs/audio.md`, "never reset a pacing timer to `now()`"): advance the
//! deadline by exact periods, and run however many emulated frames are due.
//! **`std::time::Instant` is not available here** — it panics on
//! `wasm32-unknown-unknown` — so the clock is passed in, which is also what
//! lets the browser path pace on `AudioContext.currentTime` and share one
//! clock with the audio consumer.

/// A true DMG frame: 70224 cycles / 4.194304 MHz = 59.7275 Hz.
pub const FRAME_SECONDS: f64 = 16_743.0 / 1_000_000.0;

/// Emulated frames one callback may run before the pacer gives up and
/// resynchronises.
///
/// A backgrounded tab stops `requestAnimationFrame` altogether. Without a cap,
/// the first callback after it comes back would try to replay every frame of
/// the gap in one go — minutes of emulation inside one event handler, i.e. a
/// hung page. The terminal frontend caps for the same reason (at 12).
///
/// **This cap is also the ceiling on what any audio cushion can rescue**: a
/// gap longer than `MAX_CATCHUP` frames (134 ms) is resynchronised, so the
/// audio for it is never produced and no amount of scheduling lead can hide
/// it. `sound::web_audio::TARGET_LEAD` is sized just under this. Eight frames
/// cost about 2.4 ms of emulation inside one callback, so the cap is generous
/// on purpose.
pub const MAX_CATCHUP: u32 = 8;

/// Deadline accumulator. `None` until the first tick establishes an origin.
#[derive(Debug, Default, Clone, Copy)]
pub struct FramePacer {
    deadline: Option<f64>,
    /// Times the pacer had to give up on a gap and resynchronise.
    pub resyncs: u64,
}

impl FramePacer {
    pub fn new() -> FramePacer {
        FramePacer::default()
    }

    /// How many emulated frames are due at `now` (seconds, any monotonic
    /// clock), advancing the deadline by that many exact periods.
    ///
    /// Zero is a normal answer and means "draw, do not step" — that is the
    /// whole of the 120 Hz fix.
    pub fn frames_due(&mut self, now: f64) -> u32 {
        let deadline = *self.deadline.get_or_insert(now);
        if now < deadline {
            return 0;
        }
        let elapsed = now - deadline;
        // +1 because reaching the deadline is itself one frame due.
        let due = (elapsed / FRAME_SECONDS).floor() as u32 + 1;
        if due >= MAX_CATCHUP {
            // Fell far behind. Resynchronise to now rather than replaying the
            // gap: the deadline must never carry a debt it cannot pay off.
            self.resyncs += 1;
            self.deadline = Some(now);
            return MAX_CATCHUP;
        }
        self.deadline = Some(deadline + FRAME_SECONDS * f64::from(due));
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `hz` callbacks a second for `seconds` and count emulated frames.
    fn emulated_frames_at(hz: f64, seconds: f64) -> u32 {
        let mut p = FramePacer::new();
        let mut total = 0;
        let step = 1.0 / hz;
        let ticks = (seconds * hz).round() as u32;
        for i in 0..ticks {
            total += p.frames_due(f64::from(i) * step);
        }
        total
    }

    /// **The bug this exists for.** A 120 Hz display must not run the game at
    /// double speed. Teeth: step once per callback instead (the old loop) and
    /// this reads 1200 rather than ~597.
    #[test]
    fn a_fast_display_does_not_speed_the_machine_up() {
        for hz in [60.0, 75.0, 120.0, 144.0, 165.0, 240.0] {
            let frames = emulated_frames_at(hz, 10.0);
            let expected = 10.0 / FRAME_SECONDS; // ~597.3
            let err = (f64::from(frames) - expected).abs() / expected;
            assert!(
                err < 0.01,
                "{hz} Hz display produced {frames} frames in 10 s, expected ~{expected:.0}"
            );
        }
    }

    /// A display slower than 59.7275 Hz still gets real-time emulation, by
    /// running more than one frame per callback.
    #[test]
    fn a_slow_display_still_runs_at_real_time() {
        let frames = emulated_frames_at(30.0, 10.0);
        let expected = 10.0 / FRAME_SECONDS;
        assert!(
            (f64::from(frames) - expected).abs() / expected < 0.01,
            "30 Hz display produced {frames} frames in 10 s, expected ~{expected:.0}"
        );
    }

    /// A backgrounded tab is a gap, not a debt. Coming back from a minute
    /// away must cost one capped callback, not a minute of emulation inside
    /// one event handler.
    #[test]
    fn a_long_gap_is_capped_and_resynchronised_not_replayed() {
        let mut p = FramePacer::new();
        assert_eq!(p.frames_due(0.0), 1);
        assert_eq!(p.frames_due(60.0), MAX_CATCHUP);
        assert_eq!(p.resyncs, 1);
        // And the deadline now starts from where we came back, so the next
        // callback one period later is due exactly one frame.
        assert_eq!(p.frames_due(60.0 + FRAME_SECONDS), 1);
    }

    /// The deadline advances by exact periods; it is never reset to `now` on
    /// an ordinary tick. Resetting is what makes a paced loop run a few per
    /// cent slow, which drains an audio queue (`docs/audio.md`).
    #[test]
    fn the_deadline_does_not_drift_with_callback_jitter() {
        let mut p = FramePacer::new();
        let mut total = 0;
        // 60 Hz nominal with ±4 ms of deterministic jitter on every callback.
        for i in 0..600u32 {
            let jitter = if i % 2 == 0 { 0.004 } else { -0.004 };
            total += p.frames_due(f64::from(i) / 60.0 + jitter);
        }
        let expected = (600.0 / 60.0) / FRAME_SECONDS;
        assert!(
            (f64::from(total) - expected).abs() / expected < 0.01,
            "jittered 60 Hz produced {total} frames, expected ~{expected:.0}"
        );
    }
}

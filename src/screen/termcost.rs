//! What the *terminal* costs us per displayed frame — measured, not assumed.
//!
//! # The counter that could not go down
//!
//! Since emulation was decoupled from rendering, the frontend's `fps` counted
//! **display frames we issued**: one per `terminal.draw` plus image write. Both
//! of those are ours and both are fast — with the Kitty shared-memory transport
//! a frame is about eighty bytes on the wire and 0.15 ms of our CPU — so the
//! number sat at 59 whatever the terminal did with them, and the whole of the
//! displayed frame that a *user* experiences happened after we stopped looking.
//! A counter that cannot go down is not a counter.
//!
//! # A true present count does not exist, and this says so
//!
//! No terminal protocol reports "I painted your image":
//!
//! * The Kitty graphics protocol's response acknowledges the **command** — it
//!   is sent when the transmission has been received and decoded, and the spec
//!   describes no display acknowledgement at all.
//! * A DSR/CPR round trip (`CSI 6 n`) is a **fence**: the terminal answers it
//!   after working through everything written before it. That is strictly
//!   stronger than "we wrote bytes" — it includes the terminal's own decode and
//!   composite — but it still cannot report a frame the terminal *coalesced*
//!   and never painted, because from its side that frame was processed.
//! * Write back-pressure on the pty measures nothing here: eighty bytes never
//!   fills a pty buffer, so the write always returns instantly.
//!
//! So the honest quantity is not a present count, it is **how long the terminal
//! takes to accept one frame** — which is a hard ceiling on how many it can
//! possibly present, and is what this module measures. The frontend then paces
//! drawing to that ceiling, which is what makes the issued-frame counter true
//! instead of merely lower.
//!
//! # What it measures, on real hardware
//!
//! WezTerm on a 5120×2880 display, kitty `t=s`, every frame changed
//! (`render_bench --tty --motion`; the full ladder is in `docs/rendering.md`
//! §11, including the measurement trap that produced a wrong answer first).
//! The payload is flat at 86–88 bytes across the whole range, so every
//! millisecond below is the terminal's:
//!
//! ```text
//! window (cells)   picture (px)   fence ms   terminal ms   ceiling
//!  40 × 12           516 ×  563     3.44        0.04        —
//!  80 × 20           840 ×  939     3.77        0.37       2700 fps
//! 140 × 34          1452 × 1596     4.45        1.05        950 fps
//! 200 × 48          2044 × 2253     5.25        1.85        540 fps
//! 268 × 60 (full)   2540 × 2816     6.37        2.97        337 fps
//! ```
//!
//! That is ~0.42 ms per megapixel of composited picture — linear in **area**,
//! independent of bytes. It is the shape the frontend must respect; the
//! constant belongs to the machine, and this is why the figure is measured on
//! the user's box rather than assumed from ours. On *this* one the terminal is
//! nowhere near the bottleneck at any size that fits the screen: reaching a
//! 16.74 ms budget would take roughly 40 megapixels of picture.
//!
//! # How the sample is taken, and why it is not taken every frame
//!
//! A fence blocks. Blocking the loop every frame would starve the audio queue
//! on a terminal slow enough to matter (the cushion is ~41 ms — see
//! `docs/audio.md`), and it would add its own round trip to every displayed
//! frame: fencing per frame caps the fullscreen figure above at 143 fps purely
//! because of the 3.4 ms round trip, not because the terminal cannot do more.
//! But the quantity is a smooth function of the displayed area and only moves
//! when the window or the scene does, so it is sampled on a **layout change**
//! and re-checked every [`RESAMPLE_AFTER`]. Steady-state cost: nothing at all.
//!
//! Each sample takes [`SAMPLES`] fences and keeps the **minimum** — a fence is
//! a round trip through a scheduler and the maximum is pure noise (measured
//! 3.4 ms mean against a 12.4 ms max with nothing to draw).
//!
//! The fixed part of that round trip — pty latency plus the terminal's input
//! loop — is not the terminal's *frame* work, so it is measured once at startup
//! with nothing to draw and subtracted. Measured on WezTerm it is 3.35 ms at
//! 80×24 and 3.39 ms at fullscreen: flat across a 25× change in composited
//! area, which is what a fixed latency looks like and why subtracting a
//! constant is the right correction.

use std::time::{Duration, Instant};

/// How many fences one sample averages over. The minimum of these is kept.
pub const SAMPLES: usize = 3;

/// How long a sample stays good.
///
/// Short enough to follow the *scene*, not just the window: a sample taken
/// while the game screen was static measures the terminal's idle marginal cost,
/// which is optimistic for a scene where every frame changes. Long enough that
/// the blocking cost stays negligible — three fences every three seconds.
pub const RESAMPLE_AFTER: Duration = Duration::from_secs(3);

/// Give up after this many fences in a row fail. A terminal that stops
/// answering must not cost a stall per draw for the rest of the session.
const MAX_FAILURES: u32 = 3;

/// A fence longer than this is not a measurement of the terminal's frame work.
///
/// An occluded, iconified or unmapped window stops presenting, and everything
/// queued for it backs up; so does a machine that started swapping. Committing
/// such a sample would pace the display to single-figure fps and hold it there
/// until the next re-sample. It is discarded instead, which self-heals as soon
/// as the window is visible again.
const SANE_MAX: Duration = Duration::from_millis(500);

/// The terminal's own per-frame cost, and the evidence for it.
///
/// Construct with [`TerminalCost::new`], feed it [`TerminalCost::observe`] after
/// each picture goes out, and read [`TerminalCost::per_frame`] /
/// [`TerminalCost::ceiling_fps`].
pub struct TerminalCost {
    /// False when there is nothing to measure with — the terminal never
    /// answered a cursor report at startup, or the user turned fencing off.
    enabled: bool,
    /// Fixed round-trip latency of a fence with nothing to draw.
    floor: Duration,
    /// Terminal work per displayed frame at [`TerminalCost::layout`].
    per_frame: Option<Duration>,
    /// Which display rectangle (in cells) the current figure belongs to.
    layout: Option<(u16, u16)>,
    /// When the current figure was committed.
    taken: Option<Instant>,
    /// Fences collected towards the next figure.
    pending: Vec<Duration>,
    failures: u32,
    /// Total fences that returned an answer — telemetry, and the honest way to
    /// say "this figure is measured" rather than "assumed".
    fences: u64,
}

impl TerminalCost {
    /// `enabled` should be the startup probe's verdict on whether this terminal
    /// answers `CSI 6 n` at all; `floor` the fixed round-trip latency it
    /// measured (pass [`Duration::ZERO`] if it did not).
    pub fn new(enabled: bool, floor: Duration) -> TerminalCost {
        TerminalCost {
            enabled,
            floor,
            per_frame: None,
            layout: None,
            taken: None,
            pending: Vec::with_capacity(SAMPLES),
            failures: 0,
            fences: 0,
        }
    }

    /// A cost model that measures nothing, for a terminal that cannot be asked.
    pub fn unmeasurable() -> TerminalCost {
        TerminalCost::new(false, Duration::ZERO)
    }

    /// Whether the current figure was measured rather than assumed. **Report
    /// this beside the number** — an unmeasured ceiling is not a ceiling.
    pub fn measured(&self) -> bool {
        self.per_frame.is_some()
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn fences(&self) -> u64 {
        self.fences
    }

    /// The fixed round-trip latency that is subtracted from every sample.
    pub fn floor(&self) -> Duration {
        self.floor
    }

    /// Terminal work per displayed frame, or `None` when it is not measurable.
    pub fn per_frame(&self) -> Option<Duration> {
        self.per_frame
    }

    /// The most displayed frames a second this terminal can accept at this
    /// size. `None` when unmeasured.
    pub fn ceiling_fps(&self) -> Option<f32> {
        let d = self.per_frame?.as_secs_f32();
        Some(if d <= 0.0 { f32::INFINITY } else { 1.0 / d })
    }

    /// Called immediately after a picture has been written to the terminal.
    ///
    /// `fence` must write a round-trip request and block until the terminal
    /// answers, returning how long that took (or `None` on timeout). It is
    /// called **only** when a fresh sample is due — a layout change or a stale
    /// figure — so in the steady state `observe` does no I/O at all.
    pub fn observe<F>(&mut self, now: Instant, layout: (u16, u16), mut fence: F)
    where
        F: FnMut() -> Option<Duration>,
    {
        if !self.enabled {
            return;
        }
        if self.layout != Some(layout) {
            // A resize changes the composited area, which is the whole of what
            // this measures. Everything known about the old size is void.
            self.layout = Some(layout);
            self.per_frame = None;
            self.taken = None;
            self.pending.clear();
        } else if self.pending.is_empty() {
            match self.taken {
                Some(t) if now.saturating_duration_since(t) < RESAMPLE_AFTER => return,
                Some(_) => {} // stale: fall through and re-sample
                None => {}
            }
        }
        match fence() {
            Some(d) if d > SANE_MAX => {
                // Answered, so the terminal is alive — just not showing us
                // anything useful. Keep fencing enabled and drop the burst.
                self.failures = 0;
                self.fences += 1;
                self.pending.clear();
            }
            Some(d) => {
                self.failures = 0;
                self.fences += 1;
                self.pending.push(d);
                if self.pending.len() >= SAMPLES {
                    // Minimum, not mean: a fence is a round trip through a
                    // scheduler and every millisecond above the floor of the
                    // distribution is somebody else's.
                    let best = self.pending.iter().copied().min().unwrap_or_default();
                    self.per_frame = Some(best.saturating_sub(self.floor));
                    self.taken = Some(now);
                    self.pending.clear();
                }
            }
            None => {
                self.pending.clear();
                self.failures += 1;
                if self.failures >= MAX_FAILURES {
                    // Stop paying a timeout per draw for a terminal that has
                    // gone quiet. The figure stays whatever was last measured,
                    // flagged by `enabled() == false`.
                    self.enabled = false;
                }
            }
        }
    }

    /// How long to leave between draws so we never issue a picture faster than
    /// the terminal can take one. `frame_time` is the display's own period; the
    /// answer is never shorter than that.
    pub fn draw_interval(&self, frame_time: Duration) -> Duration {
        match self.per_frame {
            Some(d) if d > frame_time => d,
            _ => frame_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_figure_needs_a_full_burst_and_keeps_the_minimum() {
        let mut c = TerminalCost::new(true, ms(3));
        let t = Instant::now();
        let mut give = vec![ms(40), ms(34), ms(60)].into_iter();
        for _ in 0..SAMPLES {
            assert!(!c.measured(), "committed before the burst finished");
            c.observe(t, (100, 40), || give.next());
        }
        // min 34 ms less the 3 ms fixed round trip.
        assert_eq!(c.per_frame(), Some(ms(31)));
        assert!(c.measured());
        assert!((c.ceiling_fps().unwrap() - 32.25).abs() < 0.1);
    }

    #[test]
    fn the_steady_state_costs_no_round_trips_at_all() {
        // The whole reason this is sampled rather than fenced per frame: a
        // fence blocks the loop, and at fullscreen that is 34 ms.
        let mut c = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        let mut calls = 0;
        for _ in 0..500 {
            c.observe(t, (100, 40), || {
                calls += 1;
                Some(ms(30))
            });
        }
        assert_eq!(calls, SAMPLES, "fenced {calls} times, not {SAMPLES}");
    }

    #[test]
    fn a_resize_voids_the_figure_immediately() {
        // The measurement *is* the composited area, so a figure from another
        // window size is not a stale figure, it is a wrong one.
        let mut c = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        for _ in 0..SAMPLES {
            c.observe(t, (100, 40), || Some(ms(30)));
        }
        assert_eq!(c.per_frame(), Some(ms(30)));
        c.observe(t, (200, 80), || Some(ms(60)));
        assert!(!c.measured(), "kept a figure measured at another size");
        for _ in 0..SAMPLES - 1 {
            c.observe(t, (200, 80), || Some(ms(60)));
        }
        assert_eq!(c.per_frame(), Some(ms(60)));
    }

    #[test]
    fn a_stale_figure_is_remeasured_but_a_fresh_one_is_not() {
        let mut c = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        for _ in 0..SAMPLES {
            c.observe(t, (100, 40), || Some(ms(30)));
        }
        let mut calls = 0;
        c.observe(t + RESAMPLE_AFTER / 2, (100, 40), || {
            calls += 1;
            Some(ms(10))
        });
        assert_eq!(calls, 0, "re-measured a figure that was still fresh");
        for _ in 0..SAMPLES {
            c.observe(t + RESAMPLE_AFTER, (100, 40), || {
                calls += 1;
                Some(ms(10))
            });
        }
        assert_eq!(calls, SAMPLES);
        assert_eq!(c.per_frame(), Some(ms(10)));
    }

    #[test]
    fn a_terminal_that_stops_answering_is_dropped_not_waited_on() {
        let mut c = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        let mut calls = 0;
        for _ in 0..20 {
            c.observe(t, (100, 40), || {
                calls += 1;
                None
            });
        }
        assert_eq!(
            calls, MAX_FAILURES as usize,
            "kept fencing a silent terminal"
        );
        assert!(!c.enabled());
        assert!(!c.measured(), "claimed a measurement it never got");
    }

    /// An occluded window makes the fence blow up, and pacing the game to the
    /// resulting 2 fps and holding it there is the wrong answer. The sample is
    /// dropped and measurement stays live.
    #[test]
    fn an_absurd_sample_is_discarded_not_committed() {
        let mut c = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        for _ in 0..SAMPLES {
            c.observe(t, (100, 40), || Some(ms(20)));
        }
        assert_eq!(c.per_frame(), Some(ms(20)));

        let later = t + RESAMPLE_AFTER;
        for _ in 0..SAMPLES * 2 {
            c.observe(later, (100, 40), || Some(ms(900)));
        }
        assert_eq!(
            c.per_frame(),
            Some(ms(20)),
            "committed an occluded-window sample"
        );
        assert!(c.enabled(), "gave up on a terminal that was answering");

        // ...and it recovers as soon as the window is visible again.
        for _ in 0..SAMPLES {
            c.observe(later, (100, 40), || Some(ms(25)));
        }
        assert_eq!(c.per_frame(), Some(ms(25)));
    }

    #[test]
    fn an_unmeasurable_terminal_reports_nothing_rather_than_guessing() {
        let mut c = TerminalCost::unmeasurable();
        let mut calls = 0;
        c.observe(Instant::now(), (100, 40), || {
            calls += 1;
            Some(ms(30))
        });
        assert_eq!(calls, 0);
        assert_eq!(c.per_frame(), None);
        assert_eq!(c.ceiling_fps(), None);
        assert!(!c.measured());
    }

    #[test]
    fn pacing_never_goes_faster_than_the_display_and_never_slower_than_the_terminal() {
        let frame = Duration::from_micros(16_743);
        // Unmeasured: the display's own period, i.e. exactly today's behaviour.
        assert_eq!(TerminalCost::unmeasurable().draw_interval(frame), frame);

        let mut cheap = TerminalCost::new(true, Duration::ZERO);
        let t = Instant::now();
        for _ in 0..SAMPLES {
            cheap.observe(t, (53, 24), || Some(ms(1)));
        }
        assert_eq!(
            cheap.draw_interval(frame),
            frame,
            "a fast terminal is not a reason to draw slower"
        );

        let mut slow = TerminalCost::new(true, Duration::ZERO);
        for _ in 0..SAMPLES {
            slow.observe(t, (391, 176), || Some(ms(30)));
        }
        assert_eq!(slow.draw_interval(frame), ms(30));
    }
}

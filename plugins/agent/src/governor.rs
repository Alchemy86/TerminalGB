//! Budget, circuit breaker and stuck detection — the three things that stop an
//! unattended agent costing money or achieving nothing.
//!
//! This runs unattended and every call costs real money, so the caps live in
//! the **host**, not in the driver and not in a prompt. A driver cannot raise
//! them, ignore them or fail to implement them: [`Governor::gate`] is asked
//! before any call is started, and it is the only place a call can begin.
//!
//! Four independent ceilings, because they fail in different ways:
//!
//! * **calls** — the count. Catches a fast loop.
//! * **tokens** — usage the driver reports. Catches a slow loop with big prompts.
//! * **spend** — the same in money, when the driver reports a price.
//! * **wall clock** — catches everything else, including a driver that reports
//!   no usage at all. This is the backstop: with no usage reporting the first
//!   three can be evaded, and this one cannot.
//!
//! Plus a **circuit breaker**: consecutive failures open it, and it then stays
//! open for a cooldown before letting exactly one probe call through. That is
//! what stops a broken adapter being retried sixty times a second forever.
//!
//! None of this stops the *emulator*. A halted agent means the game keeps
//! running with nobody at the controller, which is a perfectly good stream.

use std::time::{Duration, Instant};

/// Every ceiling, in one place. The defaults are deliberately modest — an
/// unattended overnight run should annoy you, not bankrupt you.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub max_calls: u32,
    pub max_tokens: u64,
    pub max_usd: f64,
    pub max_wall: Duration,
    /// How long one call may take before the host gives up on it. The driver is
    /// cancelled and the failure counted.
    pub call_timeout: Duration,
    /// Consecutive failures that open the breaker.
    pub max_consecutive_failures: u32,
    /// How long the breaker stays open before one probe call is allowed.
    pub breaker_cooldown: Duration,
    /// Shortest gap between two calls. Zero by default; a positive value is the
    /// cheapest possible defence against a runaway loop.
    pub min_call_interval: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            max_calls: 400,
            max_tokens: 2_000_000,
            max_usd: 5.0,
            max_wall: Duration::from_secs(2 * 60 * 60),
            call_timeout: Duration::from_secs(60),
            max_consecutive_failures: 4,
            breaker_cooldown: Duration::from_secs(60),
            min_call_interval: Duration::ZERO,
        }
    }
}

/// Why the agent has stopped for good.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Halt {
    Calls,
    Tokens,
    Spend,
    Wall,
    /// The breaker opened and the retry budget for reopening is spent.
    Broken,
}

impl Halt {
    /// One short public sentence. Safe to broadcast — no numbers a viewer
    /// cannot interpret, no internals.
    pub fn public_text(self) -> &'static str {
        match self {
            Halt::Calls => "the agent has used up its decision budget",
            Halt::Tokens => "the agent has used up its token budget",
            Halt::Spend => "the agent has reached its spending limit",
            Halt::Wall => "the agent has reached its time limit",
            Halt::Broken => "the agent kept failing, so it has been switched off",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Halt::Calls => "call-cap",
            Halt::Tokens => "token-cap",
            Halt::Spend => "spend-cap",
            Halt::Wall => "time-cap",
            Halt::Broken => "breaker",
        }
    }
}

/// The answer to "may I make a call right now?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Go,
    /// Not yet: rate limit or breaker cooldown.
    Wait,
    /// Never again this session.
    Stop(Halt),
}

/// What one call cost.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub usd: f64,
}

impl Usage {
    pub fn tokens(&self) -> u64 {
        self.input_tokens.saturating_add(self.output_tokens)
    }
}

/// Enforces [`Budget`].
pub struct Governor {
    pub budget: Budget,
    started: Instant,
    calls: u32,
    tokens: u64,
    usd: f64,
    failures: u32,
    consecutive: u32,
    /// The breaker is open until this instant.
    open_until: Option<Instant>,
    /// How many times the breaker has opened. Opening repeatedly means the
    /// adapter is broken, not unlucky.
    trips: u32,
    /// After this many trips the agent is switched off for good.
    max_trips: u32,
    halted: Option<Halt>,
    last_call: Option<Instant>,
    /// Set while a call is in flight, for the timeout.
    in_flight: Option<Instant>,
}

impl Governor {
    pub fn new(budget: Budget) -> Governor {
        Governor {
            budget,
            started: Instant::now(),
            calls: 0,
            tokens: 0,
            usd: 0.0,
            failures: 0,
            consecutive: 0,
            open_until: None,
            trips: 0,
            max_trips: 3,
            halted: None,
            last_call: None,
            in_flight: None,
        }
    }

    /// Restart the wall clock. Used by tests and by a session that starts the
    /// agent some time after the machine booted.
    pub fn reset_clock(&mut self, now: Instant) {
        self.started = now;
    }

    pub fn calls(&self) -> u32 {
        self.calls
    }
    pub fn tokens(&self) -> u64 {
        self.tokens
    }
    pub fn usd(&self) -> f64 {
        self.usd
    }
    pub fn failures(&self) -> u32 {
        self.failures
    }
    pub fn halted(&self) -> Option<Halt> {
        self.halted
    }
    pub fn breaker_open(&self) -> bool {
        self.open_until.is_some()
    }

    /// Fraction of the tightest budget used, 0.0..=1.0 — one number for the
    /// status panel.
    pub fn used_fraction(&self) -> f64 {
        let b = &self.budget;
        let mut worst: f64 = 0.0;
        if b.max_calls > 0 {
            worst = worst.max(self.calls as f64 / b.max_calls as f64);
        }
        if b.max_tokens > 0 {
            worst = worst.max(self.tokens as f64 / b.max_tokens as f64);
        }
        if b.max_usd > 0.0 {
            worst = worst.max(self.usd / b.max_usd);
        }
        let wall = self.budget.max_wall.as_secs_f64();
        if wall > 0.0 {
            worst = worst.max(self.started.elapsed().as_secs_f64() / wall);
        }
        worst.clamp(0.0, 1.0)
    }

    /// May a call start now? **Every call must go through this.**
    pub fn gate(&mut self, now: Instant) -> Gate {
        if let Some(h) = self.halted {
            return Gate::Stop(h);
        }
        // Hard ceilings first: reaching one is permanent.
        let over = if self.calls >= self.budget.max_calls {
            Some(Halt::Calls)
        } else if self.tokens >= self.budget.max_tokens {
            Some(Halt::Tokens)
        } else if self.usd >= self.budget.max_usd {
            Some(Halt::Spend)
        } else if now.saturating_duration_since(self.started) >= self.budget.max_wall {
            Some(Halt::Wall)
        } else {
            None
        };
        if let Some(h) = over {
            self.halted = Some(h);
            return Gate::Stop(h);
        }
        // Breaker.
        if let Some(until) = self.open_until {
            if now < until {
                return Gate::Wait;
            }
            // Half-open: let exactly one probe through.
            self.open_until = None;
        }
        // Rate limit.
        if let Some(last) = self.last_call {
            if now.saturating_duration_since(last) < self.budget.min_call_interval {
                return Gate::Wait;
            }
        }
        Gate::Go
    }

    /// Record that a call has started. Only valid after [`Gate::Go`].
    pub fn call_started(&mut self, now: Instant) {
        self.calls += 1;
        self.last_call = Some(now);
        self.in_flight = Some(now);
    }

    /// Has the in-flight call run past its timeout?
    pub fn call_timed_out(&self, now: Instant) -> bool {
        match self.in_flight {
            Some(t) => now.saturating_duration_since(t) >= self.budget.call_timeout,
            None => false,
        }
    }

    /// A call came back well.
    pub fn call_succeeded(&mut self, usage: Usage) {
        self.in_flight = None;
        self.consecutive = 0;
        self.tokens = self.tokens.saturating_add(usage.tokens());
        self.usd += usage.usd.max(0.0);
    }

    /// A call failed. Returns `true` if that opened the breaker.
    ///
    /// A failure with **no call in flight** is ignored. A driver that reports
    /// one spuriously would otherwise be counted against the breaker on every
    /// frame, which trips it in milliseconds and reads as "the agent is
    /// broken" when the truth is "the driver answered a question nobody
    /// asked". Found by a test driver that did exactly that.
    pub fn call_failed(&mut self, now: Instant) -> bool {
        if self.in_flight.is_none() {
            return false;
        }
        self.in_flight = None;
        self.failures += 1;
        self.consecutive += 1;
        if self.consecutive >= self.budget.max_consecutive_failures {
            self.consecutive = 0;
            self.trips += 1;
            if self.trips > self.max_trips {
                self.halted = Some(Halt::Broken);
            } else {
                self.open_until = Some(now + self.budget.breaker_cooldown);
            }
            return true;
        }
        false
    }

    /// One line for the status panel. Safe to broadcast: counts and
    /// percentages, nothing internal.
    pub fn status(&self) -> String {
        if let Some(h) = self.halted {
            return format!("stopped: {}", h.label());
        }
        if self.breaker_open() {
            return format!("recovering ({} failures)", self.failures);
        }
        format!(
            "{}/{} calls · {}% of budget",
            self.calls,
            self.budget.max_calls,
            (self.used_fraction() * 100.0).round() as u32
        )
    }
}

/// Notices when the agent is getting nowhere.
///
/// Agents walk into walls and reopen the same menu forever, and the failure is
/// silent: each individual decision looks reasonable. The detector compares a
/// **fingerprint of the observable world** — map, coordinates, screen contents —
/// across decisions, and counts how many decisions in a row changed nothing.
///
/// It does not *fix* anything. It escalates, in three stages, because the right
/// response depends on how stuck you are:
///
/// 1. tell the agent it is repeating itself (usually enough — it can see its
///    own last outcome and will try something else),
/// 2. tell it more insistently and forbid repeating the last plan,
/// 3. run a local unstick routine (press B to back out of whatever menu it is
///    trapped in, then move) and start again from a clean state.
#[derive(Debug, Clone, Default)]
pub struct Stuck {
    last: Option<u64>,
    repeats: u32,
    /// Highest repeat count reached this session — a fact for the run summary.
    worst: u32,
}

/// How hard to push back on a stuck agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    None,
    /// Mention it.
    Nudge,
    /// Insist, and tell it not to repeat itself.
    Insist,
    /// Take over locally for one plan.
    Intervene,
}

impl Stuck {
    /// Feed the current world fingerprint. Returns how stuck we now are.
    pub fn observe(&mut self, fingerprint: u64) -> Escalation {
        if self.last == Some(fingerprint) {
            self.repeats += 1;
        } else {
            self.repeats = 0;
            self.last = Some(fingerprint);
        }
        self.worst = self.worst.max(self.repeats);
        self.level()
    }

    pub fn level(&self) -> Escalation {
        match self.repeats {
            0..=1 => Escalation::None,
            2..=3 => Escalation::Nudge,
            4..=6 => Escalation::Insist,
            _ => Escalation::Intervene,
        }
    }

    pub fn repeats(&self) -> u32 {
        self.repeats
    }

    pub fn worst(&self) -> u32 {
        self.worst
    }

    /// Called after an intervention, so the escalation ladder starts again
    /// rather than intervening on every subsequent decision.
    pub fn intervened(&mut self) {
        self.repeats = 0;
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Budget {
        Budget {
            max_calls: 3,
            max_tokens: 100,
            max_usd: 0.10,
            max_wall: Duration::from_secs(10),
            call_timeout: Duration::from_millis(50),
            max_consecutive_failures: 2,
            breaker_cooldown: Duration::from_millis(100),
            min_call_interval: Duration::ZERO,
        }
    }

    #[test]
    fn the_call_cap_stops_the_agent_for_good() {
        let mut g = Governor::new(tiny());
        let now = Instant::now();
        for _ in 0..3 {
            assert_eq!(g.gate(now), Gate::Go);
            g.call_started(now);
            g.call_succeeded(Usage::default());
        }
        assert_eq!(g.gate(now), Gate::Stop(Halt::Calls));
        // And it stays stopped — no amount of waiting reopens a spent budget.
        assert_eq!(
            g.gate(now + Duration::from_secs(3600)),
            Gate::Stop(Halt::Calls)
        );
        assert_eq!(g.halted(), Some(Halt::Calls));
    }

    #[test]
    fn each_ceiling_fires_on_its_own() {
        let now = Instant::now();

        let mut g = Governor::new(tiny());
        g.gate(now);
        g.call_started(now);
        g.call_succeeded(Usage {
            input_tokens: 60,
            output_tokens: 60,
            usd: 0.0,
        });
        assert_eq!(g.gate(now), Gate::Stop(Halt::Tokens));

        let mut g = Governor::new(tiny());
        g.gate(now);
        g.call_started(now);
        g.call_succeeded(Usage {
            usd: 0.5,
            ..Default::default()
        });
        assert_eq!(g.gate(now), Gate::Stop(Halt::Spend));

        let mut g = Governor::new(tiny());
        assert_eq!(
            g.gate(now + Duration::from_secs(11)),
            Gate::Stop(Halt::Wall)
        );
    }

    /// The wall clock is the backstop for a driver that reports no usage at
    /// all — without it, a free-looking driver could run for ever.
    #[test]
    fn a_driver_that_reports_no_usage_is_still_bounded() {
        let mut b = tiny();
        b.max_calls = 1_000_000;
        let mut g = Governor::new(b);
        let now = Instant::now();
        for _ in 0..50 {
            assert_eq!(g.gate(now), Gate::Go);
            g.call_started(now);
            g.call_succeeded(Usage::default());
        }
        assert_eq!(
            g.gate(now + Duration::from_secs(10)),
            Gate::Stop(Halt::Wall)
        );
    }

    #[test]
    fn the_breaker_opens_on_consecutive_failures_and_then_probes_once() {
        let mut g = Governor::new(Budget {
            max_calls: 100,
            ..tiny()
        });
        let t0 = Instant::now();
        g.gate(t0);
        g.call_started(t0);
        assert!(!g.call_failed(t0), "one failure is not a pattern");
        g.gate(t0);
        g.call_started(t0);
        assert!(g.call_failed(t0), "two in a row opens the breaker");
        assert!(g.breaker_open());
        assert_eq!(g.gate(t0), Gate::Wait);
        // After the cooldown exactly one probe is allowed.
        let t1 = t0 + Duration::from_millis(150);
        assert_eq!(g.gate(t1), Gate::Go);
        assert!(!g.breaker_open());
        // A success clears the streak.
        g.call_started(t1);
        g.call_succeeded(Usage::default());
        g.gate(t1);
        g.call_started(t1);
        assert!(!g.call_failed(t1));
    }

    #[test]
    fn a_permanently_broken_driver_is_switched_off_rather_than_retried_forever() {
        let mut g = Governor::new(Budget {
            max_calls: 1000,
            breaker_cooldown: Duration::ZERO,
            ..tiny()
        });
        let mut t = Instant::now();
        for _ in 0..40 {
            if let Gate::Stop(_) = g.gate(t) {
                break;
            }
            g.call_started(t);
            g.call_failed(t);
            t += Duration::from_millis(10);
        }
        assert_eq!(g.halted(), Some(Halt::Broken));
        assert!(g.calls() < 20, "it gave up long before the call cap");
    }

    #[test]
    fn a_failure_with_no_call_outstanding_is_ignored() {
        let mut g = Governor::new(tiny());
        let t = Instant::now();
        for _ in 0..100 {
            assert!(!g.call_failed(t), "must never trip on a phantom failure");
        }
        assert_eq!(g.failures(), 0);
        assert!(!g.breaker_open());
        assert_eq!(g.halted(), None);
    }

    #[test]
    fn a_call_that_never_answers_times_out() {
        let mut g = Governor::new(tiny());
        let t0 = Instant::now();
        g.gate(t0);
        g.call_started(t0);
        assert!(!g.call_timed_out(t0));
        assert!(g.call_timed_out(t0 + Duration::from_millis(60)));
    }

    #[test]
    fn the_rate_limit_spaces_calls_out() {
        let mut g = Governor::new(Budget {
            max_calls: 100,
            min_call_interval: Duration::from_millis(100),
            ..tiny()
        });
        let t0 = Instant::now();
        assert_eq!(g.gate(t0), Gate::Go);
        g.call_started(t0);
        g.call_succeeded(Usage::default());
        assert_eq!(g.gate(t0 + Duration::from_millis(10)), Gate::Wait);
        assert_eq!(g.gate(t0 + Duration::from_millis(120)), Gate::Go);
    }

    #[test]
    fn stuck_detection_escalates_and_then_resets() {
        let mut s = Stuck::default();
        assert_eq!(s.observe(1), Escalation::None);
        assert_eq!(s.observe(2), Escalation::None);
        assert_eq!(s.observe(2), Escalation::None); // 1 repeat
        assert_eq!(s.observe(2), Escalation::Nudge); // 2
        assert_eq!(s.observe(2), Escalation::Nudge); // 3
        assert_eq!(s.observe(2), Escalation::Insist); // 4
        for _ in 0..3 {
            s.observe(2);
        }
        assert_eq!(s.level(), Escalation::Intervene);
        assert_eq!(s.worst(), 7);
        s.intervened();
        assert_eq!(s.level(), Escalation::None);
        // Real progress clears it immediately.
        assert_eq!(s.observe(9), Escalation::None);
    }

    #[test]
    fn the_status_line_is_safe_to_broadcast() {
        let mut g = Governor::new(tiny());
        let now = Instant::now();
        g.gate(now);
        g.call_started(now);
        g.call_succeeded(Usage::default());
        let s = g.status();
        assert!(s.contains("1/3 calls"), "{s}");
        assert!(crate::commentary::sanitise(&s, 200).is_some());
    }
}

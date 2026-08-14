//! The seam between the emulator and whatever is deciding what to do.
//!
//! # The contract, in one line
//!
//! *Here is the state as JSON; give me back JSON with some commentary and a
//! short plan.* Nothing else. The driver never touches the machine, never sees
//! a `Machine` handle, and cannot press a button itself — the host parses its
//! reply, clamps every number in it and performs the plan. That is what makes
//! the driver swappable: Claude, a scripted policy, a random walker, or a human
//! typing JSON all satisfy it, and none of them can do anything the others
//! could not.
//!
//! # It is polled, never awaited
//!
//! [`Driver::submit`] hands over a request and returns **immediately**;
//! [`Driver::poll`] is called once per frame and returns [`Poll::Pending`]
//! until an answer exists. The emulator therefore cannot block on a slow model:
//! a driver that takes ten seconds, or never answers at all, costs exactly one
//! `poll` call per frame. This is the single most important property in the
//! whole design — see `docs/claude-plays.md` §"The emulator never waits".
//!
//! # Replies are raw text on purpose
//!
//! A driver returns the *text* of a reply, not a parsed structure. Parsing,
//! clamping and sanitising then happen in one place for every driver, so the
//! deterministic test driver exercises exactly the code path the real model
//! does — including the leniency for models that wrap JSON in prose.

/// What a driver told the host, before parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Poll {
    /// Nothing outstanding.
    Idle,
    /// A request is in flight.
    Pending,
    /// The reply text.
    Ready(String),
    /// The call failed. The detail is for the log; the panel sees only a
    /// [`crate::agent::commentary::Problem`] category.
    Failed(DriverError),
}

/// Why a call failed. Every variant carries detail **for the log file only**.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverError {
    /// The driver did not answer inside the host's timeout.
    Timeout,
    /// It answered with something that could not be read.
    Unreadable(String),
    /// It could not be run, or it fell over.
    Crashed(String),
}

impl DriverError {
    pub fn category(&self) -> crate::agent::commentary::Problem {
        use crate::agent::commentary::Problem;
        match self {
            DriverError::Timeout => Problem::Timeout,
            DriverError::Unreadable(_) => Problem::Unreadable,
            DriverError::Crashed(_) => Problem::Crashed,
        }
    }

    /// The detail, for the log file. **Never** show this to a viewer.
    pub fn detail(&self) -> String {
        match self {
            DriverError::Timeout => "timeout".to_string(),
            DriverError::Unreadable(d) => format!("unreadable: {d}"),
            DriverError::Crashed(d) => format!("crashed: {d}"),
        }
    }
}

/// Something that decides what to do next.
pub trait Driver: Send {
    /// Short stable name, shown in the status panel.
    fn name(&self) -> &str;

    /// Whether this driver costs real money. The host applies its spend caps to
    /// every driver regardless, but a free driver need not be announced as a
    /// spend risk.
    fn costs_money(&self) -> bool {
        false
    }

    /// Start a call. Returns `false` if it could not be started (already busy,
    /// or the driver is finished). **Must not block.**
    fn submit(&mut self, request: &str) -> bool;

    /// Check for an answer. **Must not block.** Called once per emulated frame.
    fn poll(&mut self) -> Poll;

    /// Abandon anything in flight. Called when the host's timeout fires and
    /// when the session ends.
    fn cancel(&mut self) {}

    /// Anything the last call wrote to its diagnostic channel, consumed by the
    /// host for its **log file**.
    ///
    /// This exists so a subprocess adapter's stderr is not simply thrown away —
    /// it is the only place an operator can see why an adapter is failing. It
    /// must never reach the commentary panel: see
    /// [`crate::agent::commentary`]. Most drivers have nothing to say.
    fn take_stderr(&mut self) -> String {
        String::new()
    }
}

/// A driver that replays a fixed list of replies.
///
/// **Build this first and keep using it.** It is what makes every other part of
/// the system testable without spending a penny: the executor, the governor,
/// the stuck detector, the commentary sanitiser and the whole plugin can be
/// driven end to end against a real cartridge with completely deterministic
/// input. `latency` fakes a slow model in frames, which is how "the emulator
/// never waits" is proved rather than asserted.
pub struct ScriptedDriver {
    name: String,
    replies: Vec<String>,
    next: usize,
    /// Frames a call takes to "arrive".
    latency: u32,
    waited: u32,
    busy: bool,
    /// Replies are reused from the start once exhausted, so a long run does not
    /// need a long script.
    loop_replies: bool,
    finished: bool,
    calls: u32,
}

impl ScriptedDriver {
    pub fn new(replies: Vec<String>) -> ScriptedDriver {
        ScriptedDriver {
            name: "scripted".to_string(),
            replies,
            next: 0,
            latency: 0,
            waited: 0,
            busy: false,
            loop_replies: false,
            finished: false,
            calls: 0,
        }
    }

    /// Make each call take `frames` emulated frames to answer.
    pub fn with_latency(mut self, frames: u32) -> Self {
        self.latency = frames;
        self
    }

    /// Start again at the first reply once the script runs out.
    pub fn looping(mut self) -> Self {
        self.loop_replies = true;
        self
    }

    pub fn with_name(mut self, name: &str) -> Self {
        self.name = name.to_string();
        self
    }

    /// How many calls have been made — the number a test asserts on.
    pub fn calls(&self) -> u32 {
        self.calls
    }
}

impl Driver for ScriptedDriver {
    fn name(&self) -> &str {
        &self.name
    }

    fn submit(&mut self, _request: &str) -> bool {
        if self.busy || self.finished {
            return false;
        }
        if self.next >= self.replies.len() {
            if self.loop_replies && !self.replies.is_empty() {
                self.next = 0;
            } else {
                self.finished = true;
                return false;
            }
        }
        self.busy = true;
        self.waited = 0;
        self.calls += 1;
        true
    }

    fn poll(&mut self) -> Poll {
        if !self.busy {
            return Poll::Idle;
        }
        if self.waited < self.latency {
            self.waited += 1;
            return Poll::Pending;
        }
        self.busy = false;
        let reply = self.replies[self.next].clone();
        self.next += 1;
        Poll::Ready(reply)
    }

    fn cancel(&mut self) {
        self.busy = false;
    }
}

/// A driver that presses buttons at random, deterministically.
///
/// Useful for two things and honest about both: shaking out executor bugs no
/// scripted sequence would reach, and providing a "the emulator is fine, the
/// agent is not" control when a real run goes badly.
pub struct RandomDriver {
    state: u64,
    busy: bool,
}

impl RandomDriver {
    pub fn new(seed: u64) -> RandomDriver {
        RandomDriver {
            // Any non-zero state; a zero seed would otherwise stick at zero.
            state: seed | 1,
            busy: false,
        }
    }

    fn next(&mut self) -> u64 {
        // xorshift64*, so a run is reproducible from its seed.
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

impl Driver for RandomDriver {
    fn name(&self) -> &str {
        "random"
    }

    fn submit(&mut self, _request: &str) -> bool {
        if self.busy {
            return false;
        }
        self.busy = true;
        true
    }

    fn poll(&mut self) -> Poll {
        if !self.busy {
            return Poll::Idle;
        }
        self.busy = false;
        const BUTTONS: [&str; 6] = ["a", "b", "up", "down", "left", "right"];
        let b = BUTTONS[(self.next() % BUTTONS.len() as u64) as usize];
        let mut w = super::json::Writer::new();
        w.begin_obj();
        w.str_field("say", "Trying something at random.");
        w.arr_field("plan");
        w.begin_obj();
        w.str_field("do", "press");
        w.str_field("button", b);
        w.end_obj();
        w.end_arr();
        w.end_obj();
        Poll::Ready(w.finish())
    }

    fn cancel(&mut self) {
        self.busy = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scripted_driver_answers_in_order_and_then_stops() {
        let mut d = ScriptedDriver::new(vec!["one".into(), "two".into()]);
        assert!(d.submit("{}"));
        assert_eq!(d.poll(), Poll::Ready("one".into()));
        assert_eq!(d.poll(), Poll::Idle);
        assert!(d.submit("{}"));
        assert_eq!(d.poll(), Poll::Ready("two".into()));
        assert!(!d.submit("{}"), "the script is exhausted");
        assert_eq!(d.calls(), 2);
    }

    /// Latency is what proves "the emulator never waits": the driver reports
    /// Pending for exactly as many polls as it was told to, and every one of
    /// those polls returns instantly.
    #[test]
    fn latency_is_reported_as_pending_not_as_a_block() {
        let mut d = ScriptedDriver::new(vec!["x".into()]).with_latency(3);
        assert!(d.submit("{}"));
        assert_eq!(d.poll(), Poll::Pending);
        assert_eq!(d.poll(), Poll::Pending);
        assert_eq!(d.poll(), Poll::Pending);
        assert_eq!(d.poll(), Poll::Ready("x".into()));
    }

    #[test]
    fn a_looping_script_never_runs_out() {
        let mut d = ScriptedDriver::new(vec!["a".into()]).looping();
        for _ in 0..5 {
            assert!(d.submit("{}"));
            assert_eq!(d.poll(), Poll::Ready("a".into()));
        }
    }

    #[test]
    fn the_random_driver_is_reproducible_from_its_seed() {
        let run = |seed| {
            let mut d = RandomDriver::new(seed);
            (0..6)
                .map(|_| {
                    d.submit("{}");
                    match d.poll() {
                        Poll::Ready(s) => s,
                        other => panic!("{other:?}"),
                    }
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        // And what it emits is a plan the host can actually parse.
        let v = super::super::json::parse(&run(7)[0]).unwrap();
        assert!(v.get("plan").and_then(|p| p.as_array()).is_some());
    }
}

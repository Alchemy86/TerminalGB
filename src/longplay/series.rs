//! Running an [`Episode`] over and over — the reset-and-repeat loop.
//!
//! # The loop, and the one thing that makes it possible
//!
//! ```text
//!   load the start save state ──▶ let the agent play ──▶ success? failure?
//!         ▲                                                  timeout?
//!         └───────────────────── record, capture, reset ◀────────┘
//! ```
//!
//! The reset is a save-state load, and `testharness/savestate.rs` proves that
//! a load produces **byte-identical frames from then on**. So every attempt
//! genuinely starts from the same machine, and the only thing that varies
//! between them is the agent. That is what makes a success *rate* mean
//! something.
//!
//! # Every attempt resets, and that is declared
//!
//! [`super::report::Scoreboard`] and [`Episode`]'s own summary both carry an
//! `assistance` block, and `state_resets` in it is the attempt count. That is
//! not a caveat, it is the design — but an undeclared reset would make the
//! numbers meaningless, so it is stated rather than assumed.
//!
//! # What is deliberately *not* reset
//!
//! The **agent's** memory. `Gameboy::load_state` calls `PluginHost::reset`, so
//! a plan in flight is dropped, and nothing in this harness carries a
//! knowledge base from one attempt into the next. That is on purpose and it
//! is worth saying why: the best-documented failure of a long LLM Pokémon run
//! is *memory poisoning* — a wrong belief written down early and then acted on
//! for hours. An episode is a clean-room measurement of a policy, not of a
//! policy plus whatever it happened to write down. A harness that wants
//! cross-attempt memory should add it deliberately and declare it.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::episode::{Attempt, Episode, EpisodeReport, Outcome};
use super::journal::Journal;
use super::progress::Progress;
use super::watch::Sample;
use crate::gameboy::Gameboy;
use crate::plugin::Capability;
use std::collections::BTreeMap;
use terminalgb_plugin_agent::json;
use terminalgb_plugin_agent::{AgentPlugin, PhaseView};

/// How often the machine is sampled, in frames. Matches the long runner's, so
/// an episode's area totals are directly comparable with a run's.
const SAMPLE_EVERY: u64 = 6;

/// One emulated frame of wall clock.
const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// Runs one episode, many times.
pub struct Series {
    gb: Gameboy,
    episode: Episode,
    journal: Journal,
    /// The save state every attempt starts from. Taken from the episode's
    /// `start_state`, or from wherever the machine was when the series began.
    start: Vec<u8>,
    dir: PathBuf,
    verbose: bool,
    capture_failures: bool,
    /// Pace to real time while a model call is in flight.
    ///
    /// **This matters more here than anywhere else in the harness.** An
    /// episode's timeout is in emulated frames, and headless the emulator does
    /// ~3,500 of them a second — so a 26-second model call is ninety thousand
    /// frames, and a 30,000-frame episode times out before its *first* decision
    /// comes back. Measured, on a real run. With this on, thinking costs real
    /// seconds instead of emulated hours and the timeout means what a person
    /// reading the episode file thinks it means.
    pace_while_thinking: bool,
    attempts: Vec<Attempt>,
    resets: u32,
}

impl Series {
    /// Build a series. `start` is the state every attempt begins from; passing
    /// `None` snapshots the machine as it stands, which is what "run the
    /// episode from here" means.
    pub fn new(
        mut gb: Gameboy,
        episode: Episode,
        dir: PathBuf,
        start: Option<Vec<u8>>,
        verbose: bool,
    ) -> Result<Series, String> {
        episode.validate()?;
        let journal = Journal::open(&dir)
            .map_err(|e| format!("cannot open {}: {e}", dir.display()))?;
        let start = match start {
            Some(bytes) => {
                gb.load_state(&bytes).map_err(|e| {
                    format!("the episode's start state was rejected: {e:?}")
                })?;
                bytes
            }
            None => gb.save_state(),
        };
        Ok(Series {
            gb,
            episode,
            journal,
            start,
            dir,
            verbose,
            capture_failures: true,
            pace_while_thinking: false,
            attempts: Vec::new(),
            resets: 0,
        })
    }

    pub fn without_captures(mut self) -> Self {
        self.capture_failures = false;
        self
    }

    /// See [`Series::pace_while_thinking`].
    pub fn pacing_while_thinking(mut self, on: bool) -> Self {
        self.pace_while_thinking = on;
        self
    }

    /// The machine, for a harness standing in for a cartridge. The same
    /// argument as [`super::Runner::gameboy_mut`]: the "agent only presses
    /// buttons" guarantee is enforced per call against the plugin's
    /// capability, not by hiding the machine from its owner.
    pub fn gameboy_mut(&mut self) -> &mut Gameboy {
        &mut self.gb
    }

    /// Run `n` attempts and summarise them.
    pub fn run(mut self, n: u32) -> EpisodeReport {
        if self.verbose {
            println!(
                "episode {:?}: {n} attempts, timeout {} frames each, ≤{} calls each \
                 (worst case {} calls)",
                self.episode.name,
                self.episode.timeout_frames,
                self.episode.max_calls,
                n as u64 * self.episode.max_calls as u64
            );
        }
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", super::episode::RESULT_SCHEMA);
        w.str_field("ev", "episode-start");
        w.str_field("episode", &self.episode.name);
        w.num_field("attempts", n as f64);
        w.end_obj();
        let line = w.finish();
        self.journal.write(&line);

        for i in 1..=n {
            let a = self.attempt(i);
            let line = a.to_json(&self.episode.name);
            self.journal.write(&line);
            if self.verbose {
                println!(
                    "  attempt {i}/{n}: {} · {} frames · {} calls{}",
                    a.outcome.tag(),
                    a.frames,
                    a.calls,
                    match &a.capture {
                        Some(p) => format!(" · saved {}", p.display()),
                        None => String::new(),
                    }
                );
            }
            self.attempts.push(a);
        }

        let report = EpisodeReport {
            episode: self.episode.clone(),
            attempts: std::mem::take(&mut self.attempts),
            resets: self.resets,
        };
        self.journal.write(&report.to_json());
        let _ = self
            .journal
            .write_report("episode.md", &report.to_markdown());
        let _ = self.journal.write_report("episode.json", &report.to_json());
        report
    }

    /// One attempt: reset, play, judge.
    fn attempt(&mut self, index: u32) -> Attempt {
        // The reset. This is the assistance the report declares.
        let start = self.start.clone();
        if let Err(e) = self.gb.load_state(&start) {
            // A start state that will not load is a broken episode, not a
            // failed attempt — say so rather than reporting a run of zeros.
            return Attempt {
                index,
                outcome: Outcome::Faulted(format!("the start state was rejected: {e:?}")),
                frames: 0,
                wall_ms: 0,
                calls: 0,
                decisions: 0,
                usd: 0.0,
                play_seconds: 0,
                areas: Vec::new(),
                final_progress: Progress::default(),
                watches: Sample::default(),
                capture: None,
            };
        }
        self.resets += 1;
        let (calls0, decisions0, usd0) = self.counters();

        let t = Instant::now();
        let mut frames = 0u64;
        let mut areas: BTreeMap<u8, u64> = BTreeMap::new();
        let mut last = Progress::default();
        let mut watch = Sample::default();
        let mut next_frame = Instant::now() + FRAME_TIME;
        let outcome = loop {
            self.gb.frame();
            frames += 1;
            if self.pace_while_thinking && self.agent_thinking() {
                let now = Instant::now();
                if now < next_frame {
                    std::thread::sleep(next_frame - now);
                    next_frame += FRAME_TIME;
                } else {
                    next_frame = now + FRAME_TIME;
                }
            } else {
                next_frame = Instant::now() + FRAME_TIME;
            }

            if frames % SAMPLE_EVERY == 0 {
                let (p, w) = self.sample();
                if p.in_game {
                    *areas.entry(p.map).or_insert(0) += SAMPLE_EVERY;
                }
                last = p.clone();
                watch = w;
                // Only judge once the save is actually loaded: before that
                // every byte is uninitialised WRAM and a condition on it is
                // noise. A start state is in-game by construction, so this
                // only skips the first frames of an odd one.
                if p.in_game {
                    if let Some(v) = self.episode.verdict(&p, &watch) {
                        break v;
                    }
                }
            }

            if let Some(f) = self.gb.fault() {
                break Outcome::Faulted(f.to_string());
            }
            if frames >= self.episode.timeout_frames {
                break Outcome::TimedOut;
            }
            let (calls, ..) = self.counters();
            if calls.saturating_sub(calls0) >= self.episode.max_calls {
                break Outcome::OutOfCalls;
            }
            // A halted agent cannot make progress; ending here rather than
            // burning the timeout is what keeps a thousand attempts affordable
            // in wall clock.
            if self.agent_stopped() {
                break Outcome::OutOfCalls;
            }
        };

        let (calls, decisions, usd) = self.counters();
        let capture = outcome
            .worth_capturing()
            .then(|| self.capture(index, outcome.tag()))
            .flatten();
        let mut areas: Vec<(u8, u64)> = areas.into_iter().collect();
        areas.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        Attempt {
            index,
            outcome,
            frames,
            wall_ms: t.elapsed().as_millis(),
            calls: calls.saturating_sub(calls0),
            decisions: decisions.saturating_sub(decisions0),
            usd: (usd - usd0).max(0.0),
            play_seconds: last.play_seconds,
            areas,
            final_progress: last,
            watches: watch,
            capture,
        }
    }

    fn sample(&mut self) -> (Progress, Sample) {
        let watches = self.episode.watches.clone();
        self.gb
            .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                (super::read_progress(m), watches.read(m))
            })
    }

    fn counters(&self) -> (u32, u64, f64) {
        match self.gb.plugin::<AgentPlugin>() {
            Some(p) => (p.governor().calls(), p.decisions(), p.governor().usd()),
            None => (0, 0, 0.0),
        }
    }

    fn agent_thinking(&self) -> bool {
        self.gb
            .plugin::<AgentPlugin>()
            .map(|p| p.phase() == PhaseView::Thinking)
            .unwrap_or(false)
    }

    fn agent_stopped(&self) -> bool {
        self.gb
            .plugin::<AgentPlugin>()
            .map(|p| p.phase() == PhaseView::Stopped && p.is_enabled())
            .unwrap_or(false)
    }

    /// **A failing attempt you can reload is worth ten you cannot.**
    fn capture(&mut self, index: u32, why: &str) -> Option<PathBuf> {
        if !self.capture_failures {
            return None;
        }
        let dir = self.dir.join("captures");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}-{index:04}-{why}.gbss", self.episode.name));
        let state = self.gb.save_state();
        std::fs::write(&path, &state).ok()?;
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::super::episode::Cond;
    use super::*;

    /// The reset is the assistance, and it is counted per attempt. A series
    /// that ran ten attempts and declared nine resets would mean one attempt
    /// kept the previous one's state, which would quietly invalidate it.
    #[test]
    fn every_attempt_is_one_declared_reset() {
        let r = EpisodeReport {
            episode: Episode {
                success: vec![Cond::parse("map=1").unwrap()],
                ..Default::default()
            },
            attempts: Vec::new(),
            resets: 7,
        };
        let j = r.to_json();
        assert!(j.contains("\"state_resets\":7"), "{j}");
        assert!(r.to_markdown().contains("state resets"));
    }
}

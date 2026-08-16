//! Playing a [`Scenario`] under its own rules, and replaying what came out.
//!
//! # The loop
//!
//! ```text
//!   load start state ──▶ frame ──▶ sample every 6 frames
//!                          ▲          │
//!                          │          ├─ referee: is a rule broken?  ──▶ over
//!                          │          ├─ goal reached? require rules? ─▶ over
//!                          │          ├─ learn: did a door lead somewhere new?
//!                          │          └─ brief: what does the agent need told?
//!                          └──────────┘
//!                                     ──▶ recording.json
//! ```
//!
//! # Four things happen at a sample and only one of them is new
//!
//! Sampling, judging the goal and reporting are [`crate::longplay::series`]'s,
//! reused rather than reimplemented. What this adds is the **referee** — and it
//! runs *before* the goal is judged, because a run that broke a rule did not
//! reach the goal, it forfeited.
//!
//! # The briefing is recomputed per decision, and it costs nothing to replay
//!
//! Between decisions the runner tells the agent its rules and whatever the
//! knowledge base has for its current position. That changes the *request*,
//! which is what a live model reasons about — and a replay's
//! [`terminalgb_plugin_agent::ScriptedDriver`] ignores the request entirely, so
//! none of it can make a replay diverge. That asymmetry is deliberate: guidance
//! may improve a live run without ever being able to invalidate a recorded one.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::knowledge::{Guidance, Knowledge};
use super::record::{fnv1a, Decision, Ending, Recording, ReplayReport};
use super::rules::{standing_notes, Allowances, Observed, Referee, Verdict};
use super::{Scenario, Source};
use crate::gameboy::Gameboy;
use crate::longplay::progress::Progress;
use crate::longplay::watch::{Sample, Watch, Watches};
use crate::plugin::Capability;
use terminalgb_plugin_agent::{AgentPlugin, PhaseView};

/// Matches the long runner's and the episode runner's, so an area total from
/// one is directly comparable with the others'.
const SAMPLE_EVERY: u64 = 6;

/// One emulated frame of wall clock.
const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// Everything one play produced.
#[derive(Debug, Clone)]
pub struct RunResult {
    pub recording: Recording,
    /// What this run worked out and is handing to the next.
    pub learned: Guidance,
    pub final_progress: Progress,
    /// Emulated frames per second of wall clock, this run.
    pub frames_per_second: f64,
    pub wall_ms: u128,
    /// Where the failing machine was saved, when it was worth saving.
    pub capture: Option<PathBuf>,
    /// The machine exactly as the run left it.
    ///
    /// **This is what makes a chain of scenarios possible**: link *n*'s finish
    /// is link *n+1*'s start, so a twelve-link playthrough can be built by
    /// playing forward once and every link afterwards can be attempted
    /// instantly without replaying the game. Present whatever the ending was —
    /// a failed run's machine is worth reloading too, which is why
    /// [`RunResult::capture`] exists — and it is the same bytes
    /// [`Recording::final_state`] is the hash of.
    pub final_state_bytes: Vec<u8>,
    /// Where the player was on **every emulated frame**, when the run was
    /// asked to track it.
    ///
    /// Sampled per frame rather than per decision on purpose: a decision is
    /// tens of frames apart and a film built from one point per decision is a
    /// slideshow of teleports, while a heat map built from one is a scatter of
    /// waypoints rather than a picture of where the time went. Five bus reads
    /// a frame, and only when [`ScenarioRun::tracking`] asked for it.
    pub path: Vec<PathPoint>,
}

/// One frame of where the player was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPoint {
    pub frame: u32,
    pub map: u8,
    pub x: u8,
    pub y: u8,
    /// 0 down, 4 up, 8 left, 12 right — the game's own encoding.
    pub facing: u8,
    /// Non-zero while a step animation is in progress, which is what tells a
    /// film that the character is walking rather than standing still.
    pub walking: u8,
    pub in_battle: u8,
}

impl RunResult {
    pub fn reached_goal(&self) -> bool {
        self.recording.ending.is_goal()
    }

    /// The headline for a run that used a model: how much game each decision
    /// bought. Zero decisions is reported as zero rather than as infinity.
    pub fn frames_per_decision(&self) -> f64 {
        if self.recording.decisions.is_empty() {
            return 0.0;
        }
        self.recording.frames as f64 / self.recording.decisions.len() as f64
    }

    pub fn lines(&self) -> Vec<String> {
        let r = &self.recording;
        let mut v = vec![
            format!(
                "{} after {} frames and {} decisions{}",
                match &r.ending {
                    Ending::Goal => "GOAL REACHED".to_string(),
                    Ending::Violated(d) => format!("FORFEIT — {d}"),
                    Ending::Failed(d) => format!("failed — {d}"),
                    Ending::TimedOut => "timed out".to_string(),
                    Ending::OutOfCalls => "ran out of model calls".to_string(),
                    Ending::Faulted(d) => format!("the emulator faulted — {d}"),
                },
                r.frames,
                r.decisions.len(),
                if r.decisions.is_empty() {
                    String::new()
                } else {
                    format!(" ({:.0} frames per decision)", self.frames_per_decision())
                }
            ),
            format!(
                "{} ms — {:.0} frames/second, {:.1}x real time",
                self.wall_ms,
                self.frames_per_second,
                self.frames_per_second / 59.7275
            ),
        ];
        // Where it ended up. The first question anybody asks of a run that did
        // not reach its goal, and it costs one line.
        if self.final_progress.in_game {
            let p = &self.final_progress;
            v.push(format!(
                "ended on map {} (0x{:02X}) at ({}, {}) — {} in the party, {} badge(s)",
                crate::longplay::map_name(p.map),
                p.map,
                p.x,
                p.y,
                p.party.len(),
                p.badges.count_ones()
            ));
        }
        if let Some(x) = &r.violation {
            v.push(x.line());
        }
        v.extend(r.allowances.lines());
        if !self.learned.is_empty() {
            v.push(format!(
                "learned {} door(s) between maps",
                self.learned.links.len()
            ));
        }
        if let Some(c) = &self.capture {
            v.push(format!(
                "the machine that failed was saved to {}",
                c.display()
            ));
        }
        v
    }
}

/// Plays one scenario.
pub struct ScenarioRun {
    gb: Gameboy,
    scenario: Scenario,
    referee: Referee,
    knowledge: Knowledge,
    /// The scenario's own watches plus the ones the runner needs to see a
    /// battle begin and end. Kept apart so the scenario's `watch:` conditions
    /// see exactly what the scenario declared.
    internal: Watches,
    start: Vec<u8>,
    dir: PathBuf,
    verbose: bool,
    capture_failures: bool,
    pace_while_thinking: bool,
    /// Written by every run that asked for guidance; merged into whatever was
    /// there before.
    guidance_file: Option<PathBuf>,
    track: bool,
    /// A raw video sink fed from the **emulated** frame, not from a draw.
    /// Headless there is no draw at all, and the point of a film of a solved
    /// scenario is that every emulated frame is in it.
    video: Option<crate::video::VideoOut>,
}

impl ScenarioRun {
    /// Build a run. The caller supplies a `Gameboy` with an [`AgentPlugin`]
    /// already attached, because choosing a driver is the front end's business
    /// and there are several — a live model, a scripted policy, a replay.
    ///
    /// `scenario_file` is what relative paths in the scenario resolve against;
    /// pass the scenario's own path.
    pub fn new(
        mut gb: Gameboy,
        scenario: Scenario,
        scenario_file: &Path,
        dir: PathBuf,
        start: Option<Vec<u8>>,
    ) -> Result<ScenarioRun, String> {
        scenario.validate()?;
        let referee = scenario.referee();
        let knowledge = scenario.knowledge_from(scenario_file);
        let start = match start {
            Some(bytes) => {
                gb.load_state(&bytes)
                    .map_err(|e| format!("the start state was rejected: {e:?}"))?;
                bytes
            }
            None => gb.save_state(),
        };
        // `in_battle` and `enemy_hp` are how a battle's *outcome* is observed —
        // the flag alone cannot tell a win from an escape. Both are built-in
        // named watches, so nothing here invents an address.
        let internal = Watches(
            ["in_battle", "enemy_hp"]
                .iter()
                .filter_map(|n| Watch::named(n))
                .collect(),
        );
        let guidance_file = scenario
            .wants(Source::Guidance)
            .then(|| scenario.guidance_from(scenario_file))
            .flatten();
        Ok(ScenarioRun {
            gb,
            scenario,
            referee,
            knowledge,
            internal,
            start,
            dir,
            verbose: true,
            capture_failures: true,
            pace_while_thinking: false,
            guidance_file,
            track: false,
            video: None,
        })
    }

    /// Stream every emulated frame to a raw video sink.
    pub fn filming(mut self, v: crate::video::VideoOut) -> Self {
        self.video = Some(v);
        self
    }

    /// Record the player's position on every frame. See [`RunResult::path`].
    pub fn tracking(mut self, on: bool) -> Self {
        self.track = on;
        self
    }

    pub fn quiet(mut self) -> Self {
        self.verbose = false;
        self
    }

    pub fn without_captures(mut self) -> Self {
        self.capture_failures = false;
        self
    }

    /// Pace to real time while a model call is in flight.
    ///
    /// **Turn this on for anything with a live model in it.** A scenario's
    /// timeout is in emulated frames and headless the emulator does thousands
    /// of them a second, so a twenty-second call is a hundred thousand frames
    /// and a scenario times out before its first decision comes back. Measured,
    /// on a real run — see `docs/longplay.md`.
    pub fn pacing_while_thinking(mut self, on: bool) -> Self {
        self.pace_while_thinking = on;
        self
    }

    pub fn gameboy_mut(&mut self) -> &mut Gameboy {
        &mut self.gb
    }

    pub fn knowledge(&self) -> &Knowledge {
        &self.knowledge
    }

    /// Play it once.
    pub fn play(mut self) -> RunResult {
        let t = Instant::now();
        let start_hash = fnv1a(&self.start);
        let rom_title = self.gb.with_machine(Capability::OBSERVE, |m| m.rom_title());
        let rom_checksum = self.gb.global_checksum();
        let driver = self
            .gb
            .plugin::<AgentPlugin>()
            .map(|p| p.driver_name().to_string())
            .unwrap_or_else(|| "none".into());

        // The reset every attempt is made of. Declared, not assumed.
        let mut allowances = Allowances {
            state_resets: 1,
            sources: self.knowledge.sources.clone(),
            start_state_supplied: self.scenario.start_state.is_some(),
            ..Default::default()
        };
        if let Err(e) = self.gb.load_state(&self.start) {
            return self.broken(
                format!("the start state was rejected: {e:?}"),
                start_hash,
                rom_title,
                rom_checksum,
                driver,
                allowances,
                t,
            );
        }
        // The counter must not carry the harness's own setup writes into the
        // run's verdict: the rule is about what happened *during* the attempt.
        let writes_before = self.gb.host_writes();

        let mut learned = Guidance {
            rom_title: rom_title.clone(),
            ..Default::default()
        };
        let mut decisions: Vec<Decision> = Vec::new();
        let mut frames = 0u64;
        let mut last = Progress::default();
        let mut battles = Battles::default();
        let mut prev: Option<Progress> = None;
        let mut next_frame = Instant::now() + FRAME_TIME;
        let mut guided = 0u64;
        let mut path: Vec<PathPoint> = Vec::new();

        // **Brief before the first frame, not on the first map change.** The
        // agent's opening decision is taken within a handful of frames, and a
        // rule it is only told about afterwards is a rule it can be judged on
        // without ever having heard of. The rules do not depend on position;
        // the route does, and it is refreshed whenever the map moves.
        let mut briefed_map = {
            let (p, _, _) = self.sample();
            let notes = standing_notes(
                &self.referee,
                &self.knowledge,
                p.map,
                self.scenario.goal_map(),
            );
            if notes.len() > self.referee.instructions().len() {
                guided += 1;
            }
            if let Some(a) = self.gb.plugin_mut::<AgentPlugin>() {
                a.set_briefing(notes);
            }
            p.in_game.then_some(p.map)
        };

        let ending = loop {
            self.gb.frame();
            frames += 1;
            if self.track {
                path.push(self.point(frames as u32));
            }
            if let Some(v) = &mut self.video {
                let img = self.gb.image();
                v.frame(&img, 160, 144, Instant::now());
            }
            if self.pace_while_thinking && self.thinking() {
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

            // Every reply the agent accepted since the last frame. Taken every
            // frame rather than every sample so a decision's frame number is
            // the frame it actually landed on.
            if let Some(p) = self.gb.plugin_mut::<AgentPlugin>() {
                for a in p.drain_accepted() {
                    decisions.push(Decision {
                        index: a.decision,
                        frame: a.frame,
                        reply: a.reply,
                    });
                }
            }

            if frames % SAMPLE_EVERY == 0 {
                let (p, watch, iw) = self.sample();
                if p.in_game {
                    // Learn before judging: a door that led to the goal map is
                    // exactly the one the next run most wants to know about.
                    if let Some(before) = &prev {
                        if before.in_game && before.map != p.map {
                            learned.observe_link(before.map, p.map, before.x, before.y);
                        }
                    }
                    battles.observe(&iw, p.party_hp);
                    prev = Some(p.clone());

                    let observed = Observed {
                        frame: frames,
                        wall_seconds: t.elapsed().as_secs(),
                        calls: self.calls(),
                        usd: self.usd(),
                        memory_writes: self.writes_since(writes_before),
                        rewinds: 0,
                        battles_won: battles.won,
                        battles_fled: battles.fled,
                        // `wIsInBattle`: 1 wild, 2 trainer, $FF blacked out.
                        in_trainer_battle: iw.get("in_battle") == Some(2),
                    };
                    // The referee first: a run that broke a rule forfeited, it
                    // did not reach the goal.
                    if let Verdict::Violated(v) =
                        self.referee.check(&observed, &p, &watch)
                    {
                        last = p;
                        break Ending::Violated(v.rule.clone());
                    }
                    if let Some(c) =
                        self.scenario.failure.iter().find(|c| c.holds(&p, &watch))
                    {
                        last = p;
                        break Ending::Failed(c.source.clone());
                    }
                    if !self.scenario.goal.is_empty()
                        && self.scenario.goal.iter().all(|c| c.holds(&p, &watch))
                    {
                        if let Verdict::Violated(v) =
                            self.referee.check_requirements(frames, &p, &watch)
                        {
                            last = p;
                            break Ending::Violated(v.rule.clone());
                        }
                        last = p;
                        break Ending::Goal;
                    }
                    // Brief the agent when it has moved somewhere new. Doing it
                    // per sample would rewrite an identical list 600 times a
                    // second for nothing.
                    if briefed_map != Some(p.map) {
                        briefed_map = Some(p.map);
                        let notes = standing_notes(
                            &self.referee,
                            &self.knowledge,
                            p.map,
                            self.scenario.goal_map(),
                        );
                        if notes.len() > self.referee.instructions().len() {
                            guided += 1;
                        }
                        if let Some(a) = self.gb.plugin_mut::<AgentPlugin>() {
                            a.set_briefing(notes);
                        }
                    }
                    last = p;
                }
            }

            if let Some(f) = self.gb.fault() {
                break Ending::Faulted(f.to_string());
            }
            if frames >= self.scenario.timeout_frames {
                break Ending::TimedOut;
            }
            // A halted agent cannot make progress; ending here rather than
            // burning the timeout is what keeps a thousand runs affordable in
            // wall clock.
            if self.agent_stopped() {
                break Ending::OutOfCalls;
            }
        };

        // Drain whatever landed on the final frame.
        if let Some(p) = self.gb.plugin_mut::<AgentPlugin>() {
            for a in p.drain_accepted() {
                decisions.push(Decision {
                    index: a.decision,
                    frame: a.frame,
                    reply: a.reply,
                });
            }
        }

        allowances.memory_writes = self.writes_since(writes_before);
        allowances.guided_decisions = guided;
        if self.scenario.wants(Source::BattleAdvice) {
            // Reported as the decisions that *could* have carried advice. The
            // honest count until a battle-advice channel is actually wired; it
            // never under-reports, which is the direction that matters.
            allowances.battle_advice = decisions.len() as u64;
        }

        let capture = (!ending.is_goal())
            .then(|| self.capture(ending.tag()))
            .flatten();

        if let Some(path) = &self.guidance_file {
            let mut merged = Guidance::load(path).unwrap_or_default();
            merged.merge(&learned);
            if merged.rom_title.is_empty() {
                merged.rom_title = rom_title.clone();
            }
            if let Err(e) = merged.save(path) {
                if self.verbose {
                    eprintln!("could not write guidance: {e}");
                }
            }
        }

        let wall_ms = t.elapsed().as_millis();
        let final_state_bytes = self.gb.save_state();
        let recording = Recording {
            scenario: self.scenario.name.clone(),
            scenario_hash: self.scenario.hash(),
            rom_title,
            rom_checksum,
            start_state: start_hash,
            final_state: fnv1a(&final_state_bytes),
            driver,
            decisions,
            frames,
            ending,
            allowances,
            violation: self.referee.violation().cloned(),
        };
        RunResult {
            frames_per_second: rate(recording.frames, wall_ms),
            recording,
            learned,
            final_progress: last,
            wall_ms,
            capture,
            final_state_bytes,
            path,
        }
    }

    /// Five bus reads. `wSpritePlayerStateData1FacingDirection` is the player
    /// sprite's own facing byte and `wWalkCounter` is non-zero for the eight
    /// frames of a step, which is how a film knows the difference between
    /// walking and turning on the spot.
    fn point(&mut self, frame: u32) -> PathPoint {
        self.gb
            .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                PathPoint {
                    frame,
                    map: m.peek(0xD35E).unwrap_or(0),
                    y: m.peek(0xD361).unwrap_or(0),
                    x: m.peek(0xD362).unwrap_or(0),
                    facing: m.peek(0xC109).unwrap_or(0),
                    walking: m.peek(0xCFC5).unwrap_or(0),
                    in_battle: m.peek(0xD057).unwrap_or(0),
                }
            })
    }

    /// Replay a recording into this machine and say whether it reproduced.
    ///
    /// The caller must have attached an [`AgentPlugin`] whose driver is a
    /// [`terminalgb_plugin_agent::ScriptedDriver`] built from
    /// [`Recording::replies`] — see `examples/scenario.rs`. This method does not
    /// build it, for the same reason [`ScenarioRun::new`] does not: which driver
    /// is in the machine is the front end's business.
    pub fn replay(mut self, recorded: &Recording) -> Result<ReplayReport, String> {
        let rom_title = self.gb.with_machine(Capability::OBSERVE, |m| m.rom_title());
        recorded.check_against(
            &rom_title,
            self.gb.global_checksum(),
            fnv1a(&self.start),
        )?;
        let drifted = recorded.scenario_hash != self.scenario.hash();
        let t = Instant::now();
        // A replay must not pace: there is nothing to wait for, and pacing
        // would throw away the whole point of it.
        self.pace_while_thinking = false;
        self.capture_failures = false;
        self.guidance_file = None;
        let played = self.play();
        Ok(ReplayReport {
            state_matched: played.recording.final_state == recorded.final_state,
            recorded_state: recorded.final_state,
            replayed_state: played.recording.final_state,
            recorded_frames: recorded.frames,
            replayed_frames: played.recording.frames,
            recorded_ending: recorded.ending.clone(),
            replayed_ending: played.recording.ending.clone(),
            wall_ms: t.elapsed().as_millis(),
            decisions_used: played.recording.decisions.len(),
            decisions_recorded: recorded.decisions.len(),
            scenario_drifted: drifted,
        })
    }

    // -- plumbing ----------------------------------------------------------

    #[allow(clippy::too_many_arguments)]
    fn broken(
        self,
        why: String,
        start_hash: u64,
        rom_title: String,
        rom_checksum: u16,
        driver: String,
        allowances: Allowances,
        t: Instant,
    ) -> RunResult {
        RunResult {
            recording: Recording {
                scenario: self.scenario.name.clone(),
                scenario_hash: self.scenario.hash(),
                rom_title,
                rom_checksum,
                start_state: start_hash,
                final_state: 0,
                driver,
                decisions: Vec::new(),
                frames: 0,
                ending: Ending::Faulted(why),
                allowances,
                violation: None,
            },
            learned: Guidance::default(),
            final_progress: Progress::default(),
            frames_per_second: 0.0,
            wall_ms: t.elapsed().as_millis(),
            capture: None,
            final_state_bytes: Vec::new(),
            path: Vec::new(),
        }
    }

    fn sample(&mut self) -> (Progress, Sample, Sample) {
        let declared = self.scenario.watches.clone();
        let internal = self.internal.clone();
        self.gb
            .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                (
                    crate::longplay::read_progress(m),
                    declared.read(m),
                    internal.read(m),
                )
            })
    }

    /// Writes made *during* the attempt: plugin writes count too, because a
    /// plugin poking RAM is exactly the thing `no_memory_writes` is about.
    fn writes_since(&self, before: u64) -> u64 {
        let host = self.gb.host_writes().saturating_sub(before);
        let plugins = self
            .gb
            .plugin_statuses()
            .iter()
            .filter(|s| s.has_written)
            .count() as u64;
        host + plugins
    }

    fn calls(&self) -> u32 {
        self.gb
            .plugin::<AgentPlugin>()
            .map(|p| p.governor().calls())
            .unwrap_or(0)
    }

    fn usd(&self) -> f64 {
        self.gb
            .plugin::<AgentPlugin>()
            .map(|p| p.governor().usd())
            .unwrap_or(0.0)
    }

    fn thinking(&self) -> bool {
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

    /// **A failing run you can reload is worth ten you cannot.**
    fn capture(&mut self, why: &str) -> Option<PathBuf> {
        if !self.capture_failures {
            return None;
        }
        let dir = self.dir.join("captures");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(format!("{}-{why}.gbss", self.scenario.name));
        std::fs::write(&path, self.gb.save_state()).ok()?;
        Some(path)
    }
}

fn rate(frames: u64, wall_ms: u128) -> f64 {
    if wall_ms == 0 {
        return 0.0;
    }
    frames as f64 * 1000.0 / wall_ms as f64
}

/// Watches a battle begin and end, and works out which way it went.
///
/// The battle flag alone cannot tell a win from an escape — it clears either
/// way. What separates them is the *opponent* at the moment it clears: an enemy
/// on zero HP was beaten, an enemy still standing was left behind. A party on
/// zero HP is neither; that is a blackout, and it is a `failure` condition's
/// business rather than a battle policy's.
#[derive(Debug, Clone, Default)]
struct Battles {
    /// The first sample only *records*. A run whose start state was captured
    /// mid-battle would otherwise be charged with its outcome — and a `battle
    /// flee` leg then forfeits, at three decisions, for a fight it did not
    /// have. Measured: link 11's exit state is taken wherever the lead reached
    /// level 10, which is inside a battle about half the time, and link 12
    /// forfeited on every seed because of it.
    primed: bool,
    /// The battle the priming sample found already in progress. Its outcome
    /// belongs to whoever produced the start state, not to this run.
    inherited: bool,
    in_battle: bool,
    last_enemy_hp: u32,
    won: u32,
    fled: u32,
    lost: u32,
}

impl Battles {
    fn observe(&mut self, w: &Sample, party_hp: u32) {
        let now = w.get("in_battle").unwrap_or(0) != 0;
        let enemy_hp = w.get("enemy_hp").unwrap_or(0);
        if !self.primed {
            self.primed = true;
            self.in_battle = now;
            self.inherited = now;
            self.last_enemy_hp = enemy_hp;
            return;
        }
        if now {
            self.in_battle = true;
            self.last_enemy_hp = enemy_hp;
            return;
        }
        if self.in_battle {
            self.in_battle = false;
            if self.inherited {
                self.inherited = false;
                return;
            }
            if party_hp == 0 {
                self.lost += 1;
            } else if self.last_enemy_hp == 0 {
                self.won += 1;
            } else {
                self.fled += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every test below drives a run that begins *out* of a battle, which is
    /// what the priming sample represents.
    fn fresh() -> Battles {
        let mut b = Battles::default();
        b.observe(&s(0, 0), 100);
        b
    }

    fn s(in_battle: u32, enemy_hp: u32) -> Sample {
        Sample(vec![
            ("in_battle".into(), in_battle),
            ("enemy_hp".into(), enemy_hp),
        ])
    }

    /// A win and an escape both clear the battle flag. What tells them apart is
    /// the opponent's HP at the moment it clears, and the whole `battle flee`
    /// rule rests on getting that right.
    #[test]
    fn a_win_and_an_escape_are_told_apart_by_the_opponent() {
        let mut b = fresh();
        b.observe(&s(1, 20), 100);
        b.observe(&s(1, 4), 100);
        b.observe(&s(1, 0), 100); // knocked out
        b.observe(&s(0, 0), 100);
        assert_eq!((b.won, b.fled, b.lost), (1, 0, 0));

        let mut b = fresh();
        b.observe(&s(1, 20), 100);
        b.observe(&s(1, 18), 100);
        b.observe(&s(0, 18), 100); // walked away, opponent still standing
        assert_eq!((b.won, b.fled, b.lost), (0, 1, 0));
    }

    /// A blackout is neither a win nor an escape, and counting it as either
    /// would break a `battle flee` run for the wrong reason.
    #[test]
    fn a_blackout_is_neither_a_win_nor_an_escape() {
        let mut b = fresh();
        b.observe(&s(1, 20), 40);
        b.observe(&s(0, 20), 0);
        assert_eq!((b.won, b.fled, b.lost), (0, 0, 1));
    }

    /// Sampling out of a battle before ever being in one must not invent an
    /// outcome — the first samples of a run are exactly that.
    #[test]
    fn never_being_in_a_battle_produces_no_outcomes() {
        let mut b = fresh();
        for _ in 0..10 {
            b.observe(&s(0, 0), 100);
        }
        assert_eq!((b.won, b.fled, b.lost), (0, 0, 0));
    }

    /// A start state captured mid-battle is not a battle this attempt had.
    #[test]
    fn a_battle_already_under_way_at_the_start_is_not_this_run_s() {
        let mut b = Battles::default();
        b.observe(&s(1, 12), 100); // the very first sample: already fighting
        b.observe(&s(1, 0), 100);
        b.observe(&s(0, 0), 100); // and it ends in a knockout
        assert_eq!(
            (b.won, b.fled, b.lost),
            (0, 0, 0),
            "the run inherited the battle, it did not have it"
        );
    }

    #[test]
    fn a_rate_never_divides_by_a_zero_clock() {
        assert_eq!(rate(1000, 0), 0.0);
        assert!((rate(4200, 1000) - 4200.0).abs() < 0.001);
    }
}

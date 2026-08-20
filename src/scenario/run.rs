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

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::knowledge::{Guidance, Knowledge};
use super::record::{fnv1a, Decision, Ending, Recording, ReplayReport};
use super::rules::{standing_notes, Allowances, Observed, Referee, Verdict};
use super::screens::ScreenTrack;
use super::track::{Track, TrackPoint};
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

/// Tile changes kept, to decide afterwards whether a run was going anywhere.
///
/// Twenty-four is generous: a run that has genuinely moved twenty-four times
/// has walked a long way, and a route that doubles back through a doorway is
/// still well clear of [`LOOP_TILES`].
const LOOP_VISITS: usize = 24;
/// Distinct tiles those changes must cover to count as travel rather than
/// oscillation. Four is a two-by-two shuffle, or a pair of tiles bounced
/// between with a turn on each — the shape every one of these has taken.
const LOOP_TILES: usize = 4;

/// Was the run going round in circles when its budget ran out?
///
/// **A budget failure and a stuck failure are different findings, and calling
/// both "budget" is what hid a family of defects.** A run that spends its
/// whole allowance covering ground was given too little allowance; a run that
/// spends it revisiting the same four tiles is looping, and a bigger allowance
/// buys only a longer loop. They are indistinguishable in a pass rate and
/// indistinguishable in a reason histogram that has one bucket for both — so
/// the tail of the walk is examined and the two are named separately.
///
/// The evidence this exists for: seventeen failures of one link all reported
/// "budget", all having used *exactly* the cap, on a route the expert walks in
/// fourteen steps against a sixty-step cap. Forty-six steps of headroom is not
/// a budget problem. The same signature appears in the chain report as
/// "oscillated between (20,28) and (21,28) for 430 decisions", where it was
/// found by a human reading a trace rather than by anything automatic.
fn looping(visits: &VecDeque<(u8, u8, u8)>) -> Option<String> {
    if visits.len() < LOOP_VISITS {
        return None;
    }
    let mut distinct: Vec<(u8, u8, u8)> = visits.iter().copied().collect();
    distinct.sort_unstable();
    distinct.dedup();
    if distinct.len() > LOOP_TILES {
        return None;
    }
    let where_ = distinct
        .iter()
        .map(|(m, x, y)| format!("0x{m:02X}({x},{y})"))
        .collect::<Vec<_>>()
        .join(" ");
    Some(format!(
        "the last {} moves covered {} tile(s): {where_}",
        visits.len(),
        distinct.len()
    ))
}

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
    /// Of [`RunResult::wall_ms`], how much was inside [`Gameboy::frame`].
    ///
    /// **The number that says whether a swarm is emulating or bookkeeping.**
    /// The rest is this loop plus the agent: the per-frame plugin dispatch, the
    /// six-frame sample, the referee and the guidance. Measured with two clock
    /// reads a frame, which is ~50 ns against a frame that costs tens of
    /// microseconds — 0.04 %, and worth it, because "how much of a step is
    /// emulation" is otherwise a guess.
    pub emulate_ms: u128,
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
    /// How the battles inside this run went, observed by the **host**.
    ///
    /// **This is here so that two different players can be scored the same
    /// way.** The battle numbers this project has published so far come from
    /// `ChainDriver`'s own bookkeeping, which is driver-side: only the scripted
    /// expert can be measured by it, so a comparison between the expert and
    /// anything else — a trained network, a baseline, a live model — has no
    /// battle column at all. These figures are read off the machine on the
    /// run's own sampling clock and are therefore identical work whoever is
    /// pressing the buttons. See [`BattleSummary`].
    ///
    /// Driver-side bookkeeping still has its place: it can see *why* a move was
    /// chosen, which nothing outside the driver can. The two are complementary
    /// and the numbers below are the ones that are comparable.
    pub battles: BattleSummary,
    /// Where the player was, sampled on **every emulated frame** and kept
    /// whenever it changed.
    ///
    /// **On by default**, because a run whose position was not recorded can
    /// never be filmed afterwards and the interesting material — the failures,
    /// the blackouts, the tail — is exactly what a certification pass used to
    /// throw away. [`ScenarioRun::tracking`] turns it off for a throughput
    /// measurement and for nothing else.
    ///
    /// Sampled per frame rather than per decision on purpose: a decision is
    /// tens of frames apart and a film built from one point per decision is a
    /// slideshow of teleports, while a heat map built from one is a scatter of
    /// waypoints rather than a picture of where the time went. Four bus reads a
    /// frame and a comparison; the point is dropped unless something moved, so
    /// a run that stands still for a hundred thousand frames costs one record.
    /// See [`crate::scenario::track`] for the storage format and its measured
    /// cost.
    pub path: Track,
    /// Every screen this run drew, downscaled and packed, when the run was
    /// asked to record them.
    ///
    /// **A position track carries no picture**, which is why a mosaic of many
    /// runs cannot be built out of a corpus that has already been collected —
    /// see [`crate::scenario::screens`] for the format and for the arithmetic
    /// that keeps it affordable.
    pub screens: Option<ScreenTrack>,
}

/// What happened in the battles of one run, as the host saw them.
///
/// Every field is read from the machine on the run's own sampling clock
/// ([`SAMPLE_EVERY`] frames) through named watches another crate derived, so a
/// scripted expert, a trained policy, a live model and a random baseline all
/// produce these numbers by exactly the same work. That is the whole point:
/// **a battle figure that only one player can produce cannot be compared.**
///
/// What it deliberately does **not** carry is anything about *reasoning* — why
/// a move was picked, whether a damage estimate landed inside Gen 1's roll
/// range, how often an escape was refused. Those are properties of a policy
/// that publishes them, not of a machine, and asking a network for them would
/// be asking it to explain itself. `ChainDriver`'s own tally still reports them
/// for the expert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BattleSummary {
    /// Battles that began *and ended* inside this run. A battle already in
    /// progress at the first sample belongs to whoever produced the start
    /// state and is excluded from every field here.
    pub battles: u32,
    /// The opponent was at zero HP when the battle ended.
    pub won: u32,
    /// The battle ended with both sides standing.
    pub fled: u32,
    /// The party was at zero HP when the battle ended.
    pub lost: u32,
    /// Battles whose `wIsInBattle` read 1 when they began.
    pub wild: u32,
    /// Battles whose `wIsInBattle` read 2 when they began. A trainer battle
    /// cannot be fled, which is why the two are counted apart.
    pub trainer: u32,
    /// Emulated frames spent inside a battle. Rounded to the sampling clock,
    /// so it is a proportion rather than an exact count.
    pub frames_in_battle: u64,
    /// The lead's HP as a permille of its maximum, summed over battle endings.
    /// Divide by [`BattleSummary::hp_samples`] for the mean.
    pub hp_left_permille: u64,
    /// How many battle endings supplied an HP reading. Zero when the lead had
    /// no maximum HP to divide by, which is what an empty party looks like.
    pub hp_samples: u32,
}

impl BattleSummary {
    /// Mean HP the lead had left when a battle ended, as a percentage of its
    /// maximum. Zero battles is reported as zero rather than as a division by
    /// zero.
    pub fn hp_left_percent(&self) -> f64 {
        if self.hp_samples == 0 {
            return 0.0;
        }
        self.hp_left_permille as f64 / self.hp_samples as f64 / 10.0
    }

    /// Fold another run's battles in, for a corpus total.
    pub fn merge(&mut self, o: &BattleSummary) {
        self.battles += o.battles;
        self.won += o.won;
        self.fled += o.fled;
        self.lost += o.lost;
        self.wild += o.wild;
        self.trainer += o.trainer;
        self.frames_in_battle += o.frames_in_battle;
        self.hp_left_permille += o.hp_left_permille;
        self.hp_samples += o.hp_samples;
    }
}

/// One recorded change of where the player was.
///
/// The old name for [`TrackPoint`], kept because it is what the front ends
/// spell. **It no longer carries the walk counter**: `wWalkCounter` ticks
/// across a step, so it was 83 % of every row a track ever held, nothing has
/// ever read it, and the renderer reconstructs the glide by interpolating
/// between consecutive recorded tiles. See [`crate::scenario::track`].
pub type PathPoint = TrackPoint;

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
                    Ending::Stuck(d) => format!("went round in circles — {d}"),
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
    /// Nobody is watching — see [`ScenarioRun::collecting`].
    collecting: bool,
    /// See [`ScenarioRun::speed_mode`]. `None` means "whatever `$GB_SPEED`
    /// says", which is normally nothing at all.
    speed_flags: Option<crate::speed::SpeedFlags>,
    /// A downscaled screen every N frames — see [`ScenarioRun::screens`].
    screens: Option<ScreenTrack>,
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
        // the flag alone cannot tell a win from an escape. `lead_hp` and
        // `lead_max_hp` are how expensively it was won: a leg can pass every
        // attempt while every battle underneath it ends on a sliver, and only
        // the second pair can say so. All four are built-in named watches, so
        // nothing here invents an address.
        let internal = Watches(
            ["in_battle", "enemy_hp", "lead_hp", "lead_max_hp"]
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
            // **On by default.** A run that did not record its position can
            // never be filmed, and the split between a certification pass and
            // a film pass is exactly what made the certified runs unfilmable.
            track: true,
            video: None,
            collecting: false,
            speed_flags: None,
            screens: None,
        })
    }

    /// Stream every emulated frame to a raw video sink.
    pub fn filming(mut self, v: crate::video::VideoOut) -> Self {
        self.video = Some(v);
        self
    }

    /// Whether to record where the player was. **On by default** — see
    /// [`RunResult::path`] for why, and [`crate::scenario::track`] for what it
    /// costs. Pass `false` only when nothing will ever want a picture of this
    /// run, which in practice means a throughput benchmark.
    pub fn tracking(mut self, on: bool) -> Self {
        self.track = on;
        self
    }

    /// Record a **downscaled** screen every `every` emulated frames.
    ///
    /// This is the only way a picture leaves a headless run: a position track
    /// says where the player was and carries no pixels, so a mosaic of dozens
    /// of runs needs a capture pass and cannot be derived from a corpus
    /// afterwards. `scale` is the downscale factor — 2 gives 80x72, which is
    /// already at or above the size a mosaic tile is drawn at.
    ///
    /// **It is not free and the cost is the pixels, not the packing.** A
    /// [`ScenarioRun::collecting`] run has the PPU's pixel path switched off
    /// (worth about half a frame on this box); asking for screens switches it
    /// back on for the whole run, exactly as [`ScenarioRun::filming`] does.
    /// Everything a cartridge can see is unchanged either way — the
    /// framebuffer is emulator output and is reachable from no address.
    ///
    /// `every` of 0 records nothing. See [`crate::scenario::screens::estimate`]
    /// for what it will cost before a byte is written.
    pub fn screens(mut self, every: u32, scale: u32) -> Self {
        self.screens = (every > 0).then(|| ScreenTrack::new(every, scale));
        self
    }

    /// **Nobody is watching this run**, so the machine may stop producing the
    /// two things only a watcher consumes: pixels and samples.
    ///
    /// Both are emulator *output* and neither is reachable from any address a
    /// cartridge can name, so this changes nothing the game can see — see
    /// [`Gameboy::set_render_pixels`] and [`Gameboy::set_render_audio`] for the
    /// constructions. The agent's perception is `wTileMap` decoded out of
    /// memory and a handful of WRAM reads; it has never looked at the
    /// framebuffer, and nothing anywhere in a scenario listens.
    ///
    /// [`ScenarioRun::filming`] is the exception and wins: a video sink is a
    /// watcher, so **pixels** come back on for a filmed run however this was
    /// called. Order does not matter — the two are resolved together at the top
    /// of [`ScenarioRun::play`]. Samples do not come back, because the sink is
    /// `rawvideo` and carries no audio; if one ever does, that is the line to
    /// change.
    ///
    /// Worth 1.7x on this box, measured on the machine a collector actually
    /// runs (an APU installed, because a machine with *no* sound chip is one a
    /// cartridge can tell apart): 12,012 frames/s with both on against 20,695
    /// with both off, and the WRAM hash identical across the pair. Left off by
    /// default so a library caller that does read `image()` is never surprised;
    /// every headless front end asks for it.
    pub fn collecting(mut self) -> Self {
        self.collecting = true;
        self
    }

    /// Ask for **speed mode**: the peripherals advance once per instruction
    /// rather than once per bus access, which on a real certification workload
    /// is worth about 9 % and changes nothing the cartridge reads back. See
    /// [`Gameboy::set_speed_mode`] for the trade and `docs/modes.md` for what
    /// it may and may not be used to judge. `$GB_SPEED=speed` does the same.
    pub fn speed_mode(mut self, on: bool) -> Self {
        self.speed_flags = on.then_some(crate::speed::SpeedFlags::PRESET);
        self
    }

    /// Ask for a specific set of shortcuts, overriding `$GB_SPEED`.
    pub fn speed_flags(mut self, flags: crate::speed::SpeedFlags) -> Self {
        self.speed_flags = Some(flags);
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
        // Resolved here rather than in the builders so the two declarations
        // commute: a filmed run keeps its pixels whichever order they arrive
        // in. See [`ScenarioRun::collecting`].
        if self.collecting {
            self.gb
                .set_render_pixels(self.video.is_some() || self.screens.is_some());
            self.gb.set_render_audio(false);
        }
        // **Speed mode**, opt-in and off by default — see
        // [`Gameboy::set_speed_mode`] and `docs/modes.md`. A run driving the
        // machine may take it; a run judging pixels may not, and identical mode
        // refuses it outright. Read here rather than in the core because the
        // core's host surface is deliberately one environment variable
        // (`$GB_MODEL`) and this is a harness decision.
        let (mut flags, bad) = crate::speed::SpeedFlags::from_env();
        if let Some(asked) = self.speed_flags {
            flags = asked;
        }
        for token in &bad {
            eprintln!("terminalgb: unknown speed flag {token:?} — ignored");
        }
        if flags.any() {
            self.gb.set_speed_flags(flags);
        }
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
        let mut path = Track::new();
        // Where the run has been lately, one entry per *change* of tile. See
        // `looping`.
        let mut visits: VecDeque<(u8, u8, u8)> = VecDeque::new();

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

        let mut emulating = Duration::ZERO;
        let ending = loop {
            let f0 = Instant::now();
            self.gb.frame();
            emulating += f0.elapsed();
            frames += 1;
            if self.track {
                path.push(self.point(frames as u32));
            }
            if let Some(v) = &mut self.video {
                let img = self.gb.image();
                v.frame(img, 160, 144, Instant::now());
            }
            if let Some(sc) = self.screens.as_mut() {
                if frames % sc.every() as u64 == 0 {
                    sc.push(self.gb.image());
                }
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
                    if visits.back() != Some(&(p.map, p.x, p.y)) {
                        visits.push_back((p.map, p.x, p.y));
                        if visits.len() > LOOP_VISITS {
                            visits.pop_front();
                        }
                    }
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
                break match looping(&visits) {
                    Some(d) => Ending::Stuck(d),
                    None => Ending::TimedOut,
                };
            }
            // A halted agent cannot make progress; ending here rather than
            // burning the timeout is what keeps a thousand runs affordable in
            // wall clock.
            if self.agent_stopped() {
                break match looping(&visits) {
                    Some(d) => Ending::Stuck(d),
                    None => Ending::OutOfCalls,
                };
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
            emulate_ms: emulating.as_millis(),
            capture,
            final_state_bytes,
            battles: battles.summary(),
            path,
            screens: self.screens.take(),
        }
    }

    /// Five bus reads. `wSpritePlayerStateData1FacingDirection` is the player
    /// sprite's own facing byte and `wWalkCounter` is non-zero for the eight
    /// frames of a step, which is how a film knows the difference between
    /// walking and turning on the spot.
    fn point(&mut self, frame: u32) -> PathPoint {
        self.gb
            .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                TrackPoint {
                    frame,
                    map: m.peek(0xD35E).unwrap_or(0),
                    y: m.peek(0xD361).unwrap_or(0),
                    x: m.peek(0xD362).unwrap_or(0),
                    facing: m.peek(0xC109).unwrap_or(0),
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
            emulate_ms: 0,
            capture: None,
            final_state_bytes: Vec::new(),
            battles: BattleSummary::default(),
            path: Track::new(),
            screens: None,
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
    /// `wIsInBattle` as it stood in the battle now running: 1 wild, 2 trainer.
    kind: u32,
    wild: u32,
    trainer: u32,
    /// Samples spent inside a battle this run did not inherit, so
    /// `frames_in_battle` below is a real fraction of the run.
    battle_samples: u64,
    /// The lead's HP as a permille of its maximum, summed over battle
    /// *endings*, and how many endings supplied one.
    hp_permille: u64,
    hp_samples: u32,
    /// The lead's HP and maximum as they stood at the last sample, so an
    /// ending can be scored from what the battle looked like just before it
    /// disappeared.
    lead_hp: u32,
    lead_max_hp: u32,
}

impl Battles {
    fn observe(&mut self, w: &Sample, party_hp: u32) {
        let flag = w.get("in_battle").unwrap_or(0);
        // `wIsInBattle`: 1 wild, 2 trainer, `$FF` blacked out. The sentinel is
        // *not* a battle, and a catch-all arm that treats it as one is the
        // defect `plugins/trainer/src/encounter.rs` records — see
        // `a_blackout_sentinel_is_not_a_battle` below.
        let now = flag == 1 || flag == 2;
        let enemy_hp = w.get("enemy_hp").unwrap_or(0);
        let lead_hp = w.get("lead_hp").unwrap_or(0);
        let lead_max = w.get("lead_max_hp").unwrap_or(0);
        if !self.primed {
            self.primed = true;
            self.in_battle = now;
            self.inherited = now;
            self.last_enemy_hp = enemy_hp;
            self.kind = flag;
            return;
        }
        if now {
            if !self.in_battle {
                self.kind = flag;
            }
            self.in_battle = true;
            self.last_enemy_hp = enemy_hp;
            self.lead_hp = lead_hp;
            self.lead_max_hp = lead_max;
            if !self.inherited {
                self.battle_samples += 1;
            }
            return;
        }
        if self.in_battle {
            self.in_battle = false;
            if self.inherited {
                self.inherited = false;
                return;
            }
            if self.kind == 2 {
                self.trainer += 1;
            } else {
                self.wild += 1;
            }
            if self.lead_max_hp > 0 {
                self.hp_permille += (self.lead_hp.min(self.lead_max_hp) as u64 * 1000)
                    / self.lead_max_hp as u64;
                self.hp_samples += 1;
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

    fn summary(&self) -> BattleSummary {
        BattleSummary {
            battles: self.won + self.fled + self.lost,
            won: self.won,
            fled: self.fled,
            lost: self.lost,
            wild: self.wild,
            trainer: self.trainer,
            frames_in_battle: self.battle_samples * SAMPLE_EVERY,
            hp_left_permille: self.hp_permille,
            hp_samples: self.hp_samples,
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
        hp(in_battle, enemy_hp, 20, 20)
    }

    /// The same sample with the lead's own HP stated, for the cost figures.
    fn hp(in_battle: u32, enemy_hp: u32, lead: u32, lead_max: u32) -> Sample {
        Sample(vec![
            ("in_battle".into(), in_battle),
            ("enemy_hp".into(), enemy_hp),
            ("lead_hp".into(), lead),
            ("lead_max_hp".into(), lead_max),
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

    /// **`wIsInBattle` is a raw byte and `$FF` means blacked out, not
    /// fighting.** Third instance of the same class in this repository — the
    /// first was `plugins/trainer/src/encounter.rs`, whose catch-all arm drew a
    /// whole stale battle panel in the overworld. Teeth: with `!= 0` the wipe
    /// below reads as a battle beginning and then never ending, so the run
    /// reports a battle it never had and no HP figure at all.
    #[test]
    fn a_blackout_sentinel_is_not_a_battle() {
        let mut b = fresh();
        b.observe(&hp(1, 20, 12, 20), 12);
        b.observe(&hp(1, 20, 0, 20), 0); // the lead goes down
        b.observe(&hp(0xFF, 20, 0, 20), 0); // wIsInBattle == $FF: blacked out
        b.observe(&hp(0, 0, 20, 20), 20); // and we wake up in a Pokémon Center
        let s = b.summary();
        assert_eq!((s.battles, s.lost), (1, 1), "one battle, and it was lost");
    }

    /// A wild battle and a trainer battle are different situations — one can be
    /// fled and one cannot — so they are counted apart, and the kind is taken
    /// when the battle *begins* rather than when it ends, because by then the
    /// flag has already gone.
    #[test]
    fn a_wild_battle_and_a_trainer_battle_are_counted_apart() {
        let mut b = fresh();
        b.observe(&s(1, 20), 100);
        b.observe(&s(0, 18), 100);
        b.observe(&s(2, 30), 100);
        b.observe(&s(2, 0), 100);
        b.observe(&s(0, 0), 100);
        let sum = b.summary();
        assert_eq!((sum.wild, sum.trainer), (1, 1));
        assert_eq!((sum.fled, sum.won), (1, 1));
    }

    /// **A leg can pass every attempt while every battle under it ends on a
    /// sliver.** The pass rate cannot see that and the HP figure is the whole
    /// reason it is reported beside it.
    #[test]
    fn how_much_hp_a_battle_cost_is_read_at_the_ending() {
        let mut b = fresh();
        b.observe(&hp(1, 20, 19, 20), 19);
        b.observe(&hp(1, 0, 5, 20), 5); // won it, but only just
        b.observe(&hp(0, 0, 5, 20), 5);
        let sum = b.summary();
        assert_eq!(sum.hp_samples, 1);
        assert!(
            (sum.hp_left_percent() - 25.0).abs() < 0.01,
            "5 of 20 is 25%, got {}",
            sum.hp_left_percent()
        );
        assert_eq!(sum.frames_in_battle, 2 * SAMPLE_EVERY);
    }

    /// An inherited battle contributes to nothing at all, not merely to the
    /// win column: counting its frames or its HP would charge this run for a
    /// fight the previous link had.
    #[test]
    fn an_inherited_battle_contributes_to_no_field() {
        let mut b = Battles::default();
        b.observe(&hp(1, 12, 3, 20), 3);
        b.observe(&hp(1, 0, 3, 20), 3);
        b.observe(&hp(0, 0, 3, 20), 3);
        assert_eq!(b.summary(), BattleSummary::default());
    }

    #[test]
    fn a_rate_never_divides_by_a_zero_clock() {
        assert_eq!(rate(1000, 0), 0.0);
        assert!((rate(4200, 1000) - 4200.0).abs() < 0.001);
    }

    /// **A stuck run and a run out of budget are different findings.**
    ///
    /// Teeth: the oscillation case is the exact shape seventeen failures of one
    /// link took — a pair of tiles bounced between until the cap — and it was
    /// reported as "budget" every time. The travelling case is the control: a
    /// run that really did walk somewhere must keep its honest budget verdict,
    /// or the new bucket swallows the old one and nothing is gained.
    #[test]
    fn oscillation_is_not_a_budget_failure() {
        let mut bounce: VecDeque<(u8, u8, u8)> = VecDeque::new();
        for i in 0..LOOP_VISITS {
            bounce.push_back((0x00, 20 + (i % 2) as u8, 28));
        }
        let d = looping(&bounce).expect("a two-tile shuffle is a loop");
        assert!(d.contains("2 tile(s)"), "{d}");

        let mut walked: VecDeque<(u8, u8, u8)> = VecDeque::new();
        for i in 0..LOOP_VISITS {
            walked.push_back((0x00, 10, 30 - i as u8));
        }
        assert!(looping(&walked).is_none(), "a straight walk is not a loop");

        // Too little evidence is not evidence: a short run that happens to
        // have revisited a tile has not been shown to be looping.
        let mut brief: VecDeque<(u8, u8, u8)> = VecDeque::new();
        brief.push_back((0, 1, 1));
        brief.push_back((0, 1, 2));
        assert!(looping(&brief).is_none(), "two moves prove nothing");
    }
}

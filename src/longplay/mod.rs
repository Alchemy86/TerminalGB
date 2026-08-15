//! **The long-horizon runner** — an agent plays a cartridge unattended for
//! hours at full headless speed, and reports in.
//!
//! Reader-facing page: [`docs/longplay.md`](../docs/longplay.md). The agent
//! substrate this drives is [`terminalgb_plugin_agent`] and is documented in
//! [`docs/claude-plays.md`](../docs/claude-plays.md).
//!
//! # The one number that decides the design
//!
//! Headless, no audio, uncapped, this emulator runs at **0.238 ms/frame — 70x
//! real time** (`docs/mcp.md` §6), i.e. about 4,200 frames a second. A model
//! round trip is *seconds*. So a design that consults the model every few
//! frames runs at the speed of the network and throws the 70x away.
//!
//! The agent plugin already solves that at the scale of one decision: a plan of
//! up to twelve actions executes locally for hundreds of frames before the
//! model is asked again. This module is what turns that into a run that lasts
//! all night, and its headline metric is therefore **frames per model call**
//! ([`report::Scoreboard::frames_per_call`]) — how much game each decision
//! bought. Everything else in the report is context for that number.
//!
//! # What is here that is not in `examples/claude_plays.rs`
//!
//! That example is a session: start, play, print. Four things separate a
//! session from a *run*.
//!
//! 1. **Durable state.** A model's context does not span seventy hours of game
//!    time and neither does a process. [`journal`] keeps the append-only event
//!    log and the checkpoint a restart resumes from.
//! 2. **Meaningful progress reports.** Not a frame counter: badges, towns, a
//!    first Pokémon, a wipe, a wall — [`progress::MilestoneKind`] — pushed to a
//!    phone through [`notify`] when they are worth waking somebody for.
//! 3. **A stuck detector at the scale of the *game*.** The agent's own detector
//!    asks "am I repeating myself?" per decision; [`progress::Wall`] asks "has
//!    the game got anywhere?" over game time, which is the failure an
//!    unattended run actually dies of. See that module for why one cannot
//!    substitute for the other.
//! 4. **An honest scoreboard**, [`report`].
//!
//! # A session is also a dataset
//!
//! Beside the human-readable report the run writes `steps.ndjson`: one line per
//! decision holding the **observation** the agent was given, the **intent** it
//! stated, the **actions** that ran and the **outcome** they produced. Small
//! and honest rather than grand: it is the four things a learner needs and
//! nothing else, and every field is already measured for the report.
//!
//! # What this may not do
//!
//! It presses buttons. The agent declares `OBSERVE | READ_MEMORY |
//! CONTROL_INPUT` and the host refuses anything else, so no amount of prompting
//! can make it edit RAM. The runner itself never pokes and never rewinds —
//! **except** on an explicit `--resume`, which is a rewind, which is why it is
//! counted, reported as a `Major` milestone and printed in the report's
//! "Declared" section.

pub mod episode;
pub mod journal;
pub mod notify;
pub mod progress;
pub mod report;
pub mod series;
pub mod watch;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::gameboy::Gameboy;
use crate::plugin::Capability;
use journal::{Checkpoint, Journal};
use notify::{Notifier, Push};
use progress::{Importance, Milestone, MilestoneKind, Progress};
use report::Scoreboard;
use terminalgb_plugin_agent::json;
use terminalgb_plugin_agent::{AgentPlugin, PhaseView};
use terminalgb_plugin_trainer::gen1::addr as tr;
use watch::{Sample, Watches};

/// One emulated frame, in game seconds.
const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// How often the machine is sampled, in frames. ~10 Hz: a milestone is a state
/// that persists for many frames, so sampling faster buys nothing and every
/// sample is ~40 bus reads.
const SAMPLE_EVERY: u64 = 6;

// -- Pokédex and bag, derived and checked ------------------------------------
//
// `terminalgb_plugin_trainer::gen1` carries the party, badges and money; it does not
// carry the Pokédex flags or the bag, and this module needs both (catching a
// species and picking something up are exactly the "did anything happen?"
// signals a run reports on).
//
// Derived by walking `pret/pokered`'s `ram/wram.asm`, and the walk **lands on
// a second verified symbol from each direction**, which is its own proof:
//
//   forwards  from wPartyCount ($D163, verified in `gen1::addr::PARTY_COUNT`):
//     1 (wPartyCount) + 7 (wPartySpecies) + 6*44 (wPartyMons) + 6*11 (OT)
//     + 6*11 (nicks) = 404 = $194  →  $D163 + $194 = $D2F7
//   backwards from wPlayerMoney ($D347, verified in `gen1::addr::PLAYER_MONEY`):
//     $D347 − 41 (wBagItems = BAG_ITEM_CAPACITY*2+1, capacity 20)
//           − 1  (wNumBagItems)          = $D31D
//           − 19 (wPokedexSeen, flag_array NUM_POKEMON = 151)  = $D30A
//           − 19 (wPokedexOwned)                               = $D2F7
//
// Both land on $D2F7, which `wram.asm` also labels `wMainDataStart`.

/// `wPokedexOwned` — 19 bytes of flags, one bit per species.
pub const W_POKEDEX_OWNED: u16 = 0xD2F7;
/// `wPokedexSeen`.
pub const W_POKEDEX_SEEN: u16 = 0xD30A;
/// Bytes in each flag array: `NUM_POKEMON` = 151, rounded up to whole bytes.
pub const DEX_BYTES: usize = 19;
/// `wNumBagItems`.
pub const W_NUM_BAG_ITEMS: u16 = 0xD31D;

/// Map names, deferred to the agent plugin's table so there is one of them.
pub fn map_name(id: u8) -> &'static str {
    terminalgb_plugin_agent::observe::map_name(id)
}

/// How a run is set up. Everything with a cost or a risk is explicit.
#[derive(Debug, Clone)]
pub struct RunConfig {
    /// Where the log, checkpoint and report go.
    pub dir: PathBuf,
    /// One sentence, handed to the agent every call.
    pub objective: String,
    /// Emulated-frame ceiling. Zero means no limit (the wall clock still
    /// applies).
    pub max_frames: u64,
    /// Wall-clock ceiling for **this session**.
    pub max_wall: Duration,
    /// Game-seconds of flat progress before a wall is declared.
    pub wall_after_seconds: u32,
    /// Checkpoint every this many emulated frames.
    pub checkpoint_every: u64,
    /// Print a heartbeat this often. A heartbeat says what has happened since
    /// the last one, and says so briefly when the answer is "nothing".
    pub heartbeat: Duration,
    /// Run at 59.7 fps instead of flat out. For a stream, not for a long run.
    pub pace_realtime: bool,
    /// Pace to real time **only while a model call is in flight**.
    ///
    /// The emulator never blocks on the agent, which is right — but headless
    /// and uncapped that means a ten-second call is ~40,000 frames of game
    /// running on with nobody at the controller, and the plan that comes back
    /// is aimed at a world that has moved. This bounds it to what a real
    /// player's console would have done in the same wall-clock time, at the
    /// cost of the wall clock: the run is no longer faster than real time
    /// while thinking. Off by default, because "the emulator never waits" is
    /// the property the design is built on and changing it must be a choice.
    pub pace_while_thinking: bool,
    /// Resume from the checkpoint in `dir` if one is there. **A rewind** — see
    /// the module docs.
    pub resume: bool,
    /// Stop when this is true: `badges>=1`, `map=0x02`, `party>=1`.
    pub until: Option<String>,
    pub ntfy: Option<notify::NtfyConfig>,
    /// Named memory locations to sample and record. **Reporting only** — a
    /// watch is never shown to the agent; see [`watch`].
    pub watches: Watches,
    /// Write a save state when the run faults or is declared stuck, so the
    /// failure is reproducible rather than merely described.
    pub capture_failures: bool,
    /// Echo commentary and milestones to stdout.
    pub verbose: bool,
}

impl Default for RunConfig {
    fn default() -> Self {
        RunConfig {
            dir: PathBuf::from("target/longplay"),
            objective: String::new(),
            max_frames: 0,
            max_wall: Duration::from_secs(4 * 60 * 60),
            wall_after_seconds: 20 * 60,
            checkpoint_every: 60 * 60 * 5, // ~5 minutes of game time
            heartbeat: Duration::from_secs(300),
            pace_realtime: false,
            pace_while_thinking: false,
            resume: false,
            until: None,
            ntfy: None,
            watches: Watches::default(),
            capture_failures: true,
            verbose: true,
        }
    }
}

/// What one decision looked like, for `steps.ndjson`.
struct Step {
    decision: u64,
    frame_started: u64,
    observation: Progress,
    score_before: u64,
}

/// Drives one long run.
pub struct Runner {
    gb: Gameboy,
    cfg: RunConfig,
    journal: Journal,
    notifier: Notifier,
    detector: progress::Detector,

    started: Instant,
    /// Wall seconds carried over from earlier sessions of this run.
    prior_wall: u64,
    /// Frames carried over from earlier sessions.
    prior_frames: u64,
    frame: u64,

    milestones: Vec<Milestone>,
    resumes: u32,
    prior_calls: u32,
    prior_decisions: u64,
    prior_tokens: u64,
    prior_usd: f64,

    last: Progress,
    last_watch: Sample,
    /// Emulated frames spent on each map — the number that says where the
    /// agent is actually stuck.
    areas: BTreeMap<u8, u64>,
    captures: Vec<PathBuf>,
    step: Option<Step>,
    seen_decisions: u64,
    worst_stall: u32,
    wall_reports: u32,
    frames_thinking: u64,
    frames_executing: u64,
    steps_written: u64,
    /// Loop bookkeeping, held on the struct so [`Runner::tick`] can be called
    /// one frame at a time from outside.
    next_frame_at: Instant,
    last_beat: Instant,
    beat_milestones: usize,
    last_checkpoint: u64,
}

/// Why a run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    FrameBudget,
    WallClock,
    AgentStopped(String),
    Reached(String),
    Fault(String),
}

impl Stop {
    pub fn text(&self) -> String {
        match self {
            Stop::FrameBudget => "the emulated-frame budget ran out".into(),
            Stop::WallClock => "the wall-clock budget ran out".into(),
            Stop::AgentStopped(w) => format!("the agent stopped — {w}"),
            Stop::Reached(w) => format!("it reached what it was asked for ({w})"),
            Stop::Fault(w) => format!("the emulator faulted — {w}"),
        }
    }
}

impl Runner {
    /// Build a run. The caller supplies an already-configured `Gameboy` with the
    /// agent attached, because how the driver is chosen is the front end's
    /// business and there are several.
    pub fn new(gb: Gameboy, cfg: RunConfig) -> Result<Runner, String> {
        let journal = Journal::open(&cfg.dir)
            .map_err(|e| format!("cannot open {}: {e}", cfg.dir.display()))?;
        let notifier = match cfg.ntfy.clone() {
            Some(n) => Notifier::start(n),
            None => Notifier::disabled(),
        };
        Ok(Runner {
            detector: progress::Detector::new(cfg.wall_after_seconds),
            gb,
            journal,
            notifier,
            cfg,
            started: Instant::now(),
            prior_wall: 0,
            prior_frames: 0,
            frame: 0,
            milestones: Vec::new(),
            resumes: 0,
            prior_calls: 0,
            prior_decisions: 0,
            prior_tokens: 0,
            prior_usd: 0.0,
            last: Progress::default(),
            last_watch: Sample::default(),
            areas: BTreeMap::new(),
            captures: Vec::new(),
            step: None,
            seen_decisions: 0,
            worst_stall: 0,
            wall_reports: 0,
            frames_thinking: 0,
            frames_executing: 0,
            steps_written: 0,
            next_frame_at: Instant::now(),
            last_beat: Instant::now(),
            beat_milestones: 0,
            last_checkpoint: 0,
        })
    }

    /// Resume from the checkpoint in the run directory, if there is one.
    ///
    /// **This rewinds the game.** Legitimate after a crash, illegitimate as a
    /// way to make an agent look competent, and the two are told apart by
    /// making it impossible to do quietly: it is counted, it is a `Major`
    /// milestone, and it is printed in the report's Declared section.
    pub fn resume(&mut self) -> Result<bool, String> {
        let Some((cp, state)) = Journal::read_checkpoint(&self.cfg.dir)? else {
            return Ok(false);
        };
        let title = self.rom_title();
        if !cp.rom_title.is_empty() && cp.rom_title != title {
            return Err(format!(
                "this checkpoint is from {:?} and the cartridge is {:?} — refusing \
                 to resume into a different game",
                cp.rom_title, title
            ));
        }
        self.gb
            .load_state(&state)
            .map_err(|e| format!("the checkpoint's save state was rejected: {e:?}"))?;
        self.prior_frames = cp.frame;
        self.prior_wall = cp.wall_seconds;
        self.prior_calls = cp.calls;
        self.prior_decisions = cp.decisions;
        self.prior_tokens = cp.tokens;
        self.prior_usd = cp.usd;
        self.resumes = cp.resumes + 1;
        self.detector.restore(
            cp.visited.clone(),
            cp.badges,
            cp.levels.clone(),
            cp.owned,
            cp.wall_best,
            cp.play_seconds,
        );
        let m = MilestoneKind::Resumed {
            checkpoint_frame: cp.frame,
            resumes: self.resumes,
        };
        self.record(m, cp.play_seconds);
        Ok(true)
    }

    fn rom_title(&mut self) -> String {
        self.gb.with_machine(Capability::OBSERVE, |m| m.rom_title())
    }

    /// Read everything the run watches, plus the declared memory watches.
    fn sample(&mut self) -> (Progress, Sample) {
        let watches = self.cfg.watches.clone();
        self.gb
            .with_machine(Capability::OBSERVE | Capability::READ_MEMORY, |m| {
                (read_progress(m), watches.read(m))
            })
    }
}

/// Read everything a run watches out of a machine. ~40 bus reads.
///
/// A free function rather than a method so the episode runner
/// ([`series::Series`]) reads exactly the same bytes the same way — two
/// samplers that drift apart would make an episode's numbers incomparable with
/// a long run's.
pub fn read_progress(m: &mut crate::plugin::Machine<'_>) -> Progress {
    let mut p = Progress::default();
    let mut name = [0u8; 11];
    m.peek_into(tr::PLAYER_NAME, &mut name);
    let count = m.peek(tr::PARTY_COUNT).unwrap_or(0xFF);
    // The same "is a save really loaded?" test the observer makes: before
    // that, every byte below is uninitialised WRAM.
    p.in_game = terminalgb_plugin_trainer::text::looks_like_a_name(&name) && count <= 6;
    if !p.in_game {
        return p;
    }
    p.map = m
        .peek(terminalgb_plugin_agent::observe::W_CUR_MAP)
        .unwrap_or(0);
    p.x = m
        .peek(terminalgb_plugin_agent::observe::W_X_COORD)
        .unwrap_or(0);
    p.y = m
        .peek(terminalgb_plugin_agent::observe::W_Y_COORD)
        .unwrap_or(0);
    p.badges = m.peek(tr::OBTAINED_BADGES).unwrap_or(0);
    p.money = terminalgb_plugin_trainer::text::bcd3([
        m.peek(tr::PLAYER_MONEY).unwrap_or(0),
        m.peek(tr::PLAYER_MONEY + 1).unwrap_or(0),
        m.peek(tr::PLAYER_MONEY + 2).unwrap_or(0),
    ]);
    use terminalgb_plugin_trainer::gen1::party as pm;
    for i in 0..count as u16 {
        let base = tr::PARTY_MONS + i * terminalgb_plugin_trainer::gen1::PARTY_MON_SIZE;
        let species = m.peek(base + pm::SPECIES).unwrap_or(0);
        let level = m.peek(base + pm::LEVEL).unwrap_or(0);
        // Big-endian, like every multi-byte field in Gen 1.
        let hp = u16::from_be_bytes([
            m.peek(base + pm::HP).unwrap_or(0),
            m.peek(base + pm::HP + 1).unwrap_or(0),
        ]);
        p.party.push((species, level));
        p.party_hp += hp as u32;
    }
    let mut dex = [0u8; DEX_BYTES];
    m.peek_into(W_POKEDEX_OWNED, &mut dex);
    p.owned = dex.iter().map(|b| b.count_ones() as u16).sum();
    m.peek_into(W_POKEDEX_SEEN, &mut dex);
    p.seen = dex.iter().map(|b| b.count_ones() as u16).sum();
    p.items = m.peek(W_NUM_BAG_ITEMS).unwrap_or(0).min(20);
    p.play_seconds = m.peek(tr::PLAY_TIME_HOURS).unwrap_or(0) as u32 * 3600
        + m.peek(tr::PLAY_TIME_MINUTES).unwrap_or(0).min(59) as u32 * 60
        + m.peek(tr::PLAY_TIME_SECONDS).unwrap_or(0).min(59) as u32;
    p
}

impl Runner {
    fn wall_seconds(&self) -> u64 {
        self.prior_wall + self.started.elapsed().as_secs()
    }

    /// The same span in milliseconds — what the report's rates divide by.
    fn wall_ms(&self) -> u128 {
        self.prior_wall as u128 * 1000 + self.started.elapsed().as_millis()
    }

    fn total_frames(&self) -> u64 {
        self.prior_frames + self.frame
    }

    /// Note a milestone: journal, phone, stdout, report.
    fn record(&mut self, kind: MilestoneKind, play_seconds: u32) {
        let m = Milestone {
            frame: self.total_frames(),
            play_seconds,
            wall_seconds: self.wall_seconds(),
            map: self.last.map,
            kind,
        };
        if let MilestoneKind::Wall {
            stalled_seconds, ..
        } = m.kind
        {
            self.worst_stall = self.worst_stall.max(stalled_seconds);
            self.wall_reports += 1;
        }
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", journal::EVENT_SCHEMA);
        w.str_field("ev", "milestone");
        w.str_field("tag", m.kind.tag());
        w.str_field("importance", m.importance().label());
        w.str_field("text", &m.kind.text());
        w.num_field("frame", m.frame as f64);
        w.num_field("play_seconds", m.play_seconds as f64);
        w.num_field("wall_seconds", m.wall_seconds as f64);
        w.num_field("map", m.map as f64);
        w.end_obj();
        self.journal.write(&w.finish());

        if self.cfg.verbose {
            println!(
                "  ★ [{} game / {} wall] {}",
                progress::human_seconds(m.play_seconds),
                progress::human_seconds(m.wall_seconds as u32),
                m.kind.text()
            );
        }
        self.notifier.push(Push {
            title: push_title(&m.kind),
            body: m.kind.text(),
            importance: m.importance(),
        });
        self.milestones.push(m);
    }

    /// Has a decision completed since the last check? If so, close the step
    /// record and open the next one.
    ///
    /// A decision is *taken* when the plugin's counter moves and *finished*
    /// when the plan it produced stops running. Both edges are needed: the
    /// observation belongs to the first and the outcome to the second.
    fn track_decision(&mut self, now: &Progress) {
        let (decisions, phase, goal, say, outcomes) = {
            let Some(p) = self.gb.plugin::<AgentPlugin>() else {
                return;
            };
            let c = p.commentary();
            let say = c
                .history()
                .rev()
                .find(|e| e.kind == terminalgb_plugin_agent::commentary::Kind::Thought)
                .map(|e| e.text.clone())
                .unwrap_or_default();
            (
                p.decisions(),
                p.phase(),
                c.goal().to_string(),
                say,
                p.last_plan()
                    .iter()
                    .map(|o| (o.action.clone(), o.result.clone(), o.changed))
                    .collect::<Vec<_>>(),
            )
        };

        // A plan has just finished: everything the step record needs exists.
        if self.step.is_some() && phase != PhaseView::Executing && !outcomes.is_empty() {
            let step = self.step.take().expect("just checked");
            self.write_step(&step, now, &goal, &say, &outcomes);
        }

        if decisions > self.seen_decisions {
            self.seen_decisions = decisions;
            self.step = Some(Step {
                decision: decisions,
                frame_started: self.total_frames(),
                observation: self.last.clone(),
                score_before: self.last.score(self.detector.maps_visited()),
            });
        }
    }

    /// One line of `steps.ndjson`: observation → intent → actions → outcome.
    ///
    /// Deliberately the four things and no more. It is a by-product of numbers
    /// the report already needs, so it costs a run nothing and it makes a
    /// session replayable by something that was not there.
    fn write_step(
        &mut self,
        s: &Step,
        after: &Progress,
        goal: &str,
        say: &str,
        outcomes: &[(String, String, bool)],
    ) {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", "gblongplay-step/1");
        w.num_field("decision", s.decision as f64);
        w.num_field("frame", s.frame_started as f64);
        w.num_field("frames", (self.total_frames() - s.frame_started) as f64);
        w.num_field("wall_seconds", self.wall_seconds() as f64);

        w.obj_field("observation");
        write_progress(&mut w, &s.observation);
        w.end_obj();

        w.obj_field("intent");
        w.str_field("goal", goal);
        w.str_field("say", say);
        w.end_obj();

        w.arr_field("actions");
        for (action, result, changed) in outcomes {
            w.begin_obj();
            w.str_field("action", action);
            w.str_field("result", result);
            w.bool_field("changed", *changed);
            w.end_obj();
        }
        w.end_arr();

        let after_score = after.score(self.detector.maps_visited());
        w.obj_field("outcome");
        w.bool_field(
            "moved",
            after.map != s.observation.map
                || after.x != s.observation.x
                || after.y != s.observation.y,
        );
        w.num_field("score_before", s.score_before as f64);
        w.num_field("score_after", after_score as f64);
        w.bool_field("progressed", after_score > s.score_before);
        w.end_obj();

        w.obj_field("state_after");
        write_progress(&mut w, after);
        w.end_obj();
        w.end_obj();

        let line = w.finish();
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.cfg.dir.join("steps.ndjson"))
        {
            use std::io::Write as _;
            let _ = writeln!(f, "{line}");
        }
        self.steps_written += 1;
    }

    fn write_checkpoint(&mut self, p: &Progress) {
        let (calls, decisions, tokens, usd) = self.agent_counters();
        let cp = Checkpoint {
            frame: self.total_frames(),
            play_seconds: p.play_seconds,
            wall_seconds: self.wall_seconds(),
            resumes: self.resumes,
            calls,
            decisions,
            milestones: self.milestones.len() as u64,
            tokens,
            usd,
            visited: self.detector.visited().clone(),
            badges: self.detector.badges_reported(),
            levels: self.detector.best_levels().to_vec(),
            owned: self.detector.best_owned(),
            wall_best: self.detector.wall.best(),
            objective: self.cfg.objective.clone(),
            rom_title: self.rom_title(),
        };
        let state = self.gb.save_state();
        if let Err(e) = self.journal.checkpoint(&cp, &state) {
            eprintln!("longplay: checkpoint failed: {e}");
        }
    }

    /// Write the machine's save state beside the log, so a failure is
    /// **reproducible** rather than merely described.
    ///
    /// A failing run you can reload is worth ten you cannot: the state is the
    /// whole machine at the moment it went wrong, and it can be handed to a
    /// person, a debugger, or the next version of the agent. Named for what
    /// went wrong and when, so a directory of them reads as a list.
    pub fn capture(&mut self, why: &str) -> Option<PathBuf> {
        if !self.cfg.capture_failures {
            return None;
        }
        let dir = self.cfg.dir.join("captures");
        if std::fs::create_dir_all(&dir).is_err() {
            return None;
        }
        let path = dir.join(format!("{why}-f{:08}.gbss", self.total_frames()));
        let state = self.gb.save_state();
        if std::fs::write(&path, &state).is_err() {
            return None;
        }
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", journal::EVENT_SCHEMA);
        w.str_field("ev", "capture");
        w.str_field("why", why);
        w.str_field("state", &path.to_string_lossy());
        w.num_field("frame", self.total_frames() as f64);
        w.num_field("map", self.last.map as f64);
        w.str_field("map_name", map_name(self.last.map));
        w.num_field("x", self.last.x as f64);
        w.num_field("y", self.last.y as f64);
        w.end_obj();
        self.journal.write(&w.finish());
        if self.cfg.verbose {
            println!("  ⎘ saved the machine as it stands: {}", path.display());
        }
        self.captures.push(path.clone());
        Some(path)
    }

    /// Record a change in the watched addresses.
    ///
    /// **This goes to the log, never to the agent.** See [`watch`] for why the
    /// two paths are kept apart, and `testharness/longplay.rs` for the check
    /// that they still are.
    fn log_watch(&mut self, w: &Sample, p: &Progress) {
        let mut out = json::Writer::new();
        out.begin_obj();
        out.str_field("schema", journal::EVENT_SCHEMA);
        out.str_field("ev", "watch");
        out.num_field("frame", self.total_frames() as f64);
        out.num_field("play_seconds", p.play_seconds as f64);
        out.num_field("map", p.map as f64);
        out.obj_field("values");
        for (name, value) in &w.0 {
            out.num_field(name, *value as f64);
        }
        out.end_obj();
        out.end_obj();
        self.journal.write(&out.finish());
    }

    /// Frames spent on each map, commonest first.
    pub fn areas(&self) -> Vec<(u8, u64)> {
        let mut v: Vec<(u8, u64)> = self.areas.iter().map(|(k, n)| (*k, *n)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    fn agent_counters(&self) -> (u32, u64, u64, f64) {
        match self.gb.plugin::<AgentPlugin>() {
            Some(p) => {
                let g = p.governor();
                (
                    self.prior_calls + g.calls(),
                    self.prior_decisions + p.decisions(),
                    self.prior_tokens + g.tokens(),
                    self.prior_usd + g.usd(),
                )
            }
            None => (
                self.prior_calls,
                self.prior_decisions,
                self.prior_tokens,
                self.prior_usd,
            ),
        }
    }

    /// The machine, so an embedder can interleave its own work with the run —
    /// and so the harness can stand in for a cartridge by writing the RAM a
    /// real Gen 1 game would write.
    ///
    /// **This is not a hole in the "the agent only presses buttons"
    /// guarantee.** That guarantee is enforced per call by the capability the
    /// *plugin* holds — `Machine::poke` returns `false` for a plugin that did
    /// not declare `WRITE_MEMORY`, and the agent does not — rather than by
    /// hiding the machine from the process that owns it. The runner itself
    /// never pokes; the only write it makes to the machine is loading a
    /// checkpoint on an explicit `--resume`, which is counted and declared.
    pub fn gameboy_mut(&mut self) -> &mut Gameboy {
        &mut self.gb
    }

    /// Announce the run and write the `start` event. Call once, before
    /// [`Runner::tick`]. [`Runner::run`] does it for you.
    pub fn begin(&mut self) {
        let start_line =
            format!(
            "longplay: objective={:?} caps: {} frames, {}s wall, wall-detector {}s of \
             game time, checkpoint every {} frames, ntfy {}",
            self.cfg.objective,
            if self.cfg.max_frames == 0 {
                "no".to_string()
            } else {
                self.cfg.max_frames.to_string()
            },
            self.cfg.max_wall.as_secs(),
            self.cfg.wall_after_seconds,
            self.cfg.checkpoint_every,
            if self.notifier.is_enabled() { "on" } else { "off" },
        );
        if self.cfg.verbose {
            println!("{start_line}");
        }
        self.journal.write(&{
            let mut w = json::Writer::new();
            w.begin_obj();
            w.str_field("schema", journal::EVENT_SCHEMA);
            w.str_field("ev", "start");
            w.str_field("objective", &self.cfg.objective);
            w.num_field("resumes", self.resumes as f64);
            w.num_field("unix", unix_now() as f64);
            w.end_obj();
            w.finish()
        });
        self.next_frame_at = Instant::now() + FRAME_TIME;
        self.last_beat = Instant::now();
    }

    /// Run **one emulated frame** and everything that hangs off it: sampling,
    /// milestones, the step record, the checkpoint, the heartbeat and the stop
    /// conditions. `Some(stop)` means the run is over.
    ///
    /// Split out from [`Runner::run`] so a harness — or an operator embedding
    /// the runner in something larger — can drive the loop in slices and do
    /// its own work between frames. `run` is this in a loop.
    pub fn tick(&mut self) -> Option<Stop> {
        self.gb.frame();
        self.frame += 1;

        // Where the frame went. The emulator never blocks on the agent, so a
        // ten-second round trip is tens of thousands of frames of game running
        // on with nobody at the controller — that is not a bug, but it must be
        // measured or every efficiency number here is flattery.
        match self.gb.plugin::<AgentPlugin>().map(|p| p.phase()) {
            Some(PhaseView::Thinking) => self.frames_thinking += 1,
            Some(PhaseView::Executing) => self.frames_executing += 1,
            _ => {}
        }

        if self.frame % SAMPLE_EVERY == 0 {
            let (p, w) = self.sample();
            // Attribute this slice of frames to the map it was spent on. A
            // sample is `SAMPLE_EVERY` frames' worth, which is the resolution
            // the answer needs — a run is measured in thousands of frames per
            // area, not in sixes.
            if p.in_game {
                *self.areas.entry(p.map).or_insert(0) += SAMPLE_EVERY;
            }
            let was_stuck = self.detector.wall.stuck();
            for kind in self.detector.observe(&p) {
                self.record(kind, p.play_seconds);
            }
            if !was_stuck && self.detector.wall.stuck() {
                self.capture("wedged");
            }
            if w.differs(&self.last_watch) {
                self.log_watch(&w, &p);
            }
            self.last = p.clone();
            self.last_watch = w;
            self.track_decision(&p);

            if self.total_frames() - self.last_checkpoint >= self.cfg.checkpoint_every {
                self.last_checkpoint = self.total_frames();
                self.write_checkpoint(&p);
            }
        }

        // -- stopping -------------------------------------------------------
        if let Some(f) = self.gb.fault() {
            let f = f.to_string();
            self.capture("fault");
            return Some(Stop::Fault(f));
        }
        if self.cfg.max_frames > 0 && self.frame >= self.cfg.max_frames {
            return Some(Stop::FrameBudget);
        }
        if self.started.elapsed() >= self.cfg.max_wall {
            return Some(Stop::WallClock);
        }
        if let Some(halted) = self.agent_halted() {
            self.record(
                MilestoneKind::AgentHalted {
                    why: halted.clone(),
                },
                self.last.play_seconds,
            );
            return Some(Stop::AgentStopped(halted));
        }
        if let Some(want) = self.cfg.until.clone() {
            if self.reached(&want) {
                return Some(Stop::Reached(want));
            }
        }

        // -- heartbeat ------------------------------------------------------
        if self.last_beat.elapsed() >= self.cfg.heartbeat {
            let fresh = self.milestones.len() - self.beat_milestones;
            self.beat_milestones = self.milestones.len();
            self.last_beat = Instant::now();
            self.heartbeat(fresh);
        }

        let thinking = self.gb.plugin::<AgentPlugin>().map(|p| p.phase())
            == Some(PhaseView::Thinking);
        if self.cfg.pace_realtime || (self.cfg.pace_while_thinking && thinking) {
            let now = Instant::now();
            if now < self.next_frame_at {
                std::thread::sleep(self.next_frame_at - now);
                self.next_frame_at += FRAME_TIME;
            } else {
                self.next_frame_at = now + FRAME_TIME;
            }
        }
        None
    }

    /// Close the run: a final sample and checkpoint, the battery flushed, and
    /// the scoreboard written. Consumes the runner.
    pub fn end(mut self, stop: &Stop) -> Scoreboard {
        let (p, w) = self.sample();
        self.last = p.clone();
        self.last_watch = w;
        self.write_checkpoint(&p);
        self.gb.flush_battery();
        self.finish(stop, &p)
    }

    /// Run until something stops it. Returns the reason and the scoreboard.
    pub fn run(mut self) -> (Stop, Scoreboard) {
        self.begin();
        let stop = loop {
            if let Some(s) = self.tick() {
                break s;
            }
        };
        let board = self.end(&stop);
        (stop, board)
    }

    fn agent_halted(&self) -> Option<String> {
        let p = self.gb.plugin::<AgentPlugin>()?;
        if p.phase() != PhaseView::Stopped || !p.is_enabled() {
            return None;
        }
        Some(
            p.commentary()
                .pause_reason()
                .unwrap_or("it had nothing more to say")
                .to_string(),
        )
    }

    /// A heartbeat says what has happened, and says it briefly when nothing
    /// has. It is deliberately **not** a frame counter: a supervisor who reads
    /// "412,000 frames" learns nothing, and one who reads "nothing new for 20
    /// minutes, still in Viridian Forest" learns everything.
    fn heartbeat(&mut self, fresh: usize) {
        let (calls, _, _, usd) = self.agent_counters();
        let where_ = if self.last.in_game {
            let n = map_name(self.last.map);
            if n.is_empty() {
                format!("map {:#04x}", self.last.map)
            } else {
                n.to_string()
            }
        } else {
            "not in the game yet".to_string()
        };
        let fpc = if calls == 0 {
            0.0
        } else {
            self.total_frames() as f64 / calls as f64
        };
        let line = if fresh == 0 {
            format!(
                "… nothing new for {}. In {where_}, {} badges. {calls} calls, \
                 {fpc:.0} frames/call, ${usd:.2}.{}",
                progress::human_seconds(self.cfg.heartbeat.as_secs() as u32),
                self.last.badges.count_ones(),
                if self.detector.wall.stuck() {
                    format!(
                        " STUCK — {} tiles, no progress.",
                        self.detector.wall.churn()
                    )
                } else {
                    String::new()
                }
            )
        } else {
            format!(
                "… {fresh} new milestone(s). In {where_}, {} badges, {} maps seen. \
                 {calls} calls, {fpc:.0} frames/call, ${usd:.2}.",
                self.last.badges.count_ones(),
                self.detector.maps_visited(),
            )
        };
        if self.cfg.verbose {
            println!("{line}");
        }
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", journal::EVENT_SCHEMA);
        w.str_field("ev", "heartbeat");
        w.str_field("text", &line);
        w.num_field("frame", self.total_frames() as f64);
        w.num_field("frames_per_call", (fpc * 100.0).round() / 100.0);
        w.num_field("calls", calls as f64);
        w.num_field("new_milestones", fresh as f64);
        w.bool_field("stuck", self.detector.wall.stuck());
        w.end_obj();
        self.journal.write(&w.finish());
    }

    /// `badges>=N`, `party>=N`, `map=0xNN`, `owned>=N`.
    fn reached(&self, want: &str) -> bool {
        let p = &self.last;
        if !p.in_game {
            return false;
        }
        let n = |s: &str| s.trim().parse::<u32>().ok();
        if let Some(v) = want.strip_prefix("badges>=").and_then(n) {
            return p.badges.count_ones() >= v;
        }
        if let Some(v) = want.strip_prefix("party>=").and_then(n) {
            return p.party.len() as u32 >= v;
        }
        if let Some(v) = want.strip_prefix("owned>=").and_then(n) {
            return p.owned as u32 >= v;
        }
        if let Some(v) = want.strip_prefix("map=") {
            let v = v.trim();
            let id = v
                .strip_prefix("0x")
                .and_then(|h| u8::from_str_radix(h, 16).ok())
                .or_else(|| v.parse().ok());
            return Some(p.map) == id;
        }
        false
    }

    fn finish(&mut self, stop: &Stop, p: &Progress) -> Scoreboard {
        let (calls, decisions, tokens, usd) = self.agent_counters();
        let (driver, failed, repeats) = match self.gb.plugin::<AgentPlugin>() {
            Some(a) => (
                a.driver_name().to_string(),
                a.governor().failures(),
                a.stuck_worst(),
            ),
            None => (String::from("none"), 0, 0),
        };
        let board = Scoreboard {
            objective: self.cfg.objective.clone(),
            rom_title: self.rom_title(),
            driver,
            frames: self.total_frames(),
            wall_seconds: self.wall_seconds(),
            wall_ms: self.wall_ms(),
            play_seconds: p.play_seconds,
            calls,
            decisions,
            failed_calls: failed,
            frames_thinking: self.frames_thinking,
            frames_executing: self.frames_executing,
            tokens,
            usd,
            milestones: self.milestones.clone(),
            maps_visited: self.detector.maps_visited(),
            badges: p.badges,
            party: p.party.clone(),
            final_map: p.map,
            final_map_name: map_name(p.map),
            final_x: p.x,
            final_y: p.y,
            wall_reports: self.wall_reports,
            worst_stall_seconds: self.worst_stall,
            worst_repeats: repeats,
            areas: self.areas(),
            captures: self.captures.clone(),
            resumes: self.resumes,
            memory_writes: 0,
            stopped_because: stop.text(),
            ntfy: self.notifier.is_enabled().then(|| self.notifier.stats()),
        };
        self.journal.write(&board.to_json());
        let _ = self.journal.write_report("report.md", &board.to_markdown());
        let _ = self.journal.write_report("report.json", &board.to_json());

        // The final push always goes, whatever the threshold: a run ending is
        // the one thing somebody who walked away definitely wants told.
        self.notifier.push(Push {
            title: "Run finished".into(),
            body: format!("{} · {}", stop.text(), board.headline()),
            importance: Importance::Major,
        });
        self.notifier.drain(Duration::from_secs(5));
        board
    }

    /// Steps written to `steps.ndjson` this session.
    pub fn steps_written(&self) -> u64 {
        self.steps_written
    }
}

fn write_progress(w: &mut json::Writer, p: &Progress) {
    w.bool_field("in_game", p.in_game);
    // Before a save is loaded every field below is uninitialised WRAM. Writing
    // it anyway would put "Pallet Town, 0 badges" in a dataset as if it were
    // an observation, which is worse than writing nothing — a learner cannot
    // tell a real zero from a not-yet-a-game zero.
    if !p.in_game {
        return;
    }
    w.num_field("map", p.map as f64);
    w.str_field("map_name", map_name(p.map));
    w.num_field("x", p.x as f64);
    w.num_field("y", p.y as f64);
    w.num_field("badges", p.badges.count_ones() as f64);
    w.num_field("money", p.money as f64);
    w.num_field("owned", p.owned as f64);
    w.num_field("seen", p.seen as f64);
    w.num_field("items", p.items as f64);
    w.num_field("play_seconds", p.play_seconds as f64);
    w.num_field("party_hp", p.party_hp as f64);
    w.arr_field("party");
    for (species, level) in &p.party {
        w.begin_obj();
        w.num_field("species", *species as f64);
        w.num_field("level", *level as f64);
        w.end_obj();
    }
    w.end_arr();
}

/// The phone's notification title — short, and it says what happened before it
/// says where.
fn push_title(k: &MilestoneKind) -> String {
    match k {
        MilestoneKind::Badge { .. } => "Badge won".into(),
        MilestoneKind::NewArea { name, .. } if !name.is_empty() => {
            format!("Reached {name}")
        }
        MilestoneKind::NewArea { .. } => "Somewhere new".into(),
        MilestoneKind::FirstPokemon { .. } => "Got a Pokémon".into(),
        MilestoneKind::PartyGrew { .. } => "Party grew".into(),
        MilestoneKind::LevelUp { .. } => "Level up".into(),
        MilestoneKind::Caught { .. } => "New species".into(),
        MilestoneKind::Wiped => "Party wiped".into(),
        MilestoneKind::Wall { .. } => "Stuck".into(),
        MilestoneKind::Unstuck { .. } => "Moving again".into(),
        MilestoneKind::AgentHalted { .. } => "Agent stopped".into(),
        MilestoneKind::InControl => "Run started".into(),
        MilestoneKind::Resumed { .. } => "Run resumed".into(),
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The walk in the header comment, checked here so a future edit to those
    /// constants has to argue with a test rather than with a comment.
    #[test]
    fn the_pokedex_walk_lands_on_both_of_its_anchors() {
        // Forwards from wPartyCount, which the trainer crate verifies.
        let party_block = 1 + 7 + 6 * 44 + 6 * 11 + 6 * 11;
        assert_eq!(tr::PARTY_COUNT as usize + party_block, 0xD2F7);
        assert_eq!(W_POKEDEX_OWNED, 0xD2F7);
        // Backwards from wPlayerMoney, likewise verified.
        let bag = 20 * 2 + 1; // BAG_ITEM_CAPACITY * 2 + 1
        let dex = DEX_BYTES; // flag_array NUM_POKEMON, 151 species
        assert_eq!(
            tr::PLAYER_MONEY as usize - bag,
            W_NUM_BAG_ITEMS as usize + 1
        );
        assert_eq!(W_NUM_BAG_ITEMS as usize - dex, W_POKEDEX_SEEN as usize);
        assert_eq!(W_POKEDEX_SEEN as usize - dex, W_POKEDEX_OWNED as usize);
        assert_eq!(DEX_BYTES, 151_usize.div_ceil(8));
    }

    #[test]
    fn a_stop_reason_is_always_a_sentence_a_person_can_read() {
        for s in [
            Stop::FrameBudget,
            Stop::WallClock,
            Stop::AgentStopped("call-cap".into()),
            Stop::Reached("badges>=1".into()),
            Stop::Fault("undefined opcode".into()),
        ] {
            let t = s.text();
            assert!(
                !t.is_empty() && t.chars().next().unwrap().is_lowercase(),
                "{t}"
            );
        }
    }

    #[test]
    fn every_milestone_has_a_short_push_title() {
        let k = MilestoneKind::NewArea {
            map: 1,
            name: "Viridian City",
            named: true,
        };
        assert_eq!(push_title(&k), "Reached Viridian City");
        // A title is a header value: short, one line, no punctuation soup.
        for k in [
            MilestoneKind::Wiped,
            MilestoneKind::Wall {
                stalled_seconds: 1,
                times: 1,
                pinned: true,
                churn: 1,
                map: 0,
                name: "",
            },
            MilestoneKind::InControl,
        ] {
            let t = push_title(&k);
            assert!(t.len() <= 40 && !t.contains('\n'), "{t:?}");
        }
    }
}

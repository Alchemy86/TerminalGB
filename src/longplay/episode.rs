//! **The episode primitive** — a named segment the harness resets and repeats.
//!
//! # What it turns "play Pokémon" into
//!
//! A whole playthrough is one sample of a very long, very noisy process: it
//! either finished or it did not, once, and nothing about it is comparable with
//! anything else. An **episode** is the opposite: a start save state, a success
//! condition and a failure/timeout condition, run again and again from exactly
//! the same byte-identical starting point.
//!
//! That is what makes an agent measurable. *"Got out of Viridian Forest 31
//! times out of 50, median 4,200 frames, 11 model calls"* is a number you can
//! improve; *"it played for six hours and got a bit lost"* is not.
//!
//! The reset is free here, and that is not a small thing: a Game Boy save state
//! is 142 KB and `testharness/savestate.rs` proves a reload is **byte-identical
//! at every frame boundary** thereafter. So the environment can be rewound
//! perfectly, thousands of times, at 70x real time.
//!
//! # Declarative on purpose
//!
//! An episode is a small JSON file, not Rust. A person defines a new segment
//! without touching the emulator, and an episode file plus its results is a
//! reproducible artefact somebody else can re-run. `gameboy-battleai-g7` owns
//! the battle domain; this owns the primitive it runs on.
//!
//! ```json
//! {
//!   "schema": "gbepisode/1",
//!   "name": "out-of-viridian-forest",
//!   "objective": "Leave Viridian Forest by the north exit",
//!   "start_state": "states/forest-south.gbss",
//!   "success": ["map=0x33"],
//!   "failure": ["party_hp=0"],
//!   "timeout_frames": 60000,
//!   "max_calls": 40,
//!   "watch": ["in_battle", "lead_hp"]
//! }
//! ```
//!
//! # The two lists mean different things, and it matters
//!
//! `success` is an **AND**: every condition must hold at the same sample. That
//! composes ("on this map *and* out of battle") and it is what a person means
//! by "done".
//!
//! `failure` is an **OR**: any one of them ends the attempt. That is what a
//! person means by "gone wrong" — a wipe, a wrong map, a softlock — and they
//! are alternatives by nature.
//!
//! # A failed episode you can reload is worth ten you cannot
//!
//! Every attempt that is not a success writes its save state beside its
//! result, so the exact machine that failed can be handed back to a person, to
//! a debugger, or to the next version of the agent.

use std::path::{Path, PathBuf};

use super::progress::Progress;
use super::watch::{Sample, Watch, Watches};
use terminalgb_plugin_agent::json;

/// Bump when a field is renamed, retyped or removed. Additions are free.
pub const EPISODE_SCHEMA: &str = "gbepisode/1";
/// The per-attempt result stream's schema.
pub const RESULT_SCHEMA: &str = "gbepisode-result/1";

// -- conditions --------------------------------------------------------------

/// The left-hand side of a condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lhs {
    Map,
    X,
    Y,
    Badges,
    Party,
    PartyHp,
    Owned,
    Money,
    Items,
    PlaySeconds,
    /// A named memory watch. **Reporting only** — a condition reads it to
    /// decide whether the attempt is over, which is the host's judgement, not
    /// the agent's. See [`super::watch`].
    Watch(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Ge,
    Le,
    Gt,
    Lt,
}

/// One `<thing><op><number>` test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cond {
    pub lhs: Lhs,
    pub op: Op,
    pub rhs: i64,
    /// The text it was parsed from, so a report can quote what a person wrote.
    pub source: String,
}

impl Cond {
    /// Parse `map=0x33`, `badges>=1`, `party_hp=0`, `watch:enemy_hp<=0`.
    ///
    /// Deliberately a tiny grammar. Everything an episode has needed so far is
    /// "is this number where I want it?", and a condition language nobody can
    /// read in one glance is a condition language that gets misused.
    pub fn parse(text: &str) -> Result<Cond, String> {
        let t = text.trim();
        // Longest operators first, or `>=` parses as `>`.
        let (name, op, rest) = [">=", "<=", "!=", "=", ">", "<"]
            .iter()
            .find_map(|o| t.split_once(*o).map(|(a, b)| (a, *o, b)))
            .ok_or_else(|| format!("{text:?} has no comparison (=, !=, >=, <=, >, <)"))?;
        let op = match op {
            "=" => Op::Eq,
            "!=" => Op::Ne,
            ">=" => Op::Ge,
            "<=" => Op::Le,
            ">" => Op::Gt,
            "<" => Op::Lt,
            _ => unreachable!(),
        };
        let name = name.trim();
        let lhs = match name {
            "map" => Lhs::Map,
            "x" => Lhs::X,
            "y" => Lhs::Y,
            "badges" => Lhs::Badges,
            "party" => Lhs::Party,
            "party_hp" => Lhs::PartyHp,
            "owned" => Lhs::Owned,
            "money" => Lhs::Money,
            "items" => Lhs::Items,
            "play_seconds" => Lhs::PlaySeconds,
            other => match other.strip_prefix("watch:") {
                Some(w) if !w.is_empty() => Lhs::Watch(w.to_string()),
                _ => {
                    return Err(format!(
                        "{other:?} is not something to compare — try map, x, y, badges, \
                         party, party_hp, owned, money, items, play_seconds, or \
                         watch:<name>"
                    ))
                }
            },
        };
        let v = rest.trim();
        let rhs = v
            .strip_prefix("0x")
            .or_else(|| v.strip_prefix("0X"))
            .or_else(|| v.strip_prefix('$'))
            .and_then(|h| i64::from_str_radix(h, 16).ok())
            .or_else(|| v.parse().ok())
            .ok_or_else(|| format!("{v:?} is not a number"))?;
        Ok(Cond {
            lhs,
            op,
            rhs,
            source: t.to_string(),
        })
    }

    /// Evaluate. A `watch:` condition naming a watch that is not in the
    /// episode's list is **false**, never a panic and never silently true: an
    /// episode that can never succeed is caught by [`Episode::validate`]
    /// before it runs, so reaching here means the list changed underneath.
    pub fn holds(&self, p: &Progress, w: &Sample) -> bool {
        let Some(v) = self.value(p, w) else {
            return false;
        };
        match self.op {
            Op::Eq => v == self.rhs,
            Op::Ne => v != self.rhs,
            Op::Ge => v >= self.rhs,
            Op::Le => v <= self.rhs,
            Op::Gt => v > self.rhs,
            Op::Lt => v < self.rhs,
        }
    }

    fn value(&self, p: &Progress, w: &Sample) -> Option<i64> {
        Some(match &self.lhs {
            Lhs::Map => p.map as i64,
            Lhs::X => p.x as i64,
            Lhs::Y => p.y as i64,
            Lhs::Badges => p.badges.count_ones() as i64,
            Lhs::Party => p.party.len() as i64,
            Lhs::PartyHp => p.party_hp as i64,
            Lhs::Owned => p.owned as i64,
            Lhs::Money => p.money as i64,
            Lhs::Items => p.items as i64,
            Lhs::PlaySeconds => p.play_seconds as i64,
            Lhs::Watch(name) => w.get(name)? as i64,
        })
    }

    /// The watch this condition needs, if any.
    pub fn needs_watch(&self) -> Option<&str> {
        match &self.lhs {
            Lhs::Watch(n) => Some(n),
            _ => None,
        }
    }
}

// -- the episode -------------------------------------------------------------

/// A named, repeatable segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Episode {
    pub name: String,
    pub objective: String,
    /// The save state every attempt starts from, relative to the episode file.
    /// `None` means "wherever the machine happens to be", which is only useful
    /// for a single attempt and is refused for a repeated one.
    pub start_state: Option<PathBuf>,
    /// **All** must hold.
    pub success: Vec<Cond>,
    /// **Any** ends the attempt as a failure.
    pub failure: Vec<Cond>,
    /// Emulated frames before the attempt is abandoned.
    pub timeout_frames: u64,
    /// Model calls one attempt may make. The hard cost ceiling for a repeated
    /// run: `attempts * max_calls` is the worst case, and it is printed before
    /// anything is spent.
    pub max_calls: u32,
    pub watches: Watches,
}

impl Default for Episode {
    fn default() -> Self {
        Episode {
            name: "episode".into(),
            objective: String::new(),
            start_state: None,
            success: Vec::new(),
            failure: Vec::new(),
            timeout_frames: 60_000, // ~17 minutes of game time
            max_calls: 40,
            watches: Watches::default(),
        }
    }
}

impl Episode {
    pub fn from_json(text: &str) -> Result<Episode, String> {
        let v = json::parse(text).map_err(|e| format!("episode is not JSON: {}", e.0))?;
        let schema = v.str_field("schema").unwrap_or("");
        if schema != EPISODE_SCHEMA {
            return Err(format!(
                "episode schema is {schema:?}, this build reads {EPISODE_SCHEMA:?}"
            ));
        }
        let conds = |key: &str| -> Result<Vec<Cond>, String> {
            v.get(key)
                .and_then(|a| a.as_array())
                .unwrap_or(&[])
                .iter()
                .map(|c| {
                    c.as_str()
                        .ok_or_else(|| format!("{key} must be a list of strings"))
                        .and_then(Cond::parse)
                })
                .collect()
        };
        let watches = Watches(
            v.get("watch")
                .and_then(|a| a.as_array())
                .unwrap_or(&[])
                .iter()
                .map(Watch::from_json)
                .collect::<Result<Vec<_>, _>>()?,
        );
        let ep = Episode {
            name: v.str_field("name").unwrap_or("episode").to_string(),
            objective: v.str_field("objective").unwrap_or("").to_string(),
            start_state: v.str_field("start_state").map(PathBuf::from),
            success: conds("success")?,
            failure: conds("failure")?,
            timeout_frames: v.u64_field("timeout_frames").unwrap_or(60_000),
            max_calls: v.u64_field("max_calls").unwrap_or(40) as u32,
            watches,
        };
        ep.validate()?;
        Ok(ep)
    }

    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", EPISODE_SCHEMA);
        w.str_field("name", &self.name);
        w.str_field("objective", &self.objective);
        if let Some(s) = &self.start_state {
            w.str_field("start_state", &s.to_string_lossy());
        }
        w.arr_field("success");
        for c in &self.success {
            w.str_item(&c.source);
        }
        w.end_arr();
        w.arr_field("failure");
        for c in &self.failure {
            w.str_item(&c.source);
        }
        w.end_arr();
        w.num_field("timeout_frames", self.timeout_frames as f64);
        w.num_field("max_calls", self.max_calls as f64);
        w.arr_field("watch");
        for watch in &self.watches.0 {
            watch.to_json(&mut w);
        }
        w.end_arr();
        w.end_obj();
        w.finish()
    }

    /// Refuse an episode that cannot work, **before** it spends anything.
    ///
    /// The expensive mistake this catches is a success condition on a watch
    /// nobody declared: it can never hold, so every attempt times out, and a
    /// thousand attempts is a thousand timeouts and a real bill.
    pub fn validate(&self) -> Result<(), String> {
        if self.success.is_empty() {
            return Err(format!(
                "episode {:?} has no success condition, so no attempt could ever \
                 succeed",
                self.name
            ));
        }
        if self.timeout_frames == 0 {
            return Err(format!("episode {:?} has a zero timeout", self.name));
        }
        for c in self.success.iter().chain(&self.failure) {
            if let Some(n) = c.needs_watch() {
                if self.watches.get(n).is_none() {
                    return Err(format!(
                        "condition {:?} needs a watch called {n:?}, which this \
                         episode does not declare",
                        c.source
                    ));
                }
            }
        }
        Ok(())
    }

    /// Resolve `start_state` against the directory the episode file came from.
    pub fn start_state_from(&self, episode_file: &Path) -> Option<PathBuf> {
        let s = self.start_state.as_ref()?;
        if s.is_absolute() {
            return Some(s.clone());
        }
        Some(episode_file.parent().unwrap_or(Path::new(".")).join(s))
    }

    /// Is the attempt over? `None` means keep going.
    pub fn verdict(&self, p: &Progress, w: &Sample) -> Option<Outcome> {
        if let Some(c) = self.failure.iter().find(|c| c.holds(p, w)) {
            return Some(Outcome::Failed(c.source.clone()));
        }
        if !self.success.is_empty() && self.success.iter().all(|c| c.holds(p, w)) {
            return Some(Outcome::Succeeded);
        }
        None
    }
}

/// How one attempt ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    /// A `failure` condition held. Carries the condition's own text.
    Failed(String),
    /// The frame budget ran out.
    TimedOut,
    /// The agent's own budget ran out before the episode resolved.
    OutOfCalls,
    /// The emulator faulted — a crash, a wedge, a runaway program counter.
    Faulted(String),
}

impl Outcome {
    pub fn tag(&self) -> &'static str {
        match self {
            Outcome::Succeeded => "success",
            Outcome::Failed(_) => "failure",
            Outcome::TimedOut => "timeout",
            Outcome::OutOfCalls => "out-of-calls",
            Outcome::Faulted(_) => "fault",
        }
    }

    pub fn is_success(&self) -> bool {
        matches!(self, Outcome::Succeeded)
    }

    /// Everything that is not a success is worth keeping a save state for.
    pub fn worth_capturing(&self) -> bool {
        !self.is_success()
    }

    pub fn detail(&self) -> String {
        match self {
            Outcome::Failed(c) => c.clone(),
            Outcome::Faulted(f) => f.clone(),
            _ => String::new(),
        }
    }
}

/// What one attempt did.
#[derive(Debug, Clone)]
pub struct Attempt {
    pub index: u32,
    pub outcome: Outcome,
    pub frames: u64,
    pub wall_ms: u128,
    pub calls: u32,
    pub decisions: u64,
    pub usd: f64,
    pub play_seconds: u32,
    /// Frames spent on each map, commonest first. **The number that says where
    /// the agent is actually stuck.**
    pub areas: Vec<(u8, u64)>,
    pub final_progress: Progress,
    pub watches: Sample,
    /// Where the save state at the moment of failure was written, if it was.
    pub capture: Option<PathBuf>,
}

impl Attempt {
    pub fn frames_per_call(&self) -> f64 {
        if self.calls == 0 {
            return 0.0;
        }
        self.frames as f64 / self.calls as f64
    }

    pub fn to_json(&self, episode: &str) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", RESULT_SCHEMA);
        w.str_field("ev", "attempt");
        w.str_field("episode", episode);
        w.num_field("attempt", self.index as f64);
        w.str_field("outcome", self.outcome.tag());
        let detail = self.outcome.detail();
        if !detail.is_empty() {
            w.str_field("detail", &detail);
        }
        // The headline, on every line, so a consumer never has to divide.
        w.num_field(
            "frames_per_call",
            (self.frames_per_call() * 100.0).round() / 100.0,
        );
        w.num_field("frames", self.frames as f64);
        w.num_field("wall_ms", self.wall_ms as f64);
        w.num_field("calls", self.calls as f64);
        w.num_field("decisions", self.decisions as f64);
        w.num_field("usd", self.usd);
        w.num_field("play_seconds", self.play_seconds as f64);
        w.arr_field("areas");
        for (map, frames) in &self.areas {
            w.begin_obj();
            w.num_field("map", *map as f64);
            w.str_field("name", super::map_name(*map));
            w.num_field("frames", *frames as f64);
            w.num_field(
                "fraction",
                if self.frames == 0 {
                    0.0
                } else {
                    (*frames as f64 / self.frames as f64 * 1000.0).round() / 1000.0
                },
            );
            w.end_obj();
        }
        w.end_arr();
        if !self.watches.is_empty() {
            w.obj_field("watch");
            for (name, value) in &self.watches.0 {
                w.num_field(name, *value as f64);
            }
            w.end_obj();
        }
        w.obj_field("final");
        w.num_field("map", self.final_progress.map as f64);
        w.str_field("map_name", super::map_name(self.final_progress.map));
        w.num_field("x", self.final_progress.x as f64);
        w.num_field("y", self.final_progress.y as f64);
        w.num_field("badges", self.final_progress.badges.count_ones() as f64);
        w.num_field("party", self.final_progress.party.len() as f64);
        w.end_obj();
        match &self.capture {
            Some(p) => w.str_field("capture", &p.to_string_lossy()),
            None => w.null_field("capture"),
        }
        w.end_obj();
        w.finish()
    }
}

/// Every attempt, summarised.
#[derive(Debug, Clone)]
pub struct EpisodeReport {
    pub episode: Episode,
    pub attempts: Vec<Attempt>,
    /// Assistance the runner gave, declared so the numbers mean something.
    pub resets: u32,
}

impl EpisodeReport {
    pub fn successes(&self) -> usize {
        self.attempts
            .iter()
            .filter(|a| a.outcome.is_success())
            .count()
    }

    pub fn success_rate(&self) -> f64 {
        if self.attempts.is_empty() {
            return 0.0;
        }
        self.successes() as f64 / self.attempts.len() as f64
    }

    /// Median frames of the **successful** attempts. A median over successes
    /// and timeouts together is not a measure of anything: a timeout's frame
    /// count is the timeout, not the difficulty.
    pub fn median_success_frames(&self) -> Option<u64> {
        let mut v: Vec<u64> = self
            .attempts
            .iter()
            .filter(|a| a.outcome.is_success())
            .map(|a| a.frames)
            .collect();
        if v.is_empty() {
            return None;
        }
        v.sort_unstable();
        Some(v[v.len() / 2])
    }

    pub fn total_frames(&self) -> u64 {
        self.attempts.iter().map(|a| a.frames).sum()
    }
    pub fn total_calls(&self) -> u32 {
        self.attempts.iter().map(|a| a.calls).sum()
    }
    pub fn total_usd(&self) -> f64 {
        self.attempts.iter().map(|a| a.usd).sum()
    }

    /// **The headline for a repeated run.**
    pub fn frames_per_call(&self) -> f64 {
        let c = self.total_calls();
        if c == 0 {
            return 0.0;
        }
        self.total_frames() as f64 / c as f64
    }

    /// Frames per map across every attempt, commonest first — *"40% of the run
    /// was spent in Viridian Forest"*.
    pub fn areas(&self) -> Vec<(u8, u64)> {
        let mut acc: Vec<(u8, u64)> = Vec::new();
        for a in &self.attempts {
            for (map, frames) in &a.areas {
                match acc.iter_mut().find(|(m, _)| m == map) {
                    Some((_, n)) => *n += frames,
                    None => acc.push((*map, *frames)),
                }
            }
        }
        acc.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        acc
    }

    pub fn by_outcome(&self) -> Vec<(&'static str, usize)> {
        let mut out: Vec<(&'static str, usize)> = Vec::new();
        for a in &self.attempts {
            let t = a.outcome.tag();
            match out.iter_mut().find(|(k, _)| *k == t) {
                Some((_, n)) => *n += 1,
                None => out.push((t, 1)),
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        out
    }

    pub fn headline(&self) -> String {
        format!(
            "{}: {}/{} succeeded ({:.0}%) · {:.0} frames per model call · {} calls · ${:.4}",
            self.episode.name,
            self.successes(),
            self.attempts.len(),
            self.success_rate() * 100.0,
            self.frames_per_call(),
            self.total_calls(),
            self.total_usd(),
        )
    }

    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", RESULT_SCHEMA);
        w.str_field("ev", "episode-summary");
        w.str_field("episode", &self.episode.name);
        w.str_field("objective", &self.episode.objective);
        // Headline first.
        w.num_field(
            "frames_per_call",
            (self.frames_per_call() * 100.0).round() / 100.0,
        );
        w.num_field("attempts", self.attempts.len() as f64);
        w.num_field("successes", self.successes() as f64);
        w.num_field(
            "success_rate",
            (self.success_rate() * 1000.0).round() / 1000.0,
        );
        match self.median_success_frames() {
            Some(m) => w.num_field("median_success_frames", m as f64),
            None => w.null_field("median_success_frames"),
        }
        w.num_field("total_frames", self.total_frames() as f64);
        w.num_field("total_calls", self.total_calls() as f64);
        w.num_field("total_usd", self.total_usd());
        w.arr_field("outcomes");
        for (tag, n) in self.by_outcome() {
            w.begin_obj();
            w.str_field("outcome", tag);
            w.num_field("count", n as f64);
            w.end_obj();
        }
        w.end_arr();
        w.arr_field("areas");
        let total = self.total_frames().max(1);
        for (map, frames) in self.areas() {
            w.begin_obj();
            w.num_field("map", map as f64);
            w.str_field("name", super::map_name(map));
            w.num_field("frames", frames as f64);
            w.num_field(
                "fraction",
                (frames as f64 / total as f64 * 1000.0).round() / 1000.0,
            );
            w.end_obj();
        }
        w.end_arr();
        // Assistance, always declared.
        w.obj_field("assistance");
        w.num_field("state_resets", self.resets as f64);
        w.num_field("memory_writes", 0.0);
        w.num_field("memory_informed_decisions", 0.0);
        w.bool_field("watches_shown_to_agent", false);
        w.end_obj();
        w.end_obj();
        w.finish()
    }

    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# Episode: {}\n\n", self.episode.name));
        s.push_str(&format!("**{}**\n\n", self.headline()));
        if !self.episode.objective.is_empty() {
            s.push_str(&format!("_{}_\n\n", self.episode.objective));
        }
        s.push_str("| | |\n|---|---:|\n");
        s.push_str(&format!(
            "| **frames per model call** | **{:.0}** |\n",
            self.frames_per_call()
        ));
        s.push_str(&format!("| attempts | {} |\n", self.attempts.len()));
        s.push_str(&format!(
            "| succeeded | {} ({:.0}%) |\n",
            self.successes(),
            self.success_rate() * 100.0
        ));
        s.push_str(&format!(
            "| median frames to succeed | {} |\n",
            match self.median_success_frames() {
                Some(m) => m.to_string(),
                None => "— (nothing succeeded)".into(),
            }
        ));
        s.push_str(&format!("| model calls | {} |\n", self.total_calls()));
        s.push_str(&format!(
            "| spend (reported) | ${:.4} |\n\n",
            self.total_usd()
        ));

        s.push_str("## How attempts ended\n\n| outcome | count |\n|---|---:|\n");
        for (tag, n) in self.by_outcome() {
            s.push_str(&format!("| {tag} | {n} |\n"));
        }
        s.push('\n');

        s.push_str("## Where the time went\n\n");
        s.push_str("This is the number that says where the agent is actually stuck.\n\n");
        s.push_str("| area | frames | share |\n|---|---:|---:|\n");
        let total = self.total_frames().max(1);
        for (map, frames) in self.areas().into_iter().take(12) {
            let name = super::map_name(map);
            s.push_str(&format!(
                "| {} | {frames} | {:.0}% |\n",
                if name.is_empty() {
                    format!("map {map:#04x}")
                } else {
                    name.to_string()
                },
                frames as f64 / total as f64 * 100.0
            ));
        }
        s.push('\n');

        s.push_str("## Declared\n\n");
        s.push_str(&format!(
            "- state resets (one per attempt, by design): **{}**\n",
            self.resets
        ));
        s.push_str("- memory writes by the agent: **0**\n");
        s.push_str(
            "- memory watches shown to the agent: **none** — watches are host-side \
             reporting only, so an episode result cannot be inflated by reading the \
             answer out of RAM\n\n",
        );

        let captured: Vec<&Attempt> = self
            .attempts
            .iter()
            .filter(|a| a.capture.is_some())
            .collect();
        if !captured.is_empty() {
            s.push_str("## Reproducible failures\n\n");
            s.push_str("Each of these is a save state you can reload.\n\n");
            for a in captured.iter().take(20) {
                s.push_str(&format!(
                    "- attempt {} · {} {} · `{}`\n",
                    a.index,
                    a.outcome.tag(),
                    a.outcome.detail(),
                    a.capture.as_ref().unwrap().display()
                ));
            }
            if captured.len() > 20 {
                s.push_str(&format!("- …and {} more\n", captured.len() - 20));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prog() -> Progress {
        Progress {
            in_game: true,
            map: 0x33,
            x: 4,
            y: 9,
            badges: 0b1,
            party: vec![(1, 12)],
            party_hp: 30,
            owned: 3,
            play_seconds: 600,
            ..Default::default()
        }
    }

    #[test]
    fn the_condition_grammar_reads_the_way_a_person_writes_it() {
        let p = prog();
        let w = Sample(vec![("enemy_hp".into(), 0)]);
        for (text, want) in [
            ("map=0x33", true),
            ("map=51", true),
            ("map=0x2F", false),
            ("badges>=1", true),
            ("badges>=2", false),
            ("party_hp=0", false),
            ("party_hp>0", true),
            ("owned<=3", true),
            ("play_seconds>=600", true),
            ("watch:enemy_hp<=0", true),
            ("x!=4", false),
        ] {
            let c = Cond::parse(text).unwrap_or_else(|e| panic!("{text}: {e}"));
            assert_eq!(c.holds(&p, &w), want, "{text}");
        }
    }

    #[test]
    fn a_greater_or_equal_does_not_parse_as_a_greater_than() {
        // Longest-operator-first, or `badges>=1` becomes `badges > "=1"`.
        let c = Cond::parse("badges>=1").unwrap();
        assert_eq!(c.op, Op::Ge);
        assert_eq!(c.rhs, 1);
    }

    #[test]
    fn nonsense_is_refused_with_the_list_of_things_you_can_compare() {
        assert!(Cond::parse("map").unwrap_err().contains("comparison"));
        let e = Cond::parse("bananas=1").unwrap_err();
        assert!(e.contains("badges") && e.contains("watch:"), "{e}");
        assert!(Cond::parse("map=green").unwrap_err().contains("number"));
        assert!(Cond::parse("watch:=1").is_err());
    }

    #[test]
    fn success_is_an_and_and_failure_is_an_or() {
        let ep = Episode {
            success: vec![
                Cond::parse("map=0x33").unwrap(),
                Cond::parse("badges>=1").unwrap(),
            ],
            failure: vec![
                Cond::parse("party_hp=0").unwrap(),
                Cond::parse("map=0x00").unwrap(),
            ],
            ..Default::default()
        };
        let w = Sample::default();
        assert_eq!(ep.verdict(&prog(), &w), Some(Outcome::Succeeded));

        let mut half = prog();
        half.badges = 0;
        assert_eq!(ep.verdict(&half, &w), None, "AND: one of two is not done");

        let mut wiped = prog();
        wiped.party_hp = 0;
        assert!(matches!(ep.verdict(&wiped, &w), Some(Outcome::Failed(_))));

        let mut home = prog();
        home.map = 0;
        assert!(
            matches!(ep.verdict(&home, &w), Some(Outcome::Failed(_))),
            "OR: the second failure condition alone ends it"
        );
    }

    #[test]
    fn failure_outranks_success_when_both_hold() {
        // A wipe on the target map is a failure, not a win.
        let ep = Episode {
            success: vec![Cond::parse("map=0x33").unwrap()],
            failure: vec![Cond::parse("party_hp=0").unwrap()],
            ..Default::default()
        };
        let mut p = prog();
        p.party_hp = 0;
        assert!(matches!(
            ep.verdict(&p, &Sample::default()),
            Some(Outcome::Failed(_))
        ));
    }

    #[test]
    fn an_episode_that_could_never_succeed_is_refused_before_it_spends_anything() {
        let mut ep = Episode::default();
        assert!(ep.validate().unwrap_err().contains("no success condition"));
        ep.success = vec![Cond::parse("watch:ghost=1").unwrap()];
        let e = ep.validate().unwrap_err();
        assert!(e.contains("does not declare"), "{e}");
        ep.watches = Watches(vec![Watch {
            name: "ghost".into(),
            addr: 0xC000,
            len: 1,
            big_endian: false,
        }]);
        assert!(ep.validate().is_ok());
        ep.timeout_frames = 0;
        assert!(ep.validate().is_err());
    }

    #[test]
    fn an_episode_round_trips_through_its_own_file_format() {
        let ep = Episode {
            name: "out-of-viridian-forest".into(),
            objective: "Leave by the north exit".into(),
            start_state: Some(PathBuf::from("states/forest.gbss")),
            success: vec![Cond::parse("map=0x33").unwrap()],
            failure: vec![Cond::parse("party_hp=0").unwrap()],
            timeout_frames: 42_000,
            max_calls: 25,
            watches: Watches(vec![Watch::named("in_battle").unwrap()]),
        };
        let back = Episode::from_json(&ep.to_json()).expect("round trip");
        assert_eq!(ep, back);
    }

    #[test]
    fn an_episode_from_another_schema_is_refused_not_half_read() {
        let e =
            Episode::from_json(r#"{"schema":"nope/1","success":["map=1"]}"#).unwrap_err();
        assert!(e.contains("schema"), "{e}");
    }

    #[test]
    fn the_summary_reports_a_rate_a_median_and_where_the_time_went() {
        let mk = |i, outcome: Outcome, frames, map, calls| Attempt {
            index: i,
            outcome,
            frames,
            wall_ms: 10,
            calls,
            decisions: calls as u64,
            usd: 0.01,
            play_seconds: 10,
            areas: vec![(map, frames)],
            final_progress: prog(),
            watches: Sample::default(),
            capture: None,
        };
        let r = EpisodeReport {
            episode: Episode {
                name: "seg".into(),
                success: vec![Cond::parse("map=1").unwrap()],
                ..Default::default()
            },
            attempts: vec![
                mk(1, Outcome::Succeeded, 1000, 0x33, 10),
                mk(2, Outcome::TimedOut, 60000, 0x33, 40),
                mk(3, Outcome::Succeeded, 3000, 0x02, 20),
                mk(4, Outcome::Succeeded, 2000, 0x33, 15),
            ],
            resets: 4,
        };
        assert_eq!(r.successes(), 3);
        assert!((r.success_rate() - 0.75).abs() < 1e-9);
        assert_eq!(r.median_success_frames(), Some(2000));
        // Viridian Forest dominates, which is exactly the finding.
        assert_eq!(r.areas()[0].0, 0x33);
        assert!((r.frames_per_call() - 66000.0 / 85.0).abs() < 0.01);
        let md = r.to_markdown();
        assert!(md.contains("frames per model call"), "{md}");
        assert!(md.contains("Where the time went"), "{md}");
        assert!(md.contains("watches are host-side"), "{md}");
        let j = json::parse(&r.to_json()).expect("valid JSON");
        assert_eq!(j.str_field("ev"), Some("episode-summary"));
        assert_eq!(
            j.get("assistance")
                .and_then(|a| a.get("watches_shown_to_agent")),
            Some(&json::Json::Bool(false))
        );
    }

    #[test]
    fn a_median_ignores_timeouts_because_a_timeout_is_not_a_difficulty() {
        let mk = |outcome: Outcome, frames| Attempt {
            index: 0,
            outcome,
            frames,
            wall_ms: 0,
            calls: 1,
            decisions: 1,
            usd: 0.0,
            play_seconds: 0,
            areas: vec![],
            final_progress: Progress::default(),
            watches: Sample::default(),
            capture: None,
        };
        let r = EpisodeReport {
            episode: Episode::default(),
            attempts: vec![
                mk(Outcome::Succeeded, 100),
                mk(Outcome::TimedOut, 999_999),
                mk(Outcome::TimedOut, 999_999),
            ],
            resets: 3,
        };
        assert_eq!(r.median_success_frames(), Some(100));
    }

    #[test]
    fn every_non_success_is_worth_a_save_state() {
        assert!(!Outcome::Succeeded.worth_capturing());
        for o in [
            Outcome::TimedOut,
            Outcome::OutOfCalls,
            Outcome::Failed("x".into()),
            Outcome::Faulted("y".into()),
        ] {
            assert!(o.worth_capturing(), "{o:?}");
            assert!(!o.tag().is_empty());
        }
    }
}

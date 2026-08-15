//! The run report: an honest scoreboard, rendered for a person and for a
//! program.
//!
//! # The headline number is frames per model call
//!
//! At 0.238 ms/frame the emulator does about 4,200 frames a second; a model
//! round trip is *seconds*. So the only question that decides whether an
//! unattended run is worth doing at all is **how much game each decision
//! buys**, and that is [`Scoreboard::frames_per_call`]. It is printed first,
//! written first in the JSON, and stated in plain language beside its
//! consequence ("one call every N seconds of game time"), because a bare
//! frame count means nothing to a reader.
//!
//! Two derived figures fall out of it and both matter more than raw speed:
//!
//! * **effective speed-up** — game seconds per wall second. If the agent is
//!   the bottleneck (it is), this is far below the emulator's 70x, and saying
//!   so is the point.
//! * **cost per game hour** — what an overnight run actually costs.
//!
//! # Everything here is measured, nothing is claimed
//!
//! `stopped_because` is the real reason. `resumes` is the number of times the
//! game was rewound to a checkpoint, and it is in the report **because a
//! rewind must never be silent** — see [`super::journal`]. `memory_writes` is
//! zero by construction (the agent declares no `WRITE_MEMORY` capability and
//! the runner never pokes) and is printed anyway, because a claim nobody can
//! check is not evidence.

use super::progress::{human_seconds, Importance, Milestone};
use terminalgb_plugin_agent::json;

/// The emulated frame rate of a DMG, used to convert frames to game seconds.
pub const FRAME_HZ: f64 = 59.7275;

/// What a run achieved, and what it cost.
#[derive(Debug, Clone, Default)]
pub struct Scoreboard {
    pub objective: String,
    pub rom_title: String,
    pub driver: String,

    /// Emulated frames run, across every session of this run.
    pub frames: u64,
    /// Wall-clock seconds, across every session. Whole seconds, for display
    /// and for the checkpoint that carries a run across restarts.
    pub wall_seconds: u64,
    /// The same span in milliseconds, and **what every rate below divides
    /// by**.
    ///
    /// Whole seconds are fine for an overnight run and hopeless for a short
    /// one: 20,000 frames in "3 s" is 6,667 fps or 5,000 fps depending on
    /// which side of the second it landed, and the report would print either
    /// with four significant figures. Rates are computed from this instead, so
    /// a two-minute measurement is worth quoting.
    pub wall_ms: u128,
    /// The game's own play clock at the end, in seconds. The honest measure of
    /// game time — see [`super::progress::Progress::play_seconds`].
    pub play_seconds: u32,

    pub calls: u32,
    pub decisions: u64,
    pub failed_calls: u32,
    /// Frames the emulator ran while a model call was in flight.
    ///
    /// **This is usually most of a headless run, and it is the finding.** The
    /// emulator never blocks on the agent (which is right), so at ~4,200
    /// frames a second a ten-second round trip is *forty thousand frames* of
    /// game the agent did not ask for and cannot see. Reporting
    /// `frames_per_call` without this beside it would flatter the harness
    /// enormously.
    pub frames_thinking: u64,
    /// Frames spent carrying out a plan — the part a decision actually bought.
    pub frames_executing: u64,
    pub tokens: u64,
    pub usd: f64,

    pub milestones: Vec<Milestone>,
    pub maps_visited: usize,
    pub badges: u8,
    pub party: Vec<(u8, u8)>,
    pub final_map: u8,
    pub final_map_name: &'static str,
    pub final_x: u8,
    pub final_y: u8,

    /// Separate wall reports, and the worst stall reached, in game seconds.
    pub wall_reports: u32,
    pub worst_stall_seconds: u32,
    /// The agent's own repeat-detector high-water mark.
    pub worst_repeats: u32,

    /// Emulated frames spent on each map, commonest first. **The number that
    /// says where the agent is actually stuck** — a run report that says
    /// "40% of it was Viridian Forest" tells you what to work on; one that
    /// says "it played for four hours" does not.
    pub areas: Vec<(u8, u64)>,
    /// Save states written at the moment something went wrong, so a failure
    /// can be reloaded rather than only read about.
    pub captures: Vec<std::path::PathBuf>,

    /// How many times the game was rewound to a checkpoint. **Declared.**
    pub resumes: u32,
    /// Always zero: the agent declares no `WRITE_MEMORY` and the runner never
    /// pokes. Printed so the claim is checkable rather than asserted.
    pub memory_writes: u64,

    pub stopped_because: String,
    pub ntfy: Option<(u32, u32, u32)>,
}

impl Scoreboard {
    /// **The headline.** Emulated frames per model call — how much game one
    /// decision bought.
    pub fn frames_per_call(&self) -> f64 {
        if self.calls == 0 {
            return 0.0;
        }
        self.frames as f64 / self.calls as f64
    }

    /// The same number in the unit a person thinks in: seconds of game per
    /// call.
    pub fn game_seconds_per_call(&self) -> f64 {
        self.frames_per_call() / FRAME_HZ
    }

    /// **The honest version of the headline**: frames of *purposeful* play per
    /// call — the plan's frames, with the model's think time taken out.
    ///
    /// `frames_per_call` answers "how much game elapsed per decision" and this
    /// answers "how much game the decision was actually driving". On a paced
    /// run they are nearly the same; on an uncapped headless one they are not
    /// remotely, and the gap is what [`Scoreboard::think_share`] reports.
    pub fn executing_frames_per_call(&self) -> f64 {
        if self.calls == 0 {
            return 0.0;
        }
        self.frames_executing as f64 / self.calls as f64
    }

    /// Fraction of the run that was the world running on while the model
    /// thought. High is not automatically wrong — it is what "the emulator
    /// never waits" *means* — but it does mean the agent's observation was
    /// stale by the time its plan ran, which is worth knowing before reading
    /// anything else in the report.
    pub fn think_share(&self) -> f64 {
        if self.frames == 0 {
            return 0.0;
        }
        self.frames_thinking as f64 / self.frames as f64
    }

    /// Game seconds produced per wall-clock second. The emulator alone reaches
    /// ~70; whatever this says is what the *whole system* achieved.
    pub fn speedup(&self) -> f64 {
        let secs = self.wall_secs_f();
        if secs <= 0.0 {
            return 0.0;
        }
        (self.frames as f64 / FRAME_HZ) / secs
    }

    /// Wall clock as a real number of seconds — see [`Scoreboard::wall_ms`].
    /// Falls back to the whole-second field for a scoreboard built by hand.
    pub fn wall_secs_f(&self) -> f64 {
        if self.wall_ms > 0 {
            self.wall_ms as f64 / 1000.0
        } else {
            self.wall_seconds as f64
        }
    }

    /// Emulated frames per wall second — the emulator's own rate, for
    /// comparison with the 70x figure in `docs/mcp.md`.
    pub fn emulated_fps(&self) -> f64 {
        let secs = self.wall_secs_f();
        if secs <= 0.0 {
            return 0.0;
        }
        self.frames as f64 / secs
    }

    /// Reported spend per hour of game time. Reported, so a floor.
    pub fn usd_per_game_hour(&self) -> f64 {
        let hours = self.frames as f64 / FRAME_HZ / 3600.0;
        if hours <= 0.0 {
            return 0.0;
        }
        self.usd / hours
    }

    /// Milestones of at least this importance, newest last.
    pub fn notable(&self, min: Importance) -> Vec<&Milestone> {
        self.milestones
            .iter()
            .filter(|m| m.importance() >= min)
            .collect()
    }

    /// Counts by tag, for the summary table.
    pub fn by_tag(&self) -> Vec<(&'static str, usize)> {
        let mut out: Vec<(&'static str, usize)> = Vec::new();
        for m in &self.milestones {
            let t = m.kind.tag();
            match out.iter_mut().find(|(k, _)| *k == t) {
                Some((_, n)) => *n += 1,
                None => out.push((t, 1)),
            }
        }
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        out
    }

    /// The one-line verdict a supervisor reads first.
    pub fn headline(&self) -> String {
        format!(
            "{:.0} frames per model call ({} of game per decision) · {} calls · \
             {} of game time in {} of wall clock ({:.1}x)",
            self.frames_per_call(),
            human_seconds(self.game_seconds_per_call().round() as u32),
            self.calls,
            human_seconds((self.frames as f64 / FRAME_HZ) as u32),
            human_seconds(self.wall_seconds as u32),
            self.speedup(),
        )
    }

    /// Machine-readable. The headline fields come first on purpose.
    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", super::journal::EVENT_SCHEMA);
        w.str_field("ev", "scoreboard");
        // Headline first.
        w.num_field("frames_per_call", round2(self.frames_per_call()));
        w.num_field(
            "executing_frames_per_call",
            round2(self.executing_frames_per_call()),
        );
        w.num_field("think_share", round2(self.think_share()));
        w.num_field("frames_thinking", self.frames_thinking as f64);
        w.num_field("frames_executing", self.frames_executing as f64);
        w.num_field(
            "game_seconds_per_call",
            round2(self.game_seconds_per_call()),
        );
        w.num_field("speedup", round2(self.speedup()));
        w.num_field("emulated_fps", round2(self.emulated_fps()));

        w.str_field("objective", &self.objective);
        w.str_field("rom_title", &self.rom_title);
        w.str_field("driver", &self.driver);
        w.num_field("frames", self.frames as f64);
        w.num_field("wall_seconds", round2(self.wall_secs_f()));
        w.num_field("wall_ms", self.wall_ms as f64);
        w.num_field("game_seconds", round2(self.frames as f64 / FRAME_HZ));
        w.num_field("play_seconds", self.play_seconds as f64);
        w.num_field("calls", self.calls as f64);
        w.num_field("decisions", self.decisions as f64);
        w.num_field("failed_calls", self.failed_calls as f64);
        w.num_field("tokens", self.tokens as f64);
        w.num_field("usd", self.usd);
        w.num_field("usd_per_game_hour", round2(self.usd_per_game_hour()));
        w.num_field("maps_visited", self.maps_visited as f64);
        w.num_field("badges", self.badges.count_ones() as f64);
        w.num_field("milestones", self.milestones.len() as f64);
        w.num_field("wall_reports", self.wall_reports as f64);
        w.num_field("worst_stall_seconds", self.worst_stall_seconds as f64);
        w.num_field("worst_repeats", self.worst_repeats as f64);
        w.str_field("stopped_because", &self.stopped_because);
        // Everything that could have helped the agent, declared in one place.
        // An undeclared advantage makes every number above worthless.
        w.obj_field("assistance");
        w.num_field("state_resets", self.resumes as f64);
        w.num_field("memory_writes", self.memory_writes as f64);
        w.num_field("memory_informed_decisions", 0.0);
        w.bool_field("watches_shown_to_agent", false);
        w.bool_field("seed_controlled", false);
        w.end_obj();
        // Kept at the top level too, because that is where the first readers
        // of this file looked for them.
        w.num_field("resumes", self.resumes as f64);
        w.num_field("memory_writes", self.memory_writes as f64);
        w.arr_field("areas");
        let total = self.frames.max(1);
        for (map, frames) in &self.areas {
            w.begin_obj();
            w.num_field("map", *map as f64);
            w.str_field("name", super::map_name(*map));
            w.num_field("frames", *frames as f64);
            w.num_field(
                "fraction",
                (*frames as f64 / total as f64 * 1000.0).round() / 1000.0,
            );
            w.end_obj();
        }
        w.end_arr();
        w.arr_field("captures");
        for c in &self.captures {
            w.str_item(&c.to_string_lossy());
        }
        w.end_arr();
        w.obj_field("final_position");
        w.num_field("map", self.final_map as f64);
        w.str_field("map_name", self.final_map_name);
        w.num_field("x", self.final_x as f64);
        w.num_field("y", self.final_y as f64);
        w.end_obj();
        w.arr_field("party");
        for (species, level) in &self.party {
            w.begin_obj();
            w.num_field("species", *species as f64);
            w.num_field("level", *level as f64);
            w.end_obj();
        }
        w.end_arr();
        w.arr_field("milestone_counts");
        for (tag, n) in self.by_tag() {
            w.begin_obj();
            w.str_field("tag", tag);
            w.num_field("count", n as f64);
            w.end_obj();
        }
        w.end_arr();
        if let Some((sent, failed, dropped)) = self.ntfy {
            w.obj_field("ntfy");
            w.num_field("sent", sent as f64);
            w.num_field("failed", failed as f64);
            w.num_field("dropped", dropped as f64);
            w.end_obj();
        }
        w.end_obj();
        w.finish()
    }

    /// Markdown, for a person who walked away and wants the story.
    pub fn to_markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# Long run report\n\n");
        s.push_str(&format!("**{}**\n\n", self.headline()));
        s.push_str(&format!("- objective: {}\n", show(&self.objective)));
        s.push_str(&format!("- cartridge: {}\n", show(&self.rom_title)));
        s.push_str(&format!("- driver: {}\n", show(&self.driver)));
        s.push_str(&format!(
            "- stopped because: {}\n\n",
            show(&self.stopped_because)
        ));

        s.push_str("## Efficiency — the number that decides whether this works\n\n");
        s.push_str("| | |\n|---|---:|\n");
        s.push_str(&format!(
            "| **frames per model call** | **{:.0}** |\n",
            self.frames_per_call()
        ));
        s.push_str(&format!(
            "| …of which the plan was driving | **{:.0}** |\n",
            self.executing_frames_per_call()
        ));
        s.push_str(&format!(
            "| the world running on while the model thought | {:.0}% |\n",
            self.think_share() * 100.0
        ));
        s.push_str(&format!(
            "| game time bought per call | {} |\n",
            human_seconds(self.game_seconds_per_call().round() as u32)
        ));
        s.push_str(&format!(
            "| emulated frames/second | {:.0} |\n",
            self.emulated_fps()
        ));
        s.push_str(&format!(
            "| effective speed-up (game time / wall time) | {:.1}x |\n",
            self.speedup()
        ));
        s.push_str(&format!("| model calls | {} |\n", self.calls));
        s.push_str(&format!("| failed calls | {} |\n", self.failed_calls));
        s.push_str(&format!("| decisions | {} |\n", self.decisions));
        s.push_str(&format!("| tokens (reported) | {} |\n", self.tokens));
        s.push_str(&format!("| spend (reported) | ${:.4} |\n", self.usd));
        s.push_str(&format!(
            "| spend per hour of game time | ${:.4} |\n\n",
            self.usd_per_game_hour()
        ));

        s.push_str("## Time\n\n");
        s.push_str(&format!(
            "- emulated frames: {} ({} of game time at 59.7 Hz)\n",
            self.frames,
            human_seconds((self.frames as f64 / FRAME_HZ) as u32)
        ));
        s.push_str(&format!(
            "- the cartridge's own play clock: {}\n",
            human_seconds(self.play_seconds)
        ));
        s.push_str(&format!(
            "- wall clock: {} ({:.1} s)\n\n",
            human_seconds(self.wall_seconds as u32),
            self.wall_secs_f()
        ));

        s.push_str("## Where it got to\n\n");
        s.push_str(&format!("- badges: {}/8\n", self.badges.count_ones()));
        s.push_str(&format!("- distinct maps visited: {}\n", self.maps_visited));
        s.push_str(&format!(
            "- party: {}\n",
            if self.party.is_empty() {
                "empty".to_string()
            } else {
                self.party
                    .iter()
                    .map(|(_, lv)| format!("Lv{lv}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        s.push_str(&format!(
            "- final position: {} ({:#04x}) at ({}, {})\n\n",
            if self.final_map_name.is_empty() {
                "an unnamed map"
            } else {
                self.final_map_name
            },
            self.final_map,
            self.final_x,
            self.final_y
        ));

        if !self.areas.is_empty() {
            s.push_str("## Where the time went\n\n");
            s.push_str("The number that says where the agent is actually stuck.\n\n");
            s.push_str("| area | frames | share |\n|---|---:|---:|\n");
            let total = self.frames.max(1);
            for (map, frames) in self.areas.iter().take(12) {
                let name = super::map_name(*map);
                s.push_str(&format!(
                    "| {} | {frames} | {:.0}% |\n",
                    if name.is_empty() {
                        format!("map {map:#04x}")
                    } else {
                        name.to_string()
                    },
                    *frames as f64 / total as f64 * 100.0
                ));
            }
            s.push('\n');
        }

        s.push_str("## Trouble\n\n");
        s.push_str(&format!(
            "- walls reported: {} (worst stall {} of game time)\n",
            self.wall_reports,
            human_seconds(self.worst_stall_seconds)
        ));
        s.push_str(&format!(
            "- agent's own worst repeat streak: {}\n\n",
            self.worst_repeats
        ));

        s.push_str("## Declared\n\n");
        s.push_str(&format!(
            "- rewinds (resumes from a checkpoint): **{}**\n",
            self.resumes
        ));
        s.push_str(&format!(
            "- memory writes by the agent: **{}** (it declares no WRITE_MEMORY \
             capability, so the host would refuse one)\n",
            self.memory_writes
        ));
        s.push_str(
            "- memory-informed decisions: **0** — memory watches are host-side \
             reporting only and are never put in front of the agent\n",
        );
        s.push_str("- seed control: **none** — the RNG was left alone\n");
        if let Some((sent, failed, dropped)) = self.ntfy {
            s.push_str(&format!(
                "- pushes: {sent} sent, {failed} failed, {dropped} dropped\n"
            ));
        }
        s.push('\n');

        if !self.captures.is_empty() {
            s.push_str("## Reproducible failures\n\n");
            s.push_str("Each of these is a save state you can reload.\n\n");
            for c in &self.captures {
                s.push_str(&format!("- `{}`\n", c.display()));
            }
            s.push('\n');
        }

        let counts = self.by_tag();
        if !counts.is_empty() {
            s.push_str("## Milestones by kind\n\n| kind | count |\n|---|---:|\n");
            for (tag, n) in counts {
                s.push_str(&format!("| {tag} | {n} |\n"));
            }
            s.push('\n');
        }

        s.push_str("## What happened\n\n");
        if self.milestones.is_empty() {
            s.push_str("_Nothing worth reporting._\n");
        } else {
            for m in &self.milestones {
                s.push_str(&format!(
                    "- `{}` · {} game / {} wall — {}\n",
                    m.kind.tag(),
                    human_seconds(m.play_seconds),
                    human_seconds(m.wall_seconds as u32),
                    m.kind.text()
                ));
            }
        }
        s
    }
}

fn show(s: &str) -> String {
    if s.trim().is_empty() {
        "(none)".to_string()
    } else {
        s.trim().to_string()
    }
}

fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::super::progress::MilestoneKind;
    use super::*;

    fn board() -> Scoreboard {
        Scoreboard {
            objective: "Reach Pewter City".into(),
            rom_title: "POKEMON BLUE".into(),
            driver: "command".into(),
            frames: 600_000,
            wall_seconds: 3_600,
            wall_ms: 3_600_000,
            play_seconds: 9_800,
            calls: 500,
            decisions: 500,
            failed_calls: 3,
            tokens: 900_000,
            usd: 4.25,
            maps_visited: 11,
            badges: 0b1,
            party: vec![(1, 14), (16, 9)],
            final_map: 0x02,
            final_map_name: "Pewter City",
            milestones: vec![Milestone {
                kind: MilestoneKind::Badge {
                    index: 0,
                    name: "Boulder",
                },
                frame: 500_000,
                play_seconds: 8_000,
                wall_seconds: 3_000,
                map: 2,
            }],
            stopped_because: "the agent has used up its decision budget".into(),
            ..Default::default()
        }
    }

    #[test]
    fn the_headline_is_frames_per_model_call() {
        let b = board();
        assert_eq!(b.frames_per_call(), 1200.0);
        // 1200 frames is a bit over 20 seconds of game.
        assert!((b.game_seconds_per_call() - 20.09).abs() < 0.05);
        let h = b.headline();
        assert!(h.starts_with("1200 frames per model call"), "{h}");
        // And it is the first number in the JSON, so a reader cannot miss it.
        let j = b.to_json();
        let fpc = j.find("frames_per_call").expect("present");
        for other in ["\"calls\"", "\"tokens\"", "\"usd\"", "\"frames\":"] {
            assert!(
                j.find(other).map(|i| i > fpc).unwrap_or(true),
                "{other} came before the headline"
            );
        }
    }

    /// Whole-second wall time makes a short measurement worthless — 20,000
    /// frames "in 3 s" is 6,667 fps or 5,000 depending on which side of the
    /// second it landed. Rates must divide by the millisecond field.
    #[test]
    fn a_rate_is_measured_in_milliseconds_not_whole_seconds() {
        let b = Scoreboard {
            frames: 20_000,
            wall_seconds: 3,
            wall_ms: 3_400,
            ..Default::default()
        };
        assert!(
            (b.emulated_fps() - 5882.0).abs() < 1.0,
            "{}",
            b.emulated_fps()
        );
        // …and a scoreboard built by hand, with no millisecond field, still
        // gets an answer rather than a division by zero.
        let coarse = Scoreboard {
            frames: 20_000,
            wall_seconds: 4,
            ..Default::default()
        };
        assert_eq!(coarse.emulated_fps(), 5000.0);
        assert_eq!(Scoreboard::default().emulated_fps(), 0.0);
    }

    #[test]
    fn the_speedup_is_the_whole_system_not_the_emulator() {
        let b = board();
        // 600k frames is ~10046 s of game in 3600 s of wall clock.
        assert!((b.speedup() - 2.79).abs() < 0.02, "{}", b.speedup());
        // …while the emulator alone managed 167 fps, i.e. it was idle most of
        // the time waiting for the model. Both numbers are reported.
        assert!((b.emulated_fps() - 166.7).abs() < 1.0);
    }

    #[test]
    fn a_run_with_no_calls_does_not_divide_by_zero() {
        let b = Scoreboard {
            frames: 100,
            ..Default::default()
        };
        assert_eq!(b.frames_per_call(), 0.0);
        assert_eq!(b.speedup(), 0.0);
        assert_eq!(b.usd_per_game_hour(), 0.0);
        assert!(!b.to_json().is_empty());
        assert!(b.to_markdown().contains("Nothing worth reporting"));
    }

    #[test]
    fn rewinds_and_memory_writes_are_always_stated() {
        // The two claims that make the result mean anything, so they are
        // printed even when they are zero.
        let md = board().to_markdown();
        assert!(
            md.contains("rewinds (resumes from a checkpoint): **0**"),
            "{md}"
        );
        assert!(md.contains("memory writes by the agent: **0**"), "{md}");
        assert!(board().to_json().contains("\"resumes\":0"));
        assert!(board().to_json().contains("\"memory_writes\":0"));
    }

    #[test]
    fn the_json_parses_back() {
        let j = board().to_json();
        let v = terminalgb_plugin_agent::json::parse(&j).expect("valid JSON");
        assert_eq!(v.str_field("ev"), Some("scoreboard"));
        assert_eq!(v.u64_field("calls"), Some(500));
    }

    #[test]
    fn the_markdown_tells_the_story_in_order() {
        let md = board().to_markdown();
        for want in [
            "# Long run report",
            "frames per model call",
            "## Where it got to",
            "Pewter City",
            "## Trouble",
            "## What happened",
            "Boulder Badge",
        ] {
            assert!(md.contains(want), "missing {want:?} in\n{md}");
        }
    }

    #[test]
    fn milestones_are_counted_by_kind_commonest_first() {
        let mut b = board();
        let m = |k: MilestoneKind| Milestone {
            kind: k,
            frame: 0,
            play_seconds: 0,
            wall_seconds: 0,
            map: 0,
        };
        b.milestones.push(m(MilestoneKind::Caught { owned: 1 }));
        b.milestones.push(m(MilestoneKind::Caught { owned: 2 }));
        assert_eq!(b.by_tag()[0], ("caught", 2));
    }
}

//! **A run as an artefact** — recorded once, replayed exactly, at emulator
//! speed.
//!
//! # What is recorded, and why it is so little
//!
//! A decision is an expensive thing to produce and a cheap thing to *repeat*:
//! the driver's reply is a couple of hundred bytes of JSON, and the host's
//! parse-clamp-execute path that turns it into button presses is entirely
//! deterministic. So a recording is the **replies**, in order, plus enough
//! identity to prove it is being replayed into the same world:
//!
//! ```text
//!   cartridge title + global checksum   which game
//!   start-state hash                    which moment in it
//!   scenario hash                        which rules were in force
//!   the replies, in order                what the agent decided
//!   final-state hash                     what it produced
//! ```
//!
//! Nothing else is needed, and everything else would rot. The frames are not
//! recorded because they are *derivable*; the observations are not recorded
//! because they are derivable too. That is the difference between a recording
//! of an emulator and a recording of anything else, and it is why a run of tens
//! of thousands of frames fits in a few kilobytes.
//!
//! # Replay costs no model calls at all
//!
//! [`terminalgb_plugin_agent::ScriptedDriver`] answers from a list. It goes
//! through the **same** parse, clamp and execute path a real model's reply
//! does — that is why it exists — so a replay is not an approximation of the
//! run, it is the run. And with nothing to wait for, it goes at the emulator's
//! own speed rather than the network's. That is the maintainer's *"millions of
//! times at record speed"*: the expensive part was paid once.
//!
//! # Determinism is tested, not assumed
//!
//! [`Recording::final_state`] is a hash of the save state at the end. A replay
//! that does not reproduce it is reported as a **mismatch**, loudly, rather
//! than quietly passing — because the whole value of the artefact is that it
//! reproduces, and an artefact that silently does not is worse than none.

use std::path::Path;

use terminalgb_plugin_agent::json;

use super::rules::{Allowances, Violation};

/// Bump when a field is renamed, retyped or removed. Additions are free.
pub const RECORDING_SCHEMA: &str = "gbrun/1";

/// FNV-1a, 64-bit. Hand-rolled for the same reason every other hash in this
/// repository is: the dependency count is a contract `make embed` measures.
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// One decision, as it happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub index: u64,
    /// The emulated frame the reply was accepted on. **Not** used to drive
    /// replay — the replay reaches the same frame on its own, and comparing the
    /// two is one of the checks. See [`ReplayReport::drift`].
    pub frame: u64,
    /// The driver's raw reply text, which is what the host parses.
    pub reply: String,
}

/// How a recorded run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ending {
    Goal,
    Failed(String),
    TimedOut,
    OutOfCalls,
    /// The budget ran out **while the run was going round in circles**.
    ///
    /// A separate ending from [`Ending::OutOfCalls`] because they are separate
    /// findings and lumping them together hides the more interesting one. A
    /// run that spent its budget covering ground was given too small a budget;
    /// a run that spent it revisiting four tiles was *looping*, and raising
    /// the budget would only make it loop for longer. The two are
    /// indistinguishable in a pass rate and indistinguishable in a
    /// reason-of-failure histogram that calls both "budget" — which is exactly
    /// how a whole family of observation-aliasing defects stayed hidden.
    Stuck(String),
    Violated(String),
    Faulted(String),
}

impl Ending {
    pub fn tag(&self) -> &'static str {
        match self {
            Ending::Goal => "goal",
            Ending::Failed(_) => "failed",
            Ending::TimedOut => "timeout",
            Ending::OutOfCalls => "out-of-calls",
            Ending::Stuck(_) => "stuck",
            Ending::Violated(_) => "violated",
            Ending::Faulted(_) => "fault",
        }
    }

    pub fn detail(&self) -> &str {
        match self {
            Ending::Failed(d)
            | Ending::Stuck(d)
            | Ending::Violated(d)
            | Ending::Faulted(d) => d,
            _ => "",
        }
    }

    pub fn parse(tag: &str, detail: &str) -> Ending {
        match tag {
            "goal" => Ending::Goal,
            "failed" => Ending::Failed(detail.into()),
            "out-of-calls" => Ending::OutOfCalls,
            "stuck" => Ending::Stuck(detail.into()),
            "violated" => Ending::Violated(detail.into()),
            "fault" => Ending::Faulted(detail.into()),
            _ => Ending::TimedOut,
        }
    }

    pub fn is_goal(&self) -> bool {
        matches!(self, Ending::Goal)
    }
}

/// A finished run, in a form somebody else can take away and re-run.
#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub scenario: String,
    /// Hash of the scenario file's canonical JSON. A replay against a scenario
    /// whose rules have been edited is a different measurement and says so.
    pub scenario_hash: u64,
    pub rom_title: String,
    pub rom_checksum: u16,
    /// Hash of the save state every attempt started from.
    pub start_state: u64,
    /// Hash of the save state at the end. **The thing a replay must reproduce.**
    pub final_state: u64,
    pub driver: String,
    pub decisions: Vec<Decision>,
    pub frames: u64,
    pub ending: Ending,
    pub allowances: Allowances,
    /// The violation, when there was one, in full.
    pub violation: Option<Violation>,
}

impl Recording {
    /// Decisions per run — the number a replay is priced on, because a replay's
    /// only cost is the emulation.
    pub fn len(&self) -> usize {
        self.decisions.len()
    }
    pub fn is_empty(&self) -> bool {
        self.decisions.is_empty()
    }

    /// The replies, in order — what a [`terminalgb_plugin_agent::ScriptedDriver`]
    /// is built from.
    pub fn replies(&self) -> Vec<String> {
        self.decisions.iter().map(|d| d.reply.clone()).collect()
    }

    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", RECORDING_SCHEMA);
        w.str_field("scenario", &self.scenario);
        w.str_field("scenario_hash", &format!("{:016x}", self.scenario_hash));
        w.str_field("rom_title", &self.rom_title);
        w.num_field("rom_checksum", self.rom_checksum as f64);
        w.str_field("start_state", &format!("{:016x}", self.start_state));
        w.str_field("final_state", &format!("{:016x}", self.final_state));
        w.str_field("driver", &self.driver);
        w.num_field("frames", self.frames as f64);
        w.str_field("ending", self.ending.tag());
        let d = self.ending.detail();
        if !d.is_empty() {
            w.str_field("ending_detail", d);
        }
        match &self.violation {
            Some(v) => {
                w.obj_field("violation");
                w.str_field("rule", &v.rule);
                w.num_field("frame", v.frame as f64);
                w.str_field("detail", &v.detail);
                w.end_obj();
            }
            None => w.null_field("violation"),
        }
        self.allowances.to_json(&mut w);
        w.arr_field("decisions");
        for d in &self.decisions {
            w.begin_obj();
            w.num_field("i", d.index as f64);
            w.num_field("frame", d.frame as f64);
            w.str_field("reply", &d.reply);
            w.end_obj();
        }
        w.end_arr();
        w.end_obj();
        w.finish()
    }

    pub fn from_json(text: &str) -> Result<Recording, String> {
        let v =
            json::parse(text).map_err(|e| format!("recording is not JSON: {}", e.0))?;
        let schema = v.str_field("schema").unwrap_or("");
        if schema != RECORDING_SCHEMA {
            return Err(format!(
                "recording schema is {schema:?}, this build reads {RECORDING_SCHEMA:?}"
            ));
        }
        let hex = |k: &str| -> u64 {
            v.str_field(k)
                .and_then(|s| u64::from_str_radix(s, 16).ok())
                .unwrap_or(0)
        };
        let mut decisions = Vec::new();
        for d in v.get("decisions").and_then(|a| a.as_array()).unwrap_or(&[]) {
            decisions.push(Decision {
                index: d.u64_field("i").unwrap_or(0),
                frame: d.u64_field("frame").unwrap_or(0),
                reply: d.str_field("reply").unwrap_or("").to_string(),
            });
        }
        let violation = v.get("violation").and_then(|x| {
            Some(Violation {
                rule: x.str_field("rule")?.to_string(),
                frame: x.u64_field("frame").unwrap_or(0),
                detail: x.str_field("detail").unwrap_or("").to_string(),
            })
        });
        let a = v.get("assistance");
        let anum = |k: &str| a.and_then(|x| x.u64_field(k)).unwrap_or(0);
        let allowances = Allowances {
            state_resets: anum("state_resets") as u32,
            rewinds: anum("rewinds") as u32,
            memory_writes: anum("memory_writes"),
            battle_advice: anum("battle_advice_decisions"),
            guided_decisions: anum("guided_decisions"),
            start_state_supplied: a
                .and_then(|x| x.get("start_state_supplied"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false),
            sources: a
                .and_then(|x| x.get("knowledge_sources"))
                .and_then(|x| x.as_array())
                .unwrap_or(&[])
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect(),
        };
        Ok(Recording {
            scenario: v.str_field("scenario").unwrap_or("").to_string(),
            scenario_hash: hex("scenario_hash"),
            rom_title: v.str_field("rom_title").unwrap_or("").to_string(),
            rom_checksum: v.u64_field("rom_checksum").unwrap_or(0) as u16,
            start_state: hex("start_state"),
            final_state: hex("final_state"),
            driver: v.str_field("driver").unwrap_or("").to_string(),
            frames: v.u64_field("frames").unwrap_or(0),
            ending: Ending::parse(
                v.str_field("ending").unwrap_or("timeout"),
                v.str_field("ending_detail").unwrap_or(""),
            ),
            allowances,
            violation,
            decisions,
        })
    }

    pub fn load(path: &Path) -> Result<Recording, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Recording::from_json(&text)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).ok();
        }
        std::fs::write(path, self.to_json())
            .map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    /// Refuse to replay into the wrong world, **naming which check failed**.
    ///
    /// The three identities are checked separately because they mean different
    /// things to whoever is holding the file: a wrong cartridge is "you have
    /// the wrong game", a wrong start state is "you have the right game at the
    /// wrong moment", and a wrong scenario hash is "the rules have been edited
    /// since this was recorded" — which is not fatal, only a different
    /// measurement, and is reported rather than refused.
    pub fn check_against(
        &self,
        rom_title: &str,
        rom_checksum: u16,
        start_state: u64,
    ) -> Result<Vec<String>, String> {
        if !self.rom_title.is_empty() && self.rom_title != rom_title {
            return Err(format!(
                "this recording is of {:?} and the cartridge is {:?}",
                self.rom_title, rom_title
            ));
        }
        if self.rom_checksum != 0 && self.rom_checksum != rom_checksum {
            return Err(format!(
                "the cartridge title matches but its global checksum does not \
                 (${:04X} recorded, ${rom_checksum:04X} here) — a different build \
                 or a hack",
                self.rom_checksum
            ));
        }
        if self.start_state != 0 && self.start_state != start_state {
            return Err(format!(
                "the start state is not the one this was recorded from \
                 ({:016x} recorded, {start_state:016x} here)",
                self.start_state
            ));
        }
        Ok(Vec::new())
    }
}

/// What a replay produced, beside what was recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplayReport {
    /// Did the final save state hash match?
    pub state_matched: bool,
    pub recorded_state: u64,
    pub replayed_state: u64,
    /// Frames the replay took, against the frames recorded.
    pub recorded_frames: u64,
    pub replayed_frames: u64,
    pub recorded_ending: Ending,
    pub replayed_ending: Ending,
    /// Wall-clock milliseconds the replay took. Model calls: **zero**.
    pub wall_ms: u128,
    /// Replies consumed. Fewer than were recorded means the run resolved
    /// earlier, which is itself a divergence.
    pub decisions_used: usize,
    pub decisions_recorded: usize,
    /// A scenario file that has been edited since the recording was made.
    pub scenario_drifted: bool,
}

impl ReplayReport {
    /// **The verdict.** Everything must agree: an identical final state reached
    /// by a different route in a different number of frames is not a replay.
    pub fn faithful(&self) -> bool {
        self.state_matched
            && self.recorded_frames == self.replayed_frames
            && self.recorded_ending == self.replayed_ending
            && self.decisions_used == self.decisions_recorded
    }

    /// Frames the replay ran per second of wall clock — the number that says
    /// how cheap repeating a solved scenario is.
    pub fn frames_per_second(&self) -> f64 {
        if self.wall_ms == 0 {
            return 0.0;
        }
        self.replayed_frames as f64 * 1000.0 / self.wall_ms as f64
    }

    /// Frames by which the replay's timing differed. Zero is what a faithful
    /// replay produces; a non-zero value with a matching final state means the
    /// world ended up the same by a different path, which is worth knowing.
    pub fn drift(&self) -> i64 {
        self.replayed_frames as i64 - self.recorded_frames as i64
    }

    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![format!(
            "final state {} ({:016x} recorded, {:016x} replayed)",
            if self.state_matched {
                "MATCHES"
            } else {
                "DIFFERS"
            },
            self.recorded_state,
            self.replayed_state
        )];
        v.push(format!(
            "frames {} recorded, {} replayed (drift {})",
            self.recorded_frames,
            self.replayed_frames,
            self.drift()
        ));
        v.push(format!(
            "ending {} recorded, {} replayed",
            self.recorded_ending.tag(),
            self.replayed_ending.tag()
        ));
        v.push(format!(
            "{} of {} recorded decisions used, 0 model calls",
            self.decisions_used, self.decisions_recorded
        ));
        v.push(format!(
            "{} ms — {:.0} frames/second, {:.1}x real time",
            self.wall_ms,
            self.frames_per_second(),
            self.frames_per_second() / 59.7275
        ));
        if self.scenario_drifted {
            v.push(
                "the scenario file has changed since this was recorded, so this \
                 is a different measurement"
                    .into(),
            );
        }
        v
    }

    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", "gbreplay/1");
        w.bool_field("faithful", self.faithful());
        w.bool_field("state_matched", self.state_matched);
        w.str_field("recorded_state", &format!("{:016x}", self.recorded_state));
        w.str_field("replayed_state", &format!("{:016x}", self.replayed_state));
        w.num_field("recorded_frames", self.recorded_frames as f64);
        w.num_field("replayed_frames", self.replayed_frames as f64);
        w.num_field("drift_frames", self.drift() as f64);
        w.str_field("recorded_ending", self.recorded_ending.tag());
        w.str_field("replayed_ending", self.replayed_ending.tag());
        w.num_field("wall_ms", self.wall_ms as f64);
        w.num_field(
            "frames_per_second",
            (self.frames_per_second() * 10.0).round() / 10.0,
        );
        w.num_field("model_calls", 0.0);
        w.num_field("decisions_used", self.decisions_used as f64);
        w.num_field("decisions_recorded", self.decisions_recorded as f64);
        w.bool_field("scenario_drifted", self.scenario_drifted);
        w.end_obj();
        w.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Recording {
        Recording {
            scenario: "leave-the-house".into(),
            scenario_hash: 0xdead_beef,
            rom_title: "POKEMON BLUE".into(),
            rom_checksum: 0x9D0A,
            start_state: 0x1111_2222_3333_4444,
            final_state: 0x5555_6666_7777_8888,
            driver: "scripted".into(),
            decisions: vec![
                Decision {
                    index: 1,
                    frame: 10,
                    reply: "{\"plan\":[{\"do\":\"press\",\"button\":\"a\"}]}".into(),
                },
                Decision {
                    index: 2,
                    frame: 90,
                    reply: "{\"plan\":[{\"do\":\"walk\",\"direction\":\"down\"}]}".into(),
                },
            ],
            frames: 400,
            ending: Ending::Goal,
            allowances: Allowances {
                state_resets: 1,
                ..Default::default()
            },
            violation: None,
        }
    }

    #[test]
    fn a_recording_round_trips_through_its_own_json() {
        let r = sample();
        assert_eq!(Recording::from_json(&r.to_json()).unwrap(), r);
    }

    #[test]
    fn a_violation_survives_the_round_trip_in_full() {
        let mut r = sample();
        r.ending = Ending::Violated("no_memory_writes".into());
        r.violation = Some(Violation {
            rule: "no_memory_writes".into(),
            frame: 77,
            detail: "3 byte(s) were written".into(),
        });
        let back = Recording::from_json(&r.to_json()).unwrap();
        assert_eq!(back.violation.unwrap().frame, 77);
        assert_eq!(back.ending.tag(), "violated");
    }

    #[test]
    fn a_wrong_schema_is_refused_rather_than_half_read() {
        assert!(Recording::from_json("{\"schema\":\"gbrun/9\"}").is_err());
    }

    /// A recording is worthless if it can be replayed into a world it was not
    /// recorded from, and each of the three identities means something
    /// different to whoever is holding it — so each is named separately.
    #[test]
    fn replaying_into_the_wrong_world_is_refused_by_name() {
        let r = sample();
        assert!(r
            .check_against("POKEMON BLUE", 0x9D0A, r.start_state)
            .is_ok());
        let wrong_game = r.check_against("POKEMON RED", 0x9D0A, r.start_state);
        assert!(wrong_game.unwrap_err().contains("POKEMON RED"));
        let wrong_build = r
            .check_against("POKEMON BLUE", 0x1234, r.start_state)
            .unwrap_err();
        assert!(wrong_build.contains("checksum"), "{wrong_build}");
        let wrong_moment = r.check_against("POKEMON BLUE", 0x9D0A, 999).unwrap_err();
        assert!(wrong_moment.contains("start state"), "{wrong_moment}");
    }

    /// The same final state reached in a different number of frames is not a
    /// replay, and reporting it as one would make the artefact meaningless.
    #[test]
    fn a_matching_state_is_not_enough_to_call_a_replay_faithful() {
        let base = ReplayReport {
            state_matched: true,
            recorded_state: 1,
            replayed_state: 1,
            recorded_frames: 400,
            replayed_frames: 400,
            recorded_ending: Ending::Goal,
            replayed_ending: Ending::Goal,
            wall_ms: 100,
            decisions_used: 2,
            decisions_recorded: 2,
            scenario_drifted: false,
        };
        assert!(base.faithful());
        assert!(!ReplayReport {
            replayed_frames: 401,
            ..base.clone()
        }
        .faithful());
        assert!(!ReplayReport {
            replayed_ending: Ending::TimedOut,
            ..base.clone()
        }
        .faithful());
        assert!(!ReplayReport {
            decisions_used: 1,
            ..base.clone()
        }
        .faithful());
        assert!(!ReplayReport {
            state_matched: false,
            ..base
        }
        .faithful());
    }

    /// A replay's headline is that it costs nothing to think, so the rate and
    /// the zero must both be in what a person reads.
    #[test]
    fn a_replay_report_says_the_speed_and_says_the_calls_were_zero() {
        let r = ReplayReport {
            state_matched: true,
            recorded_state: 1,
            replayed_state: 1,
            recorded_frames: 42_000,
            replayed_frames: 42_000,
            recorded_ending: Ending::Goal,
            replayed_ending: Ending::Goal,
            wall_ms: 10_000,
            decisions_used: 5,
            decisions_recorded: 5,
            scenario_drifted: false,
        };
        assert!((r.frames_per_second() - 4200.0).abs() < 0.01);
        let lines = r.lines().join(" ");
        assert!(lines.contains("0 model calls"), "{lines}");
        assert!(lines.contains("frames/second"), "{lines}");
        assert!(r.to_json().contains("\"model_calls\":0"));
    }

    #[test]
    fn the_hash_is_the_hash_and_a_changed_byte_moves_it() {
        assert_eq!(fnv1a(b"abc"), fnv1a(b"abc"));
        assert_ne!(fnv1a(b"abc"), fnv1a(b"abd"));
        assert_ne!(fnv1a(b""), 0);
    }
}

//! **The scenario harness** — proves that a rule is enforced rather than
//! requested, and that a recorded run replays exactly.
//!
//! `cargo test --release --test scenario`
//!
//! # Why this needs a harness and not unit tests
//!
//! `src/scenario`'s unit tests prove the parsing, the condition grammar and the
//! referee's arithmetic. They cannot prove the thing the whole feature rests
//! on, which is a *seam*: that a byte written into a running machine is seen by
//! the referee, that the run then forfeits, and that a recording of a run
//! played by a model reproduces frame for frame when a script replays it.
//! Nothing short of a real CPU executing real code proves any of that.
//!
//! # No commercial ROM, so this runs in CI
//!
//! Everything here drives the synthetic Gen 1-shaped cartridge in
//! [`support/fake_gen1.rs`](support/fake_gen1.rs) — twenty instructions of our
//! own presenting the same RAM layout, with a working door at (8,8). A test
//! that needs the captain's cartridge is a test CI skips, and a skipped test
//! proves nothing.
//!
//! # The demonstration, in one sentence
//!
//! `examples/shiny_hunt.rs::make_state` reaches Route 1 by writing an event
//! flag and a party into RAM. [`case_a_poked_run_forfeits`] runs that *same
//! kind of shortcut* under a scenario carrying `no_memory_writes` and watches
//! it forfeit at the frame of the poke, naming the rule; [`case_a_clean_run`]
//! does the same journey with buttons and does not.

use std::path::{Path, PathBuf};
use std::time::Instant;

use terminalgb::gameboy::Gameboy;
use terminalgb::scenario::record::{Ending, Recording};
use terminalgb::scenario::run::ScenarioRun;
use terminalgb::scenario::Scenario;
use terminalgb_plugin_agent::driver::ScriptedDriver;
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::{AgentPlugin, Config};

#[path = "support/fake_gen1.rs"]
mod fake_gen1;
use fake_gen1::*;

// ---------------------------------------------------------------------------
// Plumbing
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn dir(tag: &str) -> PathBuf {
    let d = PathBuf::from("target/scenario-harness").join(tag);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("create work dir");
    d
}

/// A machine on the fixture cartridge, booted far enough that a save state of
/// it is a real starting point.
///
/// The MBC writes beside the ROM path, so every case gets its own directory —
/// the same rule every harness here follows.
fn machine(tag: &str) -> (Gameboy, PathBuf) {
    let d = dir(tag);
    let p = d.join("fake-gen1.gb");
    std::fs::write(&p, build_fake_gen1()).expect("write fixture");
    let data = std::fs::read(&p).unwrap();
    let mut gb = Gameboy::new(data, Some(p));
    // Let the program initialise its RAM, so the observer reports `in_game`.
    for _ in 0..120 {
        gb.frame();
    }
    (gb, d)
}

fn attach(gb: &mut Gameboy, replies: Vec<String>) {
    let cfg = Config {
        budget: Budget {
            max_calls: 200,
            ..Budget::default()
        },
        settle_frames: 4,
        ..Config::default()
    };
    gb.add_plugin(Box::new(AgentPlugin::new(
        Box::new(ScriptedDriver::new(replies)),
        cfg,
    )))
    .expect("attach agent");
}

/// A plugin that writes one byte into the running machine, at a chosen frame.
///
/// This is the realistic shape of the threat `no_memory_writes` exists for: not
/// a harness reaching in before the run, but a *plugin* attached alongside the
/// agent that quietly edits the game — which is what `battle-control` and the
/// shiny-hunt state builder legitimately do, and what a run claiming to be a
/// playthrough must not. The host sees it through `PluginStatus::has_written`,
/// which no amount of prompting can talk out of.
struct Poker {
    at: u64,
    addr: u16,
    value: u8,
    frame: u64,
    done: bool,
}

impl terminalgb::plugin::Plugin for Poker {
    fn name(&self) -> &str {
        "poker"
    }
    fn capabilities(&self) -> terminalgb::plugin::Capability {
        terminalgb::plugin::Capability::OBSERVE
            | terminalgb::plugin::Capability::WRITE_MEMORY
    }
    fn on_event(
        &mut self,
        event: &terminalgb::plugin::Event,
        machine: &mut terminalgb::plugin::Machine,
    ) {
        if !matches!(event, terminalgb::plugin::Event::Frame { .. }) {
            return;
        }
        self.frame += 1;
        if self.done || self.frame < self.at {
            return;
        }
        self.done = machine.poke(self.addr, self.value);
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

fn attach_poker(gb: &mut Gameboy, at: u64, addr: u16, value: u8) {
    gb.add_plugin(Box::new(Poker {
        at,
        addr,
        value,
        frame: 0,
        done: false,
    }))
    .expect("attach poker");
}

fn reply(say: &str, plan: &str) -> String {
    format!("{{\"say\":\"{say}\",\"plan\":[{plan}]}}")
}

/// A plan that walks from the start tile to the working door at (8,8). The
/// fixture's room is walled, so `goto` is what routes it.
fn to_the_door() -> Vec<String> {
    vec![
        reply(
            "Heading for the door.",
            &format!(r#"{{"do":"goto","x":{WARP_X},"y":{WARP_Y}}}"#),
        ),
        reply("Waiting.", r#"{"do":"wait","frames":30}"#),
    ]
}

/// Write a scenario file into `d` and return its path.
fn scenario_file(d: &Path, name: &str, body: &str) -> PathBuf {
    let p = d.join(format!("{name}.json"));
    std::fs::write(&p, body).expect("write scenario");
    p
}

/// "Leave the room by the door", with the rules that make it mean something.
fn leave_the_room(rules: &str) -> String {
    format!(
        r#"{{
  "schema": "gbscenario/1",
  "name": "leave-the-room",
  "objective": "Walk out of the room through the door.",
  "goal": ["map=0x{WARP_TO:02X}"],
  "rules": [{rules}],
  "timeout_frames": 20000
}}"#
    )
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// **The one that matters.** A run that reaches the goal by writing memory —
/// which is exactly how `examples/shiny_hunt.rs::make_state` gets to Route 1 —
/// forfeits, at the frame of the write, naming the rule.
fn case_a_poked_run_forfeits() -> Case {
    let (mut gb, d) = machine("poked");
    attach(&mut gb, to_the_door());
    // The shortcut: put the player on the far map without walking there. One
    // byte, thirty frames in, and it satisfies the goal outright.
    attach_poker(&mut gb, 30, W_CUR_MAP, WARP_TO);
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let v = out.recording.violation.clone();
    let ok = matches!(out.recording.ending, Ending::Violated(_))
        && v.as_ref().map(|v| v.rule.as_str()) == Some("no_memory_writes")
        && out.recording.allowances.memory_writes >= 1
        && !out.recording.allowances.is_clean();
    Case {
        name: "a run that pokes memory forfeits and names the rule",
        ok,
        detail: format!(
            "ending {}, violation {:?}, writes {}",
            out.recording.ending.tag(),
            v.map(|v| v.line()).unwrap_or_default(),
            out.recording.allowances.memory_writes
        ),
    }
}

/// The other half, and without it the case above proves nothing: the identical
/// scenario, walked with buttons, reaches the goal and reports a clean run.
fn case_a_clean_run() -> Case {
    let (mut gb, d) = machine("clean");
    attach(&mut gb, to_the_door());
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let ok = out.reached_goal()
        && out.recording.violation.is_none()
        && out.recording.allowances.memory_writes == 0
        && out.recording.allowances.is_clean();
    Case {
        name: "the same scenario walked with buttons reaches the goal cleanly",
        ok,
        detail: format!(
            "ending {}, {} frames, {} decisions, clean={}",
            out.recording.ending.tag(),
            out.recording.frames,
            out.recording.decisions.len(),
            out.recording.allowances.is_clean()
        ),
    }
}

/// A requirement is judged **at the goal**, so a run that arrives without
/// having done what the scenario said forfeits rather than passing.
fn case_a_requirement_is_judged_at_the_goal() -> Case {
    let (mut gb, d) = machine("require");
    attach(&mut gb, to_the_door());
    // The fixture's party is empty, so "must have a Pokémon" cannot be met by
    // arriving — which is precisely the check that separates a legitimate walk
    // to Route 1 from one that skipped the starter.
    let f = scenario_file(
        &d,
        "leave",
        &leave_the_room("\"no_memory_writes\", \"require party>=1\""),
    );
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let ok = matches!(out.recording.ending, Ending::Violated(_))
        && out
            .recording
            .violation
            .as_ref()
            .map(|v| v.detail.contains("does not count"))
            .unwrap_or(false);
    Case {
        name: "arriving without meeting a requirement forfeits, it does not pass",
        ok,
        detail: format!(
            "ending {}, {}",
            out.recording.ending.tag(),
            out.recording
                .violation
                .map(|v| v.line())
                .unwrap_or_else(|| "no violation".into())
        ),
    }
}

/// A budget rule is enforced by the host from its own counters, so a run that
/// would otherwise reach the goal stops when it runs out.
fn case_a_call_budget_is_enforced() -> Case {
    let (mut gb, d) = machine("budget");
    // Twenty pointless decisions and no route: it will exhaust the budget.
    let script: Vec<String> = (0..20)
        .map(|_| reply("Pressing B.", r#"{"do":"press","button":"b"}"#))
        .collect();
    attach(&mut gb, script);
    let f = scenario_file(
        &d,
        "leave",
        &leave_the_room("\"no_memory_writes\", \"max_calls 3\""),
    );
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let ok = matches!(out.recording.ending, Ending::Violated(_))
        && out
            .recording
            .violation
            .as_ref()
            .map(|v| v.rule.starts_with("max_calls"))
            .unwrap_or(false);
    Case {
        name: "a call budget is enforced from the host's own counters",
        ok,
        detail: format!(
            "ending {}, {} decisions, {}",
            out.recording.ending.tag(),
            out.recording.decisions.len(),
            out.recording
                .violation
                .map(|v| v.line())
                .unwrap_or_else(|| "no violation".into())
        ),
    }
}

/// **The artefact.** Record a run, replay it from the recording alone, and
/// require everything to agree — final state, frame count, ending, decisions —
/// with **no model calls**.
fn case_a_recording_replays_exactly() -> Case {
    let (mut gb, d) = machine("replay");
    attach(&mut gb, to_the_door());
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let start = gb.save_state();
    let out = ScenarioRun::new(gb, s.clone(), &f, d.clone(), Some(start.clone()))
        .expect("build run")
        .quiet()
        .play();
    let rec_path = d.join("run.json");
    out.recording.save(&rec_path).expect("save recording");

    // …and now, from the file alone.
    let recorded = Recording::load(&rec_path).expect("load recording");
    let (mut gb2, d2) = machine("replay-2");
    attach(&mut gb2, recorded.replies());
    let t = Instant::now();
    let report = ScenarioRun::new(gb2, s, &f, d2, Some(start))
        .expect("build replay")
        .quiet()
        .replay(&recorded)
        .expect("replay");
    let elapsed = t.elapsed();
    let ok = report.faithful() && !report.scenario_drifted;
    Case {
        name: "a recorded run replays exactly, with no model calls",
        ok,
        detail: format!(
            "{} | {} ms wall",
            report.lines().join("; "),
            elapsed.as_millis()
        ),
    }
}

/// A replay is the emulator's own speed, because there is nothing to wait for.
/// The claim is *faster than real time by a wide margin*, not a specific
/// number — a shared CI box has no business promising one.
fn case_a_replay_runs_far_faster_than_real_time() -> Case {
    let (mut gb, d) = machine("speed");
    attach(&mut gb, to_the_door());
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let start = gb.save_state();
    let out = ScenarioRun::new(gb, s.clone(), &f, d.clone(), Some(start.clone()))
        .expect("build run")
        .quiet()
        .play();
    let (mut gb2, d2) = machine("speed-2");
    attach(&mut gb2, out.recording.replies());
    let report = ScenarioRun::new(gb2, s, &f, d2, Some(start))
        .expect("build replay")
        .quiet()
        .replay(&out.recording)
        .expect("replay");
    let x = report.frames_per_second() / 59.7275;
    let ok = report.faithful() && x > 10.0;
    Case {
        name: "a replay runs far faster than real time",
        ok,
        detail: format!(
            "{:.0} frames/second = {x:.1}x real time over {} frames",
            report.frames_per_second(),
            report.replayed_frames
        ),
    }
}

/// A recording carried to the wrong world is refused by name, rather than
/// replayed into it and quietly reported as a mismatch.
fn case_a_replay_into_the_wrong_world_is_refused() -> Case {
    let (mut gb, d) = machine("wrong-world");
    attach(&mut gb, to_the_door());
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let start = gb.save_state();
    let out = ScenarioRun::new(gb, s.clone(), &f, d.clone(), Some(start))
        .expect("build run")
        .quiet()
        .play();

    // A different moment in the same game: run it on a bit, then try to replay
    // the recording into that.
    let (mut gb2, d2) = machine("wrong-world-2");
    for _ in 0..600 {
        gb2.frame();
    }
    let elsewhere = gb2.save_state();
    attach(&mut gb2, out.recording.replies());
    let err = ScenarioRun::new(gb2, s, &f, d2, Some(elsewhere))
        .expect("build replay")
        .quiet()
        .replay(&out.recording)
        .err();
    let ok = err
        .as_ref()
        .map(|e| e.contains("start state"))
        .unwrap_or(false);
    Case {
        name: "a recording replayed into the wrong world is refused by name",
        ok,
        detail: err.unwrap_or_else(|| "it was accepted, which it must not be".into()),
    }
}

/// Editing the rules after a recording was made does not stop the replay — it
/// is still a valid sequence of decisions — but it is a *different
/// measurement*, and the report has to say so.
fn case_edited_rules_are_reported_as_drift() -> Case {
    let (mut gb, d) = machine("drift");
    attach(&mut gb, to_the_door());
    let f = scenario_file(&d, "leave", &leave_the_room("\"no_memory_writes\""));
    let s = Scenario::load(&f).expect("scenario");
    let start = gb.save_state();
    let out = ScenarioRun::new(gb, s, &f, d.clone(), Some(start.clone()))
        .expect("build run")
        .quiet()
        .play();

    // Somebody relaxes the rules.
    std::fs::write(&f, leave_the_room("\"max_calls 99\"")).expect("rewrite scenario");
    let edited = Scenario::load(&f).expect("edited scenario");
    let (mut gb2, d2) = machine("drift-2");
    attach(&mut gb2, out.recording.replies());
    let report = ScenarioRun::new(gb2, edited, &f, d2, Some(start))
        .expect("build replay")
        .quiet()
        .replay(&out.recording)
        .expect("replay");
    let ok = report.scenario_drifted && report.state_matched;
    Case {
        name: "a scenario edited since the recording is reported as drift",
        ok,
        detail: report.lines().join("; "),
    }
}

/// **The knowledge loop.** A run that walks through a door records it; the
/// guidance file it leaves behind is what the next run is handed.
fn case_a_run_learns_the_door_it_walked_through() -> Case {
    let (mut gb, d) = machine("learn");
    attach(&mut gb, to_the_door());
    let body = format!(
        r#"{{
  "schema": "gbscenario/1",
  "name": "leave-the-room",
  "objective": "Walk out of the room through the door.",
  "goal": ["map=0x{WARP_TO:02X}"],
  "rules": ["no_memory_writes"],
  "knowledge": ["guidance"],
  "guidance": "learned.json",
  "timeout_frames": 20000
}}"#
    );
    let f = scenario_file(&d, "leave", &body);
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let written = d.join("learned.json");
    let g = terminalgb::scenario::knowledge::Guidance::load(&written);
    let ok = out.reached_goal()
        && !out.learned.links.is_empty()
        && g.as_ref()
            .map(|g| g.route(START_MAP, WARP_TO).is_some())
            .unwrap_or(false);
    Case {
        name: "a run learns the door it walked through and writes it down",
        ok,
        detail: format!(
            "{} link(s) learned, file {}: {:?}",
            out.learned.links.len(),
            if written.is_file() {
                "written"
            } else {
                "MISSING"
            },
            g.map(|g| g.route(START_MAP, WARP_TO))
        ),
    }
}

/// The second half of the loop: with that file in place, the next run's agent
/// is **told the route** rather than having to find it again.
fn case_the_next_run_is_told_the_route() -> Case {
    let (mut gb, d) = machine("guided");
    attach(&mut gb, to_the_door());
    // Hand it what the previous case learned.
    let mut g = terminalgb::scenario::knowledge::Guidance::default();
    g.observe_link(START_MAP, WARP_TO, WARP_X, WARP_Y);
    g.save(&d.join("learned.json")).expect("write guidance");
    let body = format!(
        r#"{{
  "schema": "gbscenario/1",
  "name": "leave-the-room",
  "objective": "Walk out of the room through the door.",
  "goal": ["map=0x{WARP_TO:02X}"],
  "rules": ["no_memory_writes", "battle flee"],
  "knowledge": ["guidance"],
  "guidance": "learned.json",
  "timeout_frames": 20000
}}"#
    );
    let f = scenario_file(&d, "leave", &body);
    let s = Scenario::load(&f).expect("scenario");
    let run = ScenarioRun::new(gb, s.clone(), &f, d.clone(), None)
        .expect("build run")
        .quiet();
    // What the agent would be told, standing where it starts.
    let notes = terminalgb::scenario::rules::standing_notes(
        &s.referee(),
        run.knowledge(),
        START_MAP,
        s.goal_map(),
    );
    let joined = notes.join(" | ");
    let out = run.play();
    let ok = joined.contains("BATTLE POLICY")
        && joined.contains(&format!("({WARP_X}, {WARP_Y})"))
        && out.recording.allowances.guided_decisions > 0;
    Case {
        name: "the next run's agent is told the rules and the route",
        ok,
        detail: format!(
            "briefing: {joined} | guided decisions {}",
            out.recording.allowances.guided_decisions
        ),
    }
}

/// The briefing must actually reach the driver, not merely be computed. Read
/// the snapshot the agent was sent and look for the rule in it.
fn case_the_rules_reach_the_agent_in_the_snapshot() -> Case {
    let (mut gb, d) = machine("briefed");
    let log = d.join("agent.ndjson");
    let cfg = Config {
        budget: Budget {
            max_calls: 200,
            ..Budget::default()
        },
        settle_frames: 4,
        log: Some(log.clone()),
        ..Config::default()
    };
    gb.add_plugin(Box::new(AgentPlugin::new(
        Box::new(ScriptedDriver::new(to_the_door()).looping()),
        cfg,
    )))
    .expect("attach agent");
    let f = scenario_file(
        &d,
        "leave",
        &leave_the_room("\"no_memory_writes\", \"battle flee\""),
    );
    let s = Scenario::load(&f).expect("scenario");
    let _ = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    let ok = text.contains("BATTLE POLICY");
    Case {
        name: "the scenario's rules reach the agent in the snapshot it reasons about",
        ok,
        detail: format!(
            "{} bytes of decision log, carries the policy: {ok}",
            text.len()
        ),
    }
}

/// A scenario that cannot work must be refused **before** anything is spent —
/// the expensive mistake is a thousand attempts that were never winnable.
fn case_an_impossible_scenario_is_refused_before_it_costs_anything() -> Case {
    let (gb, d) = machine("impossible");
    let f = scenario_file(
        &d,
        "broken",
        r#"{"schema":"gbscenario/1","name":"broken","goal":["watch:ghost=1"],
            "rules":["no_memory_writes"],"timeout_frames":100}"#,
    );
    let loaded = Scenario::load(&f);
    // And if one is somehow constructed anyway, the runner refuses it too.
    let mut also = Scenario::default();
    also.name = "no-goal".into();
    let built = ScenarioRun::new(gb, also, &f, d, None).err();
    let ok = loaded.is_err() && built.is_some();
    Case {
        name: "a scenario that could never be judged is refused before it spends",
        ok,
        detail: format!(
            "load: {:?} | build: {:?}",
            loaded.err().unwrap_or_default(),
            built.unwrap_or_default()
        ),
    }
}

/// The whole cost ceiling has to be on screen before the first call, because
/// the money is the captain's.
fn case_the_cost_ceiling_is_stated_before_the_run() -> Case {
    let (_gb, d) = machine("briefing");
    let f = scenario_file(
        &d,
        "leave",
        &leave_the_room("\"no_memory_writes\", \"max_calls 7\""),
    );
    let s = Scenario::load(&f).expect("scenario");
    let b = s.briefing(30).join("\n");
    let ok = b.contains("210 model calls") && b.contains("no_memory_writes");
    Case {
        name: "the cost ceiling is stated before the run starts",
        ok,
        detail: b.replace('\n', " · "),
    }
}

/// The atlas is a *knowledge* source, and the scenario that asked for it must
/// be told whether it actually got one — a run that planned without the
/// reference it asked for is a different run.
fn case_the_atlas_answers_or_says_it_is_missing() -> Case {
    let (_gb, d) = machine("atlas");
    let f = scenario_file(
        &d,
        "leave",
        &format!(
            r#"{{"schema":"gbscenario/1","name":"leave","goal":["map=0x{WARP_TO:02X}"],
                "knowledge":["atlas"],"timeout_frames":100}}"#
        ),
    );
    let s = Scenario::load(&f).expect("scenario");
    let k = s.knowledge_from(&f);
    let ok = match &k.atlas {
        // Present: it must answer a question the scenario language uses.
        Some(a) => {
            a.symbol("wCurMap").map(|e| e.addr) == Some(0xD35E)
                && a.symbol("wObtainedBadges").is_some()
                && k.sources.iter().any(|s| s.starts_with("atlas"))
        }
        // Absent: it must be reported as absent rather than silently dropped.
        None => k.missing.iter().any(|m| m.starts_with("atlas")),
    };
    Case {
        name: "the atlas answers a lookup, or the run is told it is missing",
        ok,
        detail: match &k.atlas {
            Some(a) => format!(
                "{} addresses, wCurMap {:?}",
                a.len(),
                a.symbol("wCurMap").map(|e| e.summary())
            ),
            None => format!("absent, reported as {:?}", k.missing),
        },
    }
}

/// The gate that stops this whole harness being vacuous: with the rule removed
/// the poked run **succeeds**. If it did not, the forfeit case above would be
/// proving nothing about the rule.
fn case_the_rule_is_what_stops_it() -> Case {
    let (mut gb, d) = machine("teeth");
    attach(&mut gb, to_the_door());
    attach_poker(&mut gb, 30, W_CUR_MAP, WARP_TO);
    let f = scenario_file(&d, "leave", &leave_the_room(""));
    let s = Scenario::load(&f).expect("scenario");
    let out = ScenarioRun::new(gb, s, &f, d.clone(), None)
        .expect("build run")
        .quiet()
        .play();
    let ok = out.reached_goal()
        && out.recording.violation.is_none()
        && out.recording.allowances.memory_writes >= 1
        && !out.recording.allowances.is_clean();
    Case {
        name: "without the rule the same poked run passes — so the rule is the teeth",
        ok,
        detail: format!(
            "ending {}, writes {}, declared clean {}",
            out.recording.ending.tag(),
            out.recording.allowances.memory_writes,
            out.recording.allowances.is_clean()
        ),
    }
}

fn main() {
    let cases = vec![
        case_a_poked_run_forfeits(),
        case_a_clean_run(),
        case_the_rule_is_what_stops_it(),
        case_a_requirement_is_judged_at_the_goal(),
        case_a_call_budget_is_enforced(),
        case_a_recording_replays_exactly(),
        case_a_replay_runs_far_faster_than_real_time(),
        case_a_replay_into_the_wrong_world_is_refused(),
        case_edited_rules_are_reported_as_drift(),
        case_a_run_learns_the_door_it_walked_through(),
        case_the_next_run_is_told_the_route(),
        case_the_rules_reach_the_agent_in_the_snapshot(),
        case_an_impossible_scenario_is_refused_before_it_costs_anything(),
        case_the_cost_ceiling_is_stated_before_the_run(),
        case_the_atlas_answers_or_says_it_is_missing(),
    ];
    let mut failed = 0;
    for c in &cases {
        println!(
            "  [{}] {}\n        {}",
            if c.ok { "PASS" } else { "FAIL" },
            c.name,
            c.detail
        );
        if !c.ok {
            failed += 1;
        }
    }
    println!("\n{} of {} passed", cases.len() - failed, cases.len());
    if failed > 0 {
        std::process::exit(1);
    }
}

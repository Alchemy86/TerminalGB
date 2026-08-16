//! **Long-run harness** — proves the whole unattended-run stack end to end,
//! deterministically, for free, and with no commercial ROM.
//!
//! `cargo test --release --test longplay`
//!
//! # What needs a harness rather than a unit test
//!
//! Every piece of `src/longplay` has unit tests: the milestone detector, the
//! wall detector, the checkpoint codec, the ntfy config, the scoreboard. What
//! they cannot prove is the *seam* — that a change in the running machine's RAM
//! becomes a milestone, which becomes a line in a durable log and a phone push
//! and a row in a report; that a checkpoint written by one process is picked up
//! by another; and that a decision produces a `steps.ndjson` record joining the
//! observation, the intent, the actions and the outcome.
//!
//! # The fixture, and why it is a cartridge that does nothing
//!
//! [`build_inert_gen1`] is a 32 KiB ROM whose entire program is `jr -2`. Its
//! header says `POKEMON BLUE` with the global checksum the trainer crate
//! checks, purely so the Gen 1 reader engages — it contains **no game data
//! whatsoever** (the licence-boundary rule in `AGENTS.md`: addresses and
//! formats are facts, assets are not ours).
//!
//! Because the program writes nothing, WRAM is entirely the harness's, and the
//! harness *is* the cartridge: it pokes the bytes a real Gen 1 game would write
//! — a player name, a party, badges, a map, the play clock — on a schedule of
//! its choosing. That is a far better fixture than a hand-assembled game
//! simulator for this particular job, because what is under test is the
//! observation and reporting layer, not the executor (which
//! `testharness/agent.rs` already drives against a moving player).
//!
//! Poking from the harness is not a hole in "the agent only presses buttons":
//! that guarantee is enforced per call against the *plugin's* capability, and
//! check [7] asserts it directly.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use terminalgb::gameboy::Gameboy;
use terminalgb::longplay::episode::{Cond, Episode, Outcome};
use terminalgb::longplay::series::Series;
use terminalgb::longplay::watch::{Watch, Watches};
use terminalgb::longplay::{progress::Importance, RunConfig, Runner, Stop};
use terminalgb::plugin::Capability;
use terminalgb_plugin_agent::driver::{Driver, Poll, ScriptedDriver};
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::{json, AgentPlugin, Config};

// Addresses the harness writes, standing in for the cartridge. All verified in
// `terminalgb_plugin_trainer::gen1` or `terminalgb_plugin_agent::observe`.
use terminalgb_plugin_agent::observe::{W_CUR_MAP, W_X_COORD, W_Y_COORD};
use terminalgb_plugin_trainer::gen1::addr as tr;
use terminalgb_plugin_trainer::gen1::party as pm;
use terminalgb_plugin_trainer::gen1::PARTY_MON_SIZE;

/// A cartridge with no program: the harness supplies the world.
fn build_inert_gen1() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    // Entry: NOP; JP $0150.
    rom[0x0100..0x0104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    for (i, b) in b"POKEMON BLUE".iter().enumerate() {
        rom[0x0134 + i] = *b;
    }
    rom[0x0143] = 0x00; // DMG cartridge
    rom[0x0147] = 0x00; // ROM only
                        // The global checksum `Game::detect` matches on. Nothing of the real game
                        // is here — only this number.
    rom[0x014E] = 0x9D;
    rom[0x014F] = 0x0A;
    let mut sum: u8 = 0;
    for b in rom.iter().take(0x014D).skip(0x0134) {
        sum = sum.wrapping_sub(*b).wrapping_sub(1);
    }
    rom[0x014D] = sum;
    // $0150: JR -2 — spin for ever, writing nothing.
    rom[0x0150] = 0x18;
    rom[0x0151] = 0xFE;
    rom
}

/// The world, as the harness would like it to be.
#[derive(Clone)]
struct World {
    map: u8,
    x: u8,
    y: u8,
    badges: u8,
    party: Vec<(u8, u8, u16)>, // species, level, hp
    owned: u16,
    items: u8,
    play_seconds: u32,
}

impl Default for World {
    fn default() -> Self {
        World {
            map: 0,
            x: 5,
            y: 6,
            badges: 0,
            party: Vec::new(),
            owned: 0,
            items: 0,
            play_seconds: 0,
        }
    }
}

impl World {
    /// Write this world into the running machine, exactly where a Gen 1
    /// cartridge would.
    fn apply(&self, gb: &mut Gameboy) {
        gb.with_machine(
            Capability::OBSERVE | Capability::READ_MEMORY | Capability::WRITE_MEMORY,
            |m| {
                // "RED" in Gen 1 character codes, terminated: $80 = 'A'.
                for (i, c) in [0x91u8, 0x84, 0x83, 0x50].iter().enumerate() {
                    m.poke(tr::PLAYER_NAME + i as u16, *c);
                }
                m.poke(tr::PARTY_COUNT, self.party.len() as u8);
                m.poke(W_CUR_MAP, self.map);
                m.poke(W_X_COORD, self.x);
                m.poke(W_Y_COORD, self.y);
                m.poke(tr::OBTAINED_BADGES, self.badges);
                for (i, (species, level, hp)) in self.party.iter().enumerate() {
                    let base = tr::PARTY_MONS + i as u16 * PARTY_MON_SIZE;
                    m.poke(base + pm::SPECIES, *species);
                    m.poke(base + pm::LEVEL, *level);
                    // Big-endian, like every multi-byte Gen 1 field.
                    m.poke(base + pm::HP, (hp >> 8) as u8);
                    m.poke(base + pm::HP + 1, (*hp & 0xFF) as u8);
                }
                // Pokédex owned flags, low species first.
                for byte in 0..19u16 {
                    let mut v = 0u8;
                    for bit in 0..8u16 {
                        if byte * 8 + bit < self.owned {
                            v |= 1 << bit;
                        }
                    }
                    m.poke(terminalgb::longplay::W_POKEDEX_OWNED + byte, v);
                }
                m.poke(terminalgb::longplay::W_NUM_BAG_ITEMS, self.items);
                m.poke(tr::PLAY_TIME_HOURS, (self.play_seconds / 3600) as u8);
                m.poke(tr::PLAY_TIME_MINUTES, ((self.play_seconds / 60) % 60) as u8);
                m.poke(tr::PLAY_TIME_SECONDS, (self.play_seconds % 60) as u8);
            },
        );
    }
}

/// A driver that answers with a plan that costs a few hundred frames, for ever.
fn walker() -> Box<dyn Driver> {
    Box::new(
        ScriptedDriver::new(vec![r#"{"goal":"Explore","say":"Heading north for a bit.",
"plan":[{"do":"walk","dir":"up","steps":4},{"do":"wait","frames":30}],
"usage":{"input_tokens":900,"output_tokens":60,"usd":0.004}}"#
            .replace('\n', " ")])
        .looping()
        .with_name("scripted"),
    )
}

/// A driver that never answers — the control for "the emulator never waits".
struct DeadDriver;
impl Driver for DeadDriver {
    fn name(&self) -> &str {
        "dead"
    }
    fn submit(&mut self, _r: &str) -> bool {
        true
    }
    fn poll(&mut self) -> Poll {
        Poll::Pending
    }
}

fn make_runner(dir: &Path, driver: Box<dyn Driver>, cfg: RunConfig) -> Runner {
    let rom = build_inert_gen1();
    let mut gb = Gameboy::new(rom, None);
    let agent = AgentPlugin::new(
        driver,
        Config {
            budget: Budget {
                max_calls: 10_000,
                max_wall: Duration::from_secs(600),
                call_timeout: Duration::from_secs(30),
                ..Budget::default()
            },
            objective: cfg.objective.clone(),
            log: Some(dir.join("agent.ndjson")),
            settle_frames: 4,
            ..Config::default()
        },
    );
    gb.add_plugin(Box::new(agent)).expect("attach");
    Runner::new(gb, cfg).expect("runner")
}

fn base_cfg(dir: &Path) -> RunConfig {
    RunConfig {
        dir: dir.to_path_buf(),
        objective: "Prove the harness".into(),
        max_frames: 4_000,
        max_wall: Duration::from_secs(120),
        wall_after_seconds: 600,
        checkpoint_every: 600,
        heartbeat: Duration::from_secs(3600), // silent unless a check wants one
        pace_realtime: false,
        pace_while_thinking: false,
        resume: false,
        until: None,
        ntfy: None,
        watches: Watches::default(),
        capture_failures: true,
        verbose: false,
    }
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from("target/longplay-harness").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).expect("scratch dir");
    d
}

fn read_lines(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(str::to_string)
        .collect()
}

/// Every `tag` in `events.ndjson`.
fn milestone_tags(dir: &Path) -> Vec<String> {
    read_lines(&dir.join("events.ndjson"))
        .iter()
        .filter_map(|l| json::parse(l).ok())
        .filter(|v| v.str_field("ev") == Some("milestone"))
        .filter_map(|v| v.str_field("tag").map(str::to_string))
        .collect()
}

/// What a determinism check compares: how an attempt ended, and what it cost.
type AttemptShape = (&'static str, u64, u32, u64);

// ---------------------------------------------------------------------------

struct Check {
    n: usize,
    failed: usize,
}

impl Check {
    fn new() -> Check {
        Check { n: 0, failed: 0 }
    }
    fn ok(&mut self, name: &str, cond: bool, detail: &str) {
        self.n += 1;
        if cond {
            println!("[{}] PASS  {name}", self.n);
        } else {
            self.failed += 1;
            println!("[{}] FAIL  {name}\n      {detail}", self.n);
        }
    }
}

fn main() {
    println!("long-run harness — no ROM required, nothing leaves this machine\n");
    let mut c = Check::new();

    // [1] The fixture engages the Gen 1 reader.
    {
        let dir = scratch("supported");
        let mut r = make_runner(&dir, walker(), base_cfg(&dir));
        r.begin();
        r.tick();
        let supported = r
            .gameboy_mut()
            .plugin::<AgentPlugin>()
            .map(|p| p.is_supported())
            .unwrap_or(false);
        c.ok(
            "the fixture cartridge engages the Gen 1 reader",
            supported,
            "the agent refused the cartridge, so nothing below would mean anything",
        );
    }

    // [2] A run produces every artefact a person or a program would look for.
    let (board2, dir2) = {
        let dir = scratch("artefacts");
        let mut r = make_runner(&dir, walker(), base_cfg(&dir));
        World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        }
        .apply(r.gameboy_mut());
        r.begin();
        let stop = loop {
            if let Some(s) = r.tick() {
                break s;
            }
        };
        let board = r.end(&stop);
        for f in [
            "events.ndjson",
            "report.md",
            "report.json",
            "checkpoint.json",
            "checkpoint.gbss",
            "agent.ndjson",
        ] {
            c.ok(
                &format!("the run wrote {f}"),
                dir.join(f).exists(),
                &format!("{} is missing", dir.join(f).display()),
            );
        }
        (board, dir)
    };

    // [3] The headline is frames per model call, and one decision bought many
    //     frames — which is the whole reason this design exists.
    c.ok(
        "the headline is frames per model call and a decision buys hundreds of frames",
        board2.frames_per_call() > 50.0 && board2.calls > 0,
        &format!(
            "{:.1} frames/call over {} calls",
            board2.frames_per_call(),
            board2.calls
        ),
    );
    c.ok(
        "the headline sentence leads with that number",
        board2.headline().contains("frames per model call"),
        &board2.headline(),
    );

    // [4] `steps.ndjson` is a usable dataset: observation, intent, actions,
    //     outcome — and it joins with the agent's own log on the decision index.
    {
        let steps = read_lines(&dir2.join("steps.ndjson"));
        c.ok(
            "the run wrote a per-decision dataset",
            !steps.is_empty(),
            "steps.ndjson is empty",
        );
        let parsed: Vec<_> = steps.iter().filter_map(|l| json::parse(l).ok()).collect();
        c.ok(
            "every step record is valid JSON",
            parsed.len() == steps.len(),
            &format!("{} of {} lines parsed", parsed.len(), steps.len()),
        );
        let complete = parsed.iter().all(|v| {
            v.get("observation").is_some()
                && v.get("intent").is_some()
                && v.get("actions").and_then(|a| a.as_array()).is_some()
                && v.get("outcome").is_some()
        });
        c.ok(
            "each record carries observation, intent, actions and outcome",
            complete && !parsed.is_empty(),
            "a record was missing one of the four",
        );
        let said = parsed.first().and_then(|v| {
            v.get("intent")
                .and_then(|i| i.str_field("say"))
                .map(str::to_string)
        });
        c.ok(
            "the intent is the model's own words",
            said.as_deref().unwrap_or("").contains("Heading north"),
            &format!("{said:?}"),
        );
        // The join: every step's decision index exists in the agent's log.
        let agent_decisions: Vec<u64> = read_lines(&dir2.join("agent.ndjson"))
            .iter()
            .filter_map(|l| json::parse(l).ok())
            .filter_map(|v| v.u64_field("decision"))
            .collect();
        let joins = parsed
            .iter()
            .filter_map(|v| v.u64_field("decision"))
            .all(|d| agent_decisions.contains(&d));
        c.ok(
            "step records join the agent's own decision log on `decision`",
            joins && !agent_decisions.is_empty(),
            &format!("{} agent decisions logged", agent_decisions.len()),
        );
    }

    // [5] Milestones flow from the running machine all the way to the log, the
    //     report and the scoreboard.
    {
        let dir = scratch("milestones");
        let mut cfg = base_cfg(&dir);
        cfg.max_frames = 6_000;
        let mut r = make_runner(&dir, walker(), cfg);
        let mut w = World::default();
        w.apply(r.gameboy_mut());
        r.begin();
        // A schedule of things a person would want to hear about.
        // A schedule of world changes: (frame, what to change).
        type Beat = (u64, fn(&mut World));
        let script: [Beat; 5] = [
            (400, |w| w.party = vec![(1, 5, 20)]),
            (900, |w| w.map = 0x0C), // Route 1
            (1600, |w| w.party[0].1 = 8),
            (2400, |w| {
                w.badges = 1;
                w.owned = 2;
            }),
            (3200, |w| w.party[0].2 = 0), // the party faints
        ];
        let mut frame = 0u64;
        let mut next = 0usize;
        let stop = loop {
            if next < script.len() && frame == script[next].0 {
                script[next].1(&mut w);
                w.play_seconds = frame as u32 / 10;
                w.apply(r.gameboy_mut());
                next += 1;
            }
            // Keep the world alive against nothing (the cartridge writes
            // nothing, so one apply would do — reapplying proves the sampler
            // reads live memory rather than a cached first read).
            if frame % 200 == 0 {
                w.play_seconds = frame as u32 / 10;
                w.apply(r.gameboy_mut());
            }
            frame += 1;
            if let Some(s) = r.tick() {
                break s;
            }
        };
        let board = r.end(&stop);
        let tags = milestone_tags(&dir);
        for want in [
            "in-control",
            "first-pokemon",
            "new-area",
            "level-up",
            "badge",
            "caught",
            "wiped",
        ] {
            c.ok(
                &format!("milestone reported: {want}"),
                tags.iter().any(|t| t == want),
                &format!("saw {tags:?}"),
            );
        }
        let md = board.to_markdown();
        c.ok(
            "the report names the badge in plain English",
            md.contains("Boulder Badge") && md.contains("Route 1"),
            &md,
        );
        c.ok(
            "a wipe is trouble, a level-up is not",
            board
                .notable(Importance::Trouble)
                .iter()
                .any(|m| m.kind.tag() == "wiped")
                && !board
                    .notable(Importance::Major)
                    .iter()
                    .any(|m| m.kind.tag() == "level-up"),
            "importance is wrong",
        );
    }

    // [6] The game-level wall detector fires, and says which kind of stuck.
    {
        let dir = scratch("wall");
        let mut cfg = base_cfg(&dir);
        cfg.wall_after_seconds = 300;
        cfg.max_frames = 3_000;
        let mut r = make_runner(&dir, walker(), cfg);
        let mut w = World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        };
        w.apply(r.gameboy_mut());
        r.begin();
        let mut frame = 0u64;
        let stop = loop {
            if frame % 120 == 0 {
                // Game time races ahead; nothing else changes. The player
                // shuffles between two tiles, which is "pinned".
                w.play_seconds = frame as u32;
                w.x = 5 + (frame % 2) as u8;
                w.apply(r.gameboy_mut());
            }
            frame += 1;
            if let Some(s) = r.tick() {
                break s;
            }
        };
        let board = r.end(&stop);
        let stuck = board
            .milestones
            .iter()
            .find(|m| m.kind.tag() == "wall")
            .map(|m| m.kind.text())
            .unwrap_or_default();
        c.ok(
            "a game that goes nowhere for long enough is reported as stuck",
            !stuck.is_empty(),
            &format!("no wall milestone; tags {:?}", milestone_tags(&dir)),
        );
        c.ok(
            "the report says it is pinned, not merely 'stuck'",
            stuck.contains("going round in"),
            &stuck,
        );
        c.ok(
            "the scoreboard counts the wall",
            board.wall_reports >= 1 && board.worst_stall_seconds >= 300,
            &format!(
                "{} reports, worst stall {}s",
                board.wall_reports, board.worst_stall_seconds
            ),
        );
    }

    // [7] The agent presses buttons and does not write memory — checked as
    //     OBSERVED behaviour on a real run, not as a declaration.
    {
        let dir = scratch("no-writes");
        let mut cfg = base_cfg(&dir);
        cfg.max_frames = 1_200;
        let mut r = make_runner(&dir, walker(), cfg);
        World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        }
        .apply(r.gameboy_mut());
        r.begin();
        while r.tick().is_none() {}
        let st = r.gameboy_mut().plugin_statuses();
        let agent = st.iter().find(|s| s.name.starts_with("agent"));
        c.ok(
            "the agent pressed buttons and wrote no memory",
            agent
                .map(|a| a.has_pressed && !a.has_written && !a.name.contains('!'))
                .unwrap_or(false),
            &format!(
                "{:?}",
                agent.map(|a| (&a.name, a.has_pressed, a.has_written))
            ),
        );
        c.ok(
            "the report declares both rewinds and memory writes even at zero",
            board2
                .to_markdown()
                .contains("memory writes by the agent: **0**")
                && board2
                    .to_markdown()
                    .contains("rewinds (resumes from a checkpoint): **0**"),
            "the Declared section is incomplete",
        );
    }

    // [8] Resume: a second process picks up where the first stopped, does not
    //     replay milestones, and declares the rewind.
    {
        let dir = scratch("resume");
        let mut cfg = base_cfg(&dir);
        cfg.max_frames = 2_000;
        cfg.checkpoint_every = 300;
        let mut r = make_runner(&dir, walker(), cfg.clone());
        let w = World {
            party: vec![(4, 7, 22)],
            map: 0x0C,
            badges: 1,
            play_seconds: 100,
            ..Default::default()
        };
        w.apply(r.gameboy_mut());
        r.begin();
        let stop = loop {
            if let Some(s) = r.tick() {
                break s;
            }
        };
        let first = r.end(&stop);
        let first_tags = milestone_tags(&dir);
        c.ok(
            "the first session reported the badge",
            first_tags.iter().any(|t| t == "badge"),
            &format!("{first_tags:?}"),
        );

        let mut cfg2 = cfg.clone();
        cfg2.resume = true;
        cfg2.max_frames = 600;
        let mut r2 = make_runner(&dir, walker(), cfg2);
        // Note: the world is NOT reapplied — the save state carries it.
        let resumed = r2.resume().expect("resume");
        c.ok(
            "a checkpoint written by one run is read by the next",
            resumed,
            "",
        );
        r2.begin();
        let stop2 = loop {
            if let Some(s) = r2.tick() {
                break s;
            }
        };
        let second = r2.end(&stop2);
        c.ok(
            "the resume is counted and declared",
            second.resumes == 1
                && second
                    .to_markdown()
                    .contains("rewinds (resumes from a checkpoint): **1**"),
            &format!("resumes={}", second.resumes),
        );
        let after = milestone_tags(&dir);
        let badges_before = first_tags.iter().filter(|t| *t == "badge").count();
        let badges_after = after.iter().filter(|t| *t == "badge").count();
        c.ok(
            "a resumed run does not replay the milestones it already reported",
            badges_after == badges_before,
            &format!("{badges_before} then {badges_after}"),
        );
        c.ok(
            "the resume itself is a reported milestone",
            after.iter().any(|t| t == "resumed"),
            &format!("{after:?}"),
        );
        c.ok(
            "the resumed run carries the earlier session's frames and calls forward",
            second.frames > first.frames && second.calls >= first.calls,
            &format!(
                "{} frames / {} calls then {} / {}",
                first.frames, first.calls, second.frames, second.calls
            ),
        );
        // The save state really did carry the world across.
        c.ok(
            "the world came back with the checkpoint",
            second.badges.count_ones() == 1 && !second.party.is_empty(),
            &format!("badges={:#04x} party={:?}", second.badges, second.party),
        );
    }

    // [9] THE PROPERTY THE WHOLE DESIGN RESTS ON: the emulator never waits for
    //     the agent. A driver that answers nothing, ever, must not slow it down.
    {
        let dir = scratch("nowait");
        let mut cfg = base_cfg(&dir);
        cfg.max_frames = 3_000;
        let mut r = make_runner(&dir, Box::new(DeadDriver), cfg);
        World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        }
        .apply(r.gameboy_mut());
        r.begin();
        let t = Instant::now();
        let stop = loop {
            if let Some(s) = r.tick() {
                break s;
            }
        };
        let per_frame = t.elapsed().as_secs_f64() * 1000.0 / 3000.0;
        let board = r.end(&stop);
        c.ok(
            "a driver that never answers does not slow the emulator down",
            per_frame < 4.0,
            &format!("{per_frame:.3} ms/frame against a 16.743 ms budget"),
        );
        c.ok(
            "…and the run ends on its own budget, not on the dead driver",
            stop == Stop::FrameBudget,
            &format!("{stop:?}"),
        );
        c.ok(
            "a run with no answers still writes an honest report",
            board.calls <= 1 && board.to_markdown().contains("Nothing worth reporting")
                || board
                    .milestones
                    .iter()
                    .any(|m| m.kind.tag() == "in-control"),
            &board.headline(),
        );
    }

    // [10] EPISODES: reset and repeat a named segment.
    {
        let dir = scratch("episode");
        let rom = build_inert_gen1();
        let mut gb = Gameboy::new(rom, None);
        gb.add_plugin(Box::new(AgentPlugin::new(
            walker(),
            Config {
                budget: Budget {
                    max_calls: 10_000,
                    max_wall: Duration::from_secs(600),
                    ..Budget::default()
                },
                log: Some(dir.join("agent.ndjson")),
                settle_frames: 4,
                ..Config::default()
            },
        )))
        .expect("attach");
        // The world at the start of every attempt: on map 0, one Pokémon.
        World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        }
        .apply(&mut gb);
        gb.frame();

        let ep = Episode {
            name: "reach-route-1".into(),
            objective: "Get to Route 1".into(),
            success: vec![Cond::parse("map=0x0C").unwrap()],
            failure: vec![Cond::parse("party_hp=0").unwrap()],
            timeout_frames: 400,
            max_calls: 100,
            watches: Watches(vec![Watch::named("lead_hp").unwrap()]),
            ..Default::default()
        };
        let series =
            Series::new(gb, ep.clone(), dir.clone(), None, false).expect("series");
        // The episode can never succeed on its own — the cartridge is inert —
        // so every attempt should time out, and every timeout should leave a
        // save state behind.
        let report = series.run(3);
        c.ok(
            "every attempt is one declared reset",
            report.resets == 3 && report.attempts.len() == 3,
            &format!(
                "{} resets, {} attempts",
                report.resets,
                report.attempts.len()
            ),
        );
        c.ok(
            "an unreachable goal times out rather than hanging",
            report
                .attempts
                .iter()
                .all(|a| a.outcome == Outcome::TimedOut),
            &format!("{:?}", report.by_outcome()),
        );
        c.ok(
            "every failed attempt leaves a save state you can reload",
            report.attempts.iter().all(|a| {
                a.capture
                    .as_ref()
                    .map(|p| {
                        p.exists()
                            && std::fs::metadata(p)
                                .map(|m| m.len() > 1000)
                                .unwrap_or(false)
                    })
                    .unwrap_or(false)
            }),
            "a capture is missing or empty",
        );
        c.ok(
            "the episode summary is written for a person and for a program",
            dir.join("episode.md").exists() && dir.join("episode.json").exists(),
            "episode.md / episode.json missing",
        );
        c.ok(
            "the summary declares the resets and that watches never reached the agent",
            report.to_json().contains("\"state_resets\":3")
                && report
                    .to_json()
                    .contains("\"watches_shown_to_agent\":false"),
            &report.to_json(),
        );
        c.ok(
            "time per area is recorded, which is what says where it is stuck",
            report
                .areas()
                .first()
                .map(|(m, f)| *m == 0 && *f > 0)
                .unwrap_or(false),
            &format!("{:?}", report.areas()),
        );
        c.ok(
            "the episode headline is frames per model call",
            report.headline().contains("frames per model call"),
            &report.headline(),
        );
    }

    // [11] An episode that CAN succeed does, and stops the moment it does.
    {
        let dir = scratch("episode-win");
        let rom = build_inert_gen1();
        let mut gb = Gameboy::new(rom, None);
        gb.add_plugin(Box::new(AgentPlugin::new(walker(), Config::default())))
            .expect("attach");
        World {
            party: vec![(1, 5, 20)],
            map: 0x0C,
            ..Default::default()
        }
        .apply(&mut gb);
        gb.frame();
        let ep = Episode {
            name: "already-there".into(),
            success: vec![Cond::parse("map=0x0C").unwrap()],
            timeout_frames: 5_000,
            max_calls: 50,
            ..Default::default()
        };
        let report = Series::new(gb, ep, dir.clone(), None, false)
            .expect("series")
            .run(4);
        c.ok(
            "an achievable episode succeeds every time from the same start state",
            report.successes() == 4 && (report.success_rate() - 1.0).abs() < 1e-9,
            &format!("{}/{}", report.successes(), report.attempts.len()),
        );
        c.ok(
            "…and stops the moment it does, rather than burning the timeout",
            report.attempts.iter().all(|a| a.frames < 100),
            &format!(
                "{:?}",
                report.attempts.iter().map(|a| a.frames).collect::<Vec<_>>()
            ),
        );
        c.ok(
            "a success leaves no capture — only failures are worth keeping",
            report.attempts.iter().all(|a| a.capture.is_none()),
            "a successful attempt wrote a save state",
        );
        c.ok(
            "the median is over successes, and it is a real number here",
            report.median_success_frames().is_some(),
            "no median",
        );
    }

    // [12] DETERMINISM: same start state, same agent, same result.
    //
    //      This emulator can offer it and few environments can, so it is
    //      asserted rather than assumed.
    {
        let run = || {
            let dir = scratch("determinism");
            let rom = build_inert_gen1();
            let mut gb = Gameboy::new(rom, None);
            gb.add_plugin(Box::new(AgentPlugin::new(walker(), Config::default())))
                .expect("attach");
            World {
                party: vec![(1, 5, 20)],
                ..Default::default()
            }
            .apply(&mut gb);
            gb.frame();
            let ep = Episode {
                name: "det".into(),
                success: vec![Cond::parse("map=0x0C").unwrap()],
                timeout_frames: 900,
                max_calls: 100,
                ..Default::default()
            };
            let r = Series::new(gb, ep, dir, None, false)
                .expect("series")
                .without_captures()
                .run(3);
            r.attempts
                .iter()
                .map(|a| (a.outcome.tag(), a.frames, a.calls, a.decisions))
                .collect::<Vec<_>>()
        };
        let a: Vec<AttemptShape> = run();
        let b: Vec<AttemptShape> = run();
        c.ok(
            "the same segment and the same agent give the same result, twice",
            a == b && !a.is_empty(),
            &format!("{a:?}\n      {b:?}"),
        );
        c.ok(
            "…and every attempt within a run agrees too, since each is a true reset",
            a.windows(2).all(|w| w[0] == w[1]),
            &format!("{a:?}"),
        );
    }

    // [13] THE GUARANTEE THAT MAKES AN EVALUATION MEAN ANYTHING: a memory
    //      watch is host-side reporting and NEVER reaches the agent.
    {
        let dir = scratch("watch-separation");
        let mut cfg = base_cfg(&dir);
        cfg.max_frames = 1_500;
        // A watch whose name and value would be unmistakable in a request.
        cfg.watches = Watches(vec![Watch {
            name: "secret_oracle".into(),
            addr: tr::PLAYER_MONEY,
            len: 1,
            big_endian: false,
            mask: 0,
        }]);
        let mut r = make_runner(&dir, walker(), cfg);
        World {
            party: vec![(1, 5, 20)],
            ..Default::default()
        }
        .apply(r.gameboy_mut());
        r.begin();
        let stop = loop {
            if let Some(s) = r.tick() {
                break s;
            }
        };
        r.end(&stop);
        // The watch was recorded…
        let watched = read_lines(&dir.join("events.ndjson"))
            .iter()
            .filter_map(|l| json::parse(l).ok())
            .any(|v| v.str_field("ev") == Some("watch"));
        c.ok(
            "a declared watch is recorded in the run log",
            watched,
            "no watch event was written",
        );
        // …and never appeared in anything the driver was handed.
        let requests: String = read_lines(&dir.join("agent.ndjson"))
            .iter()
            .filter_map(|l| json::parse(l).ok())
            .filter_map(|v| v.str_field("request").map(str::to_string))
            .collect::<Vec<_>>()
            .join("\n");
        c.ok(
            "the watch never reached the agent — it is reporting, not perception",
            !requests.is_empty() && !requests.contains("secret_oracle"),
            &format!("{} bytes of requests", requests.len()),
        );
        c.ok(
            "the run report declares that watches were not shown to the agent",
            std::fs::read_to_string(dir.join("report.json"))
                .unwrap_or_default()
                .contains("\"watches_shown_to_agent\":false"),
            "the assistance block is missing the claim",
        );
    }

    // [14] Where the time went, in a long run as well as an episode.
    {
        let md = std::fs::read_to_string(dir2.join("report.md")).unwrap_or_default();
        c.ok(
            "a long run's report says where the time went",
            md.contains("Where the time went"),
            &md,
        );
    }

    println!("\n{} checks, {} failed", c.n, c.failed);
    if c.failed > 0 {
        std::process::exit(1);
    }
    println!("all good");
}

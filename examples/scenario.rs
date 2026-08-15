//! **The scenario front end** — write a goal and its rules in a small JSON
//! file, play it, record what happened, replay the recording for nothing.
//!
//! ```text
//! cargo run --release --example scenario -- check    scenarios/leave-the-house.json
//! cargo run --release --example scenario -- run      --rom <rom> --scenario <s> [--driver …]
//! cargo run --release --example scenario -- replay   --rom <rom> --scenario <s> --recording <r>
//! cargo run --release --example scenario -- knowledge --search badges
//! cargo run --release --example scenario -- guidance  <guidance.json>
//! ```
//!
//! # Two of the five subcommands cost nothing and need no cartridge
//!
//! `check` validates a scenario and prints its **cost ceiling before anything
//! is spent** — the captain's money, so the number is on screen first rather
//! than discoverable afterwards from a bill. `knowledge` answers a question out
//! of the vendored AtlasGB snapshot. Neither loads a ROM.
//!
//! # `replay` makes no model calls at all
//!
//! A recording is the driver's replies in order; a
//! [`terminalgb_plugin_agent::ScriptedDriver`] answers from that list through
//! the identical parse-clamp-execute path the live run took. So a replay is not
//! an approximation of the run, and it goes at the emulator's own speed rather
//! than the network's. What the harness measures is printed at the end.
//!
//! # The ROM is staged, always
//!
//! The MBC writes a `.sav` beside the cartridge, so a run pointed at somebody's
//! ROM directory would write into it. Everything here copies the cartridge into
//! the output directory first — the standing rule in `AGENTS.md`.

use std::path::{Path, PathBuf};
use std::process::exit;

use terminalgb::gameboy::Gameboy;
use terminalgb::scenario::knowledge::{Atlas, Guidance};
use terminalgb::scenario::record::Recording;
use terminalgb::scenario::run::ScenarioRun;
use terminalgb::scenario::Scenario;
use terminalgb_plugin_agent::driver::{Driver, RandomDriver, ScriptedDriver};
use terminalgb_plugin_agent::governor::Budget;
use terminalgb_plugin_agent::subprocess::CommandDriver;
use terminalgb_plugin_agent::{AgentPlugin, Config};

const USAGE: &str = "\
scenario — a goal, rules the host enforces, and a run you can replay

  check <scenario.json>
      Validate it and print the cost ceiling. No cartridge, no cost.

  knowledge [--symbol NAME | --at 0xNNNN | --search TEXT] [--limit N]
      Ask the vendored AtlasGB snapshot. No cartridge, no cost.

  guidance <guidance.json>
      Print the map graph earlier runs walked.

  run --rom R --scenario S [options]
      --start FILE        save state every attempt begins from
      --driver KIND       scripted | random | command   (default scripted)
      --script FILE       one JSON reply per line, for --driver scripted
      --seed N            for --driver random
      --command CMD ...   the adapter, for --driver command (MUST BE LAST)
      --out DIR           where the recording and captures go (default
                          target/scenario/<name>)
      --pace-thinking     run at real time while a model call is in flight.
                          Turn this ON for anything with a live model in it.
      --repeat N          play it N times and report the success rate

  replay --rom R --scenario S --recording FILE [--start FILE] [--out DIR]
      Replay a recording. Makes NO model calls.
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        println!("{USAGE}");
        exit(2);
    }
    match args[0].as_str() {
        "check" => cmd_check(&args[1..]),
        "knowledge" => cmd_knowledge(&args[1..]),
        "guidance" => cmd_guidance(&args[1..]),
        "run" => cmd_run(&args[1..], false),
        "replay" => cmd_run(&args[1..], true),
        "-h" | "--help" | "help" => println!("{USAGE}"),
        other => {
            eprintln!("unknown subcommand {other:?}\n\n{USAGE}");
            exit(2);
        }
    }
}

// -- check --------------------------------------------------------------------

fn cmd_check(args: &[String]) {
    if args.is_empty() {
        eprintln!("check needs a scenario file");
        exit(2);
    }
    let path = PathBuf::from(&args[0]);
    match Scenario::load(&path) {
        Ok(s) => {
            for line in s.briefing(1) {
                println!("{line}");
            }
            let k = s.knowledge_from(&path);
            for src in &k.sources {
                println!("  knowledge available: {src}");
            }
            for m in &k.missing {
                println!("  knowledge MISSING:   {m}");
            }
            // Say what the agent will actually be told, because a rule stated
            // only to the host is a rule the agent is judged on without ever
            // having heard of it.
            println!("\nthe agent will be told:");
            for note in s.referee().instructions() {
                println!("  - {note}");
            }
            println!("\nOK");
        }
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            exit(1);
        }
    }
}

// -- knowledge ----------------------------------------------------------------

fn cmd_knowledge(args: &[String]) {
    let Some(atlas) = Atlas::load() else {
        eprintln!(
            "no atlas found — expected {} beneath the repository root, or \
             $GB_ATLAS pointing at one",
            Atlas::VENDORED
        );
        exit(1);
    };
    println!(
        "{} addresses from {}",
        atlas.len(),
        atlas
            .source()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );
    let mut limit = 12usize;
    let mut i = 0;
    let mut asked = false;
    while i < args.len() {
        match args[i].as_str() {
            "--limit" => {
                limit = args.get(i + 1).and_then(|v| v.parse().ok()).unwrap_or(12);
                i += 2;
            }
            "--symbol" => {
                asked = true;
                match args.get(i + 1).and_then(|n| atlas.symbol(n)) {
                    Some(e) => println!("{}", e.summary()),
                    None => println!("no such symbol"),
                }
                i += 2;
            }
            "--at" => {
                asked = true;
                let a = args
                    .get(i + 1)
                    .and_then(|v| {
                        let t = v.trim_start_matches("0x").trim_start_matches('$');
                        u16::from_str_radix(t, 16).ok()
                    })
                    .unwrap_or(0);
                match atlas.at(a) {
                    Some(e) => println!("${a:04X} is inside {}", e.summary()),
                    None => println!("${a:04X} is not in the atlas"),
                }
                i += 2;
            }
            "--search" => {
                asked = true;
                let needle = args.get(i + 1).cloned().unwrap_or_default();
                for e in atlas.search(&needle, limit) {
                    println!("{}", e.summary());
                }
                i += 2;
            }
            other => {
                eprintln!("unknown option {other:?}");
                exit(2);
            }
        }
    }
    if !asked {
        println!("(ask with --symbol, --at or --search)");
    }
}

// -- guidance -----------------------------------------------------------------

fn cmd_guidance(args: &[String]) {
    if args.is_empty() {
        eprintln!("guidance needs a file");
        exit(2);
    }
    match Guidance::load(Path::new(&args[0])) {
        Ok(g) => {
            println!("guidance for {:?}", g.rom_title);
            if g.links.is_empty() {
                println!("  no doors walked yet — nothing has run this scenario");
            }
            for l in &g.links {
                println!(
                    "  {} ({}) --({},{})--> {} ({})  ×{}",
                    terminalgb::longplay::map_name(l.from),
                    l.from,
                    l.x,
                    l.y,
                    terminalgb::longplay::map_name(l.to),
                    l.to,
                    l.uses
                );
            }
            for (map, notes) in &g.notes {
                for n in notes {
                    println!("  note on {}: {n}", terminalgb::longplay::map_name(*map));
                }
            }
            for n in &g.general {
                println!("  note: {n}");
            }
        }
        Err(e) => {
            eprintln!("{e}");
            exit(1);
        }
    }
}

// -- run / replay -------------------------------------------------------------

#[derive(Default)]
struct RunArgs {
    rom: Option<PathBuf>,
    scenario: Option<PathBuf>,
    start: Option<PathBuf>,
    recording: Option<PathBuf>,
    out: Option<PathBuf>,
    driver: String,
    script: Option<PathBuf>,
    seed: u64,
    command: Vec<String>,
    pace_thinking: bool,
    repeat: u32,
}

fn parse_run_args(args: &[String]) -> RunArgs {
    let mut a = RunArgs {
        driver: "scripted".into(),
        seed: 1,
        repeat: 1,
        ..Default::default()
    };
    let mut i = 0;
    while i < args.len() {
        let next = |i: usize| -> String {
            args.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("{} needs a value", args[i]);
                exit(2);
            })
        };
        match args[i].as_str() {
            "--rom" => {
                a.rom = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--scenario" => {
                a.scenario = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--start" => {
                a.start = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--recording" => {
                a.recording = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--out" => {
                a.out = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--driver" => {
                a.driver = next(i);
                i += 2;
            }
            "--script" => {
                a.script = Some(PathBuf::from(next(i)));
                i += 2;
            }
            "--seed" => {
                a.seed = next(i).parse().unwrap_or(1);
                i += 2;
            }
            "--repeat" => {
                a.repeat = next(i).parse().unwrap_or(1).max(1);
                i += 2;
            }
            "--pace-thinking" => {
                a.pace_thinking = true;
                i += 1;
            }
            // Swallows the rest, like `examples/claude_plays.rs` — so it must
            // come last, and saying so in the usage is the whole defence.
            "--command" => {
                a.command = args[i + 1..].to_vec();
                break;
            }
            other => {
                eprintln!("unknown option {other:?}\n\n{USAGE}");
                exit(2);
            }
        }
    }
    a
}

/// Stage the cartridge into the output directory. **Never run against a ROM
/// where it lives** — the MBC writes a `.sav` beside it.
fn stage(rom: &Path, out: &Path) -> PathBuf {
    std::fs::create_dir_all(out).expect("create output directory");
    let dest = out.join(rom.file_name().unwrap_or_else(|| "rom.gb".as_ref()));
    let bytes = std::fs::read(rom).unwrap_or_else(|e| {
        eprintln!("cannot read {}: {e}", rom.display());
        exit(1);
    });
    std::fs::write(&dest, &bytes).expect("stage the cartridge");
    dest
}

fn build_driver(a: &RunArgs, replay: Option<Vec<String>>) -> Box<dyn Driver> {
    if let Some(replies) = replay {
        return Box::new(ScriptedDriver::new(replies));
    }
    match a.driver.as_str() {
        "scripted" => {
            let replies = match &a.script {
                Some(p) => std::fs::read_to_string(p)
                    .unwrap_or_else(|e| {
                        eprintln!("cannot read {}: {e}", p.display());
                        exit(1);
                    })
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(str::to_string)
                    .collect(),
                None => {
                    eprintln!("--driver scripted needs --script FILE");
                    exit(2);
                }
            };
            Box::new(ScriptedDriver::new(replies))
        }
        "random" => Box::new(RandomDriver::new(a.seed)),
        "command" => {
            if a.command.is_empty() {
                eprintln!("--driver command needs --command CMD ... (last)");
                exit(2);
            }
            let (prog, rest) = a.command.split_first().unwrap();
            Box::new(CommandDriver::new(prog, rest.to_vec()))
        }
        other => {
            eprintln!("unknown driver {other:?} (scripted|random|command)");
            exit(2);
        }
    }
}

fn cmd_run(args: &[String], replay: bool) {
    let a = parse_run_args(args);
    let (Some(rom), Some(scenario_path)) = (a.rom.clone(), a.scenario.clone()) else {
        eprintln!("--rom and --scenario are both required\n\n{USAGE}");
        exit(2);
    };
    let s = Scenario::load(&scenario_path).unwrap_or_else(|e| {
        eprintln!("{}: {e}", scenario_path.display());
        exit(1);
    });
    let out = a
        .out
        .clone()
        .unwrap_or_else(|| PathBuf::from("target/scenario").join(&s.name));

    let recorded = if replay {
        let Some(p) = &a.recording else {
            eprintln!("replay needs --recording FILE");
            exit(2);
        };
        Some(Recording::load(p).unwrap_or_else(|e| {
            eprintln!("{e}");
            exit(1);
        }))
    } else {
        None
    };

    // The start state: the scenario's own, or one named on the command line.
    let start_path = a
        .start
        .clone()
        .or_else(|| s.start_state_from(&scenario_path));
    let start = start_path.as_ref().map(|p| {
        std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("cannot read start state {}: {e}", p.display());
            exit(1);
        })
    });

    // Everything with a cost, before anything is spent.
    for line in s.briefing(a.repeat) {
        println!("{line}");
    }
    if let Some(r) = &recorded {
        println!(
            "replaying {} decisions recorded on {:?} — 0 model calls",
            r.decisions.len(),
            r.rom_title
        );
    }
    println!();

    let mut goals = 0u32;
    let mut last_recording = None;
    for attempt in 1..=a.repeat {
        let staged = stage(&rom, &out);
        let data = std::fs::read(&staged).expect("read staged cartridge");
        let mut gb = Gameboy::new(data, Some(staged));
        let driver = build_driver(&a, recorded.as_ref().map(|r| r.replies()));
        let cfg = Config {
            budget: Budget {
                max_calls: s.episode().max_calls,
                ..Budget::default()
            },
            objective: s.objective.clone(),
            log: Some(out.join(format!("agent-{attempt}.ndjson"))),
            ..Config::default()
        };
        gb.add_plugin(Box::new(AgentPlugin::new(driver, cfg)))
            .expect("attach the agent");

        let run =
            ScenarioRun::new(gb, s.clone(), &scenario_path, out.clone(), start.clone())
                .unwrap_or_else(|e| {
                    eprintln!("{e}");
                    exit(1);
                })
                .pacing_while_thinking(a.pace_thinking);

        if let Some(r) = &recorded {
            match run.replay(r) {
                Ok(report) => {
                    for line in report.lines() {
                        println!("  {line}");
                    }
                    let path = out.join("replay.json");
                    let _ = std::fs::write(&path, report.to_json());
                    println!("  written to {}", path.display());
                    println!(
                        "\n{}",
                        if report.faithful() {
                            "FAITHFUL — the recording reproduced exactly"
                        } else {
                            "MISMATCH — the recording did NOT reproduce"
                        }
                    );
                    exit(if report.faithful() { 0 } else { 1 });
                }
                Err(e) => {
                    eprintln!("  {e}");
                    exit(1);
                }
            }
        }

        let result = run.play();
        if a.repeat > 1 {
            println!("attempt {attempt}/{}:", a.repeat);
        }
        for line in result.lines() {
            println!("  {line}");
        }
        if result.reached_goal() {
            goals += 1;
        }
        last_recording = Some(result.recording.clone());
        let path = out.join(format!("run-{attempt}.json"));
        if let Err(e) = result.recording.save(&path) {
            eprintln!("  could not write the recording: {e}");
        } else {
            println!("  recorded to {}", path.display());
        }
        println!();
    }

    if a.repeat > 1 {
        println!(
            "{goals} of {} attempts reached the goal ({:.0}%)",
            a.repeat,
            goals as f64 * 100.0 / a.repeat as f64
        );
    }
    // Point at the replay, because a recording nobody replays is a log file.
    if let Some(r) = last_recording {
        println!(
            "\nreplay it with:\n  cargo run --release --example scenario -- replay \\\n\
             \x20   --rom {} --scenario {} --recording {}",
            rom.display(),
            scenario_path.display(),
            out.join(format!("run-{}.json", a.repeat)).display()
        );
        let _ = r;
    }
}

//! **The long-horizon runner's front end** — point it at a cartridge, a goal
//! and a budget, and walk away.
//!
//! ```text
//! # deterministic, free, and what CI runs (a canned script, no model):
//! cargo run --release --features longplay --example longplay -- \
//!     --rom target/blue.gb --dir target/longplay/demo \
//!     --driver scripted --script script.ndjson --loop-script --hours 0.01
//!
//! # a real model, overnight, telling a phone when something happens:
//! cargo run --release --features longplay --example longplay -- \
//!     --rom ~/roms/blue.gb --dir ~/runs/blue-night \
//!     --objective "Leave Pallet Town, get a starter, reach Viridian City" \
//!     --hours 8 --max-usd 2 --max-calls 400 \
//!     --ntfy https://ntfy.sh/my-topic \
//!     --driver command --command tools/claude_plays/claude_api_adapter.py
//!
//! # …and if it dies at hour four, the same command plus --resume picks up
//! # from the last checkpoint. That is a REWIND and the report says so.
//! ```
//!
//! `--command` swallows the rest of the command line, so it must be last; that
//! is checked rather than left as a surprise (`examples/claude_plays.rs` learnt
//! this the expensive way).

use std::path::PathBuf;
use std::time::Duration;

use terminalgb::gameboy::Gameboy;
use terminalgb::longplay::{
    episode::Episode,
    notify,
    progress::Importance,
    series::Series,
    watch::{Watch, Watches},
    RunConfig, Runner,
};
use terminalgb_plugin_agent::{
    driver::{Driver, RandomDriver, ScriptedDriver},
    governor::Budget,
    subprocess::CommandDriver,
    AgentPlugin, Config,
};

struct Args {
    rom: Option<PathBuf>,
    dir: PathBuf,
    objective: String,
    driver: String,
    script: Option<PathBuf>,
    loop_script: bool,
    seed: u64,
    latency: u32,
    command: Vec<String>,
    hours: f64,
    max_frames: u64,
    max_calls: u32,
    max_usd: f64,
    timeout: u64,
    wall_after: u32,
    checkpoint_every: u64,
    heartbeat: u64,
    until: Option<String>,
    ntfy: Option<String>,
    ntfy_token: Option<String>,
    ntfy_min: String,
    resume: bool,
    pace_realtime: bool,
    pace_thinking: bool,
    state_in: Option<PathBuf>,
    frame_capture: Option<PathBuf>,
    quiet: bool,
    dry_run: bool,
    episode: Option<PathBuf>,
    repeat: u32,
    watch: Vec<String>,
    no_captures: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            rom: None,
            dir: PathBuf::from("target/longplay"),
            objective: String::new(),
            driver: "scripted".into(),
            script: None,
            loop_script: false,
            seed: 1,
            latency: 0,
            command: Vec::new(),
            hours: 1.0,
            max_frames: 0,
            // Deliberately modest. An unattended overnight run should annoy
            // you, not bankrupt you — the same call `governor::Budget` makes.
            max_calls: 300,
            max_usd: 2.0,
            timeout: 150,
            wall_after: 20 * 60,
            checkpoint_every: 18_000,
            heartbeat: 300,
            until: None,
            ntfy: None,
            ntfy_token: None,
            ntfy_min: "major".into(),
            resume: false,
            pace_realtime: false,
            pace_thinking: false,
            state_in: None,
            frame_capture: None,
            quiet: false,
            dry_run: false,
            episode: None,
            repeat: 1,
            watch: Vec::new(),
            no_captures: false,
        }
    }
}

const FLAGS: [&str; 30] = [
    "--episode",
    "--repeat",
    "--watch",
    "--no-captures",
    "--rom",
    "--dir",
    "--objective",
    "--driver",
    "--script",
    "--loop-script",
    "--seed",
    "--latency",
    "--command",
    "--hours",
    "--frames",
    "--max-calls",
    "--max-usd",
    "--timeout",
    "--wall-after",
    "--checkpoint-every",
    "--heartbeat",
    "--until",
    "--ntfy",
    "--ntfy-token",
    "--ntfy-min",
    "--resume",
    "--pace",
    "--state",
    "--frame-capture",
    "--dry-run",
];

fn main() {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rom" => a.rom = it.next().map(PathBuf::from),
            "--dir" => a.dir = it.next().map(PathBuf::from).unwrap_or(a.dir),
            "--objective" => a.objective = it.next().unwrap_or_default(),
            "--driver" => a.driver = it.next().unwrap_or_default(),
            "--script" => a.script = it.next().map(PathBuf::from),
            "--loop-script" => a.loop_script = true,
            "--seed" => a.seed = num(it.next(), a.seed),
            "--latency" => a.latency = num(it.next(), a.latency as u64) as u32,
            "--command" => a.command = it.by_ref().collect(),
            "--hours" => {
                a.hours = it.next().and_then(|s| s.parse().ok()).unwrap_or(a.hours)
            }
            "--frames" => a.max_frames = num(it.next(), a.max_frames),
            "--max-calls" => a.max_calls = num(it.next(), a.max_calls as u64) as u32,
            "--max-usd" => {
                a.max_usd = it.next().and_then(|s| s.parse().ok()).unwrap_or(a.max_usd)
            }
            "--timeout" => a.timeout = num(it.next(), a.timeout),
            "--wall-after" => a.wall_after = num(it.next(), a.wall_after as u64) as u32,
            "--checkpoint-every" => {
                a.checkpoint_every = num(it.next(), a.checkpoint_every)
            }
            "--heartbeat" => a.heartbeat = num(it.next(), a.heartbeat),
            "--until" => a.until = it.next(),
            "--ntfy" => a.ntfy = it.next(),
            "--ntfy-token" => a.ntfy_token = it.next(),
            "--ntfy-min" => a.ntfy_min = it.next().unwrap_or(a.ntfy_min),
            "--resume" => a.resume = true,
            "--pace" => match it.next().as_deref() {
                Some("realtime") => a.pace_realtime = true,
                // The interesting one headlessly: full speed while a plan
                // runs, real time while the model thinks, so the world does
                // not run away underneath the decision that is being made.
                Some("thinking") => a.pace_thinking = true,
                Some("max") | None => {}
                Some(other) => {
                    eprintln!("--pace must be max|thinking|realtime, not {other:?}");
                    std::process::exit(2);
                }
            },
            "--state" => a.state_in = it.next().map(PathBuf::from),
            "--frame-capture" => a.frame_capture = it.next().map(PathBuf::from),
            "--quiet" => a.quiet = true,
            "--dry-run" => a.dry_run = true,
            "--episode" => a.episode = it.next().map(PathBuf::from),
            "--repeat" => a.repeat = num(it.next(), a.repeat as u64).max(1) as u32,
            "--watch" => {
                if let Some(v) = it.next() {
                    a.watch
                        .extend(v.split(',').map(str::trim).map(str::to_string));
                }
            }
            "--no-captures" => a.no_captures = true,
            "--help" | "-h" => {
                usage();
                return;
            }
            other => {
                eprintln!("unknown argument {other}");
                usage();
                std::process::exit(2);
            }
        }
    }

    if let Some(stray) = a
        .command
        .iter()
        .skip(1)
        .find(|v| FLAGS.contains(&v.as_str()))
    {
        eprintln!(
            "'{stray}' came after --command, so it was passed to the adapter rather \
             than read as an option. --command must be LAST."
        );
        std::process::exit(2);
    }

    let Some(rom_path) = a.rom.clone() else {
        eprintln!("--rom is required");
        usage();
        std::process::exit(2);
    };

    let ntfy = match &a.ntfy {
        Some(url) => {
            let min = match a.ntfy_min.as_str() {
                "minor" | "all" => Importance::Minor,
                "major" => Importance::Major,
                "trouble" | "problems" => Importance::Trouble,
                other => {
                    eprintln!("--ntfy-min must be minor|major|trouble, not {other:?}");
                    std::process::exit(2);
                }
            };
            match notify::NtfyConfig::parse(url, a.ntfy_token.clone(), min) {
                Ok(c) => Some(c),
                Err(e) => {
                    eprintln!("--ntfy: {e}");
                    std::process::exit(2);
                }
            }
        }
        None => None,
    };

    // The MBC writes a `.sav` beside the ROM, so a run must never touch the
    // player's own library. Stage a copy into the run directory, where its
    // save belongs to the run.
    if let Err(e) = std::fs::create_dir_all(&a.dir) {
        eprintln!("cannot create {}: {e}", a.dir.display());
        std::process::exit(1);
    }
    let staged = a.dir.join(
        rom_path
            .file_name()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("rom.gb")),
    );
    if staged != rom_path && !(a.resume && staged.exists()) {
        if let Err(e) = std::fs::copy(&rom_path, &staged) {
            eprintln!("cannot stage the ROM: {e}");
            std::process::exit(1);
        }
        if !a.resume {
            for ext in ["sav", "gbsave", "srm"] {
                let _ = std::fs::remove_file(staged.with_extension(ext));
            }
        }
    }
    let data = match std::fs::read(&staged) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read the ROM: {e}");
            std::process::exit(1);
        }
    };

    // Load the episode first: it is refused if it could never succeed, and a
    // refusal must come before anything spends anything.
    let episode = match &a.episode {
        Some(p) => match std::fs::read_to_string(p)
            .map_err(|e| format!("cannot read {}: {e}", p.display()))
            .and_then(|t| Episode::from_json(&t))
        {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!("--episode: {e}");
                std::process::exit(2);
            }
        },
        None => None,
    };
    if episode.is_none() && a.repeat > 1 {
        eprintln!("--repeat only means something with --episode");
        std::process::exit(2);
    }

    // Watches named on the command line, on top of any the episode declares.
    let mut watches = Watches(match &episode {
        Some(e) => e.watches.0.clone(),
        None => Vec::new(),
    });
    for name in &a.watch {
        if name.is_empty() {
            continue;
        }
        match Watch::named(name) {
            Some(w) => watches.0.push(w),
            None => {
                eprintln!(
                    "--watch: unknown name {name:?} — known names are {}",
                    terminalgb::longplay::watch::NAMED
                        .iter()
                        .map(|(n, ..)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                std::process::exit(2);
            }
        }
    }

    // -- the budget banner, before anything spends anything -----------------
    let driver_costs = a.driver == "command";
    println!("--- long run ---");
    println!("cartridge   : {}", rom_path.display());
    println!("run dir     : {}", a.dir.display());
    println!("objective   : {}", show(&a.objective));
    println!(
        "driver      : {}{}",
        a.driver,
        if driver_costs {
            "  (COSTS MONEY)"
        } else {
            "  (free)"
        }
    );
    if let Some(e) = &episode {
        {
            println!("episode     : {} x{}", e.name, a.repeat);
            println!(
                "  success   : {}",
                joined(
                    &e.success
                        .iter()
                        .map(|c| c.source.clone())
                        .collect::<Vec<_>>(),
                    " AND "
                )
            );
            println!(
                "  failure   : {}",
                joined(
                    &e.failure
                        .iter()
                        .map(|c| c.source.clone())
                        .collect::<Vec<_>>(),
                    " OR "
                )
            );
            println!(
                "  per try   : {} frames, ≤{} calls  →  WORST CASE {} calls over the series",
                e.timeout_frames,
                e.max_calls,
                a.repeat as u64 * e.max_calls as u64
            );
        }
    }
    println!(
        "budget      : {} calls · ${:.2} · {:.2} h wall · {} s per call",
        a.max_calls, a.max_usd, a.hours, a.timeout
    );
    // Measured, on this cartridge, and worth a warning because it silently
    // makes an episode meaningless: headless the emulator does ~3,500 frames a
    // second, so a 26-second model call is ~90,000 frames. The same episode
    // (12,000-frame timeout, real model) scored 3/3 with `--pace thinking` and
    // 0/3 without — every unpaced attempt timed out before its first decision
    // came back. See docs/longplay.md §9.
    if episode.is_some() && !(a.pace_thinking || a.pace_realtime) && driver_costs {
        let ep = episode.as_ref().unwrap();
        let think_frames = 26.0 * 3_500.0_f64;
        if (ep.timeout_frames as f64) < think_frames * 2.0 {
            println!(
                "NOTE        : this episode's timeout is {} frames, and headless one \n\
                 \x20             model call is roughly {:.0} frames of game running on \n\
                 \x20             while it thinks. Attempts will very likely time out \n\
                 \x20             before their first decision arrives. Add --pace thinking.",
                ep.timeout_frames, think_frames
            );
        }
    }
    if !watches.is_empty() {
        println!(
            "watching    : {}  (reporting only — never shown to the agent)",
            watches
                .0
                .iter()
                .map(|w| w.name.clone())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!(
        "stuck alarm : no progress for {} of game time",
        terminalgb::longplay::progress::human_seconds(a.wall_after)
    );
    println!(
        "checkpoint  : every {} emulated frames  ·  heartbeat every {} s",
        a.checkpoint_every, a.heartbeat
    );
    println!(
        "push        : {}",
        match &ntfy {
            Some(c) => format!("{} (>= {})", c.url, a.ntfy_min),
            None => "off".into(),
        }
    );
    if let Some(u) = &a.until {
        println!("stop when   : {u}");
    }
    if a.resume {
        println!("resume      : YES — this REWINDS the game to the last checkpoint");
    }
    println!("---");
    if a.dry_run {
        println!("--dry-run: nothing was started.");
        return;
    }

    let driver: Box<dyn Driver> = match a.driver.as_str() {
        "scripted" => {
            let Some(p) = &a.script else {
                eprintln!("--driver scripted needs --script <ndjson>");
                std::process::exit(2);
            };
            let replies: Vec<String> = match std::fs::read_to_string(p) {
                Ok(s) => s
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .map(str::to_string)
                    .collect(),
                Err(e) => {
                    eprintln!("cannot read {}: {e}", p.display());
                    std::process::exit(1);
                }
            };
            let d = ScriptedDriver::new(replies).with_latency(a.latency);
            Box::new(if a.loop_script { d.looping() } else { d })
        }
        "random" => Box::new(RandomDriver::new(a.seed)),
        "command" => {
            if a.command.is_empty() {
                eprintln!("--driver command needs --command <program> [args…]");
                std::process::exit(2);
            }
            let (prog, args) = a.command.split_first().unwrap();
            Box::new(CommandDriver::new(prog, args.to_vec()))
        }
        other => {
            eprintln!("unknown driver '{other}' (scripted|random|command)");
            std::process::exit(2);
        }
    };

    let mut gb = Gameboy::new(data, Some(staged.clone()));
    // A machine with no APU is not a quiet Game Boy, it is one with the sound
    // chip pulled out, and a cartridge can tell — see `examples/blackout_probe.rs`.
    gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    if let Some(p) = &a.state_in {
        match std::fs::read(p).map(|b| gb.load_state(&b)) {
            Ok(Ok(())) => println!("loaded save state {}", p.display()),
            Ok(Err(e)) => {
                eprintln!("save state rejected: {e:?}");
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("cannot read the save state: {e}");
                std::process::exit(1);
            }
        }
    }

    let wall = Duration::from_secs_f64((a.hours * 3600.0).max(1.0));
    let cfg = Config {
        budget: Budget {
            max_calls: a.max_calls,
            max_usd: a.max_usd,
            call_timeout: Duration::from_secs(a.timeout),
            // The agent's own wall cap matches the run's, so whichever fires
            // first the reason is the same one.
            max_wall: wall,
            ..Budget::default()
        },
        objective: a.objective.clone(),
        // The agent's own decision log lives beside the run's, and the two join
        // on the decision index.
        log: Some(a.dir.join("agent.ndjson")),
        frame_capture: a.frame_capture.clone(),
        ..Config::default()
    };
    if let Err(e) = gb.add_plugin(Box::new(AgentPlugin::new(driver, cfg))) {
        eprintln!("cannot attach the agent: {e}");
        std::process::exit(1);
    }
    // `is_supported` is only known after the first dispatch.
    gb.frame();
    if let Some(p) = gb.plugin::<AgentPlugin>() {
        if !p.is_supported() {
            eprintln!("{}", p.refusal().unwrap_or_else(|| "unsupported".into()));
            std::process::exit(1);
        }
    }

    // -- episode mode --------------------------------------------------------
    if let Some(ep) = episode {
        let start = match ep.start_state_from(a.episode.as_ref().unwrap()) {
            Some(p) => match std::fs::read(&p) {
                Ok(b) => Some(b),
                Err(e) => {
                    eprintln!(
                        "cannot read the episode's start state {}: {e}",
                        p.display()
                    );
                    std::process::exit(1);
                }
            },
            None => {
                if a.repeat > 1 && a.state_in.is_none() {
                    eprintln!(
                        "this episode has no start_state, so every attempt after the \
                         first would begin wherever the last one ended. Give it a \
                         start_state (or --state) before repeating it."
                    );
                    std::process::exit(2);
                }
                None
            }
        };
        let mut series = match Series::new(gb, ep, a.dir.clone(), start, !a.quiet) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{e}");
                std::process::exit(1);
            }
        };
        if a.no_captures {
            series = series.without_captures();
        }
        series = series.pacing_while_thinking(a.pace_thinking || a.pace_realtime);
        let report = series.run(a.repeat);
        println!("\n{}", report.to_markdown());
        println!("written to {}", a.dir.join("episode.md").display());
        return;
    }

    let run_cfg = RunConfig {
        dir: a.dir.clone(),
        objective: a.objective.clone(),
        max_frames: a.max_frames,
        max_wall: wall,
        wall_after_seconds: a.wall_after,
        checkpoint_every: a.checkpoint_every,
        heartbeat: Duration::from_secs(a.heartbeat.max(1)),
        pace_realtime: a.pace_realtime,
        pace_while_thinking: a.pace_thinking,
        resume: a.resume,
        until: a.until.clone(),
        ntfy,
        watches,
        capture_failures: !a.no_captures,
        verbose: !a.quiet,
    };
    let mut runner = match Runner::new(gb, run_cfg) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if a.resume {
        match runner.resume() {
            Ok(true) => {}
            Ok(false) => {
                println!("no checkpoint in {} — starting fresh", a.dir.display())
            }
            Err(e) => {
                eprintln!("cannot resume: {e}");
                std::process::exit(1);
            }
        }
    }

    let (stop, board) = runner.run();
    println!("\n{}", board.to_markdown());
    println!("written to {}", a.dir.join("report.md").display());
    // A failure to reach the goal is not a failure of the harness. Exit 0
    // unless the emulator itself faulted, which is the only outcome that means
    // "do not trust this run".
    if matches!(stop, terminalgb::longplay::Stop::Fault(_)) {
        std::process::exit(1);
    }
}

/// A condition list, or an honest "there are none" — never the objective
/// placeholder, which would read as if the episode had a failure condition
/// somebody forgot to print.
fn joined(parts: &[String], sep: &str) -> String {
    if parts.is_empty() {
        "(none)".into()
    } else {
        parts.join(sep)
    }
}

fn show(s: &str) -> String {
    if s.trim().is_empty() {
        "(none — the agent will pick its own)".into()
    } else {
        s.trim().into()
    }
}

fn num(s: Option<String>, dflt: u64) -> u64 {
    s.and_then(|v| v.parse().ok()).unwrap_or(dflt)
}

fn usage() {
    eprintln!(
        "usage: longplay --rom <file> [options]

  --dir <dir>                   run directory: log, checkpoint, report
                                (default target/longplay)
  --objective <text>            one sentence, handed to the agent every call
  --hours F                     wall-clock ceiling for this session (default 1)
  --frames N                    emulated-frame ceiling (0 = none)
  --until badges>=1|party>=1|owned>=5|map=0x02
                                stop when this becomes true

  --driver scripted|random|command
  --script <ndjson> [--loop-script] [--latency N]
  --command <prog> [args…]      adapter program; MUST BE LAST

  --max-calls N --max-usd F     budget caps (default 300, \\$2.00)
  --timeout S                   per-call ceiling (default 150)

  --wall-after S                declare a wall after S seconds of GAME time
                                with no progress (default 1200)
  --checkpoint-every N          emulated frames between checkpoints
  --heartbeat S                 seconds between progress lines (default 300)
  --resume                      continue from the checkpoint — A REWIND, and
                                the run report declares it
  --state <file>                start from a save state

  --ntfy <topic-url>            push milestones, e.g. https://ntfy.sh/my-topic
  --ntfy-token <tok>            bearer token for a private server
  --ntfy-min minor|major|trouble   what is worth a buzz (default major)

  --episode <file.json>         run a named segment instead of a long run
  --repeat N                    …that many times, resetting between attempts
  --watch name[,name…]          extra memory locations to RECORD (never shown
                                to the agent). See --help output of the module
                                docs for the built-in names.
  --no-captures                 do not save a state when something goes wrong

  --frame-capture <dir>         write frame.png per decision for the driver
  --pace max|thinking|realtime  max (default) runs flat out; `thinking` runs
                                flat out while a plan executes but paces to
                                real time while the model is deciding, so the
                                world does not run away under a stale
                                observation; `realtime` paces everything
  --dry-run                     print the plan and the budget, start nothing
  --quiet                       no stdout chatter
"
    );
}

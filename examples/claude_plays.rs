//! Headless runner for the agent substrate — "Claude plays Pokémon" with no
//! terminal, no window and no sound, so it can be scripted, recorded and run in
//! CI.
//!
//! ```text
//! # deterministic, free, and the thing CI runs:
//! cargo run --release --example claude_plays -- \
//!     --rom target/pokemon-blue.gb --driver scripted --script script.ndjson
//!
//! # a real model, through any adapter that speaks the contract:
//! cargo run --release --example claude_plays -- \
//!     --rom target/pokemon-blue.gb \
//!     --driver command --command tools/claude_plays/claude_cli_adapter.sh \
//!     --objective "Get out of the house and choose a starter Pokemon" \
//!     --pace realtime --shots target/run --log target/run/agent.ndjson
//! ```
//!
//! Two things it exists to prove, beyond being useful:
//!
//! * **the emulator never waits on the agent** — `--report` prints the emulated
//!   frame rate, and `--driver command --command 'sleep 30'` keeps it at full
//!   speed while nothing answers;
//! * **the caps fire** — `--max-calls`, `--max-usd`, `--timeout` and the
//!   circuit breaker all end the run with a printed reason.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use terminalgb::gameboy::Gameboy;
use terminalgb_plugin_agent::{
    commentary::Kind, driver::Driver, driver::RandomDriver, driver::ScriptedDriver,
    governor::Budget, subprocess::CommandDriver, AgentPlugin, Config, PhaseView,
};

const FRAME_TIME: Duration = Duration::from_micros(16_743);

struct Args {
    rom: Option<PathBuf>,
    driver: String,
    script: Option<PathBuf>,
    command: Vec<String>,
    frames: u64,
    objective: String,
    log: Option<PathBuf>,
    frames_dir: Option<PathBuf>,
    shots: Option<PathBuf>,
    shot_every: u64,
    max_calls: u32,
    max_usd: f64,
    timeout: u64,
    wall: u64,
    pace_realtime: bool,
    report: bool,
    quiet: bool,
    seed: u64,
    latency: u32,
    state_in: Option<PathBuf>,
    state_out: Option<PathBuf>,
    stop_on: Option<String>,
    loop_script: bool,
    cooldown: u64,
    failures: u32,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            rom: None,
            driver: "scripted".into(),
            script: None,
            command: Vec::new(),
            frames: 60 * 60 * 20,
            objective: String::new(),
            log: None,
            frames_dir: None,
            shots: None,
            shot_every: 600,
            max_calls: 400,
            max_usd: 5.0,
            timeout: 120,
            wall: 3600,
            pace_realtime: false,
            report: false,
            quiet: false,
            seed: 1,
            latency: 0,
            state_in: None,
            state_out: None,
            stop_on: None,
            loop_script: false,
            cooldown: 60,
            failures: 4,
        }
    }
}

fn main() {
    let mut a = Args::default();
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rom" => a.rom = it.next().map(PathBuf::from),
            "--driver" => a.driver = it.next().unwrap_or_default(),
            "--script" => a.script = it.next().map(PathBuf::from),
            "--command" => a.command = it.by_ref().collect(),
            "--frames" => a.frames = num(it.next(), a.frames),
            "--objective" => a.objective = it.next().unwrap_or_default(),
            "--log" => a.log = it.next().map(PathBuf::from),
            "--frame-capture" => a.frames_dir = it.next().map(PathBuf::from),
            "--shots" => a.shots = it.next().map(PathBuf::from),
            "--shot-every" => a.shot_every = num(it.next(), a.shot_every),
            "--max-calls" => a.max_calls = num(it.next(), a.max_calls as u64) as u32,
            "--max-usd" => {
                a.max_usd = it.next().and_then(|s| s.parse().ok()).unwrap_or(a.max_usd)
            }
            "--timeout" => a.timeout = num(it.next(), a.timeout),
            "--wall" => a.wall = num(it.next(), a.wall),
            "--pace" => a.pace_realtime = it.next().as_deref() == Some("realtime"),
            "--report" => a.report = true,
            "--quiet" => a.quiet = true,
            "--seed" => a.seed = num(it.next(), a.seed),
            "--latency" => a.latency = num(it.next(), a.latency as u64) as u32,
            "--state" => a.state_in = it.next().map(PathBuf::from),
            "--state-out" => a.state_out = it.next().map(PathBuf::from),
            "--stop-on" => a.stop_on = it.next(),
            "--loop-script" => a.loop_script = true,
            "--cooldown" => a.cooldown = num(it.next(), a.cooldown),
            "--failures" => a.failures = num(it.next(), a.failures as u64) as u32,
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

    // `--command` deliberately swallows the rest of the command line, which is
    // the usual convention and is also a quiet footgun: put `--shots` after it
    // and the runner silently passes `--shots` to the adapter instead of taking
    // any screenshots. It cost one wasted run here, so it is now an error
    // rather than a surprise.
    const FLAGS: [&str; 22] = [
        "--rom",
        "--driver",
        "--script",
        "--command",
        "--frames",
        "--objective",
        "--log",
        "--frame-capture",
        "--shots",
        "--shot-every",
        "--max-calls",
        "--max-usd",
        "--timeout",
        "--wall",
        "--pace",
        "--report",
        "--quiet",
        "--seed",
        "--latency",
        "--state",
        "--state-out",
        "--stop-on",
    ];
    if let Some(stray) = a
        .command
        .iter()
        .skip(1)
        .find(|v| FLAGS.contains(&v.as_str()))
    {
        eprintln!(
            "'{stray}' came after --command, so it was passed to the adapter \
             rather than read as an option. --command must be LAST."
        );
        std::process::exit(2);
    }

    let Some(rom_path) = a.rom.clone() else {
        eprintln!("--rom is required");
        usage();
        std::process::exit(2);
    };

    // The MBC writes a `.sav` next to the ROM, so never run against the
    // player's own library. Stage a copy.
    let scratch = PathBuf::from("target/claude-plays");
    if let Err(e) = std::fs::create_dir_all(&scratch) {
        eprintln!("cannot create {}: {e}", scratch.display());
        std::process::exit(1);
    }
    let staged = scratch.join(
        rom_path
            .file_name()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("rom.gb")),
    );
    if staged != rom_path {
        if let Err(e) = std::fs::copy(&rom_path, &staged) {
            eprintln!("cannot stage the ROM: {e}");
            std::process::exit(1);
        }
        // A fresh save is the point of the demo: remove any staged one.
        for ext in ["sav", "gbsave", "srm"] {
            let _ = std::fs::remove_file(staged.with_extension(ext));
        }
    }

    let data = match std::fs::read(&staged) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read the ROM: {e}");
            std::process::exit(1);
        }
    };
    let mut gb = Gameboy::new(data.clone(), Some(staged.clone()));
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

    let driver: Box<dyn Driver> = match a.driver.as_str() {
        "scripted" => {
            let replies = match &a.script {
                Some(p) => match std::fs::read_to_string(p) {
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
                },
                None => {
                    eprintln!("--driver scripted needs --script <ndjson>");
                    std::process::exit(2);
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

    if let Some(d) = &a.frames_dir {
        let _ = std::fs::create_dir_all(d);
    }
    if let Some(d) = &a.shots {
        let _ = std::fs::create_dir_all(d);
    }
    if let Some(p) = &a.log {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let cfg = Config {
        budget: Budget {
            max_calls: a.max_calls,
            max_usd: a.max_usd,
            call_timeout: Duration::from_secs(a.timeout),
            max_wall: Duration::from_secs(a.wall),
            breaker_cooldown: Duration::from_secs(a.cooldown),
            max_consecutive_failures: a.failures,
            ..Budget::default()
        },
        objective: a.objective.clone(),
        log: a.log.clone(),
        frame_capture: a.frames_dir.clone(),
        ..Config::default()
    };
    let plugin = AgentPlugin::new(driver, cfg);
    if let Err(e) = gb.add_plugin(Box::new(plugin)) {
        eprintln!("cannot attach the agent: {e}");
        std::process::exit(1);
    }
    if let Some(r) = gb.plugin::<AgentPlugin>().and_then(|p| {
        if p.is_supported() {
            None
        } else {
            p.refusal()
        }
    }) {
        // `is_supported` is only known after the first dispatch; run one frame
        // so the refusal, if any, is the real one.
        gb.frame();
        if let Some(p) = gb.plugin::<AgentPlugin>() {
            if !p.is_supported() {
                eprintln!("{}", p.refusal().unwrap_or(r));
                std::process::exit(1);
            }
        }
    }

    println!(
        "agent: driver={} objective={:?} caps: {} calls, ${:.2}, {}s wall, {}s/call",
        a.driver, a.objective, a.max_calls, a.max_usd, a.wall, a.timeout
    );

    let started = Instant::now();
    let mut next = Instant::now() + FRAME_TIME;
    // Count *accepted* lines, not the length of the history buffer: the
    // buffer is bounded, so once it is full its length stops growing and a
    // naive "print everything past what I have shown" prints nothing ever
    // again. (It did, for 28,000 frames of a real run.)
    let mut shown_history = 0u64;
    let mut last_report = Instant::now();
    let mut frames_since_report = 0u64;
    let mut worst_frame = Duration::ZERO;
    let mut stop_reason = "frame budget".to_string();
    let mut frame = 0u64;

    while frame < a.frames {
        let t = Instant::now();
        gb.frame();
        let cost = t.elapsed();
        worst_frame = worst_frame.max(cost);
        frame += 1;
        frames_since_report += 1;

        if let Some(dir) = &a.shots {
            if frame % a.shot_every == 0 {
                write_png(&dir.join(format!("frame-{frame:07}.png")), gb.image());
            }
        }

        // Print new commentary as it appears, which is also how a human watches
        // a headless run go wrong.
        if !a.quiet {
            if let Some(p) = gb.plugin::<AgentPlugin>() {
                let c = p.commentary();
                let total = c.accepted();
                if total > shown_history {
                    let fresh = (total - shown_history) as usize;
                    let held = c.history().count();
                    for e in c.history().skip(held.saturating_sub(fresh)) {
                        println!("[{:>7}] {} {}", e.frame, tag(e.kind), e.text);
                    }
                    shown_history = total;
                }
            }
        }

        if a.report && last_report.elapsed() >= Duration::from_secs(5) {
            let fps = frames_since_report as f64 / last_report.elapsed().as_secs_f64();
            let (phase, calls) = gb
                .plugin::<AgentPlugin>()
                .map(|p| (p.phase(), p.governor().calls()))
                .unwrap_or((PhaseView::Stopped, 0));
            println!(
                "   … {frame} frames · {fps:.0} emulated fps · {:?} · {calls} calls \
                 · worst frame {:.2} ms",
                phase,
                worst_frame.as_secs_f64() * 1000.0
            );
            worst_frame = Duration::ZERO;
            frames_since_report = 0;
            last_report = Instant::now();
        }

        if let Some(p) = gb.plugin::<AgentPlugin>() {
            if p.phase() == PhaseView::Stopped && p.is_enabled() {
                stop_reason = p
                    .commentary()
                    .pause_reason()
                    .unwrap_or("the agent stopped")
                    .to_string();
                break;
            }
        }
        if let Some(want) = &a.stop_on {
            if reached(&mut gb, want) {
                stop_reason = format!("reached {want}");
                break;
            }
        }

        if a.pace_realtime {
            let now = Instant::now();
            if now < next {
                std::thread::sleep(next - now);
                next += FRAME_TIME;
            } else {
                // Behind: resync rather than fast-forward, so a slow host does
                // not turn into a sprint.
                next = now + FRAME_TIME;
            }
        }
    }

    let elapsed = started.elapsed();
    gb.flush_battery();
    if let Some(p) = &a.state_out {
        let bytes = gb.save_state();
        if let Err(e) = std::fs::write(p, &bytes) {
            eprintln!("cannot write the save state: {e}");
        } else {
            println!("save state written to {}", p.display());
        }
    }
    if let Some(dir) = &a.shots {
        write_png(&dir.join("final.png"), gb.image());
    }

    // Summary. Everything here is a measured fact about the run.
    let (calls, tokens, usd, failures, decisions, driver_name, halted) = gb
        .plugin::<AgentPlugin>()
        .map(|p| {
            let g = p.governor();
            (
                g.calls(),
                g.tokens(),
                g.usd(),
                g.failures(),
                p.decisions(),
                p.driver_name().to_string(),
                g.halted().map(|h| h.label().to_string()),
            )
        })
        .unwrap_or_default();

    println!("\n--- run summary ---");
    println!("stopped because   : {stop_reason}");
    println!("emulated frames   : {frame}");
    println!(
        "wall clock        : {:.1} s  ({:.0} emulated fps, {:.1}x real time)",
        elapsed.as_secs_f64(),
        frame as f64 / elapsed.as_secs_f64(),
        (frame as f64 / elapsed.as_secs_f64()) / 59.7275
    );
    println!("driver            : {driver_name}");
    println!("decisions         : {decisions}");
    println!("calls             : {calls}");
    println!("failed calls      : {failures}");
    println!("tokens (reported) : {tokens}");
    println!("spend  (reported) : ${usd:.4}");
    if let Some(h) = halted {
        println!("halted by         : {h}");
    }
    print_position(&mut gb);
    if let Some(p) = gb.plugin::<AgentPlugin>() {
        println!("final goal        : {}", p.commentary().goal());
    }
}

/// Has the run reached what it was told to wait for? Two forms:
/// `party>=1` and `map=0x28`.
fn reached(gb: &mut Gameboy, want: &str) -> bool {
    if let Some(n) = want.strip_prefix("party>=") {
        let need: u8 = n.trim().parse().unwrap_or(1);
        return gb.peek(0xD163) <= 6 && gb.peek(0xD163) >= need;
    }
    if let Some(m) = want.strip_prefix("map=") {
        let m = m.trim();
        let id = m
            .strip_prefix("0x")
            .and_then(|h| u8::from_str_radix(h, 16).ok())
            .or_else(|| m.parse().ok());
        return Some(gb.peek(0xD35E)) == id;
    }
    false
}

fn print_position(gb: &mut Gameboy) {
    let map = gb.peek(0xD35E);
    let (x, y) = (gb.peek(0xD362), gb.peek(0xD361));
    let party = gb.peek(0xD163);
    let name = terminalgb_plugin_agent::observe::map_name(map);
    println!(
        "final position    : map {map:#04x}{} at ({x},{y}), {} in party",
        if name.is_empty() {
            String::new()
        } else {
            format!(" ({name})")
        },
        if party <= 6 { party } else { 0 }
    );
}

fn tag(k: Kind) -> &'static str {
    match k {
        Kind::Goal => "GOAL ",
        Kind::Thought => "think",
        Kind::Act => "  do ",
        Kind::Event => "  !  ",
        Kind::System => " sys ",
    }
}

fn num(s: Option<String>, dflt: u64) -> u64 {
    s.and_then(|v| v.parse().ok()).unwrap_or(dflt)
}

/// Screenshots go through the `image` crate, not the terminal encoder — the
/// latter is stored-deflate, right for a 60 Hz wire and wrong for a file
/// someone keeps (see `docs/video-out.md`).
fn write_png(path: &Path, rgba: &[u8]) {
    let buf: image::RgbaImage =
        match image::ImageBuffer::from_raw(160, 144, rgba.to_vec()) {
            Some(b) => b,
            None => return,
        };
    if let Err(e) = buf.save(path) {
        eprintln!("screenshot {}: {e}", path.display());
    }
}

fn usage() {
    eprintln!(
        "usage: claude_plays --rom <file> [options]

  --driver scripted|random|command   who decides (default scripted)
  --script <ndjson>                  replies, one JSON object per line
  --command <prog> [args…]           adapter program; MUST BE LAST
  --objective <text>                 the session goal, given to the driver
  --frames N                         emulated-frame budget (default 72000)
  --pace realtime                    run at 59.7 fps instead of flat out
  --stop-on party>=1 | map=0x28      end the run when this is true
  --shots <dir> [--shot-every N]     periodic PNGs (default every 600 frames)
  --frame-capture <dir>              write frame.png per decision for the driver
  --log <file>                       NDJSON decision log (requests, replies, cost)
  --state <file> / --state-out <f>   start from / write a save state
  --loop-script                      replay the script for ever
  --cooldown S --failures N          circuit-breaker tuning (default 60 s, 4)
  --max-calls N --max-usd F          budget caps
  --timeout S --wall S               per-call and whole-run time caps
  --latency N                        fake N frames of thinking (scripted only)
  --seed N                           random driver seed
  --report                           print emulated fps every 5 s
  --quiet                            do not echo commentary
"
    );
}

//! Pure-emulation throughput, reported in **both** units.
//!
//! Everything else in this repository that carries a millisecond figure
//! measures a *frontend*: `render_bench` times the terminal transport,
//! `audio_frame_cost` times the audio hand-off, `apu_bench` times the sound
//! chip. None of them answers the question a reinforcement-learning fan-out
//! asks, which is **how many emulated frames a second does one core produce,
//! and how many does the whole box produce**.
//!
//! ```text
//! cargo run --release --example core_bench -- --rom game.gb --frames 3000
//! cargo run --release --example core_bench -- --rom game.gb --state s.gbss \
//!         --workers 20 --json
//! ```
//!
//! Two units, because two audiences read them:
//!
//!  * **ms/frame and x-real-time** — this project's own convention, comparable
//!    with every other figure in `docs/`.
//!  * **aggregate steps per second across the fan-out** — the unit the
//!    reinforcement-learning community publishes ("count multiple envs in your
//!    favour"), so our number can be placed beside a GPU implementation's.
//!    Reported with the worker count and the hardware, never bare.
//!
//! It uses only the public embedding API, so it builds on
//! `--no-default-features` and measures the core with nothing bolted to it:
//! no plugins, no audio player, no terminal, no video sink.
//!
//! ## The frame hash is the point, not a nicety
//!
//! Every run prints an FNV-1a hash of the final framebuffer **and of a WRAM
//! sample**. A build-configuration change (`opt-level`, `lto`, `target-cpu`,
//! PGO) that moves either has changed emulated behaviour and is a bug, not an
//! optimisation. Compare the hashes across an A/B before believing any
//! millisecond figure.
//!
//! ## `--no-pixels`
//!
//! Declares that the caller will not read the framebuffer, so the PPU may skip
//! pixel generation. Guest-invisible by construction (see
//! `Gameboy::set_render_pixels`), which is why the CPU-side hash is printed
//! separately: it must not move.
//!
//! ## `--breakdown` (needs `--features profile`)
//!
//! Where the frame actually goes, by removing one component's work at a time
//! and timing whole frames — `crate::profile`'s method, because a single
//! aggregate figure cannot be optimised, only guessed at. **Read the
//! instruction count in every row**: an A/B whose instruction counts differ is
//! not an A/B, and only the `no_pixels` row is guest-invisible.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use terminalgb::gameboy::Gameboy;

/// True DMG refresh rate. "100 % speed" means this, and it is what an
/// x-real-time figure divides by.
const DMG_HZ: f64 = 59.7275;

/// FNV-1a, hand-rolled for the same reason the harness hand-rolls one: this
/// example must build with no features and no crates.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

struct Args {
    rom: Option<String>,
    state: Option<String>,
    frames: u32,
    warmup: u32,
    workers: usize,
    repeat: u32,
    no_pixels: bool,
    json: bool,
    breakdown: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            rom: None,
            state: None,
            frames: 3000,
            warmup: 300,
            workers: 1,
            repeat: 3,
            no_pixels: false,
            json: false,
            breakdown: false,
        }
    }
}

fn parse() -> Args {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| -> String {
            argv.get(i + 1)
                .unwrap_or_else(|| {
                    eprintln!("{} needs a value", argv[i]);
                    std::process::exit(2)
                })
                .clone()
        };
        match argv[i].as_str() {
            "--rom" => {
                a.rom = Some(next(i));
                i += 1;
            }
            "--state" => {
                a.state = Some(next(i));
                i += 1;
            }
            "--frames" => {
                a.frames = next(i).parse().unwrap_or(a.frames);
                i += 1;
            }
            "--warmup" => {
                a.warmup = next(i).parse().unwrap_or(a.warmup);
                i += 1;
            }
            "--workers" => {
                a.workers = next(i).parse().unwrap_or(a.workers);
                i += 1;
            }
            "--repeat" => {
                a.repeat = next(i).parse().unwrap_or(a.repeat);
                i += 1;
            }
            "--no-pixels" => a.no_pixels = true,
            "--json" => a.json = true,
            "--breakdown" => a.breakdown = true,
            "-h" | "--help" => {
                println!(
                    "core_bench --rom PATH [--state PATH] [--frames N] [--warmup N]\n\
                                [--workers N] [--repeat N] [--no-pixels] [--json]\n\
                                [--breakdown]   (needs --features profile)"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown argument {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    a
}

/// One worker's result: how long it took, and what it computed.
struct Run {
    seconds: f64,
    frame_hash: u64,
    /// FNV over a WRAM window. Guest-side state, so it must be identical
    /// whether or not pixels were drawn.
    wram_hash: u64,
}

fn one_worker(rom: &[u8], state: Option<&[u8]>, a: &Args) -> Run {
    // `None` for the save path: a benchmark must never write a `.sav` beside
    // somebody's cartridge (`src/mbc/battery.rs`).
    let mut gb = Gameboy::new(rom.to_vec(), None);
    if let Some(bytes) = state {
        gb.load_state(bytes).expect("save state did not load");
    }
    gb.set_render_pixels(!a.no_pixels);

    for _ in 0..a.warmup {
        gb.frame();
    }

    let t0 = Instant::now();
    for _ in 0..a.frames {
        gb.frame();
    }
    let seconds = t0.elapsed().as_secs_f64();

    // Read WRAM through the debug peek, which suppresses the bus tick, so
    // sampling it cannot perturb the run it is measuring.
    let mut wram = Vec::with_capacity(0x2000);
    for addr in 0xC000u32..0xE000 {
        wram.push(gb.peek(addr as u16));
    }

    Run {
        seconds,
        frame_hash: fnv1a(gb.image()),
        wram_hash: fnv1a(&wram),
    }
}

/// Time `frames` frames with one component's work removed, and report both the
/// milliseconds and the instruction count — the second is what says whether the
/// row is comparable with the baseline at all.
#[cfg(feature = "profile")]
fn breakdown(rom: &[u8], state: Option<&[u8]>, a: &Args) {
    use terminalgb::profile::{self, Switches};

    let rows: [(&str, Switches, &str); 6] = [
        ("baseline", Switches::default(), "everything on"),
        (
            "no pixels",
            Switches {
                no_pixels: true,
                ..Switches::default()
            },
            "guest-INVISIBLE",
        ),
        (
            "no bus tick",
            Switches {
                no_bus_tick: true,
                ..Switches::default()
            },
            "guest-visible",
        ),
        (
            "no timer",
            Switches {
                no_timer: true,
                ..Switches::default()
            },
            "guest-visible",
        ),
        (
            "no stat line",
            Switches {
                no_stat: true,
                ..Switches::default()
            },
            "guest-visible",
        ),
        (
            "no idle batch",
            Switches {
                no_idle_batch: true,
                ..Switches::default()
            },
            "guest-invisible",
        ),
    ];

    println!(
        "{:<14} {:>10} {:>8} {:>12} {:>10}   {}",
        "row", "ms/frame", "delta", "instr/frame", "scanlines", "note"
    );
    let mut base_ms = 0.0f64;
    for (name, sw, note) in rows {
        let mut best = f64::INFINITY;
        let mut counters = profile::Counters::default();
        for _ in 0..a.repeat.max(1) {
            let mut gb = Gameboy::new(rom.to_vec(), None);
            if let Some(bytes) = state {
                gb.load_state(bytes).expect("save state did not load");
            }
            profile::set_switches(Switches::default());
            for _ in 0..a.warmup {
                gb.frame();
            }
            profile::set_switches(sw);
            let _ = profile::take_counters();
            let t0 = Instant::now();
            for _ in 0..a.frames {
                gb.frame();
            }
            let s = t0.elapsed().as_secs_f64();
            let c = profile::take_counters();
            if s < best {
                best = s;
                counters = c;
            }
        }
        profile::set_switches(Switches::default());
        let ms = best / a.frames as f64 * 1e3;
        if name == "baseline" {
            base_ms = ms;
        }
        let delta = if name == "baseline" {
            "  —".to_string()
        } else {
            format!("{:+.1}%", (base_ms / ms - 1.0) * 100.0)
        };
        println!(
            "{name:<14} {ms:>10.4} {delta:>8} {:>12} {:>10}   {note}",
            counters.instructions / a.frames.max(1),
            counters.scanlines / a.frames.max(1),
        );
    }
    println!();
    println!("A row whose instr/frame differs from the baseline's is NOT an A/B.");
}

#[cfg(not(feature = "profile"))]
fn breakdown(_rom: &[u8], _state: Option<&[u8]>, _a: &Args) {
    eprintln!("--breakdown needs `--features profile` (measurement-only, off in every shipping build)");
    std::process::exit(2);
}

fn main() {
    let a = parse();
    let Some(rom_path) = a.rom.clone() else {
        eprintln!("core_bench needs --rom PATH (no cartridge ships with this repository)");
        std::process::exit(2);
    };
    let rom = std::fs::read(&rom_path).unwrap_or_else(|e| {
        eprintln!("cannot read {rom_path}: {e}");
        std::process::exit(2)
    });
    let state = a.state.as_ref().map(|p| {
        std::fs::read(p).unwrap_or_else(|e| {
            eprintln!("cannot read {p}: {e}");
            std::process::exit(2)
        })
    });

    if a.breakdown {
        breakdown(&rom, state.as_deref(), &a);
        return;
    }

    let rom = Arc::new(rom);
    let state = Arc::new(state);

    // Best of `--repeat` rounds. A throughput figure taken once on a shared
    // box is a measurement of the box's other work.
    let mut best_ms = f64::INFINITY;
    let mut best_sps = 0.0f64;
    let mut frame_hash = 0u64;
    let mut wram_hash = 0u64;
    let mut hashes_agree = true;

    for _ in 0..a.repeat {
        let wall = Instant::now();
        let busy = Arc::new(AtomicU64::new(0));
        let mut handles = Vec::with_capacity(a.workers);
        for _ in 0..a.workers {
            let rom = Arc::clone(&rom);
            let state = Arc::clone(&state);
            let busy = Arc::clone(&busy);
            let args = Args {
                rom: None,
                state: None,
                frames: a.frames,
                warmup: a.warmup,
                workers: 1,
                repeat: 1,
                no_pixels: a.no_pixels,
                json: a.json,
                breakdown: false,
            };
            handles.push(std::thread::spawn(move || {
                let r = one_worker(&rom, state.as_deref(), &args);
                busy.fetch_add((r.seconds * 1e9) as u64, Ordering::Relaxed);
                r
            }));
        }
        let results: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wall = wall.elapsed().as_secs_f64();

        // Per-instance cost is the mean of each worker's own busy time, which
        // is what a single-instance user experiences and what every other
        // ms/frame figure in this repository means.
        let busy_total = busy.load(Ordering::Relaxed) as f64 / 1e9;
        let ms = busy_total / (a.workers as f64) / (a.frames as f64) * 1e3;
        // Aggregate steps per second is wall-clock across the whole fan-out —
        // the community convention, which counts every environment in your
        // favour.
        let sps = (a.workers as f64) * (a.frames as f64) / wall;

        for r in &results {
            if frame_hash == 0 {
                frame_hash = r.frame_hash;
                wram_hash = r.wram_hash;
            } else if r.frame_hash != frame_hash || r.wram_hash != wram_hash {
                hashes_agree = false;
            }
        }
        if ms < best_ms {
            best_ms = ms;
        }
        if sps > best_sps {
            best_sps = sps;
        }
    }

    let per_core_fps = 1e3 / best_ms;
    let realtime = per_core_fps / DMG_HZ;

    if a.json {
        println!(
            "{{\"ms_per_frame\":{best_ms:.4},\"fps_per_instance\":{per_core_fps:.1},\
             \"x_realtime\":{realtime:.1},\"aggregate_sps\":{best_sps:.0},\
             \"workers\":{},\"frames\":{},\"no_pixels\":{},\
             \"frame_hash\":\"{frame_hash:#018x}\",\"wram_hash\":\"{wram_hash:#018x}\",\
             \"deterministic\":{hashes_agree}}}",
            a.workers, a.frames, a.no_pixels
        );
    } else {
        println!("rom            {rom_path}");
        if let Some(p) = &a.state {
            println!("state          {p}");
        }
        println!(
            "workers        {}   frames/worker {}   best of {}",
            a.workers, a.frames, a.repeat
        );
        println!("pixels         {}", if a.no_pixels { "off" } else { "on" });
        println!();
        println!("per instance   {best_ms:.4} ms/frame   {per_core_fps:.0} fps   {realtime:.1}x real time");
        println!(
            "aggregate      {best_sps:.0} steps/s across {} worker(s)",
            a.workers
        );
        println!();
        println!("frame hash     {frame_hash:#018x}");
        println!("wram hash      {wram_hash:#018x}");
        println!(
            "workers agree  {}",
            if hashes_agree { "yes" } else { "NO — NOT DETERMINISTIC" }
        );
    }

    if !hashes_agree {
        std::process::exit(1);
    }
}

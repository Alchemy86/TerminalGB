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
//! ## Modelling a collector's machine: `--audio`, `--no-pixels`, `--no-audio-out`
//!
//! `--no-pixels` declares that the caller will not read the framebuffer, so the
//! PPU may skip pixel generation. `--no-audio-out` declares that nobody is
//! listening, so the APU may skip the band-limited resample. Both are
//! guest-invisible by construction (see `Gameboy::set_render_pixels` and
//! `Gameboy::set_render_audio`), which is why the CPU-side WRAM hash is printed
//! separately: it must not move across either.
//!
//! **`--audio` is what makes the figure a collector's figure.** A bare
//! `Gameboy` has `sound` at `None`, which is a machine with *no sound chip*
//! rather than a quiet one — a difference a cartridge can see — so every
//! scenario, longplay and swarm run installs a `SilentPlayer`. Without
//! `--audio` this example is measuring a machine none of them run, and the APU
//! is worth about 14 % of their frame. The WRAM hash moves when `--audio` is
//! added, and should: the machine really is different.
//!
//! ## `--reset-every N`
//!
//! Reload the start state every N frames — the shape of a certification run,
//! which is thousands of attempts and so thousands of `load_state` calls.
//! Answers "is an episode long enough to amortise its own reset?" by doing it
//! and timing whole frames, rather than by timing one call.
//!
//! ## `--breakdown` (needs `--features profile`)
//!
//! Where the frame actually goes, by removing one component's work at a time
//! and timing whole frames — `crate::profile`'s method, because a single
//! aggregate figure cannot be optimised, only guessed at. **Read the
//! instruction count in every row**: an A/B whose instruction counts differ is
//! not an A/B, and only the `no_pixels` row is guest-invisible.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Instant;

use terminalgb::gameboy::Gameboy;

/// True DMG refresh rate. "100 % speed" means this, and it is what an
/// x-real-time figure divides by.
const DMG_HZ: f64 = 59.7275;

/// Nanoseconds this **thread** has actually spent on a CPU, or `None` where the
/// kernel does not offer it.
///
/// **The per-instance figure is a property of the binary, not of the box's
/// afternoon, and a wall clock cannot tell the two apart.** On a machine with
/// other work on it a descheduled benchmark reports the scheduler's decisions
/// as if they were the emulator's cost: measured here, the same binary read
/// 0.0930 ms/frame with the box quiet and 0.1336 with eight cores busy
/// elsewhere — a 44 % error, larger than every build-configuration difference
/// this example exists to measure, and it moved variants past each other.
///
/// `/proc/thread-self/schedstat`'s first field is that time in nanoseconds
/// (`Documentation/scheduler/sched-stats.rst`). It is read rather than called
/// because this example must build on `--no-default-features`, where there is
/// no `libc` to reach `clock_gettime` with, and because `/proc` costs one file
/// read per *run* rather than one per frame.
///
/// Frequency scaling still varies with the box's load and this does not correct
/// for it, which is why the runs are interleaved and the best round is taken.
/// The wall clock is still what the aggregate steps-per-second figure divides
/// by — that one is wall-clock *by definition*, since it is what a fan-out
/// actually delivers on the machine it is running on.
fn thread_cpu_nanos() -> Option<u64> {
    let s = std::fs::read_to_string("/proc/thread-self/schedstat").ok()?;
    s.split_whitespace().next()?.parse().ok()
}

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
    speed: String,
    audio: bool,
    audio_out: bool,
    reset_every: u32,
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
            speed: String::new(),
            audio: false,
            audio_out: true,
            reset_every: 0,
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
            "--reset-every" => {
                a.reset_every = next(i).parse().unwrap_or(0);
                i += 1;
            }
            "--no-pixels" => a.no_pixels = true,
            "--speed-mode" => a.speed = "speed".into(),
            "--speed" => {
                a.speed = next(i).to_string();
                i += 1;
            }
            "--audio" => a.audio = true,
            "--no-audio-out" => a.audio_out = false,
            "--json" => a.json = true,
            "--breakdown" => a.breakdown = true,
            "-h" | "--help" => {
                println!(
                    "core_bench --rom PATH [--state PATH] [--frames N] [--warmup N]\n\
                                [--workers N] [--repeat N] [--no-pixels] [--audio]\n\
                                [--speed-mode] [--speed SPEC]\n\
                                [--no-audio-out] [--reset-every N] [--json]\n\
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
    /// Wall clock over the timed section.
    seconds: f64,
    /// CPU time over the same section, where the kernel offers it — see
    /// [`thread_cpu_nanos`]. This is what the per-instance figure uses.
    cpu_seconds: Option<f64>,
    frame_hash: u64,
    /// FNV over a WRAM window. Guest-side state, so it must be identical
    /// whether or not pixels were drawn.
    wram_hash: u64,
}

fn one_worker(rom: &[u8], state: Option<&[u8]>, a: &Args, start: &Barrier) -> Run {
    // `None` for the save path: a benchmark must never write a `.sav` beside
    // somebody's cartridge (`src/mbc/battery.rs`).
    let mut gb = Gameboy::new(rom.to_vec(), None);
    // **A collector's machine has an APU and a bare `Gameboy` does not.** With
    // `sound` at `None` the chip is absent rather than quiet, which a cartridge
    // can tell (`sound::SilentPlayer`), so every scenario, longplay and swarm
    // run installs one. Without `--audio` this bench is measuring a machine
    // none of them run.
    if a.audio {
        gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
    }
    gb.set_render_audio(a.audio_out);
    if let Some(bytes) = state {
        gb.load_state(bytes).expect("save state did not load");
    }
    gb.set_render_pixels(!a.no_pixels);
    apply_speed(&mut gb, &a.speed);

    for _ in 0..a.warmup {
        gb.frame();
    }

    // **Every worker finishes warming up before the wall clock starts.**
    // Without it the aggregate figure divides the measured frames by a wall
    // clock that also contains the warmup, and understates itself by
    // `warmup / (warmup + frames)` — 13 % at the defaults. The community unit
    // is the one an outside reader compares us on, so it has to be honest in
    // our own disfavour as well as our favour.
    start.wait();

    // **What an episode's reset costs, in the unit an episode is measured in.**
    // A certification run is thousands of attempts and every one of them is a
    // `load_state`, so if restoring is expensive relative to a short episode
    // that is swarm throughput spent on setup rather than on play. Measured
    // the same way as everything else here: by doing it and timing whole
    // frames, never by timing one call.
    let episode = if a.reset_every > 0 {
        Some(gb.save_state())
    } else {
        None
    };

    let c0 = thread_cpu_nanos();
    let t0 = Instant::now();
    for f in 0..a.frames {
        gb.frame();
        if let Some(bytes) = &episode {
            if (f + 1) % a.reset_every == 0 {
                gb.load_state(bytes).expect("reset");
            }
        }
    }
    let seconds = t0.elapsed().as_secs_f64();
    let cpu_seconds = match (c0, thread_cpu_nanos()) {
        (Some(a), Some(b)) if b > a => Some((b - a) as f64 / 1e9),
        _ => None,
    };

    // Read WRAM through the debug peek, which suppresses the bus tick, so
    // sampling it cannot perturb the run it is measuring.
    let mut wram = Vec::with_capacity(0x2000);
    for addr in 0xC000u32..0xE000 {
        wram.push(gb.peek(addr as u16));
    }

    Run {
        seconds,
        cpu_seconds,
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

    // The rows are additive on top of the run's own configuration, so
    // `--breakdown --audio --no-pixels --no-audio-out` breaks down the machine a
    // *collector* runs rather than a bare `Gameboy`.
    let base = Switches {
        no_pixels: a.no_pixels,
        ..Switches::default()
    };
    let rows: [(&str, Switches, &str); 9] = [
        ("baseline", base, "everything on"),
        (
            "no pixels",
            Switches {
                no_pixels: true,
                ..base
            },
            "guest-INVISIBLE",
        ),
        (
            "no bus tick",
            Switches {
                no_bus_tick: true,
                ..base
            },
            "guest-visible",
        ),
        (
            "no timer",
            Switches {
                no_timer: true,
                ..base
            },
            "guest-visible",
        ),
        (
            "no stat line",
            Switches {
                no_stat: true,
                ..base
            },
            "guest-visible",
        ),
        (
            "no idle batch",
            Switches {
                no_idle_batch: true,
                ..base
            },
            "guest-invisible",
        ),
        (
            "no timer batch",
            Switches {
                no_timer_batch: true,
                ..base
            },
            "guest-invisible",
        ),
        (
            "no apu",
            Switches {
                no_apu: true,
                ..base
            },
            "guest-visible",
        ),
        (
            "no oam dma",
            Switches {
                no_oam_dma: true,
                ..base
            },
            "guest-visible",
        ),
    ];

    println!(
        "{:<14} {:>10} {:>8} {:>12} {:>9} {:>9}   {}",
        "row", "ms/frame", "delta", "instr/frame", "wram", "frame", "note"
    );
    let mut base_ms = 0.0f64;
    let mut base_counters = profile::Counters::default();
    let mut base_hashes = (0u64, 0u64);
    for (name, sw, note) in rows {
        let mut best = f64::INFINITY;
        let mut counters = profile::Counters::default();
        let mut hashes = (0u64, 0u64);
        for _ in 0..a.repeat.max(1) {
            let mut gb = Gameboy::new(rom.to_vec(), None);
            // Same machine `one_worker` builds: a collector's has a sound chip
            // and a bare `Gameboy` does not, which a cartridge can tell.
            if a.audio {
                gb.enable_audio(Box::new(terminalgb::sound::SilentPlayer));
            }
            gb.set_render_audio(a.audio_out);
            if let Some(bytes) = state {
                gb.load_state(bytes).expect("save state did not load");
            }
            gb.set_render_pixels(!a.no_pixels);
            apply_speed(&mut gb, &a.speed);
            profile::set_switches(base);
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
                // What the cartridge could see. The `note` column *claims*
                // guest-visible or not; these two say whether it was, for this
                // workload, which is a different and more useful question — a
                // row marked guest-visible whose WRAM hash never moves is a
                // liberty a *performance* mode could take for this game.
                let mut wram = Vec::with_capacity(0x2000);
                for addr in 0xC000u32..0xE000 {
                    wram.push(gb.peek(addr as u16));
                }
                hashes = (fnv1a(&wram), fnv1a(gb.image()));
            }
        }
        profile::set_switches(base);
        let ms = best / a.frames as f64 * 1e3;
        if name == "baseline" {
            base_ms = ms;
            base_counters = counters;
            base_hashes = hashes;
        }
        let delta = if name == "baseline" {
            "  —".to_string()
        } else {
            format!("{:+.1}%", (base_ms / ms - 1.0) * 100.0)
        };
        let mark = |same: bool| {
            if same || name == "baseline" {
                "same"
            } else {
                "MOVED"
            }
        };
        println!(
            "{name:<14} {ms:>10.4} {delta:>8} {:>12} {:>9} {:>9}   {note}",
            counters.instructions / a.frames.max(1),
            mark(hashes.0 == base_hashes.0),
            mark(hashes.1 == base_hashes.1),
        );
    }
    println!();
    println!(
        "halted {} M-cycles a frame in {} batches ({:.1} each); \
         could not batch because [dma {} serial {} irq {} no-ppu-horizon {}], \
         bounded by [timer {} ppu {}]",
        base_counters.halts / a.frames.max(1),
        base_counters.halt_batches / a.frames.max(1),
        base_counters.halts as f64 / base_counters.halt_batches.max(1) as f64,
        base_counters.idle_reason[0],
        base_counters.idle_reason[1],
        base_counters.idle_reason[2],
        base_counters.idle_reason[3],
        base_counters.idle_reason[4],
        base_counters.idle_reason[5],
    );
    println!("A row whose instr/frame differs from the baseline's is NOT an A/B.");
    println!(
        "`wram`/`frame` are what the cartridge and the screen ended up holding: \
         `same` means this row's work was invisible TO THIS WORKLOAD, whatever \
         the note says about hardware."
    );
}

#[cfg(not(feature = "profile"))]
fn breakdown(_rom: &[u8], _state: Option<&[u8]>, _a: &Args) {
    eprintln!("--breakdown needs `--features profile` (measurement-only, off in every shipping build)");
    std::process::exit(2);
}

/// Apply a `$GB_SPEED`-style spec, warning about anything not understood.
fn apply_speed(gb: &mut terminalgb::gameboy::Gameboy, spec: &str) {
    if spec.is_empty() {
        return;
    }
    let (flags, bad) = terminalgb::speed::SpeedFlags::parse(spec);
    for token in &bad {
        eprintln!("unknown speed flag {token:?} — ignored");
    }
    gb.set_speed_flags(flags);
    // `identical` is the default picture mode since 2026-08-20 and refuses
    // every shortcut, so a bare `core_bench --speed-mode` measures the accurate
    // machine. Every figure in docs/throughput.md is `standard`; say so rather
    // than letting the run look four times slower for no stated reason. Once,
    // not once per machine — this is called per worker and per breakdown row.
    static SAID: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    if flags.any()
        && !gb.speed_flags().any()
        && !SAID.swap(true, std::sync::atomic::Ordering::Relaxed)
    {
        eprintln!(
            "speed flags refused: the picture mode is `identical`, which takes \
             no shortcuts. Re-run with GB_PPU=standard (docs/modes.md)."
        );
    }
}

fn main() {
    let a = parse();
    let Some(rom_path) = a.rom.clone() else {
        eprintln!(
            "core_bench needs --rom PATH (no cartridge ships with this repository)"
        );
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
    let mut best_wall_ms = f64::INFINITY;
    let mut best_sps = 0.0f64;
    let mut on_cpu_clock = false;
    let mut frame_hash = 0u64;
    let mut wram_hash = 0u64;
    let mut hashes_agree = true;

    for _ in 0..a.repeat {
        let busy = Arc::new(AtomicU64::new(0));
        let wallbusy = Arc::new(AtomicU64::new(0));
        // `workers + 1`: this thread waits on it too, so the wall clock starts
        // at the same instant every worker's timed section does.
        let gate = Arc::new(Barrier::new(a.workers + 1));
        let mut handles = Vec::with_capacity(a.workers);
        for _ in 0..a.workers {
            let rom = Arc::clone(&rom);
            let state = Arc::clone(&state);
            let busy = Arc::clone(&busy);
            let wallbusy = Arc::clone(&wallbusy);
            let gate = Arc::clone(&gate);
            let args = Args {
                rom: None,
                state: None,
                frames: a.frames,
                warmup: a.warmup,
                workers: 1,
                repeat: 1,
                no_pixels: a.no_pixels,
                speed: a.speed.clone(),
                audio: a.audio,
                audio_out: a.audio_out,
                reset_every: a.reset_every,
                json: a.json,
                breakdown: false,
            };
            handles.push(std::thread::spawn(move || {
                let r = one_worker(&rom, state.as_deref(), &args, &gate);
                busy.fetch_add(
                    (r.cpu_seconds.unwrap_or(r.seconds) * 1e9) as u64,
                    Ordering::Relaxed,
                );
                wallbusy.fetch_add((r.seconds * 1e9) as u64, Ordering::Relaxed);
                r
            }));
        }
        gate.wait();
        let wall = Instant::now();
        let results: Vec<Run> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wall = wall.elapsed().as_secs_f64();

        // Per-instance cost is the mean of each worker's own **CPU** time,
        // which is what a single-instance user experiences on a machine that is
        // theirs and what every other ms/frame figure in this repository means.
        // See `thread_cpu_nanos` for why it is not the wall clock.
        on_cpu_clock = results.iter().all(|r| r.cpu_seconds.is_some());
        let busy_total = busy.load(Ordering::Relaxed) as f64 / 1e9;
        let ms = busy_total / (a.workers as f64) / (a.frames as f64) * 1e3;
        let wall_ms = wallbusy.load(Ordering::Relaxed) as f64
            / 1e9
            / (a.workers as f64)
            / (a.frames as f64)
            * 1e3;
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
        if wall_ms < best_wall_ms {
            best_wall_ms = wall_ms;
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
             \"wall_ms_per_frame\":{best_wall_ms:.4},\"cpu_clock\":{on_cpu_clock},\
             \"workers\":{},\"frames\":{},\"no_pixels\":{},\"audio\":{},\"audio_out\":{},\
             \"frame_hash\":\"{frame_hash:#018x}\",\"wram_hash\":\"{wram_hash:#018x}\",\
             \"deterministic\":{hashes_agree}}}",
            a.workers, a.frames, a.no_pixels, a.audio, a.audio_out
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
        println!(
            "apu            {}",
            match (a.audio, a.audio_out) {
                (false, _) => "not installed (a machine a cartridge can tell apart)",
                (true, true) => "installed, samples produced",
                (true, false) => "installed, samples skipped (--no-audio-out)",
            }
        );
        println!();
        println!(
            "clock          {}",
            if on_cpu_clock {
                "thread CPU time (immune to a busy box) — see thread_cpu_nanos"
            } else {
                "wall (no /proc/thread-self/schedstat here)"
            }
        );
        println!();
        println!("per instance   {best_ms:.4} ms/frame   {per_core_fps:.0} fps   {realtime:.1}x real time");
        if on_cpu_clock {
            println!(
                "  same by wall {best_wall_ms:.4} ms/frame   (gap = what this box's other work cost)"
            );
        }
        println!(
            "aggregate      {best_sps:.0} steps/s across {} worker(s)",
            a.workers
        );
        println!();
        println!("frame hash     {frame_hash:#018x}");
        println!("wram hash      {wram_hash:#018x}");
        println!(
            "workers agree  {}",
            if hashes_agree {
                "yes"
            } else {
                "NO — NOT DETERMINISTIC"
            }
        );
    }

    if !hashes_agree {
        std::process::exit(1);
    }
}

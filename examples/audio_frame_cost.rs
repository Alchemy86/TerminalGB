//! What one *emulated* frame costs when a real sound card is attached — the
//! number the terminal frontend reports as `emulate_ms`, measured against the
//! actual device rather than headless.
//!
//! ```text
//! cargo run --release --features audio --example audio_frame_cost -- [rom.gb] [frames]
//! ```
//!
//! This is the tool that named the 17.9 ms-per-frame bug. Three things it
//! reports that a headless benchmark cannot:
//!
//! * **wall vs. thread CPU time per frame.** The frame cost was 17.29 ms of
//!   which 0.42 ms was executing — 97.6 % of it was the audio player sleeping
//!   inside `Cpu::do_cycle`. Any future "emulation got slow" report should
//!   start here: if the two numbers diverge, emulation is not the problem.
//! * **achieved emulated fps.** With the old blocking hand-off this sat at
//!   57.8 Hz *unpaced* — slower than real time — so a frame-paced frontend
//!   could never catch up and fell to a `MAX_CATCHUP` batch per draw (6 fps).
//! * **cycles per frame.** Not always 70224: while a game holds the LCD off
//!   (Pokémon Blue's boot and its screen transitions) the GPU never raises
//!   `updated`, so a frame-paced loop runs the CPU unbounded — 845,836 cycles
//!   in one iteration was measured — and the APU emits a matching burst of
//!   samples. That is why the queue's latency bound occasionally discards
//!   during a transition; see docs/audio.md.
//!
//! `PACED=1` paces to the 59.7275 Hz deadline like the frontends do (use this
//! for realistic numbers); without it the loop runs flat out, which is how you
//! see whether anything in the audio path is silently acting as a clock.
//! `GB_AUDIO_STATS=1` adds the player's five-second queue lines.
#[cfg(feature = "audio")]
fn main() {
    use gameboy::cpu::core::Cpu;
    use std::time::{Duration, Instant};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let frames: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(600);
    let paced = std::env::var("PACED").is_ok();

    // The cartridge is always staged into target/ first: the MBC writes its
    // save file next to the ROM path it was given, and a benchmark must never
    // touch a real save (see CLAUDE.md).
    let dir = std::path::PathBuf::from("target/testrom-artifacts/perf");
    std::fs::create_dir_all(&dir).unwrap();
    let (rom, path) = match args.first() {
        Some(p) => {
            let src = std::path::Path::new(p);
            let dst = dir.join(src.file_name().unwrap());
            let data = std::fs::read(src).expect("cannot read ROM");
            std::fs::write(&dst, &data).unwrap();
            (data, dst)
        }
        None => tone_rom(&dir),
    };

    let mut cpu = Cpu::new(rom, Some(path));
    let (player, _stream) = match gameboy::sound::cpal_audio::CpalPlayer::new() {
        Some(x) => x,
        None => {
            eprintln!("no audio device — this benchmark needs real hardware");
            std::process::exit(1);
        }
    };
    let device_rate = {
        use gameboy::sound::AudioPlayer;
        player.samples_rate()
    };
    let queue = player.queue_handle();
    cpu.memory.enable_audio(Box::new(player));

    const FRAME: Duration = Duration::from_micros(16_743);
    let mut costs: Vec<f64> = Vec::with_capacity(frames as usize);
    let (mut cpu_ms, mut total_cycles, mut max_cycles) = (0.0f64, 0u64, 0u64);
    let start = Instant::now();
    let mut next = start + FRAME;
    for _ in 0..frames {
        let t = Instant::now();
        let c0 = thread_cpu_secs();
        let mut cyc = 0u64;
        loop {
            cyc += cpu.do_cycle() as u64;
            if cpu.memory.gpu.updated {
                cpu.memory.gpu.updated = false;
                break;
            }
        }
        cpu_ms += (thread_cpu_secs() - c0) * 1000.0;
        costs.push(t.elapsed().as_secs_f64() * 1000.0);
        total_cycles += cyc;
        max_cycles = max_cycles.max(cyc);
        if paced {
            let now = Instant::now();
            if now < next {
                std::thread::sleep(next - now);
            }
            next += FRAME;
        }
    }
    let wall = start.elapsed().as_secs_f64();

    costs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = costs.len();
    let avg = costs.iter().sum::<f64>() / n as f64;
    let cpu_avg = cpu_ms / n as f64;

    println!(
        "{} run at device rate {device_rate} Hz",
        if paced { "paced" } else { "unpaced" }
    );
    println!(
        "emulate_ms   : avg {avg:.3}  p50 {:.3}  p90 {:.3}  p99 {:.3}  max {:.3}  (budget 16.743)",
        costs[n / 2],
        costs[n * 9 / 10],
        costs[n * 99 / 100],
        costs[n - 1]
    );
    println!(
        "  executing  : {cpu_avg:.3} ms/frame — {:.1}% of the frame cost was NOT executing",
        100.0 * (1.0 - cpu_avg / avg)
    );
    println!(
        "cycles/frame : avg {:.0} (70224 = one LCD frame)  max {max_cycles}",
        total_cycles as f64 / n as f64
    );
    println!(
        "emulated fps : {:.2} over {wall:.2} s (real time = 59.73)",
        n as f64 / wall
    );
    let q = queue.lock().unwrap();
    println!(
        "queue        : depth {}..{}, underruns {}, held {}, discarded {} (+{} pre-roll)",
        if q.stats.depth_min == usize::MAX {
            0
        } else {
            q.stats.depth_min
        },
        q.stats.depth_max,
        q.stats.underruns,
        q.stats.held,
        q.stats.dropped,
        q.stats.dropped_before_playback
    );
}

/// CPU time actually burned by this thread. The gap against wall time is time
/// spent blocked, which is the entire question this benchmark exists to answer.
#[cfg(feature = "audio")]
fn thread_cpu_secs() -> f64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 / 1e9
}

/// Same quiet 440 Hz tone program `audio_soak` uses, so a run needs no ROM.
#[cfg(feature = "audio")]
fn tone_rom(dir: &std::path::Path) -> (Vec<u8>, std::path::PathBuf) {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x134..0x134 + 8].copy_from_slice(b"TONEPERF");
    let mut chk: u8 = 0;
    for &b in rom.iter().take(0x14D).skip(0x134) {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x14D] = chk;
    let prog: &[u8] = &[
        0x3E, 0x80, 0xE0, 0x26, // NR52 on
        0x3E, 0xFF, 0xE0, 0x25, // NR51 all channels
        0x3E, 0x77, 0xE0, 0x24, // NR50 max
        0x3E, 0x80, 0xE0, 0x11, // NR11 duty 50%
        0x3E, 0x20, 0xE0, 0x12, // NR12 vol 2 (quiet), DAC on
        0x3E, 0xD6, 0xE0, 0x13, // NR13
        0x3E, 0x86, 0xE0, 0x14, // NR14: trigger
        0x18, 0xFE, // spin
    ];
    rom[0x150..0x150 + prog.len()].copy_from_slice(prog);
    let p = dir.join("toneperf.gb");
    std::fs::write(&p, &rom).unwrap();
    (rom, p)
}

#[cfg(not(feature = "audio"))]
fn main() {
    println!(
        "audio_frame_cost needs the `audio` feature: \
         cargo run --release --features audio --example audio_frame_cost"
    );
}

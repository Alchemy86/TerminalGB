//! Real-device audio soak test: run the emulator against the actual sound
//! card for N seconds, paced exactly like the frontends (59.7275 Hz deadline
//! accumulator), and report **measured** queue behaviour — underruns, depth
//! min/max, drops — plus the achieved sample-production rate vs. the device
//! rate. This is the evidence run for the buffer policy in `src/sound.rs`:
//!
//! ```text
//! cargo run --release --features audio --example audio_soak -- [seconds]
//! ```
//!
//! It plays a deliberately *quiet* 440 Hz tone (channel 1, volume 2/15) so a
//! multi-minute run is not obnoxious on speakers. `GB_AUDIO_STATS=1` adds the
//! player's own 5-second stat lines on top.
#[cfg(feature = "audio")]
fn main() {
    use gameboy::cpu::core::Cpu;
    use std::time::{Duration, Instant};

    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);

    // Quiet tone ROM (same program as the audiocapture harness fixture, but
    // NR12 volume 2 instead of 15).
    let mut rom = vec![0u8; 0x8000];
    rom[0x100..0x104].copy_from_slice(&[0x00, 0xC3, 0x50, 0x01]);
    rom[0x134..0x134 + 8].copy_from_slice(b"TONESOAK");
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
        0x3E, 0xD6, 0xE0, 0x13, // NR13: 1750 & 0xFF
        0x3E, 0x86, 0xE0, 0x14, // NR14: trigger | (1750 >> 8)
        0x18, 0xFE, // spin
    ];
    rom[0x150..0x150 + prog.len()].copy_from_slice(prog);

    let dir = std::path::PathBuf::from("target/testrom-artifacts/audio");
    std::fs::create_dir_all(&dir).unwrap();
    let rom_path = dir.join("tonesoak.gb");
    std::fs::write(&rom_path, &rom).unwrap();

    let mut cpu = Cpu::new(rom, Some(rom_path));

    let (player, _stream) = match gameboy::sound::cpal_audio::CpalPlayer::new() {
        Some(x) => x,
        None => {
            eprintln!("no audio device — soak test needs real hardware");
            std::process::exit(1);
        }
    };
    let device_rate = {
        use gameboy::sound::AudioPlayer;
        player.samples_rate()
    };
    let queue = player.queue_handle();
    cpu.memory.enable_audio(Box::new(player));

    println!("soak: {seconds}s at device rate {device_rate} Hz, frame period 16.743 ms");

    const FRAME: Duration = Duration::from_micros(16_743);
    let start = Instant::now();
    let mut next_frame = start + FRAME;
    let end = start + Duration::from_secs(seconds);
    let mut frames: u64 = 0;
    while Instant::now() < end {
        // one emulated frame
        loop {
            cpu.do_cycle();
            if cpu.memory.gpu.updated {
                cpu.memory.gpu.updated = false;
                break;
            }
        }
        frames += 1;
        // deadline accumulator (same scheme as the fixed frontends)
        let now = Instant::now();
        if now < next_frame {
            std::thread::sleep(next_frame - now);
        } else if now > next_frame + 4 * FRAME {
            next_frame = now; // hopelessly behind: resync
        }
        next_frame += FRAME;
    }
    let wall = start.elapsed().as_secs_f64();

    let (stats, depth_now) = {
        let q = queue.lock().unwrap();
        (q.stats, q.len())
    };
    let produced_rate = stats.pushed as f64 / wall;
    let consumed_rate = stats.consumed as f64 / wall;
    println!("---------------- measured over {wall:.1} s ----------------");
    println!(
        "frames emulated:      {frames} ({:.4} Hz vs 59.7275 target)",
        frames as f64 / wall
    );
    println!(
        "samples produced:     {} ({produced_rate:.2}/s vs device {device_rate})",
        stats.pushed
    );
    println!(
        "  -> production drift: {:+.4}% of device rate",
        (produced_rate / device_rate as f64 - 1.0) * 100.0
    );
    println!(
        "samples consumed:     {} ({consumed_rate:.2}/s)",
        stats.consumed
    );
    println!("underruns:            {}", stats.underruns);
    println!(
        "held (substituted):   {} ({:.1} ms total, incl. initial prime)",
        stats.held,
        stats.held as f64 * 1000.0 / device_rate as f64
    );
    println!("dropped (overflow):   {}", stats.dropped);
    println!(
        "  before playback:    {} ({:.0} ms of pre-roll discarded while the \
         device was still opening)",
        stats.dropped_before_playback,
        stats.dropped_before_playback as f64 * 1000.0 / device_rate as f64
    );
    println!("queue depth:          min {} max {} now {depth_now} (start-fill {}, high-water {})",
        if stats.depth_min == usize::MAX { 0 } else { stats.depth_min },
        stats.depth_max,
        gameboy::sound::sample_queue::start_fill(),
        gameboy::sound::sample_queue::high_water());
    println!("audio callbacks:      {}", stats.callbacks);

    let ok = stats.underruns == 0 && stats.dropped == 0;
    println!(
        "verdict: {}",
        if ok {
            "PASS (zero underruns, zero drops)"
        } else {
            "FAIL"
        }
    );
    std::process::exit(if ok { 0 } else { 1 });
}

#[cfg(not(feature = "audio"))]
fn main() {
    println!("audio soak needs the `audio` feature: cargo run --release --features audio --example audio_soak");
}

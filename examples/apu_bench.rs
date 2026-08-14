//! Headless APU cost benchmark: how much emulated-frame time the APU costs.
//!
//! Runs the same ROM twice — once with no APU installed at all (`Sound::do_cycle`
//! returns immediately) and once with a device-free capture player — and reports
//! the per-frame difference. That difference is the number the APU accuracy work
//! has to keep in budget.
//!
//! ```text
//! cargo run --release --example apu_bench -- <rom.gb> [frames]
//! ```
use std::time::Instant;
use terminalgb::cpu::core::Cpu;
use terminalgb::sound::AudioPlayer;

struct Sink;
impl AudioPlayer for Sink {
    fn play(&mut self, _l: &[f32], _r: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        44_100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

fn run(rom: &std::path::Path, frames: u32, audio: bool) -> f64 {
    let data = std::fs::read(rom).unwrap();
    let mut cpu = Cpu::new(data, Some(rom.to_path_buf()));
    if audio {
        cpu.memory.enable_audio(Box::new(Sink));
    }
    // Warm up so neither run pays for cold caches or the boot sequence.
    let mut done = 0;
    while done < 60 {
        cpu.do_cycle();
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            done += 1;
        }
    }
    let t = Instant::now();
    let mut done = 0;
    while done < frames {
        cpu.do_cycle();
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            done += 1;
        }
    }
    t.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

fn main() {
    let mut args = std::env::args().skip(1);
    let rom =
        std::path::PathBuf::from(args.next().expect("usage: apu_bench <rom> [frames]"));
    let frames: u32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(2000);
    let (mut off, mut on) = (f64::MAX, f64::MAX);
    // Best-of-3: the minimum is the least noisy estimate of the real cost.
    for _ in 0..3 {
        off = off.min(run(&rom, frames, false));
        on = on.min(run(&rom, frames, true));
    }
    println!("frames          {frames}");
    println!("no APU          {off:.4} ms/frame");
    println!("APU installed   {on:.4} ms/frame");
    println!("APU cost        {:.1} us/frame", (on - off) * 1000.0);
    println!(
        "realtime factor {:.1}x (with APU, vs 16.743 ms)",
        16.743 / on
    );
}

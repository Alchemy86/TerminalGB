//! **Emulation speed with host audio attached** — the check that was missing.
//!
//! ```text
//! cargo test --release --features audio --test audioperf
//! ```
//!
//! ## Why this exists
//!
//! An `AudioPlayer` is driven from `Sound::play_samples`, which runs inside
//! `Cpu::do_cycle`, which runs inside `Gameboy::frame`. Anything the player
//! does is therefore charged to the emulated frame. The cpal player used to
//! *wait* at its queue's high watermark to make the sound card the master
//! clock; the frontends already pace to 59.7275 Hz with a deadline
//! accumulator, so the two clocks fought and the emulator lost:
//!
//! | build | `emulate_ms` | display fps |
//! |---|---|---|
//! | `--features audio` | 17.93 | 5.98 |
//! | without audio | 0.29 | 56.75 |
//!
//! Every existing benchmark missed it because **none of them attaches a
//! player**: `render_bench` is headless and reported a healthy 0.246 ms while
//! the maintainer's real session ran at 6 fps.
//!
//! ## What it measures, and why it needs no sound card
//!
//! The failure did not need a *device* — it needed a **full queue**. So this
//! attaches a player that is the real producer half (`sample_queue::hand_off`,
//! the exact code `CpalPlayer::play` calls) with **no consumer at all**: the
//! queue saturates within the first few frames and stays saturated, which is
//! precisely the state the maintainer's machine was in. Under the old
//! wait-at-the-watermark policy every frame then sleeps up to 150 ms; under the
//! current policy the excess is discarded and the frame costs what emulation
//! costs. That runs on any CI runner, headless.
//!
//! Two gates, one absolute and one scale-free:
//!
//! * `emulate_ms` averaged over the run must stay under [`MAX_EMULATE_MS`].
//! * emulating N frames must take **far less** than N frame periods
//!   ([`MIN_SPEEDUP`]×), which is the property that actually broke and does not
//!   depend on how fast the runner is.
//!
//! It also pins the latency bound itself: the queue must never exceed
//! `high_water()`, or "we stopped waiting" would have been bought by letting
//! delay grow without limit.
#[cfg(feature = "audio")]
use std::sync::{Arc, Mutex};
#[cfg(feature = "audio")]
use std::time::{Duration, Instant};
#[cfg(feature = "audio")]
use terminalgb::gameboy::Gameboy;
#[cfg(feature = "audio")]
use terminalgb::sound::sample_queue::{self, high_water, SampleQueue};
#[cfg(feature = "audio")]
use terminalgb::sound::AudioPlayer;

/// One emulated frame: 70224 cycles at 4.194304 MHz.
#[cfg(feature = "audio")]
const FRAME_TIME: Duration = Duration::from_micros(16_743);

/// Ceiling on the average cost of one emulated frame with audio attached.
/// Measured here: ~0.35 ms on a real device, ~0.3 ms in this harness. The
/// budget is 16.743 ms and the task brief called anything above ~8 ms
/// headroom-free, so 4 ms is a >10× margin over reality and still catches a
/// regression an order of magnitude smaller than the one that shipped.
#[cfg(feature = "audio")]
const MAX_EMULATE_MS: f64 = 4.0;

/// Emulation must run at least this many times faster than real time with a
/// player attached. The bug pinned it at ~1.0× (worse, in fact: 0.97×). Scale
/// free, so a slow shared runner cannot make it flaky in the way an absolute
/// millisecond gate can.
#[cfg(feature = "audio")]
const MIN_SPEEDUP: f64 = 4.0;

#[cfg(feature = "audio")]
const FRAMES: u32 = 900;

/// The producer half of the real hand-off, with no consumer behind it — a
/// device that has stopped draining, or simply an emulator running ahead.
#[cfg(feature = "audio")]
struct SaturatedPlayer {
    queue: Arc<Mutex<SampleQueue>>,
}

#[cfg(feature = "audio")]
impl AudioPlayer for SaturatedPlayer {
    fn play(&mut self, left: &[f32], right: &[f32]) {
        sample_queue::hand_off(&self.queue, left, right);
    }
    fn samples_rate(&self) -> u32 {
        44100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

#[cfg(feature = "audio")]
fn main() {
    // The vendored MIT acid2 ROM: always present, no submodule needed. It does
    // not play sound, which is deliberate — the cost under test is the
    // *hand-off*, not the synthesis, and a silent ROM still produces a full
    // sample stream (a powered-off APU emits silence at the full rate).
    let rom_path = std::path::Path::new("third_party/dmg-acid2/dmg-acid2.gb");
    let data = match std::fs::read(rom_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {}: {e}", rom_path.display());
            std::process::exit(1);
        }
    };
    // Staged into target/ so the MBC's save file never lands beside the ROM.
    let staged = std::path::PathBuf::from("target/testrom-artifacts/audioperf.gb");
    std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
    std::fs::write(&staged, &data).unwrap();

    let mut gb = Gameboy::new(data, Some(staged));
    let queue = Arc::new(Mutex::new(SampleQueue::new(
        terminalgb::sound::sample_queue::start_fill(),
        high_water(),
    )));
    gb.enable_audio(Box::new(SaturatedPlayer {
        queue: queue.clone(),
    }));

    let mut worst = 0.0f64;
    let mut total = 0.0f64;
    let mut depth_max = 0usize;
    let start = Instant::now();
    for _ in 0..FRAMES {
        let t = Instant::now();
        gb.frame();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        total += ms;
        worst = worst.max(ms);
        depth_max = depth_max.max(queue.lock().unwrap().len());
    }
    let wall = start.elapsed();

    let avg = total / FRAMES as f64;
    let realtime = FRAME_TIME.as_secs_f64() * FRAMES as f64;
    let speedup = realtime / wall.as_secs_f64();
    let stats = queue.lock().unwrap().stats;

    println!("audio-attached emulation speed ({FRAMES} frames)");
    println!("  emulate_ms      : avg {avg:.3}  worst {worst:.3}  (budget 16.743, gate {MAX_EMULATE_MS})");
    println!(
        "  wall            : {:.2} s for {:.2} s of emulated time -> {speedup:.1}x real time (gate {MIN_SPEEDUP}x)",
        wall.as_secs_f64(),
        realtime
    );
    println!(
        "  queue           : pushed {} depth_max {depth_max} (high_water {}), discarded {}",
        stats.pushed,
        high_water(),
        stats.dropped + stats.dropped_before_playback
    );

    let mut failures = Vec::new();
    if avg > MAX_EMULATE_MS {
        failures.push(format!(
            "emulate_ms avg {avg:.3} exceeds {MAX_EMULATE_MS} — the audio path \
             is charging the emulated frame again (a player must never block; \
             see sample_queue::hand_off)"
        ));
    }
    if speedup < MIN_SPEEDUP {
        failures.push(format!(
            "emulation ran at only {speedup:.2}x real time with a player \
             attached — audio is pacing the emulator, which leaves the frontend \
             permanently in catch-up (this is exactly the 6 fps bug)"
        ));
    }
    if depth_max > high_water() {
        failures.push(format!(
            "queue reached {depth_max} samples, above high_water {} — latency \
             is no longer bounded",
            high_water()
        ));
    }
    // The producer must actually have been saturated, or the run proved
    // nothing: without a consumer the queue has to hit the watermark.
    if stats.dropped + stats.dropped_before_playback == 0 {
        failures.push(
            "nothing was discarded, so the queue never saturated and this run \
             did not exercise the watermark path at all"
                .to_string(),
        );
    }

    if failures.is_empty() {
        println!("\nAudio-attached emulation stays inside the frame budget. ✅");
    } else {
        for f in &failures {
            eprintln!("\nFAIL: {f}");
        }
        std::process::exit(1);
    }
}

/// Without the feature there is no host player to charge the frame, so there
/// is nothing to measure — say so rather than passing silently.
#[cfg(not(feature = "audio"))]
fn main() {
    println!(
        "audioperf needs the `audio` feature: \
         cargo test --release --features audio --test audioperf"
    );
}

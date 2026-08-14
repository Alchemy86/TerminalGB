//! Measure **end-to-end audio latency** — the delay between the emulator
//! producing a sample and a speaker moving — together with underruns, because
//! the two trade off against each other and a fix that wins one by losing the
//! other is not a fix.
//!
//! ```text
//! cargo run --release --features audio --example audio_latency -- <rom.gb> [seconds]
//! ```
//!
//! The path a sample travels, and what this measures at each hop:
//!
//! ```text
//!   APU  --block-->  SampleQueue  --callback-->  host/device  -->  speaker
//!         BLOCK              depth/rate                cpal timestamp
//!        (5.8 ms)          (measured live)          (playback - callback)
//! ```
//!
//! `audio_soak.rs` answers "does it break up?"; this answers "how far behind is
//! it?", and reports both so a change cannot quietly trade one for the other.
//! Roughly: under ~30 ms is imperceptible, 30-60 ms is the usual target for an
//! emulator, and above ~100 ms is felt on every button press.
//!
//! Knobs, for A/B against the pre-fix build:
//!
//! ```text
//! GB_AUDIO_DEVICE_FRAMES=0   # let the host pick the device buffer (old behaviour)
//! GB_AUDIO_START_FILL=7000   # queue cushion in samples (old value)
//! GB_AUDIO_HIGH_WATER=12000  # producer wait threshold in samples (old value)
//! ```

#[cfg(all(feature = "audio", not(target_arch = "wasm32")))]
fn main() {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    use terminalgb::gameboy::Gameboy;
    use terminalgb::sound::cpal_audio::CpalPlayer;
    use terminalgb::sound::AudioPlayer;

    // One emulated frame: 70224 cycles at 4.194304 MHz = 59.7275 Hz.
    const FRAME_TIME: Duration = Duration::from_micros(16_743);

    let mut args = std::env::args().skip(1);
    let rom = match args.next() {
        Some(p) => p,
        None => {
            eprintln!("usage: audio_latency <rom.gb> [seconds]");
            std::process::exit(2);
        }
    };
    let seconds: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);

    let data = match std::fs::read(&rom) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {rom}: {e}");
            std::process::exit(2);
        }
    };
    let mut gb = Gameboy::new(data, Some(std::path::PathBuf::from(&rom)));

    let (player, _stream) = match CpalPlayer::new() {
        Some(p) => p,
        None => {
            eprintln!("no audio device available — nothing to measure");
            std::process::exit(1);
        }
    };
    let rate = player.samples_rate();
    let queue = player.queue_handle();
    let devlat = player.device_latency();
    let configured = player.configured_buffer();
    gb.enable_audio(Box::new(player));

    println!("rom            : {rom}");
    println!("device rate    : {rate} Hz");
    println!(
        "device buffer  : {}",
        match configured {
            Some(f) => format!(
                "requested {f} frames ({:.1} ms)",
                f as f64 * 1000.0 / rate as f64
            ),
            None => "host default (not pinned)".to_string(),
        }
    );
    println!("running        : {seconds}s\n");

    let start = Instant::now();
    let run_for = Duration::from_secs(seconds);
    let mut next_frame = Instant::now() + FRAME_TIME;
    let mut frames: u64 = 0;

    // Per-second samples: queue depth, device latency, and the total. Extremes
    // alone hide the shape; the timeline shows whether a number is the steady
    // state or a one-off transient.
    let mut marks: Vec<(u64, usize, u64, u64)> = Vec::new();
    let mut next_mark = start + Duration::from_secs(1);
    let mut last_underruns = 0u64;

    // Production-stall distribution. The queue cushion exists to ride out gaps
    // in production, so its size should be derived from how long those gaps
    // actually are on this machine — not from an assumed worst case. "Lateness"
    // is how far past its deadline a frame finished; a run of late frames is a
    // stall, and the worst one is what the cushion has to cover.
    let mut lateness_ms: Vec<f64> = Vec::new();
    // (seconds into the run, ms late) for every frame that ran long. *When* a
    // stall happens decides whether it could have caused an underrun: one
    // during startup priming is harmless, one mid-run is not.
    let mut late_events: Vec<(f64, f64)> = Vec::new();

    while start.elapsed() < run_for {
        gb.frame();
        frames += 1;

        // Deadline accumulator (never reset to `now()` — that silently adds the
        // frame's own cost to every period and starves the queue).
        next_frame += FRAME_TIME;
        let now = Instant::now();
        if next_frame < now {
            let late = (now - next_frame).as_secs_f64() * 1000.0;
            lateness_ms.push(late);
            if late > 1.0 {
                late_events.push((start.elapsed().as_secs_f64(), late));
            }
            next_frame = now + FRAME_TIME;
        } else {
            lateness_ms.push(0.0);
            std::thread::sleep(next_frame - now);
        }

        if Instant::now() >= next_mark {
            let (depth, underruns) = {
                let q = queue.lock().unwrap();
                (q.len(), q.stats.underruns)
            };
            marks.push((
                start.elapsed().as_secs(),
                depth,
                devlat.micros.load(Ordering::Relaxed),
                underruns - last_underruns,
            ));
            last_underruns = underruns;
            next_mark += Duration::from_secs(1);
        }
    }

    let elapsed = start.elapsed().as_secs_f64();
    let q = queue.lock().unwrap();
    let st = q.stats;
    let cb_frames = devlat.callback_frames.load(Ordering::Relaxed);

    println!("  t(s)  queue  queue_ms  device_ms  total_ms  underruns");
    for (t, d, dev, u) in &marks {
        let qms = *d as f64 * 1000.0 / rate as f64;
        let dms = *dev as f64 / 1000.0;
        println!(
            "  {t:>4}  {d:>5}  {qms:>8.1}  {dms:>9.1}  {:>8.1}  {u:>9}{}",
            qms + dms,
            if *u > 0 { "  <-- UNDERRUN" } else { "" }
        );
    }

    // Steady state excludes the first second: the host prefills its ring on
    // stream start, which is a one-off and not what the player feels.
    let steady: Vec<(f64, f64)> = marks
        .iter()
        .skip(1)
        .map(|(_, d, dev, _)| (*d as f64 * 1000.0 / rate as f64, *dev as f64 / 1000.0))
        .collect();
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
    let qms: Vec<f64> = steady.iter().map(|x| x.0).collect();
    let dms: Vec<f64> = steady.iter().map(|x| x.1).collect();
    let tms: Vec<f64> = steady.iter().map(|x| x.0 + x.1).collect();
    let minmax = |v: &[f64]| {
        v.iter()
            .fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
    };

    println!(
        "\n--- {elapsed:.1}s wall clock, {frames} frames ({:.4} fps) ---",
        frames as f64 / elapsed
    );
    println!(
        "device callback   : {cb_frames} frames ({:.1} ms) per call, {} callbacks",
        cb_frames as f64 * 1000.0 / rate as f64,
        st.callbacks
    );
    if !tms.is_empty() {
        let (qlo, qhi) = minmax(&qms);
        let (dlo, dhi) = minmax(&dms);
        let (tlo, thi) = minmax(&tms);
        println!("\nLATENCY (steady state, per-second samples)");
        println!(
            "  our queue       : mean {:.1} ms   range {qlo:.1}-{qhi:.1} ms",
            mean(&qms)
        );
        println!(
            "  device/host     : mean {:.1} ms   range {dlo:.1}-{dhi:.1} ms",
            mean(&dms)
        );
        println!(
            "  END TO END      : mean {:.1} ms   range {tlo:.1}-{thi:.1} ms",
            mean(&tms)
        );
    }
    // Percentiles of production lateness: the empirical basis for the cushion.
    let mut sorted = lateness_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| -> f64 {
        if sorted.is_empty() {
            0.0
        } else {
            sorted[((sorted.len() as f64 - 1.0) * p) as usize]
        }
    };
    let late_frames = lateness_ms.iter().filter(|x| **x > 1.0).count();
    println!("\nPRODUCTION STALLS (how late each frame finished)");
    println!(
        "  frames > 1 ms late: {late_frames} / {} ({:.3}%)",
        lateness_ms.len(),
        late_frames as f64 / lateness_ms.len().max(1) as f64 * 100.0
    );
    println!(
        "  p99 {:.1} ms   p99.9 {:.1} ms   worst {:.1} ms",
        pct(0.99),
        pct(0.999),
        sorted.last().copied().unwrap_or(0.0)
    );
    for (t, ms) in late_events.iter().take(12) {
        println!("    at t={t:.2}s: {ms:.1} ms late");
    }

    println!("\nGLITCHES");
    println!("  underruns       : {}", st.underruns);
    println!(
        "  held samples    : {} ({:.1} ms substituted in total)",
        st.held,
        st.held as f64 * 1000.0 / rate as f64
    );
    println!("  dropped         : {}", st.dropped);
    println!(
        "  dropped pre-roll: {} (device still opening; not a glitch)",
        st.dropped_before_playback
    );
    println!(
        "  produced/consumed: {} / {}  ({:+.4}% drift vs {rate} Hz)",
        st.pushed,
        st.consumed,
        (st.pushed as f64 / elapsed / rate as f64 - 1.0) * 100.0
    );

    let mean_total = mean(&tms);
    println!(
        "\nverdict: {}",
        match (st.underruns == 0 && st.dropped == 0, mean_total) {
            (true, t) if t < 60.0 => "GOOD — no glitches and latency under 60 ms",
            (true, _) => "clean but LAGGY — no glitches, latency still perceptible",
            (false, t) if t < 60.0 => "LOW LATENCY but GLITCHING — cushion is too small",
            _ => "BAD — glitching AND laggy",
        }
    );
}

#[cfg(not(all(feature = "audio", not(target_arch = "wasm32"))))]
fn main() {
    eprintln!(
        "audio_latency needs the `audio` feature: \
         cargo run --release --features audio --example audio_latency -- <rom.gb>"
    );
}

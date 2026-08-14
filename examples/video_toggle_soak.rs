//! Prove that `v` off / `v` on survives repeated pressing against a **real**
//! destination — a v4l2loopback camera, a FIFO or a file — with a real consumer
//! attached.
//!
//! ```text
//! cargo run --release --example video_toggle_soak -- /dev/video10 10
//! ```
//!
//! It mirrors the frontend's own decision (`VideoOut::must_stay_open`: close a
//! recording, hold anything live) using the public API, so what it soaks is the
//! shipped logic and not a copy of it. Each cycle paints a different flat grey,
//! so a consumer recording alongside it can be checked frame by frame: the
//! levels must appear in order, separated by the idle card, which is only
//! possible if every single "on" *and* every "off" reached the consumer.
//!
//! Why it is an example and not a test: no CI runner has a v4l2loopback node,
//! and the failure this exists to catch — `VIDIOC_S_FMT` answering `EINVAL`
//! forever once a viewer holds an `exclusive_caps` device — needs the kernel
//! module and a second process. `src/screen/tui.rs` covers the FIFO and file
//! paths headlessly.
use gameboy::video::{Destination, VideoOut, VideoSpec};
use std::time::{Duration, Instant};

fn main() {
    let dest = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/dev/video10".to_string());
    let cycles: u32 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let on = Duration::from_millis(
        std::env::args()
            .nth(3)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1500),
    );
    let off = Duration::from_millis(
        std::env::args()
            .nth(4)
            .and_then(|s| s.parse().ok())
            .unwrap_or(1000),
    );

    let spec = || VideoSpec::new(Destination::parse(&dest).expect("a destination"));
    let (mut v, note) = VideoOut::open(spec());
    report("open", &v, note.as_deref());
    if !v.is_enabled() {
        eprintln!("soak: could not open {dest} — nothing to prove");
        std::process::exit(1);
    }

    let mut frame = vec![255u8; 160 * 144 * 4];
    let mut last = (0u64, 0u64);
    let mut failures = 0;
    for c in 1..=cycles {
        // Each cycle a distinct flat grey, so the consumer's recording can be
        // read back as "cycle 1, then 2, then 3…".
        let level = (20 + (c as u8 % 12) * 20).max(20);
        for px in frame.chunks_exact_mut(4) {
            px[0] = level;
            px[1] = level;
            px[2] = level;
            px[3] = 255;
        }
        let t = Instant::now();
        while t.elapsed() < on {
            v.frame(&frame, 160, 144, Instant::now());
            std::thread::sleep(Duration::from_millis(8));
        }
        let s = v.stats();
        let (df, db) = (s.frames - last.0, s.bytes - last.1);
        last = (s.frames, s.bytes);
        println!(
            "cycle {c:>3} ON  grey={level:>3} state={:?} +{df} frames +{db} bytes \
             (total {} / {}) stalled={} errors={}",
            v.state(),
            s.frames,
            s.bytes,
            s.stalled,
            s.write_errors
        );
        if df == 0 {
            failures += 1;
            println!("  !! nothing reached the destination this cycle");
        }

        // …and off. Exactly what `App::stop_video` does. `GB_SOAK_NO_PARK=1`
        // restores the policy that shipped before this soak existed — always
        // release the destination — which is how the soak is shown to have
        // teeth: against a camera with a viewer attached it fails from the
        // second or third cycle and never recovers.
        v.flush();
        let no_park = std::env::var("GB_SOAK_NO_PARK").is_ok();
        if v.must_stay_open() && !no_park {
            v.go_idle();
        } else {
            v = VideoOut::disabled();
        }
        let idle_before = v.stats().idle_frames;
        let t = Instant::now();
        while t.elapsed() < off {
            // The frontends' per-frame tick. Rate-limited inside `idle`, so the
            // card goes out at IDLE_PERIOD however often this is called.
            v.idle(Instant::now());
            std::thread::sleep(Duration::from_millis(8));
        }
        println!(
            "cycle {c:>3} OFF state={:?} held={} idle-cards sent={} fds={}",
            v.state(),
            v.is_idle(),
            v.stats().idle_frames - idle_before,
            fds()
        );
        if v.is_idle() && v.stats().idle_frames == idle_before {
            failures += 1;
            println!("  !! held the destination but sent it nothing");
        }

        // …and on again. Exactly what `App::start_video` does.
        if v.is_idle() {
            v.resume();
        } else {
            let (nv, note) = VideoOut::open(spec());
            if !nv.is_enabled() {
                failures += 1;
                println!("  !! reopen failed: {note:?}");
            }
            // A reopened emitter starts its counters again; keep the soak's own
            // totals honest by rebasing rather than pretending they continue.
            last = (0, 0);
            v = nv;
        }
    }
    println!(
        "\n{cycles} cycles, {failures} failure(s), {} frames, {} bytes, fds={}",
        v.stats().frames,
        v.stats().bytes,
        fds()
    );
    std::process::exit(if failures == 0 { 0 } else { 1 });
}

fn report(what: &str, v: &VideoOut, note: Option<&str>) {
    println!(
        "{what}: enabled={} state={:?} geometry={:?} camera={} note={:?}",
        v.is_enabled(),
        v.state(),
        v.geometry(),
        v.is_camera(),
        note.map(|n| n.lines().next().unwrap_or("")),
    );
}

/// Open file descriptors, so a leak across cycles is visible rather than
/// inferred.
fn fds() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .map(|d| d.count())
        .unwrap_or(0)
}

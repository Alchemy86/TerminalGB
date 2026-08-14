//! Headless video recorder — run a ROM and write raw frames to a pipe, a FIFO
//! or a file, with no terminal and no window.
//!
//! This is the producer for the documented incantation: **it owns stdout**, so
//! it composes with everything.
//!
//! ```text
//! # 300 frames of the vendored acid2 ROM into an MP4
//! cargo run --release --example video_out -- --rom third_party/dmg-acid2/dmg-acid2.gb \
//!     --frames 300 | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 \
//!     -i - -pix_fmt yuv420p /tmp/gb.mp4
//!
//! # the same, at 640x480 in the format a webcam consumer wants
//! cargo run --release --example video_out -- --rom game.gb --frames 600 \
//!     --format yuyv --size 640x480 --source display --upscale lcd --factor 4 \
//!     | ffmpeg -f rawvideo -pix_fmt yuyv422 -s 640x480 -framerate 59.7275 -i - out.mkv
//! ```
//!
//! **Pacing defaults to `emulated` here**, not `realtime`: a headless run has
//! no wall clock worth respecting, so it emits exactly one frame per emulated
//! frame and finishes as fast as the CPU allows. The result plays back at
//! 59.7275 fps and is deterministic — the same ROM and frame count produce
//! byte-identical output, which is what makes it usable as a test fixture.
//! Pass `--pace realtime` to record in real time instead.
//!
//! Headless means **no input**: this records a game's attract mode, a test ROM,
//! or a demo. To stream a game you are *playing*, use the terminal frontend
//! with `--video-out <fifo>` or the desktop frontend with `--video-out -`.

use gameboy::gameboy::Gameboy;
use gameboy::screen::config::Palette;
use gameboy::screen::frame::FramePrep;
use gameboy::upscale::{Algorithm, Upscaler};
use gameboy::video::{
    Destination, Pace, PixelFormat, Source, VideoOut, VideoSpec, FRAME_PERIOD,
};

use std::time::Instant;

struct Args {
    rom: Option<String>,
    frames: u64,
    out: String,
    format: PixelFormat,
    size: Option<(u32, u32)>,
    source: Source,
    pace: Pace,
    palette: Palette,
    upscale: Algorithm,
    factor: u8,
    quiet: bool,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            rom: None,
            frames: 300,
            out: "-".to_string(),
            format: PixelFormat::Rgba,
            size: None,
            source: Source::Native,
            pace: Pace::Emulated,
            palette: Palette::Grayscale,
            upscale: Algorithm::Nearest,
            factor: 1,
            quiet: false,
        }
    }
}

fn main() {
    let args = parse_args();

    let rom_path = args.rom.clone().unwrap_or_else(|| {
        // The vendored MIT acid2 ROM, so the example runs in a fresh clone.
        "third_party/dmg-acid2/dmg-acid2.gb".to_string()
    });
    let data = std::fs::read(&rom_path).unwrap_or_else(|e| {
        eprintln!("video_out: cannot read {rom_path}: {e}");
        std::process::exit(1);
    });
    let mut gb = Gameboy::new(data, Some(std::path::PathBuf::from(&rom_path)));

    let mut upscaler = Upscaler::new(args.upscale, args.factor);
    let (up_w, up_h) = upscaler.output_size(160, 144);
    let mut prep = FramePrep::new();

    let dest = Destination::parse(&args.out).unwrap_or_else(|| {
        eprintln!("video_out: --out {} means \"disabled\"", args.out);
        std::process::exit(2);
    });
    let spec = VideoSpec {
        dest,
        format: args.format,
        source: args.source,
        // Fix the geometry up front so the announced size is the truth.
        size: args.size.or(match args.source {
            Source::Native => None,
            Source::Display => Some((up_w as u32, up_h as u32)),
        }),
        pace: args.pace,
        // Names the sidecar descriptor beside a FIFO, and through it the
        // virtual camera — see `video::descriptor`.
        title: Some(gb.rom_title()),
    };
    let (mut video, note) = VideoOut::open(spec);
    if let Some(e) = note {
        eprintln!("video_out: {e}");
    }
    // Described *after* opening, not before: a camera negotiates its own
    // geometry and format, so announcing the requested numbers would print
    // 160x144 for a stream that is about to be 640x480.
    if !args.quiet {
        eprintln!("video_out: {}", video.describe());
    }
    // A note is not a failure: a FIFO with no consumer yet, or a camera whose
    // device has not appeared, both come back armed *and* explained. Only an
    // emitter that could not be armed at all is fatal.
    if !video.is_enabled() {
        std::process::exit(1);
    }

    let start = Instant::now();
    for i in 0..args.frames {
        gb.frame();
        // `realtime` pacing is honoured against a *virtual* clock so a headless
        // run still produces the right frame count without sleeping: one frame
        // period per emulated frame is exactly what the real loop delivers.
        let now = start + FRAME_PERIOD.mul_f64(i as f64);
        match args.source {
            Source::Native => video.frame(gb.image(), 160, 144, now),
            Source::Display => {
                let mapped = prep.map(gb.image(), args.palette);
                let scaled = upscaler.apply(mapped, 160, 144);
                video.frame(scaled, up_w as u32, up_h as u32, now);
            }
        }
    }
    video.flush();

    if !args.quiet {
        let s = video.stats();
        let secs = start.elapsed().as_secs_f64();
        eprintln!(
            "video_out: {} frames, {:.1} MB, {} duplicated, {} dropped, {} write errors \
             ({:.2}s wall, {:.0}x realtime)",
            s.frames,
            s.bytes as f64 / 1_048_576.0,
            s.duplicated,
            s.dropped,
            s.write_errors,
            secs,
            (args.frames as f64 / 59.7275) / secs.max(1e-9),
        );
    }
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    let next = |i: &mut usize, flag: &str| -> String {
        *i += 1;
        argv.get(*i)
            .cloned()
            .unwrap_or_else(|| die(&format!("{flag} needs a value")))
    };
    while i < argv.len() {
        match argv[i].as_str() {
            "--rom" => a.rom = Some(next(&mut i, "--rom")),
            "--frames" => {
                a.frames = next(&mut i, "--frames")
                    .parse()
                    .unwrap_or_else(|_| die("--frames wants a number"))
            }
            "--out" => a.out = next(&mut i, "--out"),
            "--format" => {
                let v = next(&mut i, "--format");
                a.format = PixelFormat::parse(&v)
                    .unwrap_or_else(|| die("--format wants rgba, rgb24 or yuyv"))
            }
            "--size" => {
                let v = next(&mut i, "--size");
                a.size = Some(
                    gameboy::video::parse_size(&v)
                        .unwrap_or_else(|| die("--size wants WxH, e.g. 640x480")),
                )
            }
            "--source" => {
                let v = next(&mut i, "--source");
                a.source = Source::parse(&v)
                    .unwrap_or_else(|| die("--source wants native or display"))
            }
            "--pace" => {
                let v = next(&mut i, "--pace");
                a.pace = Pace::parse(&v)
                    .unwrap_or_else(|| die("--pace wants realtime or emulated"))
            }
            "--palette" => {
                let v = next(&mut i, "--palette");
                a.palette = Palette::from_str(&v)
                    .unwrap_or_else(|| die("--palette wants grayscale, green or amber"))
            }
            "--upscale" => {
                let v = next(&mut i, "--upscale");
                a.upscale = Algorithm::from_str(&v).unwrap_or_else(|| {
                    die("--upscale wants nearest, scale2x, mmpx or lcd")
                })
            }
            "--factor" => {
                let v: u8 = next(&mut i, "--factor")
                    .parse()
                    .unwrap_or_else(|_| die("--factor wants a number"));
                a.factor = v;
            }
            "--quiet" => a.quiet = true,
            "-h" | "--help" => {
                eprintln!("{USAGE}");
                std::process::exit(0);
            }
            other => die(&format!("unknown argument {other}\n{USAGE}")),
        }
        i += 1;
    }
    a.factor = a.upscale.clamp_factor(a.factor);
    a
}

fn die(msg: &str) -> ! {
    eprintln!("video_out: {msg}");
    std::process::exit(2);
}

const USAGE: &str = "\
usage: video_out [options]   (writes raw frames to stdout by default)

  --rom PATH        ROM to run (default: the vendored dmg-acid2)
  --frames N        emulated frames to record (default 300)
  --out -|PATH      stdout, a file, or a FIFO (default -)
  --format FMT      rgba (default), rgb24, yuyv
  --size WxH        letterbox into this size (default: the source size)
  --source SRC      native (default) or display (palette + upscaler)
  --pace PACE       emulated (default here) or realtime
  --palette P       grayscale (default), green, amber   [--source display]
  --upscale A       nearest (default), scale2x, mmpx, lcd  [--source display]
  --factor N        upscale factor                      [--source display]
  --quiet           no progress on stderr

consume with, e.g.:
  cargo run --release --example video_out -- --frames 300 \\
    | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - out.mp4";

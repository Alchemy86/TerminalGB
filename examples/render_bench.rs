//! Headless per-frame render breakdown for the terminal frontend.
//!
//! Answers, without a terminal and without a human: **where does the frame time
//! actually go, and how does it scale with window size?**
//!
//! It runs the real pipeline stages — emulate, palette-map, upscale, encode for
//! a terminal graphics protocol, write the bytes out — over a matrix of window
//! sizes and upscaler settings, and reports milliseconds per stage plus the
//! bytes per frame each combination puts on the wire.
//!
//! Two paths are measured so before/after is a like-for-like comparison:
//!
//! * `legacy` — what the frontend did before: `ratatui-image` with
//!   `Resize::Scale`, which resizes the source **to the pixel size of the
//!   display rectangle** and encodes that. Cost grows with the window.
//! * `capped` — [`terminalgb::termimage`]: transmit at the upscaler's own output
//!   size and let the terminal stretch to the cell rectangle. Cost is constant
//!   in the window size.
//!
//! ```text
//! cargo run --release --example render_bench                    # table
//! cargo run --release --example render_bench -- --json          # one summary line
//! cargo run --release --example render_bench -- --ndjson        # one line per frame
//! cargo run --release --example render_bench -- --rom PATH --frames 300 \
//!         --cols 213 --rows 55 --upscale mmpx --factor 2 --path capped --json
//! ```
//!
//! The write stage writes real bytes to a real file (`--out`, default
//! `/dev/null`). That measures our side of the pipe honestly; it cannot measure
//! the terminal's own parse and texture upload, which no headless tool can.
//!
//! # `--tty`: measuring the half that was hiding
//!
//! Headless numbers are why a 91 KB-per-frame bug survived: the bench reported
//! **2,395 fps** while the maintainer's WezTerm ran the same build at **6**. Our
//! side of the pipe was never the problem — the terminal's parse and upload
//! was, and writing to `/dev/null` cannot see it.
//!
//! `--tty` closes that hole. It drives the *controlling terminal*, and after
//! each frame asks for a cursor-position report (`CSI 6 n`) and waits for the
//! reply. A terminal answers only once it has consumed everything queued ahead of
//! it, so the round-trip is a real barrier: the measured frame rate includes the
//! terminal's own work, not just ours.
//!
//! ```text
//! # run it from a graphics-capable terminal, NOT inside tmux
//! cargo run --release --example render_bench -- --tty --frames 300 --json
//! GB_TERM_IMAGE=rgb \
//!   cargo run --release --example render_bench -- --tty --frames 300 --json
//! ```
//!
//! The second command is the "before" measurement: `GB_TERM_IMAGE=rgb` restores
//! the uncompressed Kitty payload, so the two runs differ only in what is on the
//! wire.

use terminalgb::gameboy::Gameboy;
use terminalgb::screen::config::Palette;
use terminalgb::screen::frame::FramePrep;
use terminalgb::telemetry::{self, AudioSample, Context, FrameSample, Telemetry};
use terminalgb::termimage::{CellRect, TermImage, Wire};
use terminalgb::upscale::{Algorithm, Upscaler};

use std::io::Write;
use std::time::Instant;

use ratatui::{buffer::Buffer, layout::Rect};
use ratatui_image::{picker::Picker, picker::ProtocolType, Resize};

const SRC_W: u32 = 160;
const SRC_H: u32 = 144;
/// True DMG refresh rate; "100 % speed" means this.
const DMG_HZ: f32 = 59.7275;

struct Args {
    rom: Option<String>,
    frames: u32,
    /// Terminal sizes to sweep, in cells: (cols, rows).
    sizes: Vec<(u16, u16)>,
    font: (u16, u16),
    upscale: Option<(Algorithm, u8)>,
    palette: Palette,
    protocol: ProtocolType,
    path: Option<&'static str>,
    json: bool,
    ndjson: bool,
    out: String,
    /// Perturb one pixel per frame so **every** frame counts as changed. Gives
    /// the worst case for a moving picture regardless of what the ROM draws;
    /// off means the ROM decides and unchanged-frame skipping shows up.
    motion: bool,
    /// Drive the real controlling terminal and measure what it can actually
    /// keep up with (see the module docs).
    tty: bool,
    /// Ask the terminal which transports it supports, and print the answers.
    probe: bool,
    /// Save state to load before measuring, so a run can start from real
    /// gameplay instead of a title screen.
    state: Option<String>,
    /// Hold walking directions during the run.
    ///
    /// The honest motion case for a Game Boy: walking scrolls the whole
    /// background every frame, so nothing is deduplicated and the transport is
    /// measured on the frames it actually has to send. `--motion` fakes that by
    /// poking one pixel; this is the real thing.
    walk: bool,
    /// Issue frames at the DMG rate instead of one-per-round-trip.
    ///
    /// This is what the frontend used to do: write a picture every 16.743 ms
    /// and never ask whether the terminal kept up. It is the *measurement* of
    /// the bug — with `--fence-every N` the fence latency shows how far behind
    /// the terminal has fallen, which is invisible from our side otherwise.
    free_run: bool,
    /// Fence (a CPR round trip) once every N frames. 1 = every frame, which is
    /// the self-pacing behaviour; larger values sample the terminal's backlog
    /// without pacing to it.
    fence_every: u32,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            rom: None,
            frames: 180,
            sizes: Vec::new(),
            // WezTerm's usual cell on a 1080p display.
            font: (8, 16),
            upscale: None,
            palette: Palette::Green,
            protocol: ProtocolType::Kitty,
            path: None,
            json: false,
            ndjson: false,
            out: "/dev/null".to_string(),
            motion: false,
            tty: false,
            probe: false,
            state: None,
            walk: false,
            free_run: false,
            fence_every: 1,
        }
    }
}

fn main() {
    let args = parse_args();
    if args.probe {
        tty_bench::probe();
        return;
    }
    let rom = load_rom(args.rom.as_deref());

    if args.tty {
        tty_bench::run(&args, &rom);
        return;
    }

    // Matrix: window sizes × upscaler settings × render paths.
    let sizes: Vec<(u16, u16)> = if args.sizes.is_empty() {
        vec![(80, 24), (120, 40), (213, 55), (320, 90)]
    } else {
        args.sizes.clone()
    };
    let settings: Vec<(Algorithm, u8)> = match args.upscale {
        Some(s) => vec![s],
        None => vec![
            (Algorithm::Nearest, 1),
            (Algorithm::Nearest, 4),
            (Algorithm::Mmpx, 2),
            (Algorithm::LcdGrid, 4),
        ],
    };
    let paths: Vec<&'static str> = match args.path {
        Some(p) => vec![p],
        None => vec!["legacy", "capped"],
    };

    let mut rows = Vec::new();
    let mut last_summary = String::new();
    for &(cols, term_rows) in &sizes {
        for &(algo, factor) in &settings {
            for &path in &paths {
                let r = run_case(&args, &rom, cols, term_rows, algo, factor, path);
                last_summary = r.summary.clone();
                rows.push(r);
            }
        }
    }

    if args.json {
        // One-shot contract: exactly one JSON object on stdout when a single
        // case was selected, otherwise one per case (still one per line).
        if rows.len() == 1 {
            println!("{last_summary}");
        } else {
            for r in &rows {
                println!("{}", r.summary);
            }
        }
        return;
    }
    if args.ndjson {
        return; // per-frame records already streamed
    }
    print_table(&rows, &args);
}

struct CaseResult {
    cols: u16,
    rows: u16,
    algo: Algorithm,
    factor: u8,
    path: &'static str,
    disp_px: (u32, u32),
    tx_px: (u32, u32),
    emulate_ms: f64,
    palette_ms: f64,
    upscale_ms: f64,
    encode_ms: f64,
    write_ms: f64,
    tx_bytes: f64,
    /// Fraction of frames the encoder had to re-encode (the rest were
    /// byte-identical to the previous frame and cost nothing).
    changed_pct: f64,
    /// Frames per second the *display* path could sustain if it were the only
    /// cost (1000 / (palette+upscale+encode+write)), and the whole-loop figure
    /// including emulation.
    render_fps: f64,
    total_fps: f64,
    summary: String,
}

#[allow(clippy::too_many_arguments)]
fn run_case(
    args: &Args,
    rom: &[u8],
    cols: u16,
    term_rows: u16,
    algo: Algorithm,
    factor: u8,
    path: &'static str,
) -> CaseResult {
    let mut gb = Gameboy::new(rom.to_vec(), None);
    apply_state(&mut gb, args.state.as_deref());
    let mut prep = FramePrep::new();
    let mut changed_frames = 0u32;
    let mut upscaler = Upscaler::new(algo, factor);
    let (font_w, font_h) = args.font;

    // The display rectangle the TUI would choose for this window (game panel =
    // terminal minus the 30-cell sidebar, minus header/footer rows).
    let panel = Rect::new(
        0,
        0,
        cols.saturating_sub(30).max(1),
        term_rows.saturating_sub(2).max(1),
    );
    let (rect, scale) = fit_metrics(panel, SRC_W, SRC_H, font_w, font_h);
    let disp_px = (
        rect.width as u32 * font_w as u32,
        rect.height as u32 * font_h as u32,
    );
    let (tx_w, tx_h) = upscaler.output_size(SRC_W as usize, SRC_H as usize);
    let tx_px = (tx_w as u32, tx_h as u32);

    let wire = match args.protocol {
        ProtocolType::Iterm2 => Wire::Iterm2,
        _ => Wire::Kitty,
    };
    let cell_rect = CellRect::new(rect.x, rect.y, rect.width, rect.height);
    let mut term = TermImage::new(wire, 1);
    let mut picker = Picker::from_fontsize((font_w, font_h));
    picker.set_protocol_type(args.protocol);
    picker.set_background_color([0, 0, 0, 0]);

    let mut sink = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)
        .unwrap_or_else(|e| panic!("cannot open --out {}: {e}", args.out));

    let mut ctx = Context {
        rom_title: gb.rom_title(),
        mbc: String::new(),
        frontend: format!("bench/{path}"),
        protocol: format!("{:?}", args.protocol).to_lowercase(),
        palette: args.palette.name().to_string(),
        upscale: algo.name().to_string(),
        upscale_factor: factor,
        scale,
        speed: "1x".to_string(),
        term_cols: cols,
        term_rows,
        display_cols: rect.width,
        display_rows: rect.height,
        tx_w: tx_px.0,
        tx_h: tx_px.1,
        display_px_w: disp_px.0,
        display_px_h: disp_px.1,
    };
    let mut tele = if args.ndjson {
        Telemetry::open(&telemetry::Destination::Stdout).0
    } else {
        Telemetry::in_memory()
    };

    // Warm-up: get past the boot logo so frames actually change, and let every
    // buffer reach its steady-state size. A loaded state is already past it.
    if args.state.is_none() {
        for _ in 0..30 {
            gb.frame();
        }
    }
    let warm = prep.map(gb.image(), args.palette).to_vec();
    let warm_up = upscaler
        .apply(&warm, SRC_W as usize, SRC_H as usize)
        .to_vec();
    let mut legacy_proto = picker.new_resize_protocol(image::DynamicImage::ImageRgba8(
        image::ImageBuffer::from_raw(tx_px.0, tx_px.1, warm_up.clone()).unwrap(),
    ));
    legacy_proto.resize_encode(&Resize::Scale(None), [0, 0, 0, 0].into(), rect);
    let mut buf = Buffer::empty(rect);
    let _ = term.place(&warm_up, tx_px.0, tx_px.1, cell_rect);
    term.invalidate();

    for n in 0..args.frames {
        let loop_start = Instant::now();

        if args.walk {
            walk_input(&mut gb, n);
        }
        let t = Instant::now();
        gb.frame();
        let emulate = t.elapsed();

        let t = Instant::now();
        let mapped = prep.map_with(gb.image(), args.palette, |rgba| {
            if args.motion {
                // One pixel toggled per frame: guarantees the frame differs
                // from the last, which is the worst case for the encoder.
                rgba[0] = rgba[0].wrapping_add(n as u8);
            }
        });
        let palette_ms = t.elapsed();

        let t = Instant::now();
        let scaled = upscaler.apply(mapped, SRC_W as usize, SRC_H as usize);
        let upscale_ms = t.elapsed();

        let (encode, write, bytes) = match path {
            "legacy" => {
                // Exactly the old frontend: build a fresh protocol from the
                // frame (which hashes it), then resize to the *display*
                // resolution and encode.
                let t = Instant::now();
                let img = image::DynamicImage::ImageRgba8(
                    image::ImageBuffer::from_raw(tx_px.0, tx_px.1, scaled.to_vec())
                        .expect("frame dimensions"),
                );
                let mut proto = picker.new_resize_protocol(img);
                proto.resize_encode(&Resize::Scale(None), [0, 0, 0, 0].into(), rect);
                buf.reset();
                proto.render(rect, &mut buf);
                let encode = t.elapsed();
                changed_frames += 1;
                let payload: Vec<&str> = buf
                    .content()
                    .iter()
                    .map(|c| c.symbol())
                    .filter(|s| s.len() > 1)
                    .collect();
                let bytes: usize = payload.iter().map(|s| s.len()).sum();
                let t = Instant::now();
                for s in payload {
                    let _ = sink.write_all(s.as_bytes());
                }
                (encode, t.elapsed(), bytes)
            }
            _ => {
                let t = Instant::now();
                let placement = term.place(scaled, tx_px.0, tx_px.1, cell_rect);
                let encode = t.elapsed();
                let bytes = placement.payload_bytes;
                changed_frames += u32::from(placement.changed);
                let t = Instant::now();
                if placement.changed {
                    let _ = sink.write_all(placement.seq.as_bytes());
                }
                (encode, t.elapsed(), bytes)
            }
        };

        let frame_ms = telemetry::ms(loop_start.elapsed());
        let sample = FrameSample {
            frame: n as u64,
            fps: 1000.0 / frame_ms.max(f32::EPSILON),
            emulated_fps: DMG_HZ,
            speed_pct: 100.0,
            frame_ms,
            emulate_ms: telemetry::ms(emulate),
            palette_ms: telemetry::ms(palette_ms),
            upscale_ms: telemetry::ms(upscale_ms),
            encode_ms: telemetry::ms(encode),
            write_ms: telemetry::ms(write),
            frames_skipped: 0,
            // The real count, not a zero. Reporting 0 here is what made
            // unchanged-frame skipping look broken when it was in fact doing
            // almost all of the work on a static screen: `tx_bytes_avg` had
            // already collapsed to single digits while this field still said
            // "no frames were identical".
            frames_identical: (n + 1 - changed_frames) as u64,
            frames_dropped: 0,
            tx_bytes: bytes,
            frames_issued: changed_frames as u64,
            // Headless: nothing was written to a terminal, so there is no
            // terminal cost to report. `--tty` is the mode that measures it.
            terminal_ms: None,
            terminal_ceiling_fps: None,
            audio: None::<AudioSample>,
            video: None,
            // This bench has no agent; the field exists so a run with one can
            // be compared against a run without.
            agent: None,
        };
        tele.frame(&ctx, &sample);
    }
    tele.flush();

    ctx.mbc = String::new();
    let summary = tele.summary_json(&ctx);
    let get = |key: &str| json_number(&summary, key);
    let emulate_ms = get("emulate_ms_avg");
    let palette_ms = get("palette_ms_avg");
    let upscale_ms = get("upscale_ms_avg");
    let encode_ms = get("encode_ms_avg");
    let write_ms = get("write_ms_avg");
    let render_cost = palette_ms + upscale_ms + encode_ms + write_ms;
    CaseResult {
        cols,
        rows: term_rows,
        algo,
        factor,
        path,
        disp_px,
        tx_px,
        emulate_ms,
        palette_ms,
        upscale_ms,
        encode_ms,
        write_ms,
        tx_bytes: get("tx_bytes_avg"),
        changed_pct: changed_frames as f64 / args.frames.max(1) as f64 * 100.0,
        render_fps: if render_cost > 0.0 {
            1000.0 / render_cost
        } else {
            f64::INFINITY
        },
        total_fps: 1000.0 / get("frame_ms_avg").max(1e-6),
        summary,
    }
}

fn print_table(rows: &[CaseResult], args: &Args) {
    println!(
        "terminalgb render benchmark — {} frames per case, cell {}x{}px, protocol {:?}",
        args.frames, args.font.0, args.font.1, args.protocol
    );
    println!(
        "'fps(disp)' is what the display path alone could sustain; 'fps(all)' includes emulation.\n\
         A frame budget of 16.743 ms is 59.7275 fps (true DMG).\n"
    );
    println!(
        "{:<10} {:>11} {:>12} {:>11} {:>7} {:>7} {:>7} {:>7} {:>7} {:>9} {:>6} {:>9} {:>9}",
        "window",
        "display px",
        "upscale",
        "sent px",
        "emu",
        "pal",
        "scale",
        "encode",
        "write",
        "KB/frame",
        "chg%",
        "fps(disp)",
        "fps(all)"
    );
    let mut last_path = "";
    for r in rows {
        if r.path != last_path {
            println!("-- {} --", r.path);
            last_path = r.path;
        }
        println!(
            "{:<10} {:>11} {:>12} {:>11} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>7.2} {:>9.1} {:>6.0} {:>9.1} {:>9.1}",
            format!("{}x{}", r.cols, r.rows),
            format!("{}x{}", r.disp_px.0, r.disp_px.1),
            format!("{} x{}", r.algo.name(), r.factor),
            format!("{}x{}", r.tx_px.0, r.tx_px.1),
            r.emulate_ms,
            r.palette_ms,
            r.upscale_ms,
            r.encode_ms,
            r.write_ms,
            r.tx_bytes / 1024.0,
            r.changed_pct,
            r.render_fps,
            r.total_fps,
        );
    }
}

// ---------------------------------------------------------------------------
// Real-terminal benchmark
// ---------------------------------------------------------------------------

/// Drives the controlling terminal for real and measures the frame rate it can
/// actually sustain, using a Device Status Report round-trip per frame as the
/// barrier. See the module docs for why this exists.
mod tty_bench {
    use super::{apply_state, fit_metrics, walk_input, Args, DMG_HZ, SRC_H, SRC_W};

    use terminalgb::gameboy::Gameboy;
    use terminalgb::screen::frame::FramePrep;
    use terminalgb::termimage::{CellRect, TermImage, Wire};
    use terminalgb::upscale::{Algorithm, Upscaler};

    use std::fs::File;
    use std::io::{Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    use ratatui::layout::Rect;
    use ratatui_image::picker::ProtocolType;

    /// How long to wait for a terminal to answer `CSI 6 n` before giving up on
    /// synchronisation. Generous: a terminal that is genuinely 6 fps takes
    /// 170 ms to answer, and that is exactly the case we are here to catch.
    const DSR_TIMEOUT: Duration = Duration::from_millis(2000);
    /// How long the terminal may refuse to drain its pty before we call it
    /// saturated and stop.
    const STALL_TIMEOUT: Duration = Duration::from_millis(3000);

    struct RawTty {
        f: File,
        saved: libc::termios,
    }

    impl RawTty {
        fn open() -> Result<RawTty, String> {
            let f = File::options()
                .read(true)
                .write(true)
                .open("/dev/tty")
                .map_err(|e| format!("cannot open /dev/tty: {e}"))?;
            let fd = f.as_raw_fd();
            if unsafe { libc::isatty(fd) } != 1 {
                return Err("/dev/tty is not a terminal".to_string());
            }
            // Job control, the two ways it bites a program that wants to both
            // write to and *read from* its terminal:
            //
            // * `tcsetattr` from outside the terminal's foreground process
            //   group raises SIGTTOU, which **stops** us — and a stopped
            //   process does not act on SIGTERM, so the run hangs in a way even
            //   `timeout` cannot clear.
            // * `read` from outside it raises SIGTTIN, and if SIGTTIN is merely
            //   ignored the read fails with EIO instead. Every reply then looks
            //   like "the terminal did not answer", which is a lie.
            //
            // So: ignore SIGTTOU (that makes `tcsetpgrp` succeed rather than
            // stop us), claim the terminal, and leave SIGTTIN alone.
            unsafe {
                libc::signal(libc::SIGTTOU, libc::SIG_IGN);
                if libc::tcgetpgrp(fd) != libc::getpgrp() {
                    libc::tcsetpgrp(fd, libc::getpgrp());
                }
            }
            let mut saved: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut saved) } != 0 {
                return Err("tcgetattr failed".to_string());
            }
            let mut raw = saved;
            unsafe { libc::cfmakeraw(&mut raw) };
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
                return Err("tcsetattr failed".to_string());
            }
            // Non-blocking writes. A terminal that cannot keep up stops draining
            // its pty, and a blocking `write` of a 91 KB frame then parks the
            // process somewhere `timeout` cannot reach it — the run hangs
            // instead of *reporting* the saturation, which is the one thing this
            // mode exists to measure.
            unsafe {
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
            }
            Ok(RawTty { f, saved })
        }

        /// Write every byte, waiting for the terminal to make room. Returns
        /// `false` if it stopped draining for [`STALL_TIMEOUT`], which means it
        /// is saturated — the honest answer for what we are measuring.
        fn write_all_timed(&mut self, mut bytes: &[u8]) -> bool {
            let fd = self.f.as_raw_fd();
            let deadline = Instant::now() + STALL_TIMEOUT;
            while !bytes.is_empty() {
                match self.f.write(bytes) {
                    Ok(0) => return false,
                    Ok(n) => bytes = &bytes[n..],
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        let left = deadline.saturating_duration_since(Instant::now());
                        if left.is_zero() {
                            return false;
                        }
                        let mut p = libc::pollfd {
                            fd,
                            events: libc::POLLOUT,
                            revents: 0,
                        };
                        if unsafe { libc::poll(&mut p, 1, left.as_millis() as i32) } <= 0
                        {
                            return false;
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => return false,
                }
            }
            true
        }

        fn size(&self) -> (u16, u16, u16, u16) {
            let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
            unsafe { libc::ioctl(self.f.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) };
            let (cols, rows) = (ws.ws_col.max(1), ws.ws_row.max(1));
            // Cell size in pixels, when the terminal reports it.
            let cw = if ws.ws_col > 0 && ws.ws_xpixel > 0 {
                ws.ws_xpixel / ws.ws_col
            } else {
                0
            };
            let ch = if ws.ws_row > 0 && ws.ws_ypixel > 0 {
                ws.ws_ypixel / ws.ws_row
            } else {
                0
            };
            (cols, rows, cw, ch)
        }

        /// Write `query` and collect whatever the terminal says back, stopping
        /// when it goes quiet. Returns `None` if it never answered.
        ///
        /// A cursor-position request is appended as a **fence**: CPR is
        /// answered by everything, and it is answered *after* whatever the
        /// query itself produced, so "silence" is a real answer rather than an
        /// impatient reader. Without it a terminal that simply ignores an
        /// unsupported query is indistinguishable from a slow one.
        fn ask(&mut self, query: &[u8]) -> Option<String> {
            let mut probe = query.to_vec();
            probe.extend_from_slice(b"\x1b[6n");
            if !self.write_all_timed(&probe) {
                return None;
            }
            let fd = self.f.as_raw_fd();
            let deadline = Instant::now() + DSR_TIMEOUT;
            let mut got = String::new();
            let mut buf = [0u8; 512];
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                let mut p = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                if unsafe { libc::poll(&mut p, 1, left.as_millis() as i32) } <= 0 {
                    break;
                }
                match self.f.read(&mut buf) {
                    Ok(k) if k > 0 => {
                        got.push_str(&String::from_utf8_lossy(&buf[..k]));
                        // The CPR fence has arrived: everything the query was
                        // going to produce is already in `got`.
                        if got.contains('R') {
                            break;
                        }
                    }
                    _ => break,
                }
            }
            if got.is_empty() {
                None
            } else {
                Some(got)
            }
        }

        /// Send a cursor-position report request and block until the terminal
        /// answers. Returns the round-trip time, or `None` on timeout.
        ///
        /// `CSI 6 n` rather than `CSI 5 n`: CPR is answered by every terminal
        /// worth measuring, while DSR-5 is optional and WezTerm — the terminal
        /// this whole investigation is about — does not answer it.
        fn sync(&mut self) -> Option<Duration> {
            let t = Instant::now();
            if !self.write_all_timed(b"\x1b[6n") {
                return None;
            }
            let fd = self.f.as_raw_fd();
            let mut buf = [0u8; 64];
            loop {
                let mut p = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let left = DSR_TIMEOUT.saturating_sub(t.elapsed());
                if left.is_zero() {
                    return None;
                }
                let n = unsafe { libc::poll(&mut p, 1, left.as_millis() as i32) };
                if n <= 0 {
                    return None;
                }
                match self.f.read(&mut buf) {
                    // The reply is `CSI <row>;<col> R`; 'R' terminates it and
                    // no other byte of the reply is 'R'.
                    Ok(k) if k > 0 => {
                        if buf[..k].contains(&b'R') {
                            return Some(t.elapsed());
                        }
                    }
                    _ => return None,
                }
            }
        }
    }

    impl Drop for RawTty {
        fn drop(&mut self) {
            unsafe {
                let fd = self.f.as_raw_fd();
                let fl = libc::fcntl(fd, libc::F_GETFL);
                libc::fcntl(fd, libc::F_SETFL, fl & !libc::O_NONBLOCK);
            }
            let _ = self.f.write_all(b"\x1b_Ga=d\x1b\\\x1b[?25h\x1b[2J\x1b[H");
            let _ = self.f.flush();
            unsafe { libc::tcsetattr(self.f.as_raw_fd(), libc::TCSANOW, &self.saved) };
        }
    }

    /// Ask the terminal what it can actually do, and print the answers.
    ///
    /// This exists because guessing cost a previous investigation real time:
    /// WezTerm implements the Kitty graphics protocol but **not** its Unicode
    /// placeholders, so the placeholder path renders as missing glyphs and the
    /// obvious conclusion ("WezTerm can't do Kitty") is wrong. The protocol has
    /// a handshake — use it.
    pub fn probe() {
        let mut tty = match RawTty::open() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("--probe needs a real terminal: {e}");
                std::process::exit(3);
            }
        };
        let (cols, rows, cw, ch) = tty.size();
        println!("terminal: {cols}x{rows} cells, cell {cw}x{ch}px");
        for (k, v) in [
            ("TERM", "TERM"),
            ("TERM_PROGRAM", "TERM_PROGRAM"),
            ("TERM_PROGRAM_VERSION", "TERM_PROGRAM_VERSION"),
        ] {
            println!("{k} = {}", std::env::var(v).unwrap_or_default());
        }

        // Primary device attributes: proves the terminal answers us at all, and
        // separates "no support" from "no reply".
        println!("DA1        : {}", tty.ask(b"\x1b[c").unwrap_or_default());

        // Kitty a=q handshakes, one per transmission medium.
        let direct = format!("\x1b_Ga=q,i={},f=24,t=d,s=1,v=1;AAAA\x1b\\", 91);
        println!("kitty t=d  : {}", describe(tty.ask(direct.as_bytes()), 91));
        match terminalgb::termimage::shm_query(92) {
            Some(q) => {
                let reply = tty.ask(q.seq.as_bytes());
                println!("kitty t=s  : {}", describe(reply, 92));
                q.finish();
            }
            None => println!("kitty t=s  : unavailable in this build/platform"),
        }
        // Unicode placeholders have no handshake of their own; the protocol
        // reports support for the *transmission*, and placement support has to
        // be seen. Say so rather than implying we tested it.
        println!("placeholders: no protocol handshake exists — must be eyeballed");
    }

    fn describe(reply: Option<String>, id: u32) -> String {
        match reply {
            None => "no reply (unsupported or swallowed by a multiplexer)".into(),
            Some(r) if terminalgb::termimage::shm_reply_ok(&r, id) => {
                format!("OK  [{}]", escape(&r))
            }
            Some(r) => format!("not OK  [{}]", escape(&r)),
        }
    }

    fn escape(s: &str) -> String {
        s.chars()
            .map(|c| {
                if c == '\x1b' {
                    "ESC".to_string()
                } else if c.is_control() {
                    format!("\\x{:02x}", c as u32)
                } else {
                    c.to_string()
                }
            })
            .collect()
    }

    pub fn run(args: &Args, rom: &[u8]) {
        let mut tty = match RawTty::open() {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "--tty needs a real terminal: {e}\n\
                     Run it directly from a graphics-capable terminal (not under a \
                     pipe, and not inside tmux, which does not pass graphics \
                     sequences through)."
                );
                std::process::exit(3);
            }
        };
        // Probe: if the terminal will not answer DSR there is nothing to
        // measure, and silently timing our own writes is exactly the lie this
        // mode exists to stop telling.
        tty.write_all_timed(b"\x1b[?25l\x1b[2J");
        if tty.sync().is_none() {
            drop(tty);
            eprintln!(
                "terminal did not answer CSI 6 n — cannot measure it honestly \
                 (foreground pgrp {}, ours {}; a mismatch means reads fail with \
                 EIO and every reply looks like a timeout)",
                unsafe { libc::tcgetpgrp(libc::STDIN_FILENO) },
                unsafe { libc::getpgrp() },
            );
            std::process::exit(3);
        }

        // Read the window size only **after** the terminal has answered, never
        // before.
        //
        // A window asked for more cells than the screen holds is clamped, and a
        // terminal launched for this process reports the size it was *asked*
        // for until it has done that. Sizing the placement from the early value
        // asks the terminal to composite an image mostly off the screen, whose
        // cost then rises with a rectangle nobody can see — which is a
        // measurement of this benchmark, not of the terminal. (Measured: a
        // "640×176" window is really 268×60 here, and the difference was a
        // reported 30 ms/frame that is not real.)
        let (cols, rows, cw, ch) = tty.size();
        let (font_w, font_h) = if cw > 0 && ch > 0 {
            (cw, ch)
        } else {
            args.font
        };

        let wire = match args.protocol {
            ProtocolType::Iterm2 => Wire::Iterm2,
            _ => Wire::Kitty,
        };
        let (algo, factor) = args.upscale.unwrap_or((Algorithm::Nearest, 1));
        let mut gb = Gameboy::new(rom.to_vec(), None);
        apply_state(&mut gb, args.state.as_deref());
        let mut prep = FramePrep::new();
        let mut upscaler = Upscaler::new(algo, factor);
        let (tx_w, tx_h) = upscaler.output_size(SRC_W as usize, SRC_H as usize);
        let (rect, scale) =
            fit_metrics(Rect::new(0, 0, cols, rows), SRC_W, SRC_H, font_w, font_h);
        let mut term = TermImage::new(wire, 0x0067_6202);
        // Choose the transport the way the frontend does: ask the terminal
        // whether it can read shared memory, and only then use it.
        let mut shm_asked = false;
        if wire == Wire::Kitty && std::env::var("GB_TERM_KITTY").as_deref() != Ok("noshm")
        {
            if let Some(q) = terminalgb::termimage::shm_query(0x5348) {
                shm_asked = true;
                let reply = tty.ask(q.seq.as_bytes()).unwrap_or_default();
                if terminalgb::termimage::shm_reply_ok(&reply, 0x5348) {
                    term.set_kitty_shm(true);
                }
                q.finish();
            }
        }
        let _ = shm_asked;
        let at = CellRect::new(rect.x, rect.y, rect.width, rect.height);

        if args.state.is_none() {
            for _ in 0..30 {
                gb.frame();
            }
        }

        let mut encode_total = Duration::ZERO;
        let mut write_total = Duration::ZERO;
        let mut sync_total = Duration::ZERO;
        let mut bytes_total = 0usize;
        let mut identical = 0u32;
        let mut timeouts = 0u32;
        let mut stalled = false;
        let mut done = 0u32;
        let mut fences = 0u32;
        let mut sync_max = Duration::ZERO;
        let fence_every = args.fence_every.max(1);
        // `--free-run` issues pictures on the DMG clock and only *samples* the
        // terminal, so the fence latency measures the backlog we built instead
        // of pacing it away.
        let period = Duration::from_micros(16_743);
        let mut next_issue = Instant::now();
        let start = Instant::now();
        for n in 0..args.frames {
            if args.free_run {
                next_issue += period;
                let now = Instant::now();
                if next_issue > now {
                    std::thread::sleep(next_issue - now);
                } else {
                    next_issue = now;
                }
            }
            if args.walk {
                walk_input(&mut gb, n);
            }
            gb.frame();
            let mapped = prep.map_with(gb.image(), args.palette, |rgba| {
                if args.motion {
                    rgba[0] = rgba[0].wrapping_add(n as u8);
                }
            });
            let scaled = upscaler.apply(mapped, SRC_W as usize, SRC_H as usize);

            let t = Instant::now();
            let p = term.place(scaled, tx_w as u32, tx_h as u32, at);
            encode_total += t.elapsed();
            bytes_total += p.payload_bytes;
            if !p.changed {
                identical += 1;
            }
            let t = Instant::now();
            let wrote = !p.changed || tty.write_all_timed(p.seq.as_bytes());
            write_total += t.elapsed();
            if !wrote {
                stalled = true;
                done = n + 1;
                break;
            }
            if (n + 1) % fence_every == 0 {
                match tty.sync() {
                    Some(d) => {
                        sync_total += d;
                        sync_max = sync_max.max(d);
                        fences += 1;
                    }
                    None => timeouts += 1,
                }
            }
            done = n + 1;
        }
        // A final fence after a free run: everything still queued in the
        // terminal has to be worked through before it can answer, so this is
        // the backlog we built by issuing faster than it could accept.
        let drain = if args.free_run {
            tty.sync().unwrap_or(Duration::ZERO)
        } else {
            Duration::ZERO
        };
        let wall = start.elapsed();
        drop(tty);

        let f = done.max(1) as f64;
        let ms = |d: Duration| d.as_secs_f64() * 1000.0 / f;
        let fence_ms = if fences > 0 {
            sync_total.as_secs_f64() * 1000.0 / fences as f64
        } else {
            0.0
        };
        let achieved = f / wall.as_secs_f64();
        let payload = if term.kitty_shm() {
            "shm"
        } else if std::env::var("GB_TERM_IMAGE").as_deref() == Ok("rgb") {
            "rgb"
        } else {
            "png"
        };
        let line = format!(
            "{{\"schema\":1,\"type\":\"tty\",\"protocol\":\"{:?}\",\"payload\":\"{payload}\",\
             \"term_cols\":{cols},\"term_rows\":{rows},\"cell_px\":\"{font_w}x{font_h}\",\
             \"display_cols\":{},\"display_rows\":{},\"scale\":{scale:.3},\
             \"tx_w\":{tx_w},\"tx_h\":{tx_h},\"frames\":{},\
             \"terminal_saturated\":{stalled},\"achieved_fps\":{achieved:.2},\"dmg_fps\":{DMG_HZ:.3},\
             \"tx_bytes_avg\":{:.0},\"tx_mb_per_s\":{:.3},\
             \"encode_ms_avg\":{:.3},\"write_ms_avg\":{:.3},\"sync_ms_avg\":{:.3},\
             \"free_run\":{},\"fence_every\":{fence_every},\"fences\":{fences},\
             \"fence_ms_avg\":{fence_ms:.3},\"fence_ms_max\":{:.3},\"drain_ms\":{:.3},\
             \"frames_identical\":{identical},\"dsr_timeouts\":{timeouts}}}",
            args.protocol,
            rect.width,
            rect.height,
            done,
            bytes_total as f64 / f,
            bytes_total as f64 / wall.as_secs_f64() / 1_048_576.0,
            ms(encode_total),
            ms(write_total),
            ms(sync_total),
            args.free_run,
            sync_max.as_secs_f64() * 1000.0,
            drain.as_secs_f64() * 1000.0,
        );
        println!("{line}");
        if !args.json {
            eprintln!(
                "\nreal terminal: {achieved:.1} fps achieved ({:.1} % of DMG's \
                 {DMG_HZ:.2}), {:.1} KB/frame, encode {:.3} ms, \
                 write+sync {:.2} ms",
                achieved / DMG_HZ as f64 * 100.0,
                bytes_total as f64 / f / 1024.0,
                ms(encode_total),
                ms(write_total) + ms(sync_total),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Copy of the frontend's fit computation (see `screen::tui::fit_metrics`),
/// duplicated here rather than made public because it is three lines of maths
/// and the frontend's copy is the one under test elsewhere.
fn fit_metrics(
    area: Rect,
    img_w: u32,
    img_h: u32,
    font_w: u16,
    font_h: u16,
) -> (Rect, f32) {
    let avail_w = area.width as f64 * font_w as f64;
    let avail_h = area.height as f64 * font_h as f64;
    let scale = (avail_w / img_w as f64)
        .min(avail_h / img_h as f64)
        .max(0.0);
    let cols = (((img_w as f64 * scale) / font_w as f64).round() as u16)
        .clamp(1, area.width.max(1));
    let rows = (((img_h as f64 * scale) / font_h as f64).round() as u16)
        .clamp(1, area.height.max(1));
    (Rect::new(0, 0, cols, rows), scale as f32)
}

/// Pull a numeric field out of one of our own JSON summaries (the bench is the
/// only consumer, and the format is ours).
fn json_number(json: &str, key: &str) -> f64 {
    let needle = format!("\"{key}\":");
    let Some(start) = json.find(&needle) else {
        return 0.0;
    };
    let rest = &json[start + needle.len()..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(rest.len());
    rest[..end].parse().unwrap_or(0.0)
}

/// Apply a save state, if one was asked for. A state that will not load is a
/// hard error: silently measuring the title screen instead of the gameplay the
/// operator asked for is exactly the kind of flattering number this tool exists
/// to stop producing.
fn apply_state(gb: &mut Gameboy, path: Option<&str>) {
    let Some(path) = path else { return };
    let bytes = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("cannot read --state {path}: {e}");
        std::process::exit(2);
    });
    if let Err(e) = gb.load_state(&bytes) {
        eprintln!("cannot load --state {path}: {e:?}");
        std::process::exit(2);
    }
}

/// Walking input: hold one direction for a stretch, then the opposite, so the
/// character keeps moving instead of walking into a wall and standing still.
/// 48 frames is about three tiles at the overworld's walking speed.
fn walk_input(gb: &mut Gameboy, frame: u32) {
    use terminalgb::KeypadKey;
    const LEG: u32 = 48;
    let dirs = [
        KeypadKey::Right,
        KeypadKey::Down,
        KeypadKey::Left,
        KeypadKey::Up,
    ];
    let leg = (frame / LEG) as usize % dirs.len();
    if frame % LEG == 0 {
        for d in dirs {
            gb.keyup(d);
        }
        gb.keydown(dirs[leg]);
    }
}

fn load_rom(path: Option<&str>) -> Vec<u8> {
    let path = path.unwrap_or("third_party/dmg-acid2/dmg-acid2.gb");
    std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("cannot read ROM {path}: {e}");
        std::process::exit(2);
    })
}

fn parse_args() -> Args {
    let mut a = Args::default();
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < argv.len() {
        let next = |i: usize| -> String {
            argv.get(i + 1).cloned().unwrap_or_else(|| {
                eprintln!("missing value for {}", argv[i]);
                std::process::exit(2);
            })
        };
        match argv[i].as_str() {
            "--rom" => {
                a.rom = Some(next(i));
                i += 1;
            }
            "--frames" => {
                a.frames = next(i).parse().unwrap_or(180);
                i += 1;
            }
            "--cols" => {
                let cols: u16 = next(i).parse().unwrap_or(213);
                a.sizes.push((cols, 0));
                i += 1;
            }
            "--rows" => {
                let rows: u16 = next(i).parse().unwrap_or(55);
                if let Some(last) = a.sizes.last_mut() {
                    last.1 = rows;
                }
                i += 1;
            }
            "--font" => {
                let v = next(i);
                if let Some((w, h)) = v.split_once('x') {
                    a.font = (w.parse().unwrap_or(8), h.parse().unwrap_or(16));
                }
                i += 1;
            }
            "--upscale" => {
                let v = next(i);
                let algo = Algorithm::from_str(&v).unwrap_or(Algorithm::Nearest);
                a.upscale = Some((algo, algo.clamp_factor(a.upscale.map_or(1, |u| u.1))));
                i += 1;
            }
            "--factor" => {
                let f: u8 = next(i).parse().unwrap_or(1);
                let algo = a.upscale.map_or(Algorithm::Nearest, |u| u.0);
                a.upscale = Some((algo, algo.clamp_factor(f)));
                i += 1;
            }
            "--palette" => {
                a.palette = Palette::from_str(&next(i)).unwrap_or(Palette::Green);
                i += 1;
            }
            "--protocol" => {
                a.protocol = match next(i).to_ascii_lowercase().as_str() {
                    "iterm2" | "iterm" => ProtocolType::Iterm2,
                    "sixel" => ProtocolType::Sixel,
                    "halfblocks" => ProtocolType::Halfblocks,
                    _ => ProtocolType::Kitty,
                };
                i += 1;
            }
            "--path" => {
                a.path = Some(match next(i).as_str() {
                    "legacy" => "legacy",
                    _ => "capped",
                });
                i += 1;
            }
            "--out" => {
                a.out = next(i);
                i += 1;
            }
            "--motion" => a.motion = true,
            "--tty" => a.tty = true,
            "--free-run" => a.free_run = true,
            "--fence-every" => {
                a.fence_every = next(i).parse().unwrap_or(1);
                i += 1;
            }
            "--probe" => a.probe = true,
            "--walk" => a.walk = true,
            "--state" => {
                a.state = Some(next(i));
                i += 1;
            }
            "--json" => a.json = true,
            "--ndjson" => a.ndjson = true,
            "-h" | "--help" => {
                println!(
                    "render_bench [--rom PATH] [--frames N] [--cols C --rows R]...\n\
                                 [--font WxH] [--upscale ALGO --factor N] [--palette P]\n\
                                 [--protocol kitty|iterm2] [--path legacy|capped]\n\
                                 [--out FILE] [--motion] [--json] [--ndjson]\n\
                                 [--tty]   drive the real terminal and measure it\n\
                                 [--probe] ask the terminal what it supports\n\
                                 [--state FILE] load a save state first\n\
                                 [--walk]  hold walking directions (real motion)"
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
    a.sizes.retain(|(c, r)| *c > 0 && *r > 0);
    a
}

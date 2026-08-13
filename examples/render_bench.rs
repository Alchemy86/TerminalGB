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
//! * `capped` — [`gameboy::termimage`]: transmit at the upscaler's own output
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

use gameboy::gameboy::Gameboy;
use gameboy::screen::config::Palette;
use gameboy::screen::frame::FramePrep;
use gameboy::telemetry::{self, AudioSample, Context, FrameSample, Telemetry};
use gameboy::termimage::{CellRect, TermImage, Wire};
use gameboy::upscale::{Algorithm, Upscaler};

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
        }
    }
}

fn main() {
    let args = parse_args();
    let rom = load_rom(args.rom.as_deref());

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
    // buffer reach its steady-state size.
    for _ in 0..30 {
        gb.frame();
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
            frames_identical: 0,
            frames_dropped: 0,
            tx_bytes: bytes,
            audio: None::<AudioSample>,
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
        "gameboy render benchmark — {} frames per case, cell {}x{}px, protocol {:?}",
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
            "--json" => a.json = true,
            "--ndjson" => a.ndjson = true,
            "-h" | "--help" => {
                println!(
                    "render_bench [--rom PATH] [--frames N] [--cols C --rows R]...\n\
                                 [--font WxH] [--upscale ALGO --factor N] [--palette P]\n\
                                 [--protocol kitty|iterm2] [--path legacy|capped]\n\
                                 [--out FILE] [--motion] [--json] [--ndjson]"
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

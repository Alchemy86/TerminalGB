//! Render one real emulated frame through every upscaling algorithm/factor,
//! write the results as PNGs, and measure per-frame cost — the evidence pack
//! for choosing an upscaler by eye.
//!
//! ```text
//! cargo run --release --example upscale_gallery                 # dmg-acid2, 600 frames
//! cargo run --release --example upscale_gallery -- path/to.gb --frames 900
//! cargo run --release --example upscale_gallery -- --out docs/upscale-samples
//! ```
//!
//! Output (default `target/upscale-gallery/<rom-stem>/`):
//! * `00-source-x1.png` — the raw 160×144 frame,
//! * one PNG per algorithm × factor (`nearest-x4.png`, `mmpx-x4.png`, …),
//! * `compare-x4.png` — nearest | scale2x | mmpx | lcd side by side,
//! * green-palette variants (`*-green.png`) for the DMG-LCD look,
//! * `bench.txt` — measured µs/frame per algorithm × factor.
//!
//! The emulator is deterministic headless (fixed WRAM seed, no audio), so the
//! same ROM + frame count always reproduces the same images.

use gameboy::gameboy::Gameboy;
use gameboy::upscale::{Algorithm, Upscaler};
use std::path::{Path, PathBuf};
use std::time::Instant;

const W: usize = 160;
const H: usize = 144;

fn main() {
    let mut rom: Option<PathBuf> = None;
    let mut frames: u32 = 600;
    let mut out_dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => {
                frames = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .expect("--frames needs a number");
            }
            "--out" => {
                out_dir = Some(PathBuf::from(args.next().expect("--out needs a path")));
            }
            other => rom = Some(PathBuf::from(other)),
        }
    }

    let rom = rom.unwrap_or_else(|| PathBuf::from("third_party/dmg-acid2/dmg-acid2.gb"));
    let stem = rom
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rom".into());
    let out = out_dir
        .unwrap_or_else(|| PathBuf::from("target").join("upscale-gallery").join(&stem));
    std::fs::create_dir_all(&out).expect("create output dir");

    // Copy the ROM into the output dir before running it: the MBC writes a
    // `.gbsave` next to the ROM path for battery carts, and we must neither
    // dirty a checkout nor touch a real save next to someone's game.
    let data = std::fs::read(&rom).unwrap_or_else(|e| {
        eprintln!("cannot read ROM {}: {e}", rom.display());
        std::process::exit(1);
    });
    let scratch_rom = out.join(format!("{stem}.gb.tmp"));
    std::fs::write(&scratch_rom, &data).expect("copy ROM to scratch");

    println!("emulating {} for {frames} frames...", rom.display());
    let mut gb = Gameboy::new(data, Some(scratch_rom.clone()));
    for _ in 0..frames {
        gb.frame();
    }
    let frame: Vec<u8> = gb.image().to_vec();
    drop(gb);
    let _ = std::fs::remove_file(&scratch_rom);
    let _ = std::fs::remove_file(out.join(format!("{stem}.gb.gbsave")));

    // The full menu: every algorithm at every supported factor.
    let all: Vec<(Algorithm, u8)> = [
        Algorithm::Nearest,
        Algorithm::Scale2x,
        Algorithm::Mmpx,
        Algorithm::LcdGrid,
    ]
    .into_iter()
    .flat_map(|a| a.supported_factors().iter().map(move |&f| (a, f)))
    .filter(|&(_, f)| f > 1)
    .collect();

    save_png(&out.join("00-source-x1.png"), &frame, W, H);

    // Individual algorithm outputs, grayscale and DMG-green.
    for &(algo, f) in &all {
        let mut up = Upscaler::new(algo, f);
        let (ow, oh) = up.output_size(W, H);
        let scaled = up.apply(&frame, W, H).to_vec();
        save_png(
            &out.join(format!("{}-x{f}.png", algo.name())),
            &scaled,
            ow,
            oh,
        );
    }
    let green = green_map(&frame);
    for &(algo, f) in &all {
        // The green LCD look, palette applied *before* upscaling exactly like
        // the terminal frontend does.
        if f == 4 || (algo == Algorithm::Scale2x && f == 3) {
            let mut up = Upscaler::new(algo, f);
            let (ow, oh) = up.output_size(W, H);
            let scaled = up.apply(&green, W, H).to_vec();
            save_png(
                &out.join(format!("{}-x{f}-green.png", algo.name())),
                &scaled,
                ow,
                oh,
            );
        }
    }

    // Side-by-side comparison strips at x4 (the interesting factor).
    let compare: Vec<(&str, Algorithm)> = vec![
        ("nearest", Algorithm::Nearest),
        ("scale2x", Algorithm::Scale2x),
        ("mmpx", Algorithm::Mmpx),
        ("lcd", Algorithm::LcdGrid),
    ];
    montage(&out.join("compare-x4.png"), &frame, &compare, 4);
    montage(&out.join("compare-x4-green.png"), &green, &compare, 4);

    // Benchmark on the real frame (not synthetic noise): warm up, then take
    // the average and minimum of many runs.
    let mut report = String::from(
        "per-frame upscale cost on one real 160x144 frame (release build)\n\
         algorithm  factor  avg µs   min µs   output px\n",
    );
    for &(algo, f) in &all {
        let mut up = Upscaler::new(algo, f);
        let iters: u32 = 200;
        for _ in 0..10 {
            let _ = up.apply(&frame, W, H);
        }
        let mut total = 0u128;
        let mut min = u128::MAX;
        for _ in 0..iters {
            let t = Instant::now();
            let outp = up.apply(&frame, W, H);
            let dt = t.elapsed().as_micros();
            assert!(!outp.is_empty());
            total += dt;
            min = min.min(dt);
        }
        let (ow, oh) = up.output_size(W, H);
        report.push_str(&format!(
            "{:<10} x{:<5} {:>7} {:>8}   {}x{}\n",
            algo.name(),
            f,
            total / iters as u128,
            min,
            ow,
            oh
        ));
    }
    print!("{report}");
    std::fs::write(out.join("bench.txt"), &report).expect("write bench.txt");
    println!("\ngallery written to {}", out.display());
}

/// Apply the classic DMG olive-green palette to a greyscale RGBA frame
/// (mirrors the terminal frontend's `Palette::Green`).
fn green_map(rgba: &[u8]) -> Vec<u8> {
    const SHADES: [[u8; 3]; 4] =
        [[224, 248, 208], [136, 192, 112], [52, 104, 86], [8, 24, 32]];
    let mut out = rgba.to_vec();
    for px in out.chunks_exact_mut(4) {
        if px[0] == px[1] && px[1] == px[2] {
            let shade = match px[0] {
                224.. => 0,
                144.. => 1,
                48.. => 2,
                _ => 3,
            };
            px[0..3].copy_from_slice(&SHADES[shade]);
        }
        px[3] = 255;
    }
    out
}

/// Write `algos` side by side at factor `f` with a thin separator, labelled by
/// the file name (left→right order matches the slice).
fn montage(path: &Path, frame: &[u8], algos: &[(&str, Algorithm)], f: u8) {
    const SEP: usize = 8;
    let mut cols = Vec::new();
    let (mut ow, mut oh) = (0, 0);
    for &(_, algo) in algos {
        let mut up = Upscaler::new(algo, f);
        (ow, oh) = up.output_size(W, H);
        cols.push(up.apply(frame, W, H).to_vec());
    }
    let total_w = ow * cols.len() + SEP * (cols.len() - 1);
    let mut out = vec![32u8; total_w * oh * 4]; // dark separator background
    for (i, col) in cols.iter().enumerate() {
        let x0 = i * (ow + SEP);
        for y in 0..oh {
            let src = &col[y * ow * 4..(y + 1) * ow * 4];
            let dst = &mut out[(y * total_w + x0) * 4..(y * total_w + x0 + ow) * 4];
            dst.copy_from_slice(src);
        }
    }
    save_png(path, &out, total_w, oh);
}

fn save_png(path: &Path, rgba: &[u8], w: usize, h: usize) {
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec())
        .expect("buffer matches dimensions");
    img.save(path).expect("write PNG");
    println!("  wrote {}", path.display());
}

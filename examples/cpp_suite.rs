//! Headless runner for the GB Emulator Shootout's **`cpp`** test group —
//! CasualPokePlayer's ROMs (three MBC3/RTC tests and `sgb-ext-test`).
//!
//! ```text
//! cargo run --release --example cpp_suite -- <dir> [--frames N]
//!                                            [--baseline <file>] [--out <dir>]
//! ```
//!
//! These four ROMs judge themselves by drawing a screen and the author ships a
//! reference photograph of a passing run beside each one, so the check is an
//! image compare against **his** PNG — nothing here is a hash of our own output.
//! The comparison is by **shade rank** (luminance order), the same rule
//! `examples/mealybug.rs` and `tools/conformance/image.py` use: our four shades
//! are `000000/606060/c0c0c0/ffffff` and a reference's need not be, but which
//! of the four each pixel got is the thing the ROM is asserting.
//!
//! **Each ROM runs on the console its own header asks for.** `sgb-ext-test`
//! sets the SGB flag (`$0146 == 0x03`) and stress-tests the SGB command-packet
//! protocol on `$FF00`; it cannot pass on a DMG, and the shootout's own manifest
//! gives it `model=SGB` for exactly that reason. The other three declare
//! nothing and get [`Model::Auto`], which is a DMG for them.
//!
//! The ROMs are **not vendored**: `tools/fetch-test-suites.sh` pulls them from
//! `gbdev/GBEmulatorShootout` at the commit pinned there, which is the same
//! corpus our published shootout score is measured on.

use std::path::{Path, PathBuf};
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::Model;

#[path = "support/suite_baseline.rs"]
mod suite_baseline;

const W: usize = 160;
const H: usize = 144;

const BASELINE_HEADER: &str = "\
# `cpp` baseline — the EXPECTED verdict of each of CasualPokePlayer's four
# shootout ROMs, judged against **his** reference screenshot by shade rank.
# Nothing here is a hash of our output.
#
# The ROMs are NOT vendored. CI fetches them from gbdev/GBEmulatorShootout at
# the commit pinned in tools/fetch-test-suites.sh — the same corpus the
# published 264-row score is measured on.
#
# `sgb-ext-test` runs on a Super Game Boy (its header sets $0146 = $03); the
# other three run on a DMG. The `diff=` figure is how many of the 23,040 pixels
# disagree with the reference, so a partial regression is visible rather than
# just PASS -> FAIL.
#
# CI is green when reality matches this file and red on ANY change, in either
# direction. Re-bless deliberately:
#
#   UPDATE_EXTERNAL_BASELINE=1 make test-external

";

/// A frame budget rather than a wall clock: every one of these ROMs finishes in
/// well under a second of emulated time and then hangs on its result screen.
const DEFAULT_FRAMES: u32 = 600;

fn main() {
    let (baseline, argv) =
        suite_baseline::take_baseline_arg(std::env::args().skip(1).collect());
    let mut args = argv.into_iter();
    let mut dir: Option<PathBuf> = None;
    let mut frames = DEFAULT_FRAMES;
    let mut out: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => {
                frames = args.next().and_then(|v| v.parse().ok()).unwrap_or(frames)
            }
            "--out" => out = args.next().map(PathBuf::from),
            _ => dir = Some(PathBuf::from(a)),
        }
    }
    let dir = dir
        .expect("usage: cpp_suite <dir> [--frames N] [--baseline <file>] [--out <dir>]");
    if let Some(o) = &out {
        let _ = std::fs::create_dir_all(o);
    }

    let mut roms: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "gb"))
        .collect();
    roms.sort();
    assert!(!roms.is_empty(), "no .gb files under {}", dir.display());

    let mut rows = Vec::new();
    let (mut pass, mut fail) = (0, 0);
    for rom in &roms {
        let name = rom.file_stem().unwrap().to_string_lossy().to_string();
        // The MBC writes a `.sav` next to the ROM, so stage a copy under
        // `target/` and leave the fetched checkout clean.
        let staged = stage(rom);
        let frame = run(&staged, frames);
        if let Some(o) = &out {
            save_png(&o.join(format!("{name}.png")), &frame);
        }
        let value = match load_reference(&rom.with_extension("png")) {
            None => "NOREF".to_string(),
            Some(reference) => {
                let diff = shade_diff(&frame, &reference);
                if diff == 0 {
                    pass += 1;
                    "PASS | diff=0".to_string()
                } else {
                    fail += 1;
                    format!("FAIL | diff={diff}")
                }
            }
        };
        println!("{value:20}  {name}");
        rows.push(suite_baseline::Row::new(name, value));
    }
    println!("\n{pass}/{} pass ({fail} fail)", roms.len());

    if let Some(path) = baseline {
        if !suite_baseline::gate("cpp", &path, BASELINE_HEADER, &rows) {
            std::process::exit(1);
        }
    }
}

/// Copy the ROM into `target/cpp-suite/` so the MBC's `.sav` lands there.
fn stage(rom: &Path) -> PathBuf {
    let dir = Path::new("target/cpp-suite");
    std::fs::create_dir_all(dir).expect("create target/cpp-suite");
    let dst = dir.join(rom.file_name().unwrap());
    std::fs::copy(rom, &dst).unwrap_or_else(|e| panic!("stage {}: {e}", rom.display()));
    dst
}

fn run(rom: &Path, frames: u32) -> Vec<u8> {
    let data = std::fs::read(rom).unwrap_or_else(|e| panic!("{}: {e}", rom.display()));
    let model = if data.get(0x146) == Some(&0x03) {
        Model::Sgb
    } else {
        Model::Auto
    };
    // Always pass the path: with `None` this core forces MBC3 for every cart,
    // which would quietly rig the three MBC3 tests.
    let mut cpu = Cpu::new_with_model(data, Some(rom.to_path_buf()), model);
    let mut done = 0;
    while done < frames {
        cpu.do_cycle();
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            done += 1;
        }
    }
    cpu.memory.gpu.data.to_vec()
}

fn load_reference(path: &Path) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.to_rgba8();
    if img.width() as usize != W || img.height() as usize != H {
        return None;
    }
    Some(img.into_raw())
}

/// Reduce both frames to four shade indices by luminance rank, then count the
/// pixels whose rank differs. Same rule as `examples/mealybug.rs`.
fn shade_diff(a: &[u8], b: &[u8]) -> usize {
    let ra = ranks(a);
    let rb = ranks(b);
    ra.iter().zip(&rb).filter(|(x, y)| x != y).count()
}

fn ranks(rgba: &[u8]) -> Vec<u8> {
    let lum: Vec<u32> = rgba
        .chunks_exact(4)
        .map(|p| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000)
        .collect();
    let mut order: Vec<u32> = lum.clone();
    order.sort_unstable();
    order.dedup();
    if order.len() > 4 {
        let (lo, hi) = (order[0], *order.last().unwrap());
        let span = (hi - lo).max(1) + 1;
        return lum
            .iter()
            .map(|v| (((v - lo) * 4 / span) as u8).min(3))
            .collect();
    }
    lum.iter()
        .map(|v| order.iter().position(|o| o == v).unwrap() as u8)
        .collect()
}

fn save_png(path: &Path, rgba: &[u8]) {
    if let Some(img) = image::RgbaImage::from_raw(W as u32, H as u32, rgba.to_vec()) {
        let _ = img.save(path);
    }
}

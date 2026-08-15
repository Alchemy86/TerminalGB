//! Score this emulator against the **Mealybug Tearoom Tests**, the PPU
//! mid-scanline suite (<https://github.com/mattcurrie/mealybug-tearoom-tests>).
//!
//! ```text
//! cargo run --release --example mealybug -- --dir <mealybug-checkout> [filter…]
//! MEALYBUG_DIR=<mealybug-checkout> cargo run --release --example mealybug
//! ```
//!
//! `--baseline <file>` gates the run against a checked-in expectation file and
//! exits non-zero on **any** change, in either direction — including a change
//! in how many pixels a *failing* test gets wrong, which is the only number
//! that makes progress on a cycle-stepped-PPU rewrite visible. See
//! `examples/support/suite_baseline.rs` and `docs/testing.md`.
//!
//! The ROMs and their reference screenshots are **not vendored**: like
//! `POKERED_GFX` in `testharness/trainer.rs`, the assets come from a path the
//! user supplies at run time. Point `--dir` at a checkout of the suite (the
//! repo ships a prebuilt `mealybug-tearoom-tests.zip`; unzip it anywhere inside
//! the checkout) and this walks it for `*.gb` and for
//! `expected/{DMG-blob,CPU CGB C,CPU CGB D}/*.png`. With no directory it prints
//! how to get one and exits 0, so it is safe to wire into a script.
//!
//! ## How a result is judged
//!
//! Exactly as the suite's own README asks: run to the `LD B,B` software
//! breakpoint, then compare the frame with the reference **pixel for pixel**.
//!
//! **The references are the test author's own emulator's screenshots**, not
//! photographs — his README is explicit ("screenshots from my Game Boy
//! emulator (which I believe to be correct)", with blurry hardware photographs
//! in a separate `photos/` directory he cross-checked them against). That is
//! still somebody else's judgement rather than ours, which is the property
//! that matters here; it is simply not the claim this file used to make.
//! The comparison is by **shade rank** (luminance order), the same rule
//! `tools/conformance/image.py` uses — a DMG frame is four shades and every
//! emulator paints them in its own palette, so a literal RGB compare would fail
//! for a reason that has nothing to do with the PPU. Ranking keeps the only
//! thing the ROM asserts: which of the four shades each pixel got.
//!
//! Every ROM is run on **both** consoles and each column is scored against the
//! frame from the machine its reference was photographed on: `DMG-blob` against
//! `Model::Dmg`, and `CPU CGB C`/`CPU CGB D` against `Model::Cgb` (a Color
//! console running the same DMG cartridge in compatibility mode). Scoring a
//! CGB reference against a DMG run, or the reverse, measures the wrong machine.
//! See [`docs/mealybug.md`](../docs/mealybug.md) for what the failures mean and
//! what it would take to fix them.
//!
//! The `PINNED` list below is the set that passes today. If one of those stops
//! being pixel-exact this exits non-zero — the rest are reported, not enforced.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use terminalgb::boot::BootRom;
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::{Model, PpuRevision};

#[path = "support/suite_baseline.rs"]
mod suite_baseline;

const BASELINE_HEADER: &str = "\
# Mealybug Tearoom baseline — the EXPECTED result of every ROM against every
# reference set, judged as the suite's own README asks: run to the `LD B,B`
# software breakpoint and compare the frame with the author's reference
# screenshot, pixel for pixel, by shade rank. Nothing here is a hash of our own
# output — the reference frames are Matt Currie's, produced by his own emulator
# (Beaten Dying Moon) and cross-checked by him against the hardware photographs
# he ships in the suite's `photos/` directory. Somebody else's judgement, which
# is the property that matters; not, as this header used to say, a photograph.
#
# Key is `<rom>@<reference set>`; the value is PASS/FAIL plus **how many of the
# 23040 pixels differ**. The pixel count is part of the expectation on purpose:
# this suite is 3/79 today and a cycle-stepped PPU rewrite is the only thing
# that will move it, so the count is the one number that shows the rewrite
# making progress instead of a flat wall of FAIL. It also means an intermediate
# PPU change WILL turn this file red — read the diff, and re-bless with the new
# counts in the same commit as the change that caused them.
#
# The ROMs and reference PNGs are NOT vendored — they are fetched at run time
# from the author's own repository at the commit pinned in
# tools/fetch-test-suites.sh (the ROMs come from the prebuilt
# `mealybug-tearoom-tests.zip` he ships there).
#
# CI is green when reality matches this file and red on ANY change, in either
# direction. Re-bless deliberately:
#
#   UPDATE_EXTERNAL_BASELINE=1 make test-external

";

const W: usize = 160;
const H: usize = 144;
const CYCLE_CAP: u64 = 250_000_000;

/// Reference sets, in the order they are printed: label, the suite's own
/// directory name, **the console that reference was photographed on** and
/// **which CGB CPU stepping's PPU it is**.
///
/// This matters. These cartridges have no CGB flag, so `Model::Auto` puts them
/// in a DMG — which is the right machine for `DMG-blob` and the wrong one for
/// the two CGB sets, where the reference is a Color console running the same
/// DMG cartridge in compatibility mode.
///
/// The two CGB sets are not one machine either, and that is not a detail: they
/// are photographs of **different CPU steppings**, and seven of the suite's
/// ROMs genuinely disagree between them by 8,265 pixels in total. `m3_scy_change`
/// alone differs by 6,217 — CPU CGB D and later latch the fetcher's `SCY + LY`
/// at the tile-number fetch, so the bitplane desync a DMG and a CGB C can
/// produce is simply unavailable there (see [`PpuRevision`]). A runner that
/// scored both columns against one Color machine could never pass both,
/// whatever the emulator did — the same lesson `examples/samesuite.rs` records
/// about SameSuite's `-cgb0`/`-cgbB` pairs. Each ROM is therefore run on all
/// three machines and each column scored against the frame from its own.
const REFS: [(&str, &str, Model, PpuRevision); 3] = [
    ("DMG", "DMG-blob", Model::Dmg, PpuRevision::CgbD),
    ("CGB-C", "CPU CGB C", Model::Cgb, PpuRevision::CgbC),
    ("CGB-D", "CPU CGB D", Model::Cgb, PpuRevision::CgbD),
];

/// Tests that are pixel-exact today, and must stay that way. `(rom, ref-label)`.
const PINNED: [(&str, &str); 3] = [
    ("m2_win_en_toggle", "DMG"),
    ("m2_win_en_toggle", "CGB-C"),
    ("m2_win_en_toggle", "CGB-D"),
];

fn main() {
    let (baseline, argv) =
        suite_baseline::take_baseline_arg(std::env::args().skip(1).collect());
    let mut dir = std::env::var_os("MEALYBUG_DIR").map(PathBuf::from);
    let mut filters: Vec<String> = Vec::new();
    let mut boots: Vec<BootRom> = Vec::new();
    let mut args = argv.into_iter();
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dir" => dir = args.next().map(PathBuf::from),
            // A user-supplied boot ROM, named explicitly and never picked up
            // from the config directory: a gated suite whose score depended on
            // what happens to be in somebody's `~/.config` would not be a gate.
            // May be given twice, once per console; each image is used for the
            // machine it belongs to. See `docs/mealybug.md` §8.6.
            "--boot-rom" => {
                let Some(path) = args.next() else {
                    eprintln!("--boot-rom needs a path");
                    std::process::exit(2);
                };
                match std::fs::read(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|d| BootRom::new(d).map_err(|e| e.to_string()))
                {
                    Ok(rom) => boots.push(rom),
                    Err(e) => {
                        eprintln!("--boot-rom {path}: {e}");
                        std::process::exit(2);
                    }
                }
            }
            "-h" | "--help" => return usage(),
            other => filters.push(other.to_string()),
        }
    }
    if baseline.is_some() && !boots.is_empty() {
        eprintln!(
            "refusing to gate a run that used a boot ROM — every checked-in\n\
             expectation is the high-level-emulation machine, which is what a\n\
             fresh clone and CI both get. Run without --baseline to see the\n\
             boot-ROM score; docs/mealybug.md 8.6 reports both."
        );
        std::process::exit(2);
    }
    let Some(dir) = dir.filter(|d| d.is_dir()) else {
        return usage();
    };

    let roms = find_roms(&dir);
    if roms.is_empty() {
        eprintln!(
            "no *.gb found under {} — unzip mealybug-tearoom-tests.zip there first",
            dir.display()
        );
        std::process::exit(2);
    }
    let staging = Path::new("target/mealybug");
    let _ = std::fs::create_dir_all(staging);

    println!(
        "{:<38} {:>7} {:>7} {:>7}   stopped at (dmg / cgb)",
        "test", "DMG", "CGB-C", "CGB-D"
    );
    println!("{}", "-".repeat(84));

    let mut exact: BTreeMap<&str, (u32, u32)> =
        REFS.iter().map(|(l, ..)| (*l, (0, 0))).collect();
    let mut regressions = Vec::new();
    let mut rows = Vec::new();

    for rom in &roms {
        let name = rom.file_stem().unwrap().to_string_lossy().to_string();
        if !filters.is_empty() && !filters.iter().any(|f| name.contains(f)) {
            continue;
        }
        // The MBC writes its save next to the ROM; never dirty the user's
        // checkout (a standing trap in this repo — see CLAUDE.md).
        let staged = staging.join(format!("{name}.gb"));
        std::fs::copy(rom, &staged).expect("stage ROM into target/");

        let runs: Vec<(Vec<u8>, &'static str)> = REFS
            .iter()
            .map(|&(_, _, model, rev)| run(&staged, model, rev, boot_for(&boots, model)))
            .collect();
        let dmg = &runs[0];
        let cgb = &runs[2];
        let mut cells = String::new();
        for (i, (label, sub, _, _)) in REFS.iter().copied().enumerate() {
            let (frame, _) = &runs[i];
            let path = dir.join("expected").join(sub).join(format!("{name}.png"));
            match load_reference(&path) {
                None => cells.push_str(&format!("{:>7} ", "-")),
                Some(reference) => {
                    let diff = shade_diff(frame, &reference);
                    cells.push_str(&format!("{diff:>7} "));
                    rows.push(suite_baseline::Row::new(
                        format!("{name}@{label}"),
                        format!(
                            "{} | {diff}/{} pixels differ",
                            if diff == 0 { "PASS" } else { "FAIL" },
                            W * H
                        ),
                    ));
                    let slot = exact.get_mut(label).unwrap();
                    slot.1 += 1;
                    if diff == 0 {
                        slot.0 += 1;
                    } else if boots.is_empty() && PINNED.contains(&(name.as_str(), label))
                    {
                        regressions
                            .push(format!("{name} vs {label}: {diff} pixels differ"));
                    }
                }
            }
        }
        println!("{name:<38} {cells}  {} / {}", dmg.1, cgb.1);
        save_png(&staging.join(format!("{name}.dmg.png")), &dmg.0);
        save_png(&staging.join(format!("{name}.cgb.png")), &cgb.0);
        // The CGB-C machine is a third frame now, and it is the one worth
        // having beside the others when a row fails on only one Color column.
        save_png(&staging.join(format!("{name}.cgb-c.png")), &runs[1].0);
    }

    println!();
    for (label, ..) in REFS {
        let (pass, total) = exact[label];
        println!("{label}: {pass}/{total} pixel-exact");
    }
    println!("\nframes written to {}/", staging.display());

    if !regressions.is_empty() {
        eprintln!("\nREGRESSION — these were pixel-exact and no longer are:");
        for r in &regressions {
            eprintln!("  {r}");
        }
        std::process::exit(1);
    }

    if let Some(path) = baseline {
        if !filters.is_empty() {
            eprintln!(
                "\nrefusing to gate a filtered run — a subset would look like every \
                 other ROM disappearing"
            );
            std::process::exit(2);
        }
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        if !suite_baseline::gate("mealybug", &path, BASELINE_HEADER, &rows) {
            std::process::exit(1);
        }
    }
}

/// The supplied image that belongs to `model`'s console, if any.
///
/// `Model::Dmg`/`Model::Cgb` are what this runner passes, so resolving against
/// the mode each produces for a DMG-flagged cartridge is exact.
fn boot_for(boots: &[BootRom], model: Model) -> Option<BootRom> {
    let mode = match model {
        Model::Dmg => terminalgb::mode::GbMode::Classic,
        _ => terminalgb::mode::GbMode::ColorAsClassic,
    };
    boots.iter().find(|b| b.suits(mode)).cloned()
}

fn usage() {
    println!(
        "usage: cargo run --release --example mealybug -- --dir <mealybug-tearoom-tests> [filter…]\n\
         \n\
         The ROMs and reference screenshots are not vendored (they are not ours to ship).\n\
         Get them with:\n\
         \n\
           git clone https://github.com/mattcurrie/mealybug-tearoom-tests\n\
           cd mealybug-tearoom-tests && unzip mealybug-tearoom-tests.zip -d build\n\
         \n\
         then pass that directory with --dir (or set MEALYBUG_DIR).\n\
         \n\
         --boot-rom <path>   run with a boot ROM you dumped from your own\n\
                             hardware (256 bytes DMG, 2304 bytes CGB). May be\n\
                             given twice, once per console. Ten of these ROMs\n\
                             draw with a tile only a real boot ROM leaves in\n\
                             VRAM; see docs/mealybug.md 8.6 for both scores.\n\
                             Never combined with --baseline: the checked-in\n\
                             expectations are the boot-ROM-less machine.\n\
         \n\
         See docs/mealybug.md for what these tests measure and where we stand."
    );
}

fn find_roms(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "gb") {
                out.push(p);
            }
        }
    }
    out.sort();
    out.dedup_by_key(|p| p.file_name().unwrap().to_owned());
    out
}

/// Run to the `LD B,B` software breakpoint the suite documents as "a good time
/// to take a screenshot", falling back to a cycle cap. Returns the framebuffer
/// as RGBA plus how it stopped.
fn run(
    rom: &Path,
    model: Model,
    rev: PpuRevision,
    boot: Option<BootRom>,
) -> (Vec<u8>, &'static str) {
    let data = std::fs::read(rom).expect("read staged ROM");
    // Pass the path so cartridge RAM is battery-backed where the ROM expects
    // it. It does not choose the mapper — that is header byte `$0147`.
    let mut cpu = Cpu::new_with_boot_rom(data, Some(rom.to_path_buf()), model, boot);
    cpu.memory.gpu.set_cgb_ppu_revision(rev);
    let mut cycles: u64 = 0;
    while cycles < CYCLE_CAP {
        if cpu.memory.rb(cpu.registers.pc) == 0x40 {
            // `LD B,B` — a no-op on hardware, the suite's breakpoint marker.
            return (cpu.memory.gpu.data.to_vec(), "LD B,B");
        }
        cycles += cpu.do_cycle() as u64;
    }
    (cpu.memory.gpu.data.to_vec(), "cycle cap")
}

fn load_reference(path: &Path) -> Option<Vec<u8>> {
    let img = image::open(path).ok()?.to_rgba8();
    if img.width() as usize != W || img.height() as usize != H {
        return None;
    }
    Some(img.into_raw())
}

/// Reduce a frame to four shade indices by luminance rank, then count the
/// pixels whose rank differs. Mirrors `tools/conformance/image.py`.
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
        // More shades than a DMG frame can hold (a colour reference): bucket
        // evenly rather than pretend to rank.
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

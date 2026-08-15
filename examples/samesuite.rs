//! Headless SameSuite runner — machine-readable verdicts, no screenshots.
//!
//! [SameSuite](https://github.com/LIJI32/SameSuite) tests report three ways
//! (`include/base.inc`), all of which are cheaper and more precise than the GB
//! Emulator Shootout's screenshot comparison:
//!
//! * `RESULT_CODE` at `$CFFE` — `"P"` (pass) or `"F"` (fail), written by the
//!   ROM's own `PrintResults` after comparing its measurements against the
//!   `CorrectResults` table assembled into the ROM.
//! * Mooneye-style registers `B=3 C=5 D=8 E=13 H=21 L=34` on success (`$42`
//!   everywhere on failure) at an `LD B,B` software breakpoint.
//! * The same six bytes shipped out of the serial port.
//!
//! We use the first (simplest, and it is set before the ROM stops), and dump
//! the raw measurement table at `RESULTS_START` so a failing test can be
//! debugged against the expectations in its `.asm` source.
//!
//! ```text
//! cargo run --release --example samesuite -- <dir-or-rom> [--dump] [--frames N]
//!                                             [--baseline <file>]
//! ```
//!
//! `--baseline <file>` gates the run against a checked-in expectation file and
//! exits non-zero on **any** change, in either direction — see
//! `examples/support/suite_baseline.rs` and `docs/testing.md`.
//!
//! The APU **must** be installed for any of this to mean anything: with no
//! player, `Sound::do_cycle` returns immediately and the APU never advances.

use std::path::{Path, PathBuf};
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::Model;
use terminalgb::sound::{AudioPlayer, CgbRevision};

#[path = "support/suite_baseline.rs"]
mod suite_baseline;

const BASELINE_HEADER: &str = "\
# SameSuite baseline — the EXPECTED verdict of every ROM, judged by **the
# ROM's own** `RESULT_CODE` byte at $CFFE ('P' or 'F'), written by its
# `PrintResults` after comparing its measurements against the `CorrectResults`
# table assembled into it. Nothing here is a hash of our output, and the
# emulator has no way to influence what counts as a pass.
#
# The ROMs are NOT vendored — they are LIJI32's, under his licence. SameSuite
# ships source only, so CI fetches the prebuilt copies vendored by
# gbdev/GBEmulatorShootout at the commit pinned in tools/fetch-test-suites.sh
# (that is also the corpus our published 264-row score is measured on).
#
# Every ROM runs on the console its own header names. Only the two `sgb/` tests
# declare anything (SGB flag $0146 = $03) and they get a Super Game Boy, which
# is what the shootout's own manifest gives them; everything else is Auto.
#
# CI is green when reality matches this file and red on ANY change, in either
# direction — a row that starts PASSING is a change too. Re-bless deliberately:
#
#   UPDATE_EXTERNAL_BASELINE=1 make test-external
#
# `HANG` means the ROM never wrote a result code within its frame budget.
#
# Five rows are scored on a **specific CPU CGB revision**, named in the ROM's
# own filename (`-cgb0`, `-cgbB`, `-cgb0B`): the APU's extra-length-clocking
# rule changed in CPU CGB C, and `-cgb0` and `-cgbB` disagree, so no single
# machine can satisfy both. The runner reads the revision off the name, the
# same way it would force `@dmg`/`@cgb` from a cartridge header — see
# `revision_for` below and `terminalgb::sound::CgbRevision`. Everything else runs
# on the default (CPU CGB C and later), which is what a game gets.

";

/// Device-free sink. SameSuite never listens to the output — it reads PCM12 /
/// PCM34 and NR52 — but the APU only runs when a player is installed.
struct NullPlayer;

impl AudioPlayer for NullPlayer {
    fn play(&mut self, _left: &[f32], _right: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        44_100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

/// `$CFFE` — `RESULT_CODE` in `include/base.inc`.
const RESULT_CODE: u16 = 0xCFFE;

#[derive(PartialEq)]
enum Verdict {
    Pass,
    Fail,
    Unfinished,
}

/// Five APU ROMs name a **CPU CGB revision** in their filename — the APU's
/// extra-length-clocking rule was changed in CPU CGB C, and CH3 differs again
/// between CGB 0 and CGB B (`terminalgb::sound::CgbRevision`). A single
/// configuration cannot satisfy `-cgb0` and `-cgbB` at once, so the runner
/// reads the revision off the name, exactly as it would force `@dmg`/`@cgb`
/// from a cartridge header. The ROM still judges itself; all this picks is
/// which machine it is judging.
fn revision_for(rom: &Path) -> CgbRevision {
    let stem = rom.file_stem().unwrap_or_default().to_string_lossy();
    match stem.rsplit_once('-') {
        Some((_, tag)) => CgbRevision::from_tag(tag).unwrap_or_default(),
        None => CgbRevision::default(),
    }
}

/// One APU ROM names a whole **console** rather than a CPU CGB revision:
/// `channel_1_freq_change_timing-A` is the Game Boy Advance's answer, and it
/// contradicts its `-cgbDE` sibling by design — the two tables differ only in
/// the width of the frequency timer's reload window
/// (`terminalgb::sound::SQUARE_RELOAD_LATCH_AGB`), so no one machine can pass
/// both. `A` is Gekkio's own single-letter tag for an AGB and
/// `examples/mooneye_scout.rs::model_for` already reads it that way; this is
/// the same rule applied to the same letter.
///
/// Everything else runs on `Model::Auto`, which for a CGB-flagged cartridge is
/// a Game Boy Color — never an AGB, which is a console you ask for by name.
fn console_for(rom: &Path) -> Model {
    let stem = rom.file_stem().unwrap_or_default().to_string_lossy();
    match stem.rsplit_once('-') {
        Some((_, "A")) => Model::Agb,
        _ => Model::Auto,
    }
}

fn run(rom: &Path, frames: u32, dump: bool) -> (Verdict, Vec<u8>) {
    let data = std::fs::read(rom).unwrap_or_else(|e| panic!("{}: {e}", rom.display()));
    // Run each ROM on the console its own header asks for. The `sgb/` tests
    // set the SGB flag ($0146 == 0x03) and measure the joypad port's SGB
    // behaviour, so a DMG cannot pass them — the GB Emulator Shootout's own
    // manifest gives them `model=SGB` for the same reason. Nothing else in the
    // suite declares it, so nothing else changes console.
    let model = if data.get(0x146) == Some(&0x03) {
        Model::Sgb
    } else {
        console_for(rom)
    };
    let mut cpu = Cpu::new_with_model(data, Some(rom.to_path_buf()), model);
    cpu.memory.enable_audio(Box::new(NullPlayer));
    if let Some(sound) = cpu.memory.sound.as_mut() {
        sound.set_cgb_revision(revision_for(rom));
    }

    let mut done = 0u32;
    let mut cycles: u64 = 0;
    let cap: u64 = 400_000_000;
    while done < frames && cycles < cap {
        cycles += cpu.do_cycle() as u64;
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            done += 1;
            if done % 8 == 0 {
                let c = cpu.memory.rb(RESULT_CODE);
                if c == b'P' || c == b'F' {
                    break;
                }
            }
        }
    }

    let verdict = match cpu.memory.rb(RESULT_CODE) {
        b'P' => Verdict::Pass,
        b'F' => Verdict::Fail,
        _ => Verdict::Unfinished,
    };
    // `RESULTS_START` is $C000 in every APU test but `channel_4_lfsr`
    // ($C006); 18 rows of 8 is the largest table.
    let table = if dump {
        (0..152u16).map(|i| cpu.memory.rb(0xC000 + i)).collect()
    } else {
        Vec::new()
    };
    (verdict, table)
}

fn collect(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        if root.extension().is_some_and(|e| e == "gb") {
            out.push(root.to_path_buf());
        }
        return;
    }
    let mut entries: Vec<_> = std::fs::read_dir(root)
        .unwrap_or_else(|e| panic!("{}: {e}", root.display()))
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for e in entries {
        collect(&e, out);
    }
}

/// The baseline key: the ROM's path relative to the suite root, without the
/// extension (`apu/channel_1/channel_1_align`). Stable across machines, which
/// an absolute path is not, and it keeps the sub-suite (`apu`/`dma`/`ppu`/
/// `sgb`) visible in the file.
fn key_for(rom: &Path, root: &Path) -> String {
    let base = if root.is_dir() {
        root
    } else {
        root.parent().unwrap_or(root)
    };
    match rom.strip_prefix(base) {
        Ok(rel) => rel.with_extension("").to_string_lossy().replace('\\', "/"),
        Err(_) => rom.file_stem().unwrap().to_string_lossy().to_string(),
    }
}

fn main() {
    let (baseline, argv) =
        suite_baseline::take_baseline_arg(std::env::args().skip(1).collect());
    let mut args = argv.into_iter();
    let mut target: Option<PathBuf> = None;
    let mut frames = 900u32;
    let mut dump = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dump" => dump = true,
            "--frames" => frames = args.next().unwrap().parse().unwrap(),
            _ => target = Some(PathBuf::from(a)),
        }
    }
    let target = target.expect(
        "usage: samesuite <dir-or-rom> [--dump] [--frames N] [--baseline <file>]",
    );

    let mut roms = Vec::new();
    collect(&target, &mut roms);
    roms.sort();

    let (mut pass, mut fail, mut unfinished) = (0, 0, 0);
    let mut rows = Vec::new();
    for rom in &roms {
        let (v, table) = run(rom, frames, dump);
        let name = rom.file_stem().unwrap().to_string_lossy();
        let tag = match v {
            Verdict::Pass => {
                pass += 1;
                println!("PASS  {name}");
                "PASS"
            }
            Verdict::Fail => {
                fail += 1;
                println!("FAIL  {name}");
                "FAIL"
            }
            Verdict::Unfinished => {
                unfinished += 1;
                println!("HANG  {name}");
                "HANG"
            }
        };
        rows.push(suite_baseline::Row::new(key_for(rom, &target), tag));
        if dump && v != Verdict::Pass {
            for row in table.chunks(8) {
                println!(
                    "        {}",
                    row.iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
            }
        }
    }
    println!(
        "\n{pass}/{} pass ({fail} fail, {unfinished} unfinished)",
        roms.len()
    );
    if let Some(path) = baseline {
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        if !suite_baseline::gate("samesuite", &path, BASELINE_HEADER, &rows) {
            std::process::exit(1);
        }
    }
}

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
//! ```
//!
//! The APU **must** be installed for any of this to mean anything: with no
//! player, `Sound::do_cycle` returns immediately and the APU never advances.

use gameboy::cpu::core::Cpu;
use gameboy::sound::AudioPlayer;
use std::path::{Path, PathBuf};

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

fn run(rom: &Path, frames: u32, dump: bool) -> (Verdict, Vec<u8>) {
    let data = std::fs::read(rom).unwrap_or_else(|e| panic!("{}: {e}", rom.display()));
    let mut cpu = Cpu::new(data, Some(rom.to_path_buf()));
    cpu.memory.enable_audio(Box::new(NullPlayer));

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

fn main() {
    let mut args = std::env::args().skip(1);
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
    let target = target.expect("usage: samesuite <dir-or-rom> [--dump] [--frames N]");

    let mut roms = Vec::new();
    collect(&target, &mut roms);
    roms.sort();

    let (mut pass, mut fail, mut unfinished) = (0, 0, 0);
    for rom in &roms {
        let (v, table) = run(rom, frames, dump);
        let name = rom.file_stem().unwrap().to_string_lossy();
        match v {
            Verdict::Pass => {
                pass += 1;
                println!("PASS  {name}");
            }
            Verdict::Fail => {
                fail += 1;
                println!("FAIL  {name}");
            }
            Verdict::Unfinished => {
                unfinished += 1;
                println!("HANG  {name}");
            }
        }
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
}

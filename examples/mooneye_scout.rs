//! Score the emulator against Mooneye GB's `acceptance` suite using **Mooneye's
//! own documented criterion**: run until the `LD B,B` (`$40`) magic breakpoint,
//! then require the Fibonacci registers `B=3 C=5 D=8 E=13 H=21 L=34`.
//!
//! Deliberately dependency-free and read-only: it never looks at a reference
//! image, so its verdict cannot drift with our own rendering. That criterion is
//! what the mooneye-test-suite README defines a pass to be; the GBEmulatorShootout
//! instead screenshot-compares, which additionally fails us on boot-register
//! differences that have nothing to do with the behaviour under test.
//!
//! usage: `mooneye_scout <dir-or-rom>... [--baseline <file>]`
//!        `--baseline <file>` gates the run against a checked-in expectation
//!        file and exits non-zero on **any** change, in either direction —
//!        see `examples/support/suite_baseline.rs` and `docs/testing.md`.
//!        `GB_MODEL=cgb` forces a Color console (the `misc/` group needs it).
//!        `GB_SCOUT_TRACE=1` prints the register dump for every ROM.
//!        `GB_SCOUT_SCREEN=1` also prints the ROM's own assertion table, read
//!        back out of the BG tilemap (the suite loads its font so a tile id
//!        *is* the ASCII code of the glyph it draws — see AGENTS.md
//!        "Debugging a Mooneye failure: read the screen"). A failing test names
//!        the register it disagreed about, which the Fibonacci dump cannot.

use gameboy::cpu::core::Cpu;
use gameboy::mode::Model;
use gameboy::sound::AudioPlayer;
use std::path::{Path, PathBuf};

#[path = "support/suite_baseline.rs"]
mod suite_baseline;

const BASELINE_HEADER: &str = "\
# Mooneye GB baseline — the EXPECTED verdict of every ROM in the groups listed
# below, judged by **Mooneye's own criterion**: run to the `LD B,B` software
# breakpoint, then require the Fibonacci registers B=3 C=5 D=8 E=13 H=21 L=34.
# Nothing here is a hash of our own output; the value on a failing row is the
# register dump the ROM itself left behind.
#
# The ROMs are NOT vendored — they are Gekkio's, fetched at run time from the
# author's own prebuilt archive at the build id pinned in
# tools/fetch-test-suites.sh. `make test-external` fetches them and runs this
# gate (see docs/testing.md, \"Gating the external suites\").
#
# CI is green when reality matches this file and red on ANY change, in either
# direction — a row that starts PASSING is a change too. Re-bless deliberately:
#
#   UPDATE_EXTERNAL_BASELINE=1 make test-external
#
# The `-S`, `-sgb`, `-sgb2`, `-mgb` and `-dmg0` rows in `acceptance` are ROMs
# for a *named machine*, and this runner boots the one each names where it can:
# `-sgb`/`-sgb2` get a Super Game Boy (see src/sgb.rs) and pass. The remaining
# seven name machines still not modelled here — the SGB's SNES side (`-S`), an
# original MGB, a revision-0 DMG — and are pinned FAIL on purpose: they are not
# evidence of a bug, and the community's 66-row denominator excludes them.
#
";

/// The console matters — `misc/` is a Color-only group and scores differently
/// on a DMG — so the file records which one produced it rather than leaving the
/// reader to guess from the filename.
fn baseline_header() -> String {
    format!(
        "{BASELINE_HEADER}# Measured with GB_MODEL={}.\n\n",
        std::env::var("GB_MODEL").unwrap_or_else(|_| "unset (Model::Auto)".into())
    )
}

/// Device-free sink. A real machine always has an APU installed; running
/// without one silently changes what some ROMs observe.
struct NullPlayer;
impl AudioPlayer for NullPlayer {
    fn play(&mut self, _l: &[f32], _r: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        44_100
    }
    fn underflowed(&self) -> bool {
        false
    }
}

#[derive(PartialEq)]
enum Verdict {
    Pass,
    Fail,
    Timeout,
}

/// Read the screen back as text. Mooneye's `print_string` writes character
/// codes straight into the BG tilemap and its font is loaded so `tile id ==
/// ASCII code`, so the tilemap *is* the text on screen.
fn screen_text(cpu: &mut Cpu<'static>) -> String {
    let lcdc = cpu.memory.rb(0xFF40);
    let base: u16 = if lcdc & 0x08 != 0 { 0x9C00 } else { 0x9800 };
    let mut lines = Vec::new();
    for y in 0..18u16 {
        let mut line = String::new();
        for x in 0..20u16 {
            let tile = cpu.memory.rb(base + y * 32 + x);
            line.push(if (0x20..0x7F).contains(&tile) {
                tile as char
            } else {
                ' '
            });
        }
        let line = line.trim_end().to_string();
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines.join("\n")
}

/// Mooneye names each ROM after the machines it is written for, and a ROM for
/// one machine cannot pass on another — `boot_regs-sgb` asserts the Super Game
/// Boy's boot registers and is *expected* to fail on a DMG. Where this
/// emulator has the machine, run it: the `-sgb` and `-sgb2` suffixes select a
/// Super Game Boy. Everything else keeps whatever the caller asked for, which
/// is `$GB_MODEL` or [`Model::Auto`] — the `-S`, `-mgb` and `-dmg0` ROMs name
/// machines this emulator still does not model, and they stay red on purpose.
fn model_for(rom: &Path) -> Model {
    let name = rom.file_stem().unwrap_or_default().to_string_lossy();
    match name.rsplit('-').next() {
        Some("sgb") => Model::Sgb,
        Some("sgb2") => Model::Sgb2,
        _ => Model::from_env().unwrap_or_default(),
    }
}

fn run(rom: &Path) -> (Verdict, String) {
    let data = match std::fs::read(rom) {
        Ok(d) => d,
        Err(e) => return (Verdict::Timeout, format!("read error: {e}")),
    };
    let mut cpu = Cpu::new_with_model(data, Some(rom.to_path_buf()), model_for(rom));
    cpu.memory.enable_audio(Box::new(NullPlayer));

    let mut budget: u64 = 400_000_000;
    loop {
        // `LD B,B` is the suite's magic breakpoint; peek so the fetch is not
        // perturbed by the check itself.
        if cpu.memory.peek(cpu.registers.pc) == 0x40 {
            let r = &cpu.registers;
            let ok =
                r.b == 3 && r.c == 5 && r.d == 8 && r.e == 13 && r.h == 21 && r.l == 34;
            let mut dump = format!(
                "b={:02X} c={:02X} d={:02X} e={:02X} h={:02X} l={:02X}",
                r.b, r.c, r.d, r.e, r.h, r.l
            );
            if std::env::var("GB_SCOUT_SCREEN").is_ok() {
                // The ROM has only just hit `LD B,B`; give it the frames it
                // needs to finish drawing its report before reading it back.
                for _ in 0..200_000 {
                    cpu.do_cycle();
                }
                dump.push('\n');
                dump.push_str(&screen_text(&mut cpu));
            }
            return (if ok { Verdict::Pass } else { Verdict::Fail }, dump);
        }
        budget = budget.saturating_sub(cpu.do_cycle() as u64);
        if budget == 0 {
            return (Verdict::Timeout, String::from("no LD B,B reached"));
        }
    }
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(p)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().map(|e| e == "gb").unwrap_or(false) {
        out.push(p.to_path_buf());
    }
}

/// The baseline key for a ROM: its path relative to the suite root it was
/// found under, without the extension (`acceptance/timer/tma_write_reloading`).
/// Stable across machines and checkout locations, which an absolute path is
/// not.
fn key_for(rom: &Path, roots: &[String]) -> String {
    for r in roots {
        let root = Path::new(r);
        // Relativise against the root's *parent* when the root is itself the
        // group directory, so the group name survives in the key.
        let base = if root.is_dir() {
            root.parent().unwrap_or(root)
        } else {
            root
        };
        if let Ok(rel) = rom.strip_prefix(base) {
            return rel.with_extension("").to_string_lossy().replace('\\', "/");
        }
    }
    rom.file_stem().unwrap().to_string_lossy().to_string()
}

fn main() {
    let (baseline, args) =
        suite_baseline::take_baseline_arg(std::env::args().skip(1).collect());
    if args.is_empty() {
        eprintln!("usage: mooneye_scout <dir-or-rom>... [--baseline <file>]");
        std::process::exit(2);
    }
    let trace = std::env::var("GB_SCOUT_TRACE").is_ok();
    let mut roms = Vec::new();
    for a in &args {
        collect(Path::new(a), &mut roms);
    }

    // Stage every ROM in `target/` first: the MBC writes a `.sav` next to the
    // ROM path, and the suite checkout must stay clean.
    let stage =
        std::env::temp_dir().join(format!("mooneye-scout-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&stage);

    let (mut pass, mut fail, mut timeout) = (0, 0, 0);
    let mut failed = Vec::new();
    let mut rows = Vec::new();
    for rom in &roms {
        let staged = stage.join(rom.file_name().unwrap());
        std::fs::copy(rom, &staged).unwrap();
        let (v, dump) = run(&staged);
        let tag = match v {
            Verdict::Pass => {
                pass += 1;
                "PASS"
            }
            Verdict::Fail => {
                fail += 1;
                "FAIL"
            }
            Verdict::Timeout => {
                timeout += 1;
                "TIMEOUT"
            }
        };
        if tag != "PASS" {
            failed.push(rom.display().to_string());
        }
        // The baseline value is the ROM's own report: `PASS`, or the verdict
        // plus the register dump it left behind. First line only — the
        // `GB_SCOUT_SCREEN` assertion table is multi-line and is a debugging
        // aid, not an expectation.
        rows.push(suite_baseline::Row::new(
            key_for(rom, &args),
            if tag == "PASS" {
                String::from("PASS")
            } else {
                format!("{tag} | {}", dump.lines().next().unwrap_or_default())
            },
        ));
        if trace || tag != "PASS" {
            println!("{tag:7} {}  {dump}", rom.display());
        } else {
            println!("{tag:7} {}", rom.display());
        }
    }
    let _ = std::fs::remove_dir_all(&stage);
    println!(
        "\n{pass} pass, {fail} fail, {timeout} timeout  ({} total)",
        roms.len()
    );
    if let Some(path) = baseline {
        rows.sort_by(|a, b| a.key.cmp(&b.key));
        if !suite_baseline::gate("mooneye", &path, &baseline_header(), &rows) {
            std::process::exit(1);
        }
    }
}

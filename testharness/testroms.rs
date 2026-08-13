//! Headless test-ROM correctness harness.
//!
//! Runs Game Boy test ROMs without a window and decides pass/fail two ways:
//!
//!   * **Serial** — Blargg's ROMs write a human-readable result string ("Passed"
//!     / "Failed") to the serial port. We install a serial callback, run until
//!     the string appears (or a deterministic cycle budget is exhausted), and
//!     read the text. No image comparison needed.
//!   * **Frame hash** — image tests (dmg-acid2, and Blargg ROMs that only draw
//!     their result to the LCD) are judged by what is *drawn*. We run a fixed
//!     number of frames, hash the RGBA framebuffer with FNV-1a, and compare the
//!     hash against a checked-in baseline. On mismatch we write the actual frame
//!     out as a PNG so a human can see what changed.
//!
//! ## Known-failures baseline
//!
//! Most ROMs FAIL today — six emulation divergences are documented on purpose
//! (see `docs/testing.md`). A CI job that is red from day one gets ignored, so
//! this harness compares against `testharness/baseline.txt`: the expected
//! result of every ROM. CI is **green** when reality matches the baseline and
//! **red** only when a result *changes* — a regression (was passing, now fails)
//! or an improvement (was failing, now passes). Both directions are surfaced
//! loudly; an improvement is fixed by deliberately regenerating the baseline:
//!
//! ```text
//! UPDATE_BASELINE=1 cargo test --release --test testroms
//! ```
//!
//! The harness has NO external dependencies beyond the emulator crate: the hash
//! and the PNG writer are hand-rolled (a few dozen lines each) rather than
//! pulling in a hashing or image crate.

use gameboy::cpu::core::Cpu;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Deterministic cycle budget for a serial ROM. ~250M T-cycles is ~60s of
/// emulated time and runs in ~1s of wall time; a ROM that has not reported a
/// result by then is treated as a timeout FAIL. Bounding by *cycles* (not wall
/// clock) keeps the result reproducible across machines.
const SERIAL_CYCLE_BUDGET: u64 = 250_000_000;

/// Fixed number of rendered frames for a frame-hash ROM. Enough for the test to
/// finish drawing and settle; a fixed count keeps the resulting hash identical
/// every run.
const FRAME_HASH_FRAMES: u32 = 600;

/// Safety cap so a ROM that never updates the LCD (e.g. display off) cannot
/// wedge the frame loop.
const FRAME_HASH_CYCLE_CAP: u64 = 250_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Judged by the serial pass/fail string.
    Serial,
    /// Judged by the hash of the rendered framebuffer.
    FrameHash,
}

struct RomSpec {
    /// Stable identifier used as the baseline key and artifact filename.
    id: &'static str,
    /// Path relative to the crate root.
    path: &'static str,
    kind: Kind,
    /// One-line note (usually which documented divergence a failure maps to).
    note: &'static str,
}

/// The ROM registry. Serial ROMs give an objective pass/fail; frame-hash ROMs
/// exercise the PPU and the reference-compare machinery.
const ROMS: &[RomSpec] = &[
    // --- Blargg cpu_instrs: one ROM per instruction group (serial) ---
    RomSpec {
        id: "blargg/cpu_instrs/01-special",
        path: "tests/cpu_instrs/individual/01-special.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/02-interrupts",
        path: "tests/cpu_instrs/individual/02-interrupts.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/03-op-sp-hl",
        path: "tests/cpu_instrs/individual/03-op sp,hl.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/04-op-r-imm",
        path: "tests/cpu_instrs/individual/04-op r,imm.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/05-op-rp",
        path: "tests/cpu_instrs/individual/05-op rp.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/06-ld-r-r",
        path: "tests/cpu_instrs/individual/06-ld r,r.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/07-jr-jp-etc",
        path: "tests/cpu_instrs/individual/07-jr,jp,call,ret,rst.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/08-misc",
        path: "tests/cpu_instrs/individual/08-misc instrs.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/09-op-r-r",
        path: "tests/cpu_instrs/individual/09-op r,r.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/10-bit-ops",
        path: "tests/cpu_instrs/individual/10-bit ops.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/cpu_instrs/11-op-a-hl",
        path: "tests/cpu_instrs/individual/11-op a,(hl).gb",
        kind: Kind::Serial,
        note: "",
    },
    // --- Blargg timing (serial) ---
    RomSpec {
        id: "blargg/instr_timing",
        path: "tests/instr_timing/instr_timing.gb",
        kind: Kind::Serial,
        note: "",
    },
    RomSpec {
        id: "blargg/mem_timing",
        path: "tests/mem_timing/mem_timing.gb",
        kind: Kind::Serial,
        note: "memory-access M-cycle timing",
    },
    // --- Blargg ROMs that only draw their result to the LCD (frame hash) ---
    // These never emit serial in this build; we hash the final screen as a
    // regression guard and dump the PNG so a human can read the verdict.
    RomSpec {
        id: "blargg/mem_timing-2",
        path: "tests/mem_timing-2/mem_timing.gb",
        kind: Kind::FrameHash,
        note: "memory-access M-cycle timing",
    },
    RomSpec {
        id: "blargg/interrupt_time",
        path: "tests/interrupt_time/interrupt_time.gb",
        kind: Kind::FrameHash,
        note: "16-vs-20 T-cycle interrupt dispatch",
    },
    RomSpec {
        id: "blargg/halt_bug",
        path: "tests/halt_bug.gb",
        kind: Kind::FrameHash,
        note: "HALT bug not modelled",
    },
    RomSpec {
        id: "blargg/oam_bug",
        path: "tests/oam_bug/oam_bug.gb",
        kind: Kind::FrameHash,
        note: "DMG OAM-corruption bug not modelled",
    },
    // --- Pixel test judged by what is drawn (frame hash + reference image) ---
    RomSpec {
        id: "acid2/dmg-acid2",
        path: "third_party/dmg-acid2/dmg-acid2.gb",
        kind: Kind::FrameHash,
        note: "PPU: OAM/mode-3 approximations; compare vs reference-dmg.png",
    },
];

// ---------------------------------------------------------------------------
// FNV-1a 64-bit hash (hand-rolled, no dependency)
// ---------------------------------------------------------------------------

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

// ---------------------------------------------------------------------------
// Emulator drivers
// ---------------------------------------------------------------------------

/// The MBC selects a save path from the ROM's on-disk path and can write a
/// `.gbsave` next to it. Copy the ROM into a private work dir first so the test
/// checkout (a submodule!) is never dirtied.
fn staged_rom(work_dir: &Path, spec: &RomSpec) -> PathBuf {
    let src = PathBuf::from(spec.path);
    let data = std::fs::read(&src)
        .unwrap_or_else(|e| panic!("cannot read ROM {}: {e}", spec.path));
    let dst = work_dir
        .join(spec.id.replace('/', "__"))
        .with_extension("gb");
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&dst, data).unwrap();
    dst
}

fn new_cpu(rom_path: &Path) -> Cpu<'static> {
    let data = std::fs::read(rom_path).unwrap();
    // Pass the path so the correct MBC is chosen (with `None` every cart is
    // driven as MBC3 — a documented sharp edge).
    Cpu::new(data, Some(rom_path.to_path_buf()))
}

struct SerialResult {
    passed: bool,
    text: String,
    cycles: u64,
    timed_out: bool,
}

fn run_serial(rom_path: &Path) -> SerialResult {
    let mut cpu = new_cpu(rom_path);
    let out = Arc::new(Mutex::new(String::new()));
    let sink = out.clone();
    cpu.memory.serial.set_callback(Box::new(move |b: u8| {
        sink.lock().unwrap().push(b as char);
        None
    }));

    let mut cycles: u64 = 0;
    let mut done = false;
    while cycles < SERIAL_CYCLE_BUDGET {
        cycles += cpu.do_cycle() as u64;
        // Only inspect the buffer periodically to keep the hot loop cheap.
        if cycles % 4096 < 8 {
            let s = out.lock().unwrap();
            if s.contains("Passed") || s.contains("Failed") {
                done = true;
                break;
            }
        }
    }
    let text = out.lock().unwrap().clone();
    let passed = text.contains("Passed");
    SerialResult {
        passed,
        text,
        cycles,
        timed_out: !done,
    }
}

/// Run one frame: cycle until the PPU signals a completed frame.
fn step_frame(cpu: &mut Cpu<'static>, cycle_cap: &mut u64) -> bool {
    loop {
        *cycle_cap = cycle_cap.saturating_sub(cpu.do_cycle() as u64);
        if *cycle_cap == 0 {
            return false;
        }
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            return true;
        }
    }
}

/// Returns the RGBA framebuffer after a fixed number of frames.
fn run_frames(rom_path: &Path, frames: u32) -> Vec<u8> {
    let mut cpu = new_cpu(rom_path);
    let mut cap = FRAME_HASH_CYCLE_CAP;
    for _ in 0..frames {
        if !step_frame(&mut cpu, &mut cap) {
            break;
        }
    }
    cpu.memory.gpu.data.to_vec()
}

// ---------------------------------------------------------------------------
// Minimal PNG writer (RGB, uncompressed) — hand-rolled, no dependency
// ---------------------------------------------------------------------------

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc: u32 = 0xffff_ffff;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b): (u32, u32) = (1, 0);
    for &byte in bytes {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(tag);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// Encode an RGBA framebuffer (width*height*4) as an 8-bit RGB PNG using zlib
/// "stored" (uncompressed) blocks. Not small, but correct, deterministic and
/// dependency-free.
fn encode_png_rgb(rgba: &[u8], width: usize, height: usize) -> Vec<u8> {
    // Raw image data with a leading filter byte (0 = none) per scanline.
    let mut raw = Vec::with_capacity(height * (1 + width * 3));
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            let i = (y * width + x) * 4;
            raw.extend_from_slice(&rgba[i..i + 3]);
        }
    }

    // zlib stream: 2-byte header, stored deflate blocks, adler32 trailer.
    let mut zlib = vec![0x78, 0x01];
    let mut pos = 0;
    while pos < raw.len() {
        let block = std::cmp::min(0xffff, raw.len() - pos);
        let last = if pos + block >= raw.len() { 1u8 } else { 0u8 };
        zlib.push(last);
        zlib.extend_from_slice(&(block as u16).to_le_bytes());
        zlib.extend_from_slice(&(!(block as u16)).to_le_bytes());
        zlib.extend_from_slice(&raw[pos..pos + block]);
        pos += block;
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit, colour type 2 (RGB)
    png_chunk(&mut png, b"IHDR", &ihdr);
    png_chunk(&mut png, b"IDAT", &zlib);
    png_chunk(&mut png, b"IEND", &[]);
    png
}

// ---------------------------------------------------------------------------
// Baseline
// ---------------------------------------------------------------------------

fn baseline_path() -> PathBuf {
    PathBuf::from("testharness/baseline.txt")
}

fn load_baseline() -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let text = std::fs::read_to_string(baseline_path()).unwrap_or_default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn write_baseline(results: &[RunResult]) {
    let mut s = String::new();
    s.push_str("# Test-ROM known-failures baseline.\n");
    s.push_str("#\n");
    s.push_str(
        "# The EXPECTED result of every ROM. CI is green when reality matches this\n",
    );
    s.push_str(
        "# file and red only when a result changes (a regression, or an improvement\n",
    );
    s.push_str("# that should be blessed here). Regenerate deliberately with:\n");
    s.push_str("#\n");
    s.push_str("#   UPDATE_BASELINE=1 cargo test --release --test testroms\n");
    s.push_str("#\n");
    s.push_str(
        "# Serial ROMs record PASS / FAIL. Frame-hash ROMs record HASH:<fnv1a64>;\n",
    );
    s.push_str(
        "# a changed hash means the rendered output changed (inspect the dumped PNG).\n",
    );
    s.push_str("# See docs/testing.md for the full explanation.\n\n");
    for r in results {
        s.push_str(&format!("{} = {}\n", r.spec.id, r.baseline_value()));
    }
    std::fs::write(baseline_path(), s).unwrap();
}

// ---------------------------------------------------------------------------
// Run + report
// ---------------------------------------------------------------------------

struct RunResult {
    spec: &'static RomSpec,
    /// PASS / FAIL for serial ROMs.
    passed: Option<bool>,
    /// Frame hash for frame-hash ROMs.
    hash: Option<u64>,
    detail: String,
    /// Path to a dumped PNG, if one was written.
    artifact: Option<PathBuf>,
}

impl RunResult {
    fn baseline_value(&self) -> String {
        match self.spec.kind {
            Kind::Serial => {
                if self.passed == Some(true) {
                    "PASS".into()
                } else {
                    "FAIL".into()
                }
            }
            Kind::FrameHash => format!("HASH:{:016x}", self.hash.unwrap_or(0)),
        }
    }
}

fn artifact_dir() -> PathBuf {
    PathBuf::from("target/testrom-artifacts")
}

fn run_one(work_dir: &Path, spec: &'static RomSpec, dump_all: bool) -> RunResult {
    let staged = staged_rom(work_dir, spec);
    match spec.kind {
        Kind::Serial => {
            let r = run_serial(&staged);
            let last = r
                .text
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("")
                .trim();
            let detail = if r.timed_out {
                format!(
                    "timeout after {} cycles (last serial: {:?})",
                    r.cycles, last
                )
            } else {
                format!("{} cycles, serial: {:?}", r.cycles, last)
            };
            RunResult {
                spec,
                passed: Some(r.passed),
                hash: None,
                detail,
                artifact: None,
            }
        }
        Kind::FrameHash => {
            let rgba = run_frames(&staged, FRAME_HASH_FRAMES);
            let hash = fnv1a64(&rgba);
            // Always dump acid2 (so a human can compare vs the reference); dump
            // others on request or on mismatch (handled by the caller).
            let mut artifact = None;
            if dump_all {
                artifact = Some(dump_frame(spec, &rgba));
            }
            RunResult {
                spec,
                passed: None,
                hash: Some(hash),
                detail: format!(
                    "frame hash {:016x} after {} frames",
                    hash, FRAME_HASH_FRAMES
                ),
                artifact,
            }
        }
    }
}

fn dump_frame(spec: &RomSpec, rgba: &[u8]) -> PathBuf {
    let dir = artifact_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let png = encode_png_rgb(rgba, 160, 144);
    let out = dir.join(format!("{}.png", spec.id.replace('/', "__")));
    std::fs::write(&out, png).unwrap();
    out
}

fn main() {
    let update = std::env::var("UPDATE_BASELINE").is_ok();
    let dump_all = std::env::var("DUMP_FRAMES").is_ok();

    let work_dir = artifact_dir().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    // Reproducibility self-check: hash acid2 twice in this same run and require
    // the two hashes to be identical. A non-reproducible hash makes the whole
    // harness worthless, so fail loudly if it ever happens.
    {
        let staged = staged_rom(&work_dir, &ROMS[ROMS.len() - 1]); // acid2
        let h1 = fnv1a64(&run_frames(&staged, FRAME_HASH_FRAMES));
        let h2 = fnv1a64(&run_frames(&staged, FRAME_HASH_FRAMES));
        assert_eq!(
            h1, h2,
            "frame hash is NOT reproducible within a run — harness is invalid"
        );
        println!(
            "reproducibility self-check: acid2 hash stable ({h1:016x}) across two runs\n"
        );
    }

    let mut results = Vec::new();
    for spec in ROMS {
        // acid2 is always dumped so the rendered face can be eyeballed vs the
        // checked-in reference image.
        let dump = dump_all || spec.id == "acid2/dmg-acid2";
        results.push(run_one(&work_dir, spec, dump));
    }

    if update {
        write_baseline(&results);
        println!(
            "baseline written to {} ({} ROMs)",
            baseline_path().display(),
            results.len()
        );
        return;
    }

    let baseline = load_baseline();
    let mut regressions = Vec::new();
    let mut improvements = Vec::new();
    let mut serial_pass = 0usize;
    let mut serial_total = 0usize;

    println!("{:<34} {:<8} DETAIL", "ROM", "RESULT");
    println!("{}", "-".repeat(90));
    for r in &results {
        let expected = baseline.get(r.spec.id).cloned();
        let actual = r.baseline_value();
        let (mark, changed) = match &expected {
            Some(e) if *e == actual => ("ok", false),
            Some(_) => ("CHANGED", true),
            None => ("NEW", true),
        };
        if r.spec.kind == Kind::Serial {
            serial_total += 1;
            if r.passed == Some(true) {
                serial_pass += 1;
            }
        }
        let label = match r.spec.kind {
            Kind::Serial => {
                if r.passed == Some(true) {
                    "PASS"
                } else {
                    "FAIL"
                }
            }
            Kind::FrameHash => "HASH",
        };
        println!("{:<34} {:<8} {} [{}]", r.spec.id, label, r.detail, mark);

        if changed {
            // On a changed frame hash, dump the actual frame so the diff is visible.
            let artifact = if r.spec.kind == Kind::FrameHash && r.artifact.is_none() {
                // Re-run to capture the frame for the artifact.
                let staged = staged_rom(&work_dir, r.spec);
                Some(dump_frame(r.spec, &run_frames(&staged, FRAME_HASH_FRAMES)))
            } else {
                r.artifact.clone()
            };
            let msg = format!(
                "{}: expected {}, got {}{}{}",
                r.spec.id,
                expected.unwrap_or_else(|| "<not in baseline>".into()),
                actual,
                if r.spec.note.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", r.spec.note)
                },
                artifact
                    .map(|p| format!("  -> {}", p.display()))
                    .unwrap_or_default(),
            );
            // An improvement is a serial ROM that flipped FAIL->PASS.
            let improved = r.spec.kind == Kind::Serial
                && r.passed == Some(true)
                && expected_was_fail(&baseline, r.spec.id);
            if improved {
                improvements.push(msg);
            } else {
                regressions.push(msg);
            }
        }
    }

    println!("\nBlargg serial suite: {serial_pass}/{serial_total} passing");
    if let Some(acid) = results.iter().find(|r| r.spec.id == "acid2/dmg-acid2") {
        if let Some(a) = &acid.artifact {
            println!("acid2 frame written to {} (compare vs third_party/dmg-acid2/reference-dmg.png)", a.display());
        }
    }

    if !improvements.is_empty() {
        println!(
            "\n🎉 {} ROM(s) now PASS that were failing — update the baseline:",
            improvements.len()
        );
        for m in &improvements {
            println!("    {m}");
        }
        println!("    Run: UPDATE_BASELINE=1 cargo test --release --test testroms");
    }
    if !regressions.is_empty() {
        println!(
            "\n❌ {} ROM(s) changed unexpectedly (regression):",
            regressions.len()
        );
        for m in &regressions {
            println!("    {m}");
        }
    }

    if !regressions.is_empty() || !improvements.is_empty() {
        eprintln!(
            "\nFAIL: {} regression(s), {} improvement(s). Baseline mismatch — see above.",
            regressions.len(),
            improvements.len()
        );
        std::process::exit(1);
    }
    println!("\nAll results match the baseline. ✅");
}

fn expected_was_fail(baseline: &BTreeMap<String, String>, id: &str) -> bool {
    baseline.get(id).map(|v| v == "FAIL").unwrap_or(false)
}

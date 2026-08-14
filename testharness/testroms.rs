//! Headless test-ROM correctness harness.
//!
//! Every ROM here is judged by **someone else's** definition of correct — never
//! by what this emulator happened to print last time:
//!
//!   * **Serial** — Blargg's ROMs write a human-readable result string
//!     ("Passed" / "Failed") to the link port. We install a serial callback and
//!     run until the string appears or a deterministic cycle budget is spent.
//!   * **Verdict** — the Blargg ROMs that never touch the link port still
//!     *print* their result. Two places carry it, and both are the ROM's own
//!     words: cartridge RAM at `$A000` behind the `$DE $B0 $61` signature, and
//!     the screen itself. Blargg's font is loaded so that a tile id **is** its
//!     ASCII code, so the BG tilemap can be read back as text.
//!   * **Image** — dmg-acid2 is compared pixel by pixel against the reference
//!     PNG shipped by the ROM's author, by luminance rank so the comparison
//!     survives a different palette (`testharness/pngdec.rs`).
//!
//! ## What this harness used to get wrong
//!
//! Four ROMs (`mem_timing-2`, `interrupt_time`, `halt_bug`, `oam_bug`) were
//! recorded as an FNV hash of our own framebuffer, and dmg-acid2's checked-in
//! reference image was never actually compared. A hash of our own output cannot
//! fail: a regression changes the hash, and the documented fix for a changed
//! hash is to re-bless it. `interrupt_time` in particular was written off as
//! "cannot run headless" when the real cause was that **the harness installed
//! no APU** — the ROM spins on `NR52`. With an APU it runs to completion and
//! prints a real verdict, which is a result worth having. Every machine this
//! harness boots now has an APU, exactly like a real one. (It has since gone
//! FAIL → PASS off the back of two independent fixes — the 5-cycle interrupt
//! dispatch, and the APU running off the base clock so the ROM can see the
//! CGB double-speed switch it makes halfway through. Neither crew could have
//! turned it green alone, which is the argument for recording a verdict rather
//! than a hash.)
//!
//! ## Models
//!
//! Every ROM is run on **both consoles** — a DMG and a Game Boy Color — because
//! they are different machines (boot registers differ, and the undocumented CGB
//! registers only exist on one of them). A cartridge whose header says it will
//! not run on a DMG (`0x0143 == 0xC0`) is only run as CGB. Baseline keys carry
//! the model: `<rom>@dmg`, `<rom>@cgb`.
//!
//! Be honest about what that buys *today*: no ROM wired here currently reports a
//! different verdict on the two consoles, so the second column is a regression
//! net rather than new coverage. The ROMs that do distinguish them are Mooneye's,
//! which print their boot registers — nine of those change verdict with a DMG
//! mode, and they are not wired here yet. `self_check_models_differ` asserts the
//! two columns really are two machines so the split cannot quietly become
//! cosmetic.
//!
//! This also settles the "which binary are we testing?" question. The
//! `retrio/gb-test-roms` submodule ships `cpu_instrs`, `instr_timing`,
//! `halt_bug` and `oam_bug` with the CGB flag **patched** to `0x80`, so
//! auto-detection alone would run all four as Color games and we would never
//! test the DMG behaviour those ROMs were written for. Forcing the model covers
//! both without mutating or vendoring a ROM.
//!
//! ## Known-failures baseline
//!
//! `testharness/baseline.txt` records the expected verdict of every ROM/model
//! pair *together with the ROM's own report*. CI is **green** when reality
//! matches and **red** on any change — in either direction. Re-bless
//! deliberately:
//!
//! ```text
//! UPDATE_BASELINE=1 cargo test --release --test testroms
//! ```
//!
//! No external dependencies: the PNG reader, the PNG writer and the hash are
//! all hand-rolled. For the full rationale see `docs/testing.md`.

use gameboy::cpu::core::Cpu;
use gameboy::mode::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[path = "pngdec.rs"]
mod pngdec;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Deterministic cycle budget for a serial ROM. ~250M T-cycles is ~60s of
/// emulated time and runs in ~1s of wall time; a ROM that has not reported a
/// result by then is treated as a timeout FAIL. Bounding by *cycles* (not wall
/// clock) keeps the result reproducible across machines.
const SERIAL_CYCLE_BUDGET: u64 = 250_000_000;

/// Frame budget for a ROM judged by its printed verdict. Generous: the run
/// stops as soon as the verdict has settled, so this only bounds a ROM that
/// never reports at all.
const VERDICT_FRAMES: u32 = 2400;

/// Fixed number of rendered frames for an image ROM. A fixed count keeps the
/// comparison reproducible.
const IMAGE_FRAMES: u32 = 600;

/// Safety cap so a ROM that never updates the LCD (e.g. display off) cannot
/// wedge the frame loop.
const FRAME_CYCLE_CAP: u64 = 900_000_000;

/// Rate reported by the harness' silent audio device. Any rate works — the APU
/// retunes — but pinning it keeps runs reproducible.
const AUDIO_RATE: u32 = 44_100;

enum Kind {
    /// Judged by the serial pass/fail string.
    Serial,
    /// Judged by the result the ROM prints (cartridge RAM, else the screen).
    Verdict,
    /// Judged against a checked-in reference image produced by someone else.
    Image { reference: &'static str },
}

struct RomSpec {
    /// Stable identifier; the baseline key is `<id>@<model>`.
    id: &'static str,
    /// Path relative to the crate root.
    path: &'static str,
    kind: Kind,
    /// One-line note (usually which documented divergence a failure maps to).
    note: &'static str,
}

/// The ROM registry.
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
    // --- Blargg ROMs that print their result instead of sending it ---
    RomSpec {
        id: "blargg/mem_timing-2",
        path: "tests/mem_timing-2/mem_timing.gb",
        kind: Kind::Verdict,
        note: "memory-access M-cycle timing",
    },
    RomSpec {
        id: "blargg/interrupt_time",
        path: "tests/interrupt_time/interrupt_time.gb",
        kind: Kind::Verdict,
        note: "interrupt dispatch time at both CPU speeds",
    },
    RomSpec {
        id: "blargg/halt_bug",
        path: "tests/halt_bug.gb",
        kind: Kind::Verdict,
        note: "HALT bug",
    },
    RomSpec {
        id: "blargg/oam_bug",
        path: "tests/oam_bug/oam_bug.gb",
        kind: Kind::Verdict,
        note: "DMG OAM-corruption bug not modelled (documented non-goal)",
    },
    // --- Pixel test judged against its author's reference image ---
    RomSpec {
        id: "acid2/dmg-acid2",
        path: "third_party/dmg-acid2/dmg-acid2.gb",
        kind: Kind::Image {
            reference: "third_party/dmg-acid2/reference-dmg.png",
        },
        note: "PPU: OAM/mode-3 approximations",
    },
];

// ---------------------------------------------------------------------------
// Models
// ---------------------------------------------------------------------------

fn model_tag(m: Model) -> &'static str {
    match m {
        Model::Dmg => "dmg",
        Model::Cgb => "cgb",
        Model::Auto => "auto",
    }
}

/// Which consoles this cartridge is run on. Both, unless its own header says it
/// will not run on a DMG (`0x0143 == 0xC0`, "CGB only" — Pan Docs, "The
/// Cartridge Header").
fn models_for(rom: &[u8]) -> &'static [Model] {
    match rom.get(0x0143) {
        Some(0xC0) => &[Model::Cgb],
        _ => &[Model::Dmg, Model::Cgb],
    }
}

// ---------------------------------------------------------------------------
// Emulator drivers
// ---------------------------------------------------------------------------

/// A device-free audio sink. **Every machine the harness boots gets one**: a
/// real Game Boy always has an APU, several test ROMs poll `NR52`, and running
/// without one silently changes what the ROMs report.
struct SilentPlayer;

impl gameboy::sound::AudioPlayer for SilentPlayer {
    fn play(&mut self, _left: &[f32], _right: &[f32]) {}
    fn samples_rate(&self) -> u32 {
        AUDIO_RATE
    }
    fn underflowed(&self) -> bool {
        false
    }
}

/// The MBC selects a save path from the ROM's on-disk path and can write a
/// `.sav` next to it. Copy the ROM into a private work dir first so the test
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

fn new_cpu(rom_path: &Path, model: Model) -> Cpu<'static> {
    let data = std::fs::read(rom_path).unwrap();
    // Pass the path so the correct MBC is chosen (with `None` every cart is
    // driven as MBC3 — a documented sharp edge).
    let mut cpu = Cpu::new_with_model(data, Some(rom_path.to_path_buf()), model);
    cpu.memory.enable_audio(Box::new(SilentPlayer));
    cpu
}

struct SerialResult {
    passed: bool,
    text: String,
    cycles: u64,
    timed_out: bool,
}

fn run_serial(rom_path: &Path, model: Model) -> SerialResult {
    let mut cpu = new_cpu(rom_path, model);
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
fn run_frames(rom_path: &Path, model: Model, frames: u32) -> Vec<u8> {
    let mut cpu = new_cpu(rom_path, model);
    let mut cap = FRAME_CYCLE_CAP;
    for _ in 0..frames {
        if !step_frame(&mut cpu, &mut cap) {
            break;
        }
    }
    cpu.memory.gpu.data.to_vec()
}

/// Blargg's memory-mapped result block: status at `$A000`, the signature
/// `$DE $B0 $61` at `$A001..$A003`, zero-terminated text from `$A004`. Only
/// present on cartridges that actually have RAM — `halt_bug` and
/// `interrupt_time` do not, which is why the screen is read as well.
fn blargg_ram_result(cpu: &mut Cpu<'static>) -> Option<(u8, String)> {
    let sig = [
        cpu.memory.rb(0xA001),
        cpu.memory.rb(0xA002),
        cpu.memory.rb(0xA003),
    ];
    if sig != [0xDE, 0xB0, 0x61] {
        return None;
    }
    let status = cpu.memory.rb(0xA000);
    let mut text = String::new();
    for a in 0xA004..0xA400u16 {
        let b = cpu.memory.rb(a);
        if b == 0 {
            break;
        }
        text.push(b as char);
    }
    Some((status, text))
}

/// Read the screen back as text.
///
/// Blargg's print routine (gb-test-roms `common/`) loads its font so that a
/// tile id **is** the ASCII code of the character it draws, and writes
/// characters straight into the background tilemap. So the tilemap is literally
/// the text on screen, and a headless run can read the ROM's own verdict
/// without a reference image and without a hash of our own output.
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
        lines.push(line.trim_end().to_string());
    }
    lines.join("\n")
}

struct VerdictResult {
    passed: bool,
    /// The ROM's own report, whitespace-normalised.
    report: String,
    frames: u32,
    source: &'static str,
    settled: bool,
}

/// Collapse a printed report to one line so it can live in the baseline file.
fn normalise(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn verdict_reached(report: &str) -> bool {
    report.contains("Passed") || report.contains("Failed")
}

fn run_verdict(rom_path: &Path, model: Model) -> VerdictResult {
    let mut cpu = new_cpu(rom_path, model);
    let mut cap = FRAME_CYCLE_CAP;
    let mut frames = 0u32;
    let mut settled = false;
    while frames < VERDICT_FRAMES {
        if !step_frame(&mut cpu, &mut cap) {
            break;
        }
        frames += 1;
        // Poll cheaply, then let the ROM finish writing its text.
        if frames % 10 == 0 {
            let done = match blargg_ram_result(&mut cpu) {
                Some((status, _)) => status != 0x80,
                None => verdict_reached(&screen_text(&mut cpu)),
            };
            if done {
                for _ in 0..10 {
                    if !step_frame(&mut cpu, &mut cap) {
                        break;
                    }
                    frames += 1;
                }
                settled = true;
                break;
            }
        }
    }

    // Cartridge RAM carries a status byte, so prefer it; otherwise read the
    // screen. Both are the ROM's own words.
    let (passed, report, source) = match blargg_ram_result(&mut cpu) {
        Some((status, text)) => (status == 0x00, normalise(&text), "cart RAM $A000"),
        None => {
            let text = normalise(&screen_text(&mut cpu));
            (
                text.contains("Passed") && !text.contains("Failed"),
                text,
                "screen",
            )
        }
    };
    VerdictResult {
        passed,
        report,
        frames,
        source,
        settled,
    }
}

// ---------------------------------------------------------------------------
// Reference-image comparison
// ---------------------------------------------------------------------------

struct ImageResult {
    differing: usize,
    total: usize,
}

/// Compare a rendered RGBA frame against a reference image by **shade rank**,
/// the same rule the shipped conformance runner uses
/// (`tools/conformance/image.py`). Our shades are `000000/606060/c0c0c0/ffffff`
/// and the upstream dmg-acid2 reference is `000000/555555/aaaaaa/ffffff`, so a
/// literal RGB compare would fail every run for a reason the ROM is not
/// asserting.
fn compare_to_reference(rgba: &[u8], reference: &pngdec::Image) -> ImageResult {
    let ours = pngdec::quantise_shades(&pngdec::rgba_to_rgb(rgba), 4);
    let theirs = pngdec::quantise_shades(&reference.rgb, 4);
    let total = theirs.len();
    let differing = if ours.len() != total {
        total
    } else {
        ours.iter()
            .zip(theirs.iter())
            .filter(|(a, b)| a != b)
            .count()
    };
    ImageResult { differing, total }
}

fn load_reference(path: &str) -> pngdec::Image {
    let bytes = std::fs::read(path)
        .unwrap_or_else(|e| panic!("cannot read reference image {path}: {e}"));
    pngdec::decode(&bytes).unwrap_or_else(|e| panic!("cannot decode {path}: {e}"))
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

const BASELINE_HEADER: &str = "\
# Test-ROM baseline — the EXPECTED result of every ROM on every console.
#
# Key is `<rom>@<model>`; a cartridge whose header says it will not run on a
# DMG (0x0143 == 0xC0) has no @dmg row. The value is PASS or FAIL followed by
# the ROM's OWN report: the string it sent over the link port, the text it
# wrote to cartridge RAM at $A000, or how many pixels differ from its author's
# reference image. Nothing here is a hash of our own output, so a regression
# cannot be silently re-blessed as a new expectation.
#
# CI is green when reality matches this file and red on ANY change, in either
# direction. Re-bless deliberately, after reading the diff:
#
#   UPDATE_BASELINE=1 cargo test --release --test testroms
#
# See docs/testing.md.

";

fn write_baseline(results: &[RunResult]) {
    let mut s = String::from(BASELINE_HEADER);
    for r in results {
        s.push_str(&format!("{} = {}\n", r.key, r.baseline_value()));
    }
    std::fs::write(baseline_path(), s).unwrap();
}

// ---------------------------------------------------------------------------
// Run + report
// ---------------------------------------------------------------------------

struct RunResult {
    spec: &'static RomSpec,
    key: String,
    passed: bool,
    /// The ROM's own report (or the pixel tally for an image test).
    report: String,
    /// Human-facing extra detail, not part of the baseline value.
    detail: String,
    artifact: Option<PathBuf>,
}

impl RunResult {
    fn baseline_value(&self) -> String {
        let verdict = if self.passed { "PASS" } else { "FAIL" };
        if self.report.is_empty() {
            verdict.to_string()
        } else {
            format!("{verdict} | {}", self.report)
        }
    }
}

fn artifact_dir() -> PathBuf {
    PathBuf::from("target/testrom-artifacts")
}

fn artifact_name(spec: &RomSpec, model: Model) -> String {
    format!("{}__{}", spec.id.replace('/', "__"), model_tag(model))
}

fn dump_frame(spec: &RomSpec, model: Model, rgba: &[u8]) -> PathBuf {
    let dir = artifact_dir();
    std::fs::create_dir_all(&dir).unwrap();
    let png = encode_png_rgb(rgba, 160, 144);
    let out = dir.join(format!("{}.png", artifact_name(spec, model)));
    std::fs::write(&out, png).unwrap();
    out
}

fn run_one(
    work_dir: &Path,
    spec: &'static RomSpec,
    model: Model,
    dump_all: bool,
) -> RunResult {
    let staged = staged_rom(work_dir, spec);
    let key = format!("{}@{}", spec.id, model_tag(model));
    match &spec.kind {
        Kind::Serial => {
            let r = run_serial(&staged, model);
            let detail = if r.timed_out {
                format!("timeout after {} cycles", r.cycles)
            } else {
                format!("{} cycles", r.cycles)
            };
            RunResult {
                spec,
                key,
                passed: r.passed,
                report: normalise(&r.text),
                detail,
                artifact: None,
            }
        }
        Kind::Verdict => {
            let r = run_verdict(&staged, model);
            let detail = format!(
                "{} frames, from {}{}",
                r.frames,
                r.source,
                if r.settled { "" } else { " (NEVER SETTLED)" }
            );
            let artifact = if dump_all {
                Some(dump_frame(
                    spec,
                    model,
                    &run_frames(&staged, model, IMAGE_FRAMES),
                ))
            } else {
                None
            };
            RunResult {
                spec,
                key,
                passed: r.passed && r.settled,
                report: if r.report.is_empty() {
                    "<no verdict printed>".into()
                } else {
                    r.report
                },
                detail,
                artifact,
            }
        }
        Kind::Image { reference } => {
            let rgba = run_frames(&staged, model, IMAGE_FRAMES);
            let cmp = compare_to_reference(&rgba, &load_reference(reference));
            // Always dumped: a human comparing our frame with the reference is
            // the fallback when the numbers surprise them.
            let artifact = Some(dump_frame(spec, model, &rgba));
            RunResult {
                spec,
                key,
                passed: cmp.differing == 0,
                report: format!(
                    "{}/{} pixels differ from {}",
                    cmp.differing, cmp.total, reference
                ),
                detail: format!("{IMAGE_FRAMES} frames"),
                artifact,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Self-checks — a harness that cannot fail is worse than no harness
// ---------------------------------------------------------------------------

/// Prove the reference comparison has teeth *before* trusting a green run:
/// decode the checked-in reference, then flip a single pixel and require
/// exactly one difference to be reported.
fn self_check_comparator() {
    let Kind::Image { reference } = ROMS[ROMS.len() - 1].kind else {
        panic!("the last ROM registered is expected to be the image test");
    };
    let img = load_reference(reference);
    assert_eq!(
        (img.width, img.height),
        (160, 144),
        "reference image {reference} is not a Game Boy screen"
    );
    let shades = pngdec::quantise_shades(&img.rgb, 4);
    let distinct = {
        let mut v = shades.clone();
        v.sort_unstable();
        v.dedup();
        v.len()
    };
    assert!(
        distinct > 1,
        "reference image decoded to {distinct} shade(s) — the decoder is broken"
    );

    // An identical frame must compare equal…
    let rgba: Vec<u8> = img
        .rgb
        .chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let same = compare_to_reference(&rgba, &img);
    assert_eq!(
        same.differing, 0,
        "the reference image does not compare equal to itself"
    );
    // …and a frame with one pixel moved to a different shade must not.
    let mut tampered = rgba.clone();
    let target = shades.iter().position(|&s| s != shades[0]).unwrap();
    let (dark, light) = (0u8, 255u8);
    let replacement = if tampered[target * 4] > 128 {
        dark
    } else {
        light
    };
    tampered[target * 4] = replacement;
    tampered[target * 4 + 1] = replacement;
    tampered[target * 4 + 2] = replacement;
    let tampered_cmp = compare_to_reference(&tampered, &img);
    assert!(
        tampered_cmp.differing > 0,
        "flipping a pixel produced no difference — the comparison is vacuous"
    );
    println!(
        "self-check: {reference} decodes to {}x{} with {distinct} shades; a single \
         flipped pixel is detected ({} px)",
        img.width, img.height, tampered_cmp.differing
    );
}

/// Emulation must be deterministic or none of the above means anything.
fn self_check_reproducible(work_dir: &Path) {
    let spec = &ROMS[ROMS.len() - 1];
    let staged = staged_rom(work_dir, spec);
    let a = run_frames(&staged, Model::Dmg, IMAGE_FRAMES);
    let b = run_frames(&staged, Model::Dmg, IMAGE_FRAMES);
    assert!(
        a == b,
        "the same ROM rendered two different frames in one process — the harness is invalid"
    );
    println!(
        "self-check: {} renders identically across two runs",
        spec.id
    );
}

/// The `@dmg` and `@cgb` columns must be two different **machines**, or half the
/// matrix is measuring the same thing twice.
///
/// None of the ROMs wired here happens to *print* the difference — the boot
/// registers only show up in Mooneye's register dumps — so the split is asserted
/// directly instead of being taken on trust: a DMG boots `A=0x01 HL=0x014D` and
/// a Color console `A=0x11 HL=0x007C` (Pan Docs, "Power Up Sequence"), and the
/// two resolve to different `GbMode`s.
fn self_check_models_differ(work_dir: &Path) {
    let spec = &ROMS[ROMS.len() - 1];
    let staged = staged_rom(work_dir, spec);
    let dmg = new_cpu(&staged, Model::Dmg);
    let cgb = new_cpu(&staged, Model::Cgb);
    let regs = |c: &Cpu<'static>| {
        (
            c.registers.a,
            ((c.registers.h as u16) << 8) | c.registers.l as u16,
        )
    };
    assert_eq!(
        regs(&dmg),
        (0x01, 0x014D),
        "DMG boot registers are wrong (Pan Docs: A=0x01, HL=0x014D)"
    );
    assert_eq!(
        regs(&cgb),
        (0x11, 0x007C),
        "CGB-compat boot registers are wrong (Pan Docs: A=0x11, HL=0x007C)"
    );
    assert_ne!(
        dmg.memory.gbmode, cgb.memory.gbmode,
        "@dmg and @cgb resolved to the same machine — the model split is cosmetic"
    );
    println!(
        "self-check: @dmg boots A=01 HL=014D, @cgb boots A=11 HL=007C — two machines"
    );
}

fn main() {
    let update = std::env::var("UPDATE_BASELINE").is_ok();
    let dump_all = std::env::var("DUMP_FRAMES").is_ok();

    let work_dir = artifact_dir().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    self_check_comparator();
    self_check_reproducible(&work_dir);
    self_check_models_differ(&work_dir);
    println!();

    let mut results = Vec::new();
    for spec in ROMS {
        let rom = std::fs::read(spec.path)
            .unwrap_or_else(|e| panic!("cannot read ROM {}: {e}", spec.path));
        for &model in models_for(&rom) {
            results.push(run_one(&work_dir, spec, model, dump_all));
        }
    }

    if update {
        write_baseline(&results);
        println!(
            "baseline written to {} ({} rows)",
            baseline_path().display(),
            results.len()
        );
        return;
    }

    let baseline = load_baseline();
    let mut changes = Vec::new();
    let (mut pass, mut total) = (0usize, 0usize);

    println!("{:<40} {:<6} DETAIL", "ROM@MODEL", "RESULT");
    println!("{}", "-".repeat(100));
    for r in &results {
        total += 1;
        if r.passed {
            pass += 1;
        }
        let expected = baseline.get(&r.key).cloned();
        let actual = r.baseline_value();
        let mark = match &expected {
            Some(e) if *e == actual => "ok",
            Some(_) => "CHANGED",
            None => "NEW",
        };
        println!(
            "{:<40} {:<6} {} [{}]",
            r.key,
            if r.passed { "PASS" } else { "FAIL" },
            r.detail,
            mark
        );
        if !r.report.is_empty() {
            println!("{:<40}        {}", "", r.report);
        }
        if mark != "ok" {
            let artifact = r
                .artifact
                .clone()
                .map(|p| format!("  -> {}", p.display()))
                .unwrap_or_default();
            changes.push(format!(
                "{}:\n        expected: {}\n        got:      {}{}{}",
                r.key,
                expected.unwrap_or_else(|| "<not in baseline>".into()),
                actual,
                if r.spec.note.is_empty() {
                    String::new()
                } else {
                    format!("\n        note:     {}", r.spec.note)
                },
                artifact,
            ));
        }
    }

    // Per-suite tally, in the community's unit (one ROM, one console, one row).
    println!("\n{pass}/{total} ROM/model rows passing");
    for (label, prefix) in [
        ("Blargg cpu_instrs", "blargg/cpu_instrs/"),
        ("Blargg (all)", "blargg/"),
        ("acid2", "acid2/"),
    ] {
        let rows: Vec<&RunResult> = results
            .iter()
            .filter(|r| r.spec.id.starts_with(prefix))
            .collect();
        let ok = rows.iter().filter(|r| r.passed).count();
        println!("  {label:<20} {ok}/{}", rows.len());
    }

    if !changes.is_empty() {
        println!(
            "\n❌ {} result(s) changed from the baseline:",
            changes.len()
        );
        for m in &changes {
            println!("    {m}");
        }
        eprintln!(
            "\nFAIL: {} baseline mismatch(es). Read each diff above; if the change is \
             intended, re-bless with UPDATE_BASELINE=1.",
            changes.len()
        );
        std::process::exit(1);
    }
    println!("\nAll results match the baseline. ✅");
}

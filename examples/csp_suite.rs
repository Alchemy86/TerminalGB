//! Headless runner for the test suites in **[c-sp/game-boy-test-roms]** that
//! this repository had never scored: AGE, Bully, cgb-acid2, cgb-acid-hell, the
//! Gambatte suite, GBMicrotest, MBC3 Tester, the wilbertpol fork of Mooneye,
//! (parts of) little-things-gb, rtc3test, Scribbltests, Strikethrough and
//! TurtleTests.
//!
//! ```text
//! cargo run --release --features image --example csp_suite -- \
//!     --suite <name> --dir <root> [--baseline <file>] [--out <dir>] [--filter <substr>]
//! ```
//!
//! # Every verdict is the suite author's, never ours
//!
//! Each suite has its own pass/fail convention and **each author's own
//! convention is what is used here** — the aggregation ships a
//! `game-boy-test-roms-howto.md` beside every suite saying what it is, and
//! those documents are the specification this file implements. Nothing here
//! compares against a frame we produced:
//!
//! * **Fibonacci registers** — AGE and the wilbertpol Mooneye fork stop on a
//!   magic opcode and set `B=3 C=5 D=8 E=13 H=21 L=34` on success. They differ
//!   only in the opcode: `LD B,B` (`$40`) for AGE, the undefined `$ED` for
//!   wilbertpol, which is how Mooneye worked in 2016.
//! * **A verdict byte the ROM writes** — GBMicrotest puts the actual result at
//!   `$FF80`, the expected one at `$FF81` and `$01`/`$FF` at `$FF82`. Its
//!   README says to read **only** `$FF82`, because several tests leave
//!   `$FF80 == $FF81` on failure.
//! * **A digit drawn in the top-left corner** — a Gambatte ROM's file name
//!   carries the hexadecimal value it must display, and Gambatte's own
//!   `testrunner.cpp` compares the first 8x8 tiles of the frame against a
//!   built-in font. That font is transcribed into [`OUT_TILES`] below, bit for
//!   bit.
//! * **The author's reference screenshot** — everything else. This is the same
//!   rule `examples/mealybug.rs` and `examples/cpp_suite.rs` already follow.
//!
//! # Colour, which is the part that is easy to get wrong
//!
//! The reference screenshots are not in our colour space and four encodings are
//! in play. Which one applies is decided by **the mode the machine actually
//! booted into** plus the reference's own palette, and the reference is always
//! converted *towards* us, so our framebuffer is never quantised or re-ranked:
//!
//! * A **DMG** reference uses `#000000 #555555 #AAAAAA #FFFFFF`; ours uses
//!   `#000000 #606060 #C0C0C0 #FFFFFF`. Both are exact four-entry palettes, so
//!   each maps to a shade index and the indices are compared.
//! * A **CGB compatibility-mode** reference uses the boot ROM's colourisation
//!   (`#000000 #0063C6 #7BFF31 #FFFFFF` for background, `#000000 #943939
//!   #FF8484 #FFFFFF` for objects). We do not model that colourisation and
//!   render such a frame in grey, so those eight colours map onto the same four
//!   shade indices. **A shade-rank compare cannot do this**: eight distinct
//!   luminances in the reference against four in our frame re-ranks everything.
//! * A **true CGB** reference is real colour. Most suites encode a 5-bit
//!   channel as `(X << 3) | (X >> 2)`, which is exactly invertible, so the
//!   reference is taken back to 15-bit RGB and pushed through *our* colour
//!   correction before the compare.
//! * **Gambatte's** own screenshots are already in our colour space — it uses
//!   the same correction we do — and are compared under its `& 0xF8F8F8` mask.
//!
//! # The ROMs are not vendored
//!
//! `tools/fetch-test-suites.sh` downloads the c-sp release, pinned by tag and
//! checked by SHA-256. These are other people's ROMs under their own licences.
//!
//! [c-sp/game-boy-test-roms]: https://github.com/c-sp/game-boy-test-roms

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::{GbMode, Model, PpuRevision};
use terminalgb::sound::AudioPlayer;

#[path = "support/suite_baseline.rs"]
mod suite_baseline;

const W: usize = 160;
const H: usize = 144;
/// 4.194304 MHz / 59.7275 Hz.
const CYCLES_PER_FRAME: u64 = 70_224;

// ---------------------------------------------------------------------------
// Gambatte's `_out<hex>` font, transcribed from `testrunner.cpp`'s `tiles[]`.
// One `u64` per glyph: 8 rows of 8 bits, MSB = the top-left pixel, a set bit
// meaning black. The runner compares these against the top-left 8x8 tiles of
// the frame exactly as `frameBufferMatchesOut` does.
// ---------------------------------------------------------------------------
const OUT_TILES: [u64; 16] = [
    0x007f_4141_4141_417f, // 0
    0x0008_0808_0808_0808, // 1
    0x007f_0101_7f40_407f, // 2
    0x007f_0101_3f01_017f, // 3
    0x0041_4141_7f01_0101, // 4
    0x007f_4040_7e01_017e, // 5
    0x007f_4040_7f41_417f, // 6
    0x007f_0102_0408_1010, // 7
    0x003e_4141_3e41_413e, // 8
    0x007f_4141_7f01_017f, // 9
    0x0008_2241_7f41_4141, // A
    0x007e_4141_7e41_417e, // B
    0x003e_4140_4040_413e, // C
    0x007e_4141_4141_417e, // D
    0x007f_4040_7f40_407f, // E
    0x007f_4040_7f40_4040, // F
];

// ---------------------------------------------------------------------------
// Consoles
// ---------------------------------------------------------------------------

/// One console a ROM is to be scored on. `tag` is the word the suite's own file
/// name used (`dmgC`, `cgb04c`, `ncmBCE`, `GS`), which becomes the row-key
/// suffix — several references for one ROM name different machines, and the key
/// has to keep them apart.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Run {
    model: Model,
    ppu: Option<PpuRevision>,
    tag: String,
}

impl Run {
    fn new(model: Model, tag: &str) -> Run {
        Run {
            model,
            ppu: None,
            tag: tag.to_string(),
        }
    }
}

/// Read one device word out of a file name — `dmg`, `dmg08`, `dmgC`, `cgb`,
/// `cgb04c`, `cgbBCE`, `ncmBC` — and return the console it names. `ncm` is a
/// Game Boy Color running a cartridge with no Color flag; the suites call it
/// "non-CGB mode", and the console is still a CGB.
///
/// The letters after the prefix are CPU revisions. We model two PPU steppings
/// (C-and-earlier, D-and-later), so a tag naming D or E takes
/// [`PpuRevision::CgbD`] and one naming only B or C takes
/// [`PpuRevision::CgbC`]. A tag naming both (`cgbBCE`) leaves the default
/// alone: no single machine satisfies all of them, and picking one would be
/// scoring a console the author never tested.
fn device_from_word(word: &str) -> Option<Run> {
    let lower = word.to_ascii_lowercase();
    let (model, rest) = if let Some(r) = lower.strip_prefix("dmg") {
        (Model::Dmg, r)
    } else if let Some(r) = lower.strip_prefix("cgb") {
        (Model::Cgb, r)
    } else if let Some(r) = lower.strip_prefix("ncm") {
        (Model::Cgb, r)
    } else {
        return None;
    };
    // Anything that is not a bare revision list (`04c`, `BCE`, `08`, ``) is a
    // word that merely happens to start with those three letters.
    if !rest.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let letters: Vec<char> = rest.chars().filter(|c| c.is_ascii_alphabetic()).collect();
    let ppu = if model == Model::Dmg {
        None
    } else {
        let late = letters.iter().any(|c| matches!(c, 'd' | 'e'));
        let early = letters.iter().any(|c| matches!(c, 'b' | 'c'));
        match (late, early) {
            (true, false) => Some(PpuRevision::CgbD),
            (false, true) => Some(PpuRevision::CgbC),
            _ => None,
        }
    };
    Some(Run {
        model,
        ppu,
        tag: word.to_string(),
    })
}

/// Pull every device word out of a `-`/`_` separated tail. `-cgb-dmg` names two
/// consoles; `-basic-tests-dmg` names one.
fn devices_in(tail: &str) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for word in tail.split(['-', '_']) {
        if let Some(r) = device_from_word(word) {
            if !out.iter().any(|o| o.tag == r.tag) {
                out.push(r);
            }
        }
    }
    out
}

/// Mooneye's own naming, which the wilbertpol fork inherits: a lower-case
/// machine name or one of Gekkio's single compatibility letters. A suffix
/// naming *several* machines (`-GS`, `-cgbABCDE`) must be left alone — forcing
/// one of them boots the wrong console for the rest. Same rule as
/// `examples/mooneye_scout.rs::model_for`, which is where it came from.
fn mooneye_run(rom: &Path) -> Run {
    let name = rom.file_stem().unwrap_or_default().to_string_lossy();
    let Some(suffix) = name.rsplit('-').next().filter(|s| *s != name) else {
        return Run::new(Model::Auto, "auto");
    };
    let model = match suffix {
        "dmg" | "dmgABC" => Model::Dmg,
        "dmg0" => Model::Dmg0,
        "mgb" => Model::Mgb,
        "sgb" => Model::Sgb,
        "sgb2" => Model::Sgb2,
        "cgb" => Model::Cgb,
        "cgb0" => Model::Cgb0,
        "G" => Model::Dmg,
        "M" => Model::Mgb,
        "S" => Model::Sgb,
        "C" => Model::Cgb,
        "A" => Model::Agb,
        _ => Model::Auto,
    };
    Run::new(model, if model == Model::Auto { "auto" } else { suffix })
}

// ---------------------------------------------------------------------------
// Reference-screenshot colour
// ---------------------------------------------------------------------------

/// Our own four DMG shades, as `Gpu::setcolor` writes them.
const OUR_SHADES: [u8; 4] = [0x00, 0x60, 0xC0, 0xFF];
/// The four shades every suite's DMG reference uses.
const REF_SHADES: [u8; 4] = [0x00, 0x55, 0xAA, 0xFF];
/// The Color boot ROM's compatibility-mode palettes, background then object, as
/// `game-boy-test-roms-howto.md` states them. Both map onto the same four shade
/// indices, which is what makes a compat reference comparable with a frame we
/// render in grey.
/// Shade 2 of the background palette has **two** spellings in the shipped
/// references, and the aggregation's own documents disagree with one of them.
/// Every howto says `#7BFF31`, and 30 of the 31 references that carry that
/// colour use it — but MBC3 Tester's uses `#7BFF4A`, which is the exact
/// `(X << 3) | (X >> 2)` widening of the 5-bit `(15, 31, 9)` the boot ROM
/// really loads (`#7BFF31` widens `(15, 31, 6)`). Both are accepted as index 2:
/// refusing the one that disagrees with the prose would cost a row over a digit
/// in somebody's readme.
const COMPAT: [[u32; 4]; 2] = [
    [0x000000, 0x0063C6, 0x7BFF31, 0xFFFFFF],
    [0x000000, 0x943939, 0xFF8484, 0xFFFFFF],
];
const COMPAT_BG2_ALT: u32 = 0x7BFF4A;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum RefEncoding {
    /// Four grey shades: compare shade index against shade index.
    Dmg,
    /// The Color boot ROM's compatibility palettes: same, via a lookup.
    Compat,
    /// Real colour, 5-bit channels widened with `(X << 3) | (X >> 2)`.
    Plain5Bit,
    /// Real colour already in our space (Gambatte uses our correction).
    Gambatte,
}

/// Which encoding a reference is in, given the mode the machine actually booted
/// into.
///
/// A frame we drew in `Color` mode is real colour and the encoding is a
/// property of the *suite*. Any other mode gives four grey shades, and then the
/// reference is either the DMG palette or the Color boot ROM's compatibility
/// palettes — told apart from the reference's own colours, which is a fact
/// about the picture and not a guess.
fn encoding_for(
    mode: GbMode,
    px: &[u32],
    suite_colour: RefEncoding,
) -> Option<RefEncoding> {
    if mode == GbMode::Color {
        return Some(suite_colour);
    }
    if px
        .iter()
        .all(|c| grey(*c) && REF_SHADES.contains(&((c >> 16) as u8)))
    {
        return Some(RefEncoding::Dmg);
    }
    if px.iter().all(|c| compat_index(*c).is_some()) {
        return Some(RefEncoding::Compat);
    }
    None
}

/// Which of the four shades a Color compatibility-mode colour is, background
/// and object palettes together.
fn compat_index(c: u32) -> Option<usize> {
    if c == COMPAT_BG2_ALT {
        return Some(2);
    }
    COMPAT.iter().find_map(|p| p.iter().position(|x| *x == c))
}

fn grey(c: u32) -> bool {
    let (r, g, b) = ((c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF);
    r == g && g == b
}

/// Our colour correction, lifted from `Gpu::setrgb` so a reference can be
/// brought into our space rather than our frame being pushed out of it.
fn correct(r: u32, g: u32, b: u32) -> u32 {
    (((r * 13 + g * 2 + b) >> 1) << 16)
        | (((g * 3 + b) << 1) << 8)
        | ((r * 3 + g * 2 + b * 11) >> 1)
}

fn our_shade(px: &[u8]) -> Option<u8> {
    OUR_SHADES.iter().position(|t| *t == px[0]).map(|i| i as u8)
}

/// How many of the 23,040 pixels disagree with the reference.
///
/// A colour that neither side's palette explains is a fault in this runner, not
/// a failing emulator, and is reported as such rather than as 23,040 wrong
/// pixels.
fn frame_diff(ours: &[u8], reference: &[u32], enc: RefEncoding) -> Result<usize, String> {
    let mut diff = 0;
    for (i, r) in reference.iter().enumerate() {
        let o = &ours[i * 4..i * 4 + 4];
        let same = match enc {
            RefEncoding::Dmg | RefEncoding::Compat => {
                let want = if enc == RefEncoding::Dmg {
                    REF_SHADES
                        .iter()
                        .position(|t| *t == (r >> 16) as u8)
                        .ok_or_else(|| {
                            format!("reference colour {r:06X} is not a DMG shade")
                        })?
                } else {
                    compat_index(*r).ok_or_else(|| {
                        format!("reference colour {r:06X} is not a compatibility shade")
                    })?
                } as u8;
                let got = our_shade(o).ok_or_else(|| {
                    format!(
                        "we drew {:02X}{:02X}{:02X}, which is not one of our four shades",
                        o[0], o[1], o[2]
                    )
                })?;
                want == got
            }
            RefEncoding::Plain5Bit => {
                let want =
                    correct((*r >> 19) & 0x1F, (*r >> 11) & 0x1F, (*r >> 3) & 0x1F);
                let got =
                    (u32::from(o[0]) << 16) | (u32::from(o[1]) << 8) | u32::from(o[2]);
                // Compare on the five bits the 5-bit source can actually carry.
                (want ^ got) & 0xF8F8F8 == 0
            }
            RefEncoding::Gambatte => {
                let got =
                    (u32::from(o[0]) << 16) | (u32::from(o[1]) << 8) | u32::from(o[2]);
                (*r ^ got) & 0xF8F8F8 == 0
            }
        };
        if !same {
            diff += 1;
        }
    }
    Ok(diff)
}

fn load_reference(path: &Path) -> Option<Vec<u32>> {
    let img = image::open(path).ok()?.to_rgba8();
    if img.width() as usize != W || img.height() as usize != H {
        return None;
    }
    Some(
        img.into_raw()
            .chunks_exact(4)
            .map(|p| (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Audio capture, for Gambatte's `_outaudio0` / `_outaudio1`
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Tail {
    samples: Vec<f32>,
}

/// Keeps the most recent second of samples so the caller can ask whether the
/// last frame carried anything but a constant level. Gambatte judges its audio
/// tests on exactly one frame's worth of output.
#[derive(Clone, Default)]
struct TailPlayer(Arc<Mutex<Tail>>);

/// Gambatte judges an audio test on **one frame** of output: `outaudio0` passes
/// when every sample in it is equal, `outaudio1` when they are not.
///
/// The rate is ours, not its 2 MiHz one, and that was **measured rather than
/// assumed**: reporting `2_097_152` here (which `Sound::set_player` honours, so
/// `blip_buf` really does resample at the APU's own rate) changes not one
/// verdict across all 221 audio rows, and neither does widening the window from
/// one frame to the whole second. 44.1 kHz costs a quarter of the wall clock.
const APU_RATE: u32 = 44_100;
/// One frame at [`APU_RATE`]: 44100 / 59.7275.
const FRAME_SAMPLES: usize = 738;
const TAIL_CAP: usize = FRAME_SAMPLES * 60;

impl AudioPlayer for TailPlayer {
    fn play(&mut self, l: &[f32], _r: &[f32]) {
        let mut t = self.0.lock().unwrap();
        t.samples.extend_from_slice(l);
        if t.samples.len() > TAIL_CAP {
            let drop = t.samples.len() - TAIL_CAP;
            t.samples.drain(..drop);
        }
    }
    fn samples_rate(&self) -> u32 {
        APU_RATE
    }
    fn underflowed(&self) -> bool {
        false
    }
}

impl TailPlayer {
    /// Did the last frame carry anything other than one constant level?
    fn audible(&self) -> bool {
        let t = self.0.lock().unwrap();
        let n = t.samples.len().min(FRAME_SAMPLES);
        if n == 0 {
            return false;
        }
        let tail = &t.samples[t.samples.len() - n..];
        tail.iter().any(|s| *s != tail[0])
    }
}

// ---------------------------------------------------------------------------
// The machine
// ---------------------------------------------------------------------------

struct Machine {
    cpu: Cpu<'static>,
    audio: TailPlayer,
}

impl Machine {
    fn new(rom: &Path, run: &Run, save_next_to: bool) -> Option<Machine> {
        let data = std::fs::read(rom).ok()?;
        if data.len() < 0x150 {
            return None;
        }
        let path = save_next_to.then(|| rom.to_path_buf());
        let mut cpu = Cpu::try_new_with_model(data, path, run.model).ok()?;
        if let Some(rev) = run.ppu {
            cpu.memory.gpu.set_cgb_ppu_revision(rev);
        }
        // Never run a test machine without an APU: `Sound::do_cycle` returns at
        // once when no player is installed, so a ROM that spins on `NR52` never
        // leaves its title screen. `testharness/testroms.rs` records the same
        // trap.
        let audio = TailPlayer::default();
        cpu.memory.enable_audio(Box::new(audio.clone()));
        Some(Machine { cpu, audio })
    }

    fn frames(&mut self, n: u32) {
        let mut done = 0;
        while done < n {
            self.cpu.do_cycle();
            if self.cpu.memory.gpu.updated {
                self.cpu.memory.gpu.updated = false;
                done += 1;
            }
        }
    }

    /// Run until the opcode at `PC` is `op` (peeked, so the check cannot
    /// perturb the fetch), or until the cycle budget runs out.
    fn until_opcode(&mut self, op: u8, budget: u64) -> bool {
        let mut left = budget;
        while left > 0 {
            if self.cpu.memory.peek(self.cpu.registers.pc) == op {
                return true;
            }
            left = left.saturating_sub(u64::from(self.cpu.do_cycle()));
        }
        false
    }

    fn frame(&self) -> &[u8] {
        &self.cpu.memory.gpu.data[..]
    }
}

// ---------------------------------------------------------------------------
// Suites
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, PartialEq, Eq)]
enum Judge {
    /// AGE: `LD B,B` then Fibonacci registers, with a screenshot for the ROMs
    /// whose result can only be seen.
    Age,
    /// Mooneye (wilbertpol): `$ED` then Fibonacci registers.
    WilbertPol,
    /// GBMicrotest: the verdict byte at `$FF82`.
    Microtest,
    /// Gambatte: the digit in the corner, or the audio, or a screenshot.
    Gambatte,
    /// Everything judged only by the author's screenshot.
    Screenshot,
}

struct Suite {
    name: &'static str,
    /// Directory under the c-sp release root.
    dir: &'static str,
    judge: Judge,
    /// Frames to run before reading a screenshot verdict. Zero means "run to
    /// the ROM's own `LD B,B` and then finish the frame it was drawing".
    frames: u32,
    /// Which colour encoding this suite's *colour* references use. Grey and
    /// compatibility-mode references are recognised from the picture itself.
    colour: RefEncoding,
    /// Cartridge RAM must be battery-backed for these (a `.sav` is staged under
    /// `target/`, never beside the fetched ROM).
    battery: bool,
}

const SUITES: &[Suite] = &[
    Suite {
        name: "age",
        dir: "age-test-roms",
        judge: Judge::Age,
        frames: 0,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "gbmicrotest",
        dir: "gbmicrotest",
        judge: Judge::Microtest,
        frames: 0,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "mooneye-wilbertpol",
        dir: "mooneye-test-suite-wilbertpol",
        judge: Judge::WilbertPol,
        frames: 40,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "gambatte",
        dir: "gambatte",
        judge: Judge::Gambatte,
        // "All Gambatte test roms finish after 15 Game Boy LCD frames."
        frames: 15,
        colour: RefEncoding::Gambatte,
        battery: false,
    },
    Suite {
        name: "bully",
        dir: "bully",
        judge: Judge::Screenshot,
        frames: 30,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "strikethrough",
        dir: "strikethrough",
        judge: Judge::Screenshot,
        frames: 30,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "scribbltests",
        dir: "scribbltests",
        judge: Judge::Screenshot,
        // 10 frames for all but `statcount-auto`, which needs far more — see
        // `screenshot_row`'s own comment on the 270 → 700 correction.
        frames: 10,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "turtle-tests",
        dir: "turtle-tests",
        judge: Judge::Screenshot,
        frames: 30,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "little-things-gb",
        dir: "little-things-gb",
        judge: Judge::Screenshot,
        frames: 30,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "mbc3-tester",
        dir: "mbc3-tester",
        judge: Judge::Screenshot,
        frames: 40,
        colour: RefEncoding::Plain5Bit,
        battery: true,
    },
    Suite {
        name: "cgb-acid2",
        dir: "cgb-acid2",
        judge: Judge::Screenshot,
        frames: 0,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "cgb-acid-hell",
        dir: "cgb-acid-hell",
        judge: Judge::Screenshot,
        frames: 0,
        colour: RefEncoding::Plain5Bit,
        battery: false,
    },
    Suite {
        name: "rtc3test",
        dir: "rtc3test",
        judge: Judge::Screenshot,
        frames: 0, // driven by the sub-test scripts below
        colour: RefEncoding::Plain5Bit,
        battery: true,
    },
];

/// `--frames N`, for investigating one row by hand. Zero means "use the
/// suite's own budget", which is what the gate always does.
static FRAMES_OVERRIDE: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(0);

fn suite(name: &str) -> &'static Suite {
    SUITES.iter().find(|s| s.name == name).unwrap_or_else(|| {
        let names: Vec<&str> = SUITES.iter().map(|s| s.name).collect();
        panic!("unknown suite {name:?}; known: {}", names.join(", "))
    })
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() {
    let (baseline, argv) =
        suite_baseline::take_baseline_arg(std::env::args().skip(1).collect());
    let mut args = argv.into_iter();
    let (mut name, mut dir, mut out, mut filter) = (None, None, None, None);
    let mut quiet = false;
    let mut frames_override: Option<u32> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--suite" => name = args.next(),
            "--dir" => dir = args.next().map(PathBuf::from),
            "--out" => out = args.next().map(PathBuf::from),
            "--filter" => filter = args.next(),
            // For investigating a row by hand; never used by the gate, whose
            // frame budgets come from each suite's own howto.
            "--frames" => frames_override = args.next().and_then(|v| v.parse().ok()),
            // `make test-external` gates 5,000-odd rows; the per-row lines are
            // for a human debugging one suite, not for a build log. The gate
            // still prints every row that differs from its baseline.
            "--quiet" => quiet = true,
            other => panic!("unexpected argument {other:?}"),
        }
    }
    let s = suite(&name.expect("--suite <name> is required"));
    if let Some(f) = frames_override {
        FRAMES_OVERRIDE.store(f, std::sync::atomic::Ordering::Relaxed);
    }
    let root = dir
        .expect("--dir <c-sp release root> is required")
        .join(s.dir);
    assert!(root.is_dir(), "{} is not a directory", root.display());
    if let Some(o) = &out {
        let _ = std::fs::create_dir_all(o);
    }

    let mut roms = Vec::new();
    collect(&root, &mut roms);
    roms.sort();
    assert!(!roms.is_empty(), "no ROMs under {}", root.display());

    let mut rows: Vec<suite_baseline::Row> = Vec::new();
    for rom in &roms {
        let key_base = key_for(rom, &root);
        if let Some(f) = &filter {
            if !key_base.contains(f.as_str()) {
                continue;
            }
        }
        for (key, value) in score(s, rom, &key_base, out.as_deref()) {
            if !quiet {
                println!("{value:38}  {key}");
            }
            rows.push(suite_baseline::Row::new(key, value));
        }
    }

    // A duplicate key would silently collapse two rows into one expectation.
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(before, keys.len(), "duplicate row keys in {}", s.name);

    let pass = rows.iter().filter(|r| r.value.starts_with("PASS")).count();
    let skip = rows.iter().filter(|r| r.value.starts_with("SKIP")).count();
    println!(
        "\n{}: {pass}/{} pass ({} fail, {skip} not scored)",
        s.name,
        rows.len() - skip,
        rows.len() - pass - skip
    );

    if let Some(path) = baseline {
        if !suite_baseline::gate(s.name, &path, &header(s), &rows) {
            std::process::exit(1);
        }
    }
}

fn header(s: &Suite) -> String {
    format!(
        "\
# `{name}` baseline — the EXPECTED verdict of every ROM in the {name} suite,
# judged by ITS OWN AUTHOR'S convention (see `{dir}/game-boy-test-roms-howto.md`
# in the release, and the module docs of examples/csp_suite.rs). Nothing here is
# a hash or a screenshot of our own output.
#
# The ROMs are NOT vendored. `tools/fetch-test-suites.sh` downloads the
# c-sp/game-boy-test-roms release pinned there and checks its SHA-256.
#
# A row key is `<path-under-the-suite>@<console>`, where the console is the word
# the ROM's or the reference screenshot's own file name used. `SKIP` marks a row
# this runner deliberately does not score and says why; those count in neither
# column.
#
# CI is green when reality matches this file and red on ANY change, in either
# direction: a row that starts PASSING is a change too. Re-bless deliberately:
#
#   UPDATE_EXTERNAL_BASELINE=1 make test-external
\n",
        name = s.name,
        dir = s.dir,
    )
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(p)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        entries.sort();
        for e in entries {
            collect(&e, out);
        }
    } else if p.extension().is_some_and(|e| e == "gb" || e == "gbc") {
        out.push(p.to_path_buf());
    }
}

/// A row key that is stable across machines: the ROM's path below the suite
/// root, without its extension.
fn key_for(rom: &Path, root: &Path) -> String {
    rom.strip_prefix(root)
        .unwrap_or(rom)
        .with_extension("")
        .to_string_lossy()
        .replace('\\', "/")
}

// ---------------------------------------------------------------------------
// Scoring
// ---------------------------------------------------------------------------

type Rows = Vec<(String, String)>;

fn score(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    match s.judge {
        Judge::Age => score_age(s, rom, key, out),
        Judge::WilbertPol => score_wilbertpol(s, rom, key, out),
        Judge::Microtest => score_microtest(rom, key),
        Judge::Gambatte => score_gambatte(s, rom, key, out),
        Judge::Screenshot => score_screenshot(s, rom, key, out),
    }
}

/// The cycle budget a register-verdict ROM gets before it is called a timeout.
/// The longest of these settle in well under a second of emulated time; this is
/// ~95 seconds.
const REG_BUDGET: u64 = 400_000_000;

fn fib(cpu: &Cpu<'static>) -> bool {
    let r = &cpu.registers;
    r.b == 3 && r.c == 5 && r.d == 8 && r.e == 13 && r.h == 21 && r.l == 34
}

fn dump(cpu: &Cpu<'static>) -> String {
    let r = &cpu.registers;
    format!(
        "b={:02X} c={:02X} d={:02X} e={:02X} h={:02X} l={:02X}",
        r.b, r.c, r.d, r.e, r.h, r.l
    )
}

fn registers(rom: &Path, key: &str, run: &Run, opcode: u8) -> (String, String) {
    let k = format!("{key}@{}", run.tag);
    let Some(mut m) = Machine::new(rom, run, false) else {
        return (k, "SKIP unreadable cartridge".into());
    };
    let v = if m.until_opcode(opcode, REG_BUDGET) {
        if fib(&m.cpu) {
            "PASS".to_string()
        } else {
            format!("FAIL {}", dump(&m.cpu))
        }
    } else {
        format!("FAIL no ${opcode:02X} reached")
    };
    (k, v)
}

/// AGE: a `LD B,B` breakpoint plus Fibonacci registers, except for the ROMs
/// whose result can only be seen — those ship the author's screenshots, one per
/// console, and are scored against them.
fn score_age(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    let refs = references(rom);
    if refs.is_empty() {
        let mut runs = devices_in(&rom.file_stem().unwrap().to_string_lossy());
        if runs.is_empty() {
            runs.push(Run::new(Model::Auto, "auto"));
        }
        return runs.iter().map(|r| registers(rom, key, r, 0x40)).collect();
    }
    refs.into_iter()
        .map(|(run, png)| screenshot_row(s, rom, key, &run, &png, out))
        .collect()
}

/// The wilbertpol fork is the 2016 Mooneye suite: an undefined `$ED` where
/// today's suite uses `LD B,B`. Its `manual-only` and `madness` groups are not
/// part of the automatic suite and ship reference screenshots instead;
/// `utils` and `logic-analysis` are dumpers with no verdict at all.
fn score_wilbertpol(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    if key.starts_with("utils/") || key.starts_with("logic-analysis/") {
        return vec![(
            format!("{key}@none"),
            "SKIP a state dumper, not a test".to_string(),
        )];
    }
    let refs = references(rom);
    if !refs.is_empty() {
        return refs
            .into_iter()
            .map(|(run, png)| screenshot_row(s, rom, key, &run, &png, out))
            .collect();
    }
    vec![registers(rom, key, &mooneye_run(rom), 0xED)]
}

fn score_microtest(rom: &Path, key: &str) -> Rows {
    // "tests have been checked on real hardware believed to be a DMG-CPU-08".
    let run = Run::new(Model::Dmg, "dmg");
    let Some(mut m) = Machine::new(rom, &run, false) else {
        return vec![(format!("{key}@dmg"), "SKIP unreadable cartridge".into())];
    };
    // "Running the emulation for two frames should be sufficient. The only
    // exception is `is_if_set_during_ime0.gb` which has to run for about 380
    // milliseconds." Run the two frames, then keep going only while the verdict
    // byte is still unwritten, so every test is read at the moment its own
    // README says it is ready and the one long test is not cut short.
    m.frames(2);
    let mut budget = 25 * CYCLES_PER_FRAME;
    while !matches!(m.cpu.memory.peek(0xFF82), 0x01 | 0xFF) && budget > 0 {
        budget = budget.saturating_sub(u64::from(m.cpu.do_cycle()));
    }
    let verdict = m.cpu.memory.peek(0xFF82);
    let (got, want) = (m.cpu.memory.peek(0xFF80), m.cpu.memory.peek(0xFF81));
    let v = match verdict {
        0x01 => "PASS".to_string(),
        0xFF => format!("FAIL got={got:02X} want={want:02X}"),
        other => format!("FAIL no verdict (ff82={other:02X})"),
    };
    vec![(format!("{key}@dmg"), v)]
}

// ---------------------------------------------------------------------------
// Gambatte
// ---------------------------------------------------------------------------

/// Split a Gambatte file name into the per-console expectations, exactly as
/// `testrunner.cpp`'s `main` does.
///
/// The `x` in `_xout0` is how the suite disables a test: upstream's
/// `find("_out")` misses it, so it never runs. We record it as `SKIP` rather
/// than dropping it, so the row count is the count of ROMs on disk.
///
/// AGB is deliberately not scored. `testrunner.cpp` runs the CGB expectation on
/// an AGB as well, under its own `FIXME: Actual AGB results`, and its exit
/// status ignores those rows — scoring them would double-count the CGB answer.
fn gambatte_expectations(stem: &str) -> Vec<(Run, String)> {
    let after = |marker: &str| -> Option<String> {
        let pos = stem.find(marker)?;
        Some(
            stem[pos + marker.len()..]
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect(),
        )
    };
    // `cgb04c` is CPU CGB C — the console the suite's own file names say its
    // Color expectations were taken from.
    let cgb = || Run {
        model: Model::Cgb,
        ppu: Some(PpuRevision::CgbC),
        tag: "cgb04c".to_string(),
    };
    if stem.contains("dmg08_cgb04c_out") {
        let e = after("dmg08_cgb04c_out").unwrap_or_default();
        return vec![(Run::new(Model::Dmg, "dmg08"), e.clone()), (cgb(), e)];
    }
    let mut out = Vec::new();
    if stem.contains("dmg08_out") {
        out.push((
            Run::new(Model::Dmg, "dmg08"),
            after("dmg08_out").unwrap_or_default(),
        ));
        if stem.contains("cgb04c_out") {
            out.push((cgb(), after("cgb04c_out").unwrap_or_default()));
        }
    } else if stem.contains("_out") {
        out.push((cgb(), after("_out").unwrap_or_default()));
    }
    out
}

/// The reference screenshots Gambatte's own runner looks for, and **only**
/// those three names.
///
/// The suite also ships bare `<stem>.png` files and `<stem>_gba.png` files that
/// `testrunner.cpp` never opens; picking those up scores a picture upstream
/// does not treat as an expectation (and, for the pair that differ only by
/// suffix, produces two rows claiming the same console).
fn gambatte_references(rom: &Path) -> Vec<(Run, PathBuf)> {
    let stem = rom.with_extension("");
    let both = stem.with_file_name(format!(
        "{}_dmg08_cgb04c.png",
        stem.file_name().unwrap().to_string_lossy()
    ));
    let cgb = Run {
        model: Model::Cgb,
        ppu: Some(PpuRevision::CgbC),
        tag: "cgb04c".to_string(),
    };
    if both.is_file() {
        return vec![(Run::new(Model::Dmg, "dmg08"), both.clone()), (cgb, both)];
    }
    let mut out = Vec::new();
    let name = |suffix: &str| {
        stem.with_file_name(format!(
            "{}{suffix}.png",
            stem.file_name().unwrap().to_string_lossy()
        ))
    };
    let c = name("_cgb04c");
    if c.is_file() {
        out.push((cgb, c));
    }
    let d = name("_dmg08");
    if d.is_file() {
        out.push((Run::new(Model::Dmg, "dmg08"), d));
    }
    out
}

/// `frameBufferMatchesOut`: the first `n` 8x8 tiles of the top row must draw
/// the hexadecimal string in Gambatte's own font.
fn digits_match(frame: &[u8], want: &str) -> bool {
    for (i, c) in want.chars().enumerate() {
        // Upstream stops at the first character that is not a hex digit.
        let Some(n) = c.to_digit(16) else { return true };
        if (i + 1) * 8 > W {
            return false;
        }
        let tile = OUT_TILES[n as usize];
        for y in 0..8 {
            for x in 0..8 {
                let p = ((y * W) + i * 8 + x) * 4;
                let black = (u32::from(frame[p]) << 16
                    | u32::from(frame[p + 1]) << 8
                    | u32::from(frame[p + 2]))
                    & 0xF8F8F8
                    == 0;
                if black != ((tile >> (63 - (y * 8 + x))) & 1 == 1) {
                    return false;
                }
            }
        }
    }
    true
}

fn score_gambatte(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    let stem = rom.file_stem().unwrap().to_string_lossy().to_string();
    let mut rows = Rows::new();

    for (run, want) in gambatte_expectations(&stem) {
        let k = format!("{key}@{}", run.tag);
        let Some(mut m) = Machine::new(rom, &run, false) else {
            rows.push((k, "SKIP unreadable cartridge".into()));
            continue;
        };
        m.frames(s.frames);
        let v = match want.as_str() {
            "audio0" if m.audio.audible() => "FAIL audible, expected silence".to_string(),
            "audio0" => "PASS".to_string(),
            "audio1" if m.audio.audible() => "PASS".to_string(),
            "audio1" => "FAIL silent, expected audio".to_string(),
            "" => "SKIP no expectation in the file name".to_string(),
            hex if digits_match(m.frame(), hex) => "PASS".to_string(),
            hex => format!("FAIL out={hex} not drawn"),
        };
        rows.push((k, v));
    }

    // A screenshot beside the ROM is a second, independent expectation and
    // upstream scores it as its own test.
    for (run, png) in gambatte_references(rom) {
        let (k, v) = screenshot_row(s, rom, key, &run, &png, out);
        rows.push((format!("{k}-png"), v));
    }

    if rows.is_empty() {
        rows.push((
            format!("{key}@none"),
            "SKIP no expectation (disabled upstream, or a state dumper)".to_string(),
        ));
    }
    rows
}

// ---------------------------------------------------------------------------
// Screenshot suites
// ---------------------------------------------------------------------------

/// Every reference screenshot that belongs to `rom`, with the console its file
/// name names.
///
/// **A picture belongs to the ROM with the longest matching stem.** Several
/// suites put `foo.gb` and `foo-nocgb.gb` in one directory, and a
/// shortest-match rule hands `foo-nocgb-ncmBCE.png` to `foo.gb` — a wrong
/// reference scored as a wrong emulator.
///
/// A picture whose name carries no console word is scored on the machine the
/// cartridge header asks for.
fn references(rom: &Path) -> Vec<(Run, PathBuf)> {
    let stem = rom.file_stem().unwrap().to_string_lossy().to_string();
    let dir = rom.parent().unwrap();
    let mut out: Vec<(Run, PathBuf)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut files: Vec<PathBuf> =
        entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.sort();
    let siblings: Vec<String> = files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "gb" || e == "gbc"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().to_string())
        .collect();

    for png in files
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
    {
        let name = png.file_stem().unwrap().to_string_lossy().to_string();
        let owner = siblings
            .iter()
            .filter(|s| owns(s, &name))
            .max_by_key(|s| s.len());
        if owner.map(String::as_str) != Some(stem.as_str()) {
            continue;
        }
        let tail = &name[stem.len()..];
        let devices = devices_in(tail);
        if devices.is_empty() {
            let cgb = std::fs::read(rom)
                .ok()
                .and_then(|d| d.get(0x143).copied())
                .is_some_and(|f| f & 0x80 != 0);
            out.push((
                if cgb {
                    Run::new(Model::Cgb, "cgb")
                } else {
                    Run::new(Model::Dmg, "dmg")
                },
                png.clone(),
            ));
        } else {
            for run in devices {
                out.push((run, png.clone()));
            }
        }
    }
    out
}

/// Could a picture called `name` belong to a ROM called `stem`? Requires an
/// exact match or a separator, so `statcount.gb` cannot claim
/// `statcount_auto-cgb-dmg.png` away from `statcount-auto.gb`.
///
/// **`-` and `_` are the same separator for this comparison.** Scribbltests
/// ships `statcount-auto.gb` beside a reference named `statcount_auto-…` —
/// the ROM uses a hyphen, the picture an underscore, at the exact position
/// that decides ownership. A literal `strip_prefix` sees that mismatch and
/// refuses `statcount-auto` outright, while `statcount` (nine characters,
/// with nothing after it but the picture's own underscore) still satisfies
/// the separator check unmodified — so the *wrong*, shorter ROM won the
/// longest-match contest by default, `statcount.gb` was scored against a
/// reference meant for `statcount-auto.gb` (and with the wrong frame count:
/// `screenshot_row` already special-cases 270 frames for `statcount-auto`,
/// dead code until this fix could ever route a ROM there), and
/// `statcount-auto.gb` itself was reported as having no reference at all.
/// Normalising both sides to one separator character before comparing fixes
/// the boundary check without changing which stem wins when both are
/// genuinely present (`max_by_key(|s| s.len())` still runs on the untouched,
/// original stem lengths — see the caller).
fn owns(stem: &str, name: &str) -> bool {
    let stem = stem.replace('_', "-");
    let name = name.replace('_', "-");
    let Some(tail) = name.strip_prefix(stem.as_str()) else {
        return false;
    };
    tail.is_empty() || tail.starts_with('-')
}

fn screenshot_row(
    s: &Suite,
    rom: &Path,
    key: &str,
    run: &Run,
    png: &Path,
    out: Option<&Path>,
) -> (String, String) {
    let k = format!("{key}@{}", run.tag);
    let staged = if s.battery {
        stage(rom)
    } else {
        rom.to_path_buf()
    };
    let Some(mut m) = Machine::new(&staged, run, s.battery) else {
        return (k, "SKIP unreadable cartridge".into());
    };
    // The one documented exception to a suite's frame budget. The howto puts
    // this at "around 270 frames"; measured directly (the `owns()` bug above
    // meant this branch could never actually run until now — it was matched
    // to the wrong ROM), the screen is still blank at 510 frames and has
    // shown its final "Test OK!"/"Test failed:" text and gone static by 520,
    // every time, at every frame count checked 520..=1000. `statcount-auto`
    // sweeps NOP options $01..=$FF, disabling and re-enabling the LCD for
    // each — 270 frames was never going to be enough time for 255 of those
    // round trips. 700 leaves real margin above the measured threshold.
    let mut frames = if key.contains("statcount-auto") {
        700
    } else {
        s.frames
    };
    let over = FRAMES_OVERRIDE.load(std::sync::atomic::Ordering::Relaxed);
    if over != 0 {
        frames = over;
    }
    if frames == 0 {
        // These ROMs execute `LD B,B` when they have finished drawing; give
        // them the rest of the frame before reading the screen.
        m.until_opcode(0x40, REG_BUDGET);
        m.frames(1);
    } else {
        m.frames(frames);
    }
    let v = compare(s, &m, png, &k, out);
    (k, v)
}

fn compare(s: &Suite, m: &Machine, png: &Path, key: &str, out: Option<&Path>) -> String {
    if let Some(o) = out {
        save_png(&o.join(format!("{}.png", key.replace('/', "_"))), m.frame());
    }
    let Some(reference) = load_reference(png) else {
        return format!("SKIP unreadable reference {}", png.display());
    };
    let Some(enc) = encoding_for(m.cpu.memory.gbmode, &reference, s.colour) else {
        return "SKIP reference is neither a DMG nor a compatibility palette".to_string();
    };
    match frame_diff(m.frame(), &reference, enc) {
        Err(why) => format!("SKIP {why}"),
        Ok(0) => "PASS".to_string(),
        Ok(d) => format!("FAIL diff={d}"),
    }
}

/// rtc3test is a menu: the sub-test is chosen with the d-pad and A, and each
/// then runs for a documented number of emulated seconds. Its howto states all
/// three, and the reference screenshots are named for them.
const RTC3: &[(&str, &[Button], u32)] = &[
    ("basic-tests", &[Button::A], 13),
    ("range-tests", &[Button::Down, Button::A], 8),
    (
        "sub-second-writes",
        &[Button::Down, Button::Down, Button::A],
        26,
    ),
];

#[derive(Copy, Clone)]
enum Button {
    A,
    Down,
}

fn score_screenshot(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    if s.name == "rtc3test" {
        return score_rtc3(s, rom, key, out);
    }
    // `tellinglys` only reaches its verdict after every button has been pressed
    // in an arbitrary order and is explicitly an entropy test; there is no
    // deterministic script for it, so it is recorded rather than guessed at.
    if key.contains("tellinglys") {
        return vec![(
            format!("{key}@none"),
            "SKIP needs interactive button entropy".to_string(),
        )];
    }
    let refs = references(rom);
    if refs.is_empty() {
        return vec![(
            format!("{key}@none"),
            "SKIP the suite ships no reference screenshot".to_string(),
        )];
    }
    refs.into_iter()
        .map(|(run, png)| screenshot_row(s, rom, key, &run, &png, out))
        .collect()
}

fn score_rtc3(s: &Suite, rom: &Path, key: &str, out: Option<&Path>) -> Rows {
    use terminalgb::KeypadKey;
    let mut rows = Rows::new();
    let staged = stage(rom);
    for (sub, script, seconds) in RTC3 {
        for (run, png) in references_named(rom, &format!("{key}-{sub}")) {
            let k = format!("{key}/{sub}@{}", run.tag);
            let Some(mut m) = Machine::new(&staged, &run, true) else {
                rows.push((k, "SKIP unreadable cartridge".into()));
                continue;
            };
            m.frames(30);
            for b in *script {
                let button = match b {
                    Button::A => KeypadKey::A,
                    Button::Down => KeypadKey::Down,
                };
                m.cpu.memory.keypad.keydown(button);
                m.frames(6);
                m.cpu.memory.keypad.keyup(button);
                m.frames(12);
            }
            m.frames(seconds * 60);
            let v = compare(s, &m, &png, &k, out);
            rows.push((k, v));
        }
    }
    rows
}

/// `references`, but for a ROM whose screenshots are named after a sub-test
/// rather than after the ROM (`rtc3test-basic-tests-dmg.png`).
fn references_named(rom: &Path, prefix: &str) -> Vec<(Run, PathBuf)> {
    let want = Path::new(prefix)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let dir = rom.parent().unwrap();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut pngs: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "png"))
        .collect();
    pngs.sort();
    for png in pngs {
        let name = png.file_stem().unwrap().to_string_lossy().to_string();
        let Some(tail) = name.strip_prefix(&want) else {
            continue;
        };
        for run in devices_in(tail) {
            out.push((run, png.clone()));
        }
    }
    out
}

/// Copy a ROM under `target/` so the MBC's `.sav` lands there and the fetched
/// suite stays clean — writing into it would also make a second run measure a
/// different machine.
fn stage(rom: &Path) -> PathBuf {
    let dir = Path::new("target/csp-suite");
    std::fs::create_dir_all(dir).expect("create target/csp-suite");
    let dst = dir.join(rom.file_name().unwrap());
    std::fs::copy(rom, &dst).unwrap_or_else(|e| panic!("stage {}: {e}", rom.display()));
    // A stale `.sav` would make a clock test measure the previous run's clock.
    let _ = std::fs::remove_file(dst.with_extension("sav"));
    dst
}

fn save_png(path: &Path, rgba: &[u8]) {
    if let Some(img) = image::RgbaImage::from_raw(W as u32, H as u32, rgba.to_vec()) {
        let _ = img.save(path);
    }
}

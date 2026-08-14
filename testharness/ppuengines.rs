//! **The two rendering engines, held against each other and against the clock.**
//!
//! ```text
//! cargo test --release --test ppuengines
//! ```
//!
//! There are two PPUs in this emulator now — the whole-scanline renderer
//! (`RenderMode::Fast`, the default) and the per-dot pixel FIFO
//! (`RenderMode::Accurate`) — and only one of them is covered by
//! `testharness/testroms.rs`, which runs whatever the default is. Everything a
//! second engine can do wrong is therefore invisible to CI unless something
//! asks it the same questions, which is what this does. The external suites
//! that measure the *accuracy* the FIFO exists for (Mealybug Tearoom, Mooneye
//! `acceptance/ppu`) are not vendored — they are not ours to ship — so the
//! claims pinned here are deliberately the ones a hermetic run can make:
//!
//! 1. **The accurate engine renders dmg-acid2 pixel-exactly.** Same reference
//!    image, same shade-rank rule, same standard as the fast engine. This is
//!    the check that caught the FIFO being painted over by the whole-scanline
//!    renderer: the picture was *almost* right, and only a pixel-exact
//!    comparison against somebody else's reference said so.
//! 2. **The two engines agree, frame for frame, on well-behaved ROMs.** That is
//!    the promise the render-mode switch makes to a player: an ordinary game
//!    looks identical either way, and the choice is cost, not appearance. It
//!    is also the sharpest regression test available without external ROMs —
//!    the fast engine is pinned by `testroms`, so tying the accurate one to it
//!    transitively pins the accurate one too.
//! 3. **The accurate engine stays fast enough to play.** It costs more by
//!    construction; the question is whether it costs *too* much. Measured
//!    against the real 16.7427 ms DMG frame, with a floor generous enough that
//!    only a genuine collapse trips it.
//!
//! Only the MIT-licensed vendored dmg-acid2 ROM is required. Blargg
//! `cpu_instrs` is used as a second, busier subject when the `tests/`
//! submodule happens to be checked out, and skipped with a printed note when
//! it is not — never silently.

use std::path::{Path, PathBuf};
use std::time::Instant;
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::Model;
use terminalgb::ppu::RenderMode;

mod pngdec;

const W: usize = 160;
const H: usize = 144;
const FRAME_CYCLES: u64 = 70_224;
/// 4.194304 MHz / 70224 cycles = 59.7275 Hz.
const FRAME_MS: f64 = 16.7427;

/// The vendored MIT test ROM and its author's reference image.
const ACID2: &str = "third_party/dmg-acid2/dmg-acid2.gb";
const ACID2_REF: &str = "third_party/dmg-acid2/reference-dmg.png";
/// Present only when the `retrio/gb-test-roms` submodule is initialised.
const CPU_INSTRS: &str = "tests/cpu_instrs/cpu_instrs.gb";

/// Run `frames` frames and return the framebuffer.
///
/// The ROM is staged into `target/` first: the MBC writes its `.sav` next to
/// the ROM path, and a test must never dirty a vendored file or a submodule
/// checkout (a standing trap in this repo).
fn render(rom: &str, mode: RenderMode, frames: u64) -> Vec<u8> {
    let staged = stage(rom);
    let data = std::fs::read(&staged).expect("read staged ROM");
    // Always pass the path: with `None` this core forces MBC3 on every cart.
    let mut cpu = Cpu::new_with_model(data, Some(staged), Model::Dmg);
    cpu.memory.gpu.set_render_mode(mode);
    let mut cycles: u64 = 0;
    while cycles < frames * FRAME_CYCLES {
        cycles += cpu.do_cycle() as u64;
    }
    cpu.memory.gpu.data.to_vec()
}

fn stage(rom: &str) -> PathBuf {
    let dir = Path::new("target/ppuengines");
    std::fs::create_dir_all(dir).expect("create target/ppuengines");
    let dest = dir.join(Path::new(rom).file_name().expect("ROM file name"));
    std::fs::copy(rom, &dest).unwrap_or_else(|e| panic!("stage {rom}: {e}"));
    dest
}

/// Pixels whose **shade rank** differs. Rank, not RGB: our shades are
/// `000000/606060/c0c0c0/ffffff` and the upstream reference's are
/// `000000/555555/aaaaaa/ffffff`, so a literal compare would fail for a reason
/// that has nothing to do with the PPU. Same rule as `testroms.rs` and
/// `tools/conformance/image.py`.
fn shade_diff(frame: &[u8], reference_rgb: &[u8]) -> usize {
    let ours = pngdec::quantise_shades(&pngdec::rgba_to_rgb(frame), 4);
    let theirs = pngdec::quantise_shades(reference_rgb, 4);
    ours.iter().zip(&theirs).filter(|(a, b)| a != b).count()
}

/// Milliseconds of wall clock per emulated frame, after a warm-up.
fn cost_ms(rom: &str, mode: RenderMode, frames: u64) -> f64 {
    let staged = stage(rom);
    let data = std::fs::read(&staged).expect("read staged ROM");
    let mut cpu = Cpu::new_with_model(data, Some(staged), Model::Dmg);
    cpu.memory.gpu.set_render_mode(mode);
    let mut warm: u64 = 0;
    while warm < 60 * FRAME_CYCLES {
        warm += cpu.do_cycle() as u64;
    }
    let start = Instant::now();
    let mut cycles: u64 = 0;
    while cycles < frames * FRAME_CYCLES {
        cycles += cpu.do_cycle() as u64;
    }
    start.elapsed().as_secs_f64() * 1000.0 / frames as f64
}

fn main() {
    let mut failures: Vec<String> = Vec::new();

    // ---- 1. the accurate engine against somebody else's reference ---------
    let reference = pngdec::decode(&std::fs::read(ACID2_REF).expect("read reference"))
        .expect("decode reference");
    assert_eq!(
        (reference.width, reference.height),
        (W, H),
        "the reference image is not a Game Boy frame"
    );
    let accurate = render(ACID2, RenderMode::Accurate, 600);
    let differ = shade_diff(&accurate, &reference.rgb);
    println!("dmg-acid2 @accurate  {differ}/{} pixels differ", W * H);
    if differ != 0 {
        failures.push(format!(
            "dmg-acid2 under the accurate engine: {differ}/{} pixels differ from {ACID2_REF}",
            W * H
        ));
    }

    // ---- 2. the two engines against each other ---------------------------
    let mut subjects = vec![("dmg-acid2", ACID2)];
    if Path::new(CPU_INSTRS).exists() {
        subjects.push(("blargg cpu_instrs", CPU_INSTRS));
    } else {
        println!(
            "note: {CPU_INSTRS} absent (gb-test-roms submodule not initialised) \
             — the busier of the two subjects is SKIPPED, not passed"
        );
    }
    for (name, rom) in &subjects {
        let fast = render(rom, RenderMode::Fast, 600);
        let acc = render(rom, RenderMode::Accurate, 600);
        let n = fast
            .chunks_exact(4)
            .zip(acc.chunks_exact(4))
            .filter(|(a, b)| a != b)
            .count();
        println!("{name:18} fast vs accurate: {n}/{} pixels differ", W * H);
        if n != 0 {
            failures.push(format!(
                "{name}: the two engines disagree on {n}/{} pixels. Either a \
                 well-behaved ROM has stopped looking the same on both — which \
                 breaks the promise the render-mode switch makes — or this ROM \
                 is not well-behaved and does not belong in this list.",
                W * H
            ));
        }
    }

    // ---- 3. the accurate engine against the clock ------------------------
    //
    // A floor, not a target. The measured figures on the machine this was
    // written on are 0.28 ms/frame fast and 0.57 accurate for dmg-acid2 —
    // 3.4 % of a frame — so 4 ms leaves an order of magnitude of headroom for
    // slower hardware and CI noise while still catching a collapse.
    const BUDGET_MS: f64 = 4.0;
    let fast_ms = cost_ms(ACID2, RenderMode::Fast, 400);
    let acc_ms = cost_ms(ACID2, RenderMode::Accurate, 400);
    println!(
        "cost  fast {fast_ms:.4} ms/frame ({:.1}x realtime)   accurate {acc_ms:.4} ms/frame \
         ({:.1}x realtime)   ratio {:.2}x",
        FRAME_MS / fast_ms,
        FRAME_MS / acc_ms,
        acc_ms / fast_ms
    );
    if acc_ms > BUDGET_MS {
        failures.push(format!(
            "the accurate engine costs {acc_ms:.3} ms/frame, over the {BUDGET_MS} ms floor \
             — it can no longer hold 59.7275 Hz with room to spare"
        ));
    }

    // ---- 4. the price we quote the player --------------------------------
    //
    // `RenderMode::IDENTICAL_COST` is printed on the Status row, in the toast
    // when `P` switches mode, and in the `?` overlay — it is the *whole* basis
    // on which somebody decides to trade speed for exactness, so it must stay a
    // measurement rather than becoming folklore. (A stale "audio costs 8 ms a
    // frame" is exactly what this repository has already been bitten by.)
    //
    // The band is wide because it is a ratio measured on a shared CI box, but
    // it is narrow enough to catch the constant being halved or doubled: the
    // ratio is stable within a run even when the absolute figures are not.
    let ratio = acc_ms / fast_ms;
    let quoted = f64::from(RenderMode::IDENTICAL_COST);
    if !(ratio / 1.5..=ratio * 1.5).contains(&quoted) {
        failures.push(format!(
            "RenderMode::IDENTICAL_COST says {quoted:.2}x but the engines measure \
             {ratio:.2}x ({fast_ms:.4} vs {acc_ms:.4} ms/frame) — re-measure and move \
             the constant, and the README/status-page figures with it"
        ));
    }

    if failures.is_empty() {
        println!("\nPASS: both engines agree, and the accurate one is pixel-exact on dmg-acid2");
        return;
    }
    eprintln!();
    for f in &failures {
        eprintln!("FAIL: {f}");
    }
    std::process::exit(1);
}

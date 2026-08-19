//! Save-state round-trip proof.
//!
//! The contract for a save state is: capture the machine, keep running the
//! original, then load the capture into a fresh machine and continue — the two
//! must produce **byte-identical frames** from then on. If any live field were
//! missing from the state, the two runs would drift apart and this test would
//! fail. That is exactly the determinism that also makes save states usable as
//! reproducible test fixtures.
//!
//! This lives in `testharness/` (not `tests/`) for the same reason as
//! `testroms.rs`: `tests/` is the gb-test-roms submodule and Cargo cannot track
//! `.rs` files inside it. Registered as `[[test]] name = "savestate"`.
//!
//! Run: `cargo test --release --test savestate`

use std::path::PathBuf;
use terminalgb::gameboy::Gameboy;
use terminalgb::savestate::{fnv1a64, StateError, FORMAT_VERSION, MAGIC};

// The vendored dmg-acid2 ROM is always present (unlike the gb-test-roms
// submodule), so this test always has something real to run. It is a full PPU
// exercise: background, window, and sprites.
const ACID2: &str = "third_party/dmg-acid2/dmg-acid2.gb";

// An optional gb-test-roms ROM whose screen visibly scrolls, so "frames are
// byte-identical" is a non-trivial claim. Skipped when the submodule is absent.
const BLARGG: &str = "tests/cpu_instrs/cpu_instrs.gb";

// These two exercise state the accuracy crew added: halt_bug.gb hammers the
// HALT-bug latch (`Cpu::halt_bug`), and mem_timing leans on the per-access timer
// stepping (`MMU::bus_ticked`). Round-tripping across them proves those fields
// are captured — a missed one would desync here. Skipped without the submodule.
const HALT_BUG: &str = "tests/halt_bug.gb";
const MEM_TIMING: &str = "tests/mem_timing/mem_timing.gb";

const WARMUP_FRAMES: u32 = 120;
const COMPARE_FRAMES: u32 = 240;

/// Copy a ROM into `target/` and open it from there. Battery-backed carts write
/// a `.gbsave` next to their path on drop; copying keeps the source tree (and
/// the gb-test-roms submodule) pristine — the same guard `testroms.rs` uses.
fn load(path: &str) -> Option<Gameboy> {
    let src = PathBuf::from(path);
    let data = std::fs::read(&src).ok()?;
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let _ = std::fs::create_dir_all(&dir);
    let dst = dir.join(src.file_name().unwrap());
    std::fs::write(&dst, &data).ok()?;
    Some(Gameboy::new(data, Some(dst)))
}

/// Run `warmup` frames, snapshot, then prove a fresh machine loaded from the
/// snapshot matches the original frame-for-frame for `compare` frames.
fn round_trip(label: &str, path: &str, warmup: u32, compare: u32) -> bool {
    let Some(mut reference) = load(path) else {
        println!("SKIP  {label}: ROM not found ({path})");
        return true;
    };

    for _ in 0..warmup {
        reference.frame();
    }

    // Capture the complete machine.
    let snapshot = reference.save_state();
    let snap_frame = fnv1a64(reference.image());
    println!(
        "  {label}: snapshot is {} bytes after {warmup} frames",
        snapshot.len()
    );

    // Load into a brand-new machine (same ROM) and check the visible frame is
    // restored exactly, before running a single further cycle.
    let mut restored = load(path).expect("ROM disappeared mid-test");
    restored
        .load_state(&snapshot)
        .expect("load_state rejected a state it just wrote");
    assert_eq!(
        fnv1a64(restored.image()),
        snap_frame,
        "{label}: framebuffer not restored byte-for-byte on load"
    );

    // Now run both forward in lockstep. Every frame must match — both the
    // framebuffer (the literal deliverable) and the full serialised machine
    // (a far stronger completeness check: any un-captured live field surfaces
    // here as a divergence).
    let mut frames_changed = 0u32;
    let mut prev_frame = snap_frame;
    for i in 0..compare {
        reference.frame();
        restored.frame();

        // `save_state` is `&mut` (it brings the lazily-advanced PPU current),
        // so take the framebuffers as owned bytes rather than holding two
        // borrows across the comparison below.
        let rf = reference.image().to_vec();
        let sf = restored.image().to_vec();
        assert!(
            rf == sf,
            "{label}: framebuffer diverged at frame {i} after load"
        );

        let rs = reference.save_state();
        let ss = restored.save_state();
        assert!(
            rs == ss,
            "{label}: full machine state diverged at frame {i} after load"
        );

        let h = fnv1a64(&rf);
        if h != prev_frame {
            frames_changed += 1;
            prev_frame = h;
        }
    }

    println!(
        "  {label}: {compare} frames identical after load ({frames_changed} distinct frames observed)"
    );
    true
}

/// Snapshot at **every** frame boundary over a window: save, load into a fresh
/// machine, step both once more, and require identical output + state. This is
/// what gives the completeness check teeth for fields that are only briefly
/// live (e.g. `Cpu::halt_bug`, set right after a bugged HALT and cleared on the
/// next fetch): a save that lands on such a boundary and drops the field makes
/// the fresh machine diverge on the very next frame. Reports how many boundaries
/// carried non-default transient state so the coverage is visible.
fn stress_every_frame(label: &str, path: &str, frames: u32) {
    let Some(mut reference) = load(path) else {
        println!("SKIP  {label} (stress): ROM not found ({path})");
        return;
    };
    let mut divergent_guard = 0u32;
    for i in 0..frames {
        reference.frame();
        let snap = reference.save_state();

        let mut restored = load(path).expect("ROM disappeared mid-test");
        restored
            .load_state(&snap)
            .expect("load rejected own snapshot");
        // Identical the instant it loads…
        assert!(
            reference.image() == restored.image(),
            "{label}: framebuffer differs immediately after load at frame {i}"
        );
        assert!(
            reference.save_state() == restored.save_state(),
            "{label}: state differs immediately after load at frame {i}"
        );
        // …and identical after one more step (catches transient latches).
        reference.frame();
        restored.frame();
        assert!(
            reference.image() == restored.image(),
            "{label}: framebuffer diverged one frame after a load at frame {i}"
        );
        let a = reference.save_state();
        let b = restored.save_state();
        assert!(
            a == b,
            "{label}: state diverged one frame after a load at frame {i}"
        );
        if a != snap {
            divergent_guard += 1;
        }
    }
    println!(
        "  {label}: save/load at all {frames} frame boundaries stayed identical \
         ({divergent_guard} advanced state between save and next frame)"
    );
}

/// Save/load at every *instruction* boundary over a window. The HALT-bug latch
/// (`Cpu::halt_bug`) is live for exactly one instruction and is essentially
/// never observable at a frame boundary, so the frame-level proof cannot see
/// it — this can. It also asserts the latch was actually set at least once, so
/// we know the teeth engaged. (Verified: dropping `halt_bug` from `load_state`
/// makes this fail.)
fn stress_every_instruction(
    label: &str,
    path: &str,
    warmup_frames: u32,
    instruction_cap: u32,
    want_latch_hits: u32,
) {
    let Some(mut reference) = load(path) else {
        println!("SKIP  {label} (insn stress): ROM not found ({path})");
        return;
    };
    // Run into the ROM until it reaches the HALT-bug exercise.
    for _ in 0..warmup_frames {
        reference.frame();
    }
    // Reuse one restored machine (load overwrites all state) to avoid re-parsing
    // the ROM every iteration.
    let mut restored = load(path).expect("ROM disappeared mid-test");
    let mut halt_bug_seen = 0u32;
    let mut stepped = 0u32;
    // Step cheaply (no serialisation) until the latch is set; only then pay for
    // a save/load/compare. This is what makes reaching the deep HALT-bug test
    // code affordable.
    while stepped < instruction_cap && halt_bug_seen < want_latch_hits {
        reference.step_instruction();
        stepped += 1;
        if !reference.debug_halt_bug() {
            continue;
        }
        halt_bug_seen += 1;

        let snap = reference.save_state();
        restored
            .load_state(&snap)
            .expect("load rejected own snapshot");
        assert!(
            restored.debug_halt_bug(),
            "{label}: halt_bug latch not restored (save landed on a bugged HALT)"
        );
        // Step both once more; the latch drives a PC rewind, so a dropped latch
        // diverges here.
        reference.step_instruction();
        restored.step_instruction();
        assert!(
            reference.save_state() == restored.save_state(),
            "{label}: state diverged one instruction after loading a HALT-bug state"
        );
    }
    assert!(
        halt_bug_seen > 0,
        "{label}: never observed the HALT-bug latch set in {stepped} instructions — \
         test proves nothing; pick a ROM/window that trips it"
    );
    println!(
        "  {label}: save/load on a live HALT-bug latch stayed identical \
         ({halt_bug_seen} occurrences over {stepped} instructions)"
    );
}

/// The state must reject anything that isn't a state it could have written.
fn negative_checks() {
    let mut gb = load(ACID2).expect("acid2 ROM required for negative checks");
    gb.frame();
    let good = gb.save_state();

    // Bad magic.
    match gb.load_state(b"not a save state at all") {
        Err(StateError::BadMagic) => {}
        other => panic!("expected BadMagic, got {other:?}"),
    }

    // Wrong version: keep the magic, corrupt the version word.
    let mut bad_version = good.clone();
    bad_version[4] = 0xFF;
    bad_version[5] = 0xFF;
    match gb.load_state(&bad_version) {
        Err(StateError::UnsupportedVersion(v)) => assert_eq!(v, 0xFFFF),
        other => panic!("expected UnsupportedVersion, got {other:?}"),
    }

    // Truncated blob.
    match gb.load_state(&good[..good.len() / 2]) {
        Err(StateError::Truncated) => {}
        other => panic!("expected Truncated, got {other:?}"),
    }

    // Wrong game: change the cartridge title + global checksum, then try to
    // load acid2's state into it.
    let mut other_data = std::fs::read(ACID2).unwrap();
    other_data[0x0134] = b'Z'; // title byte
    other_data[0x014E] ^= 0xFF; // global checksum high byte
    let mut other_gb = Gameboy::new(other_data, Some(PathBuf::from(ACID2)));
    match other_gb.load_state(&good) {
        Err(StateError::RomMismatch { .. }) => {}
        other => panic!("expected RomMismatch, got {other:?}"),
    }

    // Sanity: the good state still loads.
    gb.load_state(&good).expect("good state should still load");

    println!("  negative checks passed (magic / version / truncation / wrong-game all rejected)");
}

fn main() {
    println!(
        "save-state round-trip test (format v{FORMAT_VERSION}, magic {:?})",
        std::str::from_utf8(&MAGIC).unwrap()
    );

    // Emulation must be deterministic for any of this to hold.
    {
        let mut a = load(ACID2).expect("acid2 ROM required");
        let mut b = load(ACID2).expect("acid2 ROM required");
        for _ in 0..60 {
            a.frame();
            b.frame();
        }
        assert_eq!(
            a.save_state(),
            b.save_state(),
            "emulation is not deterministic; save-state comparison would be meaningless"
        );
    }

    round_trip("acid2", ACID2, WARMUP_FRAMES, COMPARE_FRAMES);
    round_trip("blargg/cpu_instrs", BLARGG, 200, COMPARE_FRAMES);
    // Directly exercise the state the accuracy crew added.
    round_trip("blargg/halt_bug", HALT_BUG, 90, COMPARE_FRAMES);
    round_trip("blargg/mem_timing", MEM_TIMING, 120, COMPARE_FRAMES);
    // Save/load at every boundary — reliably catches briefly-live fields.
    stress_every_frame("blargg/halt_bug", HALT_BUG, 300);
    stress_every_frame("blargg/cpu_instrs", BLARGG, 200);
    // Instruction-granular: the only way to land a save on the HALT-bug latch.
    stress_every_instruction("blargg/halt_bug", HALT_BUG, 60, 20_000_000, 2);
    negative_checks();

    println!("ALL SAVE-STATE ROUND-TRIP CHECKS PASSED");
}

//! The reference implementation of the **gbconform/1 adapter contract**.
//!
//! This is what an emulator has to provide in order to be scored by the
//! conformance action (`.github/workflows/conformance.yml`, runner in
//! `tools/conformance/gbconform.py`, contract written out in
//! [`docs/conformance-action.md`](../docs/conformance-action.md)). Our own
//! emulator is deliberately the first adapter, so the contract is proven
//! against real output rather than invented in the abstract — that was flagged
//! in [`docs/conformance.md`](../docs/conformance.md) as the single most
//! important early design decision, and this file is it.
//!
//! Read it as documentation: **this is the whole ask.** Take a ROM path, run
//! it, and write at most two files. There is no protocol to speak, nothing to
//! parse on stdout, and no dependency on this project.
//!
//! ```text
//! GBCONFORM_ROM         the ROM (also argv[1])
//! GBCONFORM_MODEL       "dmg" | "cgb"
//! GBCONFORM_FRAMES      run at least this many frames
//! GBCONFORM_SERIAL_OUT  set when serial bytes are wanted → write them here
//! GBCONFORM_FRAME_OUT   set when a frame is wanted → write it here
//! GBCONFORM_TIMEOUT_S   wall-clock budget
//! ```
//!
//! Build and use:
//!
//! ```text
//! cargo build --release --example conformance_adapter
//! python3 tools/conformance/gbconform.py \
//!     --adapter target/release/examples/conformance_adapter --out out
//! ```
//!
//! ## Two things worth copying into your own adapter
//!
//! * **Stop early on a serial verdict.** Blargg's ROMs print `Passed`/`Failed`
//!   and then loop forever. Running the full frame budget every time turns a
//!   two-second suite into a two-minute one.
//! * **Give the cartridge a private copy of the ROM.** Anything with a battery
//!   writes a save file next to the ROM path; pointing a test runner at a
//!   pristine checkout dirties it. (Here the runner already stages ROMs into a
//!   temporary directory, but the emulator picking its save path from the ROM
//!   path is a general trap — see `src/mbc/battery.rs`.)

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use terminalgb::cpu::core::Cpu;

/// Safety cap so a ROM that never completes a frame cannot spin forever. The
/// runner also enforces a wall-clock timeout; this one keeps the result
/// *deterministic*, which a wall-clock limit cannot.
const CYCLE_CAP: u64 = 250_000_000;

fn main() {
    let rom_path = std::env::var_os("GBCONFORM_ROM")
        .map(PathBuf::from)
        .or_else(|| std::env::args_os().nth(1).map(PathBuf::from));
    let Some(rom_path) = rom_path else {
        eprintln!("usage: conformance_adapter <rom.gb>  (or set GBCONFORM_ROM)");
        std::process::exit(2);
    };

    let frames: u32 = std::env::var("GBCONFORM_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(600);
    let serial_out = std::env::var_os("GBCONFORM_SERIAL_OUT").map(PathBuf::from);
    let frame_out = std::env::var_os("GBCONFORM_FRAME_OUT").map(PathBuf::from);

    let data = match std::fs::read(&rom_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {}: {e}", rom_path.display());
            std::process::exit(2);
        }
    };
    // Always pass the path: with `None` this core forces MBC3 for every
    // cartridge and mis-drives the rest (a documented sharp edge).
    let mut cpu = Cpu::new(data, Some(rom_path.clone()));

    // Serial capture. The contract asks for the raw bytes the ROM sends to the
    // link port — no framing, no interpretation. Whoever reads them decides
    // what they mean.
    let serial = Arc::new(Mutex::new(Vec::<u8>::new()));
    if serial_out.is_some() {
        let sink = serial.clone();
        cpu.memory.serial.set_callback(Box::new(move |b: u8| {
            sink.lock().unwrap().push(b);
            None
        }));
    }

    let mut cap = CYCLE_CAP;
    for _ in 0..frames {
        if !step_frame(&mut cpu, &mut cap) {
            break;
        }
        // A ROM that has printed its verdict is done; the rest is an idle loop.
        if serial_out.is_some() && verdict_reached(&serial) {
            break;
        }
    }

    if let Some(path) = serial_out {
        write_or_die(&path, &serial.lock().unwrap());
    }
    if let Some(path) = frame_out {
        // The framebuffer is RGBA at 160x144 — the contract accepts a raw dump
        // exactly so that an emulator with no image encoder can answer with one
        // `write()`. We do the same rather than reaching for the `image` crate.
        write_or_die(&path, &cpu.memory.gpu.data[..]);
    }
}

/// Run until the PPU reports a completed frame, or the cycle cap runs out.
fn step_frame(cpu: &mut Cpu<'static>, cap: &mut u64) -> bool {
    loop {
        *cap = cap.saturating_sub(cpu.do_cycle() as u64);
        if *cap == 0 {
            return false;
        }
        if cpu.memory.gpu.updated {
            cpu.memory.gpu.updated = false;
            return true;
        }
    }
}

fn verdict_reached(serial: &Arc<Mutex<Vec<u8>>>) -> bool {
    let buf = serial.lock().unwrap();
    let text = String::from_utf8_lossy(&buf);
    text.contains("Passed") || text.contains("Failed")
}

fn write_or_die(path: &PathBuf, bytes: &[u8]) {
    match std::fs::File::create(path).and_then(|mut f| f.write_all(bytes)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("cannot write {}: {e}", path.display());
            std::process::exit(3);
        }
    }
}

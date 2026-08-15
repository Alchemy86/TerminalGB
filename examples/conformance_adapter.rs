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
//! * **Honour `GBCONFORM_MODEL`.** It is not decoration: `oam_bug` is a *pass*
//!   on a DMG and a *fail* on a Color console, because Color silicon has no
//!   OAM-corruption defect at all, and `interrupt_time` is a CGB-only
//!   cartridge. An adapter that auto-detects the console from the cartridge
//!   header instead answers a question nobody asked. This file documented the
//!   variable and ignored it until a check finally needed it.

use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use terminalgb::cpu::core::Cpu;
use terminalgb::mode::Model;

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
    // `GBCONFORM_MODEL` names the console to emulate. An unset or unknown
    // value means "you choose", which for us is `Model::Auto` — the CGB flag
    // in the cartridge header, i.e. what a person owning both consoles would
    // do. A *stated* model always wins over the header: forcing means forcing.
    let model = std::env::var("GBCONFORM_MODEL")
        .ok()
        .and_then(|v| Model::parse(&v))
        .unwrap_or(Model::Auto);

    // Pass the path so cartridge RAM is battery-backed where the ROM expects
    // it. It does not choose the mapper — that is header byte `$0147`.
    let mut cpu = Cpu::new_with_model(data, Some(rom_path.clone()), model);

    // A machine with no sound chip is not a quiet Game Boy, it is a Game Boy
    // with a hole where the APU should be: `Sound::do_cycle` returns at once
    // when no player is installed, so `NR52` never reports itself powered on.
    // `interrupt_time` spins on exactly that and never leaves its title
    // screen — it looks like a hang, and was written off as "cannot run
    // headless" in this project once already. Install a silent player.
    cpu.memory
        .enable_audio(Box::new(terminalgb::sound::SilentPlayer));

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

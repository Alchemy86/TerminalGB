//! Battery-backed save durability harness.
//!
//! ## Why this exists
//!
//! Cartridge RAM used to be written to disk **only** in `impl Drop for MBCx`.
//! No frontend can promise that runs: `winit`'s event loop calls
//! `process::exit`, `[profile.release] panic = 'abort'` skips destructors, and
//! closing a window or `kill`ing the process skips them too. The observable
//! symptom is the worst one an emulator has: *you save inside the game, quit,
//! relaunch, and the title screen offers only "NEW GAME"* — and then the next
//! clean exit writes the fresh, empty RAM over whatever save file was there.
//!
//! No public test ROM covers this: Blargg, Mooneye GB and the acid2 suites all
//! run in one process and never check what reaches the filesystem. Persistence
//! is emulator behaviour, not hardware behaviour, so it needs its own harness —
//! which is exactly the kind of gap worth carrying into a standalone
//! conformance tool.
//!
//! ## Hardware behaviour being modelled
//!
//! A battery-backed cartridge holds SRAM through a coin cell; the data is safe
//! the instant the game writes it, with no "quit cleanly" step anywhere. Pan
//! Docs, *MBCs / Battery-buffered RAM*: <https://gbdev.io/pandocs/MBC3.html>.
//! An emulator therefore has to persist cartridge RAM **while it runs**, not at
//! shutdown.
//!
//! ## What it checks
//!
//! Runs a hand-assembled MBC3+RAM+BATTERY ROM that enables cartridge RAM and
//! writes a distinct marker into each of the four 8 KiB banks, then:
//!
//! 1. the save file appears **while the emulator runs**, with no `Drop`;
//! 2. it is a raw `.sav` — cartridge RAM byte-for-byte, no header — so other
//!    emulators can read it;
//! 3. every bank comes back through the bank register on a fresh machine
//!    (a wrong bank order passes a size check but fails a game's checksum);
//! 4. a legacy `.gbsave` still loads, migrates to `.sav`, and is left intact;
//! 5. a session that never writes cartridge RAM creates no save file — the old
//!    code's phantom all-zero write is how good saves got erased.
//!
//! Run with `cargo test --release --test battery`.

use std::path::{Path, PathBuf};
use terminalgb::gameboy::Gameboy;

// ---------------------------------------------------------------------------
// A minimal MBC3+RAM+BATTERY test cartridge, assembled by hand.
// ---------------------------------------------------------------------------

/// Marker written at offset 0 of RAM bank `b`.
fn bank_marker(b: u8) -> u8 {
    0xC0 | b
}
/// Marker written at the last byte of RAM bank `b`.
fn bank_tail(b: u8) -> u8 {
    0x50 | b
}

/// Build a 32 KiB ROM whose entry point enables cartridge RAM, stamps a marker
/// into the first and last byte of each of the four RAM banks, then spins.
fn build_battery_rom() -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];

    // Entry point at 0x0100: NOP; JP 0x0150.
    rom[0x0100] = 0x00;
    rom[0x0101] = 0xC3;
    rom[0x0102] = 0x50;
    rom[0x0103] = 0x01;

    // Header. Title, then the fields the MBC layer actually reads.
    for (i, b) in b"BATTERYTEST".iter().enumerate() {
        rom[0x0134 + i] = *b;
    }
    rom[0x0143] = 0x00; // not CGB-only
    rom[0x0147] = 0x13; // MBC3 + RAM + BATTERY
    rom[0x0148] = 0x00; // 32 KiB ROM (2 banks)
    rom[0x0149] = 0x03; // 32 KiB RAM (4 banks of 8 KiB)

    // Header checksum over 0x0134..=0x014C.
    let mut sum: u8 = 0;
    for b in rom.iter().take(0x014D).skip(0x0134) {
        sum = sum.wrapping_sub(*b).wrapping_sub(1);
    }
    rom[0x014D] = sum;

    let mut pc = 0x0150;
    let mut emit = |bytes: &[u8], pc: &mut usize| {
        rom[*pc..*pc + bytes.len()].copy_from_slice(bytes);
        *pc += bytes.len();
    };

    // LD A,$0A ; LD ($0000),A  — the RAM-enable magic value (Pan Docs: only
    // the low nibble 0xA unlocks cartridge RAM).
    emit(&[0x3E, 0x0A], &mut pc);
    emit(&[0xEA, 0x00, 0x00], &mut pc);

    for b in 0u8..4 {
        // LD A,b ; LD ($4000),A  — RAM bank select.
        emit(&[0x3E, b], &mut pc);
        emit(&[0xEA, 0x00, 0x40], &mut pc);
        // LD A,marker ; LD ($A000),A
        emit(&[0x3E, bank_marker(b)], &mut pc);
        emit(&[0xEA, 0x00, 0xA0], &mut pc);
        // LD A,tail ; LD ($BFFF),A
        emit(&[0x3E, bank_tail(b)], &mut pc);
        emit(&[0xEA, 0xFF, 0xBF], &mut pc);
    }

    // JR -2: spin forever, leaving cartridge RAM untouched so the battery
    // settles and writes out.
    emit(&[0x18, 0xFE], &mut pc);

    rom
}

/// The same cartridge, but its entry point deliberately runs off the rails and
/// then scribbles over cartridge RAM — a crashed Game Boy, in fifteen bytes.
///
/// This is the shape the Colosseum bug takes on real hardware: Pokémon Blue's
/// program counter leaves its own code, the CPU keeps executing whatever it
/// finds, and within a handful of frames thousands of bytes of cartridge RAM
/// have changed. Modelled faithfully: the LCD is switched off (as the real
/// crash does), a small routine is copied into OAM and control is handed to it;
/// it fills RAM bank 0 with `$5A` and then walks off the end of OAM into the
/// **unusable block** at `$FEA0`, which is where the emulator notices.
///
/// **Running from OAM is not itself the crash signal, and must not be.** OAM is
/// real RAM the CPU may read outside PPU modes 2 and 3, and three Mooneye
/// acceptance ROMs that pass execute from it on purpose — so the runaway-PC
/// heuristic covers `$FEA0-$FF7F` only (`Cpu::operation`; see
/// `docs/conformance-notes.md`). This cartridge therefore ends by jumping into
/// the unusable block rather than spinning inside OAM, which is also what a real
/// runaway program counter does: it keeps walking.
fn build_crash_rom() -> Vec<u8> {
    // The runaway routine, copied into OAM at $FE00 and run from there.
    #[rustfmt::skip]
    let runaway: &[u8] = &[
        0x3E, 0x0A,             // LD A,$0A
        0xEA, 0x00, 0x00,       // LD ($0000),A   ; enable cartridge RAM
        0xAF,                   // XOR A
        0xEA, 0x00, 0x40,       // LD ($4000),A   ; RAM bank 0
        0x21, 0x00, 0xA0,       // LD HL,$A000
        0x3E, 0x5A,             // .loop LD A,$5A
        0x22,                   // LD (HL+),A
        0x7C,                   // LD A,H
        0xFE, 0xC0,             // CP $C0
        0x20, 0xF8,             // JR NZ,.loop
        0xC3, 0xA0, 0xFE,       // JP $FEA0       ; walk off the end of OAM into
                                //                ; the unusable block — memory
                                //                ; that really cannot hold code
    ];

    let mut rom = build_battery_rom();
    // Park the routine in ROM where the loader can copy it from.
    rom[0x0200..0x0200 + runaway.len()].copy_from_slice(runaway);

    #[rustfmt::skip]
    let loader: &[u8] = &[
        0x3E, 0x00,             // LD A,0
        0xE0, 0x40,             // LDH ($40),A    ; LCD off, as the real crash does
        0x21, 0x00, 0x02,       // LD HL,$0200
        0x11, 0x00, 0xFE,       // LD DE,$FE00
        0x06, 23,               // LD B,<len>
        0x2A,                   // .copy LD A,(HL+)
        0x12,                   // LD (DE),A
        0x13,                   // INC DE
        0x05,                   // DEC B
        0x20, 0xFA,             // JR NZ,.copy
        0xC3, 0x00, 0xFE,       // JP $FE00       ; leave the program
    ];
    assert_eq!(loader[11] as usize, runaway.len(), "copy length");
    // Overwrite the entry point: this cartridge only crashes.
    rom[0x0150..0x0150 + loader.len()].copy_from_slice(loader);
    rom
}

// ---------------------------------------------------------------------------
// Harness plumbing
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn workdir(tag: &str) -> PathBuf {
    let dir = PathBuf::from("target/battery-harness").join(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create work dir");
    dir
}

/// Stage the ROM on disk and return its path (the battery writes its files
/// next to the ROM, so each case needs its own directory).
fn stage(dir: &Path) -> PathBuf {
    let rom = dir.join("battery.gb");
    std::fs::write(&rom, build_battery_rom()).expect("write test rom");
    rom
}

fn boot(rom: &Path) -> Gameboy {
    let data = std::fs::read(rom).expect("read test rom");
    Gameboy::new(data, Some(rom.to_path_buf()))
}

/// Read cartridge RAM back the way a game does: through the bank register.
/// Reading the file is not enough — a save whose banks are stored in the wrong
/// order is the classic reason a game's own checksum rejects it and silently
/// offers a new game.
fn read_bank_markers(gb: &mut Gameboy) -> Vec<(u8, u8)> {
    let mut out = Vec::new();
    for b in 0u8..4 {
        gb.debug_write(0x0000, 0x0A); // enable cartridge RAM
        gb.debug_write(0x4000, b); // select bank
        out.push((gb.debug_read(0xA000), gb.debug_read(0xBFFF)));
    }
    out
}

fn expected_markers() -> Vec<(u8, u8)> {
    (0u8..4).map(|b| (bank_marker(b), bank_tail(b))).collect()
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// The headline case: the game writes cartridge RAM, the process dies without
/// ever running a destructor, and the save must still be on disk and complete.
fn case_survives_an_unclean_exit() -> Case {
    let dir = workdir("unclean-exit");
    let rom = stage(&dir);
    let sav = dir.join("battery.sav");

    let mut gb = boot(&rom);
    for _ in 0..180 {
        gb.frame();
    }
    // No `drop(gb)`: this is a closed window / SIGKILL / winit's process::exit.
    std::mem::forget(gb);

    let bytes = match std::fs::read(&sav) {
        Ok(b) => b,
        Err(e) => {
            return Case {
                name: "in-game save survives an unclean exit",
                ok: false,
                detail: format!(
                    "no {} after 180 frames ({e}).\n      \
                     The emulator persists cartridge RAM only when it shuts down cleanly, \
                     so a crash, a kill, or a closed window loses everything the player \
                     saved. Hardware keeps it on a battery the instant the game writes it: \
                     persist while running, not on exit.",
                    sav.display()
                ),
            };
        }
    };

    // A raw .sav is exactly the cartridge RAM: 4 banks x 8 KiB, no header.
    if bytes.len() != 0x8000 {
        return Case {
            name: "in-game save survives an unclean exit",
            ok: false,
            detail: format!(
                "{} is {} bytes, expected 32768 (4 x 8 KiB banks, no header).\n      \
                 A `.sav` with a header cannot be read by other emulators.",
                sav.display(),
                bytes.len()
            ),
        };
    }

    let mut wrong = Vec::new();
    for b in 0u8..4 {
        let base = b as usize * 0x2000;
        if bytes[base] != bank_marker(b) {
            wrong.push(format!(
                "bank {b} byte 0 = {:02X}, expected {:02X}",
                bytes[base],
                bank_marker(b)
            ));
        }
        if bytes[base + 0x1FFF] != bank_tail(b) {
            wrong.push(format!(
                "bank {b} byte 0x1FFF = {:02X}, expected {:02X}",
                bytes[base + 0x1FFF],
                bank_tail(b)
            ));
        }
    }
    if !wrong.is_empty() {
        return Case {
            name: "in-game save survives an unclean exit",
            ok: false,
            detail: format!(
                "cartridge RAM banks are not laid out in order: {}.\n      \
                 Banks stored out of order still give the right file size, so the \
                 file looks fine — but the game's own save checksum fails and it \
                 offers a new game.",
                wrong.join("; ")
            ),
        };
    }

    Case {
        name: "in-game save survives an unclean exit",
        ok: true,
        detail: format!("{} bytes, all 4 banks in order", bytes.len()),
    }
}

/// The save must come back through the bank register on a fresh machine — the
/// player's actual "quit and continue" experience.
fn case_reload_restores_every_bank() -> Case {
    let dir = workdir("reload");
    let rom = stage(&dir);

    let mut gb = boot(&rom);
    for _ in 0..180 {
        gb.frame();
    }
    std::mem::forget(gb);

    let mut fresh = boot(&rom);
    let got = read_bank_markers(&mut fresh);
    let want = expected_markers();
    let ok = got == want;
    Case {
        name: "a relaunched machine reads back every RAM bank",
        ok,
        detail: if ok {
            "all 4 banks match".to_string()
        } else {
            format!(
                "read {got:02X?}, expected {want:02X?}.\n      \
                 The save loaded but arrives wrong through the bank register, which is \
                 indistinguishable from an empty save to the game."
            )
        },
    }
}

/// A session that never touches cartridge RAM must not create a save file. The
/// old `Drop`-based code wrote its pristine all-zero RAM unconditionally, so a
/// single launch that failed to load a save would overwrite it with zeros.
fn case_a_boot_that_never_saves_writes_nothing() -> Case {
    let dir = workdir("no-phantom");
    let rom = dir.join("battery.gb");
    // Same cart, but with the RAM-writing program replaced by a bare spin loop.
    let mut data = build_battery_rom();
    data[0x0150] = 0x18;
    data[0x0151] = 0xFE;
    for b in data.iter_mut().take(0x0200).skip(0x0152) {
        *b = 0x00;
    }
    std::fs::write(&rom, data).expect("write rom");

    // Pre-existing good save the boot must not touch.
    let sav = dir.join("battery.sav");
    let good = vec![0x77u8; 0x8000];
    std::fs::write(&sav, &good).expect("write save");

    let mut gb = boot(&rom);
    for _ in 0..180 {
        gb.frame();
    }
    drop(gb); // even a *clean* exit must not rewrite untouched RAM

    let after = std::fs::read(&sav).unwrap_or_default();
    let ok = after == good;
    Case {
        name: "a session that saves nothing leaves the save file alone",
        ok,
        detail: if ok {
            "existing save untouched".to_string()
        } else {
            format!(
                "the save file changed ({} bytes, first difference at {:?}).\n      \
                 A launch that did not load or did not save must never write over an \
                 existing save — that turns one bad launch into permanent data loss.",
                after.len(),
                after.iter().zip(good.iter()).position(|(a, b)| a != b)
            )
        },
    }
}

/// The emulator's own legacy `<rom>.gbsave` (8-byte big-endian RTC header, then
/// RAM) must still load, migrate to `.sav`, and be left on disk as a backup.
fn case_legacy_gbsave_migrates() -> Case {
    let dir = workdir("legacy");
    let rom = stage(&dir);

    let mut legacy = 0u64.to_be_bytes().to_vec();
    for b in 0u8..4 {
        let mut bank = vec![0u8; 0x2000];
        bank[0] = bank_marker(b);
        bank[0x1FFF] = bank_tail(b);
        legacy.extend_from_slice(&bank);
    }
    let legacy_path = dir.join("battery.gbsave");
    std::fs::write(&legacy_path, &legacy).expect("write legacy save");

    let mut gb = boot(&rom);
    let got = read_bank_markers(&mut gb);
    let want = expected_markers();
    // Run long enough for the migration write, then die uncleanly.
    for _ in 0..180 {
        gb.frame();
    }
    std::mem::forget(gb);

    let migrated = std::fs::read(dir.join("battery.sav")).unwrap_or_default();
    let backup = std::fs::read(&legacy_path).unwrap_or_default();

    let mut problems = Vec::new();
    if got != want {
        problems.push(format!(
            "legacy save read back as {got:02X?}, expected {want:02X?}"
        ));
    }
    if migrated.len() != 0x8000 {
        problems.push(format!(
            "no migrated .sav (got {} bytes, expected 32768)",
            migrated.len()
        ));
    }
    if backup != legacy {
        problems
            .push("the legacy .gbsave was modified; it must be left as a backup".into());
    }
    Case {
        name: "legacy .gbsave loads, migrates to .sav, and is kept as a backup",
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            "loaded, migrated, original preserved".to_string()
        } else {
            problems.join("; ")
        },
    }
}

/// A `.sav` written by another emulator (raw RAM, optionally with the 48-byte
/// VBA-M/mGBA RTC block appended) must load here.
fn case_foreign_sav_loads() -> Case {
    let dir = workdir("foreign");
    let rom = stage(&dir);

    let mut foreign = Vec::new();
    for b in 0u8..4 {
        let mut bank = vec![0u8; 0x2000];
        bank[0] = bank_marker(b);
        bank[0x1FFF] = bank_tail(b);
        foreign.extend_from_slice(&bank);
    }
    foreign.extend_from_slice(&[0u8; 48]); // VBA-M/mGBA trailing RTC block
    std::fs::write(dir.join("battery.sav"), &foreign).expect("write foreign save");

    let mut gb = boot(&rom);
    let got = read_bank_markers(&mut gb);
    std::mem::forget(gb);
    let want = expected_markers();
    let ok = got == want;
    Case {
        name: "a foreign .sav (raw RAM, optional trailing RTC) loads",
        ok,
        detail: if ok {
            "all 4 banks match".to_string()
        } else {
            format!("read {got:02X?}, expected {want:02X?}")
        },
    }
}

/// A plain `.sav` from another emulator or a real cartridge must land at RAM
/// offset 0. We used to write an 8-byte big-endian RTC anchor *in front of* the
/// RAM and consume it unconditionally on load — even for header type `0x13`,
/// MBC3+RAM+BATTERY, which has no clock at all. Every foreign save therefore
/// arrived shifted by 8 bytes (RAM byte 0 read file byte 8) and lost its last 8
/// bytes, and every save we wrote was shifted for everyone else.
fn case_no_byte_shift_on_a_plain_sav() -> Case {
    let dir = workdir("shift");
    let rom = stage(&dir);

    // A recognisable ramp: RAM offset N must read as the file's byte N.
    let plain: Vec<u8> = (0..0x8000usize).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.join("battery.sav"), &plain).expect("write plain save");

    let mut gb = boot(&rom);
    gb.debug_write(0x0000, 0x0A);
    gb.debug_write(0x4000, 0);
    let first = gb.debug_read(0xA000);
    // The very last byte of the last bank: it is what a prefix silently drops.
    gb.debug_write(0x4000, 3);
    let last = gb.debug_read(0xBFFF);
    std::mem::forget(gb);

    let (want_first, want_last) = (plain[0], plain[0x7FFF]);
    let ok = first == want_first && last == want_last;
    Case {
        name: "a plain .sav loads at offset 0, not shifted",
        ok,
        detail: if ok {
            format!("RAM[0]={first:02X}, RAM[0x7FFF]={last:02X} — no header consumed")
        } else {
            format!(
                "RAM[0]={first:02X} (expected {want_first:02X}), \
                 RAM[0x7FFF]={last:02X} (expected {want_last:02X}).\n      \
                 The loader is consuming a header the file does not have, so every \
                 save from another emulator or a real cartridge is corrupted — and \
                 every save we write is corrupted for everyone else. Cartridge type \
                 0x13 has no RTC; nothing may be prepended."
            )
        },
    }
}

/// Malformed or unusual headers and save files must not crash. Under
/// `[profile.release] panic = 'abort'` a panic is an instant process death —
/// which also means the player's save never gets written.
fn case_odd_carts_and_saves_do_not_panic() -> Case {
    let dir = workdir("robust");
    let mut problems: Vec<String> = Vec::new();

    // 1. MBC5+RUMBLE with no cartridge RAM (header type 0x1C). The rumble motor
    //    is bit 3 of the RAM-bank register, so every rumble game writes there;
    //    `bank % rambanks` with zero banks divided by zero.
    {
        let sub = dir.join("rumble");
        std::fs::create_dir_all(&sub).unwrap();
        let mut data = build_battery_rom();
        data[0x0147] = 0x1C; // MBC5+RUMBLE, no RAM
        data[0x0149] = 0x00;
        let rom = sub.join("rumble.gb");
        std::fs::write(&rom, data).unwrap();
        let mut gb = boot(&rom);
        gb.debug_write(0x4000, 0x08); // motor on — the classic rumble poke
        gb.debug_write(0x4000, 0x00);
        let _ = gb.debug_read(0xA000);
        std::mem::forget(gb);
    }

    // 2. An unofficial ROM-size code. 0x52/0x53/0x54 (1.1/1.2/1.5 MiB) are real,
    //    and a bootleg header can hold anything; `rom_banks` returned 0 and the
    //    bank register's `% rombanks` divided by zero.
    {
        let sub = dir.join("romsize");
        std::fs::create_dir_all(&sub).unwrap();
        let mut data = build_battery_rom();
        data[0x0147] = 0x01; // MBC1
        data[0x0148] = 0x54; // unofficial size code
        let rom = sub.join("odd.gb");
        std::fs::write(&rom, data).unwrap();
        let mut gb = boot(&rom);
        gb.debug_write(0x2000, 0x05); // bank switch
        let _ = gb.debug_read(0x4000);
        std::mem::forget(gb);
    }

    // 3. A save file far shorter than the cartridge's RAM — e.g. an MBC2-sized
    //    file handed to an 8 KiB cart. `readram` indexed the loaded buffer
    //    unchecked, so the first RAM read was out of bounds.
    {
        let sub = dir.join("shortsave");
        std::fs::create_dir_all(&sub).unwrap();
        let mut data = build_battery_rom();
        data[0x0147] = 0x03; // MBC1+RAM+BATTERY
        data[0x0149] = 0x02; // 8 KiB
        let rom = sub.join("short.gb");
        std::fs::write(&rom, data).unwrap();
        std::fs::write(sub.join("short.sav"), vec![0xEEu8; 512]).unwrap();
        let mut gb = boot(&rom);
        gb.debug_write(0x0000, 0x0A);
        for a in [0xA000u16, 0xA1FF, 0xBFFF] {
            let _ = gb.debug_read(a);
        }
        std::mem::forget(gb);
    }

    // 4. A ROM truncated below the 32 KiB window (homebrew, partial download).
    {
        let sub = dir.join("truncated");
        std::fs::create_dir_all(&sub).unwrap();
        let mut data = build_battery_rom();
        data[0x0147] = 0x00; // ROM ONLY
        data.truncate(0x1000);
        let rom = sub.join("trunc.gb");
        std::fs::write(&rom, data).unwrap();
        let mut gb = boot(&rom);
        let _ = gb.debug_read(0x7FFF);
        std::mem::forget(gb);
    }

    // 5. ROM+RAM+BATTERY with no mapper (header type 0x09) must load and save.
    {
        let sub = dir.join("nombc-battery");
        std::fs::create_dir_all(&sub).unwrap();
        let mut data = build_battery_rom();
        data[0x0147] = 0x09; // ROM+RAM+BATTERY, no mapper
        data[0x0149] = 0x02; // 8 KiB
        let rom = sub.join("nombc.gb");
        std::fs::write(&rom, data).unwrap();
        let mut gb = boot(&rom);
        for _ in 0..180 {
            gb.frame();
        }
        std::mem::forget(gb);
        // With no mapper there is one 8 KiB RAM chip and the bank register does
        // nothing, so all four of the ROM's passes alias it — the last one wins.
        let (want_head, want_tail) = (bank_marker(3), bank_tail(3));
        match std::fs::read(sub.join("nombc.sav")) {
            Ok(b) if b.len() == 0x2000 && b[0] == want_head && b[0x1FFF] == want_tail => {
            }
            Ok(b) => problems.push(format!(
                "no-mapper battery cart (0x09): saved {} bytes, RAM[0]={:02X?} \
                 (expected 8192 bytes, {want_head:02X})",
                b.len(),
                b.first()
            )),
            Err(e) => problems.push(format!(
                "no-mapper battery cart (0x09) wrote no save file ({e}) — a cart type \
                 with a battery that cannot save at all"
            )),
        }
    }

    Case {
        name: "malformed headers and foreign save files do not crash",
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            "RAM-less MBC5+RUMBLE bank write, unofficial ROM-size code, short .sav, \
             truncated ROM, and no-mapper battery cart all handled"
                .to_string()
        } else {
            problems.join("; ")
        },
    }
}

/// **The save-destroying case.** A game crashes and the runaway CPU writes
/// thousands of bytes into cartridge RAM; the autosave must not carry that to
/// disk. Measured on the real thing first: Pokémon Blue crashing out of a Cable
/// Club link battle changed 4,378 bytes of cartridge RAM within six frames, and
/// the battery persisted them over the player's save.
///
/// Played out in two boots against one save file, because that is the user's
/// actual sequence: play and save, then a later session crashes.
fn case_a_crashed_game_cannot_overwrite_a_save() -> Case {
    const NAME: &str = "a crashed game cannot overwrite a good save";
    let dir = workdir("crash");
    let rom = stage(&dir);
    let sav = dir.join("battery.sav");

    // Session 1: a normal game that saves.
    let mut gb = boot(&rom);
    for _ in 0..180 {
        gb.frame();
    }
    std::mem::forget(gb);
    let good = match std::fs::read(&sav) {
        Ok(b) => b,
        Err(e) => {
            return Case {
                name: NAME,
                ok: false,
                detail: format!("session 1 wrote no save to compare against ({e})"),
            }
        }
    };

    // Session 2: the same cartridge, but this time the game crashes.
    std::fs::write(&rom, build_crash_rom()).expect("write crash rom");
    let mut gb = boot(&rom);
    for _ in 0..180 {
        gb.frame();
    }

    let mut problems = Vec::new();
    if gb.fault().is_none() {
        problems.push(
            "the emulator did not notice the crash at all: a program counter that has \
             walked into the unusable block is executing bytes that are not a program"
                .to_string(),
        );
    }
    // Not vacuous: the runaway routine really did rewrite cartridge RAM. If it
    // did not, this case would pass for the wrong reason.
    let scribbled = read_bank_markers(&mut gb);
    if scribbled[0] == expected_markers()[0] {
        problems.push(format!(
            "the crash never reached cartridge RAM (bank 0 still reads {:02X?}), so \
             this case proves nothing — fix the test cartridge",
            scribbled[0]
        ));
    }
    match std::fs::read(&sav) {
        Ok(now) if now == good => {}
        Ok(now) => {
            let changed = now.iter().zip(good.iter()).filter(|(a, b)| a != b).count();
            problems.push(format!(
                "the save file changed after the crash: {changed} of {} bytes differ.\n      \
                 A save written by a crashed CPU is not a save, it is damage — and it \
                 has replaced the player's real one, which is unrecoverable.",
                good.len()
            ));
        }
        Err(e) => problems.push(format!("the save file disappeared ({e})")),
    }
    std::mem::forget(gb);

    Case {
        name: NAME,
        ok: problems.is_empty(),
        detail: if problems.is_empty() {
            format!(
                "crash detected ({}); cartridge RAM was rewritten but the {}-byte save \
                 file is byte-identical",
                "PC left the program",
                good.len()
            )
        } else {
            problems.join("; ")
        },
    }
}

fn main() {
    let cases = vec![
        case_survives_an_unclean_exit(),
        case_reload_restores_every_bank(),
        case_a_boot_that_never_saves_writes_nothing(),
        case_legacy_gbsave_migrates(),
        case_foreign_sav_loads(),
        case_no_byte_shift_on_a_plain_sav(),
        case_odd_carts_and_saves_do_not_panic(),
        case_a_crashed_game_cannot_overwrite_a_save(),
    ];

    let mut failed = 0;
    println!("\nbattery-backed save durability\n");
    for c in &cases {
        if c.ok {
            println!("  PASS  {}\n        {}", c.name, c.detail);
        } else {
            failed += 1;
            println!("  FAIL  {}\n        {}", c.name, c.detail);
        }
    }
    println!("\n  {}/{} passed\n", cases.len() - failed, cases.len());
    if failed > 0 {
        std::process::exit(1);
    }
}

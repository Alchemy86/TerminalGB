//! The optional user-supplied boot ROM, end to end.
//!
//! ```text
//! cargo test --release --test bootrom
//! ```
//!
//! **No boot ROM is bundled, fetched or required.** The image this harness runs
//! is 256 bytes written here, in this file, by hand — it is ours, it contains
//! nothing from anybody's console, and it exists to prove the *mechanism*:
//! that an image supplied by the user is mapped over the cartridge, executed
//! from `$0000`, able to leave state (including video RAM) that the high-level
//! boot cannot reconstruct, and unmapped for good by its own `$FF50` write so
//! that `PC` falls through to the cartridge at `$0100`.
//!
//! It also pins the whole of the fail-soft contract, which is the part a user
//! actually experiences: no boot ROM, a wrong-sized one, or one belonging to
//! the other console must all give exactly the machine a fresh clone gets.
//!
//! Section 6 covers the OTHER half of the boot-VRAM contract:
//! `MemoryManagementUnit::set_boot_logo_tiles` decompresses the cartridge's
//! own header logo (`$0104`-`$0133`, present in every bootable ROM) under
//! HLE, and must get entirely out of the way the instant a real boot ROM is
//! supplied — that ROM is the one running for real, and duplicating its work
//! would risk clashing with whatever it actually wrote.
//!
//! See [`src/boot.rs`](../src/boot.rs) and `docs/mealybug.md` §8.6.

use terminalgb::boot::{BootRom, BootRomError, CGB_LEN, DMG_LEN};
use terminalgb::gameboy::Gameboy;
use terminalgb::mode::Model;

/// A byte no high-level boot writes anywhere: the marker the synthetic boot ROM
/// leaves in VRAM at `$8190`, which is the address the real one leaves its
/// glyph at (`docs/mealybug.md` §8.6).
const VRAM_MARK: u8 = 0x5A;
/// And a `BGP` the HLE handoff never leaves (`$FC` is what it writes).
const BGP_MARK: u8 = 0x42;

/// 256 bytes of our own, doing what a boot ROM does: touch some state the
/// cartridge cannot have touched yet, then unmap itself.
///
/// The last two bytes are `ldh [$50],a` at `$00FE-$00FF` with `A = 1`, exactly
/// as a real DMG boot ROM ends — the write unmaps the image and `PC` lands on
/// the cartridge's `$0100` on the next fetch.
fn synthetic_dmg_boot() -> Vec<u8> {
    let mut rom = vec![0x00u8; DMG_LEN];
    let code: &[u8] = &[
        0x21, 0x90, 0x81, // ld hl,$8190
        0x3E, VRAM_MARK, // ld a,$5A
        0x77,      // ld [hl],a
        0x3E, BGP_MARK, // ld a,$42
        0xE0, 0x47, // ldh [$47],a      BGP
        0x06, 0x11, // ld b,$11
        0x0E, 0x22, // ld c,$22
        0x3E, 0x01, // ld a,$01
    ];
    rom[..code.len()].copy_from_slice(code);
    // Everything between is `nop`, so execution walks to the last instruction.
    rom[0xFE] = 0xE0; // ldh [$50],a  — the unmap, and the handoff
    rom[0xFF] = 0x50;
    rom
}

/// A 32 KiB ROM-only cartridge that parks at `$0150`. Hand-built here for the
/// same reason `testharness/battery.rs` builds its own: no commercial ROM, no
/// vendored binary.
fn cartridge() -> Vec<u8> {
    let mut rom = vec![0x00u8; 0x8000];
    rom[0x0100] = 0x00; // nop
    rom[0x0101] = 0xC3; // jp $0150
    rom[0x0102] = 0x50;
    rom[0x0103] = 0x01;
    // A recognisable byte at $0000, so "is the overlay live?" is answerable by
    // reading one address.
    rom[0x0000] = 0xDD;
    rom[0x0147] = 0x00; // ROM ONLY
    rom[0x0148] = 0x00; // 32 KiB
    rom[0x0149] = 0x00; // no RAM
                        // $0150: inc a / jr -2, so the machine is alive and going nowhere.
    rom[0x0150] = 0x3C;
    rom[0x0151] = 0x18;
    rom[0x0152] = 0xFD;
    rom
}

fn gb(boot: Option<BootRom>) -> Gameboy {
    Gameboy::new_with_boot_rom(cartridge(), None, Model::Dmg, boot)
}

/// [`cartridge`] with a caller-chosen 48-byte header-logo region — a
/// synthetic, non-Nintendo pattern, since this is a test of the *mechanism*
/// (decompress whatever the header holds), not of any specific bitmap.
fn cartridge_with_header(header: [u8; 48]) -> Vec<u8> {
    let mut rom = cartridge();
    rom[0x0104..0x0134].copy_from_slice(&header);
    rom
}

fn gb_with_header(header: [u8; 48], boot: Option<BootRom>) -> Gameboy {
    Gameboy::new_with_boot_rom(cartridge_with_header(header), None, Model::Dmg, boot)
}

/// Step until `PC` reaches the cartridge's entry point, or give up.
fn run_to_cartridge(gb: &mut Gameboy) -> bool {
    for _ in 0..200_000 {
        if gb.debug_pc() >= 0x0100 {
            return true;
        }
        gb.step_instruction();
    }
    false
}

fn main() {
    let mut failures: Vec<String> = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        println!(
            "  {}  {name}{}",
            if ok { "PASS" } else { "FAIL" },
            if detail.is_empty() {
                String::new()
            } else {
                format!("\n        {detail}")
            }
        );
        if !ok {
            failures.push(name.to_string());
        }
    };

    println!("boot ROM harness — the image is 256 bytes written in this file\n");

    // ---------------------------------------------------------------- 1. HLE
    let mut plain = gb(None);
    let hle_pc = plain.debug_pc();
    let hle_bgp = plain.peek(0xFF47);
    let hle_vram = plain.peek(0x8190);
    let hle_mapped = plain.boot_rom_mapped();
    check(
        "with no boot ROM the machine is exactly the HLE one",
        hle_pc == 0x0100 && hle_bgp == 0xFC && hle_vram == 0x00 && !hle_mapped,
        format!(
            "PC={hle_pc:04X} BGP={hle_bgp:02X} [$8190]={hle_vram:02X} mapped={hle_mapped}"
        ),
    );
    check(
        "and $FF50 reads $FF, as boot_hwio-dmgABCmgb expects",
        plain.peek(0xFF50) == 0xFF,
        format!("${:02X}", plain.peek(0xFF50)),
    );

    // ------------------------------------------------- 2. the image is mapped
    let rom = BootRom::new(synthetic_dmg_boot()).expect("256 bytes is a DMG image");
    let mut booted = gb(Some(rom));
    let start_pc = booted.debug_pc();
    let overlaid = booted.peek(0x0000);
    check(
        "with one, the machine starts at reset with the image over $0000",
        start_pc == 0x0000 && overlaid == 0x21 && booted.boot_rom_mapped(),
        format!("PC={start_pc:04X} [$0000]={overlaid:02X} (cartridge holds $DD there)"),
    );
    check(
        "and $FF50 reads $FE while it is mapped",
        booted.peek(0xFF50) == 0xFE,
        format!("${:02X}", booted.peek(0xFF50)),
    );
    check(
        "the cartridge header is NOT covered by a DMG image",
        booted.peek(0x0147) == 0x00 && booted.peek(0x0101) == 0xC3,
        format!(
            "[$0101]={:02X} [$0147]={:02X}",
            booted.peek(0x0101),
            booted.peek(0x0147)
        ),
    );

    // ------------------------------------------------------- 3. it runs, and
    //                                                    hands over correctly
    let reached = run_to_cartridge(&mut booted);
    let pc = booted.debug_pc();
    let vram = booted.peek(0x8190);
    let bgp = booted.peek(0xFF47);
    check(
        "it executes and hands over to the cartridge at $0100",
        reached && pc == 0x0100 && !booted.boot_rom_mapped(),
        format!("PC={pc:04X} mapped={}", booted.boot_rom_mapped()),
    );
    check(
        "it left VRAM the HLE machine cannot reconstruct",
        vram == VRAM_MARK,
        format!("[$8190]=${vram:02X}, wanted ${VRAM_MARK:02X}"),
    );
    check(
        "it left its own register state, not the transcribed handoff",
        bgp == BGP_MARK,
        format!("BGP=${bgp:02X}, wanted ${BGP_MARK:02X}"),
    );
    check(
        "and the cartridge is back at $0000 once it has unmapped",
        booted.peek(0x0000) == 0xDD && booted.peek(0xFF50) == 0xFF,
        format!(
            "[$0000]={:02X} $FF50={:02X}",
            booted.peek(0x0000),
            booted.peek(0xFF50)
        ),
    );

    // ------------------------------------------------------ 4. fail soft
    check(
        "a wrong-sized image is refused with its length",
        matches!(
            BootRom::new(vec![0; 1024]),
            Err(BootRomError::BadLength(1024))
        ),
        String::new(),
    );
    let cgb_image = BootRom::new(vec![0; CGB_LEN]).unwrap();
    let mut wrong = gb(Some(cgb_image));
    check(
        "a CGB image on a DMG is declined and the machine boots HLE",
        wrong.debug_pc() == 0x0100
            && !wrong.boot_rom_mapped()
            && wrong.peek(0xFF47) == 0xFC,
        format!(
            "PC={:04X} mapped={} BGP={:02X}",
            wrong.debug_pc(),
            wrong.boot_rom_mapped(),
            wrong.peek(0xFF47)
        ),
    );

    // ------------------------------------------- 5. save states know about it
    let mut mid = gb(Some(BootRom::new(synthetic_dmg_boot()).unwrap()));
    mid.step_instruction();
    let state = mid.save_state();
    let mut into_hle = gb(None);
    let refused = into_hle.load_state(&state).is_err();
    check(
        "a state taken inside the boot ROM refuses to load without one",
        refused,
        String::new(),
    );
    let mut into_booted = gb(Some(BootRom::new(synthetic_dmg_boot()).unwrap()));
    let accepted = into_booted.load_state(&state).is_ok();
    check(
        "and loads into a machine that has one",
        accepted && into_booted.boot_rom_mapped(),
        format!("mapped={}", into_booted.boot_rom_mapped()),
    );

    // ------------------------------------------- 6. the header logo, licence-
    //                                              clean and only under HLE
    // Header bytes 0-1 ($0104-$0105) become tile $01 at $8010: nibble $F
    // (rows 0-1) doubles to $FF on the low bitplane, the rest of the tile
    // stays blank. Every input byte is the cartridge's own header — see
    // `MemoryManagementUnit::set_boot_logo_tiles`.
    let mut header = [0u8; 48];
    header[0] = 0xF0;
    let mut hle_logo = gb_with_header(header, None);
    check(
        "with no boot ROM, the header's own logo is decompressed into $8010",
        hle_logo.peek(0x8010) == 0xFF && hle_logo.peek(0x8011) == 0x00,
        format!(
            "[$8010]={:02X} [$8011]={:02X}",
            hle_logo.peek(0x8010),
            hle_logo.peek(0x8011)
        ),
    );
    check(
        "and the ® glyph's own slot is left alone -- nothing derives it",
        hle_logo.peek(0x8190) == 0x00,
        format!("[$8190]={:02X}", hle_logo.peek(0x8190)),
    );

    // With a real boot ROM supplied, that ROM is the one running for real —
    // our derivation must not also run and clash with whatever it wrote (the
    // synthetic image here never touches $8010, so it must read back exactly
    // as VRAM starts: zero).
    let mut booted_logo =
        gb_with_header(header, Some(BootRom::new(synthetic_dmg_boot()).unwrap()));
    run_to_cartridge(&mut booted_logo);
    check(
        "with one supplied, the licence-clean derivation does not also run",
        booted_logo.peek(0x8010) == 0x00,
        format!(
            "[$8010]={:02X}, wanted $00 (untouched)",
            booted_logo.peek(0x8010)
        ),
    );

    println!();
    if failures.is_empty() {
        println!("all boot-ROM checks passed");
    } else {
        eprintln!("FAILED: {}", failures.join(", "));
        std::process::exit(1);
    }
}

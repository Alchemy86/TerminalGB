//! Tiny hand-assembled cartridges, used as fixtures.
//!
//! **These exist so nothing in this repository has to ship a commercial game.**
//! Music mode reads artwork and audio out of ROMs the *user* supplies at run
//! time, which is what every emulator does and is fine; committing any of it
//! here would not be. So the tests, the examples and the CI harness drive these
//! instead — a few dozen bytes of our own machine code each, with a title, a
//! picture and (for one of them) a tone.
//!
//! They are also genuinely useful: the demo shelf built from them proves the
//! whole path end to end on a machine with no ROM library at all.

/// Header checksum over `0x134..=0x14C`, which the boot ROM verifies on real
/// hardware and every tool checks.
fn stamp_header(rom: &mut [u8], title: &str) {
    for (i, b) in title.bytes().take(16).enumerate() {
        rom[0x134 + i] = b;
    }
    rom[0x147] = 0x00; // ROM ONLY
    rom[0x148] = 0x00; // 32 KiB
    rom[0x149] = 0x00; // no cartridge RAM
    let mut chk: u8 = 0;
    for &b in rom.iter().take(0x14D).skip(0x134) {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x14D] = chk;
}

fn base_rom(title: &str) -> Vec<u8> {
    let mut rom = vec![0u8; 0x8000];
    rom[0x100] = 0x00; // nop
    rom[0x101] = 0xC3; // jp $0150
    rom[0x102] = 0x50;
    rom[0x103] = 0x01;
    stamp_header(&mut rom, title);
    rom
}

/// Frequency register value for the square channels: `f = 131072 / (2048 - x)`.
/// 1750 gives 439.84 Hz — near enough concert A to be recognisable by ear and
/// exactly reproducible by arithmetic.
const TONE_FREQ_X: u16 = 1750;

/// Expected pitch of [`tone_rom`], in Hz.
pub const TONE_HZ: f64 = 131072.0 / (2048 - TONE_FREQ_X as u32) as f64;

/// A cartridge that plays a steady 439.84 Hz square wave and draws a picture.
///
/// Both halves matter: it is the fixture for the audio capture *and* for the
/// cover-art picker, so one fixture exercises the whole album.
pub fn tone_rom() -> Vec<u8> {
    let mut rom = base_rom("GB TONE 440");
    let lo = (TONE_FREQ_X & 0xFF) as u8;
    let hi = 0x80 | ((TONE_FREQ_X >> 8) & 0x7) as u8; // trigger, length off
    let mut prog: Vec<u8> = vec![
        0x3E, 0x80, 0xE0, 0x26, // LD A,$80 ; LDH (NR52),A   power on
        0x3E, 0xFF, 0xE0, 0x25, // LD A,$FF ; LDH (NR51),A   both sides
        0x3E, 0x77, 0xE0, 0x24, // LD A,$77 ; LDH (NR50),A   max volume
        0x3E, 0x80, 0xE0, 0x11, // LD A,$80 ; LDH (NR11),A   duty 50%
        0x3E, 0xF0, 0xE0, 0x12, // LD A,$F0 ; LDH (NR12),A   volume 15
        0x3E, lo, 0xE0, 0x13, //   LD A,lo  ; LDH (NR13),A
        0x3E, hi, 0xE0, 0x14, //   LD A,hi  ; LDH (NR14),A   trigger
    ];
    prog.extend_from_slice(&picture_program());
    write_program(&mut rom, &prog);
    rom
}

/// A cartridge that draws the same picture and programs no sound channel at
/// all.
///
/// Its whole job is to make "silent" a *finding* rather than an ambiguity: a
/// machine with no [`crate::sound::AudioPlayer`] installed has no APU, and both
/// states look like an empty buffer from the outside.
pub fn silent_rom() -> Vec<u8> {
    let mut rom = base_rom("GB SILENCE");
    let mut prog: Vec<u8> = vec![
        0x3E, 0x80, 0xE0, 0x26, // power the APU on, then program nothing
    ];
    prog.extend_from_slice(&picture_program());
    write_program(&mut rom, &prog);
    rom
}

/// A cartridge that declares battery-backed RAM **and writes to it**.
///
/// The fixture for the one guarantee music mode cannot get wrong: it opens no
/// save file. A cartridge that merely *declares* RAM proves nothing — the
/// battery layer deliberately writes nothing for a session that never touched
/// cartridge RAM (`src/mbc/battery.rs`), so a test built on one passes whether
/// or not the machine was given a path. This one fills the first page of
/// cartridge RAM, so a machine with a save path really does produce a file.
pub fn battery_rom() -> Vec<u8> {
    let mut rom = base_rom("GB BATTERY");
    rom[0x147] = 0x13; // MBC3+RAM+BATTERY
    rom[0x149] = 0x02; // 8 KiB of cartridge RAM
    let mut prog: Vec<u8> = vec![
        0x3E, 0x0A, 0xEA, 0x00, 0x00, // LD A,$0A ; LD [$0000],A  enable cart RAM
        0x21, 0x00, 0xA0, // LD HL,$A000
        0x06, 0x00, //       LD B,0        (256 iterations)
        0x3E, 0x5A, //       LD A,$5A
        0x22, //             LD [HL+],A
        0x05, //             DEC B
        0x20, 0xFC, //       JR NZ,-4
    ];
    prog.extend_from_slice(&picture_program());
    write_program(&mut rom, &prog);
    // The header checksum covers 0x134..=0x14C, so it has to be recomputed
    // after the cartridge-type and RAM-size bytes were changed.
    let mut chk: u8 = 0;
    for &b in rom.iter().take(0x14D).skip(0x134) {
        chk = chk.wrapping_sub(b).wrapping_sub(1);
    }
    rom[0x14D] = chk;
    rom
}

/// A cartridge whose screen stays blank — the LCD is never enabled.
///
/// The cover picker must report "no picture" for this rather than offering a
/// blank rectangle as artwork.
pub fn blank_rom() -> Vec<u8> {
    let mut rom = base_rom("GB BLANK");
    write_program(&mut rom, &[0x18, 0xFE]); // JR -2, forever
    rom
}

/// Machine code that puts a recognisable pattern on the screen and spins.
///
/// Two tiles (one solid dark, one solid light) written into VRAM, a checkerboard
/// of them in the background map, then `LCDC = $91` to switch the LCD on with
/// the background enabled. It looks like a chequerboard, which is enough shade
/// variety for the cover scorer to treat it as a real picture.
fn picture_program() -> Vec<u8> {
    let mut p: Vec<u8> = Vec::new();
    // BGP = $E4 (the identity palette: 3,2,1,0).
    p.extend_from_slice(&[0x3E, 0xE4, 0xE0, 0x47]);

    // Tile 1 at $8010: every byte $FF in both bit planes -> colour 3 (darkest).
    // HL = $8010; B = 16; A = $FF; loop { [HL+] = A; B-- }
    p.extend_from_slice(&[0x21, 0x10, 0x80]); // LD HL,$8010
    p.extend_from_slice(&[0x06, 0x10]); // LD B,16
    p.extend_from_slice(&[0x3E, 0xFF]); // LD A,$FF
    p.extend_from_slice(&[0x22]); // LD [HL+],A
    p.extend_from_slice(&[0x05]); // DEC B
    p.extend_from_slice(&[0x20, 0xFC]); // JR NZ,-4

    // Background map $9800..$9C00: alternate tile 0 and tile 1.
    // HL = $9800; DE = $0400 (count); C = 0; loop { [HL+] = C; C ^= 1; DE-- }
    p.extend_from_slice(&[0x21, 0x00, 0x98]); // LD HL,$9800
    p.extend_from_slice(&[0x11, 0x00, 0x04]); // LD DE,$0400
    p.extend_from_slice(&[0x0E, 0x00]); // LD C,0
    p.extend_from_slice(&[0x79]); // LD A,C
    p.extend_from_slice(&[0x22]); // LD [HL+],A
    p.extend_from_slice(&[0xEE, 0x01]); // XOR $01
    p.extend_from_slice(&[0x4F]); // LD C,A
    p.extend_from_slice(&[0x1B]); // DEC DE
    p.extend_from_slice(&[0x7A]); // LD A,D
    p.extend_from_slice(&[0xB3]); // OR E
    p.extend_from_slice(&[0x20, 0xF6]); // JR NZ,-10

    // LCDC = $91: LCD on, BG on, tile data at $8000.
    p.extend_from_slice(&[0x3E, 0x91, 0xE0, 0x40]);
    p.extend_from_slice(&[0x18, 0xFE]); // JR -2, forever
    p
}

fn write_program(rom: &mut [u8], prog: &[u8]) {
    rom[0x150..0x150 + prog.len()].copy_from_slice(prog);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::romlib;
    use std::path::Path;

    #[test]
    fn the_fixtures_are_valid_cartridges_with_readable_titles() {
        for (rom, title) in [
            (tone_rom(), "GB TONE 440"),
            (silent_rom(), "GB SILENCE"),
            (blank_rom(), "GB BLANK"),
        ] {
            let e = romlib::entry_from_bytes(Path::new("/x/fixture.gb"), &rom);
            assert_eq!(e.title, title);
            assert_eq!(e.mbc, "ROM ONLY");
            // Header checksum, exactly as hardware computes it.
            let mut chk: u8 = 0;
            for &b in rom.iter().take(0x14D).skip(0x134) {
                chk = chk.wrapping_sub(b).wrapping_sub(1);
            }
            assert_eq!(chk, rom[0x14D], "{title}: header checksum");
        }
    }

    #[test]
    fn the_expected_tone_is_close_to_concert_a() {
        assert!((TONE_HZ - 440.0).abs() < 1.0, "{TONE_HZ}");
    }
}

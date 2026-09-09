use std::path;

use crate::mbc::battery::Battery;
use crate::mbc::{ram_banks, rom_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

pub struct MBC1 {
    /// Shared with [`MemoryManagementUnit`](crate::mmu::MemoryManagementUnit)
    /// so the CPU's opcode fetch need not go through this trait object — one
    /// allocation, two owners, no copy.
    rom: std::sync::Arc<[u8]>,
    ram: Vec<u8>,
    ram_on: bool,
    banking_mode: u8,
    rombank: usize,
    rambank: usize,
    battery: Battery,
    rombanks: usize,
    rambanks: usize,
    /// MBC1M ("multicart") wiring — see [`is_multicart`].
    multicart: bool,
}

/// The Nintendo logo, as it appears at `$0104-$0133` of every cartridge header.
/// Used here purely as a *signature*: an MBC1M compilation cart carries a second
/// cartridge header at the start of bank `$10`, which is how such a cart is
/// identified. Pan Docs, "MBC1M": *"These carts can normally be identified by
/// having a Nintendo copyright header in bank $10."*
/// <https://gbdev.io/pandocs/MBC1.html>
const NINTENDO_LOGO: [u8; 48] = [
    0xCE, 0xED, 0x66, 0x66, 0xCC, 0x0D, 0x00, 0x0B, 0x03, 0x73, 0x00, 0x83, 0x00, 0x0C,
    0x00, 0x0D, 0x00, 0x08, 0x11, 0x1F, 0x88, 0x89, 0x00, 0x0E, 0xDC, 0xCC, 0x6E, 0xE6,
    0xDD, 0xDD, 0xD9, 0x99, 0xBB, 0xBB, 0x67, 0x63, 0x6E, 0x0E, 0xEC, 0xCC, 0xDD, 0xDC,
    0x99, 0x9F, 0xBB, 0xB9, 0x33, 0x3E,
];

/// Environment override for the multicart heuristic: `1`/`true`/`on` forces
/// MBC1M wiring, `0`/`false`/`off` forces the ordinary wiring. Anything else is
/// ignored. Mooneye's own `multicart_rom_8Mb` readme notes that MBC1M "cannot be
/// detected from the header alone", so an emulator needs "either heuristics or
/// some configuration" — this is the configuration half.
pub const MULTICART_ENV: &str = "GB_MBC1_MULTICART";

/// Is this cartridge image an MBC1M multi-game compilation cart?
///
/// MBC1M carts are always exactly 1 MiB and declare an ordinary MBC1 in the
/// header, so the only signal is the second cartridge header the compilation
/// menu leaves at the start of bank `$10` (Pan Docs, "MBC1M"). Requiring the
/// image to be exactly 1 MiB *and* to carry the logo at `$40104` keeps this off
/// every ordinary cartridge: a normal game has the logo once, in bank `$00`.
///
/// Getting this wrong in the false-positive direction breaks a normal 1 MiB
/// cart badly (bank bits 5-6 would move to 4-5, making half the ROM
/// unreachable), so [`MULTICART_ENV`] can force the answer either way.
fn is_multicart(data: &[u8]) -> bool {
    if let Some(forced) = std::env::var(MULTICART_ENV).ok().and_then(|v| {
        match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "on" | "yes" => Some(true),
            "0" | "false" | "off" | "no" => Some(false),
            _ => None,
        }
    }) {
        return forced;
    }
    const BANK_10_LOGO: usize = 0x10 * 0x4000 + 0x104;
    data.len() == 0x10_0000
        && data
            .get(BANK_10_LOGO..BANK_10_LOGO + NINTENDO_LOGO.len())
            .map(|l| l == NINTENDO_LOGO)
            .unwrap_or(false)
}

impl MBC1 {
    /// `file` is `None` when the embedder asked for no filesystem access; the
    /// cartridge then behaves identically but its RAM lives only in memory.
    pub fn new(
        data: impl Into<std::sync::Arc<[u8]>>,
        file: Option<path::PathBuf>,
    ) -> StrResult<MBC1> {
        // Free when the caller already shares one; the single copy a
        // `Vec` caller has always paid, when it does not.
        let data: std::sync::Arc<[u8]> = data.into();
        let rambanks = match data[0x147] {
            0x02 | 0x03 => ram_banks(data[0x149]),
            _ => 0,
        };
        let battery = match (data[0x147], &file) {
            (0x03, Some(f)) => Battery::new(f, rambanks * 0x2000, false),
            _ => Battery::none(),
        };
        MBC1::build(data, rambanks, battery)
    }

    fn build(
        data: std::sync::Arc<[u8]>,
        rambanks: usize,
        battery: Battery,
    ) -> StrResult<MBC1> {
        let ramsize = rambanks * 0x2000;
        let multicart = is_multicart(&data);
        // An MBC1M is 1 MiB by construction, so trust that over `0x148` — a
        // header that under-declares the size would otherwise clip BANK2 away
        // in the `% rombanks` below and strand three quarters of the games.
        let rombanks = match multicart {
            true => rom_banks(data[0x148], data.len()).max(0x40),
            false => rom_banks(data[0x148], data.len()),
        };

        let mut res = MBC1 {
            rom: data,
            ram: vec![0u8; ramsize],
            ram_on: false,
            banking_mode: 0,
            rombank: 1,
            rambank: 0,
            battery,
            rombanks,
            rambanks,
            multicart,
        };
        res.loadram().map(|_| res)
    }

    /// Which bit of the effective bank number the 2-bit BANK2 register drives:
    /// bits 5-6 for an ordinary MBC1, bits 4-5 for an MBC1M, whose BANK1 loses
    /// its top bit (Pan Docs, "MBC1M").
    fn bank2_shift(&self) -> usize {
        if self.multicart {
            4
        } else {
            5
        }
    }

    /// How much of BANK1 reaches the ROM: 5 bits normally, 4 on an MBC1M.
    fn bank1_mask(&self) -> usize {
        if self.multicart {
            0x0F
        } else {
            0x1F
        }
    }

    /// The BANK2 bits inside the effective bank number (`$60` / `$30`).
    fn bank2_mask(&self) -> usize {
        0x03 << self.bank2_shift()
    }

    fn loadram(&mut self) -> StrResult<()> {
        if !self.battery.is_active() {
            return Ok(());
        }
        let loaded = self.battery.load()?;
        self.ram = loaded.ram;
        if !loaded.fresh {
            // Migrates a legacy `.gbsave` to `.sav` on the next flush.
            self.battery.mark_dirty();
        }
        Ok(())
    }
}

impl Drop for MBC1 {
    fn drop(&mut self) {
        // Belt and braces — `autosave` is the real mechanism (see `mbc::battery`).
        self.flush_battery();
    }
}

impl MemoryBankController for MBC1 {
    fn readrom(&self, a: u16) -> u8 {
        let bank = if a < 0x4000 {
            if self.banking_mode == 0 {
                0
            } else {
                // Mode 1 shows BANK2 alone here: banks $00/$20/$40/$60, or
                // $00/$10/$20/$30 on an MBC1M — which is how a compilation
                // cart switches games.
                self.rombank & self.bank2_mask()
            }
        } else {
            self.rombank
        };
        let idx = (bank * 0x4000) | ((a as usize) & 0x3FFF);
        *self.rom.get(idx).unwrap_or(&0xFF)
    }
    fn rom_share(&self) -> Option<std::sync::Arc<[u8]>> {
        Some(self.rom.clone())
    }
    fn rom_window_bases(&self) -> (usize, usize) {
        let lo = if self.banking_mode == 0 {
            0
        } else {
            self.rombank & self.bank2_mask()
        };
        (lo * 0x4000, self.rombank * 0x4000)
    }
    fn readrom_bank(&self, bank: usize, a: u16) -> u8 {
        super::banked_rom_byte(&self.rom, bank, a)
    }
    fn readram_bank(&self, bank: usize, a: u16) -> Option<u8> {
        super::banked_ram_byte(&self.ram, bank, a)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on {
            return 0xFF;
        }
        let rambank = if self.banking_mode == 1 {
            self.rambank
        } else {
            0
        };
        // A cart can declare RAM+BATTERY with a RAM size of 0, and a foreign
        // `.sav` can be shorter than the declared size; open-bus rather than a
        // panic (which `panic = 'abort'` would turn into an instant crash).
        *self
            .ram
            .get((rambank * 0x2000) | ((a & 0x1FFF) as usize))
            .unwrap_or(&0xFF)
    }

    // MBC1 bank registers. See docs/components/mbc.md and Pan Docs:
    // https://gbdev.io/pandocs/MBC1.html
    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => {
                // RAM enable: only the low nibble == 0xA counts.
                self.ram_on = v & 0xF == 0xA;
            }
            0x2000..=0x3FFF => {
                // BANK1. Bank 0 is unreachable through this register — a
                // written 0 reads as 1 (the classic MBC1 quirk). The 00->01
                // translation uses all five written bits *before* an MBC1M
                // drops bit 4, so writing $10 to a multicart selects bank $00
                // and writing $00 selects bank $01.
                let lower_bits = match (v as usize) & 0x1F {
                    0 => 1,
                    n => n,
                } & self.bank1_mask();
                self.rombank =
                    ((self.rombank & self.bank2_mask()) | lower_bits) % self.rombanks;
            }
            0x4000..=0x5FFF => {
                // 2-bit BANK2: high ROM-bank bits once the cartridge is bigger
                // than BANK1 alone can address, else the RAM bank select.
                let shift = self.bank2_shift();
                if self.rombanks > (1 << shift) {
                    let upper_bits = (v as usize & 0x03) % (self.rombanks >> shift);
                    self.rombank =
                        self.rombank & self.bank1_mask() | (upper_bits << shift)
                }
                if self.rambanks > 1 {
                    self.rambank = (v as usize) & 0x03;
                }
            }
            0x6000..=0x7FFF => {
                // Mode 0: upper bits apply to ROM only. Mode 1: they also bank
                // the 0000-3FFF window and cart RAM (see readrom/readram).
                self.banking_mode = v & 0x01;
            }
            _ => panic!("Could not write to {:04X} (MBC1)", a),
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on {
            return;
        }
        let rambank = if self.banking_mode == 1 {
            self.rambank
        } else {
            0
        };
        let address = (rambank * 0x2000) | ((a & 0x1FFF) as usize);
        if address < self.ram.len() {
            self.ram[address] = v;
            self.battery.touch();
        }
    }

    fn rom_bank(&self) -> usize {
        self.rombank
    }

    fn autosave(&mut self) {
        self.battery.tick(&self.ram, None);
    }

    fn flush_battery(&mut self) {
        self.battery.flush(&self.ram, None);
    }

    fn battery_dirty(&mut self) {
        self.battery.mark_dirty();
    }

    fn poison_battery(&mut self, reason: &str) {
        self.battery.poison(reason);
    }

    fn battery_poisoned(&self) -> Option<&str> {
        self.battery.poisoned_reason()
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.ram_on);
        w.u8(self.banking_mode);
        w.usize(self.rombank);
        w.usize(self.rambank);
        w.blob(&self.ram);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.ram_on = r.bool()?;
        self.banking_mode = r.u8()?;
        self.rombank = r.usize()?;
        self.rambank = r.usize()?;
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc1 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbc::MemoryBankController;

    /// Every expectation below is transcribed verbatim from Mooneye GB's
    /// `emulator-only/mbc1/multicart_rom_8Mb.s` "expected banks" table (MIT,
    /// Joonas Javanainen), which the readme records as verified against a real
    /// MBC1B1 chip. Index is `BANK2 * 32 + BANK1`.
    const EXPECTED_LOW_MODE1: [u8; 128] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
        16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, //
        16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, 16, //
        32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, //
        32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, //
        48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, //
        48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48,
    ];
    const EXPECTED_HIGH: [u8; 128] = [
        1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, //
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, //
        17, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, //
        16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, //
        33, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, //
        32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, //
        49, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, //
        48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
    ];

    /// A 1 MiB MBC1 image whose every bank stamps its own number at its first
    /// byte, optionally carrying the second cartridge header in bank `$10` that
    /// identifies a compilation cart.
    fn image(with_bank10_header: bool) -> Vec<u8> {
        let mut rom = vec![0u8; 0x10_0000];
        rom[0x147] = 0x01; // MBC1
        rom[0x148] = 0x05; // 64 banks
        for bank in 0..64usize {
            rom[bank * 0x4000] = bank as u8;
        }
        if with_bank10_header {
            let at = 0x10 * 0x4000 + 0x104;
            rom[at..at + NINTENDO_LOGO.len()].copy_from_slice(&NINTENDO_LOGO);
        }
        rom
    }

    fn select(mbc: &mut MBC1, mode: u8, bank2: usize, bank1: usize) {
        mbc.writerom(0x6000, mode);
        mbc.writerom(0x4000, bank2 as u8);
        mbc.writerom(0x2000, bank1 as u8);
    }

    #[test]
    fn multicart_banking_matches_the_mooneye_table() {
        let mut mbc = MBC1::new(image(true), None).unwrap();
        assert!(mbc.multicart, "bank $10 header should select MBC1M wiring");

        for bank2 in 0..4usize {
            for bank1 in 0..32usize {
                let i = bank2 * 32 + bank1;

                select(&mut mbc, 0, bank2, bank1);
                assert_eq!(
                    mbc.readrom(0x0000),
                    0,
                    "mode 0 $0000-$3FFF, bank2={bank2} bank1={bank1}"
                );
                assert_eq!(
                    mbc.readrom(0x4000),
                    EXPECTED_HIGH[i],
                    "mode 0 $4000-$7FFF, bank2={bank2} bank1={bank1}"
                );

                select(&mut mbc, 1, bank2, bank1);
                assert_eq!(
                    mbc.readrom(0x0000),
                    EXPECTED_LOW_MODE1[i],
                    "mode 1 $0000-$3FFF, bank2={bank2} bank1={bank1}"
                );
                assert_eq!(
                    mbc.readrom(0x4000),
                    EXPECTED_HIGH[i],
                    "mode 1 $4000-$7FFF, bank2={bank2} bank1={bank1}"
                );
            }
        }
    }

    /// The same image without the bank-$10 header is an ordinary 1 MiB MBC1:
    /// BANK2 drives bits 5-6, so it reaches banks $00/$20/$40/$60 and BANK1
    /// keeps all five of its bits. Guards the heuristic against the direction
    /// that would break real games.
    #[test]
    fn a_plain_1mb_cart_keeps_the_ordinary_wiring() {
        let mut mbc = MBC1::new(image(false), None).unwrap();
        assert!(!mbc.multicart);

        // BANK1 = $10 is bank $10, not bank $00.
        select(&mut mbc, 0, 0, 0x10);
        assert_eq!(mbc.readrom(0x4000), 0x10);
        // BANK2 = 1 adds $20, and the 00->01 translation ignores it.
        select(&mut mbc, 0, 1, 0x00);
        assert_eq!(mbc.readrom(0x4000), 0x21);
        select(&mut mbc, 0, 1, 0x05);
        assert_eq!(mbc.readrom(0x4000), 0x25);
        // Only mode 1 can reach bank $20 at all, through $0000-$3FFF.
        assert_eq!(mbc.readrom(0x0000), 0x00);
        select(&mut mbc, 1, 1, 0x05);
        assert_eq!(mbc.readrom(0x0000), 0x20);
        // A 1 MiB cart has 64 banks, so only bit 0 of BANK2 exists.
        select(&mut mbc, 0, 3, 0x01);
        assert_eq!(mbc.readrom(0x4000), 0x21);
    }

    /// A cartridge that is not exactly 1 MiB is never a multicart, however its
    /// bank $10 happens to read.
    #[test]
    fn multicart_detection_requires_a_1mb_image() {
        let mut rom = image(true);
        rom.extend(std::iter::repeat(0u8).take(0x4000));
        let mbc = MBC1::new(rom, None).unwrap();
        assert!(!mbc.multicart);
    }
}

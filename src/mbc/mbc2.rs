use std::path;

use crate::mbc::battery::Battery;
use crate::mbc::{rom_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

/// MBC2 has 512 x 4 bits of RAM built into the chip; the cartridge header
/// declares no RAM size. Pan Docs: https://gbdev.io/pandocs/MBC2.html
const MBC2_RAM_SIZE: usize = 512;

pub struct MBC2 {
    /// Shared with [`MemoryManagementUnit`](crate::mmu::MemoryManagementUnit)
    /// so the CPU's opcode fetch need not go through this trait object — one
    /// allocation, two owners, no copy.
    rom: std::sync::Arc<[u8]>,
    ram: Vec<u8>,
    ram_on: bool,
    rombank: usize,
    battery: Battery,
    rombanks: usize,
}

impl MBC2 {
    /// `file` is `None` when the embedder asked for no filesystem access; the
    /// cartridge then behaves identically but its RAM lives only in memory.
    pub fn new(
        data: impl Into<std::sync::Arc<[u8]>>,
        file: Option<path::PathBuf>,
    ) -> StrResult<MBC2> {
        // Free when the caller already shares one; the single copy a
        // `Vec` caller has always paid, when it does not.
        let data: std::sync::Arc<[u8]> = data.into();
        let battery = match (data[0x147], &file) {
            (0x06, Some(f)) => Battery::new(f, MBC2_RAM_SIZE, false),
            _ => Battery::none(),
        };
        let rombanks = rom_banks(data[0x148], data.len());

        let mut res = MBC2 {
            rom: data,
            ram: vec![0; MBC2_RAM_SIZE],
            ram_on: false,
            rombank: 1,
            battery,
            rombanks,
        };
        res.loadram().map(|_| res)
    }

    fn loadram(&mut self) -> StrResult<()> {
        if !self.battery.is_active() {
            return Ok(());
        }
        let loaded = self.battery.load()?;
        self.ram = loaded.ram;
        if !loaded.fresh {
            self.battery.mark_dirty();
        }
        Ok(())
    }
}

impl Drop for MBC2 {
    fn drop(&mut self) {
        self.flush_battery();
    }
}

impl MemoryBankController for MBC2 {
    fn readrom(&self, a: u16) -> u8 {
        let bank = if a < 0x4000 { 0 } else { self.rombank };
        let idx = (bank * 0x4000) | ((a as usize) & 0x3FFF);
        *self.rom.get(idx).unwrap_or(&0xFF)
    }
    fn rom_share(&self) -> Option<std::sync::Arc<[u8]>> {
        Some(self.rom.clone())
    }
    fn rom_window_bases(&self) -> (usize, usize) {
        (0, self.rombank * 0x4000)
    }
    fn readrom_bank(&self, bank: usize, a: u16) -> u8 {
        super::banked_rom_byte(&self.rom, bank, a)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on {
            return 0xFF;
        }
        self.ram.get((a as usize) & 0x1FF).copied().unwrap_or(0) | 0xF0
    }

    fn writerom(&mut self, a: u16, v: u8) {
        if let 0x0000..=0x3FFF = a {
            if a & 0x100 == 0 {
                self.ram_on = v & 0xF == 0xA;
            } else {
                self.rombank = match (v as usize) & 0x0F {
                    0 => 1,
                    n => n,
                } % self.rombanks;
            }
        }
    }

    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on {
            return;
        }
        // Store the low nibble only. The chip holds 4 bits per cell; `readram`
        // ORs the high nibble back to 1. Storing `v | 0xF0` instead behaves
        // identically to a game but makes our `.sav` differ byte-for-byte from a
        // hardware dump of the same cartridge (FlashGBX masks with 0x0F), which
        // breaks any comparison against a real cartridge.
        if let Some(cell) = self.ram.get_mut((a as usize) & 0x1FF) {
            *cell = v & 0x0F;
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
        w.usize(self.rombank);
        w.blob(&self.ram);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.ram_on = r.bool()?;
        self.rombank = r.usize()?;
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc2 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

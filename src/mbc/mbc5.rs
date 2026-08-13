use crate::mbc::battery::Battery;
use crate::mbc::{ram_banks, rom_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

use std::path;

pub struct MBC5 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    ram_on: bool,
    battery: Battery,
    rombanks: usize,
    rambanks: usize,
}

impl MBC5 {
    pub fn new(data: Vec<u8>, file: path::PathBuf) -> StrResult<MBC5> {
        let subtype = data[0x147];
        let rambanks = match subtype {
            0x1A | 0x1B | 0x1D | 0x1E => ram_banks(data[0x149]),
            _ => 0,
        };
        let ramsize = 0x2000 * rambanks;
        let battery = match subtype {
            0x1B | 0x1E => Battery::new(&file, ramsize, false),
            _ => Battery::none(),
        };
        let rombanks = rom_banks(data[0x148], data.len());

        let mut res = MBC5 {
            rom: data,
            ram: vec![0u8; ramsize],
            rombank: 1,
            rambank: 0,
            ram_on: false,
            battery,
            rombanks,
            rambanks,
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

impl Drop for MBC5 {
    fn drop(&mut self) {
        self.flush_battery();
    }
}

impl MemoryBankController for MBC5 {
    fn readrom(&self, a: u16) -> u8 {
        let idx = if a < 0x4000 {
            a as usize
        } else {
            (self.rombank * 0x4000) | ((a as usize) & 0x3FFF)
        };
        *self.rom.get(idx).unwrap_or(&0)
    }
    fn readrom_bank(&self, bank: usize, a: u16) -> u8 {
        super::banked_rom_byte(&self.rom, bank, a)
    }
    fn readram(&self, a: u16) -> u8 {
        // Disabled or absent cartridge RAM reads as open bus (0xFF) on hardware,
        // which is what every other controller in this tree returns.
        if !self.ram_on {
            return 0xFF;
        }
        let idx = (self.rambank * 0x2000) | ((a as usize) & 0x1FFF);
        self.ram.get(idx).copied().unwrap_or(0)
    }
    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_on = v & 0x0F == 0x0A,
            0x2000..=0x2FFF => {
                self.rombank = ((self.rombank & 0x100) | (v as usize)) % self.rombanks
            }
            0x3000..=0x3FFF => {
                self.rombank =
                    ((self.rombank & 0x0FF) | (((v & 0x1) as usize) << 8)) % self.rombanks
            }
            // RAM bank select — and on MBC5+RUMBLE (types 0x1C-0x1E) bit 3 of
            // this same register drives the rumble motor, so *every* rumble game
            // writes here. Types 0x19 and 0x1C have no cartridge RAM at all, and
            // `% 0` is a divide-by-zero panic (an abort under `panic = 'abort'`).
            // With no RAM there is no bank to select. Pan Docs:
            // https://gbdev.io/pandocs/MBC5.html
            0x4000..=0x5FFF => {
                if self.rambanks > 0 {
                    self.rambank = ((v & 0x0F) as usize) % self.rambanks;
                }
            }
            0x6000..=0x7FFF => { /* ? */ }
            _ => panic!("Could not write to {:04X} (MBC5)", a),
        }
    }
    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on {
            return;
        }
        let idx = (self.rambank * 0x2000) | ((a as usize) & 0x1FFF);
        if let Some(cell) = self.ram.get_mut(idx) {
            *cell = v;
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

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.bool(self.ram_on);
        w.usize(self.rombank);
        w.usize(self.rambank);
        w.blob(&self.ram);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.ram_on = r.bool()?;
        self.rombank = r.usize()?;
        self.rambank = r.usize()?;
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc5 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

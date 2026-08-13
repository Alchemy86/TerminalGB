use std::path;

use crate::mbc::battery::Battery;
use crate::mbc::{ram_banks, rom_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

pub struct MBC1 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    ram_on: bool,
    banking_mode: u8,
    rombank: usize,
    rambank: usize,
    battery: Battery,
    rombanks: usize,
    rambanks: usize,
}

impl MBC1 {
    pub fn new(data: Vec<u8>, file: path::PathBuf) -> StrResult<MBC1> {
        let rambanks = match data[0x147] {
            0x02 | 0x03 => ram_banks(data[0x149]),
            _ => 0,
        };
        let battery = match data[0x147] {
            0x03 => Battery::new(&file, rambanks * 0x2000),
            _ => Battery::none(),
        };
        MBC1::build(data, rambanks, battery)
    }

    #[allow(dead_code)]
    pub fn new_without_save(data: Vec<u8>) -> StrResult<MBC1> {
        let rambanks = match data[0x147] {
            0x02 | 0x03 => ram_banks(data[0x149]),
            _ => 0,
        };
        MBC1::build(data, rambanks, Battery::none())
    }

    fn build(data: Vec<u8>, rambanks: usize, battery: Battery) -> StrResult<MBC1> {
        let rombanks = rom_banks(data[0x148]);
        let ramsize = rambanks * 0x2000;

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
                self.rombank & 0xE0
            }
        } else {
            self.rombank
        };
        let idx = (bank * 0x4000) | ((a as usize) & 0x3FFF);
        *self.rom.get(idx).unwrap_or(&0xFF)
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
                // ROM bank low 5 bits. Bank 0 is unreachable through this
                // register — a written 0 reads as 1 (the classic MBC1 quirk).
                let lower_bits = match (v as usize) & 0x1F {
                    0 => 1,
                    n => n,
                };
                self.rombank = ((self.rombank & 0x60) | lower_bits) % self.rombanks;
            }
            0x4000..=0x5FFF => {
                // 2-bit "upper" register: ROM bank bits 5-6 on >512 KiB carts
                // (rombanks > 0x20), else the RAM bank select.
                if self.rombanks > 0x20 {
                    let upper_bits = (v as usize & 0x03) % (self.rombanks >> 5);
                    self.rombank = self.rombank & 0x1F | (upper_bits << 5)
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

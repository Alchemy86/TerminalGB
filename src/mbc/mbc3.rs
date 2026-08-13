use crate::mbc::battery::{now_secs, Battery, RtcState};
use crate::mbc::{ram_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

use std::path;
use std::time;

pub struct MBC3 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    rambanks: usize,
    selectrtc: bool,
    ram_on: bool,
    battery: Battery,
    rtc_ram: [u8; 5],
    rtc_ram_latch: [u8; 5],
    rtc_zero: Option<u64>,
}

impl MBC3 {
    pub fn new(data: Vec<u8>, file: path::PathBuf) -> StrResult<MBC3> {
        let subtype = data[0x147];
        let rambanks = match subtype {
            0x10 | 0x12 | 0x13 => ram_banks(data[0x149]),
            _ => 0,
        };
        let ramsize = rambanks * 0x2000;
        // 0x0F TIMER+BATTERY (clock, no RAM), 0x10 TIMER+RAM+BATTERY,
        // 0x13 RAM+BATTERY (**no clock** — it must never get an RTC footer).
        let battery = match subtype {
            0x0F | 0x10 => Battery::new(&file, ramsize, true),
            0x13 => Battery::new(&file, ramsize, false),
            _ => Battery::none(),
        };
        MBC3::build(data, subtype, rambanks, ramsize, battery)
    }

    pub fn new_without_save(data: Vec<u8>) -> StrResult<MBC3> {
        let subtype = data[0x147];
        let rambanks = match subtype {
            0x10 | 0x12 | 0x13 => ram_banks(data[0x149]),
            _ => 0,
        };
        let ramsize = rambanks * 0x2000;
        MBC3::build(data, subtype, rambanks, ramsize, Battery::none())
    }

    fn build(
        data: Vec<u8>,
        subtype: u8,
        rambanks: usize,
        ramsize: usize,
        battery: Battery,
    ) -> StrResult<MBC3> {
        let rtc = match subtype {
            0x0F | 0x10 => Some(0),
            _ => None,
        };
        let mut res = MBC3 {
            rom: data,
            ram: vec![0u8; ramsize],
            rombank: 1,
            rambank: 0,
            rambanks,
            selectrtc: false,
            ram_on: false,
            battery,
            rtc_ram: [0u8; 5],
            rtc_ram_latch: [0u8; 5],
            rtc_zero: rtc,
        };
        res.loadram().map(|_| res)
    }

    fn loadram(&mut self) -> StrResult<()> {
        if !self.battery.is_active() {
            return Ok(());
        }
        let loaded = self.battery.load()?;
        self.ram = loaded.ram;
        if self.rtc_zero.is_some() {
            if let Some(state) = loaded.rtc {
                // A cartridge's clock keeps ticking while the console is off, so
                // anchor it at the moment the sidecar was written and let the
                // elapsed wall time carry it forward.
                self.rtc_ram = state.regs;
                self.rtc_ram_latch = state.latched;
                self.rtc_zero =
                    Some(state.timestamp.saturating_sub(rtc_regs_secs(&state.regs)));
            } else if let Some(anchor) = loaded.legacy_rtc_zero {
                self.rtc_zero = Some(anchor);
            }
        }
        if !loaded.fresh {
            // Content came from disk: if it came from a legacy `.gbsave`, this
            // is what writes the migrated `.sav` out (the old file is kept as a
            // backup and never touched again).
            self.battery.mark_dirty();
        }
        Ok(())
    }

    /// The RTC in the neutral shape the sidecar stores, brought up to date.
    fn rtc_state(&mut self) -> Option<RtcState> {
        self.rtc_zero?;
        self.calc_rtc_reg();
        Some(RtcState {
            regs: self.rtc_ram,
            latched: self.rtc_ram_latch,
            timestamp: now_secs(),
        })
    }

    fn latch_rtc_reg(&mut self) {
        self.calc_rtc_reg();
        self.rtc_ram_latch.clone_from_slice(&self.rtc_ram);
    }

    fn calc_rtc_reg(&mut self) {
        // Do not modify regs when halted
        if self.rtc_ram[4] & 0x40 == 0x40 {
            return;
        }

        let tzero = match self.rtc_zero {
            Some(t) => time::UNIX_EPOCH + time::Duration::from_secs(t),
            None => return,
        };

        if self.compute_difftime() == self.rtc_zero {
            // No time has passed. Do not alter registers
            return;
        }

        let difftime = match time::SystemTime::now().duration_since(tzero) {
            Ok(n) => n.as_secs(),
            _ => 0,
        };
        self.rtc_ram[0] = (difftime % 60) as u8;
        self.rtc_ram[1] = ((difftime / 60) % 60) as u8;
        self.rtc_ram[2] = ((difftime / 3600) % 24) as u8;
        let days = difftime / (3600 * 24);
        self.rtc_ram[3] = days as u8;
        self.rtc_ram[4] = (self.rtc_ram[4] & 0xFE) | (((days >> 8) & 0x01) as u8);
        if days >= 512 {
            self.rtc_ram[4] |= 0x80;
            self.calc_rtc_zero();
        }
    }

    fn compute_difftime(&self) -> Option<u64> {
        self.rtc_zero?;
        let mut difftime = match time::SystemTime::now().duration_since(time::UNIX_EPOCH)
        {
            Ok(t) => t.as_secs(),
            Err(_) => {
                panic!("System clock is set to a time before the unix epoch (1970-01-01)")
            }
        };
        difftime -= self.rtc_ram[0] as u64;
        difftime -= (self.rtc_ram[1] as u64) * 60;
        difftime -= (self.rtc_ram[2] as u64) * 3600;
        let days = ((self.rtc_ram[4] as u64 & 0x1) << 8) | (self.rtc_ram[3] as u64);
        difftime -= days * 3600 * 24;
        Some(difftime)
    }

    fn calc_rtc_zero(&mut self) {
        self.rtc_zero = self.compute_difftime();
    }
}

/// Seconds represented by the five RTC registers (s, m, h, day-lo, day-hi).
fn rtc_regs_secs(regs: &[u8; 5]) -> u64 {
    let days = ((regs[4] as u64 & 0x1) << 8) | regs[3] as u64;
    regs[0] as u64 + regs[1] as u64 * 60 + regs[2] as u64 * 3600 + days * 3600 * 24
}

impl Drop for MBC3 {
    fn drop(&mut self) {
        // A last chance, not the mechanism: `autosave` has almost always
        // written this already, and the frontends cannot guarantee `Drop` runs
        // at all (see `mbc::battery`).
        self.flush_battery();
    }
}

impl MemoryBankController for MBC3 {
    fn readrom(&self, a: u16) -> u8 {
        let idx = if a < 0x4000 {
            a as usize
        } else {
            (self.rombank * 0x4000) | ((a as usize) & 0x3FFF)
        };
        *self.rom.get(idx).unwrap_or(&0xFF)
    }
    fn readrom_bank(&self, bank: usize, a: u16) -> u8 {
        super::banked_rom_byte(&self.rom, bank, a)
    }
    fn readram(&self, a: u16) -> u8 {
        if !self.ram_on {
            return 0xFF;
        }
        if !self.selectrtc && self.rambank < self.rambanks {
            let idx = (self.rambank * 0x2000) | ((a as usize) & 0x1FFF);
            *self.ram.get(idx).unwrap_or(&0xFF)
        } else if self.selectrtc && self.rambank < 5 {
            self.rtc_ram_latch[self.rambank]
        } else {
            0xFF
        }
    }
    fn writerom(&mut self, a: u16, v: u8) {
        match a {
            0x0000..=0x1FFF => self.ram_on = (v & 0x0F) == 0x0A,
            0x2000..=0x3FFF => {
                self.rombank = match v & 0x7F {
                    0 => 1,
                    n => n as usize,
                }
            }
            0x4000..=0x5FFF => {
                self.selectrtc = v & 0x8 == 0x8;
                self.rambank = (v & 0x7) as usize;
            }
            0x6000..=0x7FFF => self.latch_rtc_reg(),
            _ => panic!("Could not write to {:04X} (MBC3)", a),
        }
    }
    fn writeram(&mut self, a: u16, v: u8) {
        if !self.ram_on {
            return;
        }
        if !self.selectrtc && self.rambank < self.rambanks {
            let idx = (self.rambank * 0x2000) | ((a as usize) & 0x1FFF);
            if idx < self.ram.len() {
                self.ram[idx] = v;
                // The battery would have held this the instant it was written.
                self.battery.touch();
            }
        } else if self.selectrtc && self.rambank < 5 {
            self.calc_rtc_reg();
            let vmask = match self.rambank {
                0 | 1 => 0x3F,
                2 => 0x1F,
                4 => 0xC1,
                _ => 0xFF,
            };
            self.rtc_ram[self.rambank] = v & vmask;
            self.calc_rtc_zero();
            // Setting the clock is battery-backed too: a game that sets the
            // date (Pokémon Gold's day/night) must find it again next launch.
            self.battery.touch();
        }
    }

    fn rom_bank(&self) -> usize {
        self.rombank
    }

    fn autosave(&mut self) {
        // `rtc_state` reads the host clock and re-derives the RTC registers;
        // only worth doing on frames where a write is actually pending.
        if !self.battery.is_active() || !self.battery.is_dirty() {
            return;
        }
        // Bring the clock up to date first: `tick` may write it out.
        let rtc = self.rtc_state();
        self.battery.tick(&self.ram, rtc);
    }

    fn flush_battery(&mut self) {
        if !self.battery.is_active() {
            return;
        }
        let rtc = self.rtc_state();
        self.battery.flush(&self.ram, rtc);
    }

    fn battery_dirty(&mut self) {
        self.battery.mark_dirty();
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.usize(self.rombank);
        w.usize(self.rambank);
        w.bool(self.selectrtc);
        w.bool(self.ram_on);
        w.bytes(&self.rtc_ram);
        w.bytes(&self.rtc_ram_latch);
        // rtc_zero is the UNIX-epoch anchor for the real-time clock; capturing
        // it (and the latch) is what keeps the RTC coherent across a load.
        w.opt_u64(self.rtc_zero);
        w.blob(&self.ram);
    }

    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.rombank = r.usize()?;
        self.rambank = r.usize()?;
        self.selectrtc = r.bool()?;
        self.ram_on = r.bool()?;
        r.fill(&mut self.rtc_ram)?;
        r.fill(&mut self.rtc_ram_latch)?;
        self.rtc_zero = r.opt_u64()?;
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc3 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

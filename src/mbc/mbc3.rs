use crate::mbc::battery::{Battery, RtcState};
use crate::mbc::rtc::Rtc;
use crate::mbc::{ram_banks, MemoryBankController};
pub type StrResult<T> = Result<T, &'static str>;

use std::path;

pub struct MBC3 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    rombank: usize,
    rambank: usize,
    rambanks: usize,
    selectrtc: bool,
    ram_on: bool,
    battery: Battery,
    /// The cartridge clock, for header types `0x0F`/`0x10` only. See
    /// [`crate::mbc::rtc`].
    rtc: Option<Rtc>,
}

impl MBC3 {
    /// `file` is `None` when the embedder asked for no filesystem access; the
    /// cartridge then behaves identically but its RAM (and RTC) live only in
    /// memory.
    pub fn new(data: Vec<u8>, file: Option<path::PathBuf>) -> StrResult<MBC3> {
        let subtype = data[0x147];
        let rambanks = match subtype {
            0x10 | 0x12 | 0x13 => ram_banks(data[0x149]),
            _ => 0,
        };
        let ramsize = rambanks * 0x2000;
        // 0x0F TIMER+BATTERY (clock, no RAM), 0x10 TIMER+RAM+BATTERY,
        // 0x13 RAM+BATTERY (**no clock** — it must never get an RTC footer).
        let battery = match (subtype, &file) {
            (0x0F | 0x10, Some(f)) => Battery::new(f, ramsize, true),
            (0x13, Some(f)) => Battery::new(f, ramsize, false),
            _ => Battery::none(),
        };
        MBC3::build(data, subtype, rambanks, ramsize, battery)
    }

    fn build(
        data: Vec<u8>,
        subtype: u8,
        rambanks: usize,
        ramsize: usize,
        battery: Battery,
    ) -> StrResult<MBC3> {
        let rtc = match subtype {
            0x0F | 0x10 => Some(Rtc::new()),
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
            rtc,
        };
        res.loadram().map(|_| res)
    }

    fn loadram(&mut self) -> StrResult<()> {
        if !self.battery.is_active() {
            return Ok(());
        }
        let loaded = self.battery.load()?;
        self.ram = loaded.ram;
        if let Some(rtc) = self.rtc.as_mut() {
            // A cartridge's clock keeps ticking while the console is off: the
            // footer's timestamp is the anchor and the elapsed wall time
            // carries the counters forward.
            if let Some(state) = loaded.rtc {
                rtc.restore(state);
            } else if let Some(anchor) = loaded.legacy_rtc_zero {
                rtc.restore_from_epoch(anchor);
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

    /// The RTC in the neutral shape the sidecar stores.
    fn rtc_state(&self) -> Option<RtcState> {
        self.rtc.as_ref().map(|r| r.snapshot())
    }
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
        } else if self.selectrtc {
            match self.rtc {
                Some(ref rtc) if self.rambank < 5 => rtc.read(self.rambank),
                _ => 0xFF,
            }
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
            0x6000..=0x7FFF => {
                if let Some(ref mut rtc) = self.rtc {
                    rtc.write_latch(v);
                }
            }
            // Unreachable through the MMU, which only routes $0000-$7FFF here;
            // ignoring it is what hardware does with an address the mapper does
            // not decode, and a `panic!` on a guest-supplied address is a
            // process abort under `panic = 'abort'`.
            _ => {}
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
            let Some(ref mut rtc) = self.rtc else { return };
            rtc.write(self.rambank, v);
            // Setting the clock is battery-backed too: a game that sets the
            // date (Pokémon Gold's day/night) must find it again next launch.
            self.battery.touch();
        }
    }

    fn rom_bank(&self) -> usize {
        self.rombank
    }

    fn has_rtc(&self) -> bool {
        self.rtc.is_some()
    }

    fn rtc_tick(&mut self, cycles: u32) {
        if let Some(ref mut rtc) = self.rtc {
            rtc.tick(cycles);
        }
    }

    fn autosave(&mut self) {
        // `rtc_state` stamps the host clock; only worth doing on frames where a
        // write is actually pending.
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

    fn poison_battery(&mut self, reason: &str) {
        self.battery.poison(reason);
    }

    fn battery_poisoned(&self) -> Option<&str> {
        self.battery.poisoned_reason()
    }

    fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.usize(self.rombank);
        w.usize(self.rambank);
        w.bool(self.selectrtc);
        w.bool(self.ram_on);
        // The clock: counters, latch snapshot, sub-second divider and latch
        // sequence. All of it is live machine state — dropping the divider
        // would move the next tick by up to a second on every load.
        w.bool(self.rtc.is_some());
        if let Some(ref rtc) = self.rtc {
            rtc.save_state(w);
        }
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
        if r.bool()? {
            let mut rtc = Rtc::new();
            rtc.load_state(r)?;
            // A state from an RTC cartridge only makes sense in one; the header
            // check upstream already refuses a different game.
            if self.rtc.is_some() {
                self.rtc = Some(rtc);
            }
        }
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc3 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

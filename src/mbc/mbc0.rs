pub type StrResult<T> = Result<T, &'static str>;
use crate::mbc::battery::Battery;
use crate::mbc::{ram_banks, MemoryBankController};
use std::path;

/// "No MBC" cartridges: header types `0x00` (ROM ONLY), `0x08` (ROM+RAM) and
/// `0x09` (ROM+RAM+BATTERY). No banking — the ROM is mapped straight through
/// and the optional 8 KiB of cartridge RAM is always visible, with no enable
/// register. Pan Docs: <https://gbdev.io/pandocs/nombc.html>
pub struct MBC0 {
    rom: Vec<u8>,
    ram: Vec<u8>,
    battery: Battery,
}

impl MBC0 {
    pub fn new(data: Vec<u8>, file: path::PathBuf) -> StrResult<MBC0> {
        let rambanks = match data[0x147] {
            0x08 | 0x09 => ram_banks(data[0x149]),
            _ => 0,
        };
        let battery = match data[0x147] {
            0x09 => Battery::new(&file, rambanks * 0x2000, false),
            _ => Battery::none(),
        };
        let mut res = MBC0 {
            rom: data,
            ram: vec![0u8; rambanks * 0x2000],
            battery,
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

impl Drop for MBC0 {
    fn drop(&mut self) {
        self.flush_battery();
    }
}

impl MemoryBankController for MBC0 {
    fn readrom(&self, a: u16) -> u8 {
        // A homebrew or truncated ROM can be shorter than the 32 KiB window;
        // unmapped cartridge space reads as open bus, it does not panic.
        *self.rom.get(a as usize).unwrap_or(&0xFF)
    }
    fn readrom_bank(&self, bank: usize, a: u16) -> u8 {
        super::banked_rom_byte(&self.rom, bank, a)
    }
    fn readram(&self, a: u16) -> u8 {
        *self.ram.get((a as usize) & 0x1FFF).unwrap_or(&0xFF)
    }
    fn writerom(&mut self, _a: u16, _v: u8) {}
    fn writeram(&mut self, a: u16, v: u8) {
        if let Some(cell) = self.ram.get_mut((a as usize) & 0x1FFF) {
            *cell = v;
            self.battery.touch();
        }
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

    // No banking: only the optional cartridge RAM is state, and it is written
    // only when the cartridge actually has some. Both sides derive that from
    // the ROM header, which is fixed for a given cart, so a plain ROM-ONLY
    // (0x00) state stays **byte-identical to the pre-0x08/0x09 format** — no
    // `savestate::FORMAT_VERSION` bump, and existing quick-saves keep loading.
    fn save_state(&self, w: &mut crate::savestate::Writer) {
        if !self.ram.is_empty() {
            w.blob(&self.ram);
        }
    }
    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        if self.ram.is_empty() {
            return Ok(());
        }
        let ram = r.blob()?;
        if ram.len() != self.ram.len() {
            return Err(crate::savestate::StateError::BadField("mbc0 ram size"));
        }
        self.ram = ram;
        Ok(())
    }
}

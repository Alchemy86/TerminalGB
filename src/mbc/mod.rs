pub type StrResult<T> = Result<T, &'static str>;

use std::path;

pub mod battery;
mod mbc0;
mod mbc1;
mod mbc2;
mod mbc3;
mod mbc5;

/// Index a cartridge image by bank number, for
/// [`MemoryBankController::readrom_bank`]. Shared by every controller because
/// the mapping from (bank, address) to a file offset is a property of the
/// cartridge layout, not of the mapper.
pub(crate) fn banked_rom_byte(rom: &[u8], bank: usize, a: u16) -> u8 {
    let idx = bank
        .wrapping_mul(0x4000)
        .wrapping_add((a as usize) & 0x3FFF);
    *rom.get(idx).unwrap_or(&0xFF)
}

pub trait MemoryBankController: Send {
    fn readrom(&self, a: u16) -> u8;
    fn readram(&self, a: u16) -> u8;
    fn writerom(&mut self, a: u16, v: u8);
    fn writeram(&mut self, a: u16, v: u8);

    /// Serialise the *mutable* MBC state (banks, RAM-enable, cartridge RAM, and
    /// the MBC3 RTC). The ROM is never written — it is referenced by identity in
    /// the state header and provided by whoever loads the state.
    fn save_state(&self, w: &mut crate::savestate::Writer);

    /// Restore MBC state written by [`MemoryBankController::save_state`]. The
    /// loader already holds the matching ROM, so only the mutable state is
    /// restored in place.
    fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError>;

    /// The currently-selected ROM bank, for debug/stats display. Defaults to 0
    /// for controllers without banking (MBC0).
    fn rom_bank(&self) -> usize {
        0
    }

    /// Read from an explicitly numbered 16 KiB ROM bank, ignoring whatever the
    /// game currently has mapped and **without touching the bank registers**.
    ///
    /// This exists for the plugin surface: cartridge data tables live in banks
    /// the game maps only while it is using them, so an observer polling once a
    /// second cannot reach them through [`MemoryBankController::readrom`]. See
    /// [`crate::plugin::Machine::peek_bank`]. `a` is taken modulo the bank
    /// window, so both `$0000` and `$4000` addressing work; reads past the end
    /// of the cartridge are open bus (`0xFF`), as in `readrom`.
    fn readrom_bank(&self, bank: usize, a: u16) -> u8;

    /// Per-frame battery hook: writes cartridge RAM out shortly after the game
    /// stops touching it. This — not `Drop` — is what makes an in-game save
    /// survive a crash, a `kill`, or a closed window. See [`battery`].
    /// Cartridges without a battery do nothing.
    fn autosave(&mut self) {}

    /// Write battery-backed RAM out immediately (clean shutdown, ROM switch).
    fn flush_battery(&mut self) {}

    /// Note that cartridge RAM was replaced wholesale (a save-state load), so
    /// the battery file needs rewriting even though the game wrote nothing.
    fn battery_dirty(&mut self) {}

    /// **Stop writing the save file, permanently, for this session.**
    ///
    /// Called when something has established that the emulated program is no
    /// longer executing its own code — see
    /// [`MemoryManagementUnit::report_fault`](crate::mmu::MemoryManagementUnit::report_fault).
    /// A crashed CPU stores thousands of arbitrary bytes into cartridge RAM
    /// within a few frames, and the autosave would then write them over a real
    /// save. Refusing every later write keeps the last good file on disk.
    /// Cartridges without a battery do nothing.
    fn poison_battery(&mut self, _reason: &str) {}

    /// Why the save file is no longer being written, if it is not.
    fn battery_poisoned(&self) -> Option<&str> {
        None
    }

    /// The cartridge's 16-bit global checksum (header bytes 0x14E-0x14F). Used
    /// together with the title to refuse loading a state against another game.
    fn global_checksum(&self) -> u16 {
        ((self.readrom(0x014E) as u16) << 8) | (self.readrom(0x014F) as u16)
    }

    fn romname(&self) -> String {
        const TITLE_START: u16 = 0x134;
        const CGB_FLAG: u16 = 0x143;

        let title_size = match self.readrom(CGB_FLAG) & 0x80 {
            0x80 => 11,
            _ => 16,
        };

        let mut result = String::with_capacity(title_size as usize);

        for i in 0..title_size {
            match self.readrom(TITLE_START + i) {
                0 => break,
                v => result.push(v as char),
            }
        }

        result
    }
}

pub fn get_mbc(
    data: Vec<u8>,
    filepath: Option<path::PathBuf>,
) -> StrResult<Box<dyn MemoryBankController + 'static>> {
    if filepath.is_none() {
        return mbc3::MBC3::new_without_save(data)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>);
    }

    let file = filepath.unwrap();

    match data[0x147] {
        // 0x00 ROM ONLY, 0x08 ROM+RAM, 0x09 ROM+RAM+BATTERY. All three are the
        // same "no mapper" cartridge; 0x09 is battery-backed, so rejecting it
        // outright meant a save-capable cart could not save at all.
        0x00 | 0x08 | 0x09 => mbc0::MBC0::new(data, file)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x01..=0x03 => mbc1::MBC1::new(data, file)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x05..=0x06 => mbc2::MBC2::new(data, file)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x0F..=0x13 => mbc3::MBC3::new(data, file)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x19..=0x1E => mbc5::MBC5::new(data, file)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        _ => Err("Unsupported MBC type"),
    }
}

fn ram_banks(v: u8) -> usize {
    match v {
        1 =>
        // "Listed in various unofficial docs as 2 KiB. However, a 2 KiB RAM chip was never
        // used in a cartridge. The source of this value is unknown."
        // Needed by some test roms. As we only deal in whole banks, just make it 1 8KiB bank.
        {
            1
        }
        2 => 1,
        3 => 4,
        4 => 16,
        5 => 8,
        _ => 0,
    }
}

/// Number of 16 KiB ROM banks, from header byte `0x148` with the ROM itself as
/// the fallback.
///
/// The header byte is not trustworthy: the unofficial codes `0x52`/`0x53`/`0x54`
/// (1.1/1.2/1.5 MiB) appear in real cartridges, and bootleg or corrupt headers
/// can hold anything. The old version returned **0** for any `v > 8`, and every
/// MBC's bank register then does `% rombanks` — an immediate divide-by-zero
/// panic (an abort, under `panic = 'abort'`) on the game's first bank switch.
/// Real dumping tools never size a cartridge from this byte either; FlashGBX
/// uses it for display only and measures the hardware. We always hold the whole
/// ROM in memory, so its length is the better fallback. Never below 2 — a
/// one-bank cartridge does not exist and would still divide by zero at `% 1`
/// giving bank 0. Pan Docs: <https://gbdev.io/pandocs/The_Cartridge_Header.html>
fn rom_banks(v: u8, rom_len: usize) -> usize {
    let declared = if v <= 8 { 2usize << v } else { 0 };
    if declared > 0 {
        declared
    } else {
        (rom_len / 0x4000).max(2)
    }
}

#[allow(dead_code)]
fn check_checksum(data: &[u8]) -> StrResult<()> {
    let mut value: u8 = 0;
    for item in data.iter().take(0x14D).skip(0x134) {
        value = value.wrapping_sub(*item).wrapping_sub(1);
    }
    match data[0x14D] == value {
        true => Ok(()),
        false => Err("Cartridge checksum is invalid"),
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn checksum_zero() {
        let mut data = vec![0; 0x150];
        data[0x14D] = -(0x14D_i32 - 0x134_i32) as u8;
        super::check_checksum(&data).unwrap();
    }

    #[test]
    fn checksum_ones() {
        let mut data = vec![1; 0x150];
        data[0x14D] = (-(0x14D_i32 - 0x134_i32) * 2) as u8;
        super::check_checksum(&data).unwrap();
    }
}

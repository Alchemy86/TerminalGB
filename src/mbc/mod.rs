pub type StrResult<T> = Result<T, &'static str>;

use std::path;

pub mod battery;
mod mbc0;
mod mbc1;
mod mbc2;
mod mbc3;
mod mbc5;
pub mod rtc;

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

/// Index cartridge RAM by 8 KiB bank number, for
/// [`MemoryBankController::readram_bank`]. `None` past the end of the RAM the
/// cartridge actually has — "this cartridge cannot answer", not open bus,
/// because the callers (save readers) must distinguish absent storage from a
/// stored `$FF`.
pub(crate) fn banked_ram_byte(ram: &[u8], bank: usize, a: u16) -> Option<u8> {
    let idx = bank
        .wrapping_mul(0x2000)
        .wrapping_add((a as usize) & 0x1FFF);
    ram.get(idx).copied()
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

    /// The cartridge ROM, shared rather than copied.
    ///
    /// The CPU fetches every opcode through `$0000-$7FFF`, so that read was an
    /// indirect call through this trait object several thousand times a frame —
    /// 10 % of a headless collector's samples. The MMU keeps its own handle to
    /// the same allocation and the offsets below, so a fetch is two loads and
    /// an index. See
    /// [`MemoryManagementUnit::read_rom`](crate::mmu::MemoryManagementUnit).
    ///
    /// `None` from a mapper that cannot express its window as a byte offset
    /// into one slice, which simply keeps the old path.
    fn rom_share(&self) -> Option<std::sync::Arc<[u8]>> {
        None
    }

    /// Byte offsets into [`Self::rom_share`] of the two 16 KiB windows the CPU
    /// currently sees at `$0000` and `$4000`. Re-read after every write into
    /// `$0000-$7FFF`, which is the only thing that can move them.
    fn rom_window_bases(&self) -> (usize, usize) {
        (0, 0)
    }

    /// Read from an explicitly numbered 8 KiB cartridge-RAM bank, ignoring the
    /// mapped bank and the RAM-enable gate, and **without touching any
    /// register**. The read-only sibling of [`Self::readram`], for the same
    /// reason [`Self::readrom_bank`] exists: save data (Gen 1's PC boxes, for
    /// instance) lives in banks the game maps only while using them, and a
    /// game normally leaves cartridge RAM *disabled*, so an observer cannot
    /// reach it through the bus at all.
    ///
    /// `None` when the bank is beyond the RAM this cartridge has (or the
    /// mapper does not support banked RAM reads) — callers must be able to
    /// tell "no storage there" from a stored `$FF`.
    fn readram_bank(&self, _bank: usize, _a: u16) -> Option<u8> {
        None
    }

    /// True for a cartridge that carries a real-time clock (MBC3 header types
    /// `0x0F`/`0x10`). The MMU caches this at boot so [`Self::rtc_tick`] is
    /// only reached for the cartridges that need it.
    fn has_rtc(&self) -> bool {
        false
    }

    /// Advance the cartridge's own clock by `cycles` T-cycles of **real** time
    /// (so the caller must divide by the CGB speed multiplier: the crystal on
    /// the cartridge does not know the console changed gear).
    ///
    /// Called once per instruction rather than per memory access — the clock's
    /// finest observable granularity is a whole second, and ax6's `rtc3test`
    /// measures it to 1 ms. See [`rtc`].
    fn rtc_tick(&mut self, _cycles: u32) {}

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

/// Smallest cartridge image that can carry a header: the header runs to
/// `0x014F`, and `0x0147`/`0x0148`/`0x0149` (type / ROM size / RAM size) are
/// what the mapper is chosen from.
const HEADER_END: usize = 0x0150;

/// Build the mapper the cartridge header asks for.
///
/// `filepath` is **only** about persistence: `Some(p)` battery-backs cartridge
/// RAM to `<p>.sav`, `None` keeps it in memory and the emulator touches no
/// file. It must never change *which* mapper is chosen — it used to, and every
/// embedder that asked for no filesystem (the wasm build, the FFI build, and
/// anyone calling `Gameboy::new(rom, None)`) got an **MBC3 regardless of the
/// header**, so a non-MBC3 game was silently mis-banked from its first bank
/// switch. Pan Docs, cartridge header byte `0x0147`:
/// <https://gbdev.io/pandocs/The_Cartridge_Header.html>
pub fn get_mbc(
    data: Vec<u8>,
    filepath: Option<path::PathBuf>,
) -> StrResult<Box<dyn MemoryBankController + 'static>> {
    // An embedder hands us arbitrary bytes. Indexing a short image would panic
    // — an abort under `panic = 'abort'` — so refuse it as an error instead.
    if data.len() < HEADER_END {
        return Err("cartridge image is too short to contain a header");
    }

    match data[0x147] {
        // 0x00 ROM ONLY, 0x08 ROM+RAM, 0x09 ROM+RAM+BATTERY. All three are the
        // same "no mapper" cartridge; 0x09 is battery-backed, so rejecting it
        // outright meant a save-capable cart could not save at all.
        0x00 | 0x08 | 0x09 => mbc0::MBC0::new(data, filepath)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x01..=0x03 => mbc1::MBC1::new(data, filepath)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x05..=0x06 => mbc2::MBC2::new(data, filepath)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x0F..=0x13 => mbc3::MBC3::new(data, filepath)
            .map(|v| Box::new(v) as Box<dyn MemoryBankController>),
        0x19..=0x1E => mbc5::MBC5::new(data, filepath)
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
/// the fallback — and as a **floor** underneath a declared value that would
/// otherwise truncate real, addressable content.
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
///
/// **The declared value can also under-report — `.max()` against the file's
/// own bank count, never the reverse.** A real cartridge's addressable range
/// is a hardware fact (however many address lines the mapper has wired to the
/// ROM chip), not a byte the game's own header self-reports; `mbc1.rs`'s
/// MBC1M path already makes exactly this correction (`.max(0x40)`, "a header
/// that under-declares the size would otherwise clip BANK2 away"). Found on
/// GBMicrotest's `mbc1_rom_banks.gb`: a genuine 2 MiB (128-bank) build whose
/// `$148` byte reads `$00` (32 KiB / 2 banks) — a defect in that ROM's own
/// assembler output, not a licensing/type ambiguity like MBC30's — which
/// silently clipped bank switches above bank 1 to a `% 2` wraparound and
/// failed the ROM's own check. Cross-checked against SameBoy: it derives its
/// addressable range from the file's length exclusively (rounded to a power
/// of two, `gb.c`'s `rounded_rom_size`) and never consults `$148` for masking
/// at all, and it passes this ROM. This fix stays narrower than that — it
/// only ever *raises* the floor a possibly-wrong header sets, never
/// discards the header outright — so a correctly-declared cartridge (every
/// real dump this project has been measured against) is unaffected: `declared
/// == rom_len / 0x4000` for all of them, and `max` is a no-op.
fn rom_banks(v: u8, rom_len: usize) -> usize {
    let declared = if v <= 8 { 2usize << v } else { 0 };
    let from_size = rom_len / 0x4000;
    declared.max(from_size).max(2)
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
    use super::*;

    /// A cartridge image with `banks` 16 KiB banks, each byte in bank *n* set
    /// to *n*, so `readrom(0x4000)` names the mapped bank directly.
    fn cart(kind: u8, banks: usize) -> Vec<u8> {
        let mut rom = Vec::with_capacity(banks * 0x4000);
        for b in 0..banks {
            rom.extend(std::iter::repeat(b as u8).take(0x4000));
        }
        rom[0x147] = kind;
        // rom_banks(5, _) == 64. rambanks 0 — persistence is not what is
        // under test here, mapper *selection* is.
        rom[0x148] = 5;
        rom[0x149] = 0;
        rom
    }

    /// The mapper must come from the cartridge header, never from whether the
    /// caller wanted battery saves.
    ///
    /// This is the embedding case: `Gameboy::new(rom, None)` — a UI, a
    /// hardware build, the wasm and FFI builds — all of which used to be handed
    /// an **MBC3 for every cartridge**. Symptom: a non-MBC3 game reads the
    /// wrong ROM bank from its first bank switch (garbage code, immediate
    /// crash or freeze). No test ROM in the suite catches this — Blargg and
    /// Mooneye are all run *with* a path, and none of them is an MBC5 or
    /// ROM-only cartridge exercising a bank register.
    #[test]
    fn the_mapper_is_chosen_from_the_header_with_or_without_a_save_path() {
        // ROM ONLY: writes to the bank register do nothing at all. Under the
        // old MBC3 fallback this read back bank 2.
        let mut m = get_mbc(cart(0x00, 64), None).unwrap();
        m.writerom(0x2000, 2);
        assert_eq!(m.readrom(0x4000), 1, "ROM-only cart must not bank switch");

        // MBC1 masks the bank register to 5 bits, so 0x22 selects bank 2.
        // MBC3 uses 7 bits and would select bank 0x22.
        let mut m = get_mbc(cart(0x01, 64), None).unwrap();
        m.writerom(0x2000, 0x22);
        assert_eq!(
            m.readrom(0x4000),
            2,
            "MBC1 masks the bank register to 5 bits"
        );

        // MBC5 can map bank 0 at 0x4000; MBC3 rewrites a written 0 to 1.
        let mut m = get_mbc(cart(0x19, 64), None).unwrap();
        m.writerom(0x2000, 0);
        assert_eq!(m.readrom(0x4000), 0, "MBC5 can select bank 0");

        // ...and an actual MBC3 still behaves like one.
        let mut m = get_mbc(cart(0x11, 64), None).unwrap();
        m.writerom(0x2000, 0x22);
        assert_eq!(m.readrom(0x4000), 0x22, "MBC3 uses 7 bank bits");
    }

    /// An embedder hands the core arbitrary bytes. Too short to hold a header
    /// must be an error, not an index panic — which under
    /// `[profile.release] panic = 'abort'` is an instant process abort.
    #[test]
    fn a_cartridge_too_short_for_a_header_is_refused_not_a_panic() {
        assert!(get_mbc(vec![0u8; 0x100], None).is_err());
        assert!(get_mbc(Vec::new(), None).is_err());
        assert!(get_mbc(vec![0u8; 0x150], None).is_ok());
    }

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

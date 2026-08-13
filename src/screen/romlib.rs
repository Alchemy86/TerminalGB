//! Reading Game Boy cartridge headers and building the ROM picker's list.
//!
//! Everything here is pure and file-system-only (no emulator, no terminal), so
//! it is straightforward to unit-test: header parsing runs on byte slices and
//! the filter runs on in-memory [`RomEntry`] lists.

use std::path::{Path, PathBuf};

/// One selectable ROM in the picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RomEntry {
    pub path: PathBuf,
    /// The cartridge's internal title (header bytes `0x134..`), trimmed. Falls
    /// back to the file stem when the header title is blank or unreadable.
    pub title: String,
    /// Human-readable cartridge type, e.g. "MBC1+RAM+BATTERY".
    pub mbc: &'static str,
    /// ROM size on disk, in bytes.
    pub size: u64,
}

impl RomEntry {
    /// The file name (with extension) for display when the title is generic.
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// The cartridge title from a raw ROM image. Header layout: title starts at
/// `0x134`; on CGB carts the title field is 11 bytes (a CGB flag occupies
/// `0x143`), otherwise up to 16. Non-printable bytes terminate the string.
pub fn read_title(rom: &[u8]) -> Option<String> {
    const TITLE_START: usize = 0x134;
    if rom.len() <= 0x143 {
        return None;
    }
    let cgb = rom[0x143] & 0x80 != 0;
    let max = if cgb { 11 } else { 16 };
    let mut title = String::new();
    for i in 0..max {
        let b = rom[TITLE_START + i];
        if b == 0 {
            break;
        }
        // Titles are ASCII; stop at anything non-printable rather than emit
        // control characters into the TUI.
        if !(0x20..=0x7e).contains(&b) {
            break;
        }
        title.push(b as char);
    }
    let trimmed = title.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Human-readable name for the cartridge-type byte at header `0x147`.
pub fn cartridge_type(byte: u8) -> &'static str {
    match byte {
        0x00 => "ROM ONLY",
        0x01 => "MBC1",
        0x02 => "MBC1+RAM",
        0x03 => "MBC1+RAM+BATTERY",
        0x05 => "MBC2",
        0x06 => "MBC2+BATTERY",
        0x08 => "ROM+RAM",
        0x09 => "ROM+RAM+BATTERY",
        0x0B..=0x0D => "MMM01",
        0x0F => "MBC3+TIMER+BATTERY",
        0x10 => "MBC3+TIMER+RAM+BATTERY",
        0x11 => "MBC3",
        0x12 => "MBC3+RAM",
        0x13 => "MBC3+RAM+BATTERY",
        0x19 => "MBC5",
        0x1A => "MBC5+RAM",
        0x1B => "MBC5+RAM+BATTERY",
        0x1C => "MBC5+RUMBLE",
        0x1D => "MBC5+RUMBLE+RAM",
        0x1E => "MBC5+RUMBLE+RAM+BATTERY",
        0x20 => "MBC6",
        0x22 => "MBC7",
        0xFC => "POCKET CAMERA",
        0xFE => "HuC3",
        0xFF => "HuC1+RAM+BATTERY",
        _ => "UNKNOWN",
    }
}

fn mbc_from_rom(rom: &[u8]) -> &'static str {
    rom.get(0x147)
        .copied()
        .map(cartridge_type)
        .unwrap_or("UNKNOWN")
}

/// True if `path` has a Game Boy ROM extension (`.gb` / `.gbc`).
pub fn is_rom_path(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()),
        Some(ext) if ext == "gb" || ext == "gbc"
    )
}

/// Build a [`RomEntry`] from a file on disk, reading just enough of the header.
/// Returns `None` for non-ROM extensions or unreadable files.
pub fn entry_from_path(path: &Path) -> Option<RomEntry> {
    if !is_rom_path(path) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    Some(entry_from_bytes(path, &data))
}

/// Build a [`RomEntry`] from a path and its bytes (split out for testing).
pub fn entry_from_bytes(path: &Path, data: &[u8]) -> RomEntry {
    let title = read_title(data).unwrap_or_else(|| {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "(unknown)".to_string())
    });
    RomEntry {
        path: path.to_path_buf(),
        title,
        mbc: mbc_from_rom(data),
        size: data.len() as u64,
    }
}

/// Scan `dir` (non-recursively) for ROMs, returning entries sorted by title.
/// A missing or unreadable directory yields an empty list, never an error.
pub fn scan_dir(dir: &Path) -> Vec<RomEntry> {
    let mut entries: Vec<RomEntry> = match std::fs::read_dir(dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .filter_map(|p| entry_from_path(&p))
            .collect(),
        Err(_) => Vec::new(),
    };
    sort_entries(&mut entries);
    entries
}

/// Sort by title (case-insensitive), then path, for a stable, human-friendly
/// order.
pub fn sort_entries(entries: &mut [RomEntry]) {
    entries.sort_by(|a, b| {
        a.title
            .to_ascii_lowercase()
            .cmp(&b.title.to_ascii_lowercase())
            .then_with(|| a.path.cmp(&b.path))
    });
}

/// Case-insensitive subsequence match: every char of `needle` appears in
/// `haystack` in order. Empty needle matches everything. This is the same
/// "fuzzy" feel as fzf / k9s filters.
pub fn fuzzy_match(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let mut hay = haystack.chars().flat_map(|c| c.to_lowercase());
    for nc in needle.chars().flat_map(|c| c.to_lowercase()) {
        let mut found = false;
        for hc in hay.by_ref() {
            if hc == nc {
                found = true;
                break;
            }
        }
        if !found {
            return false;
        }
    }
    true
}

/// Indices into `entries` (in order) that match `query` against title *or* file
/// name. An empty query returns every index.
pub fn filter_indices(entries: &[RomEntry], query: &str) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            fuzzy_match(&e.title, query) || fuzzy_match(&e.file_name(), query)
        })
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal fake ROM: 0x150 bytes with a title and a cartridge-type byte.
    fn fake_rom(title: &str, cgb: bool, type_byte: u8) -> Vec<u8> {
        let mut rom = vec![0u8; 0x150];
        rom[0x143] = if cgb { 0x80 } else { 0x00 };
        rom[0x147] = type_byte;
        for (i, b) in title.bytes().enumerate() {
            rom[0x134 + i] = b;
        }
        rom
    }

    #[test]
    fn reads_dmg_title() {
        let rom = fake_rom("POKEMON BLUE", false, 0x13);
        assert_eq!(read_title(&rom).as_deref(), Some("POKEMON BLUE"));
    }

    #[test]
    fn cgb_title_capped_at_11() {
        // 15-char title but CGB flag set -> only first 11 chars are the title.
        let rom = fake_rom("ABCDEFGHIJKLMNO", true, 0x1B);
        assert_eq!(read_title(&rom).as_deref(), Some("ABCDEFGHIJK"));
    }

    #[test]
    fn title_terminates_on_nul_and_nonprintable() {
        let mut rom = fake_rom("TETRIS", false, 0x00);
        // Junk after the NUL must not leak in.
        rom[0x134 + 6] = 0x00;
        rom[0x134 + 7] = 0xFF;
        assert_eq!(read_title(&rom).as_deref(), Some("TETRIS"));
    }

    #[test]
    fn blank_title_is_none() {
        let rom = fake_rom("   ", false, 0x00);
        assert_eq!(read_title(&rom), None);
    }

    #[test]
    fn short_rom_has_no_title() {
        assert_eq!(read_title(&[0u8; 16]), None);
    }

    #[test]
    fn cartridge_types_named() {
        assert_eq!(cartridge_type(0x00), "ROM ONLY");
        assert_eq!(cartridge_type(0x13), "MBC3+RAM+BATTERY");
        assert_eq!(cartridge_type(0x1B), "MBC5+RAM+BATTERY");
        assert_eq!(cartridge_type(0x99), "UNKNOWN");
    }

    #[test]
    fn entry_falls_back_to_file_stem() {
        let rom = fake_rom("   ", false, 0x01); // blank title
        let e = entry_from_bytes(Path::new("/roms/my-game.gb"), &rom);
        assert_eq!(e.title, "my-game");
        assert_eq!(e.mbc, "MBC1");
        assert_eq!(e.size, rom.len() as u64);
    }

    #[test]
    fn entry_prefers_header_title() {
        let rom = fake_rom("ZELDA", false, 0x03);
        let e = entry_from_bytes(Path::new("/roms/whatever.gb"), &rom);
        assert_eq!(e.title, "ZELDA");
    }

    #[test]
    fn rom_extensions_recognised() {
        assert!(is_rom_path(Path::new("a.gb")));
        assert!(is_rom_path(Path::new("a.GBC")));
        assert!(is_rom_path(Path::new("a.Gb")));
        assert!(!is_rom_path(Path::new("a.zip")));
        assert!(!is_rom_path(Path::new("a")));
    }

    #[test]
    fn fuzzy_matches_subsequence_case_insensitive() {
        assert!(fuzzy_match("Pokemon Blue", "pkb"));
        assert!(fuzzy_match("Pokemon Blue", "blue"));
        assert!(fuzzy_match("Pokemon Blue", ""));
        assert!(!fuzzy_match("Pokemon Blue", "xyz"));
        assert!(!fuzzy_match("Pokemon Blue", "bluex"));
    }

    #[test]
    fn filter_indices_matches_title_or_filename() {
        let entries = vec![
            entry_from_bytes(
                Path::new("/r/blue.gb"),
                &fake_rom("POKEMON BLUE", false, 0),
            ),
            entry_from_bytes(Path::new("/r/tetris.gb"), &fake_rom("TETRIS", false, 0)),
        ];
        // Matches title.
        assert_eq!(filter_indices(&entries, "pok"), vec![0]);
        // Matches file name even though title differs.
        assert_eq!(filter_indices(&entries, "tetr"), vec![1]);
        // Empty query returns all.
        assert_eq!(filter_indices(&entries, ""), vec![0, 1]);
    }

    #[test]
    fn sort_is_case_insensitive_by_title() {
        let mut entries = vec![
            entry_from_bytes(Path::new("/r/z.gb"), &fake_rom("zelda", false, 0)),
            entry_from_bytes(Path::new("/r/a.gb"), &fake_rom("Alleyway", false, 0)),
        ];
        sort_entries(&mut entries);
        assert_eq!(entries[0].title, "Alleyway");
        assert_eq!(entries[1].title, "zelda");
    }

    /// Read a *real* cartridge header end-to-end. Gated on the vendored acid2
    /// ROM being present so the suite still passes on a bare checkout.
    #[test]
    fn reads_real_vendored_cartridge_header() {
        let path = Path::new("third_party/dmg-acid2/dmg-acid2.gb");
        if !path.exists() {
            return; // vendored ROM absent (e.g. running from another dir)
        }
        let entry = entry_from_path(path).expect("acid2 is a valid .gb");
        assert_eq!(entry.title, "DMG-ACID2");
        assert_eq!(entry.mbc, "ROM ONLY");
        assert_eq!(entry.size, 32 * 1024);
    }
}

//! **Every species in one archive, browsed with one button.**
//!
//! `src/bin/pokedex.rs` used to hold exactly one picture — Charizard, and only
//! Charizard, because that is what the brief asked for first. This is the
//! generalisation: one flash blob holding all 151 front sprites, a directory
//! naming each one, and a firmware loop that steps through it.
//!
//! **Still nobody's picture but the cartridge's.** `src/pokedex_host.rs` is
//! `src/sprite_host.rs` run once per species instead of once — the same PNG
//! decode, the same alpha-flatten, the same [`crate::gbscreen::shade_of`]
//! quantiser — and every species' own [`crate::sprite::Sprite`] blob is
//! embedded here **unmodified**, byte for byte. This module adds nothing to a
//! picture; it only says where in the flash each one starts.
//!
//! # The archive format
//!
//! ```text
//! [0..4)   magic "GBPX"
//! [4..6)   count: u16 LE
//! directory, `count` entries of ENTRY_SIZE bytes each, right after the header:
//!   [0..2)   pokedex number: u16 LE
//!   [2..14)  name, ASCII, space-padded to NAME_LEN
//!   [14..18) offset: u32 LE, from the START OF THE PAYLOAD (not of the file)
//!   [18..22) length: u32 LE
//! payload: `count` sprite blobs, concatenated, each one exactly what
//!   `src/sprite_host.rs` already writes and [`crate::sprite::Sprite::parse`]
//!   already reads — this module never touches a pixel.
//! ```
//!
//! One file, one `include_bytes!`, the same trick `TERMINALGB_SPRITE` and
//! `TERMINALGB_ROM` already use: it lands in `.rodata`, the ESP32-S3
//! memory-maps it out of flash, and the ~108 KB of pictures for all 151
//! species costs no RAM at all.
//!
//! Like `geom.rs`, `gbscreen.rs`, `font.rs` and `sprite.rs` this file carries
//! no `esp-hal` dependency, so `tools/m5paper-build.sh test` compiles the very
//! parser the firmware runs — nobody on the development end can see the paper,
//! so a truncated archive or an off-by-one directory offset is checked here,
//! not discovered on the glass.

#[cfg(not(test))]
use crate::sprite;
#[cfg(test)]
#[path = "sprite.rs"]
mod sprite;

use sprite::Sprite;

pub const MAGIC: &[u8; 4] = b"GBPX";
/// ASCII, space-padded. 12 fits every Gen 1 species name with room to spare —
/// the longest is 10 characters — and `src/pokedex_host.rs` refuses to build
/// an archive with a name that does not fit rather than silently truncating it.
pub const NAME_LEN: usize = 12;
/// Bytes per directory entry: number (2) + name (`NAME_LEN`) + offset (4) + length (4).
pub const ENTRY_SIZE: usize = 2 + NAME_LEN + 4 + 4;
/// Bytes before the directory: magic (4) + count (2).
pub const HEADER: usize = 6;

/// One species, read out of the archive: its Pokédex number, its name, and
/// the picture itself. `name` borrows straight out of the archive's own
/// bytes — trimmed of its padding, nothing copied.
pub struct Entry<'a> {
    pub number: u16,
    pub name: &'a str,
    pub sprite: Sprite<'a>,
}

/// The whole Pokédex, as one flash blob.
pub struct Archive<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> Archive<'a> {
    /// Read an archive built by `src/pokedex_host.rs`.
    ///
    /// Every entry's sprite is parsed here, once, at boot — not lazily when a
    /// button is pressed — so [`Archive::entry`] can never fail on a device
    /// with no screen of its own to report it on. The one thing this does
    /// *not* check is whether the pictures are the right ones; that is the
    /// build command's job, from the file names `pixelgb species` wrote.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() < HEADER {
            return Err("pokedex: shorter than its own header");
        }
        if &bytes[..4] != MAGIC {
            return Err("pokedex: bad magic — is TERMINALGB_POKEDEX the file the build script wrote?");
        }
        let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        if count == 0 {
            return Err("pokedex: empty archive");
        }
        let dir_end = HEADER + count * ENTRY_SIZE;
        if bytes.len() < dir_end {
            return Err("pokedex: directory runs past the end of the file");
        }
        let payload = &bytes[dir_end..];
        for i in 0..count {
            let (_, _, offset, length) = Self::raw_entry(bytes, i);
            let slice = payload
                .get(offset..offset + length)
                .ok_or("pokedex: an entry's offset/length falls outside the payload")?;
            Sprite::parse(slice)?;
        }
        Ok(Self { bytes, count })
    }

    pub const fn len(&self) -> usize {
        self.count
    }

    /// The directory fields for entry `i`, with the name trimmed of its
    /// trailing padding. Assumes `i < count` and the archive already
    /// validated — both true of anything [`Archive::parse`] returned `Ok` for.
    fn raw_entry(bytes: &'a [u8], i: usize) -> (u16, &'a [u8], usize, usize) {
        let pos = HEADER + i * ENTRY_SIZE;
        let number = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]);
        let name = &bytes[pos + 2..pos + 2 + NAME_LEN];
        let offset = u32::from_le_bytes([
            bytes[pos + 2 + NAME_LEN],
            bytes[pos + 3 + NAME_LEN],
            bytes[pos + 4 + NAME_LEN],
            bytes[pos + 5 + NAME_LEN],
        ]) as usize;
        let length = u32::from_le_bytes([
            bytes[pos + 6 + NAME_LEN],
            bytes[pos + 7 + NAME_LEN],
            bytes[pos + 8 + NAME_LEN],
            bytes[pos + 9 + NAME_LEN],
        ]) as usize;
        (number, name, offset, length)
    }

    /// Entry `i`, or `None` if the archive has fewer than `i + 1` species.
    pub fn entry(&self, i: usize) -> Option<Entry<'a>> {
        if i >= self.count {
            return None;
        }
        let (number, name_bytes, offset, length) = Self::raw_entry(self.bytes, i);
        let dir_end = HEADER + self.count * ENTRY_SIZE;
        let payload = &self.bytes[dir_end..];
        // Both slice and parse were already proved to succeed in `parse()`.
        let slice = &payload[offset..offset + length];
        let sprite = Sprite::parse(slice).expect("validated in Archive::parse");
        let name_len = name_bytes.iter().rposition(|&b| b != b' ').map_or(0, |p| p + 1);
        let name = core::str::from_utf8(&name_bytes[..name_len]).unwrap_or("?");
        Some(Entry { number, name, sprite })
    }

    /// The index of the species with this Pokédex number, if it is in the
    /// archive. Used to pick a starting entry without hard-coding a position.
    pub fn index_of(&self, number: u16) -> Option<usize> {
        (0..self.count).find(|&i| Self::raw_entry(self.bytes, i).0 == number)
    }
}

/// The index one press of "next" lands on. Wraps at the end; `len == 0` is
/// unreachable through [`Archive::parse`] (it refuses an empty archive) but
/// this stays total rather than assuming its only caller checked.
pub const fn next_index(current: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (current + 1) % len
    }
}

/// The index one press of "previous" lands on — the mirror of [`next_index`].
pub const fn prev_index(current: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (current + len - 1) % len
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a tiny archive by hand, the way `src/pokedex_host.rs` would for
    /// three species, so the parser is checked against a known layout rather
    /// than against its own writer.
    fn archive(entries: &[(u16, &str, usize, usize)]) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut spans = Vec::new();
        for &(_, _, w, h) in entries {
            let start = payload.len();
            payload.extend_from_slice(&MAGIC_SPRITE[..]);
            payload.extend_from_slice(&(w as u16).to_le_bytes());
            payload.extend_from_slice(&(h as u16).to_le_bytes());
            let px = vec![0xFFu8; Sprite::packed_len(w, h)]; // all shade 3
            payload.extend_from_slice(&px);
            spans.push((start, payload.len() - start));
        }

        let mut out = Vec::from(&MAGIC[..]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (&(number, name, _, _), &(offset, length)) in entries.iter().zip(&spans) {
            out.extend_from_slice(&number.to_le_bytes());
            let mut field = [b' '; NAME_LEN];
            field[..name.len()].copy_from_slice(name.as_bytes());
            out.extend_from_slice(&field);
            out.extend_from_slice(&(offset as u32).to_le_bytes());
            out.extend_from_slice(&(length as u32).to_le_bytes());
        }
        out.extend_from_slice(&payload);
        out
    }

    const MAGIC_SPRITE: [u8; 4] = *b"GBSP";

    const THREE: &[(u16, &str, usize, usize)] =
        &[(1, "ONE", 4, 4), (2, "TWO", 8, 8), (150, "ONE FIFTY", 2, 2)];

    #[test]
    fn every_entry_round_trips_number_name_and_size() {
        let bytes = archive(THREE);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.len(), 3);
        for (i, &(number, name, w, h)) in THREE.iter().enumerate() {
            let e = a.entry(i).unwrap();
            assert_eq!(e.number, number);
            assert_eq!(e.name, name);
            assert_eq!((e.sprite.width(), e.sprite.height()), (w, h));
        }
    }

    #[test]
    fn a_name_shorter_than_the_field_has_no_trailing_spaces() {
        let bytes = archive(THREE);
        let a = Archive::parse(&bytes).unwrap();
        let e = a.entry(0).unwrap();
        assert_eq!(e.name, "ONE");
        assert!(!e.name.ends_with(' '));
    }

    #[test]
    fn index_out_of_range_is_none_not_a_panic() {
        let bytes = archive(THREE);
        let a = Archive::parse(&bytes).unwrap();
        assert!(a.entry(3).is_none());
        assert!(a.entry(9999).is_none());
    }

    #[test]
    fn index_of_finds_the_matching_pokedex_number() {
        let bytes = archive(THREE);
        let a = Archive::parse(&bytes).unwrap();
        assert_eq!(a.index_of(2), Some(1));
        assert_eq!(a.index_of(150), Some(2));
        assert_eq!(a.index_of(6), None, "not every number is in every archive");
    }

    #[test]
    fn wrong_magic_is_refused() {
        let mut bytes = archive(THREE);
        bytes[0] = b'X';
        assert!(Archive::parse(&bytes).is_err());
    }

    #[test]
    fn a_truncated_directory_is_refused_not_read_out_of_bounds() {
        let bytes = archive(THREE);
        // Cut it off inside the directory, well before any payload.
        let short = &bytes[..HEADER + ENTRY_SIZE];
        assert!(Archive::parse(short).is_err());
    }

    #[test]
    fn an_offset_that_overruns_the_payload_is_refused() {
        let mut bytes = archive(&[(1, "ONE", 4, 4)]);
        // The one entry's length field, at HEADER + 2 (number) + NAME_LEN + 4 (offset).
        let len_pos = HEADER + 2 + NAME_LEN + 4;
        let huge = 0xFFFF_FFFFu32;
        bytes[len_pos..len_pos + 4].copy_from_slice(&huge.to_le_bytes());
        assert!(Archive::parse(&bytes).is_err());
    }

    #[test]
    fn empty_archive_is_refused_rather_than_silently_showing_nothing() {
        let mut bytes = Vec::from(&MAGIC[..]);
        bytes.extend_from_slice(&0u16.to_le_bytes());
        assert!(Archive::parse(&bytes).is_err());
    }

    #[test]
    fn next_wraps_from_the_last_entry_to_the_first() {
        assert_eq!(next_index(0, 3), 1);
        assert_eq!(next_index(1, 3), 2);
        assert_eq!(next_index(2, 3), 0);
    }

    #[test]
    fn previous_wraps_from_the_first_entry_to_the_last() {
        assert_eq!(prev_index(0, 3), 2);
        assert_eq!(prev_index(1, 3), 0);
        assert_eq!(prev_index(2, 3), 1);
    }

    #[test]
    fn next_and_previous_are_exact_inverses_over_a_whole_lap() {
        let len = 151;
        let mut i = 0;
        for _ in 0..len {
            i = next_index(i, len);
        }
        assert_eq!(i, 0, "151 presses of next is back where it started");
        for _ in 0..len {
            i = prev_index(i, len);
        }
        assert_eq!(i, 0, "and 151 presses of previous undoes it exactly");
    }
}

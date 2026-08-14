//! Gen 1 text decoding.
//!
//! Gen 1 does not use ASCII. Its font order is given by `constants/charmap.asm`
//! in [`pret/pokered`](https://github.com/pret/pokered): `$80`-`$99` are `A`-`Z`,
//! `$A0`-`$B9` are `a`-`z`, `$F6`-`$FF` are `0`-`9`, `$7F` is a space and `$50`
//! terminates a string. Everything else in the table is punctuation or a
//! control code.
//!
//! Only the codes a *name* can contain are mapped — nicknames, trainer names and
//! the tables in ROM. Text-engine control codes (`$00`-`$5F`) are not names and
//! are rendered as nothing rather than being guessed at.

/// Decode a `$50`-terminated Gen 1 string of at most `max` bytes.
pub fn decode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    for &b in bytes {
        // $50 is "@", the string terminator.
        if b == 0x50 {
            break;
        }
        out.push_str(char_for(b));
    }
    out.trim_end().to_string()
}

/// Decode a run of tiles as displayed text rather than as a string.
///
/// `wTileMap` (`$C3A0`, 20x18) holds Gen 1 charmap codes directly — the font is
/// loaded so that a tile's index *is* its character code — so decoding it reads
/// back exactly what a player would see on screen. Unlike [`decode`] this does
/// not stop at `$50` and renders anything unmapped as a space, because a screen
/// is full of border and cursor tiles that are not text.
///
/// This is what lets a test compare the panel against the game's own display
/// instead of against another copy of our own assumptions.
pub fn decode_screen(tiles: &[u8]) -> String {
    tiles
        .iter()
        .map(|&b| match b {
            0x50 => " ",
            other => match char_for(other) {
                "" => " ",
                s => s,
            },
        })
        .collect()
}

/// One byte's rendering. Multi-character results are real: `$BD` is the single
/// tile `'s`, and `$54` is the word "POKé".
fn char_for(b: u8) -> &'static str {
    match b {
        0x4A => "PKMN",
        0x54 => "POKé",
        0x5D => "TRAINER",
        0x60 => "′",
        0x61 => "″",
        0x6D => ":",
        0x70 => "'",
        0x71 => "'",
        0x72 => "\"",
        0x73 => "\"",
        0x75 => "…",
        0x7F => " ",
        0x80..=0x99 => LETTERS_UPPER[(b - 0x80) as usize],
        0x9A => "(",
        0x9B => ")",
        0x9C => ":",
        0x9D => ";",
        0x9E => "[",
        0x9F => "]",
        0xA0..=0xB9 => LETTERS_LOWER[(b - 0xA0) as usize],
        0xBA => "é",
        0xBB => "'d",
        0xBC => "'l",
        0xBD => "'s",
        0xBE => "'t",
        0xBF => "'v",
        0xE0 => "'",
        0xE1 => "PK",
        0xE2 => "MN",
        0xE3 => "-",
        0xE4 => "'r",
        0xE5 => "'m",
        0xE6 => "?",
        0xE7 => "!",
        0xE8 => ".",
        0xEF => "♂",
        0xF0 => "¥",
        0xF1 => "×",
        0xF2 => ".",
        0xF3 => "/",
        0xF4 => ",",
        0xF5 => "♀",
        0xF6..=0xFF => DIGITS[(b - 0xF6) as usize],
        _ => "",
    }
}

const LETTERS_UPPER: [&str; 26] = [
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q",
    "R", "S", "T", "U", "V", "W", "X", "Y", "Z",
];

const LETTERS_LOWER: [&str; 26] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l", "m", "n", "o", "p", "q",
    "r", "s", "t", "u", "v", "w", "x", "y", "z",
];

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// Three-byte big-endian BCD, as `wPlayerMoney` stores it: each nybble is one
/// decimal digit, so `$01 $23 $45` is 12345.
pub fn bcd3(bytes: [u8; 3]) -> u32 {
    let mut v = 0u32;
    for b in bytes {
        v = v * 100 + (b >> 4) as u32 * 10 + (b & 0x0F) as u32;
    }
    v
}

/// True when every byte before the terminator is something a name can contain.
///
/// This is the cheap sanity check that stops the panel showing plausible
/// nonsense before a save is loaded: at the title screen the party area holds
/// whatever the boot code left there, and it almost never decodes as a name.
pub fn looks_like_a_name(bytes: &[u8]) -> bool {
    let mut seen = 0;
    for &b in bytes {
        if b == 0x50 {
            break;
        }
        let ok = matches!(b, 0x7F | 0x80..=0x9F | 0xA0..=0xBF | 0xE0..=0xFF);
        if !ok {
            return false;
        }
        seen += 1;
    }
    seen > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_upper_lower_and_digits() {
        // "RED" then the terminator.
        assert_eq!(decode(&[0x91, 0x84, 0x83, 0x50, 0x99]), "RED");
        // "abc9"
        assert_eq!(decode(&[0xA0, 0xA1, 0xA2, 0xFF, 0x50]), "abc9");
    }

    #[test]
    fn spaces_are_trimmed_but_not_internal_ones() {
        // "MR MIME" as it is stored: $7F is the space tile, trailing padding
        // before the terminator is common.
        let bytes = [0x8C, 0x91, 0x7F, 0x8C, 0x88, 0x8C, 0x84, 0x7F, 0x7F, 0x50];
        assert_eq!(decode(&bytes), "MR MIME");
    }

    #[test]
    fn gendered_nidoran_decodes_to_its_symbol() {
        // NIDORAN♂ is stored with $EF as the last character.
        let bytes = [0x8D, 0x88, 0x83, 0x8E, 0x91, 0x80, 0x8D, 0xEF, 0x50];
        assert_eq!(decode(&bytes), "NIDORAN♂");
    }

    #[test]
    fn farfetchd_uses_the_apostrophe_d_tile() {
        // FARFETCH'D is FARFETCH + $BB ("'d").
        let bytes = [0x85, 0x80, 0x91, 0x85, 0x84, 0x93, 0x82, 0x87, 0xBB, 0x50];
        assert_eq!(decode(&bytes), "FARFETCH'd");
    }

    #[test]
    fn bcd_money_reads_as_decimal_digits() {
        assert_eq!(bcd3([0x00, 0x00, 0x00]), 0);
        assert_eq!(bcd3([0x01, 0x23, 0x45]), 12345);
        assert_eq!(bcd3([0x99, 0x99, 0x99]), 999_999);
        // The starting 3000 the game gives you.
        assert_eq!(bcd3([0x00, 0x30, 0x00]), 3000);
    }

    #[test]
    fn garbage_is_rejected_as_a_name() {
        assert!(looks_like_a_name(&[0x91, 0x84, 0x83, 0x50]));
        // Control codes and zeroes are what uninitialised WRAM looks like.
        assert!(!looks_like_a_name(&[0x00, 0x00, 0x00, 0x50]));
        assert!(!looks_like_a_name(&[0x91, 0x12, 0x83, 0x50]));
        // An immediately-terminated string is not a name either.
        assert!(!looks_like_a_name(&[0x50, 0x50]));
    }
}

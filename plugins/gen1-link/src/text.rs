//! Generation 1 (English) character encoding.
//!
//! Pokémon Red/Blue store text in a bespoke encoding, not ASCII: `$50` is the
//! string terminator, `$7F` is space, `$80`–`$99` are `A`–`Z`, `$A0`–`$B9` are
//! `a`–`z`, `$F6`–`$FF` are `0`–`9`, and the punctuation is scattered. A trainer
//! name or nickname that is not encoded this way renders as garbage in the game
//! and — because names are fixed-width, terminator-padded fields inside the
//! traded party block — a missing `$50` runs the name into the next field.
//!
//! Source of the mapping: the `pret/pokered` disassembly's
//! [`constants/charmap.asm`](https://github.com/pret/pokered/blob/master/constants/charmap.asm)
//! (English branch). Only the printable subset is implemented; control codes
//! (`<PLAYER>`, `<LINE>`, …) have no business in a name.

/// String terminator ("@" in the disassembly's charmap).
pub const TERMINATOR: u8 = 0x50;
/// Space.
pub const SPACE: u8 = 0x7F;

/// Name fields in the party block are fixed 11-byte slots (`NAME_LENGTH`):
/// up to 10 characters plus the terminator.
pub const NAME_LENGTH: usize = 11;

/// Encode one ASCII character, or `None` if Gen 1 has no glyph for it.
pub fn encode_char(c: char) -> Option<u8> {
    Some(match c {
        'A'..='Z' => 0x80 + (c as u8 - b'A'),
        'a'..='z' => 0xA0 + (c as u8 - b'a'),
        '0'..='9' => 0xF6 + (c as u8 - b'0'),
        ' ' => SPACE,
        '(' => 0x9A,
        ')' => 0x9B,
        ':' => 0x9C,
        ';' => 0x9D,
        '[' => 0x9E,
        ']' => 0x9F,
        '\'' => 0xE0,
        '-' => 0xE3,
        '?' => 0xE6,
        '!' => 0xE7,
        '.' => 0xE8,
        '/' => 0xF3,
        ',' => 0xF4,
        '\u{2642}' => 0xEF, // ♂
        '\u{2640}' => 0xF5, // ♀
        '\u{00E9}' => 0xBA, // é
        _ => return None,
    })
}

/// Decode one Gen 1 byte to a `char`, or `None` for the terminator and for
/// bytes with no printable ASCII equivalent.
pub fn decode_char(b: u8) -> Option<char> {
    Some(match b {
        0x80..=0x99 => (b'A' + (b - 0x80)) as char,
        0xA0..=0xB9 => (b'a' + (b - 0xA0)) as char,
        0xF6..=0xFF => (b'0' + (b - 0xF6)) as char,
        SPACE => ' ',
        0x9A => '(',
        0x9B => ')',
        0x9C => ':',
        0x9D => ';',
        0x9E => '[',
        0x9F => ']',
        0xE0 => '\'',
        0xE3 => '-',
        0xE6 => '?',
        0xE7 => '!',
        0xE8 | 0xF2 => '.',
        0xF3 => '/',
        0xF4 => ',',
        0xEF => '\u{2642}',
        0xF5 => '\u{2640}',
        0xBA => '\u{00E9}',
        // 0x4E is the in-name word separator used by the ROM's own name and
        // move-name tables (rendered as a line break in menus, read as a space).
        0x4E => ' ',
        _ => return None,
    })
}

/// Encode `s` into a fixed `len`-byte field, terminated and padded with
/// [`TERMINATOR`].
///
/// Characters the encoding cannot represent are dropped rather than substituted,
/// and the text is truncated to `len - 1` glyphs so the terminator always fits.
/// That truncation is *why* the field is fixed width: the game copies exactly
/// `NAME_LENGTH` bytes per name with no length check.
pub fn encode_fixed(s: &str, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    for c in s.chars() {
        if out.len() + 1 >= len {
            break;
        }
        if let Some(b) = encode_char(c) {
            out.push(b);
        }
    }
    while out.len() < len {
        out.push(TERMINATOR);
    }
    out
}

/// Decode a terminator-delimited field into a `String`.
pub fn decode(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        if b == TERMINATOR {
            break;
        }
        if let Some(c) = decode_char(b) {
            s.push(c);
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip() {
        for name in ["MEW", "Pikachu", "RED", "MR.MIME", "NIDORAN\u{2642}"] {
            let enc = encode_fixed(name, NAME_LENGTH);
            assert_eq!(enc.len(), NAME_LENGTH);
            assert_eq!(decode(&enc), name);
        }
    }

    #[test]
    fn a_field_is_always_terminated_even_when_full() {
        // 10 glyphs is the maximum; the 11th byte must still be the terminator,
        // or the game reads the next field as part of this name.
        let enc = encode_fixed("ABCDEFGHIJKLMNOP", NAME_LENGTH);
        assert_eq!(enc.len(), NAME_LENGTH);
        assert_eq!(enc[10], TERMINATOR);
        assert_eq!(decode(&enc), "ABCDEFGHIJ");
    }

    #[test]
    fn known_byte_values_match_the_disassembly() {
        assert_eq!(encode_char('A'), Some(0x80));
        assert_eq!(encode_char('Z'), Some(0x99));
        assert_eq!(encode_char('a'), Some(0xA0));
        assert_eq!(encode_char('z'), Some(0xB9));
        assert_eq!(encode_char('0'), Some(0xF6));
        assert_eq!(encode_char('9'), Some(0xFF));
        assert_eq!(encode_char(' '), Some(0x7F));
    }

    #[test]
    fn unrepresentable_characters_are_dropped_not_mangled() {
        assert_eq!(decode(&encode_fixed("A\u{4E2D}B", NAME_LENGTH)), "AB");
    }
}

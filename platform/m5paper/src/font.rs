//! A 5x7 bitmap font, hand-written here, for the one caption this crate draws.
//!
//! **Why another font.** The repository already has one — `video::card`'s, also
//! 5x7 and also deliberately ours rather than vendored, so there is no
//! third-party font licence to audit. It is not reachable from this firmware:
//! `src/lib.rs` gates `pub mod video` on `not(feature = "embedded")` and this
//! crate builds the emulator with `embedded`, so the module does not exist in
//! this build at all. It is also a nested `mod card` inside a 165 KB
//! `video.rs`, so the `#[path]` include trick this crate uses for `geom.rs`
//! cannot reach it either.
//!
//! **What is different about this one, and it is the whole reason it is worth
//! writing rather than copying.** `AGENTS.md` records the trap the existing
//! font carries: it "silently draws anything else as a blank", and a `+` in a
//! caption once disappeared with nothing to say it had. [`glyph`] returns
//! [`Option`] and [`draw`] reports whether every character it was given was one
//! it had, so an unknown character is a value a caller can act on rather than a
//! hole in a picture. The strings this firmware actually draws are checked
//! against that at the bottom of this file, so a caption cannot reach the glass
//! with a character missing from it.
//!
//! Like `geom.rs` and `gbscreen.rs` this file carries no `esp-hal` dependency
//! and no `geom` dependency either — it plots through a closure — so
//! `rustc --test` compiles the very code the firmware runs.

/// Glyph cell, in pixels, before scaling. Column 0 is bit 4 of each row byte.
pub const GLYPH_W: usize = 5;
pub const GLYPH_H: usize = 7;

/// One blank column between glyphs, so `CHARIZARD` does not run together.
pub const TRACKING: usize = 1;

/// Unscaled advance from one glyph's left edge to the next.
pub const ADVANCE: usize = GLYPH_W + TRACKING;

/// The bitmap for one character, top row first, or `None` if this font has no
/// such glyph.
///
/// **Case is folded to upper**, because the whole font is capitals: a lower
/// case string would otherwise be a caption-shaped hole.
pub const fn glyph(c: char) -> Option<[u8; GLYPH_H]> {
    // `to_ascii_uppercase` is not const on char, and a match arm pair costs
    // nothing here.
    let c = match c {
        'a'..='z' => (c as u8 - b'a' + b'A') as char,
        other => other,
    };
    Some(match c {
        ' ' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00],
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x1F],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '#' => [0x0A, 0x0A, 0x1F, 0x0A, 0x1F, 0x0A, 0x0A],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        _ => return None,
    })
}

/// Width in pixels of `s` drawn at `scale`, trailing tracking excluded.
pub fn width(s: &str, scale: usize) -> usize {
    match s.chars().count() {
        0 => 0,
        n => (n * ADVANCE - TRACKING) * scale,
    }
}

/// Height in pixels of one line at `scale`.
pub const fn height(scale: usize) -> usize {
    GLYPH_H * scale
}

/// Draw `s` with its top-left at `(x, y)`, calling `plot` for each set pixel.
///
/// Returns `false` if any character had no glyph — those are **skipped**, and
/// the caller is told so rather than being handed a caption with a silent hole
/// in it. Nothing is clipped here; the framebuffer's own `set` ignores
/// out-of-range coordinates.
pub fn draw(s: &str, x: usize, y: usize, scale: usize, plot: &mut impl FnMut(usize, usize)) -> bool {
    let mut all = true;
    for (i, c) in s.chars().enumerate() {
        let Some(rows) = glyph(c) else {
            all = false;
            continue;
        };
        let gx = x + i * ADVANCE * scale;
        for (ry, bits) in rows.iter().enumerate() {
            for cx in 0..GLYPH_W {
                if bits & (1 << (GLYPH_W - 1 - cx)) == 0 {
                    continue;
                }
                for sy in 0..scale {
                    for sx in 0..scale {
                        plot(gx + cx * scale + sx, y + ry * scale + sy);
                    }
                }
            }
        }
    }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Collect the pixels `draw` plots, as a set of coordinates.
    fn plotted(s: &str, scale: usize) -> (Vec<(usize, usize)>, bool) {
        let mut v = Vec::new();
        let ok = draw(s, 0, 0, scale, &mut |x, y| v.push((x, y)));
        (v, ok)
    }

    #[test]
    fn every_character_this_firmware_draws_has_a_glyph() {
        // The captions `sprite.rs` composes, and the ink names `gbscreen.rs`
        // supplies for the look line. A missing glyph is a hole in the picture
        // and nothing on the device can report one, so it is caught here.
        for s in [
            "CHARIZARD",
            "#006",
            "POKEMON BLUE",
            "FRONT SPRITE",
            "56 X 56",
            "LOOK:",
            "DITHER",
            "RED/GREEN",
            "GREEN/YELLOW",
            "BLUE/GREEN",
            "RED/YELLOW",
            "0123456789",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            "-.:/# ",
        ] {
            let (_, ok) = plotted(s, 1);
            assert!(ok, "{s:?} contains a character this font has no glyph for");
        }
    }

    #[test]
    fn an_unknown_character_is_reported_rather_than_silently_blank() {
        // The defect AGENTS.md records against the existing font: a `+` in a
        // caption disappeared and nothing said so.
        let (px, ok) = plotted("A+A", 1);
        assert!(!ok, "an unknown character must be reported");
        // It is skipped, not substituted: two A's worth of ink, and the second
        // still lands in the third cell so the caption does not reflow.
        let a_only = plotted("A", 1).0.len();
        assert_eq!(px.len(), a_only * 2);
        assert!(px.iter().any(|&(x, _)| x >= 2 * ADVANCE));
    }

    #[test]
    fn lower_case_draws_the_same_glyph_as_upper() {
        assert_eq!(plotted("charizard", 2).0, plotted("CHARIZARD", 2).0);
    }

    #[test]
    fn a_glyph_stays_inside_its_own_cell_at_every_scale() {
        for scale in 1..=4 {
            let (px, ok) = plotted("W", scale);
            assert!(ok);
            assert!(px.iter().all(|&(x, y)| x < GLYPH_W * scale && y < GLYPH_H * scale));
        }
    }

    #[test]
    fn width_matches_what_draw_actually_covers() {
        for s in ["A", "AB", "CHARIZARD"] {
            for scale in 1..=3 {
                let (px, _) = plotted(s, scale);
                let rightmost = px.iter().map(|&(x, _)| x).max().unwrap() + 1;
                // `width` excludes the trailing tracking, so the widest glyph
                // may fall short of it but must never exceed it.
                assert!(rightmost <= width(s, scale), "{s:?} at {scale}");
                assert!(width(s, scale) - rightmost < TRACKING * scale + scale);
            }
        }
        assert_eq!(width("", 3), 0);
    }

    #[test]
    fn space_draws_nothing_but_still_advances() {
        assert!(plotted(" ", 2).0.is_empty());
        let (px, _) = plotted(" A", 1);
        assert!(px.iter().all(|&(x, _)| x >= ADVANCE));
    }

    #[test]
    fn no_two_characters_share_a_bitmap() {
        // A duplicated glyph is a typo that reads as a working font: `0` drawn
        // where `O` was meant is invisible until somebody reads the caption.
        let mut seen: Vec<(char, [u8; GLYPH_H])> = Vec::new();
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789#-./:".chars() {
            let g = glyph(c).expect("listed characters all have glyphs");
            if let Some((other, _)) = seen.iter().find(|(_, o)| *o == g) {
                panic!("{c:?} and {other:?} have the same bitmap");
            }
            seen.push((c, g));
        }
        // Space is the one deliberate blank.
        assert_eq!(glyph(' '), Some([0; GLYPH_H]));
    }
}

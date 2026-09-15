//! One species' own picture, out of the cartridge and onto the paper.
//!
//! **What this is for.** The rest of this crate runs a game: it emulates, and
//! every eight seconds a different frame reaches the glass. E Ink's own
//! strength is the opposite of that — a picture it is *holding* costs nothing
//! at all, and holds with the power off — so this module draws one still,
//! large, once, and stops. `src/bin/charizard.rs` is the firmware that uses it.
//!
//! **The pixels are the cartridge's, and nothing here invents any of them.**
//! They are decompressed by `pixelgb species` (a sibling project) from the
//! player's own Pokémon Blue image and checked byte for byte against
//! `pret/pokered`'s own committed source art — the artist's original files,
//! which the compressed data in the cartridge was compiled *from*. What reaches
//! flash is [`Sprite`]'s packed shade indices, produced at build time by
//! `src/sprite_host.rs`; **no game art is committed to this repository**, the
//! same rule the cartridge itself follows. `docs/m5paper-charizard.md` has the
//! verification and the exact command.
//!
//! **Gen 1 art is four greys and this panel has no grey at all.** Six inks,
//! black white yellow red blue green, and that is the whole palette. So a
//! choice has to be made, it is made by eye rather than by reasoning, and this
//! module refuses to make it silently: it reuses [`gbscreen::LOOKS`] unchanged
//! — the same five looks the emulator's own picture cycles through — and the
//! firmware's button C cycles them here too. What the ramps do to a Game Boy
//! shade is documented there. The one worth knowing about is
//! `Look::Ramp("red/yellow", [Black, Red, Yellow, White])`: its inks are
//! *strictly monotonic in luminance* (0, 57, 225, 255), so it preserves the
//! artist's own light-to-dark ordering exactly and changes only the hue — which
//! on a fire lizard reads as a recolour that happens to be right. **It is still
//! a recolour: Gen 1 had no colour, and this is our hue choice, not the
//! cartridge's.**
//!
//! Like `geom.rs`, `gbscreen.rs` and `font.rs` this file carries no `esp-hal`
//! dependency, so `tools/m5paper-build.sh test` compiles the very code the
//! firmware runs — which is the only way the placement, the scale and the ink
//! mapping get checked at all, because nobody on the development end can see
//! the paper.

#[cfg(not(test))]
use crate::font;
#[cfg(not(test))]
use crate::gbscreen;
#[cfg(not(test))]
use crate::geom::{FrameBuffer, Ink};

// Under `rustc --test` this file is its own crate root, so it pulls its
// siblings in directly. `gbscreen` re-exports the `geom` it included, and
// everything here goes through that one instance — see the note on its own
// `pub mod geom`.
#[cfg(test)]
#[path = "gbscreen.rs"]
mod gbscreen;
#[cfg(test)]
#[path = "font.rs"]
mod font;
#[cfg(test)]
use gbscreen::geom::{FrameBuffer, Ink};

use gbscreen::Look;

/// The header `src/sprite_host.rs` writes, so the firmware can refuse a blob
/// that is not one of ours rather than drawing whatever the bytes happen to be.
pub const MAGIC: &[u8; 4] = b"GBSP";

/// Bytes before the pixels: magic, then width and height as `u16` little-endian.
pub const HEADER: usize = 8;

/// A species picture: one two-bit shade index per pixel, four to a byte.
///
/// **Shade 0 is the DARKEST and shade 3 the lightest**, which is
/// [`gbscreen::shade_of`]'s convention and therefore [`gbscreen::LOOKS`]'
/// convention — a ramp's first ink is black. It is the *opposite* of the
/// convention `pret/pokered` and `pixelgb` use, where 0 is white, and getting
/// it backwards inverts the whole picture without failing anything. The
/// conversion happens once, in `src/sprite_host.rs`, and
/// [`tests::the_packing_round_trips`] pins this end of it.
pub struct Sprite<'a> {
    w: usize,
    h: usize,
    /// Packed 2bpp, row-major, the leftmost pixel in the two high bits.
    px: &'a [u8],
}

impl<'a> Sprite<'a> {
    /// Read a blob produced by `src/sprite_host.rs`.
    ///
    /// Every failure is named, because the alternative on a device with no
    /// screen of its own is a blank panel and no idea why.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, &'static str> {
        if bytes.len() < HEADER {
            return Err("sprite: shorter than its own header");
        }
        if &bytes[..4] != MAGIC {
            return Err("sprite: bad magic — is TERMINALGB_SPRITE the file the build script wrote?");
        }
        let w = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
        let h = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if w == 0 || h == 0 {
            return Err("sprite: zero-sized");
        }
        let want = Self::packed_len(w, h);
        if bytes.len() - HEADER != want {
            return Err("sprite: pixel data is not the size the header declares");
        }
        Ok(Self {
            w,
            h,
            px: &bytes[HEADER..],
        })
    }

    /// Bytes of packed 2bpp needed for a `w` x `h` picture. Rows are **not**
    /// byte-aligned: the picture is one run of pixels, four to a byte.
    pub const fn packed_len(w: usize, h: usize) -> usize {
        (w * h + 3) / 4
    }

    pub const fn width(&self) -> usize {
        self.w
    }
    pub const fn height(&self) -> usize {
        self.h
    }

    /// The shade at `(x, y)`, darkest first. Out of range reads as lightest,
    /// which is the page colour.
    #[inline]
    pub fn shade(&self, x: usize, y: usize) -> usize {
        if x >= self.w || y >= self.h {
            return 3;
        }
        let i = y * self.w + x;
        ((self.px[i >> 2] >> (6 - 2 * (i & 3))) & 0b11) as usize
    }
}

/// The largest whole-number magnification of `w` x `h` that fits `avail_w` x
/// `avail_h`, never zero.
///
/// **Whole numbers only, and that is not tidiness.** A Game Boy draws its
/// shading as a one-pixel checkerboard and resampling that at a fractional
/// ratio is moiré — the same finding `gbscreen::SCALE` records for the emulated
/// picture and `chainrender`'s camera rule 8 records for the swarm films.
pub const fn best_scale(w: usize, h: usize, avail_w: usize, avail_h: usize) -> usize {
    let by_w = avail_w / w;
    let by_h = avail_h / h;
    let s = if by_w < by_h { by_w } else { by_h };
    if s == 0 {
        1
    } else {
        s
    }
}

/// Margin from the panel edge to anything drawn, in pixels.
pub const MARGIN: usize = 24;
/// Width reserved down the right-hand side for the caption.
pub const CAPTION_W: usize = 190;
/// Gap between the picture and the caption column.
pub const GUTTER: usize = 20;

/// An 8x8 ordered (Bayer) threshold matrix, 0..63.
///
/// **Why ordered rather than [`gbscreen`]'s per-cell dither.** That one lights
/// *n* of a 2x2 block because the emulated picture is drawn at x2 and a 2x2
/// block is all there is. Here a pixel is magnified six or seven times, so the
/// cell is 36 to 49 panel pixels and a per-cell pattern would read as visible
/// blocks. A threshold taken in panel coordinates instead spreads the texture
/// across the whole picture and gives four convincing greys on a panel that has
/// none.
const BAYER: [[u8; 8]; 8] = [
    [0, 32, 8, 40, 2, 34, 10, 42],
    [48, 16, 56, 24, 50, 18, 58, 26],
    [12, 44, 4, 36, 14, 46, 6, 38],
    [60, 28, 52, 20, 62, 30, 54, 22],
    [3, 35, 11, 43, 1, 33, 9, 41],
    [51, 19, 59, 27, 49, 17, 57, 25],
    [15, 47, 7, 39, 13, 45, 5, 37],
    [63, 31, 55, 23, 61, 29, 53, 21],
];

/// How much of a shade is white, out of [`BAYER`]'s 64 levels.
///
/// The DMG's own greys are `000000`, `606060`, `c0c0c0`, `ffffff` — 0 %,
/// 37.6 %, 75.3 % and 100 % white — which on 64 levels is 0, 24, 48, 64.
/// Darkest first, matching [`Sprite::shade`].
const DITHER_LEVEL: [u8; 4] = [0, 24, 48, 64];

/// Which ink a dithered shade puts at panel pixel `(px, py)`.
#[inline]
fn dither_ink(shade: usize, px: usize, py: usize) -> Ink {
    if BAYER[py & 7][px & 7] < DITHER_LEVEL[shade] {
        Ink::White
    } else {
        Ink::Black
    }
}

/// Where the picture lands: its top-left corner and its magnification.
pub fn layout(sprite_w: usize, sprite_h: usize, screen_w: usize, screen_h: usize) -> (usize, usize, usize) {
    let avail_w = screen_w.saturating_sub(2 * MARGIN + CAPTION_W + GUTTER);
    let avail_h = screen_h.saturating_sub(2 * MARGIN);
    let scale = best_scale(sprite_w, sprite_h, avail_w, avail_h);
    let (w, h) = (sprite_w * scale, sprite_h * scale);
    // Centred in its own column, not on the screen: the caption owns the rest.
    let x = MARGIN + (avail_w - w.min(avail_w)) / 2;
    let y = (screen_h - h.min(screen_h)) / 2;
    (x, y, scale)
}

/// The caption beside the picture. `look` is appended as a line of its own.
pub struct Caption<'a> {
    /// Large, on top: the species' own name.
    pub name: &'a str,
    /// Large, under it: `#006`.
    pub number: &'a str,
    /// Small lines under the rule.
    pub lines: &'a [&'a str],
}

/// Paint the whole panel: white page, picture, box, caption.
///
/// Returns `false` if any caption character had no glyph — the picture is drawn
/// regardless, and the firmware prints the fact rather than shipping a caption
/// with a silent hole in it.
pub fn draw(fb: &mut FrameBuffer<'_>, sprite: &Sprite<'_>, look: Look, cap: &Caption<'_>) -> bool {
    let (sw, sh) = (fb.width(), fb.height());
    fb.clear(Ink::White);

    let (ox, oy, scale) = layout(sprite.width(), sprite.height(), sw, sh);
    let (pw, ph) = (sprite.width() * scale, sprite.height() * scale);

    // The box sits just outside the picture. A Gen 1 sprite's background is the
    // page's own colour, so without it the picture has no edge at all — the
    // test card learned that and `gbscreen::blit` carries the same rule.
    let (bx, by) = (ox.saturating_sub(2), oy.saturating_sub(2));
    let (bw, bh) = (pw + 4, ph + 4);
    fb.rect(bx, by, bw, 2, Ink::Black);
    fb.rect(bx, by + bh - 2, bw, 2, Ink::Black);
    fb.rect(bx, by, 2, bh, Ink::Black);
    fb.rect(bx + bw - 2, by, 2, bh, Ink::Black);

    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            let shade = sprite.shade(x, y);
            let (px, py) = (ox + x * scale, oy + y * scale);
            match look {
                Look::Ramp(_, inks) => fb.rect(px, py, scale, scale, inks[shade]),
                Look::Dither => {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            fb.set(px + dx, py + dy, dither_ink(shade, px + dx, py + dy));
                        }
                    }
                }
            }
        }
    }

    // --- caption column ---------------------------------------------------
    let cx = sw - MARGIN - CAPTION_W;
    let mut cy = oy;
    let mut ok = true;

    ok &= line(fb, cap.name, cx, cy, 3);
    cy += font::height(3) + 10;
    ok &= line(fb, cap.number, cx, cy, 3);
    cy += font::height(3) + 14;
    fb.rect(cx, cy, CAPTION_W, 2, Ink::Black);
    cy += 16;
    for text in cap.lines {
        ok &= line(fb, text, cx, cy, 2);
        cy += font::height(2) + 8;
    }
    ok &= line(fb, look.name(), cx, cy, 2);
    ok
}

/// One line of caption in black, reporting whether every glyph existed.
fn line(fb: &mut FrameBuffer<'_>, s: &str, x: usize, y: usize, scale: usize) -> bool {
    font::draw(s, x, y, scale, &mut |px, py| fb.set(px, py, Ink::Black))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gbscreen::geom::{Rotation, FB_BYTES};
    use gbscreen::LOOKS;

    /// Pack shade indices the way `src/sprite_host.rs` does, so the tests
    /// exercise the same layout the firmware reads.
    fn blob(w: usize, h: usize, shades: &[u8]) -> Vec<u8> {
        assert_eq!(shades.len(), w * h);
        let mut v = Vec::from(&MAGIC[..]);
        v.extend_from_slice(&(w as u16).to_le_bytes());
        v.extend_from_slice(&(h as u16).to_le_bytes());
        let mut packed = vec![0u8; Sprite::packed_len(w, h)];
        for (i, &s) in shades.iter().enumerate() {
            packed[i >> 2] |= (s & 0b11) << (6 - 2 * (i & 3));
        }
        v.extend_from_slice(&packed);
        v
    }

    fn peek(buf: &[u8; FB_BYTES], rot: Rotation, x: usize, y: usize) -> u8 {
        let (px, py) = rot.to_panel_pub(x, y);
        let byte = buf[py * gbscreen::geom::ROW_BYTES + px / 2];
        if px % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        }
    }

    const CAP: Caption<'static> = Caption {
        name: "CHARIZARD",
        number: "#006",
        lines: &["POKEMON BLUE", "FRONT SPRITE"],
    };

    #[test]
    fn the_packing_round_trips() {
        // Every shade, in an order that would survive a bit-order mistake in
        // only one arrangement.
        let shades: Vec<u8> = (0..4 * 4).map(|i| (i % 4) as u8).collect();
        let b = blob(4, 4, &shades);
        let s = Sprite::parse(&b).unwrap();
        assert_eq!((s.width(), s.height()), (4, 4));
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(s.shade(x, y), shades[y * 4 + x] as usize, "at {x},{y}");
            }
        }
    }

    #[test]
    fn a_blob_that_is_not_ours_is_refused_by_name() {
        assert!(Sprite::parse(b"").is_err());
        assert!(Sprite::parse(b"NOPE\x04\x00\x04\x00").is_err());
        // Right magic, wrong length.
        assert!(Sprite::parse(b"GBSP\x04\x00\x04\x00\x00").is_err());
        // Right magic, zero size.
        assert!(Sprite::parse(b"GBSP\x00\x00\x00\x00").is_err());
    }

    #[test]
    fn out_of_range_reads_as_the_page_not_as_ink() {
        // Shade 3 is lightest here. A sprite that read 0 outside itself would
        // draw a black border in every ramp.
        let b = blob(2, 2, &[0, 0, 0, 0]);
        let s = Sprite::parse(&b).unwrap();
        assert_eq!(s.shade(99, 0), 3);
        assert_eq!(s.shade(0, 99), 3);
    }

    #[test]
    fn the_picture_is_magnified_by_a_whole_number_and_fits() {
        // The real subject, on the real panel.
        let (x, y, scale) = layout(56, 56, 600, 400);
        assert_eq!(scale, 6, "56x56 on a 600x400 panel with a caption column");
        assert!(x >= MARGIN);
        assert!(x + 56 * scale <= 600 - MARGIN - CAPTION_W - GUTTER + GUTTER);
        assert!(y + 56 * scale <= 400);
        // Smaller species get more magnification, not more paper.
        assert_eq!(layout(40, 40, 600, 400).2, 8);
        assert_eq!(layout(48, 48, 600, 400).2, 7);
        // And a picture larger than the panel still gets a usable scale.
        assert_eq!(best_scale(1000, 1000, 600, 400), 1);
    }

    #[test]
    fn a_ramp_never_draws_a_darker_shade_lighter_than_a_lighter_one() {
        // gbscreen tests its own ramps; this asserts *this* module indexes them
        // the same way round. A reversed index inverts the whole picture and
        // nothing on the device could report it.
        for look in LOOKS {
            if let Look::Ramp(name, inks) = look {
                for i in 1..4 {
                    assert!(
                        gbscreen::ink_luma(inks[i]) > gbscreen::ink_luma(inks[i - 1]),
                        "{name}: shade {i} is not lighter than {}",
                        i - 1
                    );
                }
            }
        }
    }

    #[test]
    fn shade_zero_is_ink_and_shade_three_is_paper_in_every_look() {
        // The whole picture hangs on this: the sprite's own background is
        // shade 3, so if 3 did not come out white the page would be flooded.
        let mut buf = [0u8; FB_BYTES];
        for look in LOOKS {
            for (shade, want) in [(0usize, Ink::Black), (3, Ink::White)] {
                let b = blob(2, 2, &[shade as u8; 4]);
                let s = Sprite::parse(&b).unwrap();
                let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
                assert!(draw(&mut fb, &s, look, &CAP));
                let (x, y, scale) = layout(2, 2, fb.width(), fb.height());
                // Sample the middle of the picture, clear of the box.
                let got = peek(fb.raw(), Rotation::Landscape, x + scale, y + scale);
                assert_eq!(got, want as u8, "{} shade {shade}", look.name());
            }
        }
    }

    #[test]
    fn the_picture_is_boxed_so_a_white_background_still_has_an_edge() {
        let mut buf = [0u8; FB_BYTES];
        let b = blob(2, 2, &[3; 4]); // all page-coloured, the worst case
        let s = Sprite::parse(&b).unwrap();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        draw(&mut fb, &s, LOOKS[1], &CAP);
        let (x, y, _) = layout(2, 2, fb.width(), fb.height());
        assert_eq!(peek(fb.raw(), Rotation::Landscape, x - 1, y - 1), Ink::Black as u8);
    }

    #[test]
    fn the_caption_lands_beside_the_picture_and_never_on_it() {
        let mut buf = [0u8; FB_BYTES];
        let b = blob(56, 56, &[3; 56 * 56]);
        let s = Sprite::parse(&b).unwrap();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        assert!(draw(&mut fb, &s, LOOKS[1], &CAP), "every caption glyph exists");
        let (x, _, scale) = layout(56, 56, fb.width(), fb.height());
        let picture_right = x + 56 * scale + 2;
        let caption_left = fb.width() - MARGIN - CAPTION_W;
        assert!(
            caption_left > picture_right,
            "caption at {caption_left} overlaps picture ending at {picture_right}"
        );
        // And the longest line really fits the column it was given.
        for s in ["CHARIZARD", "#006"] {
            assert!(font::width(s, 3) <= CAPTION_W, "{s:?} at scale 3");
        }
        for s in CAP.lines {
            assert!(font::width(s, 2) <= CAPTION_W, "{s:?} at scale 2");
        }
        for look in LOOKS {
            assert!(font::width(look.name(), 2) <= CAPTION_W, "{:?}", look.name());
        }
    }

    #[test]
    fn a_caption_character_with_no_glyph_is_reported() {
        let mut buf = [0u8; FB_BYTES];
        let b = blob(2, 2, &[3; 4]);
        let s = Sprite::parse(&b).unwrap();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        let bad = Caption {
            name: "CHARIZARD+",
            number: "#006",
            lines: &[],
        };
        assert!(!draw(&mut fb, &s, LOOKS[1], &bad));
    }

    #[test]
    fn the_dither_gives_four_distinct_coverages() {
        // The reason this look exists at all: a panel with no grey producing
        // four apparent greys. If two shades covered the same fraction the
        // picture would lose a level and still look plausible.
        let mut seen = [0usize; 4];
        for (shade, slot) in seen.iter_mut().enumerate() {
            *slot = (0..8)
                .flat_map(|y| (0..8).map(move |x| (x, y)))
                .filter(|&(x, y)| dither_ink(shade, x, y) == Ink::White)
                .count();
        }
        assert_eq!(seen, [0, 24, 48, 64], "coverage per shade over one 8x8 cell");
        assert!(seen.windows(2).all(|w| w[0] < w[1]), "must be monotonic");
    }
}

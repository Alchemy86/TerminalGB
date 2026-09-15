//! Four Game Boy shades onto six inks that include no grey at all.
//!
//! The panel's whole palette is black, white, yellow, red, blue and green
//! (`geom::Ink`). A DMG draws in four greys. `docs/m5paper-first-boot.md` left
//! that mapping explicitly open — "it needs frames on the glass, not reasoning"
//! — so what this module does is refuse to guess: it carries five looks,
//! **button B cycles them on the device**, and the one that wins is the one a
//! human picks while looking at the paper.
//!
//! What is *not* left to taste is the part a wrong answer makes unreadable, and
//! those are tested rather than asserted, exactly as `geom.rs`'s rotation is:
//! every ink ramp is monotonic in luminance (a darker Game Boy shade must never
//! be drawn lighter than a lighter one), the picture lands centred and the
//! right way up, and it is boxed — the test card already learned that white on
//! white paper is invisible without a border.
//!
//! Like `geom.rs` this file carries no `esp-hal` dependency, so
//! `tools/m5paper-build.sh test` compiles the very code the firmware runs.

// In the firmware this is a sibling module; compiled standalone by
// `rustc --test` it needs its own copy, and `cfg(test)` is the only difference.
#[cfg(not(test))]
use crate::geom::{FrameBuffer, Ink};
#[cfg(test)]
#[path = "geom.rs"]
mod geom;
#[cfg(test)]
use geom::{FrameBuffer, Ink};

/// The Game Boy's own picture, in pixels.
pub const GB_W: usize = 160;
pub const GB_H: usize = 144;

/// **Two, and it is the largest whole number that fits.** The logical screen is
/// 600x400; 160x3 = 480 fits across but 144x3 = 432 does not fit down. A
/// fractional scale is not the answer either: a Game Boy draws its shading as a
/// one-pixel checkerboard, and resampling that at a non-integer ratio is moiré
/// — the same finding `chainrender`'s camera rule 8 records for the swarm
/// films. So the picture is 320x288 with paper around it.
pub const SCALE: usize = 2;

/// Where the picture's top-left corner sits on the logical screen.
pub const fn origin(screen_w: usize, screen_h: usize) -> (usize, usize) {
    ((screen_w - GB_W * SCALE) / 2, (screen_h - GB_H * SCALE) / 2)
}

/// How a Game Boy shade reaches the paper.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Look {
    /// One ink per shade, darkest first.
    Ramp(&'static str, [Ink; 4]),
    /// **No ink for the middle shades: a pattern instead.** At `SCALE` 2 every
    /// Game Boy pixel is a 2x2 block of panel pixels, which is exactly one
    /// dither cell, so a shade can be drawn as *n* white pixels of four and the
    /// eye does the mixing. It is the only look here that produces apparent
    /// greys on a panel that has none.
    Dither,
}

impl Look {
    pub const fn name(self) -> &'static str {
        match self {
            Look::Ramp(n, _) => n,
            Look::Dither => "dither",
        }
    }
}

/// How many of a 2x2 cell's four pixels are white, per Game Boy shade.
///
/// The DMG's own shades are `000000/606060/c0c0c0/ffffff`, i.e. 0 %, 37.6 %,
/// 75.3 % and 100 % white. Rounded onto quarters that is 0, 2, 3, 4 — the
/// closest four-level approximation this cell size can express.
pub const DITHER_WHITE: [usize; 4] = [0, 2, 3, 4];

/// Which pixels of the 2x2 cell go white first. Diagonal, so a half-tone reads
/// as texture rather than as stripes.
const DITHER_ORDER: [(usize, usize); 4] = [(0, 0), (1, 1), (1, 0), (0, 1)];

/// The looks button B cycles through, in order.
///
/// Every ramp is monotonic in luminance and starts at black and ends at white;
/// they differ in which two of the four remaining inks carry the middle shades,
/// which is exactly the question nobody can answer without looking at paper.
pub const LOOKS: [Look; 5] = [
    Look::Dither,
    Look::Ramp("red/green", [Ink::Black, Ink::Red, Ink::Green, Ink::White]),
    Look::Ramp(
        "green/yellow",
        [Ink::Black, Ink::Green, Ink::Yellow, Ink::White],
    ),
    Look::Ramp(
        "blue/green",
        [Ink::Black, Ink::Blue, Ink::Green, Ink::White],
    ),
    Look::Ramp(
        "red/yellow",
        [Ink::Black, Ink::Red, Ink::Yellow, Ink::White],
    ),
];

/// Rec. 601 luminance of an ink, for the monotonicity test and for `shade_of`.
/// The RGB values are the panel's own palette (M5GFX `epd_palette`).
pub const fn ink_luma(ink: Ink) -> u32 {
    let [r, g, b] = match ink {
        Ink::Black => [0u32, 0, 0],
        Ink::White => [255, 255, 255],
        Ink::Yellow => [255, 243, 56],
        Ink::Red => [191, 0, 0],
        Ink::Blue => [0, 0, 255],
        Ink::Green => [67, 138, 28],
    };
    (r * 299 + g * 587 + b * 114) / 1000
}

/// Which of the four shades a pixel of `Gameboy::image()` is, darkest first.
///
/// **By luminance rank, not by matching the four DMG greys**, which is the same
/// rule `tools/conformance/image.py` uses to compare our frames against other
/// people's: a Game Boy Color cartridge, or a palette key, produces colours
/// these thresholds still order sensibly, where an equality test would fall off
/// a cliff. On a DMG cartridge the four greys land on 0, 1, 2, 3 exactly.
#[inline]
pub fn shade_of(r: u8, g: u8, b: u8) -> usize {
    let luma = (r as u32 * 299 + g as u32 * 587 + b as u32 * 114) / 1000;
    // Midpoints of the DMG's own 0x00 / 0x60 / 0xC0 / 0xFF.
    match luma {
        0..=0x2F => 0,
        0x30..=0x8F => 1,
        0x90..=0xDF => 2,
        _ => 3,
    }
}

/// Paint one emulated frame onto the panel framebuffer.
///
/// `rgba` is `Gameboy::image()` — 160x144, four bytes a pixel. The surround is
/// white (the paper's own colour) with a black box around the picture, because
/// a mostly-white Game Boy screen has no visible edge otherwise; the test card
/// learned that the hard way and it is asserted below rather than assumed.
pub fn blit(fb: &mut FrameBuffer<'_>, rgba: &[u8], look: Look) {
    let (ox, oy) = origin(fb.width(), fb.height());
    fb.clear(Ink::White);

    // The box sits just outside the picture, so it costs the picture nothing.
    let (bx, by) = (ox - 1, oy - 1);
    let (bw, bh) = (GB_W * SCALE + 2, GB_H * SCALE + 2);
    fb.rect(bx, by, bw, 1, Ink::Black);
    fb.rect(bx, by + bh - 1, bw, 1, Ink::Black);
    fb.rect(bx, by, 1, bh, Ink::Black);
    fb.rect(bx + bw - 1, by, 1, bh, Ink::Black);

    for y in 0..GB_H {
        for x in 0..GB_W {
            let i = (y * GB_W + x) * 4;
            let shade = shade_of(rgba[i], rgba[i + 1], rgba[i + 2]);
            let px = ox + x * SCALE;
            let py = oy + y * SCALE;
            match look {
                Look::Ramp(_, inks) => {
                    fb.rect(px, py, SCALE, SCALE, inks[shade]);
                }
                Look::Dither => {
                    fb.rect(px, py, SCALE, SCALE, Ink::Black);
                    for &(dx, dy) in DITHER_ORDER.iter().take(DITHER_WHITE[shade]) {
                        fb.set(px + dx, py + dy, Ink::White);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geom::{Rotation, FB_BYTES};

    fn fb_of(rot: Rotation) -> ([u8; FB_BYTES], Rotation) {
        ([0u8; FB_BYTES], rot)
    }

    /// Read a logical pixel back out of the framebuffer.
    fn peek(buf: &[u8; FB_BYTES], rot: Rotation, x: usize, y: usize) -> u8 {
        let (px, py) = rot.to_panel_pub(x, y);
        let byte = buf[py * geom::ROW_BYTES + px / 2];
        if px % 2 == 0 {
            byte >> 4
        } else {
            byte & 0x0F
        }
    }

    /// A frame of one shade everywhere, as `Gameboy::image()` would give it.
    fn flat(shade: u8) -> Vec<u8> {
        let v = [0x00u8, 0x60, 0xC0, 0xFF][shade as usize];
        let mut f = vec![255u8; GB_W * GB_H * 4];
        for p in f.chunks_mut(4) {
            p[0] = v;
            p[1] = v;
            p[2] = v;
        }
        f
    }

    /// **The four DMG greys land on the four shades, in order.** Getting this
    /// wrong inverts or flattens the picture and nothing else would say so.
    #[test]
    fn the_dmg_greys_map_to_the_four_shades_in_order() {
        assert_eq!(shade_of(0x00, 0x00, 0x00), 0);
        assert_eq!(shade_of(0x60, 0x60, 0x60), 1);
        assert_eq!(shade_of(0xC0, 0xC0, 0xC0), 2);
        assert_eq!(shade_of(0xFF, 0xFF, 0xFF), 3);
    }

    /// The same four shades under the *other* common DMG palette (the green
    /// one the terminal frontend draws) must still rank the same way, because
    /// `shade_of` ranks luminance rather than matching greys.
    #[test]
    fn a_coloured_palette_still_ranks_darkest_to_lightest() {
        let green = [
            (0x08u8, 0x18u8, 0x20u8),
            (0x34, 0x68, 0x56),
            (0x88, 0xc0, 0x70),
            (0xe0, 0xf8, 0xd0),
        ];
        let mut last = None;
        for (i, (r, g, b)) in green.iter().enumerate() {
            let s = shade_of(*r, *g, *b);
            assert_eq!(s, i, "green palette entry {i} ranked {s}");
            if let Some(prev) = last {
                assert!(s > prev);
            }
            last = Some(s);
        }
    }

    /// **Every ramp must be monotonic in luminance.** A ramp that is not draws
    /// a darker Game Boy shade lighter than a lighter one, which inverts part
    /// of the picture — and on a six-ink panel that is very easy to do by
    /// accident, because hue is loud and luminance is not.
    #[test]
    fn every_ramp_is_monotonic_in_luminance() {
        for look in LOOKS {
            if let Look::Ramp(name, inks) = look {
                for w in inks.windows(2) {
                    assert!(
                        ink_luma(w[0]) < ink_luma(w[1]),
                        "{name}: {} ({}) is not darker than {} ({})",
                        w[0].name(),
                        ink_luma(w[0]),
                        w[1].name(),
                        ink_luma(w[1])
                    );
                }
                assert_eq!(inks[0], Ink::Black, "{name} must start at black");
                assert_eq!(inks[3], Ink::White, "{name} must end at white");
            }
        }
    }

    /// The dither cell must also be monotonic, and must use the whole cell at
    /// the ends — a "black" that is not all black has no true black in it.
    #[test]
    fn the_dither_cell_is_monotonic_and_reaches_both_ends() {
        assert_eq!(DITHER_WHITE[0], 0);
        assert_eq!(DITHER_WHITE[3], SCALE * SCALE);
        for w in DITHER_WHITE.windows(2) {
            assert!(w[0] < w[1], "dither levels must increase: {DITHER_WHITE:?}");
        }
        assert_eq!(DITHER_ORDER.len(), SCALE * SCALE);
    }

    /// The picture is centred and exactly `SCALE`x, in both axes.
    #[test]
    fn the_picture_is_centred_and_exactly_two_times() {
        let rot = Rotation::Landscape;
        let (mut buf, _) = fb_of(rot);
        {
            let mut fb = FrameBuffer::new(&mut buf, rot);
            blit(&mut fb, &flat(0), LOOKS[1]);
        }
        let (w, h) = (rot.width(), rot.height());
        let (ox, oy) = origin(w, h);
        assert_eq!((ox, oy), (140, 56), "600x400 minus 320x288, halved");

        // Inside is the picture, outside is paper.
        assert_eq!(peek(&buf, rot, ox, oy), Ink::Black as u8);
        assert_eq!(
            peek(&buf, rot, ox + GB_W * SCALE - 1, oy + GB_H * SCALE - 1),
            Ink::Black as u8
        );
        assert_eq!(peek(&buf, rot, ox - 2, oy - 2), Ink::White as u8);
        assert_eq!(peek(&buf, rot, ox + GB_W * SCALE + 1, oy), Ink::White as u8);
    }

    /// **The box is there.** White ink on white paper is invisible; the test
    /// card already found that, and a mostly-white Game Boy screen has the
    /// same problem at its edge.
    #[test]
    fn the_picture_is_boxed_so_a_white_screen_still_has_an_edge() {
        let rot = Rotation::Landscape;
        let (mut buf, _) = fb_of(rot);
        {
            let mut fb = FrameBuffer::new(&mut buf, rot);
            // The worst case: every pixel white.
            blit(&mut fb, &flat(3), LOOKS[1]);
        }
        let (ox, oy) = origin(rot.width(), rot.height());
        assert_eq!(
            peek(&buf, rot, ox - 1, oy - 1),
            Ink::Black as u8,
            "top-left corner of the box"
        );
        assert_eq!(
            peek(&buf, rot, ox + GB_W * SCALE, oy),
            Ink::Black as u8,
            "right edge"
        );
        assert_eq!(
            peek(&buf, rot, ox, oy + GB_H * SCALE),
            Ink::Black as u8,
            "bottom edge"
        );
        // ...and the picture itself really is white inside it.
        assert_eq!(peek(&buf, rot, ox + 10, oy + 10), Ink::White as u8);
    }

    /// **The picture is not transposed or flipped.** A Game Boy pixel at the
    /// top-left must reach the logical top-left of the screen. A transpose
    /// still looks like a picture, which is exactly why this is checked.
    #[test]
    fn the_top_left_game_boy_pixel_lands_top_left() {
        let rot = Rotation::Landscape;
        let (mut buf, _) = fb_of(rot);
        // White everywhere except one black pixel in the top-left corner, and
        // one mid-shade pixel one row down, so a flip in either axis shows.
        let mut frame = flat(3);
        for c in 0..4 {
            frame[c] = 0x00;
        }
        let i = (1 * GB_W) * 4;
        for c in 0..3 {
            frame[i + c] = 0x60;
        }
        {
            let mut fb = FrameBuffer::new(&mut buf, rot);
            blit(&mut fb, &frame, LOOKS[1]);
        }
        let (ox, oy) = origin(rot.width(), rot.height());
        assert_eq!(peek(&buf, rot, ox, oy), Ink::Black as u8, "GB (0,0)");
        assert_eq!(
            peek(&buf, rot, ox + 1, oy + 1),
            Ink::Black as u8,
            "GB (0,0) is 2x2"
        );
        assert_eq!(peek(&buf, rot, ox, oy + SCALE), Ink::Red as u8, "GB (0,1)");
        assert_eq!(
            peek(&buf, rot, ox + GB_W * SCALE - 1, oy),
            Ink::White as u8,
            "GB (159,0) — a horizontal flip would put the black pixel here"
        );
    }

    /// The dither look must actually produce four *distinguishable* tones over
    /// a cell, not four blocks that all read the same.
    #[test]
    fn the_dither_look_produces_four_distinct_tones() {
        let rot = Rotation::Landscape;
        let (ox, oy) = origin(rot.width(), rot.height());
        let mut whites = Vec::new();
        for shade in 0..4u8 {
            let (mut buf, _) = fb_of(rot);
            {
                let mut fb = FrameBuffer::new(&mut buf, rot);
                blit(&mut fb, &flat(shade), Look::Dither);
            }
            // Count the white pixels of one cell, well inside the picture.
            let mut n = 0;
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    if peek(&buf, rot, ox + 20 * SCALE + dx, oy + 20 * SCALE + dy)
                        == Ink::White as u8
                    {
                        n += 1;
                    }
                }
            }
            assert_eq!(n, DITHER_WHITE[shade as usize], "shade {shade}");
            whites.push(n);
        }
        assert_eq!(whites, vec![0, 2, 3, 4], "four distinct tones");
    }
}

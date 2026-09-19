//! Crisp half-block rendering — the picture for a terminal with no image
//! protocol at all.
//!
//! # Why this exists
//!
//! A terminal that speaks kitty, iTerm2 or sixel gets real pixels (see
//! [`crate::termimage`] and [`docs/rendering.md`]). A VTE-family terminal —
//! GNOME Terminal, GNOME Console, Ptyxis — speaks **none** of them (Fedora
//! builds VTE without sixel), so the picture has to be drawn out of text: the
//! upper-half-block glyph `▀`, foreground for the top half of the cell and
//! background for the bottom, giving two vertical "pixels" per character cell.
//!
//! That is crisp *by construction*: a cell is two flat colours, there is
//! nothing in it that can be soft. And yet the maintainer reported, twice, that
//! "running gameboy on our normal terminal is blurry again". The first report
//! was the desktop OpenGL frontend's `gl::LINEAR` texture filters
//! (`docs/display.md`). This one is here, and the cause is **the sampling, not
//! the drawing**: `ratatui-image`'s own half-block encoder hard-codes
//! `FilterType::Triangle` when it resizes the frame down onto the cell grid
//! (`protocol/halfblocks.rs`, `resize_exact(w, h * 2, FilterType::Triangle)`),
//! so every cell was a *weighted average* of the Game Boy pixels beneath it.
//! Measured on dmg-acid2 at 80×24: **271 distinct colours in a four-shade
//! picture**. Its sixel encoder does no such thing, and `Resize::Scale`'s own
//! default filter is `Nearest` — half-blocks is the one path with the filter
//! baked in, which is exactly why WezTerm looked right and Ptyxis did not.
//!
//! So we sample it ourselves, nearest-neighbour, straight from the framebuffer
//! into the `ratatui` buffer. Same move `crate::termimage` made for kitty and
//! iTerm2, for a related reason: the library's general-purpose answer is wrong
//! for a 60 Hz four-colour video source.
//!
//! # Rules this module keeps
//!
//! * **Nearest neighbour, sampled at pixel centres** — `(2i+1)·s / 2t`, the
//!   same rule as [`crate::video::fit_maps`], which the video crew adopted
//!   because edge sampling never reaches the last source column or row and
//!   quietly leans the whole picture towards the top-left.
//! * **A uniform cell is written as a space, not as `▀`.** Visually identical,
//!   but it asks the terminal to rasterise no glyph at all — so a font with a
//!   poor `▀` (or a font substitution for it) cannot soften the large flat
//!   areas a Game Boy picture is mostly made of.
//! * **We write into the `ratatui` buffer, never straight to the wire.**
//!   `ratatui` diffs per cell and emits only what changed, which is the
//!   cell-level differencing `docs/rendering.md` §2 identified as the whole
//!   value of a half-block path. Writing our own escapes would give up that
//!   diff and cost ~484 KB a frame.
//!
//! Nothing here allocates, and it is a single integer-arithmetic pass over the
//! cell grid — cheaper than the two `image`-crate resizes it replaces.

use ratatui::{buffer::Buffer, layout::Rect, style::Color};

/// The upper-half-block glyph. Foreground paints the top half of the cell,
/// background the bottom.
pub const UPPER_HALF: &str = "\u{2580}";

/// The other half of the same cell, needed only for a picture with
/// **transparency**: where the top sample is transparent and the bottom is
/// not, `▀` cannot express it (its foreground is the top half) and `▄` can.
///
/// The game picture is opaque everywhere, so this glyph appeared the day the
/// trainer panel's Pokémon — which have a flood-filled transparent background,
/// see `terminalgb_plugin_trainer::sprite::background_mask` — started being
/// drawn through this path.
pub const LOWER_HALF: &str = "\u{2584}";

/// A sample is transparent only when it is **fully** transparent.
///
/// There is no correct partial composite here: the terminal's own background
/// is not knowable, so blending towards it is a guess, and blending towards
/// black is right on a dark theme and backwards on a light one. So a partly
/// transparent pixel — the ghosting a fainted party member is composed with —
/// is drawn at full strength, and the fact that it has fainted is carried
/// where it always was: a hollow slot marker, an empty bar, and `FNT` beside
/// its name. That is the redundancy rule in `crate::screen::panel` doing its
/// job rather than a limitation being papered over.
#[inline]
fn opaque(a: u8) -> bool {
    a != 0
}

/// Nearest-neighbour sample index: the *centre* of output pixel `i` of `t`,
/// mapped into `s` source pixels.
#[inline]
fn sample(i: u32, s: u32, t: u32) -> u32 {
    debug_assert!(s > 0 && t > 0);
    (((2 * i as u64 + 1) * s as u64) / (2 * t as u64)).min(s as u64 - 1) as u32
}

/// Draw `rgba` (`src_w` × `src_h`, 4 bytes per pixel) over `rect` as
/// half-blocks.
///
/// Cells outside `buf`'s area are skipped. A zero-sized rect or an
/// undersized buffer is a no-op rather than a panic — this runs on every
/// displayed frame and a resize race must not take the frontend down.
pub fn render(rgba: &[u8], src_w: u32, src_h: u32, rect: Rect, buf: &mut Buffer) {
    if rect.width == 0 || rect.height == 0 || src_w == 0 || src_h == 0 {
        return;
    }
    if rgba.len() < (src_w as usize * src_h as usize * 4) {
        return;
    }
    let cols = rect.width as u32;
    // Two vertical samples per character cell — that is the whole trick.
    let rows2 = rect.height as u32 * 2;

    // The horizontal map is the same for every row, so compute it once. 512
    // entries covers a 4K-wide terminal; anything wider falls back to
    // computing per pixel rather than allocating on the hot path.
    let mut xmap = [0u32; 512];
    let cached = (cols as usize) <= xmap.len();
    if cached {
        for (x, slot) in xmap.iter_mut().enumerate().take(cols as usize) {
            *slot = sample(x as u32, src_w, cols);
        }
    }

    for cy in 0..rect.height {
        let top = sample(cy as u32 * 2, src_h, rows2);
        let bot = sample(cy as u32 * 2 + 1, src_h, rows2);
        let top_row = top as usize * src_w as usize * 4;
        let bot_row = bot as usize * src_w as usize * 4;
        for cx in 0..rect.width {
            let sx = if cached {
                xmap[cx as usize]
            } else {
                sample(cx as u32, src_w, cols)
            } as usize;
            let ti = top_row + sx * 4;
            let bi = bot_row + sx * 4;
            let up = opaque(rgba[ti + 3]);
            let lo = opaque(rgba[bi + 3]);
            let upper = Color::Rgb(rgba[ti], rgba[ti + 1], rgba[ti + 2]);
            let lower = Color::Rgb(rgba[bi], rgba[bi + 1], rgba[bi + 2]);
            let Some(cell) = buf.cell_mut((rect.x + cx, rect.y + cy)) else {
                continue;
            };
            cell.set_skip(false);
            // **A transparent sample shows the terminal's own background**,
            // which is `Color::Reset` — not black. Writing black would put an
            // opaque slab behind every Pokémon in the trainer panel's gutter,
            // and a black box is a worse answer than no picture at all.
            match (up, lo) {
                (false, false) => {
                    cell.set_char(' ').set_bg(Color::Reset).set_fg(Color::Reset);
                }
                (true, false) => {
                    cell.set_symbol(UPPER_HALF)
                        .set_fg(upper)
                        .set_bg(Color::Reset);
                }
                (false, true) => {
                    cell.set_symbol(LOWER_HALF)
                        .set_fg(lower)
                        .set_bg(Color::Reset);
                }
                (true, true) if upper == lower => {
                    // No glyph to rasterise: a flat cell is just a coloured
                    // space.
                    cell.set_char(' ').set_bg(lower).set_fg(lower);
                }
                (true, true) => {
                    cell.set_symbol(UPPER_HALF).set_fg(upper).set_bg(lower);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A framebuffer of `w`×`h` in which pixel (x, y) is shade `f(x, y)`, with
    /// the four shades spread far enough apart that any interpolation between
    /// two of them is unmistakable.
    fn frame(w: u32, h: u32, f: impl Fn(u32, u32) -> u8) -> Vec<u8> {
        let shades = [0u8, 85, 170, 255];
        let mut v = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                let s = shades[(f(x, y) & 3) as usize];
                v.extend_from_slice(&[s, s, s, 255]);
            }
        }
        v
    }

    fn colours(buf: &Buffer, rect: Rect) -> Vec<(u8, u8, u8)> {
        let mut seen = Vec::new();
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let cell = buf.cell((x, y)).unwrap();
                for c in [cell.fg, cell.bg] {
                    if let Color::Rgb(r, g, b) = c {
                        if !seen.contains(&(r, g, b)) {
                            seen.push((r, g, b));
                        }
                    }
                }
            }
        }
        seen
    }

    /// **The regression guard, at unit speed.** Every colour on screen must be
    /// a colour that was in the framebuffer. An interpolating resampler cannot
    /// pass this at any size: at 80×24 the filter this replaced produced 271
    /// distinct colours from four.
    #[test]
    fn no_colour_appears_that_was_not_in_the_framebuffer() {
        // A one-pixel checkerboard is the worst case for a smoothing filter:
        // every output sample straddles two shades.
        let src = frame(160, 144, |x, y| ((x + y) % 4) as u8);
        for (w, h) in [(80u16, 24u16), (40, 18), (213, 55), (160, 72), (7, 3)] {
            let rect = Rect::new(0, 0, w, h);
            let mut buf = Buffer::empty(rect);
            render(&src, 160, 144, rect, &mut buf);
            let seen = colours(&buf, rect);
            let allowed = [(0, 0, 0), (85, 85, 85), (170, 170, 170), (255, 255, 255)];
            for c in &seen {
                assert!(
                    allowed.contains(c),
                    "{w}x{h}: {c:?} is not one of the four shades in the \
                     framebuffer — the picture has been interpolated"
                );
            }
        }
    }

    /// An exact 1:1 grid (one cell column per source column, two cell rows per
    /// two source rows) must reproduce the framebuffer pixel for pixel. This
    /// is what says the *mapping* is right and not merely un-blurred.
    #[test]
    fn an_exact_fit_reproduces_the_framebuffer_pixel_for_pixel() {
        let src = frame(16, 8, |x, y| ((x + 2 * y) % 4) as u8);
        let rect = Rect::new(0, 0, 16, 4);
        let mut buf = Buffer::empty(rect);
        render(&src, 16, 8, rect, &mut buf);
        let shades = [0u8, 85, 170, 255];
        for y in 0..8u32 {
            for x in 0..16u32 {
                let want = shades[((x + 2 * y) % 4) as usize];
                let cell = buf.cell((x as u16, (y / 2) as u16)).unwrap();
                let got = if y % 2 == 0 { cell.fg } else { cell.bg };
                assert_eq!(
                    got,
                    Color::Rgb(want, want, want),
                    "pixel ({x},{y}) came out wrong"
                );
            }
        }
    }

    /// The picture must be sampled at pixel *centres*, not left edges:
    /// otherwise the last source column and row are never reached and the
    /// whole image leans towards the top-left. Same rule, same reason, as
    /// `video::fit_maps`. Mirroring the source must mirror the output.
    #[test]
    fn sampling_is_symmetric_so_no_edge_is_shaved_off() {
        let (sw, sh) = (160u32, 144u32);
        let src = frame(sw, sh, |x, _| (x % 4) as u8);
        let mirrored = frame(sw, sh, |x, _| ((sw - 1 - x) % 4) as u8);
        for (w, h) in [(37u16, 11u16), (100, 30), (160, 72)] {
            let rect = Rect::new(0, 0, w, h);
            let mut a = Buffer::empty(rect);
            let mut b = Buffer::empty(rect);
            render(&src, sw, sh, rect, &mut a);
            render(&mirrored, sw, sh, rect, &mut b);
            for y in 0..h {
                for x in 0..w {
                    // A column whose centre lands exactly on a source-pixel
                    // boundary is a tie, and `floor` has to pick a side; it
                    // cannot be symmetric and no sampler can make it so.
                    if (2 * x as u64 + 1) * sw as u64 % (2 * w as u64) == 0 {
                        continue;
                    }
                    let l = a.cell((x, y)).unwrap();
                    let r = b.cell((w - 1 - x, y)).unwrap();
                    assert_eq!(
                        (l.fg, l.bg),
                        (r.fg, r.bg),
                        "{w}x{h}: sampling is not symmetric at ({x},{y}) — an \
                         edge column is being shaved off"
                    );
                }
            }
        }
    }

    /// A flat cell asks the terminal to draw no glyph at all, so a font with a
    /// poor `▀` cannot soften the large uniform areas a Game Boy picture is
    /// mostly made of.
    #[test]
    fn a_uniform_cell_needs_no_glyph() {
        let src = frame(16, 8, |_, _| 0);
        let rect = Rect::new(0, 0, 16, 4);
        let mut buf = Buffer::empty(rect);
        render(&src, 16, 8, rect, &mut buf);
        for y in 0..4 {
            for x in 0..16 {
                let cell = buf.cell((x, y)).unwrap();
                assert_eq!(cell.symbol(), " ", "flat cell at ({x},{y}) drew a glyph");
                assert_eq!(cell.bg, Color::Rgb(0, 0, 0));
            }
        }
        // …and a cell whose halves differ still gets the half-block.
        let src = frame(16, 8, |_, y| (y % 2) as u8);
        let mut buf = Buffer::empty(rect);
        render(&src, 16, 8, rect, &mut buf);
        assert_eq!(buf.cell((0, 0)).unwrap().symbol(), UPPER_HALF);
    }

    /// A resize race must not take the frontend down.
    #[test]
    fn a_short_buffer_or_empty_rect_is_a_no_op() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 8, 4));
        render(&[], 160, 144, Rect::new(0, 0, 8, 4), &mut buf);
        render(
            &frame(4, 4, |_, _| 0),
            4,
            4,
            Rect::new(0, 0, 0, 0),
            &mut buf,
        );
        // A rect that runs off the end of the buffer clips instead of panicking.
        render(
            &frame(4, 4, |_, _| 0),
            4,
            4,
            Rect::new(6, 3, 8, 4),
            &mut buf,
        );
    }
}

//! Turning the machine's state into the 160x144 picture it would have drawn.
//!
//! This is a Game Boy picture unit doing only what this game asks of it: one
//! background map scrolled horizontally, a window pinned to the bottom for the
//! status bar, objects with both palettes and both flips — and the raster
//! split, which is the piece that does not survive being reimplemented as a
//! sprite or a tile animation.
//!
//! ## The raster split
//!
//! `SurfingPikachuMinigame_LoadGFXAndLayout` writes `rSCY - $ff00` into
//! `hLCDCPointer`, so the HBlank handler rewrites **SCY** — the vertical
//! scroll — once per scanline from `wLYOverrides`. The buffer holds a sine of
//! amplitude two, and `SurfingMinigame_UpdateLYOverrides` walks it up one
//! scanline per frame. The result is a shallow vertical ripple travelling up
//! the water. Reproduced here as a per-row Y offset, because there is no other
//! way to get it: every row of the picture samples the background from a
//! different place.

use crate::game::{Game, Popup, PopupKind, CENTER_X};
use crate::tiles::{TileSet, POSE_OFFSETS};

pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;
/// Bytes in one RGBA frame.
pub const FRAME_BYTES: usize = SCREEN_W * SCREEN_H * 4;

/// The four DMG shades, matching the emulator's own
/// `Gpu::get_monochrome_pal_val` so the port and a real ROM look alike.
const SHADES: [u8; 4] = [255, 192, 96, 0];

/// `PAL_PIKACHUS_BEACH` from pokeyellow's `CGBBasePalettes`
/// (`data/sgb/sgb_palettes.asm`), in the 5-bit components the Game Boy Color
/// stores: white, Pikachu yellow, sea blue, near-black.
const CGB_BEACH: [[u8; 3]; 4] = [[31, 31, 31], [31, 31, 0], [11, 23, 31], [3, 3, 3]];

/// Which machine's colours to draw with.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Look {
    /// A Game Boy Color running Yellow: `CGBBasePalettes` selected through the
    /// DMG palette registers, which is exactly what `DMGPalToCGBPal` does.
    #[default]
    Colour,
    /// An original Game Boy: four greys.
    Mono,
}

/// Which background palette register applies on this machine.
#[inline]
const fn bg_register(look: Look) -> u8 {
    match look {
        Look::Mono => BGP,
        Look::Colour => BGP_COLOUR,
    }
}

/// Widen a 5-bit Game Boy Color component to eight bits.
const fn expand5(v: u8) -> u8 {
    ((v as u16 * 255 + 15) / 31) as u8
}

/// The colour a tile's colour index becomes.
///
/// `DMGPalToCGBPal` is the whole rule: the DMG palette register picks a
/// *shade* for each colour index, and the shade indexes the base palette. On a
/// plain Game Boy the base palette is four greys, so the same expression
/// covers both machines.
#[inline]
fn colour(look: Look, palette: u8, index: u8) -> [u8; 3] {
    let shade = ((palette >> (2 * index)) & 3) as usize;
    match look {
        Look::Mono => {
            let v = SHADES[shade];
            [v, v, v]
        }
        Look::Colour => {
            let c = CGB_BEACH[shade];
            [expand5(c[0]), expand5(c[1]), expand5(c[2])]
        }
    }
}

/// `ldpal a, SHADE_BLACK, SHADE_LIGHT, SHADE_WHITE, SHADE_WHITE`. The macro is
/// `ld \1, \2 << 6 | \3 << 4 | \4 << 2 | \5`, so colour 3 is listed first and
/// this is `$d0`: indices 0 and 1 both white, 2 light, 3 black.
///
/// **This is the original Game Boy's palette and only that.** It is what makes
/// the sea a light grey on a DMG, and it is deliberately *not* applied to the
/// colour palette — see [`BGP_COLOUR`].
const BGP: u8 = 0xd0;

/// The background register a Game Boy Color effectively uses: the identity,
/// so colour index `i` takes base palette entry `i`.
///
/// Both palette tables are built for that. `PAL_PIKACHUS_BEACH` puts its blue
/// at **entry 2**, which is the colour index the water tiles are drawn in, and
/// the Super Game Boy branch of `SurfingPikachuMinigame_SetBGPals` — the one
/// place the game spells a background palette out for a colour machine — sets
/// `$e4`, the identity, and gets a blue sea from the same slot. Applying the
/// DMG `$d0` on top instead folds colour 2 onto entry 1 and turns the sea
/// yellow, which is the bug this constant fixes.
const BGP_COLOUR: u8 = 0xe4;
/// `ldpal a, SHADE_BLACK, SHADE_DARK, SHADE_LIGHT, SHADE_WHITE` = `$e4`.
const OBP0: u8 = 0xe4;
/// `ldpal a, SHADE_BLACK, SHADE_DARK, SHADE_WHITE, SHADE_WHITE` = `$e0`, the
/// foam palette: index 1 is white rather than light.
const OBP1: u8 = 0xe0;

/// `hWY` — the status bar window sits just below Pikachu's waterline.
const WINDOW_Y: usize = 0x7e;
/// Objects the hardware will draw on one scanline.
const SPRITES_PER_LINE: usize = 10;

/// One hardware object.
#[derive(Clone, Copy)]
struct Obj {
    /// OAM Y: screen Y plus 16.
    y: u8,
    /// OAM X: screen X plus 8.
    x: u8,
    tile: u8,
    pal1: bool,
    xflip: bool,
    yflip: bool,
}

/// An entry in one of `SurfingPikachuOAMData`'s shapes: offset from the
/// object's origin, the tile to add to the frame's tile offset, and whether it
/// uses the foam palette.
type Shape = &'static [(i8, i8, u8, bool)];

/// `.SurfingPikachu` — three tiles by three, centred on the origin.
#[rustfmt::skip]
const PIKACHU_SHAPE: Shape = &[
    (-12, -12, 0x00, false), (-12, -4, 0x01, false), (-12, 4, 0x02, false),
    ( -4, -12, 0x10, false), ( -4, -4, 0x11, false), ( -4, 4, 0x12, false),
    (  4, -12, 0x20, false), (  4, -4, 0x21, false), (  4, 4, 0x22, false),
];

/// `.EmptySurfboard` — what is left floating after a wipeout.
#[rustfmt::skip]
const BOARD_SHAPE: Shape = &[(4, -12, 0x00, false), (4, -4, 0x01, false), (4, 4, 0x02, false)];

/// `.WaterSpray`.
#[rustfmt::skip]
const SPRAY_SHAPE: Shape = &[(-4, 11, 0x00, true), (4, 3, 0x0f, true), (4, 11, 0x10, true)];

/// `.LargeSplash`.
#[rustfmt::skip]
const SPLASH_SHAPE: Shape = &[
    (-12, -16, 0x00, true), (-12, -8, 0x01, true), (-12, 0, 0x01, true), (-12, 8, 0x00, true),
    ( -4, -16, 0x10, true), ( -4, -8, 0x11, true), ( -4, 0, 0x11, true), ( -4, 8, 0x10, true),
    (  4, -16, 0x20, true), (  4, -8, 0x21, true), (  4, 0, 0x21, true), (  4, 8, 0x20, true),
];

/// `.StartText` / `.OhNoText` — six tiles by two.
#[rustfmt::skip]
const BANNER_SHAPE: Shape = &[
    (-8, -24, 0x00, false), (-8, -16, 0x01, false), (-8, -8, 0x02, false),
    (-8,   0, 0x03, false), (-8,   8, 0x04, false), (-8, 16, 0x05, false),
    ( 0, -24, 0x10, false), ( 0, -16, 0x11, false), ( 0,  -8, 0x12, false),
    ( 0,   0, 0x13, false), ( 0,   8, 0x14, false), ( 0,  16, 0x15, false),
];

/// The window map: `SurfingPikachuMinigame_DrawStaticTilemapLayout` writing
/// into `vBGMap1`. Twenty columns is all that is ever visible.
fn window_map() -> [[u8; 20]; 3] {
    let mut m = [[0u8; 20]; 3];
    m[0][1] = 0x15;
    m[0][2] = 0x16;
    let bar = [0x17u8, 0x18, 0x19, 0x19, 0x19, 0x19, 0x19, 0x19, 0x19];
    m[1][1..10].copy_from_slice(&bar);
    m[1][12] = 0x1b;
    m[1][13] = 0x1c;
    m
}

impl Game {
    /// Draw one frame as RGBA into `out`, which must be [`FRAME_BYTES`] long.
    /// Uses the Game Boy Color palette, which is what Yellow shows.
    pub fn render(&self, tiles: &TileSet, out: &mut [u8]) {
        self.render_as(tiles, Look::Colour, out)
    }

    /// Draw one frame with a chosen machine's colours.
    pub fn render_as(&self, tiles: &TileSet, look: Look, out: &mut [u8]) {
        if out.len() < FRAME_BYTES {
            return;
        }
        self.draw_background(tiles, look, out);
        self.draw_objects(tiles, look, out);
    }

    fn draw_background(&self, tiles: &TileSet, look: Look, out: &mut [u8]) {
        let window = window_map();
        for y in 0..SCREEN_H {
            // The raster split: this scanline's own SCY.
            let scy = if self.ly_enabled { self.ly[y] } else { 0 };
            let in_window = y >= WINDOW_Y;
            let window_row = (y - WINDOW_Y.min(y)) / 8;
            for x in 0..SCREEN_W {
                let index = if in_window {
                    let id = window.get(window_row).map_or(0, |r| r[x / 8]);
                    tiles.bg[id as usize][(y - WINDOW_Y) % 8 * 8 + x % 8]
                } else if let Some(map) = self.results_map.as_ref() {
                    // The results screen replaces the course outright, drawn
                    // in screen space with no scroll of either kind.
                    let id = map[(y / 8) * 20 + x / 8];
                    tiles.bg[id as usize][(y % 8) * 8 + x % 8]
                } else {
                    let by = (y as u8).wrapping_add(scy);
                    let bx = (x as u8).wrapping_add(self.scx);
                    let id = self.bgmap[(by as usize / 8) * 32 + bx as usize / 8];
                    tiles.bg[id as usize][(by as usize % 8) * 8 + bx as usize % 8]
                };
                let [r, g, b] = colour(look, bg_register(look), index);
                let p = (y * SCREEN_W + x) * 4;
                out[p] = r;
                out[p + 1] = g;
                out[p + 2] = b;
                out[p + 3] = 0xff;
            }
        }
    }

    /// Build the object list in OAM order, exactly as
    /// `SurfingPikachuMinigame_InitStaticSpriteLayout` lays it out: four HP
    /// digits, the progress marker, nine clouds, then everything spawned.
    fn objects(&self) -> Vec<Obj> {
        let mut objs: Vec<Obj> = Vec::with_capacity(40);
        let digit = |b: u8, hi: bool| 0xd0 + if hi { b >> 4 } else { b & 0x0f };
        for (i, tile) in [
            digit(self.hp[1], true),
            digit(self.hp[1], false),
            digit(self.hp[0], true),
            digit(self.hp[0], false),
        ]
        .into_iter()
        .enumerate()
        {
            objs.push(Obj {
                y: 0x97,
                x: 0x80 + (i as u8) * 8,
                tile,
                pal1: false,
                xflip: false,
                yflip: false,
            });
        }
        objs.push(Obj {
            y: 0x96,
            x: self.progress_marker_x,
            tile: 0xfe,
            pal1: false,
            xflip: false,
            yflip: false,
        });
        let cloud_tiles = [0xec, 0xed, 0xed, 0xee, 0xef, 0xec, 0xed, 0xee, 0xef];
        for (i, tile) in cloud_tiles.into_iter().enumerate() {
            if self.clouds[i] == 0 {
                continue;
            }
            objs.push(Obj {
                y: if i < 5 { 0x14 } else { 0x20 },
                x: self.clouds[i],
                tile,
                pal1: false,
                xflip: false,
                yflip: false,
            });
        }

        // Pikachu. Frame sets 8..=14 are 1..=7 flipped both ways, which is
        // the original's way of drawing the board upside down.
        let set = self.pika.frame_set;
        let y = self.pika.y.wrapping_add(self.pika.y_offset);
        match set {
            1..=14 => {
                let pose = ((set - 1) % 7) as usize;
                let flipped = set > 7;
                // Two animation frames, eight frames each.
                let (a, b) = POSE_OFFSETS[pose];
                let base = if (self.frame / 8) % 2 == 0 { a } else { b };
                push_shape(
                    &mut objs,
                    PIKACHU_SHAPE,
                    y,
                    CENTER_X,
                    base,
                    flipped,
                    flipped,
                );
            }
            0x0f => push_shape(&mut objs, PIKACHU_SHAPE, y, CENTER_X, 0xa0, false, false),
            0x10 => push_shape(&mut objs, BOARD_SHAPE, y, CENTER_X, 0x98, false, false),
            _ => {}
        }

        for p in &self.popups {
            self.push_popup(&mut objs, p);
        }
        objs
    }

    fn push_popup(&self, objs: &mut Vec<Obj>, p: &Popup) {
        match p.kind {
            PopupKind::Spray => {
                push_shape(objs, SPRAY_SHAPE, p.y, p.x, 0xa7, false, false)
            }
            PopupKind::Splash => {
                push_shape(objs, SPLASH_SHAPE, p.y, p.x, 0xa8, false, false)
            }
            PopupKind::Start => {
                push_shape(objs, BANNER_SHAPE, p.y, p.x, 0xe0, false, false)
            }
            PopupKind::OhNo => {
                // `y` carries the bounce offset; the text itself sits at $88.
                let y = 0x88u8.wrapping_add(p.y);
                push_shape(objs, BANNER_SHAPE, y, p.x, 0xca, false, false)
            }
            PopupKind::Score(value) => {
                let tiles: &[u8] = match value {
                    50 => &[0xbf, 0xd5, 0xd0],
                    150 => &[0xbf, 0xd1, 0xd5, 0xd0],
                    350 => &[0xbf, 0xd3, 0xd5, 0xd0],
                    180 => &[0xbf, 0xd1, 0xd8, 0xd0],
                    _ => &[0xbf, 0xd5, 0xd0, 0xd0],
                };
                let left = -(tiles.len() as i32 * 8 / 2) as i8;
                for (i, t) in tiles.iter().enumerate() {
                    objs.push(Obj {
                        y: p.y.wrapping_sub(4),
                        x: p.x.wrapping_add_signed(left + i as i8 * 8),
                        tile: *t,
                        pal1: false,
                        xflip: false,
                        yflip: false,
                    });
                }
            }
        }
    }

    fn draw_objects(&self, tiles: &TileSet, look: Look, out: &mut [u8]) {
        let objs = self.objects();
        for y in 0..SCREEN_H {
            // Ten per line, taken in OAM order; then the lowest X wins, and
            // ties go to the lowest OAM index — the DMG rule.
            let mut line: Vec<(usize, &Obj)> = objs
                .iter()
                .enumerate()
                .filter(|(_, o)| {
                    let top = o.y as i32 - 16;
                    (top..top + 8).contains(&(y as i32))
                })
                .take(SPRITES_PER_LINE)
                .collect();
            line.sort_by_key(|(i, o)| (std::cmp::Reverse(o.x), std::cmp::Reverse(*i)));
            for (_, o) in line {
                let ty = (y as i32 - (o.y as i32 - 16)) as usize;
                let row = if o.yflip { 7 - ty } else { ty };
                let tile = &tiles.obj[o.tile as usize];
                for tx in 0..8usize {
                    let sx = o.x as i32 - 8 + tx as i32;
                    if !(0..SCREEN_W as i32).contains(&sx) {
                        continue;
                    }
                    let col = if o.xflip { 7 - tx } else { tx };
                    let index = tile[row * 8 + col];
                    if index == 0 {
                        continue; // transparent
                    }
                    let [r, g, b] = colour(look, if o.pal1 { OBP1 } else { OBP0 }, index);
                    let p = (y * SCREEN_W + sx as usize) * 4;
                    out[p] = r;
                    out[p + 1] = g;
                    out[p + 2] = b;
                }
            }
        }
    }
}

fn push_shape(
    objs: &mut Vec<Obj>,
    shape: Shape,
    y: u8,
    x: u8,
    base: u8,
    xflip: bool,
    yflip: bool,
) {
    for (dy, dx, tile, pal1) in shape {
        // Flipping an object flips the arrangement as well as each tile, so
        // the three-wide block maps onto itself rather than sliding sideways.
        let ddx = if xflip { -dx - 8 } else { *dx };
        let ddy = if yflip { -dy - 8 } else { *dy };
        objs.push(Obj {
            y: y.wrapping_add_signed(ddy),
            x: x.wrapping_add_signed(ddx),
            tile: base.wrapping_add(*tile),
            pal1: *pal1,
            xflip,
            yflip,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::Input;

    #[test]
    fn palette_bytes_decode_to_the_shades_the_macro_spelled() {
        let grey = |p, i| colour(Look::Mono, p, i)[0];
        // BGP $d0: white, white, light, black.
        assert_eq!([0, 1, 2, 3].map(|i| grey(BGP, i)), [255, 255, 192, 0]);
        // OBP0 $e4 is the normal ramp; OBP1 $e0 makes index 1 white for foam.
        assert_eq!([0, 1, 2, 3].map(|i| grey(OBP0, i)), [255, 192, 96, 0]);
        assert_eq!([0, 1, 2, 3].map(|i| grey(OBP1, i)), [255, 255, 96, 0]);
    }

    #[test]
    fn the_colour_palette_puts_yellow_on_pikachu_and_blue_on_his_board() {
        // Object colour 1 is Pikachu's body and colour 2 his board; OBP0 is
        // the identity, so they land on base entries 1 and 2.
        assert_eq!(colour(Look::Colour, OBP0, 1), [255, 255, 0]);
        assert_eq!(colour(Look::Colour, OBP0, 2), [90, 189, 255]);
        // Foam takes OBP1, where colour 1 is white rather than yellow. That
        // remap is real and must survive.
        assert_eq!(colour(Look::Colour, OBP1, 1), [255, 255, 255]);
    }

    /// The bug a player found by looking at it: the sea came out yellow.
    /// Background colour 2 is the water, and in colour it must reach the
    /// palette's blue rather than being folded onto Pikachu's yellow.
    #[test]
    fn in_colour_the_sky_is_white_and_the_sea_is_blue() {
        let bg = |i| colour(Look::Colour, bg_register(Look::Colour), i);
        assert_eq!(bg(0), [255, 255, 255], "sky must be white");
        assert_eq!(bg(2), [90, 189, 255], "the sea must be blue");
        assert_eq!(bg(3), [25, 25, 25], "outlines must be near-black");
        assert_ne!(
            bg(2),
            colour(Look::Colour, OBP0, 1),
            "sea must not be Pikachu's yellow"
        );
    }

    /// The original Game Boy keeps its own register: there the sea is a light
    /// grey, and folding colour 2 onto colour 1's shade is correct.
    #[test]
    fn in_mono_the_background_keeps_the_dmg_register() {
        let bg = |i| colour(Look::Mono, bg_register(Look::Mono), i);
        assert_eq!(bg(0), [255, 255, 255]);
        assert_eq!(bg(1), [255, 255, 255]);
        assert_eq!(bg(2), [192, 192, 192]);
        assert_eq!(bg(3), [0, 0, 0]);
    }

    #[test]
    fn five_bit_components_widen_to_the_full_range() {
        assert_eq!(expand5(0), 0);
        assert_eq!(expand5(31), 255);
    }

    #[test]
    fn a_frame_renders_without_panicking_and_is_not_blank() {
        let tiles = TileSet::built_in();
        let mut g = Game::new(1);
        let mut buf = vec![0u8; FRAME_BYTES];
        for _ in 0..200 {
            g.step(Input::default());
        }
        g.render(&tiles, &mut buf);
        let distinct: std::collections::BTreeSet<u8> =
            buf.iter().step_by(4).copied().collect();
        assert!(
            distinct.len() >= 3,
            "picture has only {} shades",
            distinct.len()
        );
    }

    #[test]
    fn the_raster_split_makes_rows_differ() {
        // If the per-scanline SCY were dropped, two rows sampling the same
        // tile row would be identical. The whole point is that they are not.
        let tiles = TileSet::built_in();
        let mut g = Game::new(9);
        for _ in 0..600 {
            g.step(Input::default());
        }
        let mut with = vec![0u8; FRAME_BYTES];
        g.render(&tiles, &mut with);
        let mut without = vec![0u8; FRAME_BYTES];
        g.ly_enabled = false;
        g.render(&tiles, &mut without);
        assert_ne!(with, without, "disabling the raster split changed nothing");
    }

    #[test]
    fn flipping_a_shape_maps_the_block_onto_itself() {
        let mut a = Vec::new();
        push_shape(&mut a, PIKACHU_SHAPE, 100, 100, 0, false, false);
        let mut b = Vec::new();
        push_shape(&mut b, PIKACHU_SHAPE, 100, 100, 0, true, true);
        let mut xa: Vec<u8> = a.iter().map(|o| o.x).collect();
        let mut xb: Vec<u8> = b.iter().map(|o| o.x).collect();
        xa.sort_unstable();
        xb.sort_unstable();
        assert_eq!(xa, xb, "a flipped sprite must cover the same pixels");
    }
}

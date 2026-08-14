//! Gen 1 Pokémon picture decompression.
//!
//! # Why the ROM and not VRAM
//!
//! There are two honest places a Pokémon sprite can come from:
//!
//! * **VRAM.** Trivial — the tiles are already decoded, and whatever is on
//!   screen is by definition correct. But VRAM only holds what is *on screen*.
//!   In the overworld that is the map and the player, not a single party
//!   Pokémon; in a battle it is the two combatants and nothing else. A party
//!   panel showing six Pokémon would be blank almost all the time, which is not
//!   the feature that was asked for.
//! * **The cartridge.** Every Pokémon, at any moment, in or out of battle — at
//!   the cost of implementing the compression Gen 1 stores its pictures in.
//!
//! This module takes the second. It is the only reason the panel can show a
//! party sprite while the player is walking around, and it is the difference
//! between the view being useful and being decorative. The cost is one
//! decompression per Pokémon *when the party changes*, not per frame; see
//! [`super::TrainerPlugin`].
//!
//! # The format
//!
//! Reimplemented from the format as documented by the
//! [`pret/pokered`](https://github.com/pret/pokered) disassembly — the
//! decompressor `UncompressSpriteData` in `home/uncompress.asm`, the buffer
//! layout in `home/pics.asm`, and `tools/pkmncompress.c` which is the same
//! algorithm stated in C. The two nybble tables below are the game's own data
//! (`DecodeNybble0Table`/`DecodeNybble1Table`), not the tool's.
//!
//! A picture is a square of `W`×`W` 8×8 tiles (`W` ≤ 7) and decodes like this:
//!
//! 1. **Header.** Byte 0 is `(W << 4) | W`. Then one bit says which of the two
//!    bitplanes is transmitted first.
//! 2. **Two bitplanes**, each a run-length-coded stream of 2-bit groups that
//!    alternates between *data* runs (2-bit groups until a `00`) and *zero*
//!    runs (a unary length-of-length, then that many bits, biased by
//!    `2^(n+1) - 1` so every run length has exactly one encoding). Between the
//!    two planes sits the 1- or 2-bit unpacking mode.
//! 3. **Delta decoding.** Each plane is stored as a run-length-friendly
//!    "changes only" transform: reading left to right along a pixel row, a bit
//!    means *toggle*, not *set*. Undone a nybble at a time through the tables.
//! 4. **Mode.** Mode 0 delta-decodes both planes. Mode 1 delta-decodes one and
//!    XORs it into the other. Mode 2 delta-decodes both, then XORs. The encoder
//!    tries all of them and keeps whichever came out smallest, which is why a
//!    decoder has to implement all three.
//!
//! The bit stream is MSB-first within each byte.
//!
//! Correctness is not argued, it is *checked*: `testharness/trainer.rs`
//! decompresses the cartridge's own picture data and compares it **byte for
//! byte** against the 2bpp tiles the disassembly itself produces, which is the
//! exact oracle — the compressed data in the ROM was produced *from* those
//! bytes. Thirteen species covering every "Pics" bank and both special-cased
//! ones match. The reference graphics are supplied at run time via
//! `POKERED_GFX=<dir>` and are never committed here; see the licence boundary
//! in `AGENTS.md`.

/// A decompressed picture: 2bpp tiles in the Game Boy's own format, row-major,
/// 16 bytes per tile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pic {
    pub tiles: Vec<u8>,
    /// Side of the square, in 8×8 tiles.
    pub width_tiles: usize,
}

impl Pic {
    pub fn width_px(&self) -> usize {
        self.width_tiles * 8
    }

    pub fn height_px(&self) -> usize {
        self.width_tiles * 8
    }
}

/// The delta-decoding tables, `DecodeNybble0Table` and `DecodeNybble1Table` in
/// `home/uncompress.asm`. Which one applies depends on the last bit decoded, so
/// a run of zero bits keeps the current colour rather than clearing it.
const DECODE: [[u8; 16]; 2] = [
    [
        0x0, 0x1, 0x3, 0x2, 0x7, 0x6, 0x4, 0x5, 0xF, 0xE, 0xC, 0xD, 0x8, 0x9, 0xB, 0xA,
    ],
    [
        0xF, 0xE, 0xC, 0xD, 0x8, 0x9, 0xB, 0xA, 0x0, 0x1, 0x3, 0x2, 0x7, 0x6, 0x4, 0x5,
    ],
];

/// Largest picture the format can express, and the largest any Pokémon uses.
const MAX_TILES: usize = 7;

/// MSB-first bit reader over the compressed stream.
struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Bits<'a> {
        Bits { data, pos: 0 }
    }

    /// One bit, or `None` past the end of the data (a truncated ROM image
    /// aborts the decode rather than reading zeroes forever).
    fn bit(&mut self) -> Option<u8> {
        let byte = *self.data.get(self.pos / 8)?;
        let bit = (byte >> (7 - self.pos % 8)) & 1;
        self.pos += 1;
        Some(bit)
    }

    fn bits(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit()? as u32;
        }
        Some(v)
    }
}

/// Decompress a picture from the start of `data`.
///
/// `data` may run past the end of the picture — the stream is self-terminating,
/// so callers can hand over the rest of the ROM bank.
pub fn decompress(data: &[u8]) -> Option<Pic> {
    let mut bits = Bits::new(data);
    let width = bits.bits(4)? as usize;
    let height = bits.bits(4)? as usize;
    // Every Pokémon picture is square; the format cannot express a rectangle
    // (the compressor rejects one outright), so a mismatch means this is not
    // picture data.
    if width != height || width == 0 || width > MAX_TILES {
        return None;
    }

    let plane_len = width * width * 8;
    let order = bits.bit()? as usize;

    let mut planes = [vec![0u8; plane_len], vec![0u8; plane_len]];
    planes[order] = fill_plane(&mut bits, width)?;
    let mode = match bits.bit()? {
        0 => 0,
        _ => 1 + bits.bit()? as usize,
    };
    planes[order ^ 1] = fill_plane(&mut bits, width)?;

    delta_decode(&mut planes[order], width);
    if mode != 1 {
        delta_decode(&mut planes[order ^ 1], width);
    }
    if mode != 0 {
        for i in 0..plane_len {
            planes[order ^ 1][i] ^= planes[order][i];
        }
    }

    // Interleave into 2bpp: plane 0 is the low bit of each pixel, plane 1 the
    // high bit, one byte of each per row.
    let mut tiles = vec![0u8; plane_len * 2];
    for i in 0..plane_len {
        tiles[i * 2] = planes[0][i];
        tiles[i * 2 + 1] = planes[1][i];
    }
    transpose_tiles(&mut tiles, width);
    Some(Pic {
        tiles,
        width_tiles: width,
    })
}

/// Decode one run-length-coded bitplane into packed bytes.
///
/// The stream alternates between a zero-run and a data-run, starting with
/// whichever the first bit selects. The result is column-major: byte
/// `col * width * 8 + row` covers the 8 pixels of tile-column `col` on pixel
/// row `row`, which is how the game's own sprite buffer is laid out.
fn fill_plane(bits: &mut Bits<'_>, width: usize) -> Option<Vec<u8>> {
    let groups_len = width * width * 32;
    let mut groups = Vec::with_capacity(groups_len);
    let mut data_run = bits.bit()? == 1;

    while groups.len() < groups_len {
        if data_run {
            while groups.len() < groups_len {
                let g = bits.bits(2)? as u8;
                if g == 0 {
                    break;
                }
                groups.push(g);
            }
        } else {
            // Unary length-of-length, then that many + 1 bits, biased so every
            // run length is uniquely representable.
            let mut n = 0usize;
            while bits.bit()? == 1 {
                n += 1;
                if n >= 16 {
                    return None;
                }
            }
            let bias = (1u32 << (n + 1)) - 1;
            let run = bias as usize + bits.bits(n + 1)? as usize;
            let take = run.min(groups_len - groups.len());
            groups.resize(groups.len() + take, 0);
        }
        data_run = !data_run;
    }

    // Pack: four 2-bit groups per byte, most significant pair first. The
    // groups arrive column by column, four passes down each column (one per
    // bit-pair position), which is the reindex below.
    let stride = width * 8;
    let mut plane = vec![0u8; width * width * 8];
    for col in 0..width {
        for row in 0..stride {
            let mut byte = 0u8;
            for pass in 0..4 {
                byte = (byte << 2) | groups[(col * 4 + pass) * stride + row];
            }
            plane[col * stride + row] = byte;
        }
    }
    Some(plane)
}

/// Undo the delta encoding in place.
///
/// Walks each pixel row left to right across the tile columns, carrying the
/// last decoded bit so a zero means "same as the pixel to the left".
fn delta_decode(plane: &mut [u8], width: usize) {
    let stride = width * 8;
    for row in 0..stride {
        let mut last = 0usize;
        for col in 0..width {
            let i = col * stride + row;
            let hi = DECODE[last][(plane[i] >> 4) as usize];
            last = (hi & 1) as usize;
            let lo = DECODE[last][(plane[i] & 0x0F) as usize];
            last = (lo & 1) as usize;
            plane[i] = (hi << 4) | lo;
        }
    }
}

/// Transpose the tile order between column-major (how the picture is stored and
/// decoded) and row-major (how a Game Boy tile sheet is laid out).
fn transpose_tiles(tiles: &mut [u8], width: usize) {
    let count = width * width;
    for i in 0..count {
        let j = (i * width + i / width) % count;
        if i < j {
            for b in 0..16 {
                tiles.swap(i * 16 + b, j * 16 + b);
            }
        }
    }
}

/// Expand 2bpp tiles into one palette index (0..=3) per pixel, in reading
/// order, so a renderer can map them straight through a palette.
pub fn tiles_to_indices(pic: &Pic) -> (Vec<u8>, usize, usize) {
    let w = pic.width_px();
    let h = pic.height_px();
    let mut out = vec![0u8; w * h];
    for ty in 0..pic.width_tiles {
        for tx in 0..pic.width_tiles {
            let tile = (ty * pic.width_tiles + tx) * 16;
            for y in 0..8 {
                let lo = pic.tiles.get(tile + y * 2).copied().unwrap_or(0);
                let hi = pic.tiles.get(tile + y * 2 + 1).copied().unwrap_or(0);
                for x in 0..8 {
                    let bit = 7 - x;
                    let v = ((hi >> bit) & 1) << 1 | ((lo >> bit) & 1);
                    out[(ty * 8 + y) * w + tx * 8 + x] = v;
                }
            }
        }
    }
    (out, w, h)
}

/// The palette index Gen 1 art uses for the page the Pokémon is drawn on: 0,
/// the lightest of the four shades.
pub const BACKGROUND_INDEX: u8 = 0;

/// Which pixels are *background* — the sheet the artwork sits on — rather than
/// part of the Pokémon.
///
/// # Why this is a flood fill and not a colour key
///
/// The obvious implementation is "every lightest-shade pixel is background",
/// and it is wrong in a way that looks broken rather than subtly off: Gen 1 art
/// uses the lightest shade *inside* the figure for eyes, teeth, highlights,
/// Charmander's belly and the core of its flame. Keying on the colour punches
/// holes straight through the middle of the Pokémon.
///
/// So: a four-way flood fill from the border over lightest-shade pixels only.
/// Anything the fill reaches is outside the artwork; a light region enclosed by
/// darker pixels is never reached and stays opaque. This is the same reasoning
/// a paint program's "magic wand" uses, and it is the only rule that is right
/// for both a Pokémon standing on its baseline and one whose outline touches
/// several edges of the frame.
///
/// Costs one pass over `w * h` bytes — about 3 KB for the largest Gen 1 pic —
/// and runs once per species, behind the same cache the decompression is.
pub fn background_mask(pixels: &[u8], w: usize, h: usize) -> Vec<bool> {
    let mut bg = vec![false; w * h];
    if w == 0 || h == 0 || pixels.len() < w * h {
        return bg;
    }
    let mut stack: Vec<usize> = Vec::with_capacity(w * 2 + h * 2);
    let seed = |i: usize, stack: &mut Vec<usize>, bg: &mut Vec<bool>| {
        if pixels[i] == BACKGROUND_INDEX && !bg[i] {
            bg[i] = true;
            stack.push(i);
        }
    };
    for x in 0..w {
        seed(x, &mut stack, &mut bg);
        seed((h - 1) * w + x, &mut stack, &mut bg);
    }
    for y in 0..h {
        seed(y * w, &mut stack, &mut bg);
        seed(y * w + w - 1, &mut stack, &mut bg);
    }
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        // Four-connected, not eight: a diagonal leak would let the fill squeeze
        // between two touching outline pixels and eat the artwork behind them.
        let visit = |nx: usize, ny: usize, stack: &mut Vec<usize>, bg: &mut Vec<bool>| {
            let j = ny * w + nx;
            if pixels[j] == BACKGROUND_INDEX && !bg[j] {
                bg[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            visit(x - 1, y, &mut stack, &mut bg);
        }
        if x + 1 < w {
            visit(x + 1, y, &mut stack, &mut bg);
        }
        if y > 0 {
            visit(x, y - 1, &mut stack, &mut bg);
        }
        if y + 1 < h {
            visit(x, y + 1, &mut stack, &mut bg);
        }
    }
    bg
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of the flood fill: a light region *inside* the figure is
    /// artwork, not background. Charmander's belly and the core of its flame
    /// are exactly this shape, and a colour key erases both.
    #[test]
    fn an_enclosed_light_region_is_artwork_not_background() {
        // A 7x7 dark ring with a light hole in the middle, on a light page.
        let (w, h) = (7usize, 7usize);
        let mut px = vec![0u8; w * h];
        for y in 1..6 {
            for x in 1..6 {
                px[y * w + x] = 3;
            }
        }
        px[3 * w + 3] = 0; // the enclosed highlight
        let bg = background_mask(&px, w, h);
        assert!(bg[0], "the corner of the page is background");
        assert!(bg[w - 1] && bg[(h - 1) * w]);
        assert!(
            !bg[3 * w + 3],
            "an enclosed lightest-shade pixel must stay opaque"
        );
        assert!(!bg[2 * w + 2], "the figure itself is opaque");
        // Exactly the ring of page around the 5x5 block.
        assert_eq!(bg.iter().filter(|b| **b).count(), w * h - 25);
    }

    /// A figure that touches the frame must not let the fill in behind it, and
    /// a light area connected to the edge by a one-pixel channel *is* page.
    #[test]
    fn the_fill_is_four_connected_and_follows_channels() {
        let (w, h) = (5usize, 5usize);
        // Column of dark down the middle, open at the top row.
        let mut px = vec![0u8; w * h];
        for y in 1..5 {
            px[y * w + 2] = 3;
        }
        let bg = background_mask(&px, w, h);
        // Everything light is reachable — the dark column does not seal
        // anything off, because the top row is open.
        assert_eq!(bg.iter().filter(|b| **b).count(), w * h - 4);
        // Now seal it: dark all the way across the bottom-left quadrant.
        let mut px = vec![0u8; w * h];
        for x in 0..w {
            px[3 * w + x] = 3;
        }
        let bg = background_mask(&px, w, h);
        assert!(bg[0], "above the wall is open to the top edge");
        assert!(bg[4 * w], "below it is open to the bottom edge");
        let mut px = vec![0u8; w * h];
        for x in 0..w {
            px[3 * w + x] = 3;
            px[4 * w + x] = 3;
        }
        // With the bottom two rows solid, nothing below the wall is light at
        // all, so the count is just the light rows above it.
        let bg = background_mask(&px, w, h);
        assert_eq!(bg.iter().filter(|b| **b).count(), 3 * w);
    }

    #[test]
    fn an_all_dark_or_empty_picture_does_not_panic() {
        assert!(background_mask(&[], 0, 0).is_empty());
        assert!(background_mask(&[3; 16], 4, 4).iter().all(|b| !*b));
        assert!(background_mask(&[0; 16], 4, 4).iter().all(|b| *b));
        // Short buffers are refused rather than indexed past the end.
        assert!(background_mask(&[0; 4], 4, 4).iter().all(|b| !*b));
    }

    #[test]
    fn a_non_square_header_is_not_picture_data() {
        // The first byte is (width << 4) | height; 5x4 cannot be a Pokémon.
        assert!(decompress(&[0x54, 0, 0, 0, 0, 0, 0, 0]).is_none());
        // Zero, and larger than the 7x7 buffer, are both impossible.
        assert!(decompress(&[0x00, 0, 0, 0]).is_none());
        assert!(decompress(&[0x88, 0, 0, 0]).is_none());
    }

    #[test]
    fn a_truncated_stream_aborts_instead_of_looping() {
        // A plausible header followed by nothing must return, not spin.
        assert!(decompress(&[0x55]).is_none());
        assert!(decompress(&[0x11, 0x00]).is_none());
    }

    #[test]
    fn bits_are_read_most_significant_first() {
        let mut b = Bits::new(&[0b1010_0000, 0xFF]);
        assert_eq!(b.bit(), Some(1));
        assert_eq!(b.bit(), Some(0));
        assert_eq!(b.bit(), Some(1));
        assert_eq!(b.bits(5), Some(0));
        assert_eq!(b.bits(8), Some(0xFF));
        assert_eq!(b.bit(), None);
    }

    #[test]
    fn transposing_twice_is_the_identity() {
        let width = 3;
        let mut tiles: Vec<u8> = (0..(width * width * 16) as u8).collect();
        let original = tiles.clone();
        transpose_tiles(&mut tiles, width);
        assert_ne!(tiles, original, "a 3x3 transpose must move tiles");
        transpose_tiles(&mut tiles, width);
        assert_eq!(tiles, original);
    }

    #[test]
    fn tiles_expand_to_two_bit_indices_in_reading_order() {
        // One tile: low plane row 0 = 0b1100_0000, high plane row 0 = 0b1010_0000
        // -> per pixel (high << 1 | low): 3, 1, 2, 0, ...
        let mut tiles = vec![0u8; 16];
        tiles[0] = 0b1100_0000;
        tiles[1] = 0b1010_0000;
        let pic = Pic {
            tiles,
            width_tiles: 1,
        };
        let (px, w, h) = tiles_to_indices(&pic);
        assert_eq!((w, h), (8, 8));
        assert_eq!(&px[0..4], &[3, 1, 2, 0]);
    }

    #[test]
    fn a_round_trip_through_the_delta_transform_restores_the_plane() {
        // The delta decode is a bijection on each row, so applying the inverse
        // built from the same tables must return the original.
        let width = 2;
        let stride = width * 8;
        let mut plane: Vec<u8> = (0..(width * width * 8) as u8)
            .map(|v| v.wrapping_mul(37))
            .collect();
        let original = plane.clone();
        delta_decode(&mut plane, width);
        // Invert: for each row, walk columns rebuilding the source nybbles.
        for row in 0..stride {
            let mut last = 0usize;
            for col in 0..width {
                let i = col * stride + row;
                let hi_out = plane[i] >> 4;
                let hi_in = DECODE[last].iter().position(|&v| v == hi_out).unwrap() as u8;
                last = (hi_out & 1) as usize;
                let lo_out = plane[i] & 0x0F;
                let lo_in = DECODE[last].iter().position(|&v| v == lo_out).unwrap() as u8;
                last = (lo_out & 1) as usize;
                plane[i] = (hi_in << 4) | lo_in;
            }
        }
        assert_eq!(plane, original);
    }
}

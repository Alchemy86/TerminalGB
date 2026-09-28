//! Where the picture's 8x8 tiles come from.
//!
//! ## Why there are two sources
//!
//! The original's art is Nintendo's. This repository's standing rule is that
//! game assets are never committed — not as a fixture, not as a hash — and are
//! instead read at run time from the user's own files, with the check skipped
//! when they are absent (see the licence boundary in the root `AGENTS.md`).
//!
//! So: point `--gfx` at a directory holding pret's extracted
//! `surfing_pikachu_1a.png` and `surfing_pikachu_1b.png` and the port draws
//! the real thing. Without them it draws a built-in set written for this port,
//! and says which it used. The *game* is identical either way — the tile ids
//! gameplay asks about (`$06` rising, `$07` falling, `$14` crest) mean the same
//! thing in both.
//!
//! ## How the extracted PNGs decode
//!
//! Both are 2-bit greyscale. rgbgfx wrote them with the lightest value as the
//! background, so the Game Boy colour index is `3 - grey`: a fully white pixel
//! is index 0, which is transparent in a sprite and white in the background
//! under this game's palette. `1a` is 5 tiles per row and supplies background
//! tiles `$00`..`$4f`; `1b` is 16 per row and supplies every object tile, plus
//! `$80`..`$ff` of the background through the `$8800` signed addressing the
//! game selects by leaving `LCDC` bit 4 clear.

use std::path::{Path, PathBuf};

/// One 8x8 tile as colour indices 0..=3, row major.
pub type Tile = [u8; 64];

const BLANK: Tile = [0; 64];

/// Which art the picture is being drawn with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TileSource {
    /// pret's extracted PNGs, from this directory.
    Extracted(PathBuf),
    /// The set written for this port.
    BuiltIn,
}

impl TileSource {
    pub fn describe(&self) -> String {
        match self {
            TileSource::Extracted(p) => format!("extracted art from {}", p.display()),
            TileSource::BuiltIn => "built-in art (no --gfx directory given)".to_string(),
        }
    }
}

/// Every tile the picture can name.
pub struct TileSet {
    /// Background tiles as the game addresses them.
    pub bg: Vec<Tile>,
    /// Object tiles, `$00`..`$ff`.
    pub obj: Vec<Tile>,
    pub source: TileSource,
}

/// Object tile offsets of the seven board-angle poses, frame A then frame B,
/// from `SurfingPikachuOAMData` entries 1..=14.
pub const POSE_OFFSETS: [(u8, u8); 7] = [
    (0x00, 0x36),
    (0x03, 0x39),
    (0x06, 0x3c),
    (0x09, 0x60),
    (0x0c, 0x63),
    (0x30, 0x66),
    (0x33, 0x69),
];

impl TileSet {
    /// Load from `dir` if it holds the extracted PNGs, otherwise fall back.
    ///
    /// Never fails: a missing or unreadable directory is a reason to draw the
    /// built-in art and say so, not to refuse to run.
    pub fn load(dir: Option<&Path>) -> (TileSet, Option<String>) {
        let Some(dir) = dir else {
            return (Self::built_in(), None);
        };
        match Self::from_dir(dir) {
            Ok(set) => (set, None),
            Err(why) => (
                Self::built_in(),
                Some(format!("picture: {} — falling back to built-in art", why)),
            ),
        }
    }

    #[cfg(feature = "art")]
    fn from_dir(dir: &Path) -> Result<TileSet, String> {
        let bg_src = Self::decode(&dir.join("surfing_pikachu_1a.png"), 5)?;
        let obj_src = Self::decode(&dir.join("surfing_pikachu_1b.png"), 16)?;
        if obj_src.len() < 256 {
            return Err(format!(
                "surfing_pikachu_1b.png holds {} tiles, need 256",
                obj_src.len()
            ));
        }
        let mut bg = vec![BLANK; 256];
        // `FarCopyData` copies 80 tiles into vChars2, which the background's
        // signed addressing sees as $00..$4f.
        for (i, t) in bg_src.iter().take(0x50).enumerate() {
            bg[i] = *t;
        }
        // $80..$ff alias the object tiles: both resolve to $8800..$8fff.
        bg[0x80..0x100].copy_from_slice(&obj_src[0x80..0x100]);
        Ok(TileSet {
            bg,
            obj: obj_src,
            source: TileSource::Extracted(dir.to_path_buf()),
        })
    }

    #[cfg(not(feature = "art"))]
    fn from_dir(_dir: &Path) -> Result<TileSet, String> {
        Err("built without the `art` feature, so without a PNG decoder".to_string())
    }

    #[cfg(feature = "art")]
    fn decode(path: &Path, tiles_per_row: usize) -> Result<Vec<Tile>, String> {
        let img = image::open(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .to_luma8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        if w % 8 != 0 || h % 8 != 0 || w / 8 != tiles_per_row {
            return Err(format!(
                "{} is {w}x{h}, expected {} tiles per row",
                path.display(),
                tiles_per_row
            ));
        }
        let mut out = Vec::with_capacity((w / 8) * (h / 8));
        for trow in 0..h / 8 {
            for tcol in 0..tiles_per_row {
                let mut tile = BLANK;
                for y in 0..8 {
                    for x in 0..8 {
                        // 2-bit greyscale expands to 0/85/170/255.
                        let grey = (img
                            .get_pixel((tcol * 8 + x) as u32, (trow * 8 + y) as u32)
                            .0[0] as u16
                            + 42)
                            / 85;
                        tile[y * 8 + x] = 3 - grey.min(3) as u8;
                    }
                }
                out.push(tile);
            }
        }
        Ok(out)
    }

    /// The set written for this port. Same tile ids, same meanings.
    pub fn built_in() -> TileSet {
        let mut bg = vec![BLANK; 256];
        let mut obj = vec![BLANK; 256];

        // --- water and wave ---------------------------------------------
        bg[0x0b] = parse(OPEN_WATER);
        bg[0x08] = solid(2);
        bg[0x02] = parse(SHALLOW_RISE);
        bg[0x03] = mirror(&parse(SHALLOW_RISE));
        bg[0x04] = parse(RISE);
        bg[0x05] = mirror(&parse(RISE));
        bg[0x06] = parse(RISE);
        bg[0x07] = mirror(&parse(RISE));
        bg[0x11] = parse(WAVE_BACK);
        bg[0x12] = parse(WAVE_FACE);
        bg[0x13] = mirror(&parse(WAVE_BACK));
        bg[0x14] = parse(WAVE_CREST);
        // --- beach ------------------------------------------------------
        bg[0x0c] = parse(SAND_TOP);
        bg[0x0d] = parse(SAND);
        bg[0x0e] = parse(SAND_TOP);
        bg[0x0f] = parse(SHORE);
        bg[0x10] = mirror(&parse(SHORE));
        // --- status bar -------------------------------------------------
        bg[0x15] = glyph("H");
        bg[0x16] = glyph("P");
        bg[0x17] = parse(BAR_LEFT);
        bg[0x18] = parse(BAR_MID);
        bg[0x19] = parse(BAR_MID);
        bg[0x1b] = glyph("T");
        bg[0x1c] = glyph("o");
        // --- results text -----------------------------------------------
        for (id, ch) in RESULT_GLYPHS {
            bg[id as usize] = glyph(ch);
        }
        bg[0x3b] = parse(BOX_TL);
        bg[0x3c] = mirror(&parse(BOX_TL));
        bg[0x3d] = flip_v(&parse(BOX_TL));
        bg[0x3e] = mirror(&flip_v(&parse(BOX_TL)));
        bg[0x3f] = parse(BOX_SIDE);
        bg[0x40] = parse(BOX_TOP);
        bg[0xff] = BLANK;
        // --- digits, shared by background and objects -------------------
        for d in 0..10u8 {
            let t = glyph(DIGITS[d as usize]);
            bg[0xd0 + d as usize] = t;
            obj[0xd0 + d as usize] = t;
        }
        obj[0xbf] = glyph("+");
        obj[0xfe] = parse(MINI_PIKACHU);
        obj[0xec] = parse(CLOUD_L);
        obj[0xed] = parse(CLOUD_M);
        obj[0xee] = parse(CLOUD_M);
        obj[0xef] = mirror(&parse(CLOUD_L));

        // --- Pikachu, one base pose rotated into fourteen ---------------
        //
        // The original draws each board angle by hand. Fourteen frame sets
        // cover a full turn (the upper seven are the lower seven flipped both
        // ways), so pose N sits at (N - 3) * 360/14 degrees from level, and
        // rotating the base pose by that lands each one where the game's own
        // landing table expects it.
        let base = sprite24(PIKACHU);
        for (pose, (a, b)) in POSE_OFFSETS.iter().enumerate() {
            let degrees = (pose as f32 - 3.0) * (360.0 / 14.0);
            let rotated = rotate24(&base, degrees);
            write_block(&mut obj, *a, &rotated);
            write_block(&mut obj, *b, &rotated);
        }
        // The results pose and the empty surfboard after a wipeout.
        write_block(&mut obj, 0xa0, &base);
        write_block(&mut obj, 0xa3, &base);
        let board = sprite24(SURFBOARD);
        for tcol in 0..3usize {
            let mut t = BLANK;
            for y in 0..8 {
                for x in 0..8 {
                    t[y * 8 + x] = board[(16 + y) * 24 + tcol * 8 + x];
                }
            }
            obj[0x98 + tcol] = t;
        }
        // Splash and spray share the foam tile in the original; here one tile
        // repeated reads the same at this size.
        // Every tile the spray and splash shapes reach. Their layouts are
        // scattered across the sheet in the original; one foam tile repeated
        // reads the same at 8x8.
        for id in [0xa7, 0xb6, 0xb7, 0xa8, 0xa9, 0xb8, 0xb9, 0xc8, 0xc9] {
            obj[id] = parse(FOAM);
        }
        // The two banners: six tiles across, top row lettered, bottom blank.
        for (i, ch) in [".", "S", "T", "A", "R", "T"].into_iter().enumerate() {
            obj[0xe0 + i] = glyph(ch);
        }
        for (i, ch) in ["O", "h", ".", "n", "o", "."].into_iter().enumerate() {
            obj[0xca + i] = glyph(ch);
        }

        TileSet {
            bg,
            obj,
            source: TileSource::BuiltIn,
        }
    }
}

// ---------------------------------------------------------------------------
// Authoring helpers. `.` is colour 0, `,` is 1, `+` is 2, `#` is 3.
// ---------------------------------------------------------------------------

fn index_of(c: char) -> u8 {
    match c {
        ',' => 1,
        '+' => 2,
        '#' => 3,
        _ => 0,
    }
}

fn parse(rows: [&str; 8]) -> Tile {
    let mut t = BLANK;
    for (y, row) in rows.iter().enumerate() {
        debug_assert_eq!(
            row.chars().count(),
            8,
            "tile row {y} is not 8 wide: {row:?}"
        );
        for (x, c) in row.chars().take(8).enumerate() {
            t[y * 8 + x] = index_of(c);
        }
    }
    t
}

fn solid(colour: u8) -> Tile {
    [colour; 64]
}

fn mirror(t: &Tile) -> Tile {
    let mut out = BLANK;
    for y in 0..8 {
        for x in 0..8 {
            out[y * 8 + x] = t[y * 8 + (7 - x)];
        }
    }
    out
}

fn flip_v(t: &Tile) -> Tile {
    let mut out = BLANK;
    for y in 0..8 {
        out[y * 8..y * 8 + 8].copy_from_slice(&t[(7 - y) * 8..(7 - y) * 8 + 8]);
    }
    out
}

/// A 24x24 sprite from 24 rows of 24 characters.
fn sprite24(rows: [&str; 24]) -> [u8; 24 * 24] {
    let mut out = [0u8; 24 * 24];
    for (y, row) in rows.iter().enumerate() {
        debug_assert_eq!(
            row.chars().count(),
            24,
            "sprite row {y} is not 24 wide: {row:?}"
        );
        for (x, c) in row.chars().take(24).enumerate() {
            out[y * 24 + x] = index_of(c);
        }
    }
    out
}

/// Nearest-neighbour rotation about the centre. Rough, as rotated pixel art
/// always is, but the silhouette is what the player reads at this size.
fn rotate24(src: &[u8; 24 * 24], degrees: f32) -> [u8; 24 * 24] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    let mut out = [0u8; 24 * 24];
    for y in 0..24usize {
        for x in 0..24usize {
            let (dx, dy) = (x as f32 - 11.5, y as f32 - 11.5);
            let sx = (dx * cos + dy * sin + 11.5).round();
            let sy = (-dx * sin + dy * cos + 11.5).round();
            if (0.0..24.0).contains(&sx) && (0.0..24.0).contains(&sy) {
                out[y * 24 + x] = src[sy as usize * 24 + sx as usize];
            }
        }
    }
    out
}

/// Store a 24x24 sprite as the 3x3 tile block the OAM layout expects: three
/// tiles across, then `+$10` for the next row down.
fn write_block(obj: &mut [Tile], base: u8, src: &[u8; 24 * 24]) {
    for trow in 0..3usize {
        for tcol in 0..3usize {
            let id = base as usize + trow * 0x10 + tcol;
            if id >= obj.len() {
                continue;
            }
            let mut t = BLANK;
            for y in 0..8 {
                for x in 0..8 {
                    t[y * 8 + x] = src[(trow * 8 + y) * 24 + tcol * 8 + x];
                }
            }
            obj[id] = t;
        }
    }
}

// ---------------------------------------------------------------------------
// The built-in art.
// ---------------------------------------------------------------------------

#[rustfmt::skip]
const OPEN_WATER: [&str; 8] = [
    "++.+++++", "+++++.++", ".+++++++", "++++.+++",
    "+++++++.", "++.+++++", "++++++.+", ".+++++++",
];
#[rustfmt::skip]
const SHALLOW_RISE: [&str; 8] = [
    "........", "........", "......##", "..####++",
    "##++++++", "++++++++", "++++++++", "++++++++",
];
#[rustfmt::skip]
const RISE: [&str; 8] = [
    ".......#", "......#+", ".....#++", "....#+++",
    "...#++++", "..#+++++", ".#++++++", "#+++++++",
];
#[rustfmt::skip]
const WAVE_FACE: [&str; 8] = [
    "..######", "..++++++", "..++++++", "..++++++",
    ".+++++++", ".+++++++", "++++++++", "++++++++",
];
#[rustfmt::skip]
const WAVE_BACK: [&str; 8] = [
    "######..", "++++++..", "++++++..", "+++++++.",
    "+++++++.", "++++++++", "++++++++", "++++++++",
];
#[rustfmt::skip]
const WAVE_CREST: [&str; 8] = [
    "...####.", "..##++##", ".##+++++", "##++++++",
    "#+++++++", "++++++++", "++++++++", "++++++++",
];
// Colour 1 is deliberately absent from every background tile: this game's
// BGP is `$d0`, which paints 0 and 1 the same white, so sand drawn in 1 would
// be invisible against the sky. Three tones is all the background has.
#[rustfmt::skip]
const SAND_TOP: [&str; 8] = [
    "########", "++++++++", "+#++++#+", "++++++++",
    "++#+++++", "++++++#+", "+++#++++", "++++++++",
];
#[rustfmt::skip]
const SAND: [&str; 8] = [
    "++++++++", "+++#++++", "++++++++", "+#++++#+",
    "++++++++", "++++#+++", "++++++++", "+++++++#",
];
#[rustfmt::skip]
const SHORE: [&str; 8] = [
    "######++", "++++##++", "+++++#++", "+++++#++",
    "++++++#+", "++++++#+", "+++++++#", "+++++++#",
];
#[rustfmt::skip]
const FOAM: [&str; 8] = [
    "..,..,..", ".,,,.,,.", ",,.,,,,,", ".,,,,.,.",
    "..,,,,..", ".,..,.,.", "..,..,..", "........",
];
#[rustfmt::skip]
const CLOUD_L: [&str; 8] = [
    "........", "........", "....####", "..##,,,,",
    ".#,,,,,,", "#,,,,,,,", "#,,,,,,,", "########",
];
#[rustfmt::skip]
const CLOUD_M: [&str; 8] = [
    "........", "..####..", "##,,,,##", ",,,,,,,,",
    ",,,,,,,,", ",,,,,,,,", ",,,,,,,,", "########",
];
#[rustfmt::skip]
const BAR_LEFT: [&str; 8] = [
    "........", "########", "#.......", "#.......",
    "#.......", "#.......", "########", "........",
];
#[rustfmt::skip]
const BAR_MID: [&str; 8] = [
    "........", "########", "........", "........",
    "........", "........", "########", "........",
];
#[rustfmt::skip]
const BOX_TL: [&str; 8] = [
    "........", ".######.", ".#......", ".#......",
    ".#......", ".#......", ".#......", ".#......",
];
#[rustfmt::skip]
const BOX_SIDE: [&str; 8] = [
    ".#......", ".#......", ".#......", ".#......",
    ".#......", ".#......", ".#......", ".#......",
];
#[rustfmt::skip]
const BOX_TOP: [&str; 8] = [
    "........", "########", "........", "........",
    "........", "........", "........", "........",
];
#[rustfmt::skip]
const MINI_PIKACHU: [&str; 8] = [
    ".#....#.", ".##..##.", "..####..", ".#,,,,#.",
    ".#,#,#,#", ".#,,,,#.", "..####..", "........",
];

/// Facing right, standing on the board. Colour 1 is Pikachu (light under the
/// game's object palette), 2 the board, 3 the outline.
#[rustfmt::skip]
const PIKACHU: [&str; 24] = [
    "........................",
    ".....#..........#.......",
    "....#,#........#,#......",
    "....#,#........#,#......",
    "....#,,#......#,,#......",
    ".....#,,#....#,,#.......",
    "......#,,####,,#........",
    ".......#,,,,,,#.........",
    "......#,,,,,,,,#........",
    ".....#,#,,,,#,#,#.......",
    ".....#,,,,,,,,,,#.......",
    ".....#,,,####,,,#.......",
    "......#,,,,,,,,#........",
    ".......##,,,,##.........",
    "......#,,,##,,,,#.......",
    ".....#,,,,,,,,,,,#......",
    ".....#,,,,,,,,,,,#......",
    "......#,,,,,,,,,#.......",
    ".......#,,,,,,,#........",
    "........#######.........",
    "...####++++++++####.....",
    "..##++++++++++++++##....",
    "..#++++++++++++++++#....",
    "...##################...",
];

#[rustfmt::skip]
const SURFBOARD: [&str; 24] = [
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "........................",
    "...####++++++++####.....",
    "..##++++++++++++++##....",
    "..#++++++++++++++++#....",
    "...##################...",
];

const DIGITS: [&str; 10] = ["0", "1", "2", "3", "4", "5", "6", "7", "8", "9"];

/// The results screen's characters and the tile ids the original stores them
/// at, read straight off `.HP_Left`, `.Radness`, `.Total` and `.Hi_Score`.
const RESULT_GLYPHS: [(u8, &str); 20] = [
    (0x20, "H"),
    (0x21, "P"),
    (0x22, "L"),
    (0x23, "e"),
    (0x24, "f"),
    (0x25, "t"),
    (0x26, "s"),
    (0x27, "R"),
    (0x28, "a"),
    (0x29, "d"),
    (0x2a, "n"),
    (0x2b, "T"),
    (0x2c, "o"),
    (0x2d, "l"),
    (0x2e, "i"),
    (0x2f, "-"),
    (0x30, "S"),
    (0x31, "c"),
    (0x32, "r"),
    (0x33, "!"),
];

/// A 5x7 glyph in an 8x8 cell. Only the characters this game prints exist;
/// anything else is blank, which is what the original's `$ff` does.
fn glyph(c: &str) -> Tile {
    #[rustfmt::skip]
    let rows: [&str; 8] = match c {
        "0" => ["........", ".####...", ".#..#...", ".#..#...", ".#..#...", ".#..#...", ".####...", "........"],
        "1" => ["........", "..##....", ".#.#....", "...#....", "...#....", "...#....", ".####...", "........"],
        "2" => ["........", ".####...", "....#...", ".####...", ".#......", ".#......", ".####...", "........"],
        "3" => ["........", ".####...", "....#...", "..###...", "....#...", "....#...", ".####...", "........"],
        "4" => ["........", ".#..#...", ".#..#...", ".####...", "....#...", "....#...", "....#...", "........"],
        "5" => ["........", ".####...", ".#......", ".####...", "....#...", "....#...", ".####...", "........"],
        "6" => ["........", ".####...", ".#......", ".####...", ".#..#...", ".#..#...", ".####...", "........"],
        "7" => ["........", ".####...", "....#...", "...#....", "..#.....", "..#.....", "..#.....", "........"],
        "8" => ["........", ".####...", ".#..#...", ".####...", ".#..#...", ".#..#...", ".####...", "........"],
        "9" => ["........", ".####...", ".#..#...", ".####...", "....#...", "....#...", ".####...", "........"],
        "H" => ["........", ".#..#...", ".#..#...", ".####...", ".#..#...", ".#..#...", ".#..#...", "........"],
        "P" => ["........", ".###....", ".#..#...", ".###....", ".#......", ".#......", ".#......", "........"],
        "L" => ["........", ".#......", ".#......", ".#......", ".#......", ".#......", ".####...", "........"],
        "R" => ["........", ".###....", ".#..#...", ".###....", ".#.#....", ".#..#...", ".#..#...", "........"],
        "T" => ["........", ".#####..", "...#....", "...#....", "...#....", "...#....", "...#....", "........"],
        "S" => ["........", "..###...", ".#......", "..##....", "....#...", "....#...", ".###....", "........"],
        "e" => ["........", "........", "..##....", ".#..#...", ".####...", ".#......", "..###...", "........"],
        "f" => ["........", "...##...", "..#.....", ".####...", "..#.....", "..#.....", "..#.....", "........"],
        "t" => ["........", "..#.....", "..#.....", ".####...", "..#.....", "..#.#...", "...#....", "........"],
        "s" => ["........", "........", "..###...", ".#......", "..##....", "....#...", ".###....", "........"],
        "a" => ["........", "........", "..###...", "....#...", "..####..", ".#..#...", "..####..", "........"],
        "d" => ["........", "....#...", "....#...", "..####..", ".#..#...", ".#..#...", "..####..", "........"],
        "n" => ["........", "........", ".#.##...", ".##..#..", ".#...#..", ".#...#..", ".#...#..", "........"],
        "o" => ["........", "........", "..##....", ".#..#...", ".#..#...", ".#..#...", "..##....", "........"],
        "l" => ["........", "..##....", "...#....", "...#....", "...#....", "...#....", "..###...", "........"],
        "i" => ["........", "...#....", "........", "..##....", "...#....", "...#....", "..###...", "........"],
        "c" => ["........", "........", "..###...", ".#......", ".#......", ".#......", "..###...", "........"],
        "r" => ["........", "........", ".#.##...", ".##.....", ".#......", ".#......", ".#......", "........"],
        "-" => ["........", "........", "........", ".####...", "........", "........", "........", "........"],
        "!" => ["........", ".#..#...", ".#..#...", ".#..#...", ".#..#...", "........", ".#..#...", "........"],
        "+" => ["........", "........", "...#....", "...#....", ".#####..", "...#....", "...#....", "........"],
        "A" => ["........", "..##....", ".#..#...", ".#..#...", ".####...", ".#..#...", ".#..#...", "........"],
        "O" => ["........", "..##....", ".#..#...", ".#..#...", ".#..#...", ".#..#...", "..##....", "........"],
        "N" => ["........", ".#...#..", ".##..#..", ".#.#.#..", ".#..##..", ".#...#..", ".#...#..", "........"],
        "h" => ["........", ".#......", ".#......", ".#.##...", ".##..#..", ".#...#..", ".#...#..", "........"],
        "." => ["........", "........", "........", "........", "........", "........", "..##....", "........"],
        _ => ["........", "........", "........", "........", "........", "........", "........", "........"],
    };
    parse(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_art_is_well_formed() {
        // `parse` and `sprite24` debug-assert their row widths, so simply
        // building the set in a debug test is the check.
        let set = TileSet::built_in();
        assert_eq!(set.bg.len(), 256);
        assert_eq!(set.obj.len(), 256);
        assert_eq!(set.source, TileSource::BuiltIn);
    }

    #[test]
    fn every_tile_gameplay_names_is_drawn() {
        let set = TileSet::built_in();
        for id in [
            crate::wave::tile::RISING_SLOPE,
            crate::wave::tile::FALLING_SLOPE,
            crate::wave::tile::WAVE_CREST,
            crate::wave::tile::WAVE_FACE,
            0x0b,
        ] {
            assert!(
                set.bg[id as usize].iter().any(|p| *p != 0),
                "tile {id:#04x} is blank"
            );
        }
    }

    #[test]
    fn every_metatile_tile_exists_in_the_built_in_set() {
        // A metatile naming a tile nobody drew would render as a hole in the
        // wave, and the hole would be invisible in a diff.
        let set = TileSet::built_in();
        for (i, m) in crate::wave::METATILES.iter().enumerate() {
            for id in m {
                if *id == 0 {
                    continue; // sky is deliberately blank
                }
                assert!(
                    set.bg[*id as usize].iter().any(|p| *p != 0),
                    "metatile {i:#04x} names undrawn tile {id:#04x}"
                );
            }
        }
    }

    #[test]
    fn all_fourteen_board_angles_have_a_sprite() {
        let set = TileSet::built_in();
        for (pose, (a, b)) in POSE_OFFSETS.iter().enumerate() {
            for base in [*a, *b] {
                let filled = (0..3)
                    .flat_map(|r| (0..3).map(move |c| base as usize + r * 0x10 + c))
                    .filter(|id| set.obj[*id].iter().any(|p| *p != 0))
                    .count();
                assert!(
                    filled >= 4,
                    "pose {pose} at {base:#04x} is nearly empty ({filled}/9)"
                );
            }
        }
    }

    /// The background palette this game selects is `$d0`, which paints
    /// colour index 0 and index 1 the **same white**. A background tile drawn
    /// in index 1 is therefore invisible, and invisibly so — the beach was
    /// drawn that way first and rendered as bare sky. Nothing catches that but
    /// looking at it, or this.
    #[test]
    fn no_background_tile_relies_on_colour_index_one() {
        let set = TileSet::built_in();
        for (id, tile) in set.bg.iter().enumerate() {
            assert!(
                !tile.contains(&1),
                "background tile {id:#04x} uses colour 1, which BGP $d0 paints white"
            );
        }
    }

    /// Two background tiles that must never be confusable: you are standing
    /// on one of them and running aground on the other.
    #[test]
    fn sand_and_water_do_not_look_alike() {
        let set = TileSet::built_in();
        assert_ne!(
            set.bg[0x0d], set.bg[0x0b],
            "sand and open water are the same tile"
        );
    }

    #[test]
    fn every_glyph_the_game_prints_is_eight_by_eight_and_not_blank() {
        // `parse` debug-asserts row width, but only for rows a test happens
        // to reach. Walking the whole set is what actually covers it.
        for (_, ch) in RESULT_GLYPHS {
            let t = glyph(ch);
            assert!(t.iter().any(|p| *p != 0), "glyph {ch:?} is blank");
        }
        for ch in DIGITS
            .iter()
            .chain(["+", "A", "O", "N", "h", ".", "S", "T"].iter())
        {
            let t = glyph(ch);
            assert!(t.iter().any(|p| *p != 0), "glyph {ch:?} is blank");
        }
    }

    #[test]
    fn the_splash_and_banner_tiles_the_shapes_reach_are_all_drawn() {
        let set = TileSet::built_in();
        for id in [
            0xa7usize, 0xb6, 0xb7, 0xa8, 0xa9, 0xb8, 0xb9, 0xc8, 0xc9, 0x98, 0x99, 0x9a,
        ] {
            assert!(
                set.obj[id].iter().any(|p| *p != 0),
                "object tile {id:#04x} is blank"
            );
        }
        // The empty surfboard must not have been written over the splash.
        assert_ne!(set.obj[0x98], set.obj[0xa8]);
    }

    #[test]
    fn digits_are_drawn_and_distinct() {
        let set = TileSet::built_in();
        for d in 0..10usize {
            assert!(
                set.bg[0xd0 + d].iter().any(|p| *p != 0),
                "digit {d} is blank"
            );
            assert_eq!(
                set.bg[0xd0 + d],
                set.obj[0xd0 + d],
                "digit {d} differs between planes"
            );
        }
        assert_ne!(set.bg[0xd0], set.bg[0xd8]);
    }
}

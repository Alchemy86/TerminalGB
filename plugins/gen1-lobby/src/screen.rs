//! Deriving screen tiles from the block map — the engine's job, done by hand
//! for the few rows a speech bubble borrows.
//!
//! Nothing here runs in the ordinary path. The room is installed as blocks and
//! the engine paints every scrolled-in row and column itself; this exists so
//! that a bubble drawn over `wTileMap` can be *undone* exactly, rather than
//! left to be tidied up by a scroll that may never happen because the player is
//! standing still.
//!
//! The blockset is read out of **the player's own cartridge** through
//! `wTilesetBlocksPtr` and `wTilesetBank`, never embedded here: the block
//! definitions are the cartridge's data, and reading them at run time is both
//! the licence-clean route and the robust one.

use crate::gen1::*;
use crate::room::Room;
use terminalgb_plugin_api::Machine;

/// The CLUB blockset has 36 blocks; reading a few more costs nothing and keeps
/// this correct if a future room borrows a different tileset.
pub const MAX_BLOCKS: usize = 64;

/// A tileset's block definitions, as read from the cartridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Blockset {
    tiles: Vec<u8>,
}

impl Blockset {
    /// Read `MAX_BLOCKS` blocks through the live tileset pointer.
    ///
    /// `peek_bank` takes the bank explicitly and touches no MBC register, so
    /// this cannot perturb the running game the way a read-then-restore would.
    pub fn read(m: &mut Machine<'_>) -> Option<Blockset> {
        let bank = m.peek(W_TILESET_BANK)? as usize;
        let lo = m.peek(W_TILESET_BLOCKS_PTR)? as u16;
        let hi = m.peek(W_TILESET_BLOCKS_PTR + 1)? as u16;
        let ptr = (hi << 8) | lo;
        let mut tiles = Vec::with_capacity(MAX_BLOCKS * BLOCK_TILES);
        for i in 0..(MAX_BLOCKS * BLOCK_TILES) as u16 {
            tiles.push(m.peek_bank(bank, ptr.wrapping_add(i))?);
        }
        Some(Blockset { tiles })
    }

    /// One tile of one block. `(col, row)` are 0-3 within the block's 4x4.
    pub fn tile(&self, block: u8, col: usize, row: usize) -> u8 {
        let at = block as usize * BLOCK_TILES + row * BLOCK_SIDE + col;
        self.tiles.get(at).copied().unwrap_or(0)
    }
}

/// Which map tile and which of its four screen tiles a screen cell shows.
///
/// The player is pinned to screen tile
/// ([`PLAYER_SCREEN_COL`], [`PLAYER_SCREEN_ROW`]) and the *view* scrolls around
/// them, so the mapping has to be recomputed from `wXCoord`/`wYCoord` and never
/// cached from a spawn position — compute it once and the room slides out from
/// under the player.
pub fn cell_to_map(px: u8, py: u8, col: i32, row: i32) -> (i32, i32, usize, usize) {
    cell_to_map_at(px, py, col, row, PLAYER_SCREEN_ROW)
}

/// As [`cell_to_map`], with the anchor row given — the one thing worth
/// measuring rather than reasoning about, because Gen 1 draws the player four
/// pixels high and the row its own collision test calls "the tile the player
/// stands on" is the *bottom* row of their 2x2.
pub fn cell_to_map_at(
    px: u8,
    py: u8,
    col: i32,
    row: i32,
    anchor_row: i32,
) -> (i32, i32, usize, usize) {
    let dx = col - PLAYER_SCREEN_COL;
    let dy = row - anchor_row;
    (
        px as i32 + dx.div_euclid(2),
        py as i32 + dy.div_euclid(2),
        dx.rem_euclid(2) as usize,
        dy.rem_euclid(2) as usize,
    )
}

/// The tile id the room says belongs at a screen cell.
pub fn tile_for_cell(
    room: &Room,
    blocks: &[u8],
    bs: &Blockset,
    px: u8,
    py: u8,
    col: i32,
    row: i32,
) -> u8 {
    tile_for_cell_at(room, blocks, bs, px, py, col, row, PLAYER_SCREEN_ROW)
}

#[allow(clippy::too_many_arguments)]
pub fn tile_for_cell_at(
    room: &Room,
    blocks: &[u8],
    bs: &Blockset,
    px: u8,
    py: u8,
    col: i32,
    row: i32,
    anchor_row: i32,
) -> u8 {
    let (x, y, sub_col, sub_row) = cell_to_map_at(px, py, col, row, anchor_row);
    let (bw, bh) = (room.w as i32, room.h as i32);
    let (bx, by) = (x.div_euclid(2), y.div_euclid(2));
    let block = if (0..bw).contains(&bx) && (0..bh).contains(&by) {
        blocks[(by * bw + bx) as usize]
    } else {
        CLUB_WALL_BLOCK
    };
    let qx = x.rem_euclid(2) as usize;
    let qy = y.rem_euclid(2) as usize;
    bs.tile(block, qx * 2 + sub_col, qy * 2 + sub_row)
}

/// How well our derivation reproduces the engine's own screen, for the room
/// the machine is *currently* running.
///
/// **This is the check that earns the right to paint anything.** Deriving
/// screen tiles from a block map means reimplementing `LoadCurrentMapView` and
/// `DrawTileBlock`, and a one-row error in the anchor is invisible on a
/// checkerboard floor and glaring on anything else. So before the plugin ever
/// writes `wTileMap` it compares what it *would* have written against what the
/// engine already did, cell by cell, and only paints if they agree.
pub fn agreement(
    m: &mut Machine<'_>,
    room: &Room,
    blocks: &[u8],
    bs: &Blockset,
) -> (usize, usize) {
    agreement_at(m, room, blocks, bs, PLAYER_SCREEN_ROW)
}

pub fn agreement_at(
    m: &mut Machine<'_>,
    room: &Room,
    blocks: &[u8],
    bs: &Blockset,
    anchor_row: i32,
) -> (usize, usize) {
    let px = m.peek(W_X_COORD).unwrap_or(0);
    let py = m.peek(W_Y_COORD).unwrap_or(0);
    let mut ok = 0;
    let mut total = 0;
    for row in 0..SCREEN_H {
        for col in 0..SCREEN_W {
            let want = tile_for_cell_at(
                room, blocks, bs, px, py, col as i32, row as i32, anchor_row,
            );
            let got = m
                .peek(W_TILE_MAP + (row * SCREEN_W + col) as u16)
                .unwrap_or(0xFF);
            total += 1;
            if want == got {
                ok += 1;
            }
        }
    }
    (ok, total)
}

/// How many different tile ids the engine currently has on screen.
///
/// The cheap "is this a real picture?" test — see [`MIN_DISTINCT_TILES`].
pub fn distinct_screen_tiles(m: &mut Machine<'_>) -> usize {
    let mut tiles = [0u8; SCREEN_W * SCREEN_H];
    for (i, t) in tiles.iter_mut().enumerate() {
        *t = m.peek(W_TILE_MAP + i as u16).unwrap_or(0);
    }
    distinct(&tiles)
}

/// Is this screen detailed enough for an exact match against it to be evidence?
///
/// Pure, so the rule can be tested without a machine. See
/// [`MIN_DISTINCT_TILES`] for why the rule exists at all.
pub fn enough_detail_to_calibrate(tiles: &[u8]) -> bool {
    distinct(tiles) >= MIN_DISTINCT_TILES
}

fn distinct(tiles: &[u8]) -> usize {
    let mut seen = [false; 256];
    let mut n = 0;
    for &t in tiles {
        if !seen[t as usize] {
            seen[t as usize] = true;
            n += 1;
        }
    }
    n
}

/// Read the room the machine is currently running straight out of WRAM — the
/// cartridge's own, before we replace it — so [`agreement`] has something
/// independent to check against.
pub fn live_room(m: &mut Machine<'_>) -> Option<(Room, Vec<u8>)> {
    let w = m.peek(W_CUR_MAP_WIDTH)? as usize;
    let h = m.peek(W_CUR_MAP_HEIGHT)? as usize;
    if w == 0 || h == 0 || (w + 6) * (h + 6) > OVERWORLD_MAP_LEN {
        return None;
    }
    let room = Room::new(w.max(1), h.max(1));
    let stride = w + MAP_BORDER * 2;
    let mut blocks = Vec::with_capacity(w * h);
    for r in 0..h {
        for c in 0..w {
            let at = (MAP_BORDER + r) * stride + MAP_BORDER + c;
            blocks.push(m.peek(W_OVERWORLD_MAP + at as u16)?);
        }
    }
    Some((room, blocks))
}

/// The two candidate anchor rows. The player's walk tile is two screen rows
/// tall and the engine's own "tile the player stands on" is its *bottom* row
/// (`GetTileSpriteStandsOn` adds a row explicitly, and `CheckSpriteAvailability`
/// reads that row as "bottom left" before stepping back for "top left"). Which
/// of the two the *view* is anchored on is a question about
/// `LoadCurrentMapView`, and it is cheaper and far more honest to measure it
/// against the running engine than to argue it out of the disassembly.
pub const ANCHOR_CANDIDATES: [i32; 2] = [8, 9];

/// How many distinct tiles the engine's screen must show before a match with
/// our reconstruction counts as evidence of anything.
///
/// **A blank screen agrees with a blank block map, and that is not a
/// calibration — it is two uninitialised buffers holding the same byte.** On
/// the very first frame after a cartridge is loaded, `wTileMap` and
/// `wOverworldMap` are both untouched WRAM, and a 360-of-360 "exact
/// reproduction" falls straight out. Measured: with the plugin attached at ROM
/// load, as the terminal frontend attaches it, the room installed on **frame
/// 1** — before the boot logo — on a match of one repeated byte.
///
/// Four is a real screen: even the Cable Club's bare floor has a floor tile,
/// two wall tiles and a border. Same rule, and the same reason, as the
/// test-ROM harness requiring its reference image to have more than one shade
/// before it will believe a comparison against it.
const MIN_DISTINCT_TILES: usize = 4;

/// Pick the anchor row that reproduces the engine's screen exactly, or `None`
/// if neither does — in which case the plugin declines to paint at all rather
/// than painting something plausible and wrong.
///
/// `None` is also the answer when the engine's screen is too uniform to be
/// evidence — see [`MIN_DISTINCT_TILES`].
pub fn calibrate(
    m: &mut Machine<'_>,
    bs: &Blockset,
) -> (Option<i32>, [(usize, usize); 2]) {
    if distinct_screen_tiles(m) < MIN_DISTINCT_TILES {
        return (None, [(0, 0); 2]);
    }
    let Some((room, blocks)) = live_room(m) else {
        return (None, [(0, 0); 2]);
    };
    let mut scores = [(0usize, 0usize); 2];
    let mut best = None;
    for (i, anchor) in ANCHOR_CANDIDATES.iter().enumerate() {
        let (ok, total) = agreement_at(m, &room, &blocks, bs, *anchor);
        scores[i] = (ok, total);
        if ok == total && total > 0 && best.is_none() {
            best = Some(*anchor);
        }
    }
    (best, scores)
}

/// Restore whole screen rows from the block map — the undo for a bubble.
pub fn repaint_rows(
    m: &mut Machine<'_>,
    room: &Room,
    blocks: &[u8],
    bs: &Blockset,
    rows: &[usize],
    anchor_row: i32,
) {
    let px = m.peek(W_X_COORD).unwrap_or(0);
    let py = m.peek(W_Y_COORD).unwrap_or(0);
    for &row in rows {
        if row >= SCREEN_H {
            continue;
        }
        for col in 0..SCREEN_W {
            let t = tile_for_cell_at(
                room, blocks, bs, px, py, col as i32, row as i32, anchor_row,
            );
            m.poke(W_TILE_MAP + (row * SCREEN_W + col) as u16, t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::room::DEFAULT_ROOM;

    /// **A blank screen agrees with a blank block map, and that is not a
    /// calibration.** Measured: with the plugin attached at ROM load — which is
    /// what the terminal frontend does — the room installed on frame 1, before
    /// the boot logo, on a 360-of-360 "exact reproduction" of one repeated byte
    /// against another. See `docs/worldlobby.md` §9.1.
    #[test]
    fn a_blank_screen_is_never_a_calibration() {
        let blank = [0u8; SCREEN_W * SCREEN_H];
        assert!(!enough_detail_to_calibrate(&blank));
        // Uninitialised WRAM is not always zero, and a uniform anything is
        // just as worthless.
        assert!(!enough_detail_to_calibrate(&[0xFFu8; SCREEN_W * SCREEN_H]));
        // Nor is a screen of two or three tiles — a fade, a text box border.
        let mut nearly = blank;
        nearly[0] = 1;
        nearly[1] = 2;
        assert!(!enough_detail_to_calibrate(&nearly));
        // A real room is. The Cable Club's own screen measured 19 distinct
        // tiles; four is the floor.
        let mut real = blank;
        for (i, t) in real.iter_mut().enumerate() {
            *t = (i % 19) as u8;
        }
        assert!(enough_detail_to_calibrate(&real));
    }

    #[test]
    fn the_players_own_walk_tile_is_the_four_cells_at_eight_eight() {
        // The anchor is the TOP-LEFT of the player's 2x2, measured against the
        // engine (360/360 against 133/360 for the row below it).
        assert_eq!(cell_to_map(10, 10, 8, 8), (10, 10, 0, 0));
        assert_eq!(cell_to_map(10, 10, 9, 9), (10, 10, 1, 1));
        // (8, 9) is the cell the cartridge's own collision test reads, and it
        // is the bottom-left of the same walk tile.
        assert_eq!(cell_to_map(10, 10, 8, 9), (10, 10, 0, 1));
        // One walk tile right and one down.
        assert_eq!(cell_to_map(10, 10, 10, 10), (11, 11, 0, 0));
    }

    #[test]
    fn the_mapping_is_symmetric_about_the_player_including_negatives() {
        // `div_euclid`, not `/`: integer division rounds towards zero and would
        // put both screen columns 6 and 7 on the same map tile as 8 and 9.
        assert_eq!(cell_to_map(10, 10, 7, 8).0, 9);
        assert_eq!(cell_to_map(10, 10, 6, 8).0, 9);
        assert_eq!(cell_to_map(10, 10, 5, 8).0, 8);
        assert_eq!(cell_to_map(10, 10, 7, 8).2, 1);
        assert_eq!(cell_to_map(10, 10, 6, 8).2, 0);
    }

    #[test]
    fn the_two_candidate_anchors_are_the_two_rows_of_the_players_own_tile() {
        assert_eq!(
            ANCHOR_CANDIDATES,
            [PLAYER_SCREEN_ROW, PLAYER_SCREEN_ROW + 1]
        );
    }

    #[test]
    fn outside_the_room_reads_as_wall_rather_than_as_whatever_is_next_door() {
        let bs = Blockset {
            tiles: {
                let mut t = vec![0u8; MAX_BLOCKS * BLOCK_TILES];
                // Mark the wall block so it is distinguishable.
                for i in 0..BLOCK_TILES {
                    t[CLUB_WALL_BLOCK as usize * BLOCK_TILES + i] = 0x19;
                }
                for i in 0..BLOCK_TILES {
                    t[CLUB_FLOOR_BLOCK as usize * BLOCK_TILES + i] = 0x0F;
                }
                t
            },
        };
        let blocks = DEFAULT_ROOM.blocks();
        // Standing at the top-left, looking up and left: off the map.
        let t = tile_for_cell(&DEFAULT_ROOM, &blocks, &bs, 0, 0, 0, 0);
        assert_eq!(t, 0x19);
    }
}

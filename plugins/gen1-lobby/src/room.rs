//! The room: a block map built at run time and written into unsaved WRAM.
//!
//! # Why this is the block map and not the screen buffer
//!
//! An earlier scout painted `wTileMap` — the 20x18 buffer the screen and the
//! collision test both read — every frame. That works and it has two costs: it
//! races the engine's own scroll redraw (visible as diagonal streaks of the old
//! room), and it has to be redone on every frame for ever.
//!
//! Writing `wOverworldMap` instead does not race anything, because the engine
//! is then the only writer of the screen buffer. `RedrawRowOrColumn` draws each
//! row and column that scrolls into view **out of the blocks we wrote**, so the
//! walking, the four-frame walk cycle, the collision, the bump sound, the
//! camera and the scroll are all the cartridge's, operating on our data.
//!
//! # Why the vanilla corner is preserved exactly
//!
//! The two consoles are hidden events at walk tiles (4,4) and (5,4), keyed to
//! the map id in ROM (`data/events/hidden_events.asm`). They cannot be moved.
//! So the room keeps `maps/TradeCenter.blk` at block (0,0) and grows right and
//! down from it: the consoles, both seats and the walls immediately around them
//! are byte-identical to the cartridge's, which is what keeps a trade startable.
//!
//! It also means **the first frame after installation is already correct** and
//! no screen paint is needed at all: every block the player can currently see
//! is the block that was there a moment ago. The hall only exists where they
//! have not looked yet, and the engine paints it as they walk.

use crate::gen1::*;
use terminalgb_plugin_api::Machine;

/// A room, measured in blocks. One block is 2x2 walk tiles is 4x4 screen tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Room {
    pub w: usize,
    pub h: usize,
}

/// The recommended first version: four screens by three
/// (`data/gameboy-worldlobby-w4/report.md` §9). Big enough to feel like a
/// place, small enough to bump into people.
pub const DEFAULT_ROOM: Room = Room { w: 20, h: 14 };

/// Why a room was refused. **A room bigger than the buffer is not a room, it is
/// a crash**, so the footprint is checked before a byte is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoomError {
    /// `(W+6)·(H+6)` exceeded `wOverworldMap`'s 1300 bytes.
    TooBigForBuffer { need: usize, have: usize },
    /// A walk coordinate is a `u8`, so neither axis may exceed 127 blocks.
    AxisTooLong,
    /// Smaller than the vanilla Trade Center, which would clip the consoles.
    SmallerThanVanilla,
}

impl Room {
    pub const fn new(w: usize, h: usize) -> Room {
        Room { w, h }
    }

    /// Bytes of `wOverworldMap` this room needs, border included.
    pub const fn footprint(&self) -> usize {
        (self.w + MAP_BORDER * 2) * (self.h + MAP_BORDER * 2)
    }

    pub const fn stride(&self) -> usize {
        self.w + MAP_BORDER * 2
    }

    /// Room extent in walk tiles.
    pub const fn walk_size(&self) -> (usize, usize) {
        (self.w * 2, self.h * 2)
    }

    pub fn check(&self) -> Result<(), RoomError> {
        if self.w < TRADE_CENTER_W || self.h < TRADE_CENTER_H {
            return Err(RoomError::SmallerThanVanilla);
        }
        if self.w > 127 || self.h > 127 {
            return Err(RoomError::AxisTooLong);
        }
        let need = self.footprint();
        if need > OVERWORLD_MAP_LEN {
            return Err(RoomError::TooBigForBuffer {
                need,
                have: OVERWORLD_MAP_LEN,
            });
        }
        Ok(())
    }

    /// The room's own `w * h` block ids, in row order.
    ///
    /// Three rules, in this order, and the order is the design:
    ///
    /// 1. floor everywhere;
    /// 2. the cartridge's own Trade Center at block (0,0), so the consoles and
    ///    the seats are untouched;
    /// 3. a ring of wall all round. **There is no map boundary in the collision
    ///    check** — one tile of `wTileMap` is the whole test
    ///    (`engine/overworld/player_state.asm`) — so an unringed room lets the
    ///    player walk out of the world and `wYCoord` underflows with no fault
    ///    raised.
    pub fn blocks(&self) -> Vec<u8> {
        let mut m = vec![CLUB_FLOOR_BLOCK; self.w * self.h];
        for r in 0..TRADE_CENTER_H {
            for c in 0..TRADE_CENTER_W {
                m[r * self.w + c] = TRADE_CENTER_BLK[r * TRADE_CENTER_W + c];
            }
        }
        for c in 0..self.w {
            m[c] = CLUB_WALL_BLOCK;
            m[(self.h - 1) * self.w + c] = CLUB_WALL_BLOCK;
        }
        for r in 0..self.h {
            m[r * self.w] = CLUB_WALL_BLOCK;
            m[r * self.w + self.w - 1] = CLUB_WALL_BLOCK;
        }
        m
    }

    /// `wCurrentTileBlockMapViewPointer` for a player standing at walk tile
    /// `(x, y)`. The cartridge's own formula, from `event_displacement`
    /// (`macros/coords.asm`): the top-left of the visible block window, which
    /// sits two blocks up and two blocks left of the player's own block.
    pub fn view_pointer(&self, x: u8, y: u8) -> u16 {
        W_OVERWORLD_MAP
            + 7
            + self.w as u16
            + (self.stride() as u16) * (y as u16 >> 1)
            + (x as u16 >> 1)
    }

    /// Write the room into the machine. Returns the number of bytes written.
    ///
    /// **Everything here except the last five bytes is below `wMainDataStart`
    /// and can never reach a save file.** The five that are above it — the two
    /// dimension bytes, the two view-pointer bytes and the map id we leave
    /// alone — are the caller's responsibility to gate on
    /// `LINK_STATE_IN_CABLE_CLUB`; [`crate::LobbyPlugin`] does that.
    pub fn install(&self, m: &mut Machine<'_>) -> Result<usize, RoomError> {
        self.check()?;
        let blocks = self.blocks();
        let stride = self.stride();
        let mut wrote = 0usize;

        // The whole bordered footprint, wall first so nothing stale shows
        // through the three-block border the engine reads past the edges.
        for i in 0..self.footprint() {
            m.poke(W_OVERWORLD_MAP + i as u16, CLUB_WALL_BLOCK);
            wrote += 1;
        }
        for r in 0..self.h {
            for c in 0..self.w {
                let at = (MAP_BORDER + r) * stride + MAP_BORDER + c;
                m.poke(W_OVERWORLD_MAP + at as u16, blocks[r * self.w + c]);
            }
        }

        // Both dimension pairs. Setting only the block pair leaves the engine
        // believing the room is its old size, and the player walks off the edge
        // into map $00 with no fault raised — measured by the scout, once.
        m.poke(W_CUR_MAP_WIDTH, self.w as u8);
        m.poke(W_CUR_MAP_HEIGHT, self.h as u8);
        m.poke(W_CUR_MAP_WIDTH_2, (self.w * 2) as u8);
        m.poke(W_CUR_MAP_HEIGHT_2, (self.h * 2) as u8);

        let (x, y) = (
            m.peek(W_X_COORD).unwrap_or(0),
            m.peek(W_Y_COORD).unwrap_or(0),
        );
        let vp = self.view_pointer(x, y);
        m.poke(W_VIEW_POINTER, vp as u8);
        m.poke(W_VIEW_POINTER + 1, (vp >> 8) as u8);
        wrote += 8;
        Ok(wrote)
    }

    /// Is this machine currently running our room rather than the cartridge's?
    ///
    /// Cheap enough to ask every frame, which is the point: three engine paths
    /// restore the cartridge's own header over ours — `ReloadMapData`,
    /// `ReloadTilesetTilePatterns` and `ReturnToCableClubRoom` — and the
    /// honest response is to notice and re-install rather than to assume.
    pub fn is_installed(&self, m: &mut Machine<'_>) -> bool {
        m.peek(W_CUR_MAP_WIDTH) == Some(self.w as u8)
            && m.peek(W_CUR_MAP_HEIGHT) == Some(self.h as u8)
            && m.peek(W_CUR_MAP_WIDTH_2) == Some((self.w * 2) as u8)
    }

    /// Read the room's blocks back out of WRAM, for a test or a report.
    pub fn read_back(&self, m: &mut Machine<'_>) -> Vec<u8> {
        let stride = self.stride();
        let mut out = Vec::with_capacity(self.w * self.h);
        for r in 0..self.h {
            for c in 0..self.w {
                let at = (MAP_BORDER + r) * stride + MAP_BORDER + c;
                out.push(m.peek(W_OVERWORLD_MAP + at as u16).unwrap_or(0));
            }
        }
        out
    }
}

/// Is a walk tile inside this room and not one of the two console squares?
pub fn walkable_square(room: &Room, x: u8, y: u8) -> bool {
    let (w, h) = room.walk_size();
    // The wall ring is one block — two walk tiles — thick on every side.
    (2..w - 2).contains(&(x as usize))
        && (2..h - 2).contains(&(y as usize))
        && !CONSOLE_TILES.contains(&(x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_room_fits_the_buffer_with_room_to_spare() {
        assert_eq!(DEFAULT_ROOM.check(), Ok(()));
        assert_eq!(DEFAULT_ROOM.footprint(), 26 * 20);
        assert!(DEFAULT_ROOM.footprint() < OVERWORLD_MAP_LEN);
        assert_eq!(DEFAULT_ROOM.walk_size(), (40, 28));
    }

    #[test]
    fn a_room_bigger_than_the_buffer_is_refused_before_a_byte_is_written() {
        // Route 23, the cartridge's biggest map, needs 1248 of the 1300.
        assert_eq!(Room::new(10, 72).check(), Ok(()));
        assert!(matches!(
            Room::new(31, 31).check(),
            Err(RoomError::TooBigForBuffer { .. })
        ));
        assert_eq!(Room::new(200, 4).check(), Err(RoomError::AxisTooLong));
        assert_eq!(
            Room::new(4, 4).check(),
            Err(RoomError::SmallerThanVanilla)
        );
    }

    #[test]
    fn the_console_block_is_the_cartridges_own() {
        let m = DEFAULT_ROOM.blocks();
        // Walk tiles (4,4)/(5,4) both live in block (2,2), and that block must
        // be TradeCenter.blk's, or the hidden events sit on our floor.
        assert_eq!(m[2 * DEFAULT_ROOM.w + 2], TRADE_CENTER_BLK[2 * 5 + 2]);
        // The two seats, likewise.
        assert_eq!(m[2 * DEFAULT_ROOM.w + 1], TRADE_CENTER_BLK[2 * 5 + 1]);
        assert_eq!(m[2 * DEFAULT_ROOM.w + 3], TRADE_CENTER_BLK[2 * 5 + 3]);
    }

    #[test]
    fn the_room_is_ringed_with_wall_on_every_side() {
        let r = DEFAULT_ROOM;
        let m = r.blocks();
        for c in 0..r.w {
            assert_eq!(m[c], CLUB_WALL_BLOCK, "top edge at {c}");
            assert_eq!(m[(r.h - 1) * r.w + c], CLUB_WALL_BLOCK, "bottom at {c}");
        }
        for row in 0..r.h {
            assert_eq!(m[row * r.w], CLUB_WALL_BLOCK, "left edge at {row}");
            assert_eq!(m[row * r.w + r.w - 1], CLUB_WALL_BLOCK, "right at {row}");
        }
        // …and the middle is our floor, not the cartridge's.
        assert_eq!(m[7 * r.w + 10], CLUB_FLOOR_BLOCK);
    }

    #[test]
    fn the_view_pointer_matches_the_cartridges_own_formula() {
        // Vanilla: 5 wide, player at the internal-clock seat (3,4).
        let vanilla = Room::new(5, 4);
        let got = vanilla.view_pointer(SEAT_INTERNAL.0, SEAT_INTERNAL.1);
        assert_eq!(got, W_OVERWORLD_MAP + 7 + 5 + 11 * 2 + 1);
        // …which is the block-map address of the block two up and two left of
        // the player's own, i.e. `owcoord(bx - 2, by - 2)`.
        // Signed on purpose: with the player near an edge the window starts
        // inside the three-block border, which is exactly what the border is
        // for.
        let (bx, by) = (3i32 >> 1, 4i32 >> 1);
        let owcoord =
            |x: i32, y: i32| (W_OVERWORLD_MAP as i32 + (x + 3) + (y + 3) * (5 + 6)) as u16;
        assert_eq!(got, owcoord(bx - 2, by - 2));
    }

    #[test]
    fn the_wall_ring_is_two_walk_tiles_thick_and_the_consoles_are_never_free() {
        let r = DEFAULT_ROOM;
        assert!(!walkable_square(&r, 1, 6));
        assert!(!walkable_square(&r, 6, 1));
        assert!(walkable_square(&r, 2, 6));
        assert!(walkable_square(&r, 3, 4));
        for (x, y) in CONSOLE_TILES {
            assert!(!walkable_square(&r, x, y), "console ({x},{y})");
        }
        let (w, h) = r.walk_size();
        assert!(!walkable_square(&r, (w - 2) as u8, 6));
        assert!(!walkable_square(&r, 6, (h - 2) as u8));
    }
}

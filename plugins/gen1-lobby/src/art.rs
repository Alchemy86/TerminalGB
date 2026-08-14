//! Our own tile art, written straight into VRAM.
//!
//! This is the part of the room that is unambiguously ours. Everything else
//! rearranges blocks the cartridge already loaded; these are pixels drawn in
//! this file, in this repository, and they replace someone else's rather than
//! borrowing them. That is also why the art route is the clean one: no
//! cartridge bitmap is copied, extracted or redistributed — three tiles are
//! overwritten in the user's own emulator, at run time, on the user's own
//! cartridge.
//!
//! # Where it may be written, and where it may not
//!
//! | Range | What lives there |
//! |---|---|
//! | `$8000-$87FF` | ten walking sprite slots plus two still ones |
//! | `$8800-$8FFF` | **the font** — BG ids `$80-$FF` address here, which is how text is drawn |
//! | `$9000-$95FF` | `vTileset`: 96 background tiles, **ours to redefine** |
//! | `$9600-$97FF` | the text box (`LoadTextBoxTilePatterns`) |
//!
//! We touch three tiles of the 96 and nothing else.
//!
//! # Reload discipline
//!
//! Three engine paths restore the cartridge's tileset over ours —
//! `ReloadMapData` (closing a menu, the Pokédex, a PC, using an item),
//! `ReloadTilesetTilePatterns` (after a battle) and `ReturnToCableClubRoom`
//! (after a trade). The honest response is to notice and re-blit rather than to
//! assume, which is what [`Floor::is_installed`] is for: it is a single VRAM
//! read per frame and it costs no emulated cycles at all.

use crate::gen1::*;
use terminalgb_plugin_api::Machine;

/// Turn eight rows of `0`-`3` into a Game Boy 2bpp tile: two bit planes,
/// low byte then high byte, one pair per row. Written this way so the art in
/// this file can be read as art.
const fn tile(rows: [&str; 8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut r = 0;
    while r < 8 {
        let bytes = rows[r].as_bytes();
        let (mut lo, mut hi) = (0u8, 0u8);
        let mut c = 0;
        while c < 8 {
            let v = bytes[c] - b'0';
            lo |= (v & 1) << (7 - c);
            hi |= ((v >> 1) & 1) << (7 - c);
            c += 1;
        }
        out[r * 2] = lo;
        out[r * 2 + 1] = hi;
        r += 1;
    }
    out
}

/// The floor's two tiles. `Club_Coll` lists both `$0f` and `$1f` as walkable
/// and the CLUB floor block alternates them in a checkerboard, so redefining
/// the pair changes the floor of the whole room — the vanilla corner included —
/// without touching collision at all.
pub const FLOOR_TILE_A: u8 = 0x0F;
pub const FLOOR_TILE_B: u8 = 0x1F;
/// The wall block `$0E` is solid `$19` throughout.
pub const WALL_TILE: u8 = 0x19;

/// A tiled floor: a one-pixel grout line along two edges so the 8x8 grid reads
/// as flooring, and a stud in the middle of every other tile.
const FLOOR_A: [u8; 16] = tile([
    "11111111", //
    "10000000", //
    "10000000", //
    "10000000", //
    "10000000", //
    "10000000", //
    "10000000", //
    "10000000", //
]);

const FLOOR_B: [u8; 16] = tile([
    "11111111", //
    "10000000", //
    "10000000", //
    "10002200", //
    "10002200", //
    "10000000", //
    "10000000", //
    "10000000", //
]);

/// Panelled wall: two courses, dark mortar, a lighter face.
const WALL: [u8; 16] = tile([
    "33333333", //
    "32222223", //
    "32222223", //
    "33333333", //
    "22222222", //
    "23333332", //
    "23333332", //
    "22222222", //
]);

/// The room's art, as ids paired with pixels.
pub struct Floor;

impl Floor {
    pub const TILES: [(u8, [u8; 16]); 3] = [
        (FLOOR_TILE_A, FLOOR_A),
        (FLOOR_TILE_B, FLOOR_B),
        (WALL_TILE, WALL),
    ];

    fn address(id: u8) -> u16 {
        V_TILESET + id as u16 * TILE_BYTES
    }

    /// Blit the art. Returns the number of bytes written.
    pub fn install(m: &mut Machine<'_>) -> usize {
        let mut wrote = 0;
        for (id, data) in Self::TILES {
            debug_assert!(id < MAP_TILESET_SIZE, "art must stay inside vTileset");
            let at = Self::address(id);
            for (i, b) in data.iter().enumerate() {
                m.poke(at + i as u16, *b);
                wrote += 1;
            }
        }
        wrote
    }

    /// Is our art still the art? One read is enough — the engine's reload paths
    /// restore the whole tileset in one go, never half of it.
    pub fn is_installed(m: &mut Machine<'_>) -> bool {
        Self::TILES.iter().all(|(id, data)| {
            let at = Self::address(*id);
            m.peek(at) == Some(data[0]) && m.peek(at + 15) == Some(data[15])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_encoder_produces_the_two_bit_planes_the_hardware_reads() {
        let t = tile([
            "01230123", //
            "00000000", //
            "00000000", //
            "00000000", //
            "00000000", //
            "00000000", //
            "00000000", //
            "00000000", //
        ]);
        // low plane: bit set where the colour is 1 or 3 -> 0b0101_0101
        assert_eq!(t[0], 0b0101_0101);
        // high plane: bit set where the colour is 2 or 3 -> 0b0011_0011
        assert_eq!(t[1], 0b0011_0011);
        assert_eq!(&t[2..], &[0u8; 14]);
    }

    #[test]
    fn every_tile_we_redefine_is_inside_the_background_tileset() {
        // Past $60 is the text box; past $80 is the font. Overwrite either and
        // dialogue breaks — including the trade's own "Trade completed!".
        for (id, _) in Floor::TILES {
            assert!(id < MAP_TILESET_SIZE, "{id:#04x} is outside vTileset");
            let at = Floor::address(id);
            assert!(at >= V_TILESET);
            assert!(at + 15 < 0x9600, "{id:#04x} reaches the text box");
        }
    }

    #[test]
    fn redefining_the_floor_cannot_change_where_a_player_may_walk() {
        // Collision is by tile *id* against `Club_Coll`, never by pixels, so
        // the art is free: both floor ids stay walkable and the wall id stays
        // solid however they are drawn.
        assert!(CLUB_WALKABLE.contains(&FLOOR_TILE_A));
        assert!(CLUB_WALKABLE.contains(&FLOOR_TILE_B));
        assert!(!CLUB_WALKABLE.contains(&WALL_TILE));
    }
}

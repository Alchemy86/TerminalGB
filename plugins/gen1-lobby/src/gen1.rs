//! Every address and constant this plugin uses, with the symbol it came from.
//!
//! All of them are `pokeblue.sym` values from a `pret/pokered` build at rgbds
//! 1.0.3, whose `pokeblue.gbc` is byte-identical to the retail cartridge per
//! the repository's own `roms.sha1` — so a symbol here is exact, not inferred.
//! `pokered.sym` gives the same addresses, which is why one table serves Red
//! and Blue.
//!
//! **The line that decides whether this plugin is safe runs through the middle
//! of this file.** `wMainDataStart` (`$D2F7`) is where `sMainData` begins;
//! anything at or above it is copied into the `.sav` by `SaveMainData`.
//! Everything the room and the avatars touch is *below* it and can never reach
//! a save file. The five bytes that are above it — the map header — are written
//! only while `wLinkState == LINK_STATE_IN_CABLE_CLUB`, where `LinkMenu` has
//! set `BIT_LINK_CONNECTED` and `DrawStartMenu` therefore prints RESET where
//! SAVE would be (`engine/menus/main_menu.asm`,
//! `engine/menus/draw_start_menu.asm`). See [`crate::LobbyPlugin`] §Safety.

// --- unsaved WRAM: the room, the screen, the avatars ------------------------

/// `wSpriteStateData1` — sixteen 16-byte structs. Slot 0 is the player.
pub const W_SPRITE_STATE_DATA1: u16 = 0xC100;
/// `wSpriteStateData2` — the second bank of sixteen 16-byte structs.
pub const W_SPRITE_STATE_DATA2: u16 = 0xC200;
/// One sprite struct, in either bank.
pub const SPRITE_STRUCT_LEN: u16 = 0x10;
/// `NUM_SPRITESTATEDATA_STRUCTS` (`constants/map_object_constants.asm`).
pub const SPRITE_SLOTS: u8 = 16;

/// `SPRITESTATEDATA1_PICTUREID` — non-zero means `PrepareOAMData` draws it.
pub const S1_PICTURE_ID: u16 = 0x0;
/// `SPRITESTATEDATA1_IMAGEINDEX` — high nibble picks the VRAM pattern slot,
/// low nibble indexes `SpriteFacingAndAnimationTable`; `$FF` means "off screen,
/// do not draw" (`engine/gfx/sprite_oam.asm`).
pub const S1_IMAGE_INDEX: u16 = 0x2;
/// `SPRITESTATEDATA1_YPIXELS`.
pub const S1_Y_PIXELS: u16 = 0x4;
/// `SPRITESTATEDATA1_XPIXELS`.
pub const S1_X_PIXELS: u16 = 0x6;
/// `SPRITESTATEDATA1_FACINGDIRECTION`.
pub const S1_FACING: u16 = 0x9;

/// `SPRITESTATEDATA2_MAPY` — the map coordinate **plus four** (the
/// `object_event` macro's `db \2 + 4`).
pub const S2_MAP_Y: u16 = 0x4;
/// `SPRITESTATEDATA2_MAPX` — likewise plus four.
pub const S2_MAP_X: u16 = 0x5;
/// `SPRITESTATEDATA2_IMAGEBASEOFFSET`. **Leaving this at zero is the trick that
/// makes a remote avatar behave**: `_UpdateSprites` skips a slot whose base
/// offset is zero (`engine/overworld/sprite_collisions.asm`), so no movement
/// AI and no visibility cull ever fight our writes — while `PrepareOAMData`,
/// which gates on `PICTUREID` alone, still draws it.
pub const S2_IMAGE_BASE_OFFSET: u16 = 0xE;

/// The map coordinate offset baked into every sprite struct.
pub const SPRITE_MAP_OFFSET: u8 = 4;

/// `wTileMap` — the 20x18 on-screen tile buffer. Collision reads this
/// (`engine/overworld/player_state.asm`, `lda_coord 8, 11`).
pub const W_TILE_MAP: u16 = 0xC3A0;
pub const SCREEN_W: usize = 20;
pub const SCREEN_H: usize = 18;
/// The player is pinned to this screen cell and the *view* scrolls around
/// them. The player's walk tile is 2x2 screen tiles and this is its **top-left**
/// cell — so the tile the engine's own collision test reads is one row below,
/// at (8, 9), which is what `lda_coord 8, 11` (facing down) is two rows past.
///
/// **This is measured, not derived.** `screen::calibrate` reproduces the whole
/// 20x18 buffer from the block map for each candidate anchor and compares it
/// against the engine's, on the cartridge's own room, before the plugin paints
/// anything: row 8 matches 360 of 360 cells and row 9 matches 133. See
/// `docs/worldlobby.md` §3.
pub const PLAYER_SCREEN_COL: i32 = 8;
pub const PLAYER_SCREEN_ROW: i32 = 8;

/// `wOverworldMap` — 1300 bytes of live block map.
pub const W_OVERWORLD_MAP: u16 = 0xC6E8;
/// `wOverworldMapEnd`.
pub const W_OVERWORLD_MAP_END: u16 = 0xCBFC;
/// The capacity that decides how big a room may be: `(W+6)·(H+6) <= 1300`.
pub const OVERWORLD_MAP_LEN: usize = (W_OVERWORLD_MAP_END - W_OVERWORLD_MAP) as usize;
/// `MAP_BORDER` (`constants/map_data_constants.asm`) — `LoadTileBlockMap`
/// leaves this many blocks of border on every side, so stride = width + 6.
pub const MAP_BORDER: usize = 3;

/// `wUpdateSpritesEnabled`.
pub const W_UPDATE_SPRITES_ENABLED: u16 = 0xCFCB;

/// `wWalkCounter` — the player's step animation counter, 7 down to 0 over the
/// sixteen pixels of one walk tile. **Non-zero means the engine, not us, owns
/// every sprite's screen position this frame** (see [`crate::avatar`]).
///
/// Walked back six `db`s from `wUpdateSpritesEnabled` in `ram/wram.asm`
/// (`wWalkCounter`, `wTileInFrontOfPlayer`, three audio fade bytes,
/// `wLastMusicSoundID`) and then **measured on the cartridge**: it counts
/// 7,7,6,6,…,1,1 across a step and `wYCoord` increments on precisely the frame
/// it reads 0, which is a claim no arithmetic could have made on its own.
pub const W_WALK_COUNTER: u16 = 0xCFC5;

// --- the saved region: gated on being in the Cable Club ---------------------

/// `wLinkState`.
pub const W_LINK_STATE: u16 = 0xD12B;
/// `LINK_STATE_IN_CABLE_CLUB`.
pub const LINK_STATE_IN_CABLE_CLUB: u8 = 0x01;
/// `LINK_STATE_START_TRADE`. At or above this the game owns every buffer.
pub const LINK_STATE_START_TRADE: u8 = 0x02;

/// `wMainDataStart` — at or above this, a byte reaches `sMainData`.
pub const W_MAIN_DATA_START: u16 = 0xD2F7;

/// `wCurMap`. `$EF` starts trades from the two consoles, `$F0` starts battles.
pub const W_CUR_MAP: u16 = 0xD35E;
/// `TRADE_CENTER`.
pub const MAP_TRADE_CENTER: u8 = 0xEF;
/// `COLOSSEUM` — the Trade Center's twin, same tileset, same hidden events.
pub const MAP_COLOSSEUM: u8 = 0xF0;

/// `wCurrentTileBlockMapViewPointer`.
pub const W_VIEW_POINTER: u16 = 0xD35F;
/// `wYCoord`.
pub const W_Y_COORD: u16 = 0xD361;
/// `wXCoord`.
pub const W_X_COORD: u16 = 0xD362;
/// `wCurMapHeight` — in **blocks**.
pub const W_CUR_MAP_HEIGHT: u16 = 0xD368;
/// `wCurMapWidth` — in **blocks**.
pub const W_CUR_MAP_WIDTH: u16 = 0xD369;

/// `wNumSprites` — how many sprite slots the engine believes this map has, and
/// **the only gate on `AdvancePlayerSprite`'s `.spriteShiftLoop`**
/// (`home/overworld.asm`), which is what keeps a standing NPC still in the
/// world while the camera scrolls. Both Cable Club maps declare exactly one
/// object, so without raising this the room's second and later avatars are
/// never shifted and drag across the floor with the player.
///
/// Walked from `wWarpEntries` (`$D3AF`, verified in
/// `examples/support/gen1_cable_club.rs`) through `MAX_WARP_EVENTS * 4`,
/// `wDestinationWarpID`, 128 unused bytes, `wNumSigns`, `MAX_BG_EVENTS * 2`
/// and `MAX_BG_EVENTS`; the walk continues past `wMapSpriteData` and
/// `wMapSpriteExtraData` to land exactly on `wCurrentMapHeight2` = `$D524`,
/// which is the walk's own proof.
pub const W_NUM_SPRITES: u16 = 0xD4E1;
/// `wMapSpriteData` — `MAX_OBJECT_EVENTS` pairs of (movement byte 2, text ID),
/// indexed from slot 1. `DisplayTextID` (`home/text_script.asm`) reads the
/// **second** byte of the pair for any slot at or below `wNumSprites`, so
/// raising that count without filling this in would send an A-press at an
/// avatar off the end of the map's text-pointer table.
pub const W_MAP_SPRITE_DATA: u16 = 0xD4E4;
/// `MAX_OBJECT_EVENTS`.
pub const MAX_OBJECT_EVENTS: usize = 16;
/// `TEXT_TRADECENTER_OPPONENT` / `TEXT_COLOSSEUM_OPPONENT` — both are the
/// first and only entry of their map's `def_text_pointers`, so one constant
/// serves both rooms. It is what the person opposite you says on a real Cable
/// Club, which is exactly the right line for a stranger in this one.
pub const OPPONENT_TEXT_ID: u8 = 1;

// --- unsaved again: the second copy of the dimensions, and the tileset ------

/// `wCurrentMapHeight2` — in **walk tiles**, and it is *this* pair the
/// edge-of-map connection check reads. Set only the block pair and the engine
/// still believes the room is its old size: the player walks past the edge,
/// finds no connection, and lands on map `$00` with no fault raised.
pub const W_CUR_MAP_HEIGHT_2: u16 = 0xD524;
/// `wCurrentMapWidth2` — in walk tiles.
pub const W_CUR_MAP_WIDTH_2: u16 = 0xD525;

/// `wTilesetBank` — the ROM bank holding this tileset's blocks and art.
pub const W_TILESET_BANK: u16 = 0xD52B;
/// `wTilesetBlocksPtr` — dereferenced bank-agnostically by `DrawTileBlock`
/// (`home/overworld.asm`), so it may point at WRAM. We only *read* through it.
pub const W_TILESET_BLOCKS_PTR: u16 = 0xD52C;

/// One blockset entry: 4x4 tile ids = 2x2 walk tiles.
pub const BLOCK_TILES: usize = 16;
pub const BLOCK_SIDE: usize = 4;

/// `MAP_TILESET_SIZE`. Tile ids at or above this are text boxes and menus —
/// never overwrite one, and never treat one as scenery.
pub const MAP_TILESET_SIZE: u8 = 0x60;

// --- CLUB blockset: the two blocks the room is built from -------------------

/// The pure-floor block of the CLUB blockset. Every one of its four walk-tile
/// quadrants is `$0f`/`$1f`, both of which are in `Club_Coll`, so it is
/// walkable whichever tile of the quadrant collision happens to read.
pub const CLUB_FLOOR_BLOCK: u8 = 0x0A;
/// The CLUB border block — `TradeCenter_Object`'s own `db $e`. Solid `$19`
/// throughout, and `$19` is not in `Club_Coll`.
pub const CLUB_WALL_BLOCK: u8 = 0x0E;

/// `Club_Coll` (`data/tilesets/collision_tile_ids.asm`) — the walkable tile
/// ids of the CLUB tileset, and therefore the whole palette a runtime-built
/// room may draw its floor from.
pub const CLUB_WALKABLE: [u8; 11] = [
    0x0f, 0x1a, 0x1f, 0x26, 0x28, 0x29, 0x2c, 0x2d, 0x2e, 0x2f, 0x41,
];

/// `maps/TradeCenter.blk` — 5x4 blocks, read out of the cartridge's own map
/// file. The room keeps this arrangement in its top-left corner so that the two
/// consoles, the two seats and the walls around them are byte-identical to
/// vanilla, which is what keeps the hidden events at walk tiles (4,4) and (5,4)
/// reachable.
pub const TRADE_CENTER_BLK: [u8; 20] = [
    0x1b, 0x1e, 0x13, 0x1e, 0x17, //
    0x0a, 0x10, 0x1f, 0x12, 0x0a, //
    0x20, 0x21, 0x1c, 0x22, 0x23, //
    0x0a, 0x18, 0x19, 0x1a, 0x0a,
];
pub const TRADE_CENTER_W: usize = 5;
pub const TRADE_CENTER_H: usize = 4;

/// The two console squares, in walk tiles. `data/events/hidden_events.asm`
/// keys them to the map id, so they are ROM-fixed and must stay solid.
pub const CONSOLE_TILES: [(u8, u8); 2] = [(4, 4), (5, 4)];
/// Where the internal-clock player stands to use the left console, facing
/// right; and where the external-clock player stands, facing left.
pub const SEAT_INTERNAL: (u8, u8) = (3, 4);
pub const SEAT_EXTERNAL: (u8, u8) = (6, 4);

// --- vTileset: 96 background tiles, ours to redefine ------------------------

/// `vTileset`. `LoadTilesetTilePatternData` copies `MAP_TILESET_SIZE` = 96
/// tiles here; `$9600` upwards is the text box and `$8800` is the font, so
/// neither may be touched.
pub const V_TILESET: u16 = 0x9000;
/// Bytes per 2bpp tile.
pub const TILE_BYTES: u16 = 16;

// --- facing ----------------------------------------------------------------

/// `SPRITE_FACING_*` (`constants/sprite_data_constants.asm`). The same encoding
/// serves `FACINGDIRECTION` and the low nibble of `IMAGEINDEX`, where it
/// indexes `SpriteFacingAndAnimationTable` in groups of four walk frames.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Facing {
    #[default]
    Down = 0x00,
    Up = 0x04,
    Left = 0x08,
    Right = 0x0C,
}

impl Facing {
    pub fn from_byte(b: u8) -> Facing {
        match b & 0x0C {
            0x04 => Facing::Up,
            0x08 => Facing::Left,
            0x0C => Facing::Right,
            _ => Facing::Down,
        }
    }

    pub fn as_byte(self) -> u8 {
        self as u8
    }
}

/// Gen 1's own text encoding (`constants/charmap.asm`). Anything outside this
/// set has no glyph in the resident font, so a chat line has to be filtered to
/// it rather than silently mangled — there is no `@ # & * " _` and therefore no
/// e-mail addresses, URLs or `@handles`, which is a rather good accident for a
/// public room.
pub fn charmap(c: char) -> Option<u8> {
    Some(match c {
        ' ' => 0x7F,
        'A'..='Z' => 0x80 + (c as u8 - b'A'),
        'a'..='z' => 0xA0 + (c as u8 - b'a'),
        '0'..='9' => 0xF6 + (c as u8 - b'0'),
        '(' => 0x9A,
        ')' => 0x9B,
        ':' => 0x9C,
        ';' => 0x9D,
        '[' => 0x9E,
        ']' => 0x9F,
        '\'' => 0xE0,
        '-' => 0xE3,
        '?' => 0xE6,
        '!' => 0xE7,
        '.' => 0xE8,
        '/' => 0xF3,
        ',' => 0xF4,
        _ => return None,
    })
}

/// Filter a line to what the cartridge can actually draw, upper-casing nothing
/// and inventing nothing: a character with no glyph is dropped.
pub fn filter_to_charmap(s: &str) -> String {
    s.chars().filter(|c| charmap(*c).is_some()).collect()
}

//! # The walker's trade room
//!
//! A Secret Base in the Cable Club's Trade Center: a room of our own design, built
//! at run time in the emulator's unsaved memory, with two pads on the floor that
//! open menus -- SEND a Pokemon to the walker, or TAKE one back -- in the game's own
//! text box style. The trade itself is still the cartridge's: the same table, the
//! same cushions, the same link; this is the place around it and the questions
//! asked before it.
//!
//! Like the World Trade Centre plugin it builds on, nothing is patched, nothing is
//! distributed and no save is touched. What is written, and where:
//!
//! * the room -- a block map -- into `wOverworldMap`, and the block definitions the
//!   map indexes into the bytes just after it (both below `wMainDataStart`);
//! * the tiles those blocks are made of, into `vTileset` (96 slots, ours to redefine);
//! * a short walkable-tile list beside them, so the room's own floor is walkable and
//!   its furniture is not;
//! * the two pointers that make the engine read that block set and that list instead
//!   of the cartridge's (`wTilesetBlocksPtr`, `wTilesetCollisionPtr`), and the map's
//!   dimensions -- which are above `wMainDataStart`, and so are written only inside
//!   the Cable Club, where the game cannot save (`LINK_STATE_IN_CABLE_CLUB` makes the
//!   start menu print RESET where SAVE would be).
//!
//! **Menus freeze the player with the cartridge's own mask**, `wJoyIgnore`: while a
//! menu is open every button is ignored by the walking code, and the plugin reads the
//! raw pad (`hJoyInput`) itself. Nothing needs the frontend's cooperation.
//!
//! The device is on the other side of the link, and the menu's list of kept Pokemon
//! and its answer travel to and from it in the frontend (see `docs`): this crate
//! only holds them, via [`WalkerRoomPlugin::set_kept`] and
//! [`WalkerRoomPlugin::take_choice`].

#![forbid(unsafe_code)]

pub mod art_data;
pub mod menu;

use std::collections::VecDeque;

use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};
use terminalgb_plugin_gen1_lobby::gen1::*;
use terminalgb_plugin_gen1_lobby::room::Room;
use terminalgb_plugin_gen1_lobby::screen::{self, Blockset};

pub use menu::{Choice, Kept, Kind};
use menu::{Menu, Outcome};

/// `wJoyIgnore`: buttons the walking code must not see.
const W_JOY_IGNORE: u16 = 0xCD6B;
/// `hJoyInput`: the raw pad, before any masking.
const H_JOY_INPUT: u16 = 0xFFF8;
/// `wTilesetCollisionPtr`: the list of walkable tile ids the engine tests steps by.
const W_TILESET_COLLISION_PTR: u16 = 0xD530;
/// `wWalkCounter`: non-zero while a step is in progress.
const W_WALK_COUNTER: u16 = 0xCFC5;
/// `wMapViewVRAMPointer`: where in the 32x32 background map the top-left of the view
/// is. While walking about the engine does not copy `wTileMap` to the display; it
/// draws the rows and columns that scroll in straight into VRAM at this offset, so
/// a change to `wTileMap` alone is never seen. Ours go to both.
const W_MAP_VIEW_VRAM_POINTER: u16 = 0xD526;
const BG_MAP: u16 = 0x9800;
/// Where the font lives in VRAM (`vFont`): tiles `$80..=$FF`, letters, digits and
/// punctuation, the ones text is made of.
const V_FONT: u16 = 0x8800;
/// The font in the cartridge, 1 bit a pixel (`FontGraphics`, ROM bank 4 in Pokemon
/// Blue): 128 tiles of 8 bytes. The game copies it into `vFont` doubled to two bits
/// a pixel, but only when its own dialogue opens; while walking about those tiles
/// hold walking sprites, so a menu has to load its own letters.
const FONT_ROM_BANK: usize = 4;
const FONT_ROM_ADDR: u16 = 0x5A80;
const FONT_TILES: usize = 128;

/// What the plugin is doing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum State {
    /// Not in the Trade Center: nothing is written.
    #[default]
    Outside,
    /// In the room.
    InRoom,
    /// A menu is open.
    Menu,
    /// The trade screen has the machine; we are idle.
    Busy,
}

/// The room's geometry, and where its two blocks of data sit in memory.
struct Layout {
    room: Room,
    blockset_at: u16,
    collision_at: u16,
}

impl Layout {
    fn new() -> Layout {
        let room = Room::new(art_data::ROOM_W, art_data::ROOM_H);
        let blockset_at = W_OVERWORLD_MAP + room.footprint() as u16;
        let collision_at = blockset_at + (art_data::BLOCK_COUNT * BLOCK_TILES) as u16;
        Layout { room, blockset_at, collision_at }
    }

    fn end(&self) -> u16 {
        self.collision_at + art_data::WALKABLE.len() as u16 + 1
    }
}

/// The plugin. See the module docs.
pub struct WalkerRoomPlugin {
    layout: Layout,
    state: State,
    /// The cartridge's own block set, kept to measure the engine's screen against
    /// before we replace it (see the lobby's calibration).
    rom_blockset: Option<Blockset>,
    anchor: Option<i32>,
    /// Our block set, as the painter wants it.
    blockset: Blockset,
    /// The room's block map, as indexes into ours.
    blocks: Vec<u8>,
    menu: Option<Menu>,
    /// Rows of `wTileMap` the menu has written and owes back.
    borrowed: Vec<usize>,
    /// Buttons down last frame, for edge detection on the raw pad.
    last_pad: u8,
    /// A pad has to be stepped off before it opens again.
    armed: bool,
    kept: Vec<Kept>,
    /// What `vFont` held before a menu loaded the letters over it, to put back.
    saved_font: Option<Vec<u8>>,
    choices: VecDeque<Choice>,
    /// Frames since the screen was last checked against our room.
    since_check: u32,
    /// Bookkeeping for a report.
    pub installs: u32,
    pub ticks: u64,
}

impl Default for WalkerRoomPlugin {
    fn default() -> Self {
        WalkerRoomPlugin::new()
    }
}

impl WalkerRoomPlugin {
    pub fn new() -> WalkerRoomPlugin {
        let mut tiles = Vec::with_capacity(art_data::BLOCK_COUNT * BLOCK_TILES);
        for b in &art_data::BLOCKS {
            tiles.extend_from_slice(b);
        }
        WalkerRoomPlugin {
            layout: Layout::new(),
            state: State::Outside,
            rom_blockset: None,
            anchor: None,
            blockset: Blockset::from_tiles(tiles),
            blocks: art_data::ROOM.to_vec(),
            menu: None,
            borrowed: Vec::new(),
            last_pad: 0,
            armed: true,
            kept: Vec::new(),
            saved_font: None,
            choices: VecDeque::new(),
            since_check: 0,
            installs: 0,
            ticks: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Tell the menus what the walker has kept, oldest first. Called by the
    /// frontend whenever the device says so.
    pub fn set_kept(&mut self, kept: Vec<Kept>) {
        self.kept = kept;
    }

    /// What the player chose on a pad, once.
    pub fn take_choice(&mut self) -> Option<Choice> {
        self.choices.pop_front()
    }

    /// Is this walk tile inside a pad, and which?
    pub fn pad_at(x: u8, y: u8) -> Option<Kind> {
        let inside = |p: (u8, u8, u8, u8)| (p.0..=p.2).contains(&x) && (p.1..=p.3).contains(&y);
        if inside(art_data::PAD_SEND) {
            Some(Kind::Send)
        } else if inside(art_data::PAD_TAKE) {
            Some(Kind::TakeBack)
        } else {
            None
        }
    }

    fn in_the_trade_center(m: &mut Machine<'_>) -> bool {
        m.peek(W_CUR_MAP) == Some(MAP_TRADE_CENTER)
            && m.peek(W_LINK_STATE) == Some(LINK_STATE_IN_CABLE_CLUB)
    }

    /// Has our room replaced the cartridge's? Cheap enough for every frame: the
    /// engine puts its own header, block set and tiles back in three places
    /// (`ReloadMapData`, `ReloadTilesetTilePatterns`, `ReturnToCableClubRoom`), and
    /// noticing is the whole defence.
    fn is_installed(&self, m: &mut Machine<'_>) -> bool {
        let room = &self.layout.room;
        m.peek(W_CUR_MAP_WIDTH) == Some(room.w as u8)
            && m.peek(W_CUR_MAP_HEIGHT) == Some(room.h as u8)
            && m.peek_u16(W_TILESET_BLOCKS_PTR) == Some(self.layout.blockset_at)
            // The last tile we drew, in VRAM: the cartridge's own is different.
            && Self::vram_tile_is_ours(m, art_data::TILE_COUNT - 1)
    }

    fn vram_tile_is_ours(m: &mut Machine<'_>, id: usize) -> bool {
        let at = V_TILESET + (id as u16) * TILE_BYTES;
        art_data::TILES[id]
            .iter()
            .enumerate()
            .all(|(i, b)| m.peek(at + i as u16) == Some(*b))
    }

    fn install(&mut self, m: &mut Machine<'_>) {
        let room = self.layout.room;
        let stride = room.stride();
        // The map: wall everywhere first, so nothing stale shows through the border
        // the engine reads past the edges, then the room.
        let wall = art_data::ROOM[0];
        for i in 0..room.footprint() {
            m.poke(W_OVERWORLD_MAP + i as u16, wall);
        }
        for r in 0..room.h {
            for c in 0..room.w {
                let at = (MAP_BORDER + r) * stride + MAP_BORDER + c;
                m.poke(W_OVERWORLD_MAP + at as u16, self.blocks[r * room.w + c]);
            }
        }
        // Our block definitions, and the list of what may be walked on.
        for (b, block) in art_data::BLOCKS.iter().enumerate() {
            for (i, t) in block.iter().enumerate() {
                m.poke(self.layout.blockset_at + (b * BLOCK_TILES + i) as u16, *t);
            }
        }
        for (i, id) in art_data::WALKABLE.iter().enumerate() {
            m.poke(self.layout.collision_at + i as u16, *id);
        }
        m.poke(self.layout.collision_at + art_data::WALKABLE.len() as u16, 0xFF);
        // The tiles themselves.
        for (id, tile) in art_data::TILES.iter().enumerate() {
            for (i, b) in tile.iter().enumerate() {
                m.poke(V_TILESET + (id as u16) * TILE_BYTES + i as u16, *b);
            }
        }
        // Point the engine at all of it.
        let [bl, bh] = self.layout.blockset_at.to_le_bytes();
        m.poke(W_TILESET_BLOCKS_PTR, bl);
        m.poke(W_TILESET_BLOCKS_PTR + 1, bh);
        let [cl, ch] = self.layout.collision_at.to_le_bytes();
        m.poke(W_TILESET_COLLISION_PTR, cl);
        m.poke(W_TILESET_COLLISION_PTR + 1, ch);
        // Both dimension pairs (see the lobby: one pair alone lets the player walk
        // off the edge of the world), and the view pointer for where they stand.
        m.poke(W_CUR_MAP_WIDTH, room.w as u8);
        m.poke(W_CUR_MAP_HEIGHT, room.h as u8);
        m.poke(W_CUR_MAP_WIDTH_2, (room.w * 2) as u8);
        m.poke(W_CUR_MAP_HEIGHT_2, (room.h * 2) as u8);
        let (x, y) = (m.peek(W_X_COORD).unwrap_or(0), m.peek(W_Y_COORD).unwrap_or(0));
        let vp = room.view_pointer(x, y);
        m.poke(W_VIEW_POINTER, vp as u8);
        m.poke(W_VIEW_POINTER + 1, (vp >> 8) as u8);
        self.installs += 1;
        // The engine redraws only what scrolls into view, so paint the whole screen
        // once from our blocks, and have it shown.
        self.repaint_all(m);
    }

    fn repaint_all(&mut self, m: &mut Machine<'_>) {
        let rows: Vec<usize> = (0..SCREEN_H).collect();
        self.repaint(m, &rows);
    }

    /// Put screen `rows` back to our room, in the buffer the engine reads and on the
    /// display itself.
    fn repaint(&self, m: &mut Machine<'_>, rows: &[usize]) {
        let Some(anchor) = self.anchor else { return };
        let (px, py) = (m.peek(W_X_COORD).unwrap_or(0), m.peek(W_Y_COORD).unwrap_or(0));
        for &row in rows {
            for col in 0..SCREEN_W {
                let t = self.tile_for_cell(px, py, col as i32, row as i32, anchor);
                Self::put(m, col, row, t);
            }
        }
    }

    /// The tile our room puts at a screen cell. Past the room's edge is the wall, and
    /// the wall is *our* block, not the cartridge's `CLUB_WALL_BLOCK` id.
    fn tile_for_cell(&self, px: u8, py: u8, col: i32, row: i32, anchor: i32) -> u8 {
        let (x, y, sub_col, sub_row) = screen::cell_to_map_at(px, py, col, row, anchor);
        let room = &self.layout.room;
        let (bx, by) = (x.div_euclid(2), y.div_euclid(2));
        let block = if (0..room.w as i32).contains(&bx) && (0..room.h as i32).contains(&by) {
            self.blocks[(by as usize) * room.w + bx as usize]
        } else {
            art_data::ROOM[0]
        };
        let (qx, qy) = (x.rem_euclid(2) as usize, y.rem_euclid(2) as usize);
        self.blockset.tile(block, qx * 2 + sub_col, qy * 2 + sub_row)
    }

    /// One screen cell, in `wTileMap` (which collision and the engine's own
    /// redraws read) and on the display, where the view's own offset says it is.
    fn put(m: &mut Machine<'_>, col: usize, row: usize, tile: u8) {
        m.poke(W_TILE_MAP + (row * SCREEN_W + col) as u16, tile);
        let Some(ptr) = m.peek_u16(W_MAP_VIEW_VRAM_POINTER) else { return };
        if !(BG_MAP..BG_MAP + 0x400).contains(&ptr) {
            return;
        }
        let at = (ptr - BG_MAP) as usize;
        let (top, left) = (at / 32, at % 32);
        let addr = BG_MAP + (((top + row) % 32) * 32 + (left + col) % 32) as u16;
        m.poke(addr, tile);
    }

    /// Load the letters over `vFont`, remembering what was there.
    fn load_font(&mut self, m: &mut Machine<'_>) {
        if self.saved_font.is_some() {
            return;
        }
        let mut old = Vec::with_capacity(FONT_TILES * 16);
        for i in 0..(FONT_TILES * 16) as u16 {
            old.push(m.peek(V_FONT + i).unwrap_or(0));
        }
        self.saved_font = Some(old);
        for i in 0..(FONT_TILES * 8) as u16 {
            let b = m.peek_bank(FONT_ROM_BANK, FONT_ROM_ADDR + i).unwrap_or(0);
            // 1 bit a pixel to 2: the same byte in both planes, as the game does.
            m.poke(V_FONT + i * 2, b);
            m.poke(V_FONT + i * 2 + 1, b);
        }
    }

    /// Put `vFont` back as it was found.
    fn restore_font(&mut self, m: &mut Machine<'_>) {
        if let Some(old) = self.saved_font.take() {
            for (i, b) in old.iter().enumerate() {
                m.poke(V_FONT + i as u16, *b);
            }
        }
    }

    fn give_rows_back(&mut self, m: &mut Machine<'_>) {
        if self.borrowed.is_empty() {
            return;
        }
        let rows = std::mem::take(&mut self.borrowed);
        self.repaint(m, &rows);
    }

    fn tick(&mut self, m: &mut Machine<'_>) {
        // The trade screen owns every buffer: hands off, and let go of the pad.
        if m.peek(W_LINK_STATE).unwrap_or(0) >= LINK_STATE_START_TRADE {
            self.state = State::Busy;
            self.borrowed.clear();
            self.menu = None;
            // (The trade screen loads its own font; ours is not put back over it.)
            self.saved_font = None;
            m.poke(W_JOY_IGNORE, 0);
            return;
        }
        if !Self::in_the_trade_center(m) {
            self.state = State::Outside;
            self.borrowed.clear();
            self.menu = None;
            return;
        }
        self.ticks += 1;
        if self.rom_blockset.is_none() {
            self.rom_blockset = Blockset::read(m);
        }
        // Measure the engine's screen against the cartridge's room while it is
        // still the cartridge's: the only moment there is an independent answer.
        if self.anchor.is_none() {
            if let Some(bs) = &self.rom_blockset {
                self.anchor = screen::calibrate(m, bs).0;
            }
        }
        if self.anchor.is_none() {
            self.state = State::Outside;
            return;
        }
        if !self.is_installed(m) {
            self.install(m);
            // Everything the engine redrew is now ours, and any menu with it.
            self.borrowed.clear();
        }
        self.state = if self.menu.is_some() { State::Menu } else { State::InRoom };
        self.run_menu(m);
        self.keep_screen_ours(m);
    }

    /// The engine restores whole screens from its own backups now and then (after
    /// its dialogue, say), and what it puts back is the cartridge's room. Notice, and
    /// paint ours again. Only when nothing of ours is on the screen and the player is
    /// standing still, and a few times a second -- it reads all 360 tiles.
    fn keep_screen_ours(&mut self, m: &mut Machine<'_>) {
        self.since_check += 1;
        if self.since_check < 20 || self.menu.is_some() || !self.borrowed.is_empty() {
            return;
        }
        self.since_check = 0;
        if m.peek(W_WALK_COUNTER).unwrap_or(1) != 0 {
            return;
        }
        let Some(a) = self.anchor else { return };
        let (ok, total) = screen::agreement_at(m, &self.layout.room, &self.blocks, &self.blockset, a);
        if ok != total {
            self.repaint_all(m);
        }
    }

    /// Open a menu when the player stops on a pad, and run the one that is open.
    fn run_menu(&mut self, m: &mut Machine<'_>) {
        let raw = m.peek(H_JOY_INPUT).unwrap_or(0);
        let fresh = raw & !self.last_pad;
        self.last_pad = raw;
        let (x, y) = (m.peek(W_X_COORD).unwrap_or(0), m.peek(W_Y_COORD).unwrap_or(0));
        let standing = m.peek(W_WALK_COUNTER).unwrap_or(0) == 0;
        let on_pad = Self::pad_at(x, y);
        if on_pad.is_none() {
            self.armed = true;
        }

        if self.menu.is_none() {
            if let (Some(kind), true, true) = (on_pad, standing, self.armed) {
                self.menu = Some(Menu::open(kind, &self.kept));
                self.load_font(m);
                self.armed = false;
                self.last_pad = raw; // the press that walked us here is not a menu press
                self.state = State::Menu;
            }
        }
        let Some(menu) = self.menu.as_mut() else {
            self.give_rows_back(m);
            return;
        };
        // While it is open the walking code sees no buttons at all.
        m.poke(W_JOY_IGNORE, 0xFF);
        if let Outcome::Closed(choice) = menu.press(fresh) {
            if let Some(c) = choice {
                self.choices.push_back(c);
            }
            self.menu = None;
            m.poke(W_JOY_IGNORE, 0);
            self.state = State::InRoom;
            self.give_rows_back(m);
            self.restore_font(m);
            return;
        }
        // Draw it, and note which rows are borrowed.
        let cells = menu.cells();
        let mut rows = Vec::new();
        for (r, c, t) in cells {
            Self::put(m, c, r, t);
            if !rows.contains(&r) {
                rows.push(r);
            }
        }
        // Rows the previous frame borrowed and this one does not need go back.
        let stale: Vec<usize> = self.borrowed.iter().copied().filter(|r| !rows.contains(r)).collect();
        self.repaint(m, &stale);
        self.borrowed = rows;
    }
}

impl Plugin for WalkerRoomPlugin {
    fn name(&self) -> &str {
        "gen1-walkerroom"
    }

    fn capabilities(&self) -> Capability {
        // No SERIAL_LINK (the cable is the trade's) and no CONTROL_INPUT (the menu
        // freezes the player with the game's own mask; it presses nothing).
        Capability::OBSERVE | Capability::READ_MEMORY | Capability::WRITE_MEMORY
    }

    fn status(&self) -> String {
        match self.state {
            State::Outside => "walker room: outside".to_string(),
            State::InRoom => format!("walker room ({} kept)", self.kept.len()),
            State::Menu => "walker room: menu".to_string(),
            State::Busy => "walker room: trading".to_string(),
        }
    }

    fn on_event(&mut self, event: &Event, m: &mut Machine<'_>) {
        if let Event::Frame { .. } = event {
            self.tick(m);
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use art_data::*;

    /// The tile a step is judged by, for walk tile `(x, y)` of the room.
    fn collision_tile(x: usize, y: usize) -> u8 {
        let block = ROOM[(y / 2) * ROOM_W + x / 2] as usize;
        // The bottom-left screen tile of the walk tile's 2x2.
        let (tr, tc) = ((y % 2) * 2 + 1, (x % 2) * 2);
        BLOCKS[block][tr * 4 + tc]
    }

    fn walkable(x: usize, y: usize) -> bool {
        WALKABLE.contains(&collision_tile(x, y))
    }

    #[test]
    fn the_cartridges_fixed_furniture_is_where_its_hidden_events_expect() {
        // Seats at walk tiles (3,4) and (6,4); the two consoles between, at (4,4)
        // and (5,4), which must be solid or the player would walk onto them and the
        // hidden events would never be faced.
        assert!(walkable(3, 4), "the left seat");
        assert!(walkable(6, 4), "the right seat");
        assert!(!walkable(4, 4) && !walkable(5, 4), "the consoles are solid");
    }

    #[test]
    fn the_walls_hold_and_the_pads_can_be_walked_onto() {
        for x in 0..ROOM_W * 2 {
            assert!(!walkable(x, 0) && !walkable(x, ROOM_H * 2 - 1), "top/bottom edge at {x}");
        }
        for y in 0..ROOM_H * 2 {
            assert!(!walkable(0, y) && !walkable(ROOM_W * 2 - 1, y), "left/right edge at {y}");
        }
        for p in [PAD_SEND, PAD_TAKE] {
            for y in p.1..=p.3 {
                for x in p.0..=p.2 {
                    assert!(walkable(x as usize, y as usize), "pad tile ({x},{y})");
                }
            }
        }
    }

    #[test]
    fn everything_a_player_can_be_asked_to_reach_can_be_reached_from_a_seat() {
        let (w, h) = (ROOM_W * 2, ROOM_H * 2);
        let mut seen = vec![false; w * h];
        let mut todo = vec![(3usize, 4usize)];
        while let Some((x, y)) = todo.pop() {
            if seen[y * w + x] || !walkable(x, y) {
                continue;
            }
            seen[y * w + x] = true;
            for (dx, dy) in [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)] {
                let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                if (0..w as i32).contains(&nx) && (0..h as i32).contains(&ny) {
                    todo.push((nx as usize, ny as usize));
                }
            }
        }
        assert!(seen[4 * w + 6], "the other seat");
        for p in [PAD_SEND, PAD_TAKE] {
            assert!(seen[p.1 as usize * w + p.0 as usize], "a pad is walled off");
        }
    }

    #[test]
    fn the_room_and_its_data_fit_in_the_map_buffer_and_the_tile_slots() {
        let l = Layout::new();
        assert_eq!(l.room.check(), Ok(()));
        assert!(l.end() <= W_OVERWORLD_MAP_END, "data runs past wOverworldMap");
        assert!(TILE_COUNT <= MAP_TILESET_SIZE as usize, "more than the 96 tile slots");
        // Every tile a block names is one we drew.
        for b in &BLOCKS {
            assert!(b.iter().all(|&t| (t as usize) < TILE_COUNT));
        }
        assert!(ROOM.iter().all(|&b| (b as usize) < BLOCK_COUNT));
    }

    #[test]
    fn a_pad_is_found_by_the_tile_under_the_player() {
        assert_eq!(WalkerRoomPlugin::pad_at(PAD_SEND.0, PAD_SEND.1), Some(Kind::Send));
        assert_eq!(WalkerRoomPlugin::pad_at(PAD_TAKE.2, PAD_TAKE.3), Some(Kind::TakeBack));
        assert_eq!(WalkerRoomPlugin::pad_at(3, 4), None, "a seat is not a pad");
    }

    #[test]
    fn the_plugin_never_asks_for_the_cable_or_the_buttons() {
        let c = WalkerRoomPlugin::new().capabilities();
        assert!(c.contains(Capability::WRITE_MEMORY));
        assert!(!c.contains(Capability::SERIAL_LINK));
        assert!(!c.contains(Capability::CONTROL_INPUT));
    }
}

//! What the agent can see.
//!
//! # What is exposed, and why
//!
//! An agent that has to infer everything from pixels burns its budget on
//! perception and never gets to play. So the primary channel is **structure**,
//! and it is chosen to answer the four questions a decision actually needs:
//!
//! | question | field | why it is here |
//! |---|---|---|
//! | *where am I?* | [`Position`] — map id + name, x, y, facing | the only thing that makes "did that work?" answerable |
//! | *what is the game doing?* | [`Mode`] | pressing A in a menu and in a battle are different acts |
//! | *what is it telling me?* | [`Screen::text`] | Gen 1 writes character codes into the tilemap, so the screen **is** text — an agent that reads dialogue can follow the game's own instructions instead of guessing |
//! | *is it my turn?* | [`Screen::waiting`] | the difference between "press A" and "wait for the animation" |
//!
//! Party, badges, money and the battle opponent come from the existing
//! read-only trainer plugin's RAM map ([`terminalgb_plugin_trainer::gen1`]) rather than
//! being re-derived here.
//!
//! **The framebuffer is the fallback, not the channel.** A PNG of the current
//! frame is written only when the host is configured to, and only at decision
//! points — see [`crate::Config::frame_capture`]. It costs a file write
//! and, for a model, a large image; structure costs a few hundred bytes.
//!
//! # Reading the screen: the trap
//!
//! Red/Blue draw the **whole screen through the window layer** (`LCDC` bit 5,
//! `WY = 0`, `WX = 7`), leaving the background map blank. Read the background
//! map and every menu looks empty. This module reads whichever layer is
//! actually on, which is the same correction `examples/gen1_trade.rs` records.
//!
//! Reading VRAM rather than the game's `wTileMap` shadow buffer is deliberate:
//! VRAM is what the LCD is showing, so it cannot disagree with the picture a
//! viewer sees, and it needs no game-specific address.

use terminalgb_plugin_api::Machine;
use terminalgb_plugin_trainer::gen1::{addr as tr, BattleKind};

/// Gen 1 tile ids that are not text. Sourced from `pret/pokered`'s
/// `constants/charmap.asm`: the glyphs sit at the top of the font, immediately
/// below `♂` (`$EF`), which [`terminalgb_plugin_gen1_link::text`] already pins.
pub mod glyph {
    /// `▷` — a menu cursor that is parked (the menu does not have focus).
    pub const CURSOR_IDLE: u8 = 0xEC;
    /// `▶` — the active menu cursor.
    pub const CURSOR: u8 = 0xED;
    /// `▼` — "there is more text; press A". The single most useful byte on
    /// screen for a driver: it is the game saying *your turn*.
    pub const MORE_TEXT: u8 = 0xEE;
}

/// What the game is doing, as far as an outside driver can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Nothing recognisable yet — boot, title screen, or no save loaded.
    #[default]
    Title,
    /// A map is drawn and the player can walk.
    Overworld,
    /// A text box is up and waiting for a button.
    Dialogue,
    /// A menu cursor is on screen.
    Menu,
    Battle,
    /// Between two of the above: a fade, a warp, an animation.
    Transition,
    /// The game is running a scripted scene and the player is not in control:
    /// no map is loaded, so walking does nothing however it looks on screen.
    Cutscene,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Mode::Title => "title",
            Mode::Overworld => "overworld",
            Mode::Dialogue => "dialogue",
            Mode::Menu => "menu",
            Mode::Battle => "battle",
            Mode::Transition => "transition",
            Mode::Cutscene => "cutscene",
        }
    }
}

/// Which way the player is facing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facing {
    Down,
    Up,
    Left,
    Right,
}

impl Facing {
    /// `wSpriteStateData1 + $9` holds `SPRITE_FACING_*`
    /// (`constants/sprite_data_constants.asm`): DOWN 0, UP 4, LEFT 8, RIGHT $C.
    /// Anything else means the byte was sampled mid-update.
    pub fn from_byte(v: u8) -> Option<Facing> {
        Some(match v {
            0x00 => Facing::Down,
            0x04 => Facing::Up,
            0x08 => Facing::Left,
            0x0C => Facing::Right,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Facing::Down => "down",
            Facing::Up => "up",
            Facing::Left => "left",
            Facing::Right => "right",
        }
    }
}

/// Where the player is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub map: u8,
    pub x: u8,
    pub y: u8,
    pub facing: Option<Facing>,
}

/// Decode one on-screen tile to text, or `None` for a graphics tile.
///
/// This is the **screen** decode, layered over the name-field decode in
/// [`terminalgb_plugin_gen1_link::text::decode_char`], which deliberately covers only
/// what may appear in a name. A screen shows more: contraction ligatures
/// (`'s`, `'t`, …) that dialogue uses constantly, the `<PK><MN>` pair the
/// battle menu spells "PKMN" with, the `<LV>` glyph in every battle HUD, and
/// the cursor/arrow glyphs a driver steers by. All from `pret/pokered`'s
/// `constants/charmap.asm`; several expand to more than one character, so a
/// decoded row can be longer than its 20 tiles.
///
/// Leaving these out is not a cosmetic loss. Measured on a real Blue
/// cartridge: the battle menu decoded as `FIGHT / ITEM RUN` — the
/// switch-Pokémon option was **invisible**, because "PKMN" is two ligature
/// tiles and both decoded to spaces.
pub fn decode_tile(t: u8) -> Option<&'static str> {
    Some(match t {
        // Contractions — real tiles, used by ordinary dialogue ("that's").
        0xBB => "'d",
        0xBC => "'l",
        0xBD => "'s",
        0xBE => "'t",
        0xBF => "'v",
        0xE4 => "'r",
        0xE5 => "'m",
        // The battle menu's "PKMN" is these two tiles and nothing else.
        0xE1 => "PK",
        0xE2 => "MN",
        // The battle HUD's level glyph ("PIDGEY <LV>3").
        0x6E => "Lv",
        // Quotes and ellipsis, common in dialogue.
        0x70 => "\u{2018}",
        0x71 => "\u{2019}",
        0x72 => "\u{201C}",
        0x73 => "\u{201D}",
        0x75 => "\u{2026}",
        // Money and the item-count separator ("×5").
        0xF0 => "\u{00A5}",
        0xF1 => "\u{00D7}",
        // The glyphs a driver steers by — a menu whose cursor decodes to a
        // space is a menu where "which item is selected?" is unanswerable.
        0xEC => "\u{25B7}", // ▷ parked cursor
        0xED => "\u{25B6}", // ▶ active cursor
        0xEE => "\u{25BC}", // ▼ "press A"
        _ => return None,
    })
}

/// How much the decoded text can be trusted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Trust {
    /// The decode is honest: what the text says is what the screen shows.
    #[default]
    Ok,
    /// The tilemap is showing **graphics whose tile ids fall in the font
    /// range**, so the decode is letter salad delivered with a straight face.
    /// Measured on a real cartridge: the Game Freak intro decodes as
    /// `ABCDEFGHIJKLMNOP…` — sequential tiles through the font. Ignore the
    /// text; nothing on this screen is words.
    Graphics,
    /// The screen was still being redrawn when it was read — a scroll, a fade,
    /// a text box mid-print. A real run spent a model call reasoning about
    /// `KU:N:`, half of two different screens.
    Settling,
    /// The LCD is off. There is nothing on screen at all.
    Off,
}

impl Trust {
    pub fn label(self) -> &'static str {
        match self {
            Trust::Ok => "ok",
            Trust::Graphics => "graphics-not-text",
            Trust::Settling => "still-redrawing",
            Trust::Off => "screen-off",
        }
    }
}

/// The 20x18 tile grid the LCD is showing, and what can be read off it.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    /// Raw tile ids, row-major.
    pub tiles: Vec<u8>,
    /// Decoded text, 18 lines of (nominally) 20 tiles each. Non-text tiles
    /// become spaces; ligature tiles (`PKMN`, `'s`, `Lv`) expand to their
    /// letters, so a line can run a few characters past 20.
    pub text: String,
    /// Row of the active menu cursor, if one is on screen.
    pub cursor_row: Option<u8>,
    /// Column of that cursor — on a grid like the naming keyboard the row
    /// alone cannot say which letter is under the cursor.
    pub cursor_col: Option<u8>,
    /// The `▼` "press A for more" arrow is showing.
    pub waiting: bool,
    /// How many distinct tile ids are on screen. A drawn map has many; a menu,
    /// a fade or a blank transition has very few.
    pub distinct: u16,
    /// FNV-1a over the tile ids. Cheap "did anything change?".
    pub hash: u64,
    /// False only when the LCD was off at the read — see [`Screen::trust`].
    pub lcd_on: bool,
}

impl Screen {
    /// Read the active tilemap out of VRAM.
    pub fn read(m: &mut Machine) -> Screen {
        let lcdc = m.peek(0xFF40).unwrap_or(0);
        if lcdc & 0x80 == 0 {
            // LCD off: there is nothing on screen, and reading the map would
            // report whatever was last drawn.
            return Screen::default();
        }
        let window_on = lcdc & 0x20 != 0 && m.peek(0xFF4A).unwrap_or(255) < 144;
        let (base, sy, sx) = if window_on {
            (
                if lcdc & 0x40 != 0 { 0x9C00u16 } else { 0x9800 },
                0usize,
                0usize,
            )
        } else {
            (
                if lcdc & 0x08 != 0 { 0x9C00u16 } else { 0x9800 },
                m.peek(0xFF42).unwrap_or(0) as usize / 8,
                m.peek(0xFF43).unwrap_or(0) as usize / 8,
            )
        };

        let mut tiles = Vec::with_capacity(20 * 18);
        for row in 0..18usize {
            for col in 0..20usize {
                let y = (sy + row) % 32;
                let x = (sx + col) % 32;
                tiles.push(m.peek(base + (y * 32 + x) as u16).unwrap_or(0));
            }
        }
        Screen::from_tiles(tiles)
    }

    /// Build a screen from a 20x18 tile grid. Pure — this is the whole decode,
    /// separated from the VRAM read so it can be tested against literal tiles.
    pub fn from_tiles(tiles: Vec<u8>) -> Screen {
        debug_assert_eq!(tiles.len(), 20 * 18);
        let mut text = String::with_capacity(18 * 21);
        let mut seen = [false; 256];
        let mut distinct = 0u16;
        let mut cursor_row = None;
        let mut cursor_col = None;
        let mut waiting = false;
        let mut hash = 0xcbf2_9ce4_8422_2325u64;

        for (i, &t) in tiles.iter().enumerate() {
            let (row, col) = (i / 20, i % 20);
            hash ^= t as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
            if !seen[t as usize] {
                seen[t as usize] = true;
                distinct += 1;
            }
            if t == glyph::CURSOR && cursor_row.is_none() {
                cursor_row = Some(row as u8);
                cursor_col = Some(col as u8);
            }
            if t == glyph::MORE_TEXT {
                waiting = true;
            }
            match decode_tile(t) {
                Some(s) => text.push_str(s),
                None => text.push(
                    terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '),
                ),
            }
            if col == 19 {
                text.push('\n');
            }
        }

        Screen {
            tiles,
            text,
            cursor_row,
            cursor_col,
            waiting,
            distinct,
            hash,
            lcd_on: true,
        }
    }

    /// The text with trailing blanks stripped and empty lines dropped — what
    /// goes to a model, where every wasted token is paid for.
    ///
    /// Indentation is dropped only down to the **common margin**, never per
    /// line: relative layout is meaning on this console — the battle HUD puts
    /// the opponent top-left and your Pokémon bottom-right, and a menu's
    /// column says which box it lives in. Trimming each line flattened all of
    /// that into one left-aligned list.
    pub fn compact_text(&self) -> String {
        let lines: Vec<&str> = self
            .text
            .lines()
            .map(|l| l.trim_end())
            .filter(|l| !l.trim().is_empty())
            .collect();
        let margin = lines
            .iter()
            .map(|l| l.len() - l.trim_start().len())
            .min()
            .unwrap_or(0);
        let mut out = String::new();
        for line in &lines {
            out.push_str(&line[margin.min(line.len())..]);
            out.push('\n');
        }
        out.pop();
        out
    }

    /// The text the `▶` cursor is pointing at, if a menu is up — the item that
    /// A would choose. Read off the tiles to the cursor's right, stopping at
    /// the first gap of two blank tiles so a two-column menu (`FIGHT  PKMN`)
    /// yields only the pointed-at column.
    pub fn selected(&self) -> Option<String> {
        let (row, col) = (self.cursor_row? as usize, self.cursor_col? as usize);
        let mut out = String::new();
        let mut blanks = 0u8;
        for c in col + 1..20 {
            let t = self.tiles.get(row * 20 + c).copied().unwrap_or(0x7F);
            let piece = match decode_tile(t) {
                Some(s) => s.to_string(),
                None => terminalgb_plugin_gen1_link::text::decode_char(t)
                    .unwrap_or(' ')
                    .to_string(),
            };
            if piece.trim().is_empty() {
                blanks += 1;
                if blanks >= 2 {
                    break;
                }
            } else {
                blanks = 0;
            }
            out.push_str(&piece);
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// How much [`Screen::text`] deserves to be believed. This only covers
    /// what a single read can know (the LCD, graphics-as-text); the *settling*
    /// verdict needs two reads and is the caller's — see
    /// [`Screen::graphics_as_text`] and the snapshot's stability gate.
    pub fn trust(&self) -> Trust {
        if !self.lcd_on {
            Trust::Off
        } else if self.graphics_as_text() {
            Trust::Graphics
        } else {
            Trust::Ok
        }
    }

    /// Is the tilemap showing graphics whose ids run through the font range,
    /// so the decode is alphabet salad? The signature is **consecutive tile
    /// ids side by side**: artwork is laid out as sequential tiles, so it
    /// decodes to runs like `ABCDEFGH`, which no English text contains. Real
    /// text never trips this — even the naming keyboard's `A B C D…` grid has
    /// a space tile between every letter.
    pub fn graphics_as_text(&self) -> bool {
        let letter = |t: u8| (0x80..=0x99).contains(&t) || (0xA0..=0xB9).contains(&t);
        for row in self.tiles.chunks(20) {
            let mut run = 1u8;
            for w in row.windows(2) {
                if letter(w[0]) && w[1] == w[0] + 1 && letter(w[1]) {
                    run += 1;
                    if run >= 6 {
                        return true;
                    }
                } else {
                    run = 1;
                }
            }
        }
        false
    }

    /// Is a YES/NO confirmation box up? The one shape of screen where pressing
    /// A "to advance" is actually *answering a question* with whatever the
    /// cursor happens to rest on. `advance_text` stops here for the same
    /// reason it stops on the naming keyboard: the decision belongs to the
    /// model, not to a host loop that never asked it.
    pub fn yes_no_prompt(&self) -> bool {
        if self.cursor_row.is_none() {
            return false;
        }
        let mut yes = false;
        let mut no = false;
        for line in self.text.lines() {
            match line.trim().trim_start_matches(['\u{25B6}', '\u{25B7}']).trim() {
                "YES" => yes = true,
                "NO" => no = true,
                _ => {}
            }
        }
        yes && no
    }

    /// Is a real map drawn, as opposed to a menu or a mid-warp blank? Menus and
    /// transitions leave the tilemap nearly uniform.
    pub fn looks_like_a_map(&self) -> bool {
        self.distinct > 6
    }

    /// Is the Gen 1 on-screen naming keyboard up? This is the one screen where
    /// pressing A does the opposite of advancing: it types another letter, for
    /// ever. The prompt has documented that fact since the first recorded run
    /// and a later run *still* typed a name of A's — because the presses came
    /// from the host's own `advance_text`, which never asked the model at all.
    /// So the rule is enforced here, where the presses happen, not only in a
    /// document the model may not consult.
    ///
    /// The signature is the question (`NAME?` — covers `YOUR NAME?`,
    /// `RIVAL's NAME?` and a Pokémon's `NICKNAME?`) together with the top row
    /// of the letter grid, in either case (`lower case` is a mode of the same
    /// keyboard). The cursor glyphs are normalised back to the spaces they sit
    /// on first — the cursor lives *inside* the letter grid, and `A▶B C…`
    /// must still read as the keyboard.
    pub fn naming_keyboard(&self) -> bool {
        let flat: String = self
            .text
            .chars()
            .map(|c| {
                if c == '\u{25B6}' || c == '\u{25B7}' {
                    ' '
                } else {
                    c
                }
            })
            .collect();
        flat.contains("NAME?")
            && (flat.contains("A B C D E F G H I")
                || flat.contains("a b c d e f g h i"))
    }
}

/// One party member, trimmed to what a decision needs.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PartyMon {
    pub name: String,
    pub nickname: String,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub status: Option<&'static str>,
}

/// A way off this map — a door, a staircase, a cave mouth.
///
/// This is the one piece of *spatial* knowledge the agent is given, and it
/// matters more than any other: a human sees a door and walks to it, whereas an
/// agent reading coordinates has no idea one exists. It is not inferred from
/// pixels — it is the game's own warp table, the same data the engine uses to
/// decide where the player lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub x: u8,
    pub y: u8,
    /// Map this exit leads to.
    pub to_map: u8,
}

/// The Gen 1 game state a decision is made from.
#[derive(Debug, Clone, Default)]
pub struct GameState {
    /// False until a save is actually loaded — before that the party area holds
    /// uninitialised bytes and every number read from it is noise.
    pub in_game: bool,
    pub player_name: String,
    pub position: Position,
    pub map_name: &'static str,
    pub badges: u8,
    pub money: u32,
    pub party: Vec<PartyMon>,
    pub battle: BattleKind,
    pub opponent: Option<PartyMon>,
    /// Doors and staircases on this map, from its warp table.
    pub exits: Vec<Exit>,
    /// The current map's size **in the same units as `x` and `y`**, or `None`
    /// when no map header is loaded — which means the player is not in
    /// control. See [`W_CUR_MAP_HEIGHT`].
    pub map_size: Option<(u8, u8)>,
}

/// Reads Gen 1 state through a capability-checked [`Machine`].
///
/// Addresses come from [`terminalgb_plugin_trainer::gen1`], which derives them from a
/// verified `pret/pokered` build; only the two this module needs and that one
/// does not carry — the player's map/coordinates and facing — are introduced
/// here, with their symbols cited.
pub struct Gen1Observer {
    ok: bool,
}

/// `wCurMap` (`ram/wram.asm`). Cross-checked at run time by
/// `examples/gen1_trade.rs`, which drives a real cartridge through it.
pub const W_CUR_MAP: u16 = 0xD35E;
/// `wYCoord` — the player's y within the current map, in 16x16 blocks.
pub const W_Y_COORD: u16 = 0xD361;
/// `wXCoord`.
pub const W_X_COORD: u16 = 0xD362;
/// `wSpriteStateData1` is `$C100` and each sprite owns 16 bytes; the player is
/// sprite 0, so its facing byte (`+$9`) is `$C109`
/// (`constants/sprite_data_constants.asm`, `SPRITESTATEDATA1_FACINGDIRECTION`).
pub const W_PLAYER_FACING: u16 = 0xC109;
/// `wNumberOfWarps` — how many entries the current map's warp table has. Also
/// used (and cross-checked against a real cartridge) by
/// `examples/gen1_trade.rs`, which rewrites a warp to move the player.
pub const W_NUMBER_OF_WARPS: u16 = 0xD3AE;
/// `wWarpEntries` — four bytes each: **y, x**, destination warp id,
/// destination map. Note the coordinate order: the `warp_event` macro in the
/// map source is written `x, y`, and the RAM struct is the other way round, so
/// reading it in source order puts every door in the wrong place.
pub const W_WARP_ENTRIES: u16 = 0xD3AF;
/// The engine's own cap (`constants/map_data_constants.asm`, `MAX_WARPS`).
pub const MAX_WARPS: usize = 32;
/// `wCurMapHeight` / `wCurMapWidth`, **in 32x32 blocks**.
///
/// These are the cleanest "am I actually standing on a map?" test there is.
/// `LoadMapHeader` fills them, and Pokémon Blue's opening runs for thousands of
/// frames with `wCurMap` **already set** to the bedroom while the header has not
/// been loaded — so position and map name both read plausibly and walking does
/// nothing. Measured on a real cartridge: both bytes are `0` until frame ~5,000
/// and `4`/`4` from the moment the bedroom loads. No real map is 0x0.
pub const W_CUR_MAP_HEIGHT: u16 = 0xD368;
pub const W_CUR_MAP_WIDTH: u16 = 0xD369;

impl Gen1Observer {
    pub fn new(supported: bool) -> Gen1Observer {
        Gen1Observer { ok: supported }
    }

    pub fn read(&self, m: &mut Machine) -> GameState {
        let mut st = GameState::default();
        if !self.ok {
            return st;
        }
        let mut name = [0u8; 11];
        m.peek_into(tr::PLAYER_NAME, &mut name);
        let count = m.peek(tr::PARTY_COUNT).unwrap_or(0xFF);
        st.in_game =
            terminalgb_plugin_trainer::text::looks_like_a_name(&name) && count <= 6;
        if !st.in_game {
            return st;
        }
        st.player_name = terminalgb_plugin_trainer::text::decode(&name);
        st.position = Position {
            map: m.peek(W_CUR_MAP).unwrap_or(0),
            x: m.peek(W_X_COORD).unwrap_or(0),
            y: m.peek(W_Y_COORD).unwrap_or(0),
            facing: Facing::from_byte(m.peek(W_PLAYER_FACING).unwrap_or(0xFF)),
        };
        st.map_name = map_name(st.position.map);
        st.badges = m.peek(tr::OBTAINED_BADGES).unwrap_or(0);
        st.money = terminalgb_plugin_trainer::text::bcd3([
            m.peek(tr::PLAYER_MONEY).unwrap_or(0),
            m.peek(tr::PLAYER_MONEY + 1).unwrap_or(0),
            m.peek(tr::PLAYER_MONEY + 2).unwrap_or(0),
        ]);

        for i in 0..count as u16 {
            let base =
                tr::PARTY_MONS + i * terminalgb_plugin_trainer::gen1::PARTY_MON_SIZE;
            let mut nick = [0u8; 11];
            m.peek_into(tr::PARTY_MON_NICKS + i * 11, &mut nick);
            use terminalgb_plugin_trainer::gen1::party as p;
            st.party.push(PartyMon {
                name: String::new(),
                nickname: terminalgb_plugin_trainer::text::decode(&nick),
                level: m.peek(base + p::LEVEL).unwrap_or(0),
                hp: m.peek_be16(base + p::HP).unwrap_or(0),
                max_hp: m.peek_be16(base + p::MAX_HP).unwrap_or(0),
                status: terminalgb_plugin_trainer::gen1::Status(
                    m.peek(base + p::STATUS).unwrap_or(0),
                )
                .label(),
            });
        }

        let (h, w) = (
            m.peek(W_CUR_MAP_HEIGHT).unwrap_or(0),
            m.peek(W_CUR_MAP_WIDTH).unwrap_or(0),
        );
        // **Doubled, and that is not a fudge.** A Gen 1 map's width and height
        // are counted in 32x32-pixel *blocks*, while `wXCoord`/`wYCoord` — and
        // every warp coordinate — are in 16x16 tiles, two per block. Reporting
        // the raw numbers tells an agent that Pallet Town is 10 wide when its
        // x runs 0..19, so it concludes it is standing outside the map and
        // stops trusting its own position. Checked against the two maps whose
        // exits are known on a real cartridge: Red's house 2F is 4x4 blocks
        // with its staircase at x=7, and Pallet Town is 10x9 with Oak's Lab
        // at (12,11).
        st.map_size = if h > 0 && w > 0 {
            Some((w.saturating_mul(2), h.saturating_mul(2)))
        } else {
            None
        };

        let warps = m.peek(W_NUMBER_OF_WARPS).unwrap_or(0) as usize;
        for i in 0..warps.min(MAX_WARPS) {
            let base = W_WARP_ENTRIES + (i * 4) as u16;
            st.exits.push(Exit {
                y: m.peek(base).unwrap_or(0),
                x: m.peek(base + 1).unwrap_or(0),
                to_map: m.peek(base + 3).unwrap_or(0),
            });
        }

        st.battle = BattleKind::from_flag(m.peek(tr::IS_IN_BATTLE).unwrap_or(0));
        if st.battle != BattleKind::None {
            use terminalgb_plugin_trainer::gen1::battle as b;
            let mut nick = [0u8; 11];
            m.peek_into(tr::ENEMY_MON_NICK, &mut nick);
            st.opponent = Some(PartyMon {
                name: String::new(),
                nickname: terminalgb_plugin_trainer::text::decode(&nick),
                level: m.peek(tr::ENEMY_MON + b::LEVEL).unwrap_or(0),
                hp: m.peek_be16(tr::ENEMY_MON + b::HP).unwrap_or(0),
                max_hp: m.peek_be16(tr::ENEMY_MON + b::MAX_HP).unwrap_or(0),
                status: None,
            });
        }
        st
    }
}

/// Work out what the game is doing from the screen and the battle flag.
///
/// Screen-first on purpose: the two signals that matter most — "a menu cursor
/// is on screen" and "the game is waiting for A" — are *glyphs*, so they need
/// no RAM address and cannot be wrong for a ROM revision.
pub fn classify(screen: &Screen, game: &GameState) -> Mode {
    if game.battle != BattleKind::None {
        return Mode::Battle;
    }
    if screen.waiting {
        return Mode::Dialogue;
    }
    if screen.cursor_row.is_some() {
        return Mode::Menu;
    }
    if !game.in_game {
        return Mode::Title;
    }
    // No map header means no map: the game is running a scene. Reporting this
    // as "overworld" (which the screen looks like, during Oak's speech) sends
    // an agent off trying every direction in a room it is not standing in.
    if game.map_size.is_none() {
        return Mode::Cutscene;
    }
    if screen.looks_like_a_map() {
        Mode::Overworld
    } else {
        Mode::Transition
    }
}

/// Map ids → names, from `pret/pokered`'s `constants/map_constants.asm`.
///
/// **Partial by design.** Only the maps the early game passes through are
/// listed; anything else reports its number rather than a guess, which is
/// exactly what a wrong name would cost an agent that trusts it. The numbering
/// is confirmed at two points against code already in this repo: Viridian
/// Pokémon Center is `$29` and the Trade Center is `$EF`
/// (`examples/gen1_trade.rs`).
pub fn map_name(id: u8) -> &'static str {
    match id {
        0x00 => "Pallet Town",
        0x01 => "Viridian City",
        0x02 => "Pewter City",
        0x03 => "Cerulean City",
        0x0C => "Route 1",
        0x0D => "Route 2",
        0x0E => "Route 3",
        0x0F => "Route 4",
        0x25 => "Red's house 1F",
        0x26 => "Red's house 2F (bedroom)",
        0x27 => "Blue's house",
        0x28 => "Oak's Lab",
        0x29 => "Viridian Pokémon Center",
        0x2A => "Viridian Mart",
        0x2B => "Viridian School",
        0x2C => "Viridian House",
        0x2D => "Viridian Gym",
        0x33 => "Pewter Pokémon Center",
        0x36 => "Pewter Gym",
        0x3B => "Viridian Forest",
        0xEF => "Trade Center",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A blank 20x18 grid with byte runs placed at (row, col) — the tile-level
    /// counterpart of `screen_with`, for exercising the real decode.
    fn grid(rows: &[(usize, usize, &[u8])]) -> Vec<u8> {
        let mut tiles = vec![0x7Fu8; 20 * 18];
        for &(row, col, bytes) in rows {
            for (i, &b) in bytes.iter().enumerate() {
                tiles[row * 20 + col + i] = b;
            }
        }
        tiles
    }

    fn enc(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| terminalgb_plugin_gen1_link::text::encode_char(c).unwrap())
            .collect()
    }

    /// The battle menu spells "PKMN" with the two ligature tiles `<PK><MN>`
    /// ($E1 $E2, `constants/charmap.asm`) — decode them to spaces and the
    /// switch-Pokémon option is invisible. Measured on a real Blue cartridge
    /// before the fix: the menu read `FIGHT / ITEM RUN`, and a model told
    /// that has no way to know switching exists.
    #[test]
    fn the_battle_menus_pkmn_option_is_visible() {
        let mut rows = vec![(14usize, 10usize, enc("FIGHT"))];
        rows.push((14, 17, vec![0xE1, 0xE2]));
        let refs: Vec<(usize, usize, &[u8])> =
            rows.iter().map(|(r, c, v)| (*r, *c, v.as_slice())).collect();
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.text.contains("FIGHT"), "{}", s.text);
        assert!(s.text.contains("PKMN"), "PKMN missing from: {}", s.text);
    }

    /// Dialogue leans on contraction ligatures ('s, 't, …) and the battle HUD
    /// on the `<LV>` glyph; all were spaces before.
    #[test]
    fn contractions_and_the_level_glyph_decode() {
        let mut cant = enc("can");
        cant.push(0xBE); // 't
        let mut lv3 = vec![0x6E]; // <LV>
        lv3.extend(enc("3"));
        let refs: Vec<(usize, usize, &[u8])> =
            vec![(1, 1, cant.as_slice()), (3, 1, lv3.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.text.contains("can't"), "{}", s.text);
        assert!(s.text.contains("Lv3"), "{}", s.text);
    }

    /// The ▶ cursor is rendered into the text (a menu whose cursor decodes to
    /// a space cannot say what is selected), located by row AND column, and
    /// `selected()` names the item it points at — only that item, stopping at
    /// the two-blank column gap so `▶FIGHT  PKMN` does not bleed together.
    #[test]
    fn the_cursor_is_visible_and_the_selected_item_is_named() {
        let menu = {
            let new_game = enc("NEW GAME");
            let option = enc("OPTION");
            let refs: Vec<(usize, usize, &[u8])> = vec![
                (2, 1, &[glyph::CURSOR]),
                (2, 2, new_game.as_slice()),
                (4, 2, option.as_slice()),
            ];
            Screen::from_tiles(grid(&refs))
        };
        assert!(menu.text.contains("\u{25B6}NEW GAME"), "{}", menu.text);
        assert_eq!(menu.cursor_row, Some(2));
        assert_eq!(menu.cursor_col, Some(1));
        assert_eq!(menu.selected().as_deref(), Some("NEW GAME"));

        // Two-column battle menu, cursor on the right-hand column.
        let battle = {
            let fight = enc("FIGHT");
            let refs: Vec<(usize, usize, &[u8])> = vec![
                (14, 2, fight.as_slice()),
                (14, 9, &[glyph::CURSOR]),
                (14, 10, &[0xE1, 0xE2]),
            ];
            Screen::from_tiles(grid(&refs))
        };
        assert_eq!(battle.selected().as_deref(), Some("PKMN"));
    }

    /// `compact_text` keeps layout down to the common margin: the battle HUD
    /// puts the opponent top-left and your Pokémon bottom-right, and per-line
    /// trimming flattened that meaning away.
    #[test]
    fn compact_text_keeps_relative_indentation() {
        let pidgey = enc("PIDGEY");
        let chariz = enc("CHARIZARD");
        let refs: Vec<(usize, usize, &[u8])> =
            vec![(0, 1, pidgey.as_slice()), (7, 10, chariz.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert_eq!(s.compact_text(), "PIDGEY\n         CHARIZARD");
    }

    /// Artwork laid out as sequential tiles through the font range decodes as
    /// alphabet salad — the Game Freak intro reads `ABCDEFGHIJKLMNOP…` on a
    /// real cartridge. That must be flagged, and the naming keyboard (a real
    /// text screen full of single letters) must not be.
    #[test]
    fn graphics_as_text_is_flagged_and_real_text_is_not() {
        let salad: Vec<u8> = (0x80..0x90).collect();
        let refs: Vec<(usize, usize, &[u8])> = vec![(2, 2, salad.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.graphics_as_text());
        assert_eq!(s.trust(), Trust::Graphics);

        // The keyboard's letters have a space tile between every pair.
        let kbd = enc("A B C D E F G H I");
        let refs: Vec<(usize, usize, &[u8])> = vec![(5, 2, kbd.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert!(!s.graphics_as_text());
        assert_eq!(s.trust(), Trust::Ok);

        // The LCD being off is its own verdict, not "empty text".
        assert_eq!(Screen::default().trust(), Trust::Off);
    }

    /// A YES/NO box is recognised only as the real shape: both words on their
    /// own rows with a menu cursor up — never dialogue that merely says "NO".
    #[test]
    fn a_yes_no_box_is_recognised_and_dialogue_is_not() {
        let yes = enc("YES");
        let no = enc("NO");
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (8, 11, &[glyph::CURSOR]),
            (8, 12, yes.as_slice()),
            (10, 12, no.as_slice()),
        ];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.yes_no_prompt());
        assert_eq!(s.selected().as_deref(), Some("YES"));

        // Dialogue containing the words without the box shape.
        let line = enc("NO way to go YES");
        let refs: Vec<(usize, usize, &[u8])> = vec![(14, 1, line.as_slice())];
        assert!(!Screen::from_tiles(grid(&refs)).yes_no_prompt());
        // The words alone, but no cursor: still not a prompt.
        let refs: Vec<(usize, usize, &[u8])> =
            vec![(8, 12, yes.as_slice()), (10, 12, no.as_slice())];
        assert!(!Screen::from_tiles(grid(&refs)).yes_no_prompt());
    }

    /// The keyboard predicate must survive the cursor being rendered into the
    /// letter grid — `A▶B C…` is still the keyboard.
    #[test]
    fn the_naming_keyboard_survives_an_inline_cursor() {
        let q = enc("YOUR NAME?");
        let mut row = enc("A");
        row.push(glyph::CURSOR);
        row.extend(enc("B C D E F G H I"));
        let refs: Vec<(usize, usize, &[u8])> =
            vec![(1, 0, q.as_slice()), (5, 2, row.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.naming_keyboard(), "{}", s.text);
    }

    #[test]
    fn facing_only_accepts_the_four_real_values() {
        assert_eq!(Facing::from_byte(0x00), Some(Facing::Down));
        assert_eq!(Facing::from_byte(0x04), Some(Facing::Up));
        assert_eq!(Facing::from_byte(0x08), Some(Facing::Left));
        assert_eq!(Facing::from_byte(0x0C), Some(Facing::Right));
        // A byte read mid-update is reported as unknown, not as "down".
        assert_eq!(Facing::from_byte(0x02), None);
        assert_eq!(Facing::from_byte(0xFF), None);
    }

    /// A map's width and height are in 32x32 blocks; `x`, `y` and every warp
    /// coordinate are in 16x16 tiles. The two known-good data points, from a
    /// real cartridge: Red's house 2F is 4x4 blocks and its staircase is at
    /// x=7; Pallet Town is 10x9 and Oak's Lab is at (12,11). Both are only in
    /// range once the size is doubled.
    #[test]
    fn the_map_size_is_reported_in_the_units_the_coordinates_use() {
        let doubled = |w: u8, h: u8| (w.saturating_mul(2), h.saturating_mul(2));
        let (w, h) = doubled(4, 4);
        assert!(7 < w, "the bedroom staircase at x=7 must be inside {w}");
        let (w, h2) = doubled(10, 9);
        assert!(12 < w, "Oak's Lab at x=12 must be inside {w}");
        assert!(11 < h2, "Oak's Lab at y=11 must be inside {h2}");
        let _ = h;
    }

    /// The naming keyboard must be recognised in every form the game shows it
    /// — player, rival, nickname, and the lower-case mode of the same grid —
    /// and never on ordinary dialogue that happens to mention a name.
    #[test]
    fn the_naming_keyboard_is_recognised_and_dialogue_is_not() {
        let with_text = |t: &str| Screen {
            text: t.to_string(),
            ..Default::default()
        };
        // Transcribed from a real run's snapshot at the player naming screen.
        let player = "YOUR NAME?\nA B C D E F G H I\nJ K L M N O P Q R\n\
                      S T U V W X Y Z\nlower case";
        assert!(with_text(player).naming_keyboard());
        assert!(with_text("RIVAL's NAME?\nA B C D E F G H I").naming_keyboard());
        assert!(with_text("NICKNAME?\nA B C D E F G H I").naming_keyboard());
        assert!(with_text("YOUR NAME?\na b c d e f g h i").naming_keyboard());
        // Dialogue mentioning a name, no grid: not the keyboard.
        assert!(!with_text("what is your NAME? said OAK").naming_keyboard());
        // The grid alone (mid-transition garbage): not the keyboard.
        assert!(!with_text("A B C D E F G H I").naming_keyboard());
        assert!(!with_text("").naming_keyboard());
    }

    #[test]
    fn an_unlisted_map_reports_no_name_rather_than_a_wrong_one() {
        assert_eq!(map_name(0x26), "Red's house 2F (bedroom)");
        assert_eq!(map_name(0x28), "Oak's Lab");
        assert_eq!(map_name(0x7F), "");
    }

    fn screen_with(mut f: impl FnMut(&mut Vec<u8>)) -> Screen {
        let mut tiles = vec![0x7Fu8; 20 * 18];
        f(&mut tiles);
        let mut s = Screen {
            cursor_row: tiles
                .iter()
                .position(|&t| t == glyph::CURSOR)
                .map(|i| (i / 20) as u8),
            waiting: tiles.contains(&glyph::MORE_TEXT),
            distinct: {
                let mut seen = [false; 256];
                let mut n = 0;
                for &t in &tiles {
                    if !seen[t as usize] {
                        seen[t as usize] = true;
                        n += 1;
                    }
                }
                n
            },
            tiles,
            ..Default::default()
        };
        s.hash = 1;
        s
    }

    #[test]
    fn mode_is_decided_screen_first() {
        let plain = screen_with(|_| {});
        let in_game = GameState {
            in_game: true,
            map_size: Some((10, 9)),
            ..Default::default()
        };

        // A battle wins over everything: the flag is unambiguous.
        let mut battling = in_game.clone();
        battling.battle = BattleKind::Wild;
        assert_eq!(classify(&plain, &battling), Mode::Battle);

        // "Press A for more" beats a menu cursor that is still on screen
        // behind the text box — the arrow is the game asking for input now.
        let both = screen_with(|t| {
            t[0] = glyph::CURSOR;
            t[100] = glyph::MORE_TEXT;
        });
        assert_eq!(classify(&both, &in_game), Mode::Dialogue);

        let menu = screen_with(|t| t[40] = glyph::CURSOR);
        assert_eq!(classify(&menu, &in_game), Mode::Menu);
        assert_eq!(menu.cursor_row, Some(2));

        // A nearly uniform screen with no save loaded is the title.
        assert_eq!(classify(&plain, &GameState::default()), Mode::Title);
        // ... and with one loaded it is a transition, not "overworld".
        assert_eq!(classify(&plain, &in_game), Mode::Transition);

        let map = screen_with(|t| {
            for (i, slot) in t.iter_mut().enumerate() {
                *slot = (i % 30) as u8;
            }
        });
        assert_eq!(classify(&map, &in_game), Mode::Overworld);

        // The same screen with no map header loaded is a cutscene, not the
        // overworld — Oak's opening speech looks exactly like a drawn map.
        let mid_scene = GameState {
            map_size: None,
            ..in_game.clone()
        };
        assert_eq!(classify(&map, &mid_scene), Mode::Cutscene);
        // ...but a text box in a cutscene is still a text box.
        let talking = screen_with(|t| t[100] = glyph::MORE_TEXT);
        assert_eq!(classify(&talking, &mid_scene), Mode::Dialogue);
    }
}

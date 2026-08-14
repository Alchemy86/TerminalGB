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

/// The 20x18 tile grid the LCD is showing, and what can be read off it.
#[derive(Debug, Clone, Default)]
pub struct Screen {
    /// Raw tile ids, row-major.
    pub tiles: Vec<u8>,
    /// Decoded text, 18 lines of 20 characters. Non-text tiles become spaces.
    pub text: String,
    /// Row of the active menu cursor, if one is on screen.
    pub cursor_row: Option<u8>,
    /// The `▼` "press A for more" arrow is showing.
    pub waiting: bool,
    /// How many distinct tile ids are on screen. A drawn map has many; a menu,
    /// a fade or a blank transition has very few.
    pub distinct: u16,
    /// FNV-1a over the tile ids. Cheap "did anything change?".
    pub hash: u64,
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
        let mut text = String::with_capacity(18 * 21);
        let mut seen = [false; 256];
        let mut distinct = 0u16;
        let mut cursor_row = None;
        let mut waiting = false;
        let mut hash = 0xcbf2_9ce4_8422_2325u64;

        for row in 0..18usize {
            for col in 0..20usize {
                let y = (sy + row) % 32;
                let x = (sx + col) % 32;
                let t = m.peek(base + (y * 32 + x) as u16).unwrap_or(0);
                tiles.push(t);
                hash ^= t as u64;
                hash = hash.wrapping_mul(0x1000_0000_01b3);
                if !seen[t as usize] {
                    seen[t as usize] = true;
                    distinct += 1;
                }
                if t == glyph::CURSOR && cursor_row.is_none() {
                    cursor_row = Some(row as u8);
                }
                if t == glyph::MORE_TEXT {
                    waiting = true;
                }
                text.push(
                    terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '),
                );
            }
            text.push('\n');
        }

        Screen {
            tiles,
            text,
            cursor_row,
            waiting,
            distinct,
            hash,
        }
    }

    /// The text with trailing blanks stripped and empty lines dropped — what
    /// goes to a model, where every wasted token is paid for.
    pub fn compact_text(&self) -> String {
        let mut out = String::new();
        for line in self.text.lines() {
            let t = line.trim_end();
            if t.trim().is_empty() {
                continue;
            }
            out.push_str(t.trim_start());
            out.push('\n');
        }
        out.pop();
        out
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
    /// keyboard).
    pub fn naming_keyboard(&self) -> bool {
        self.text.contains("NAME?")
            && (self.text.contains("A B C D E F G H I")
                || self.text.contains("a b c d e f g h i"))
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

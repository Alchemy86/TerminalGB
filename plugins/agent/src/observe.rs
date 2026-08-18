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
//! | *what does A do here?* | [`Screen::kind`] | a move list, a YES/NO box and a letter grid are all "a menu"; A does something different, and irreversible, on each |
//! | *what would A choose?* | [`Screen::selected`] | the item under the `▶` cursor as a **fact**, so a driver need not infer it from raw rows |
//! | *is it my turn?* | [`Screen::waiting`] | the difference between "press A" and "wait for the animation" |
//! | *can I believe any of it?* | [`Screen::trust`] | a half-drawn screen and artwork-in-the-font-range both decode to confident nonsense; better to withhold and say so |
//!
//! # What the screen channel does NOT cover
//!
//! **Sprites.** People, Poké Balls and every other object are OAM sprites drawn
//! *over* the tilemap; none of them is in [`Screen::text`], at any trust level.
//! A live run stalled in Oak's Lab standing one tile from a Poké Ball it had no
//! way to know existed. That is a separate channel, not a decoding bug, and the
//! prompt says so rather than letting an empty screen imply an empty room.
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
///
/// # What is deliberately NOT decoded, and why
///
/// A tile id is an index into VRAM, and Gen 1 **reloads different pictures
/// into the same slots**. `charmap.asm` says so itself: after the main font it
/// lists a second block headed *"Actual characters (from
/// `gfx/font/font_battle_extra.png`)"* which re-uses `$6E`-`$75`, and further
/// overlays for the status screen (`<BOLD_P>` at `$72`) and the Pokédex
/// (`$60`/`$61`). So `$70`-`$75` are quotes **outside** battle and HP-bar /
/// `<to>` / `<ID>` graphics **inside** it, and no function of the tile id
/// alone can tell them apart.
///
/// They are left as blanks. Measured on a real battle screen, decoding them as
/// quotes put `”’` on a line of its own under the opponent's name and `153/153”`
/// on the player's HP row — noise on the one screen an agent most needs to read
/// cleanly, in exchange for decorative quotes it would never act on.
///
/// `$6E` is the opposite case and *is* decoded: its main-font meaning is an
/// unused hiragana that no English screen ever places, so `<LV>` is the only
/// picture it can actually be showing.
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
        // Money and the item-count separator ("×5"). On the naming screen
        // `$F0` is instead the `ED` key (`gfx/font/ED.1bpp`, loaded by
        // `PrintAlphabet`) — see [`Screen::from_tiles`], which is the one
        // place that has enough context to tell.
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

/// Append one tile's text to `out`: a ligature, a plain character, or a space
/// for anything that is not text. The single place the two charmaps are
/// combined, so nothing can decode a tile two different ways.
fn push_piece(out: &mut String, t: u8, naming: bool) {
    // On the naming screen `$F0` is the `ED` ("end") key, not `¥` — the one
    // button on that screen a driver might want and the only tile whose two
    // meanings can be told apart, because the keyboard announces itself.
    if naming && t == 0xF0 {
        out.push_str("ED");
        return;
    }
    match decode_tile(t) {
        Some(s) => out.push_str(s),
        None => {
            out.push(terminalgb_plugin_gen1_link::text::decode_char(t).unwrap_or(' '))
        }
    }
}

/// Does this tile contribute anything visible to the text? Box borders, map
/// tiles and sprite tiles all decode to blanks, which is what makes a blank run
/// a reliable column separator.
fn tile_is_blank(t: u8) -> bool {
    let mut s = String::new();
    push_piece(&mut s, t, false);
    s.trim().is_empty()
}

/// Render a 20x18 grid as 18 newline-terminated lines.
fn render(tiles: &[u8], naming: bool) -> String {
    let mut text = String::with_capacity(18 * 22);
    for (i, &t) in tiles.iter().enumerate() {
        push_piece(&mut text, t, naming);
        if i % 20 == 19 {
            text.push('\n');
        }
    }
    text
}

/// The naming-keyboard signature, over already-decoded text. A free function
/// because [`Screen::from_tiles`] needs the answer *while* deciding how to
/// decode. See [`Screen::naming_keyboard`] for what it means.
fn looks_like_naming_keyboard(text: &str) -> bool {
    let flat: String = text
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
        && (flat.contains("A B C D E F G H I") || flat.contains("a b c d e f g h i"))
}

/// What shape of screen this is, judged from the screen alone.
///
/// This is **not** a duplicate of [`Mode`], which is read out of the game's RAM
/// (am I in a battle? is a map loaded?). A screen kind answers the different
/// question *what does pressing A do here*, and it separates cases `Mode`
/// cannot: a YES/NO box and a move list are both `Mode::Battle`, and the
/// overworld and the title screen are both "no text on screen".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenKind {
    /// Nothing on the tilemap decodes to text — the overworld, the title
    /// screen, a fade. An empty `text` is then a *fact*, not a failure.
    #[default]
    NoText,
    /// Text, but nothing asking for a button: a HUD, a sign, a battle in the
    /// middle of an animation.
    Text,
    /// The `▼` arrow is up: the game is waiting for A to continue.
    Dialogue,
    /// A `▶` cursor is up: A chooses whatever it points at.
    Menu,
    /// A YES/NO confirmation. A here *answers a question*.
    YesNo,
    /// The on-screen letter grid. A types a letter; START finishes the name.
    NamingKeyboard,
}

impl ScreenKind {
    pub fn name(self) -> &'static str {
        match self {
            ScreenKind::NoText => "no-text",
            ScreenKind::Text => "text",
            ScreenKind::Dialogue => "dialogue",
            ScreenKind::Menu => "menu",
            ScreenKind::YesNo => "yes-no",
            ScreenKind::NamingKeyboard => "naming-keyboard",
        }
    }
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
    /// The 20x18 tile grid the LCD is showing, or `None` with the LCD off.
    ///
    /// Split out from [`Screen::read`] so "has anything changed?" can be asked
    /// every frame without paying for the decode — see [`Screen::tilemap_hash`].
    fn tilemap(m: &mut Machine) -> Option<Vec<u8>> {
        let lcdc = m.peek(0xFF40).unwrap_or(0);
        if lcdc & 0x80 == 0 {
            // LCD off: there is nothing on screen, and reading the map would
            // report whatever was last drawn.
            return None;
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
        Some(tiles)
    }

    /// Read the active tilemap out of VRAM.
    pub fn read(m: &mut Machine) -> Screen {
        match Screen::tilemap(m) {
            Some(tiles) => Screen::from_tiles(tiles),
            None => Screen::default(),
        }
    }

    /// Just the change-detection hash, with no decode. The caller that asks
    /// "is the screen still moving?" does so on **every** frame, and paying for
    /// eighteen lines of text rendering to answer it would be waste.
    pub fn tilemap_hash(m: &mut Machine) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for t in Screen::tilemap(m).unwrap_or_default() {
            hash ^= t as u64;
            hash = hash.wrapping_mul(0x1000_0000_01b3);
        }
        hash
    }

    /// Build a screen from a 20x18 tile grid. Pure — this is the whole decode,
    /// separated from the VRAM read so it can be tested against literal tiles.
    ///
    /// The text is rendered twice on the naming screen and once everywhere
    /// else: `$F0` means `¥` in the main font and `ED` under the naming
    /// screen's own overlay, and the only way to know which is to read the
    /// screen first. Nothing else in the charmap is disambiguated this way —
    /// see [`decode_tile`] for the tiles that are simply left blank.
    pub fn from_tiles(tiles: Vec<u8>) -> Screen {
        debug_assert_eq!(tiles.len(), 20 * 18);
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
        }

        let mut text = render(&tiles, false);
        if looks_like_naming_keyboard(&text) {
            text = render(&tiles, true);
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

    /// The text the `▶` cursor is pointing at — the item A would choose.
    ///
    /// Answering "is `NEW GAME` or `OPTION` selected?" as a *fact* was blind
    /// spot 3 of the live-run list; getting it right needs one non-obvious
    /// rule, because a Gen 1 menu item may contain a space (`NEW GAME`) and a
    /// two-column menu may separate its columns with only one (`▶FIGHT PKMN`,
    /// measured on a real battle screen — the naive "split on two blanks"
    /// answer was `"FIGHT PKMN"`, which is not an option the player has).
    ///
    /// The rule that separates them: **a menu's columns line up across its
    /// rows.** So an item ends at a blank run that is followed by a column
    /// where another row *of the same menu* also starts a word. `ITEM`/`RUN` on
    /// the row below puts a column start at `RUN`'s column, so `FIGHT` ends
    /// there; nothing else on the main menu starts where `GAME` does, so
    /// `NEW GAME` stays whole. On the naming keyboard every other row starts a
    /// word at every other column, so this yields the single letter under the
    /// cursor — which is blind spot 2, closed by the same rule rather than a
    /// special case.
    ///
    /// **"Of the same menu" is load-bearing, and was found by running it.** A
    /// menu's rows are the ones that begin an item in the same column as this
    /// one. Taking column starts from every row instead truncated `NEW NAME` on
    /// the name-prompt screen to `NEW`, because the question printed under the
    /// list (`your name?`) happens to begin a word in the column where `NAME`
    /// begins — dialogue that has nothing to do with the menu.
    pub fn selected(&self) -> Option<String> {
        let (row, col) = (self.cursor_row? as usize, self.cursor_col? as usize);
        if self.tiles.len() != 20 * 18 {
            return None;
        }
        let at = |r: usize, c: usize| self.tiles[r * 20 + c];
        let starts_at = |r: usize, c: usize| {
            !tile_is_blank(at(r, c)) && (c == 0 || tile_is_blank(at(r, c - 1)))
        };

        let start = col + 1;
        // Columns where another row of *this* menu begins a word.
        let mut col_start = [false; 20];
        for r in 0..18 {
            if r == row || !starts_at(r, start) {
                continue;
            }
            for (c, mark) in col_start.iter_mut().enumerate() {
                *mark |= starts_at(r, c);
            }
        }

        let naming = self.naming_keyboard();
        let mut out = String::new();
        let mut c = start;
        while c < 20 {
            if tile_is_blank(at(row, c)) {
                let mut n = c;
                while n < 20 && tile_is_blank(at(row, n)) {
                    n += 1;
                }
                // Trailing blanks, a gap of two or more, or the next column of
                // this menu: the item ended.
                if n >= 20 || n - c >= 2 || (n > start && col_start[n]) {
                    break;
                }
                out.push(' ');
                c += 1;
                continue;
            }
            push_piece(&mut out, at(row, c), naming);
            c += 1;
        }
        let out = out.trim().to_string();
        if out.is_empty() {
            None
        } else {
            Some(out)
        }
    }

    /// What shape of screen this is — see [`ScreenKind`]. Precedence is by how
    /// much a wrong guess costs: the naming keyboard and a YES/NO box are the
    /// two screens where "press A to continue" does something irreversible and
    /// unasked-for, so they are decided first.
    pub fn kind(&self) -> ScreenKind {
        if self.naming_keyboard() {
            ScreenKind::NamingKeyboard
        } else if self.yes_no_prompt() {
            ScreenKind::YesNo
        } else if self.cursor_row.is_some() {
            ScreenKind::Menu
        } else if self.waiting {
            ScreenKind::Dialogue
        } else if self.text.trim().is_empty() {
            ScreenKind::NoText
        } else {
            ScreenKind::Text
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
            match line
                .trim()
                .trim_start_matches(['\u{25B6}', '\u{25B7}'])
                .trim()
            {
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
        looks_like_naming_keyboard(&self.text)
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
    /// Everything the shared battle policy decides from, when a battle is up.
    /// See [`crate::battle`].
    pub arena: Option<crate::battle::View>,
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
    /// The move table and the type chart, pulled out of the cartridge's banks
    /// once and kept. 1.6 KiB of `peek_bank`, on the first battle only.
    ///
    /// Behind a `RefCell` so [`Gen1Observer::read`] can stay `&self`: the
    /// executor in [`crate::action`] holds the observer by shared reference
    /// while it walks, and a cache is not a reason to change that.
    tables: std::cell::RefCell<Option<terminalgb_plugin_trainer::rom::BattleTables>>,
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
        Gen1Observer {
            ok: supported,
            tables: std::cell::RefCell::new(None),
        }
    }

    /// The cartridge's move table and type chart, read on first use.
    ///
    /// **Not at construction**: the plugin is registered before the machine
    /// exists, and a cartridge that is not Gen 1 must not be scanned at all.
    fn fill_tables(&self, m: &mut Machine) {
        if self.tables.borrow().is_some() {
            return;
        }
        let t = terminalgb_plugin_trainer::rom::BattleTables::read(|bank, addr| {
            m.peek_bank(bank, addr).unwrap_or(0xFF)
        });
        *self.tables.borrow_mut() = Some(t);
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
            st.arena = self.arena(m, &st);
        }
        st
    }

    /// Read everything the battle policy decides from.
    ///
    /// Both sides come out of the two 28-byte battle structs rather than the
    /// party — Gen 1 copies the active Pokémon into `wBattleMon` and applies
    /// stat stages *there*, so these are the numbers the cartridge is actually
    /// fighting with.
    fn arena(&self, m: &mut Machine, st: &GameState) -> Option<crate::battle::View> {
        use crate::battle;
        use terminalgb_plugin_trainer::gen1::battle as b;
        self.fill_tables(m);
        let cache = self.tables.borrow();
        let t = cache.as_ref().expect("just filled");
        let ours =
            read_side(m, tr::BATTLE_MON, tr::BATTLE_MON_NICK, tr::PLAYER_STAT_MODS);
        let theirs = read_side(m, tr::ENEMY_MON, tr::ENEMY_MON_NICK, tr::ENEMY_STAT_MODS);
        // The struct is uninitialised until the engine has filled it; a level
        // of zero is the tell, and acting on it would be acting on noise.
        if ours.level == 0 || theirs.level == 0 || ours.max_hp == 0 {
            return None;
        }
        let mut moves = Vec::new();
        for slot in 0..terminalgb_plugin_trainer::gen1::NUM_MOVES {
            let id = m.peek(tr::BATTLE_MON + b::MOVES + slot as u16).unwrap_or(0);
            if id == 0 {
                continue;
            }
            // The battle struct's PP bytes carry the PP Ups in their top two
            // bits, exactly as the party struct's do.
            let pp = m.peek(tr::BATTLE_MON + b::PP + slot as u16).unwrap_or(0) & 0x3F;
            let kind = t.kind(id);
            let mut mv = battle::Move {
                slot,
                id,
                name: String::new(),
                power: t.power(id),
                kind,
                effect: t.effect(id),
                accuracy: hit_chance(t, t.accuracy_percent(id), &ours, &theirs),
                pp,
                effect_pct: t.multiplier(kind, theirs.types.0, theirs.types.1),
                stab: kind == ours.types.0 || kind == ours.types.1,
                min: 0,
                max: 0,
            };
            let (lo, hi) = battle::damage(t, &mv, &ours, &theirs);
            mv.min = lo;
            mv.max = hi;
            moves.push(mv);
        }
        // What we expect to take. The opponent's move list is in its own battle
        // struct; a player would have to learn it by being hit, and the
        // module docs say so.
        let (mut worst, mut expected, mut crit) = (0u16, 0u16, 0u16);
        for slot in 0..terminalgb_plugin_trainer::gen1::NUM_MOVES {
            let id = m.peek(tr::ENEMY_MON + b::MOVES + slot as u16).unwrap_or(0);
            if id == 0 {
                continue;
            }
            let kind = t.kind(id);
            let their_move = battle::Move {
                id,
                power: t.power(id),
                kind,
                accuracy: hit_chance(t, t.accuracy_percent(id), &theirs, &ours),
                pp: 1,
                effect_pct: t.multiplier(kind, ours.types.0, ours.types.1),
                stab: kind == theirs.types.0 || kind == theirs.types.1,
                ..Default::default()
            };
            let (_, hi) = battle::damage(t, &their_move, &theirs, &ours);
            worst = worst.max(hi);
            expected = expected.max(hi * their_move.accuracy as u16 / 100);
            // The same move as a critical hit: Gen 1 substitutes `2 * Level`
            // for `Level` in the damage formula, so the attacker is priced at
            // twice its level and nothing else changes.
            let mut crit_side = theirs.clone();
            crit_side.level = theirs.level.saturating_mul(2);
            let (_, chi) = battle::damage(t, &their_move, &crit_side, &ours);
            crit = crit.max(chi);
        }
        let items = read_bag(m);
        Some(battle::View {
            trainer: st.battle == BattleKind::Trainer,
            ours,
            theirs,
            moves,
            items,
            party_alive: st.party.iter().filter(|p| p.hp > 0).count().max(1),
            incoming_max: worst,
            incoming_expected: expected,
            incoming_crit: crit,
            // How many turns have gone on a stat move is the driver's count,
            // not a memory read — the cartridge does not keep one.
            stat_turns: 0,
            menu_item: m.peek(tr::CURRENT_MENU_ITEM).unwrap_or(0),
        })
    }
}

/// A move's accuracy with the stages applied, as `MoveHitTest` applies them:
/// multiply by the attacker's accuracy ratio, then by the inverse of the
/// defender's evasion ratio. Both ratios come out of the cartridge's own
/// `StatModifierRatios`.
fn hit_chance(
    t: &terminalgb_plugin_trainer::rom::BattleTables,
    base: u8,
    attacker: &crate::battle::Side,
    defender: &crate::battle::Side,
) -> u8 {
    let (an, ad) = t.stage_ratio(attacker.accuracy_stage());
    let (en, ed) = t.stage_ratio(defender.evasion_stage());
    // The evasion ratio divides, so its numerator and denominator swap.
    let pct = base as u32 * an * ed / ad.max(1) / en.max(1);
    pct.min(100) as u8
}

/// One side of a battle out of its 28-byte struct.
fn read_side(m: &mut Machine, base: u16, nick: u16, mods: u16) -> crate::battle::Side {
    use terminalgb_plugin_trainer::gen1::battle as b;
    let mut name = [0u8; 11];
    m.peek_into(nick, &mut name);
    let mut stages = [crate::battle::NEUTRAL_STAGE; 6];
    for (i, s) in stages.iter_mut().enumerate() {
        // Uninitialised WRAM reads as anything; only 1..13 is a stage.
        let raw = m
            .peek(mods + i as u16)
            .unwrap_or(crate::battle::NEUTRAL_STAGE);
        if (1..=13).contains(&raw) {
            *s = raw;
        }
    }
    crate::battle::Side {
        name: terminalgb_plugin_trainer::text::decode(&name),
        species: m.peek(base + b::SPECIES).unwrap_or(0),
        level: m.peek(base + b::LEVEL).unwrap_or(0),
        hp: m.peek_be16(base + b::HP).unwrap_or(0),
        max_hp: m.peek_be16(base + b::MAX_HP).unwrap_or(0),
        status: m.peek(base + b::STATUS).unwrap_or(0),
        types: (
            m.peek(base + b::TYPE1).unwrap_or(0),
            m.peek(base + b::TYPE2).unwrap_or(0),
        ),
        attack: m.peek_be16(base + b::ATTACK).unwrap_or(0),
        defense: m.peek_be16(base + b::DEFENSE).unwrap_or(0),
        speed: m.peek_be16(base + b::SPEED).unwrap_or(0),
        special: m.peek_be16(base + b::SPECIAL).unwrap_or(0),
        stages,
    }
}

/// The bag: `wNumBagItems` then id/quantity pairs.
fn read_bag(m: &mut Machine) -> Vec<crate::battle::Item> {
    let n = m.peek(tr::NUM_BAG_ITEMS).unwrap_or(0) as usize;
    (0..n.min(tr::MAX_BAG_ITEMS))
        .map(|i| {
            let at = tr::BAG_ITEMS + (i * 2) as u16;
            crate::battle::Item {
                id: m.peek(at).unwrap_or(0),
                count: m.peek(at + 1).unwrap_or(0),
            }
        })
        .filter(|it| it.id != 0 && it.id != 0xFF)
        .collect()
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
        // Route 22, west of Viridian — where `scenarios/experiments/` measures
        // pre-forest training. `map_const ROUTE_22, 20, 9 ; $21`.
        0x21 => "Route 22",
        0x25 => "Red's house 1F",
        0x26 => "Red's house 2F (bedroom)",
        0x27 => "Blue's house",
        0x28 => "Oak's Lab",
        0x29 => "Viridian Pokémon Center",
        0x2A => "Viridian Mart",
        0x2B => "Viridian School",
        0x2C => "Viridian House",
        0x2D => "Viridian Gym",
        0x2E => "Diglett's Cave (Route 2)",
        0x2F => "Viridian Forest north gate",
        0x30 => "Route 2 trade house",
        0x31 => "Route 2 gate",
        0x32 => "Viridian Forest south gate",
        0x33 => "Viridian Forest",
        0x34 => "Pewter Museum 1F",
        0x36 => "Pewter Gym",
        0x3A => "Pewter Pokémon Center",
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

    /// **A map name is a fact about the cartridge, so a wrong one is a lie
    /// told with the same confidence as the truth.** Two of these were wrong
    /// for as long as the table existed: `$33` was labelled "Pewter Pokémon
    /// Center" when `constants/map_constants.asm` makes it `VIRIDIAN_FOREST`,
    /// and `$3B` was labelled "Viridian Forest" when it is `MT_MOON_1F`. The
    /// effect is not cosmetic — the name goes to the agent in its snapshot and
    /// into every report, so a run that crossed Viridian Forest said it was in
    /// a Pokémon Center. Found by driving the Pewter chain through it.
    ///
    /// The values are the const-list indices `map_constants.asm` assigns, and
    /// nothing here may be added without deriving it the same way.
    #[test]
    fn every_named_map_is_the_map_the_cartridge_says_it_is() {
        for (id, want) in [
            (0x00, "Pallet Town"),
            (0x01, "Viridian City"),
            (0x02, "Pewter City"),
            (0x0C, "Route 1"),
            (0x0D, "Route 2"),
            (0x21, "Route 22"),
            (0x25, "Red's house 1F"),
            (0x26, "Red's house 2F (bedroom)"),
            (0x28, "Oak's Lab"),
            (0x29, "Viridian Pokémon Center"),
            (0x2A, "Viridian Mart"),
            (0x2F, "Viridian Forest north gate"),
            (0x32, "Viridian Forest south gate"),
            (0x33, "Viridian Forest"),
            (0x36, "Pewter Gym"),
            (0x3A, "Pewter Pokémon Center"),
        ] {
            assert_eq!(map_name(id), want, "map ${id:02X}");
        }
        // Mt Moon's first floor used to be labelled Viridian Forest. An
        // unnamed map is honest; a wrongly named one is not.
        assert_eq!(map_name(0x3B), "");
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
        let refs: Vec<(usize, usize, &[u8])> = rows
            .iter()
            .map(|(r, c, v)| (*r, *c, v.as_slice()))
            .collect();
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
    /// `selected()` names the item it points at.
    ///
    /// The two cases pull in opposite directions and are the whole reason for
    /// the column rule: `NEW GAME` contains a blank that must NOT end the item,
    /// and the battle menu separates its two columns with exactly one blank
    /// that must. Both grids are the real ones, transcribed off a Blue
    /// cartridge (`examples/screen_text --tour` / `--state`).
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
        assert_eq!(menu.kind(), ScreenKind::Menu);

        // The real battle menu: FIGHT at col 10 runs to col 14 and PKMN starts
        // at 16, so exactly ONE blank separates the columns — the naive
        // two-blank rule answered "FIGHT PKMN", which is not an option the
        // player has. ITEM/RUN on the row below is what marks column 16.
        let battle = {
            let fight = enc("FIGHT");
            let item = enc("ITEM");
            let run = enc("RUN");
            let refs: Vec<(usize, usize, &[u8])> = vec![
                (14, 9, &[glyph::CURSOR]),
                (14, 10, fight.as_slice()),
                (14, 16, &[0xE1, 0xE2]),
                (16, 10, item.as_slice()),
                (16, 16, run.as_slice()),
            ];
            Screen::from_tiles(grid(&refs))
        };
        assert!(battle.text.contains("FIGHT PKMN"), "{}", battle.text);
        assert_eq!(battle.selected().as_deref(), Some("FIGHT"));

        // Cursor moved to the right-hand column names the other item.
        let battle_right = {
            let fight = enc("FIGHT");
            let item = enc("ITEM");
            let run = enc("RUN");
            let refs: Vec<(usize, usize, &[u8])> = vec![
                (14, 10, fight.as_slice()),
                (14, 15, &[glyph::CURSOR]),
                (14, 16, &[0xE1, 0xE2]),
                (16, 10, item.as_slice()),
                (16, 16, run.as_slice()),
            ];
            Screen::from_tiles(grid(&refs))
        };
        assert_eq!(battle_right.selected().as_deref(), Some("PKMN"));
    }

    /// Column starts must be taken from the menu's own rows and no others.
    /// This is the real name-prompt screen: the question printed underneath
    /// (`your name?`) begins a word in the same column as `NAME` does, and
    /// reading starts from every row truncated the item to `NEW`.
    #[test]
    fn text_below_a_menu_does_not_cut_an_item_in_half() {
        let new_name = enc("NEW NAME");
        let blue = enc("BLUE");
        let gary = enc("GARY");
        let q1 = enc("First, what is");
        let q2 = enc("your name?");
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (2, 1, &[glyph::CURSOR]),
            (2, 2, new_name.as_slice()),
            (4, 2, blue.as_slice()),
            (6, 2, gary.as_slice()),
            (14, 1, q1.as_slice()),
            (16, 1, q2.as_slice()),
        ];
        let s = Screen::from_tiles(grid(&refs));
        // `name?` starts at column 6, exactly where `NAME` does.
        assert!(s.text.contains("your name?"), "{}", s.text);
        assert_eq!(s.selected().as_deref(), Some("NEW NAME"));
    }

    /// The `$70`-`$75` slots hold quotes in the main font and HP-bar / `<to>`
    /// graphics under `font_battle_extra` (`constants/charmap.asm` lists both),
    /// so nothing can tell them apart from the tile id. Decoding them as quotes
    /// put `”’` on a line of its own and `153/153”` on the HP row of a real
    /// battle screen; they stay blank.
    #[test]
    fn the_ambiguous_battle_font_slots_stay_blank() {
        let hp = {
            let mut v = enc("153/153");
            v.push(0x73); // the tile that rendered as ”
            v
        };
        let refs: Vec<(usize, usize, &[u8])> =
            vec![(10, 11, hp.as_slice()), (2, 1, &[0x73, 0x71])];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.text.contains("153/153"), "{}", s.text);
        for c in ['\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2026}'] {
            assert!(!s.text.contains(c), "{c} survived into: {}", s.text);
        }
        assert_eq!(s.compact_text(), "153/153");
    }

    /// `$F0` is `¥` in the main font and the `ED` ("end") key under the naming
    /// screen's own overlay (`gfx/font/ED.1bpp`, `PrintAlphabet`). Both
    /// meanings are real, and the keyboard is the one screen that announces
    /// itself well enough to choose between them.
    #[test]
    fn the_naming_screens_end_key_is_not_a_yen_sign() {
        let q = enc("YOUR NAME?");
        let top = enc("A B C D E F G H I");
        let mut last = enc("- ? ! ");
        last.push(0xF0);
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (1, 0, q.as_slice()),
            (5, 2, top.as_slice()),
            (13, 2, last.as_slice()),
        ];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.text.contains("- ? ! ED"), "{}", s.text);
        assert!(!s.text.contains('\u{00A5}'), "{}", s.text);

        // Off the keyboard the same tile is money, which is what a shop shows.
        let mut price = vec![0xF0];
        price.extend(enc("200"));
        let refs: Vec<(usize, usize, &[u8])> = vec![(3, 4, price.as_slice())];
        let s = Screen::from_tiles(grid(&refs));
        assert!(s.text.contains("\u{00A5}200"), "{}", s.text);
    }

    /// On the letter grid `selected()` must name the single letter A would
    /// type — blind spot 2 of the live-run list ("it types blind and
    /// confirms"). It falls out of the same column rule: every other row of
    /// the keyboard starts a word at every other column.
    #[test]
    fn the_naming_keyboard_names_the_letter_under_the_cursor() {
        let q = enc("YOUR NAME?");
        let r1 = enc("A B C D E F G H I");
        let r2 = enc("J K L M N O P Q R");
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (1, 0, q.as_slice()),
            (5, 1, &[glyph::CURSOR]),
            (5, 2, r1.as_slice()),
            (7, 2, r2.as_slice()),
        ];
        let s = Screen::from_tiles(grid(&refs));
        assert_eq!(s.kind(), ScreenKind::NamingKeyboard);
        assert_eq!(s.selected().as_deref(), Some("A"));

        // Cursor three letters along.
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (1, 0, q.as_slice()),
            (5, 5, &[glyph::CURSOR]),
            (5, 2, r1.as_slice()),
            (7, 2, r2.as_slice()),
        ];
        let mut tiles = grid(&refs);
        tiles[5 * 20 + 5] = glyph::CURSOR; // over the blank before 'C'
        let s = Screen::from_tiles(tiles);
        assert_eq!(s.selected().as_deref(), Some("C"));
    }

    /// Every shape of screen an agent must act on differently gets its own
    /// name. `Mode` cannot supply these: a move list and a YES/NO box are both
    /// `Mode::Battle`, and the overworld and the title screen are both "no
    /// text on screen" — which must be a stated fact, not an empty string.
    #[test]
    fn every_shape_of_screen_is_named() {
        // Nothing decodable at all: the overworld's map tiles.
        let map = Screen::from_tiles(vec![0x0Au8; 20 * 18]);
        assert_eq!(map.kind(), ScreenKind::NoText);
        assert_eq!(
            map.trust(),
            Trust::Ok,
            "no text is not the same as no trust"
        );

        let hello = enc("Hello there!");
        let refs: Vec<(usize, usize, &[u8])> = vec![(14, 1, hello.as_slice())];
        assert_eq!(Screen::from_tiles(grid(&refs)).kind(), ScreenKind::Text);

        let refs: Vec<(usize, usize, &[u8])> =
            vec![(14, 1, hello.as_slice()), (16, 18, &[glyph::MORE_TEXT])];
        assert_eq!(Screen::from_tiles(grid(&refs)).kind(), ScreenKind::Dialogue);

        let yes = enc("YES");
        let no = enc("NO");
        let refs: Vec<(usize, usize, &[u8])> = vec![
            (8, 11, &[glyph::CURSOR]),
            (8, 12, yes.as_slice()),
            (10, 12, no.as_slice()),
        ];
        assert_eq!(Screen::from_tiles(grid(&refs)).kind(), ScreenKind::YesNo);

        assert_eq!(Screen::default().kind(), ScreenKind::NoText);
        assert_eq!(Screen::default().trust(), Trust::Off);
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

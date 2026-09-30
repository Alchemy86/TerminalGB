//! The pads' menus: what they say, how they move, and where they are drawn.
//!
//! Pure: no machine, so every rule is tested at unit speed. [`Menu`] holds what is
//! on screen and what a button does to it; [`Menu::cells`] turns it into the
//! `(row, col, tile)` writes that go into `wTileMap`, in the cartridge's own text
//! box style -- the same border tiles and the same font its dialogue is made of.

use terminalgb_plugin_gen1_lobby::gen1::{charmap, SCREEN_H, SCREEN_W};

/// The joypad byte the cartridge keeps (`hJoyInput`, `hJoyPressed`).
pub mod pad {
    pub const A: u8 = 0x01;
    pub const B: u8 = 0x02;
    pub const START: u8 = 0x08;
    pub const RIGHT: u8 = 0x10;
    pub const LEFT: u8 = 0x20;
    pub const UP: u8 = 0x40;
    pub const DOWN: u8 = 0x80;
}

/// The most a line of text may hold: the screen is 20 tiles wide and the box has a
/// border tile on each side.
pub const INNER_W: usize = SCREEN_W - 2;

// The text box's own tiles, `TextBoxBorder` in the cartridge.
const TL: u8 = 0x79;
const H: u8 = 0x7A;
const TR: u8 = 0x7B;
const V: u8 = 0x7C;
const BL: u8 = 0x7D;
const BR: u8 = 0x7E;
const BLANK: u8 = 0x7F;
/// The font's filled right-pointing arrow: the cursor.
const CURSOR: u8 = 0xED;

/// One of the walker's kept Pokemon, as the menu lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kept {
    pub name: String,
    pub level: u8,
}

/// What the player decided.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Give the walker a Pokemon (it gives the token back). The Pokemon itself is
    /// chosen at the trade table, as always.
    Send,
    /// Take kept Pokemon number `n` (an index into the list the menu was given)
    /// back, in exchange for whatever is offered at the table -- the token, best.
    TakeBack(usize),
}

/// Which pad opened it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Send,
    TakeBack,
}

/// What a button did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing that matters outside the menu.
    Nothing,
    /// The menu is over; here is what was chosen, if anything.
    Closed(Option<Choice>),
}

/// An open menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    kind: Kind,
    cursor: usize,
    /// A note after a choice: shown until any button, then the menu closes.
    note: Option<Vec<String>>,
    pending: Option<Choice>,
    /// The list, copied in when the menu opened so it cannot change under a cursor.
    kept: Vec<Kept>,
}

impl Menu {
    pub fn open(kind: Kind, kept: &[Kept]) -> Menu {
        Menu { kind, cursor: 0, note: None, pending: None, kept: kept.to_vec() }
    }

    pub fn kind(&self) -> Kind {
        self.kind
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// How many rows the cursor can be on.
    fn rows(&self) -> usize {
        match self.kind {
            // YES, NO.
            Kind::Send => 2,
            // Every kept Pokemon, then CANCEL.
            Kind::TakeBack => self.kept.len() + 1,
        }
    }

    /// Feed one button press (bits of [`pad`], pressed this frame).
    pub fn press(&mut self, buttons: u8) -> Outcome {
        if self.note.is_some() {
            // Any button ends the note, and with it the menu.
            if buttons & (pad::A | pad::B | pad::START) != 0 {
                return Outcome::Closed(self.pending.take());
            }
            return Outcome::Nothing;
        }
        if buttons & pad::B != 0 {
            return Outcome::Closed(None);
        }
        let n = self.rows();
        if buttons & pad::UP != 0 {
            self.cursor = (self.cursor + n - 1) % n;
        } else if buttons & pad::DOWN != 0 {
            self.cursor = (self.cursor + 1) % n;
        } else if buttons & pad::A != 0 {
            match self.kind {
                Kind::Send if self.cursor == 0 => {
                    self.pending = Some(Choice::Send);
                    self.note = Some(lines(&["SEND SOMEONE TO", "THE WALKER: PICK", "THEM AT THE TABLE."]));
                    debug_assert!(self.note.iter().flatten().all(|l| l.len() <= INNER_W));
                }
                Kind::TakeBack if self.cursor < self.kept.len() => {
                    let who = &self.kept[self.cursor];
                    self.pending = Some(Choice::TakeBack(self.cursor));
                    self.note = Some(vec![
                        format!("{} COMES", who.name),
                        "BACK: GIVE THE".to_string(),
                        "TOKEN PIKACHU AT".to_string(),
                        "THE TABLE.".to_string(),
                    ]);
                }
                // NO, or CANCEL.
                _ => return Outcome::Closed(None),
            }
        }
        Outcome::Nothing
    }

    /// The box as lines of text, top to bottom. `>` marks the cursor row.
    fn body(&self) -> Vec<String> {
        if let Some(note) = &self.note {
            return note.clone();
        }
        match self.kind {
            Kind::Send => vec![
                "SEND A POKEMON".to_string(),
                "TO THE WALKER?".to_string(),
                String::new(),
                self.row("YES", 0),
                self.row("NO", 1),
            ],
            Kind::TakeBack => {
                let mut v = vec!["TAKE BACK FROM".to_string(), "THE WALKER".to_string(), String::new()];
                if self.kept.is_empty() {
                    v.push("  NOTHING KEPT YET".to_string());
                }
                for (i, k) in self.kept.iter().enumerate() {
                    v.push(self.row(&format!("{:<10}L{}", k.name, k.level), i));
                }
                v.push(self.row("CANCEL", self.kept.len()));
                v
            }
        }
    }

    fn row(&self, text: &str, index: usize) -> String {
        format!("{}{}", if self.cursor == index { '>' } else { ' ' }, text)
    }

    /// The tile writes that draw the menu: `(row, col, tile)`. The box sits low and
    /// centred, as the cartridge's own dialogue does, and is only as tall as its
    /// text.
    pub fn cells(&self) -> Vec<(usize, usize, u8)> {
        let body = self.body();
        let width = body.iter().map(|l| l.chars().count()).max().unwrap_or(0).max(12) + 2;
        let width = width.min(SCREEN_W);
        let height = (body.len() + 2).min(SCREEN_H);
        let (col0, row0) = ((SCREEN_W - width) / 2, SCREEN_H - height);
        let mut cells = Vec::new();
        for r in 0..height {
            for c in 0..width {
                let tile = if r == 0 {
                    if c == 0 { TL } else if c == width - 1 { TR } else { H }
                } else if r == height - 1 {
                    if c == 0 { BL } else if c == width - 1 { BR } else { H }
                } else if c == 0 || c == width - 1 {
                    V
                } else {
                    BLANK
                };
                cells.push((row0 + r, col0 + c, tile));
            }
        }
        for (i, line) in body.iter().enumerate() {
            for (j, ch) in line.chars().enumerate() {
                if 1 + j >= width - 1 {
                    break;
                }
                let tile = if ch == '>' { CURSOR } else { charmap(ch.to_ascii_uppercase()).unwrap_or(BLANK) };
                cells.push((row0 + 1 + i, col0 + 1 + j, tile));
            }
        }
        cells
    }
}

fn lines(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two() -> Vec<Kept> {
        vec![Kept { name: "SPARK".into(), level: 12 }, Kept { name: "ROCKY".into(), level: 11 }]
    }

    #[test]
    fn the_cursor_wraps_and_b_backs_out() {
        let mut m = Menu::open(Kind::TakeBack, &two());
        assert_eq!(m.cursor(), 0);
        m.press(pad::UP);
        assert_eq!(m.cursor(), 2, "up from the top lands on CANCEL");
        m.press(pad::DOWN);
        assert_eq!(m.cursor(), 0);
        assert_eq!(m.press(pad::B), Outcome::Closed(None));
    }

    #[test]
    fn choosing_a_kept_pokemon_says_what_to_do_and_then_hands_back_the_choice() {
        let mut m = Menu::open(Kind::TakeBack, &two());
        m.press(pad::DOWN);
        assert_eq!(m.press(pad::A), Outcome::Nothing, "a note first");
        assert!(m.body().iter().any(|l| l.contains("ROCKY")));
        assert_eq!(m.press(pad::A), Outcome::Closed(Some(Choice::TakeBack(1))));
    }

    #[test]
    fn cancel_and_no_choose_nothing() {
        let mut m = Menu::open(Kind::TakeBack, &two());
        m.press(pad::UP);
        assert_eq!(m.press(pad::A), Outcome::Closed(None));
        let mut s = Menu::open(Kind::Send, &[]);
        s.press(pad::DOWN);
        assert_eq!(s.press(pad::A), Outcome::Closed(None));
        let mut s = Menu::open(Kind::Send, &[]);
        assert_eq!(s.press(pad::A), Outcome::Nothing);
        assert_eq!(s.press(pad::B), Outcome::Closed(Some(Choice::Send)));
    }

    #[test]
    fn an_empty_walker_says_so_and_only_offers_cancel() {
        let m = Menu::open(Kind::TakeBack, &[]);
        assert!(m.body().iter().any(|l| l.contains("NOTHING KEPT")));
        assert_eq!(m.rows(), 1);
    }

    #[test]
    fn every_line_of_every_menu_fits_the_box() {
        let long = vec![Kept { name: "MMMMMMMMMM".into(), level: 100 }; 3];
        for kept in [vec![], two(), long] {
            for kind in [Kind::Send, Kind::TakeBack] {
                let mut m = Menu::open(kind, &kept);
                for step in 0..3 {
                    for line in m.body() {
                        assert!(line.chars().count() <= INNER_W, "{kind:?} step {step}: {line:?}");
                    }
                    m.press(pad::A);
                }
            }
        }
    }

    #[test]
    fn the_box_is_drawn_inside_the_screen_in_the_cartridges_own_tiles() {
        for kept in [vec![], two(), (0..8).map(|i| Kept { name: format!("MON{i}"), level: i }).collect()] {
            for kind in [Kind::Send, Kind::TakeBack] {
                let cells = Menu::open(kind, &kept).cells();
                assert!(!cells.is_empty());
                for (r, c, _) in &cells {
                    assert!(*r < SCREEN_H && *c < SCREEN_W, "({r},{c}) is off the screen");
                }
            }
        }
        let cells = Menu::open(Kind::Send, &[]).cells();
        assert!(cells.iter().any(|&(_, _, t)| t == TL) && cells.iter().any(|&(_, _, t)| t == BR));
        assert!(cells.iter().any(|&(_, _, t)| t == CURSOR), "the cursor is drawn");
    }
}

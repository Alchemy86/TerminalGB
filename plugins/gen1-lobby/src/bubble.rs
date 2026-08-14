//! Speech: a canned phrase drawn over the speaker's head in the cartridge's
//! own font.
//!
//! # The output is the constraint, not the input
//!
//! A Game Boy has no keyboard; TerminalGB's player has one. So *typing* is a
//! solved problem in this codebase and the interesting limit is the other end:
//! text reaches the screen the way the game's own text does, as charmap bytes
//! written into `wTileMap`, and the resident font covers `A-Z a-z 0-9` plus
//! `( ) : ; [ ] ' - ? ! . / ,` **and nothing else**. There is no
//! `@ # & * " _ + = < >`, which means no e-mail addresses, no URLs and no
//! `@handles` — a rather good accident for a public room.
//!
//! [`crate::gen1::filter_to_charmap`] drops what cannot be drawn rather than
//! mangling it, and this module refuses to draw a phrase that filtered down to
//! nothing rather than showing an empty box.
//!
//! # Why the phrases are canned in this version
//!
//! Free text needs an answer to "what happens when a stranger types something
//! unpleasant", and that is a decision, not an implementation. Until it exists
//! the room ships a short menu: no moderation problem, no glyph problem,
//! instantly readable, and entirely in keeping with a game where everyone you
//! meet says one line.

use crate::gen1::*;
use crate::screen::Blockset;
use terminalgb_plugin_api::Machine;

/// The phrase menu. Every one of them is inside the charmap and inside the
/// width a bubble can hold.
pub const PHRASES: [&str; 8] = [
    "HI!",
    "WANT TO TRADE?",
    "NICE TEAM!",
    "BATTLE ME!",
    "FOLLOW ME",
    "THANKS!",
    "WAIT HERE",
    "BYE!",
];

/// The widest phrase a bubble draws, in tiles. Two rows of this is the whole
/// budget; anything longer is truncated rather than wrapped off the screen.
pub const BUBBLE_W: usize = 18;

/// How long a phrase stays up, in emulated frames. Two seconds at 59.7275 Hz —
/// long enough to read, short enough that a crowd does not become a wall of
/// text.
pub const BUBBLE_FRAMES: u32 = 120;

/// One thing being said, and where.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Speech {
    pub text: String,
    /// The speaker's walk tile.
    pub x: u8,
    pub y: u8,
}

/// The screen row a speaker's bubble sits on: two rows above their own head,
/// clamped onto the screen so someone at the top of the view still gets one.
fn bubble_row(me_y: u8, y: u8) -> usize {
    let dy = y as i32 - me_y as i32;
    let head = PLAYER_SCREEN_ROW + dy * 2;
    (head - 2).clamp(0, SCREEN_H as i32 - 1) as usize
}

/// Where the bubble starts, so it is centred on the speaker and never runs off
/// either edge.
fn bubble_col(me_x: u8, x: u8, len: usize) -> usize {
    let dx = x as i32 - me_x as i32;
    let centre = PLAYER_SCREEN_COL + dx * 2;
    (centre - len as i32 / 2).clamp(0, (SCREEN_W - len) as i32) as usize
}

/// What a bubble will occupy: `(row, col, charmap bytes)`. Pure, so the layout
/// is testable without a machine.
pub fn layout(me: (u8, u8), s: &Speech) -> Option<(usize, usize, Vec<u8>)> {
    let filtered = filter_to_charmap(&s.text);
    let trimmed = filtered.trim();
    if trimmed.is_empty() {
        return None;
    }
    let bytes: Vec<u8> = trimmed.chars().take(BUBBLE_W).filter_map(charmap).collect();
    if bytes.is_empty() {
        return None;
    }
    Some((
        bubble_row(me.1, s.y),
        bubble_col(me.0, s.x, bytes.len()),
        bytes,
    ))
}

/// Draw the current speech and report which screen rows were borrowed, so the
/// caller can hand them back.
pub fn draw(m: &mut Machine<'_>, speech: &[Speech]) -> Vec<usize> {
    let me = (
        m.peek(W_X_COORD).unwrap_or(0),
        m.peek(W_Y_COORD).unwrap_or(0),
    );
    let mut rows = Vec::new();
    for s in speech {
        let Some((row, col, bytes)) = layout(me, s) else {
            continue;
        };
        for (i, b) in bytes.iter().enumerate() {
            m.poke(W_TILE_MAP + (row * SCREEN_W + col + i) as u16, *b);
        }
        if !rows.contains(&row) {
            rows.push(row);
        }
    }
    rows
}

/// Hand the borrowed rows back to the engine's own picture.
pub fn undraw(
    m: &mut Machine<'_>,
    room: &crate::room::Room,
    blocks: &[u8],
    bs: &Blockset,
    rows: &[usize],
    anchor_row: i32,
) {
    crate::screen::repaint_rows(m, room, blocks, bs, rows, anchor_row);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_canned_phrase_survives_the_cartridges_own_charmap() {
        for p in PHRASES {
            assert_eq!(filter_to_charmap(p), p, "{p} loses characters");
            assert!(p.len() <= BUBBLE_W, "{p} is wider than a bubble");
        }
    }

    #[test]
    fn a_phrase_of_nothing_but_unusable_glyphs_draws_nothing_at_all() {
        // An empty box would read as "they said something I cannot see".
        let s = Speech {
            text: "@#$%^&*_=+".into(),
            x: 10,
            y: 10,
        };
        assert_eq!(layout((10, 10), &s), None);
    }

    #[test]
    fn a_bubble_never_runs_off_either_edge_of_the_screen() {
        for x in 0..40u8 {
            for y in 0..28u8 {
                let s = Speech {
                    text: "WANT TO TRADE?".into(),
                    x,
                    y,
                };
                if let Some((row, col, bytes)) = layout((20, 14), &s) {
                    assert!(col + bytes.len() <= SCREEN_W, "x={x} col={col}");
                    assert!(row < SCREEN_H, "y={y} row={row}");
                }
            }
        }
    }

    #[test]
    fn the_bubble_sits_above_the_speaker_and_is_centred_on_them() {
        let s = Speech {
            text: "HI!".into(),
            x: 10,
            y: 10,
        };
        let (row, col, bytes) = layout((10, 10), &s).unwrap();
        assert_eq!(bytes.len(), 3);
        // Two rows above the top of the player's own walk tile.
        assert_eq!(row, PLAYER_SCREEN_ROW as usize - 2);
        // Centred on the player's own column.
        assert_eq!(col, PLAYER_SCREEN_COL as usize - 1);
    }

    #[test]
    fn the_charmap_is_the_cartridges_and_the_gaps_are_deliberate() {
        assert_eq!(charmap('A'), Some(0x80));
        assert_eq!(charmap('Z'), Some(0x99));
        assert_eq!(charmap('a'), Some(0xA0));
        assert_eq!(charmap('0'), Some(0xF6));
        assert_eq!(charmap('9'), Some(0xFF));
        assert_eq!(charmap(' '), Some(0x7F));
        assert_eq!(charmap('!'), Some(0xE7));
        // The absences that matter.
        for c in ['@', '#', '&', '*', '"', '_', '+', '=', '<', '>', '%'] {
            assert_eq!(charmap(c), None, "{c} has no glyph in Gen 1");
        }
    }
}

//! Wild or trainer, **from the picture**.
//!
//! # Why this exists beside a one-byte memory read
//!
//! [`terminalgb_plugin_trainer::gen1::BattleKind`] answers the same question
//! from `wIsInBattle`, exactly and for nothing, and that is what the
//! memory-derived expert uses. A student that only ever sees the screen has no
//! such byte, and the question is not cosmetic for it: a Poké Ball thrown at a
//! trainer's Pokémon is refused by the game, so a policy that cannot tell the
//! two apart either wastes turns or never throws at all.
//!
//! So the discriminator is established **twice**, and the two are scored
//! against each other by `examples/gen1_catch_drill.rs kinds` — the memory read
//! is the truth, this is the candidate, and the confusion rate is published
//! rather than assumed.
//!
//! # What actually separates them on screen
//!
//! Two signals, and they are very different in character:
//!
//! 1. **The opening text.** `WildMonAppearedText` reads `Wild <NAME>\nappeared!`
//!    and `TrainerWantsToFightText` reads `<NAME> wants\nto fight!` — both
//!    located in this cartridge at bank `$22` (file `0x89c1e` and `0x89c54` on
//!    the Blue image this was read from) by decoding the charmap. Decisive, and
//!    **transient**: it is gone by the time the battle menu is up.
//! 2. **The opponent's Poké Ball row.** `DrawAllPokeballs`
//!    (`engine/battle/core.asm`) opens `ld a, [wIsInBattle] / dec a / ret z` —
//!    *return if wild* — so the row of ball icons showing how many Pokémon each
//!    side has left is drawn for a trainer battle and for nothing else. It is
//!    on screen for the whole battle, which is what makes it the signal a
//!    policy can consult at the moment it wants to throw.
//!
//! [`BALL_ROW_TILES`] and [`BALL_ROW`] are **measured**, not transcribed: the
//! drill synthesises the two battles from one save with only the preset's
//! `wild =`/`trainer =` line different, diffs the two tilemaps, and the cells
//! that move are these. See `docs/gen1/catching.md`.

/// What the screen says, with a third answer for "it does not say".
///
/// **`Unknown` is a real verdict and must stay reachable.** A battle whose
/// intro text has gone and whose HUD is mid-redraw genuinely does not say which
/// kind it is, and a discriminator that guessed there would be wrong silently.
/// The catch policy treats `Unknown` as "do not throw", which is the safe
/// direction: a ball not thrown costs a turn, a ball thrown at a trainer's
/// Pokémon may cost the ball.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenVerdict {
    Wild,
    Trainer,
    #[default]
    Unknown,
}

impl ScreenVerdict {
    pub fn label(self) -> &'static str {
        match self {
            ScreenVerdict::Wild => "wild",
            ScreenVerdict::Trainer => "trainer",
            ScreenVerdict::Unknown => "unknown",
        }
    }
}

/// The screen row the opponent's ball icons are drawn on.
///
/// Measured, see the module docs.
pub const BALL_ROW: usize = 0;

/// The columns of that row the icons occupy.
pub const BALL_COLS: std::ops::Range<usize> = 1..7;

/// The tile ids `DrawAllPokeballs` writes. A trainer battle draws one per
/// Pokémon the trainer has; a wild battle draws none of them anywhere.
///
/// Measured, see the module docs.
pub const BALL_ROW_TILES: &[u8] = &[0x71, 0x72, 0x73];

/// Judge a 20x18 tilemap (`wTileMap`, `$C3A0`).
pub fn from_tiles(tiles: &[u8]) -> ScreenVerdict {
    if tiles.len() < 20 * 18 {
        return ScreenVerdict::Unknown;
    }
    if has_ball_row(tiles) {
        return ScreenVerdict::Trainer;
    }
    from_text(&terminalgb_plugin_trainer::text::decode_screen(tiles))
        .unwrap_or(ScreenVerdict::Unknown)
}

/// Is the opponent's ball row on screen?
pub fn has_ball_row(tiles: &[u8]) -> bool {
    if tiles.len() < 20 * 18 {
        return false;
    }
    BALL_COLS
        .clone()
        .any(|c| BALL_ROW_TILES.contains(&tiles[BALL_ROW * 20 + c]))
}

/// Judge the decoded text alone. `None` when it says neither, which is most
/// of a battle.
///
/// The two phrases are the cartridge's own and are quoted from its charmap, not
/// from a wiki: `Wild @\nappeared!` and `@ wants\nto fight!`, where `@` is the
/// name the game splices in. Matching on the fixed halves is what makes this
/// immune to the name.
pub fn from_text(text: &str) -> Option<ScreenVerdict> {
    let flat = flatten(text);
    if flat.contains("wants to fight") {
        return Some(ScreenVerdict::Trainer);
    }
    // "Wild <NAME> appeared!" — and **not** merely "appeared!", which a
    // trainer's own send-out also prints (`<NAME> appeared!`, the line right
    // after "wants to fight"). The word `Wild` is the whole of the signal.
    if flat.contains("Wild ") && flat.contains("appeared") {
        return Some(ScreenVerdict::Wild);
    }
    // The refusal itself, for a policy that threw anyway: `TrainerBlockedText`
    // is "The trainer\nblocked the BALL!" and can only appear in one kind of
    // battle.
    if flat.contains("blocked the") {
        return Some(ScreenVerdict::Trainer);
    }
    None
}

/// A decoded screen is a 20x18 grid, so a phrase the game wrapped across two
/// lines is broken by a newline. Flattening lets one pattern match either
/// layout.
fn flatten(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = true;
    for c in text.chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
                space = true;
            }
        } else {
            out.push(c);
            space = false;
        }
    }
    out.trim().to_string()
}

/// A latch, because **the screen only says it once**.
///
/// The measurement is in `docs/gen1/catching.md`: at the battle's own menu —
/// the moment a policy decides whether to throw — a wild battle and a trainer
/// battle with the same opponent are identical to the cell, so a stateless
/// reader is `Unknown` every time. The signal is the opening line, and it is
/// gone within a couple of seconds.
///
/// So a screen-only player has to do what a person does: read the first line
/// and remember it for the rest of the battle. `Watcher` is that memory, and
/// nothing more — it holds one verdict, takes the **first** one the screen
/// offers, and is reset by whoever notices the battle ended.
///
/// A later `Trainer` still overrides an earlier `Wild`, and only in that
/// direction: `TrainerBlockedText` is proof, and being wrong towards "trainer"
/// costs a turn while being wrong towards "wild" can cost a ball.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Watcher {
    verdict: ScreenVerdict,
}

impl Watcher {
    /// Feed it a tilemap. Cheap enough to call every sample — one decode and a
    /// couple of `contains`.
    pub fn observe(&mut self, tiles: &[u8]) {
        let seen = from_tiles(tiles);
        match (self.verdict, seen) {
            (_, ScreenVerdict::Unknown) => {}
            (ScreenVerdict::Unknown, s) => self.verdict = s,
            // Only ever upgrade towards `Trainer`; see the type docs.
            (ScreenVerdict::Wild, ScreenVerdict::Trainer) => {
                self.verdict = ScreenVerdict::Trainer
            }
            _ => {}
        }
    }

    pub fn verdict(&self) -> ScreenVerdict {
        self.verdict
    }

    /// A battle ended. **Forgetting is not optional** — carrying a verdict into
    /// the next battle is exactly the failure this whole discriminator exists
    /// to avoid, and the next battle need not be the same kind.
    pub fn reset(&mut self) {
        self.verdict = ScreenVerdict::Unknown;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The screen says it once. A latch that forgot would be `Unknown` for the
    /// whole of the battle after the first two seconds — which is measured, and
    /// is the reason this type exists.
    #[test]
    fn the_watcher_remembers_a_line_that_is_no_longer_on_screen() {
        let mut w = Watcher::default();
        assert_eq!(w.verdict(), ScreenVerdict::Unknown);
        w.observe(&tilemap("Wild RATTATA appeared!"));
        assert_eq!(w.verdict(), ScreenVerdict::Wild);
        // The battle menu says nothing at all, and the verdict survives it.
        w.observe(&tilemap("FIGHT PKMN ITEM RUN"));
        assert_eq!(w.verdict(), ScreenVerdict::Wild);
        w.reset();
        assert_eq!(w.verdict(), ScreenVerdict::Unknown);
    }

    /// A refusal outranks an earlier "wild", and nothing outranks a refusal.
    #[test]
    fn only_the_upgrade_towards_trainer_is_allowed() {
        let mut w = Watcher::default();
        w.observe(&tilemap("Wild RATTATA appeared!"));
        w.observe(&tilemap("The trainer blocked the BALL!"));
        assert_eq!(w.verdict(), ScreenVerdict::Trainer);
        // …and it does not go back.
        w.observe(&tilemap("Wild RATTATA appeared!"));
        assert_eq!(w.verdict(), ScreenVerdict::Trainer);
    }

    /// Write a phrase into a 20x18 tilemap the way the game does — one
    /// character per cell, in the charmap the decoder reads back.
    fn tilemap(text: &str) -> Vec<u8> {
        let mut t = vec![0x7Fu8; 20 * 18];
        for (i, c) in text.chars().enumerate() {
            let cell = 20 * 12 + i;
            if cell >= t.len() {
                break;
            }
            t[cell] = match c {
                'A'..='Z' => 0x80 + (c as u8 - b'A'),
                'a'..='z' => 0xA0 + (c as u8 - b'a'),
                '!' => 0xE7,
                _ => 0x7F,
            };
        }
        t
    }

    #[test]
    fn the_two_intro_lines_are_told_apart_however_the_game_wraps_them() {
        assert_eq!(
            from_text("Wild RATTATA\nappeared!"),
            Some(ScreenVerdict::Wild)
        );
        assert_eq!(
            from_text("YOUNGSTER BEN\nwants to fight!"),
            Some(ScreenVerdict::Trainer)
        );
        // The wrap point is the game's and moves with the name's length.
        assert_eq!(
            from_text("YOUNGSTER JOE wants\nto fight!"),
            Some(ScreenVerdict::Trainer)
        );
    }

    /// A trainer sending out its next Pokémon prints `<NAME> appeared!` with no
    /// `Wild` in front of it. Matching on `appeared` alone would call that a
    /// wild battle — in the middle of a trainer battle, which is the one place
    /// the answer must not be wrong.
    #[test]
    fn a_trainers_send_out_is_not_a_wild_appearance() {
        assert_eq!(from_text("RATTATA\nappeared!"), None);
    }

    #[test]
    fn a_screen_that_says_nothing_says_nothing() {
        assert_eq!(from_text(""), None);
        assert_eq!(from_text("FIGHT PKMN\nITEM  RUN"), None);
        assert_eq!(from_tiles(&[0u8; 20 * 18]), ScreenVerdict::Unknown);
        // A short buffer is not a screen.
        assert_eq!(from_tiles(&[0u8; 10]), ScreenVerdict::Unknown);
    }

    #[test]
    fn the_ball_row_is_a_trainer_battle_wherever_the_text_has_gone() {
        let mut t = [0u8; 20 * 18];
        t[BALL_ROW * 20 + 2] = BALL_ROW_TILES[0];
        assert!(has_ball_row(&t));
        assert_eq!(from_tiles(&t), ScreenVerdict::Trainer);
    }

    /// The refusal is itself a verdict: if a ball was thrown and the game said
    /// "The trainer blocked the BALL!", the battle's kind is settled.
    #[test]
    fn the_refusal_names_the_kind() {
        assert_eq!(
            from_text("The trainer\nblocked the BALL!"),
            Some(ScreenVerdict::Trainer)
        );
    }
}

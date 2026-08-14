//! Offering a Pokémon **from the loaded game** — the picker's state machine
//! and the offer spec it produces.
//!
//! The rule this module exists to enforce: an offer never comes from a form —
//! it comes from [`terminalgb_plugin_trainer::collection`], which read the
//! party and the PC boxes out of the running save. If a Pokémon cannot be
//! read out of the loaded game, it cannot be offered; if it cannot legally
//! cross a trade (its entry carries a `refusal`), it is shown greyed with the
//! reason and cannot be armed.
//!
//! The terminal frontend owns the drawing; everything about *what the picker
//! does* lives here so it is testable without a terminal.

use terminalgb_plugin_gen1_link::spec::MonSpec;

// Re-exported so `testharness/portal.rs` (a `harness = false` test) can build
// entries **without naming the plugin crate**. A test target that links a
// plugin crate the lib also links intermittently resolves it to a second,
// unwind-built rlib and fails E0277 on traits the type plainly implements —
// the two-rlib trap recorded in `docs/portal.md` and AGENTS.md. Harnesses
// talk only to `terminalgb` re-exports.
pub use terminalgb_plugin_trainer::collection::{Collection, Location, Offerable};
pub use terminalgb_plugin_trainer::gen1::Dvs;

/// The offer picker: the player's collection, a filter, and a selection.
pub struct OfferPicker {
    /// Every Pokémon read out of the save, party first, in storage order.
    pub entries: Vec<Offerable>,
    /// Honest gaps from the read (an unreadable box, a corrupt one).
    pub notes: Vec<String>,
    /// The player whose save this is, for the header.
    pub player: String,
    /// Live filter text; empty shows everything.
    pub filter: String,
    /// Indices into `entries` that match the filter.
    visible: Vec<usize>,
    /// Position **within `visible`**.
    pub selected: usize,
    /// Set when the player has pressed Enter once on an offerable row; the
    /// second Enter submits. Any other key disarms.
    pub armed: bool,
}

impl OfferPicker {
    pub fn new(c: Collection) -> OfferPicker {
        let mut p = OfferPicker {
            entries: c.entries,
            notes: c.notes,
            player: c.player_name,
            filter: String::new(),
            visible: Vec::new(),
            selected: 0,
            armed: false,
        };
        p.refilter();
        p
    }

    /// One entry's searchable text. Location, nickname, species and level all
    /// match, and a Pokémon flagged by the trainer panel's prediction also
    /// matches the word `shiny` — so typing `shiny` filters to the ones that
    /// **would be shiny if traded to Gen 2** (Gen 1 itself has none; the
    /// picker's legend says so in those words).
    fn haystack(e: &Offerable) -> String {
        let mut h = format!(
            "{} {} {} lv{}",
            e.location.label(),
            e.nickname,
            e.species_name,
            e.level
        )
        .to_lowercase();
        if e.shiny_if_traded {
            h.push_str(" shiny");
        }
        h
    }

    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        let words: Vec<&str> = f.split_whitespace().collect();
        self.visible = (0..self.entries.len())
            .filter(|&i| {
                let h = Self::haystack(&self.entries[i]);
                words.iter().all(|w| h.contains(w))
            })
            .collect();
        self.selected = self.selected.min(self.visible.len().saturating_sub(1));
        self.armed = false;
    }

    /// The rows the filter leaves, as `(entry index, entry)`.
    pub fn visible(&self) -> impl Iterator<Item = (usize, &Offerable)> {
        self.visible.iter().map(|&i| (i, &self.entries[i]))
    }

    pub fn visible_len(&self) -> usize {
        self.visible.len()
    }

    /// The highlighted entry.
    pub fn current(&self) -> Option<&Offerable> {
        self.visible.get(self.selected).map(|&i| &self.entries[i])
    }

    pub fn type_char(&mut self, c: char) {
        self.filter.push(c);
        self.refilter();
    }

    pub fn backspace(&mut self) {
        self.filter.pop();
        self.refilter();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() - 1;
        self.selected = self.selected.saturating_add_signed(delta).min(last);
        self.armed = false;
    }

    /// Enter. First press arms the highlighted Pokémon (the confirm step);
    /// the second returns the spec to submit. A row with a refusal never
    /// arms — the offer must be one the trade can actually carry.
    pub fn confirm(&mut self) -> Option<MonSpec> {
        if self.current().is_none_or(|e| e.refusal.is_some()) {
            return None;
        }
        if !self.armed {
            self.armed = true;
            return None;
        }
        self.armed = false;
        self.current().map(spec_for)
    }
}

/// The offer spec for one owned Pokémon — nothing here is typed by anyone:
/// every field was read out of the save (and the names resolved through the
/// cartridge's own tables).
pub fn spec_for(e: &Offerable) -> MonSpec {
    MonSpec {
        species: e.species_name.clone(),
        level: e.level,
        nickname: e.nickname.clone(),
        ot_name: e.ot_name.clone(),
        ot_id: e.ot_id,
        moves: e.moves.clone(),
        dvs: (e.dvs.attack, e.dvs.defense, e.dvs.speed, e.dvs.special),
    }
}

/// One line describing an entry, for toasts and confirmations.
pub fn describe(e: &Offerable) -> String {
    let name = if e.nickname.is_empty() {
        e.species_name.clone()
    } else {
        e.nickname.clone()
    };
    format!("{name} the {} (Lv{})", e.species_name, e.level)
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_trainer::collection::Location;
    use terminalgb_plugin_trainer::gen1::Dvs;

    fn entry(nick: &str, species: &str, level: u8, shiny: bool) -> Offerable {
        Offerable {
            location: Location::Party(0),
            species: 1,
            species_name: species.into(),
            dex: 1,
            nickname: nick.into(),
            level,
            ot_name: "RED".into(),
            ot_id: 12345,
            moves: vec!["POUND".into()],
            dvs: if shiny {
                // Attack bit 1 set, def/spd/spc exactly 10 — the one rule,
                // from the trainer panel's implementation.
                Dvs {
                    attack: 2,
                    defense: 10,
                    speed: 10,
                    special: 10,
                }
            } else {
                Dvs::default()
            },
            shiny_if_traded: shiny,
            refusal: None,
        }
    }

    fn picker(entries: Vec<Offerable>) -> OfferPicker {
        OfferPicker::new(Collection {
            player_name: "RED".into(),
            player_id: 1,
            entries,
            notes: Vec::new(),
        })
    }

    #[test]
    fn the_filter_matches_species_nickname_and_the_shiny_prediction() {
        let mut p = picker(vec![
            entry("BULBA", "BULBASAUR", 14, false),
            entry("GLIMMER", "ABRA", 9, true),
            entry("WORM", "CATERPIE", 3, false),
        ]);
        assert_eq!(p.visible_len(), 3);
        for c in "abra".chars() {
            p.type_char(c);
        }
        assert_eq!(p.visible_len(), 1);
        assert_eq!(p.current().unwrap().nickname, "GLIMMER");
        p.filter.clear();
        // Typing `shiny` filters to the would-be-shiny ones — the flag the
        // captain asked to be filterable.
        for c in "shiny".chars() {
            p.type_char(c);
        }
        assert_eq!(p.visible_len(), 1);
        assert!(p.current().unwrap().shiny_if_traded);
    }

    #[test]
    fn a_refused_entry_can_never_be_armed_or_submitted() {
        let mut refused = entry("MEW8", "MEW", 5, false);
        refused.refusal = Some("nickname contains the digit 8".into());
        let mut p = picker(vec![refused]);
        assert!(p.confirm().is_none(), "first Enter must not arm it");
        assert!(!p.armed);
        assert!(p.confirm().is_none(), "…nor may a second submit it");
    }

    #[test]
    fn enter_arms_and_enter_again_submits_the_saves_own_fields() {
        let mut p = picker(vec![entry("BULBA", "BULBASAUR", 14, false)]);
        assert!(p.confirm().is_none());
        assert!(p.armed, "first Enter arms");
        let spec = p.confirm().expect("second Enter submits");
        assert_eq!(spec.species, "BULBASAUR");
        assert_eq!(spec.nickname, "BULBA");
        assert_eq!(spec.level, 14);
        assert_eq!(spec.ot_name, "RED");
        assert_eq!(spec.ot_id, 12345);
        assert_eq!(spec.moves, vec!["POUND".to_string()]);
    }

    #[test]
    fn moving_or_filtering_disarms() {
        let mut p = picker(vec![
            entry("A", "BULBASAUR", 5, false),
            entry("B", "IVYSAUR", 6, false),
        ]);
        p.confirm();
        assert!(p.armed);
        p.move_selection(1);
        assert!(!p.armed, "moving must disarm the pending confirm");
        p.confirm();
        p.type_char('a');
        assert!(!p.armed, "filtering must disarm too");
    }

    #[test]
    fn the_selection_stays_in_range_as_the_filter_narrows() {
        let mut p = picker(vec![
            entry("A", "BULBASAUR", 5, false),
            entry("B", "IVYSAUR", 6, false),
            entry("C", "VENUSAUR", 7, false),
        ]);
        p.move_selection(2);
        assert_eq!(p.selected, 2);
        for c in "ivy".chars() {
            p.type_char(c);
        }
        assert_eq!(p.visible_len(), 1);
        assert_eq!(p.selected, 0);
        assert_eq!(p.current().unwrap().species_name, "IVYSAUR");
    }
}

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
    /// The save's 16-bit trainer id — with the name, the natural key for
    /// "which account is this offer from" ([`super::card::OfferGame`]).
    pub player_id: u16,
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
            player_id: c.player_id,
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

/// The per-save identity heuristic for one Pokémon — the key the live-offer
/// watch uses to find "the same Pokémon" again in a later read of the save.
///
/// **Gen 1 has no unique per-Pokémon id**, so this is built from the only
/// fields that never change over a Pokémon's life in one save — each claim
/// verified against `pret/pokered`, not quoted from memory:
///
/// * **DVs** — rolled once and copied verbatim at capture (`_AddPartyMon`,
///   `engine/pokemon/add_mon.asm`: "copy IVs from cur enemy mon"); nothing
///   in the engine rewrites them afterwards, and the Time Capsule work
///   proved they survive even a trade byte-identical.
/// * **OT id and OT name** — written from `wPlayerID`/`wPlayerName` at
///   capture (same file), never rewritten.
///
/// Everything else is deliberately excluded, each for a cited reason:
/// **species** changes on evolution (the exact case live tracking exists
/// for); **nickname** is NOT permanent in Gen 1, despite the folklore —
/// `RenameEvolvedMon` (`engine/pokemon/evos_moves.asm`) rewrites an
/// unnicknamed Pokémon's name field to the evolved species' name, and the
/// Lavender Town Name Rater (`scripts/NameRatersHouse.asm`) renames any
/// Pokémon whose OT matches the player — precisely the Pokémon a player can
/// offer (`NameRatersHouseCheckMonOTScript` refuses only traded ones);
/// **level, experience, moves and stats** change constantly.
///
/// **It is a heuristic, not a guarantee.** Two Pokémon caught by the same
/// trainer with identical DV spreads collide (1 in 65,536 per pair — but
/// wild DVs are not uniform, see `docs/shiny-hunt.md`, so it is reachable).
/// The fail-safe is the caller's, and it is *withdraw, never guess*: an
/// offer whose identity matches two Pokémon is withdrawn rather than risk
/// updating it from — or ultimately trading — the wrong one. A trade is
/// irreversible.
pub fn identity_for(e: &Offerable) -> String {
    format!(
        "{:04x}:{:x}{:x}{:x}{:x}:{}",
        e.ot_id, e.dvs.attack, e.dvs.defense, e.dvs.speed, e.dvs.special, e.ot_name
    )
}

/// True when exactly one entry in the collection carries this identity — the
/// precondition for tracking an offer against it.
pub fn identity_is_unique(entries: &[Offerable], key: &str) -> bool {
    entries.iter().filter(|e| identity_for(e) == key).count() == 1
}

/// What of a Pokémon's *changeable* state the offer reflects. When this
/// moves — a level gained, a move learned, an evolution, a rename, a newly
/// refused name — the posted offer no longer matches the save and must be
/// updated (or withdrawn, when the change is a refusal).
pub fn fingerprint_for(e: &Offerable) -> String {
    format!(
        "{}|{}|{}|{}",
        e.species_name,
        e.level,
        e.nickname,
        e.moves.join(",")
    )
}

/// One offer posted this session, tracked against the live save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchedOffer {
    /// The portal's offer id, from the POST reply.
    pub id: String,
    /// [`identity_for`] of the Pokémon at post time.
    pub identity: String,
    /// Which save the offer belongs to — the watch only acts while the SAME
    /// save is loaded. Loading a different game (or a different player's
    /// save) suspends tracking; it must never withdraw offers that belong to
    /// a save that is simply not loaded right now.
    pub game: String,
    pub player: String,
    pub player_id: u16,
    /// [`fingerprint_for`] as last posted.
    pub fingerprint: String,
    /// Consecutive maintenance failures; the caller gives up loudly after a
    /// few rather than hammering a portal that keeps refusing.
    pub failures: u8,
}

/// What the watch decided one offer needs. Pure data — the caller owns the
/// network.
#[derive(Debug, Clone)]
pub enum WatchAction {
    /// The offer no longer reflects anything offerable: withdraw it
    /// (`POST /api/offers/<id>/cancel`). The reason is user-facing.
    Withdraw { id: String, reason: String },
    /// The Pokémon is still there and still offerable but has changed:
    /// repost the offer's contents (`POST /api/offers/<id>/update`) from
    /// this entry.
    Update { id: String, entry: Offerable },
}

/// Compare the watched offers against a fresh read of the loaded save and
/// say what each needs. At most the caller performs one action per poll;
/// the rest keep coming up until acted on.
///
/// The rules, in order:
/// * a different save is loaded (game title or player differ) → **nothing**
///   — tracking is suspended, not failed;
/// * the identity matches no entry → withdraw ("left the save");
/// * the identity matches two or more → withdraw (ambiguous — fail safe,
///   see [`identity_for`]);
/// * the match now carries a refusal → withdraw, naming it;
/// * the fingerprint moved → update from the live entry.
pub fn plan(
    watch: &[WatchedOffer],
    col: &Collection,
    game_title: &str,
) -> Vec<WatchAction> {
    let mut actions = Vec::new();
    for w in watch {
        if w.game != game_title
            || w.player != col.player_name
            || w.player_id != col.player_id
        {
            continue;
        }
        let matches: Vec<&Offerable> = col
            .entries
            .iter()
            .filter(|e| identity_for(e) == w.identity)
            .collect();
        match matches.as_slice() {
            [] => actions.push(WatchAction::Withdraw {
                id: w.id.clone(),
                reason: "it left the save".into(),
            }),
            [one] => {
                if let Some(why) = &one.refusal {
                    actions.push(WatchAction::Withdraw {
                        id: w.id.clone(),
                        reason: format!("it can no longer be traded — {why}"),
                    });
                } else if fingerprint_for(one) != w.fingerprint {
                    actions.push(WatchAction::Update {
                        id: w.id.clone(),
                        entry: (*one).clone(),
                    });
                }
            }
            _ => actions.push(WatchAction::Withdraw {
                id: w.id.clone(),
                reason: "two identical Pokémon in the save — cannot tell which \
                         was offered"
                    .into(),
            }),
        }
    }
    actions
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

    fn watched(e: &Offerable) -> WatchedOffer {
        WatchedOffer {
            id: "81".into(),
            identity: identity_for(e),
            game: "Pokémon Blue".into(),
            player: "RED".into(),
            player_id: 1,
            fingerprint: fingerprint_for(e),
            failures: 0,
        }
    }

    fn collection(entries: Vec<Offerable>) -> Collection {
        Collection {
            player_name: "RED".into(),
            player_id: 1,
            entries,
            notes: Vec::new(),
        }
    }

    /// The identity key survives exactly the changes live tracking exists
    /// for — evolution (species), the Name Rater (nickname), levelling — and
    /// the fingerprint moves on each of them so the offer reposts.
    #[test]
    fn the_identity_survives_evolution_and_renames_and_the_fingerprint_does_not() {
        let before = entry("EVEE", "EEVEE", 24, false);
        let mut after = before.clone();
        after.species_name = "VAPOREON".into();
        after.nickname = "SOGGY".into(); // renamed at the Name Rater too
        after.level = 25;
        after.moves.push("WATER GUN".into());
        assert_eq!(identity_for(&before), identity_for(&after));
        assert_ne!(fingerprint_for(&before), fingerprint_for(&after));
    }

    #[test]
    fn a_changed_pokemon_is_updated_and_a_missing_one_withdrawn() {
        let e = entry("BULBA", "BULBASAUR", 14, false);
        let w = vec![watched(&e)];
        // Unchanged → nothing to do.
        assert!(plan(&w, &collection(vec![e.clone()]), "Pokémon Blue").is_empty());
        // Evolved → update, carrying the live entry.
        let mut evolved = e.clone();
        evolved.species_name = "IVYSAUR".into();
        evolved.level = 16;
        match &plan(&w, &collection(vec![evolved]), "Pokémon Blue")[..] {
            [WatchAction::Update { id, entry }] => {
                assert_eq!(id, "81");
                assert_eq!(entry.species_name, "IVYSAUR");
            }
            other => panic!("expected an update, got {other:?}"),
        }
        // Gone from the save → withdraw.
        match &plan(&w, &collection(vec![]), "Pokémon Blue")[..] {
            [WatchAction::Withdraw { id, reason }] => {
                assert_eq!(id, "81");
                assert!(reason.contains("left the save"), "{reason}");
            }
            other => panic!("expected a withdraw, got {other:?}"),
        }
    }

    /// The fail-safe: an identity matching two Pokémon withdraws rather than
    /// guessing which one was offered — a trade is irreversible. And a newly
    /// refused name (the Name Rater writing an `8`) withdraws with the
    /// reason, because the wire can no longer carry it.
    #[test]
    fn ambiguity_and_a_new_refusal_both_withdraw_never_guess() {
        let e = entry("BULBA", "BULBASAUR", 14, false);
        let w = vec![watched(&e)];
        let twin = e.clone(); // same OT, same DVs — indistinguishable
        match &plan(&w, &collection(vec![e.clone(), twin]), "Pokémon Blue")[..] {
            [WatchAction::Withdraw { reason, .. }] => {
                assert!(reason.contains("identical"), "{reason}");
            }
            other => panic!("expected a withdraw, got {other:?}"),
        }
        let mut refused = e.clone();
        refused.nickname = "BULBA8".into();
        refused.refusal = Some("the name carries the digit 8".into());
        match &plan(&w, &collection(vec![refused]), "Pokémon Blue")[..] {
            [WatchAction::Withdraw { reason, .. }] => {
                assert!(reason.contains("digit 8"), "{reason}");
            }
            other => panic!("expected a withdraw, got {other:?}"),
        }
    }

    /// Loading a different save (another game, or another player's save of
    /// the same game) suspends tracking — it must never withdraw offers that
    /// belong to a save that is simply not loaded right now.
    #[test]
    fn a_different_save_suspends_the_watch_instead_of_withdrawing() {
        let e = entry("BULBA", "BULBASAUR", 14, false);
        let w = vec![watched(&e)];
        // Same player name and id, different game title.
        assert!(plan(&w, &collection(vec![]), "Pokémon Red").is_empty());
        // Same game, different player.
        let mut other = collection(vec![]);
        other.player_id = 2;
        assert!(plan(&w, &other, "Pokémon Blue").is_empty());
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

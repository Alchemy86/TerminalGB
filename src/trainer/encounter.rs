//! "Would this wild Pokémon be shiny if traded to Gen 2?" — decided the moment
//! it appears, while a Ball is still unspent.
//!
//! # Why this can be answered at the encounter, and why it is worth answering
//!
//! Gen 1 has **no shiny Pokémon at all**. Nothing on this screen is shiny, and
//! nothing said here claims otherwise. What Gen 2 added was a *rule*, and the
//! rule reads bits that a Gen 1 Pokémon already carries: its DVs. So a Red/Blue
//! Pokémon has a settled answer to "what happens when this is traded forward
//! through the Time Capsule" long before Gold/Silver exists to ask it.
//!
//! Three facts, each from the disassemblies, are what make the answer *early*
//! and *actionable*:
//!
//! 1. **A wild Pokémon's DVs are rolled when the battle starts.**
//!    `LoadEnemyMonData` (`pokered/engine/battle/core.asm`) branches on
//!    `wIsInBattle`: a trainer's Pokémon gets the fixed pair
//!    `ATKDEFDV_TRAINER`/`SPDSPCDV_TRAINER` (`$98`/`$88`), a wild one gets two
//!    `BattleRandom` bytes. Either way they land in `wEnemyMonDVs` before the
//!    battle is drawn.
//! 2. **Capture copies those exact bytes.** `_AddPartyMon`
//!    (`pokered/engine/pokemon/add_mon.asm`) takes the `.copyEnemyMonData`
//!    branch for a Pokémon caught in battle and copies `wEnemyMonDVs` verbatim
//!    into the new party entry. The verdict shown at the encounter is therefore
//!    still true of the Pokémon in the Ball — that is the whole point. Nothing
//!    is re-rolled on capture.
//! 3. **A trainer's Pokémon is not a roll at all.** Because of the fixed pair in
//!    (1), *every* trainer Pokémon in the game has the same DVs — and `$98`/`$88`
//!    is not the shiny pattern, so the answer for a trainer battle is always
//!    "no", always for the same reason, and it is never actionable because the
//!    Pokémon cannot be caught (`ItemUseBall`: "Balls can't catch trainers'
//!    Pokémon"). Showing a verdict there would be noise, so [`applies`] refuses.
//!
//! The Gen 2 rule itself lives in [`super::derived::shiny_if_traded`], cited to
//! `CheckShininess` in `pokecrystal/engine/gfx/color.asm`.
//!
//! # Where this works, and where it cannot
//!
//! * **Wild encounters — yes.** Grass, water, fishing, caves: all of them reach
//!   `InitWildBattle`, which is what sets `wIsInBattle` to 1.
//! * **Static / scripted encounters — yes, the same way.** Snorlax, the
//!   legendary birds, Mewtwo, the Power Plant Voltorbs and the Pokémon Tower
//!   ghosts are all started by `InitOpponent` with `wCurOpponent` holding a
//!   *species* (below `OPP_ID_OFFSET`), which falls through to `InitWildBattle`
//!   (`pokered/engine/battle/core.asm`). They are ordinary wild battles with
//!   ordinary rolled DVs, so the verdict is shown and is correct.
//! * **Trainer battles — deliberately not.** See (3) above.
//! * **The old man's catching demonstration — deliberately not.** It is a wild
//!   battle (`wIsInBattle` = 1) with real rolled DVs, but the Weedle is never
//!   added to the player's party, so a "worth a Ball?" verdict is misleading.
//!   `wBattleType` distinguishes it.
//! * **Gift, starter and in-game-trade Pokémon — no, and there is nothing to
//!   fix.** They never enter a battle, so there is no encounter to warn about.
//!   `_AddPartyMon` rolls their DVs with `Random` at the instant they are handed
//!   over (the `; Not wild.` branch), by which time they are already yours and
//!   there is no decision left to make. Their verdict is still shown — by the
//!   party panel's `★`, which reads the same DVs out of `wPartyMon*DVs` — it is
//!   just retrospective rather than a prompt.
//! * **Two wild Pokémon you are shown but cannot catch:** an unidentified ghost
//!   in Pokémon Tower (no Silph Scope) and the ghost Marowak on 6F are both
//!   refused by `ItemUseBall` even though they are wild battles. The verdict is
//!   still displayed for them, which is a known and small overclaim of
//!   *catchability*, never of shininess.

use super::derived::shiny_if_traded;
use super::gen1::{BattleKind, BattleType, Dvs};

/// Whether a shiny-if-traded verdict is worth showing for the battle the game
/// is currently in.
///
/// Takes the two raw bytes rather than decoded values so the whole decision —
/// including what an undefined flag value means — is one testable function.
pub fn applies(is_in_battle: u8, battle_type: u8) -> bool {
    BattleKind::from_flag(is_in_battle) == BattleKind::Wild
        && BattleType::from_flag(battle_type).is_catchable()
}

/// The latched verdict for one wild encounter.
///
/// Latched, not sampled: it is read once, when the battle's enemy structure
/// first looks valid, and then held unchanged until the battle ends. The DVs
/// cannot change under it (only Transform rewrites `wEnemyMonDVs`, and that
/// cannot have happened in the tenth of a second before the first sample), and
/// holding it is what makes the banner *persist for the encounter* rather than
/// flashing once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WildEncounter {
    /// Internal index number, as stored in `wEnemyMonSpecies`.
    pub species: u8,
    /// National Pokédex number, resolved through `PokedexOrder`.
    pub dex: u8,
    pub name: String,
    pub level: u8,
    pub dvs: Dvs,
    /// Which flavour of wild battle this is — a Safari encounter is worth
    /// naming, because the Ball being weighed is a Safari Ball.
    pub kind: BattleType,
}

impl WildEncounter {
    /// The claim, and it is precisely this claim: **not** "this is shiny".
    pub fn would_be_shiny(&self) -> bool {
        shiny_if_traded(self.dvs)
    }

    /// `hp/atk/def/spd/spc`, the order the detail view already uses.
    pub fn dv_string(&self) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            self.dvs.hp(),
            self.dvs.attack,
            self.dvs.defense,
            self.dvs.speed,
            self.dvs.special
        )
    }

    /// How many of the three DVs the Gen 2 rule pins to exactly 10 are right.
    /// A 2/3 is a genuine near miss and worth seeing; the number is what makes
    /// the verdict explicable rather than an oracle.
    pub fn shiny_matches(&self) -> u8 {
        [self.dvs.defense, self.dvs.speed, self.dvs.special]
            .iter()
            .filter(|&&v| v == 10)
            .count() as u8
    }

    /// Whether the attack DV satisfies its half of the rule (bit 1 set).
    pub fn attack_bit_ok(&self) -> bool {
        self.dvs.attack & 0b0010 != 0
    }
}

/// Does this look like a populated `wEnemyMon`, or like whatever was in WRAM
/// before `LoadEnemyMonData` ran?
///
/// `InitWildBattle` sets `wIsInBattle` to 1 **and then** calls
/// `LoadEnemyMonData`, so there is a window — a few frames at most, but the
/// plugin samples at ~10 Hz — in which the flag says "wild battle" and the
/// structure is still the previous battle's. Latching in that window would show
/// the wrong Pokémon's DVs, which is exactly the "plausible but wrong" failure
/// this plugin exists to avoid.
pub fn looks_populated(dex: u8, level: u8, hp: u16, max_hp: u16) -> bool {
    (1..=151).contains(&dex) && (1..=100).contains(&level) && max_hp > 0 && hp <= max_hp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verdict_is_offered_only_for_a_catchable_wild_battle() {
        // The one case that earns a verdict, plus the Safari Zone.
        assert!(applies(1, 0), "an ordinary wild encounter");
        assert!(applies(1, 2), "a Safari Zone encounter");
        // The old man's demonstration is a wild battle whose Pokémon is never
        // the player's.
        assert!(!applies(1, 1), "the catching tutorial");
        // A trainer's Pokémon cannot be caught and its DVs are the fixed pair.
        assert!(!applies(2, 0), "a trainer battle");
        // Not in a battle, and the blacked-out sentinel.
        assert!(!applies(0, 0));
        assert!(!applies(0xFF, 0));
    }

    #[test]
    fn no_undefined_flag_combination_produces_a_verdict() {
        // Exhaustive: the only (wIsInBattle, wBattleType) pairs that may show a
        // verdict are (1,0) and (1,2). Anything else — including every value
        // the game never writes — must stay silent.
        for battle in 0u8..=255 {
            for kind in 0u8..=255 {
                let want = battle == 1 && (kind == 0 || kind == 2);
                assert_eq!(applies(battle, kind), want, "({battle}, {kind})");
            }
        }
    }

    fn enc(hi: u8, lo: u8) -> WildEncounter {
        WildEncounter {
            species: 0x24,
            dex: 16,
            name: "PIDGEY".into(),
            level: 3,
            dvs: Dvs::from_bytes(hi, lo),
            kind: BattleType::Normal,
        }
    }

    #[test]
    fn the_verdict_is_the_gen2_rule_and_nothing_looser() {
        // ATKDEFDV_SHINY $EA / SPDSPCDV_SHINY $AA.
        let shiny = enc(0xEA, 0xAA);
        assert!(shiny.would_be_shiny());
        // The HP DV is derived from the low bit of the other four, and the
        // shiny pattern makes three of them even — so a shiny Gen 1 Pokémon
        // has an HP DV of 0 unless its attack DV is odd. Worth pinning: it
        // looks like a bug the first time anyone reads it.
        assert_eq!(shiny.dv_string(), "0/14/10/10/10");
        assert_eq!(shiny.shiny_matches(), 3);
        assert!(shiny.attack_bit_ok());

        // Perfect DVs are emphatically not shiny — the mistake a "high DVs"
        // heuristic would make, and the one that costs a Ball.
        let perfect = enc(0xFF, 0xFF);
        assert!(!perfect.would_be_shiny());
        assert_eq!(perfect.dv_string(), "15/15/15/15/15");
        assert_eq!(perfect.shiny_matches(), 0);

        // A genuine near miss: two of the three tens.
        let near = enc(0xEA, 0xAB);
        assert!(!near.would_be_shiny());
        assert_eq!(near.shiny_matches(), 2);
    }

    #[test]
    fn the_fixed_trainer_dv_pair_would_never_be_shiny_anyway() {
        // ATKDEFDV_TRAINER $98 / SPDSPCDV_TRAINER $88 — the same for every
        // trainer Pokémon in the game, and not the shiny pattern. So refusing
        // to show a trainer verdict hides nothing.
        let t = enc(0x98, 0x88);
        assert!(!t.would_be_shiny());
        assert_eq!(t.dv_string(), "8/9/8/8/8");
    }

    #[test]
    fn a_half_initialised_enemy_structure_is_not_latched() {
        assert!(looks_populated(16, 3, 12, 12));
        assert!(!looks_populated(0, 3, 12, 12), "species not resolved yet");
        assert!(!looks_populated(200, 3, 12, 12), "not a Pokédex number");
        assert!(!looks_populated(16, 0, 12, 12), "level 0");
        assert!(!looks_populated(16, 101, 12, 12), "level over 100");
        assert!(!looks_populated(16, 3, 12, 0), "stats not calculated yet");
        assert!(!looks_populated(16, 3, 13, 12), "HP above maximum");
    }
}

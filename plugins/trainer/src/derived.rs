//! Values the game computes with but never shows you.
//!
//! This is the layer that makes a tracker worth having rather than being a
//! prettier copy of the party menu: every number here is *already* in the
//! cartridge's memory, and the game deliberately keeps all of it hidden.
//!
//! Two of them are not Gen 1 concepts at all — they are Gen 2 rules applied to
//! Gen 1 data. That is legitimate, and it is the interesting part: a Gen 1
//! Pokémon's DVs travel forward unchanged through the Time Capsule, so Gen 2
//! decides shininess, gender and Hidden Power from bits that already exist in
//! the save sitting in front of you. **The Gen 1 game itself has no notion of
//! any of them**, so this says "would be", never "is".
//!
//! Every rule is cited to the disassembly it comes from. The Gen 2 rules are
//! [`pret/pokecrystal`](https://github.com/pret/pokecrystal); the Gen 1 storage
//! is [`pret/pokered`](https://github.com/pret/pokered).
//!
//! # What Gen 1 genuinely does not have
//!
//! * **The Poké Ball a Pokémon was caught in.** There is no ball field anywhere
//!   in the Gen 1 `party_struct` — the concept arrives in Gen 2, which stores it
//!   in the caught-data word. It cannot be shown for Red/Blue at any price, and
//!   guessing it would be fabrication. A future Gen 2 mode would gain it.
//! * **Abilities, natures, held items, EVs as Gen 3 knows them.** None exist.
//!   Gen 1's "stat experience" below is the ancestor of EVs and works quite
//!   differently.
//! * **Gender.** Gen 2 derives it from the attack DV compared against the
//!   *species' gender ratio*, and that ratio is Gen 2 data which is not present
//!   anywhere in a Gen 1 cartridge. Deriving it would mean embedding a 151-entry
//!   table copied out of pokecrystal, which is a bigger and more debatable step
//!   than the rest of this file; it is named as a follow-up rather than
//!   half-built.

use super::gen1::Dvs;

/// Whether this Pokémon would be shiny in Gen 2, if traded forward.
///
/// `CheckShininess` — [`pret/pokecrystal`](https://github.com/pret/pokecrystal)
/// `engine/gfx/color.asm`, which defines `SHINY_ATK_MASK EQU %0010` and
/// `SHINY_DEF_DV EQU 10`, then requires speed and special to equal 10 as well
/// (the canonical shiny DV pair is `ATKDEFDV_SHINY EQU $EA` /
/// `SPDSPCDV_SHINY EQU $AA` in `constants/battle_constants.asm`).
///
/// So: defense, speed and special DVs must all be exactly 10, and the attack DV
/// must have bit 1 set — one of 2, 3, 6, 7, 10, 11, 14, 15. That is 8 attack
/// values out of 16 with three fixed nybbles, i.e. 8/65536 = **1 in 8192**, the
/// familiar odds.
pub fn shiny_if_traded(dvs: Dvs) -> bool {
    const SHINY_ATK_MASK: u8 = 0b0010;
    const SHINY_DV: u8 = 10;
    dvs.attack & SHINY_ATK_MASK != 0
        && dvs.defense == SHINY_DV
        && dvs.speed == SHINY_DV
        && dvs.special == SHINY_DV
}

/// Hidden Power as Gen 2 would compute it for this Pokémon.
///
/// `HiddenPowerDamage` — `pokecrystal/engine/battle/hidden_power.asm`. The type
/// comes from the low two bits of the attack and defense DVs; the power from
/// the top bit of all four DVs plus the low two bits of special.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HiddenPower {
    /// Type name. Never Normal — the mapping deliberately skips it.
    pub type_name: &'static str,
    /// Base power, 31..=70.
    pub power: u8,
}

/// Gen 2's type ids, needed because Hidden Power's type is a *Gen 2* type and
/// Gen 2 added Steel and Dark, which do not exist in the cartridge's own type
/// table. `pokecrystal/constants/type_constants.asm`.
fn gen2_type_name(id: u8) -> &'static str {
    match id {
        0 => "NORMAL",
        1 => "FIGHTING",
        2 => "FLYING",
        3 => "POISON",
        4 => "GROUND",
        5 => "ROCK",
        6 => "BIRD",
        7 => "BUG",
        8 => "GHOST",
        9 => "STEEL",
        0x14 => "FIRE",
        0x15 => "WATER",
        0x16 => "GRASS",
        0x17 => "ELECTRIC",
        0x18 => "PSYCHIC",
        0x19 => "ICE",
        0x1A => "DRAGON",
        0x1B => "DARK",
        _ => "?",
    }
}

pub fn hidden_power(dvs: Dvs) -> HiddenPower {
    // Type: (Atk & 3) << 2 | (Def & 3), then walk past the ids Hidden Power
    // cannot produce — Normal, the unused Bird type, and the unused block
    // between Steel and Fire.
    const BIRD: u8 = 6;
    const UNUSED_TYPES: u8 = 10;
    const UNUSED_TYPES_LEN: u8 = 10; // UNUSED_TYPES_END - UNUSED_TYPES

    let mut id = ((dvs.attack & 3) << 2) | (dvs.defense & 3);
    id += 1; // skip Normal
    if id >= BIRD {
        id += 1;
    }
    if id >= UNUSED_TYPES {
        id += UNUSED_TYPES_LEN;
    }

    // Power: the top bit of each DV, most significant from attack, then the
    // low two bits of special as a tie-break.
    let top = |v: u8| (v >> 3) & 1;
    let v = (top(dvs.attack) << 3)
        | (top(dvs.defense) << 2)
        | (top(dvs.speed) << 1)
        | top(dvs.special);
    let power = (5 * v + (dvs.special & 3)) / 2 + 31;

    HiddenPower {
        type_name: gen2_type_name(id),
        power,
    }
}

/// Gen 1 "stat experience" — the ancestor of EVs, and completely invisible in
/// game.
///
/// Every Pokémon that faints in a battle adds its *base stats* to the winner's
/// counters, which cap at 65535 each. The contribution to a stat is
/// `floor(ceil(sqrt(exp)) / 4)` at level 100, scaled by level below that
/// (`CalcStat`, `pokered/home/move_mon.asm`). Stored big-endian at
/// `wPartyMon1HPExp` (`$D17C`) and the four following pairs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StatExp {
    pub hp: u16,
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub special: u16,
}

impl StatExp {
    /// The number of stat points this experience is currently worth at `level`,
    /// which is what a player actually wants to know. `CalcStat` computes
    /// `ceil(sqrt(exp))`, halves it twice, and scales by level — so the bonus at
    /// level `l` is `floor(ceil(sqrt(exp)) / 4) * l / 100`.
    pub fn points(exp: u16, level: u8) -> u16 {
        let root = (exp as f64).sqrt().ceil() as u32;
        ((root / 4) * level as u32 / 100) as u16
    }

    /// True once every counter has reached the 65535 cap — "fully trained".
    pub fn is_maxed(&self) -> bool {
        [self.hp, self.attack, self.defense, self.speed, self.special]
            .iter()
            .all(|&v| v == u16::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_canonical_shiny_dv_pair_is_shiny() {
        // ATKDEFDV_SHINY $EA / SPDSPCDV_SHINY $AA.
        assert!(shiny_if_traded(Dvs::from_bytes(0xEA, 0xAA)));
    }

    #[test]
    fn shininess_needs_all_three_fixed_dvs_and_the_attack_bit() {
        // Defense, speed and special must all be exactly 10.
        assert!(!shiny_if_traded(Dvs::from_bytes(0xEB, 0xAA)), "def 11");
        assert!(!shiny_if_traded(Dvs::from_bytes(0xEA, 0xBA)), "speed 11");
        assert!(!shiny_if_traded(Dvs::from_bytes(0xEA, 0xAB)), "special 11");
        // Attack must have bit 1 set: 2,3,6,7,10,11,14,15 shiny; the rest not.
        for atk in 0u8..16 {
            let dv = Dvs {
                attack: atk,
                defense: 10,
                speed: 10,
                special: 10,
            };
            assert_eq!(shiny_if_traded(dv), atk & 0b0010 != 0, "attack DV {atk}");
        }
    }

    #[test]
    fn perfect_dvs_are_not_shiny() {
        // 15/15/15/15 is the best possible Pokémon and is emphatically not
        // shiny — a detail that surprises people, and a good regression guard
        // against "all DVs high" being mistaken for the rule.
        assert!(!shiny_if_traded(Dvs::from_bytes(0xFF, 0xFF)));
    }

    #[test]
    fn hidden_power_covers_the_sixteen_types_and_never_normal() {
        let mut seen = std::collections::BTreeSet::new();
        for atk in 0u8..4 {
            for def in 0u8..4 {
                let hp = hidden_power(Dvs {
                    attack: atk,
                    defense: def,
                    speed: 0,
                    special: 0,
                });
                assert_ne!(hp.type_name, "NORMAL");
                assert_ne!(hp.type_name, "?", "atk {atk} def {def} mapped nowhere");
                seen.insert(hp.type_name);
            }
        }
        assert_eq!(
            seen.len(),
            16,
            "all sixteen Hidden Power types are reachable"
        );
        assert!(seen.contains("DARK") && seen.contains("STEEL"));
    }

    #[test]
    fn hidden_power_endpoints_match_the_formula() {
        // All DVs zero: type index 1 (Fighting), power 5*0 + 0 / 2 + 31 = 31.
        let lo = hidden_power(Dvs::default());
        assert_eq!((lo.type_name, lo.power), ("FIGHTING", 31));
        // All DVs 15: type index 15 -> Dark; v = 0b1111 = 15, special & 3 = 3,
        // so (75 + 3) / 2 + 31 = 70, the maximum.
        let hi = hidden_power(Dvs::from_bytes(0xFF, 0xFF));
        assert_eq!((hi.type_name, hi.power), ("DARK", 70));
    }

    #[test]
    fn stat_experience_converts_to_points_at_level() {
        // 65535 fully trained: ceil(sqrt) = 256, /4 = 64, at level 100 that is
        // the full 63-64 point bonus Gen 1 tops out at.
        assert_eq!(StatExp::points(65535, 100), 64);
        assert_eq!(StatExp::points(65535, 50), 32);
        // Nothing earned is worth nothing.
        assert_eq!(StatExp::points(0, 100), 0);
        let maxed = StatExp {
            hp: u16::MAX,
            attack: u16::MAX,
            defense: u16::MAX,
            speed: u16::MAX,
            special: u16::MAX,
        };
        assert!(maxed.is_maxed());
        assert!(!StatExp::default().is_maxed());
    }
}

/// The six in-battle stat stages, as the panel reports them.
///
/// # What Gen 1 stores, and why the panel shows the *stage* rather than a
/// "before" value
///
/// `wPlayerMonStatMods` / `wEnemyMonStatMods` are six bytes — attack, defense,
/// speed, special, then accuracy and evasion (`wPlayerMonAccuracyMod`,
/// `wPlayerMonEvasionMod` follow the four in `ram/wram.asm`). Each holds the
/// stage **biased by 7**, so `7` is neutral and the range `1..=13` is `-6..=+6`
/// (`MIN_STAT_MOD = 1`, `MAX_STAT_MOD = 13`,
/// `constants/battle_constants.asm`).
///
/// The stat *values* in the battle struct are already modified: `wEnemyMonAttack`
/// is what the damage formula will use, which is why a Growl visibly moves the
/// number the panel prints. What Gen 1 does **not** give us cheaply is the
/// unmodified value — it lives in a separate `wEnemyMonUnmodifiedAttack` block,
/// and the multipliers are integer ratios applied with truncation
/// (`StatModifierRatios`, `data/battle/stat_modifiers.asm`:
/// 25/100, 28/100, 33/100, 40/100, 50/100, 66/100, 1/1, 15/10, 2/1, 25/10,
/// 3/1, 35/10, 4/1) so the original cannot be recovered by dividing back out.
///
/// So the panel shows the **stage**, which is exact, unambiguous, and the number
/// a player actually reasons with ("that drop has landed twice"). It is also the
/// only way to report accuracy and evasion at all — neither has a value column,
/// because neither is a stat the game stores a number for.
///
/// Two Gen 1 details worth not tripping over: badge boosts are applied to the
/// *player's* battle stats as well (`BadgeStatBoosts`,
/// `engine/battle/core.asm`), so the player's printed value already includes
/// them and the stage is still the honest delta; and stages do not survive a
/// switch or the end of a battle, which is why this is only ever read from the
/// battle structures and never from the party copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StatMods {
    /// Raw bytes as the game stores them, biased by [`STAT_MOD_NEUTRAL`].
    pub raw: [u8; 6],
}

/// The value a byte in `wPlayerMonStatMods` holds when the stat is unmodified.
pub const STAT_MOD_NEUTRAL: u8 = 7;

/// Labels for the six entries, in the order the game stores them.
pub const STAT_MOD_NAMES: [&str; 6] = ["ATK", "DEF", "SPD", "SPC", "ACC", "EVA"];

/// The narrowest and widest a stored stat-mod byte can legally be:
/// `MIN_STAT_MOD` / `MAX_STAT_MOD`, `constants/battle_constants.asm`.
pub const STAT_MOD_MIN: u8 = 1;
pub const STAT_MOD_MAX: u8 = 13;

impl StatMods {
    pub fn from_bytes(raw: [u8; 6]) -> StatMods {
        StatMods { raw }
    }

    /// Whether this block is one the battle engine has actually initialised.
    ///
    /// **This is not paranoia, it is a bug that was observed.** The block is
    /// zeroed WRAM until `InitBattleVariables` writes the neutral 7s, and for
    /// those first frames of a battle every byte reads `0` — which, taken at
    /// face value, is a stage of **-6 on every stat**, i.e. the panel
    /// announcing a catastrophic debuff that has not happened. Anything outside
    /// `MIN_STAT_MOD..=MAX_STAT_MOD` therefore means "not yet", and the caller
    /// shows nothing rather than a number it cannot stand behind.
    pub fn is_valid(self) -> bool {
        self.raw
            .iter()
            .all(|&v| (STAT_MOD_MIN..=STAT_MOD_MAX).contains(&v))
    }

    /// The stage for one entry, `-6..=6`. Out-of-range bytes (a structure read
    /// mid-update) clamp rather than wrapping into nonsense.
    pub fn stage(self, i: usize) -> i8 {
        let v = self.raw.get(i).copied().unwrap_or(STAT_MOD_NEUTRAL);
        (v as i16 - STAT_MOD_NEUTRAL as i16).clamp(-6, 6) as i8
    }

    /// Every entry that is not neutral, as `(name, stage)`.
    pub fn changed(self) -> Vec<(&'static str, i8)> {
        if !self.is_valid() {
            return Vec::new();
        }
        (0..6)
            .filter(|&i| self.stage(i) != 0)
            .map(|i| (STAT_MOD_NAMES[i], self.stage(i)))
            .collect()
    }

    pub fn any(self) -> bool {
        self.is_valid() && (0..6).any(|i| self.stage(i) != 0)
    }
}

/// The multiplier Gen 1 applies at a given stage, as the numerator and
/// denominator it stores.
///
/// `StatModifierRatios`, `data/battle/stat_modifiers.asm`. Present so the
/// non-linearity is stated in code rather than in a comment — a reader who
/// assumes "-1 is half" is wrong, and so is one who assumes the steps are even.
pub fn stat_stage_ratio(stage: i8) -> (u16, u16) {
    const RATIOS: [(u16, u16); 13] = [
        (25, 100),
        (28, 100),
        (33, 100),
        (40, 100),
        (50, 100),
        (66, 100),
        (1, 1),
        (15, 10),
        (2, 1),
        (25, 10),
        (3, 1),
        (35, 10),
        (4, 1),
    ];
    RATIOS[(stage.clamp(-6, 6) + 6) as usize]
}

#[cfg(test)]
mod stat_mod_tests {
    use super::*;

    #[test]
    fn seven_is_neutral_and_the_range_is_minus_six_to_six() {
        let m = StatMods::from_bytes([7, 6, 8, 7, 1, 13]);
        assert_eq!(m.stage(0), 0);
        assert_eq!(m.stage(1), -1);
        assert_eq!(m.stage(2), 1);
        assert_eq!(m.stage(4), -6);
        assert_eq!(m.stage(5), 6);
        assert!(m.any());
        assert_eq!(
            m.changed(),
            vec![("DEF", -1), ("SPD", 1), ("ACC", -6), ("EVA", 6)]
        );
        // All neutral is genuinely nothing to report.
        assert!(!StatMods::from_bytes([7; 6]).any());
        assert!(StatMods::from_bytes([7; 6]).changed().is_empty());
        // A byte read mid-update cannot produce a stage outside the range.
        assert_eq!(StatMods::from_bytes([0, 255, 7, 7, 7, 7]).stage(0), -6);
        assert_eq!(StatMods::from_bytes([0, 255, 7, 7, 7, 7]).stage(1), 6);
    }

    /// The block is zeroed WRAM until the battle engine initialises it, and a
    /// zero is "-6 on everything". Observed live: a run that sampled on the
    /// first frames of a trainer battle printed `YOU ATK-6 DEF-6 SPD-6 SPC-6`
    /// for a Pokémon nothing had touched.
    #[test]
    fn an_uninitialised_block_reports_nothing_rather_than_minus_six() {
        let zeroed = StatMods::from_bytes([0; 6]);
        assert!(!zeroed.is_valid());
        assert!(!zeroed.any());
        assert!(zeroed.changed().is_empty());
        // One byte out of range poisons the whole block: a half-written
        // structure is not a set of stages, it is a structure being written.
        assert!(!StatMods::from_bytes([7, 7, 7, 7, 7, 14]).is_valid());
        assert!(StatMods::from_bytes([1, 13, 7, 7, 7, 7]).is_valid());
    }

    /// The maintainer's own observation is the test: one Growl took an attack of
    /// 10 down to 6. That is `-1`, and `-1` is **two thirds**, not a half —
    /// which is exactly the assumption this table exists to stop.
    #[test]
    fn one_growl_takes_ten_to_six() {
        let (n, d) = stat_stage_ratio(-1);
        assert_eq!((n, d), (66, 100));
        assert_eq!(10u32 * n as u32 / d as u32, 6);
        assert_eq!(stat_stage_ratio(0), (1, 1));
        assert_eq!(stat_stage_ratio(1), (15, 10), "+1 is 1.5x, not 2x");
        assert_eq!(stat_stage_ratio(6), (4, 1));
        assert_eq!(stat_stage_ratio(-6), (25, 100));
        // Up and down are NOT reciprocals: -1 is 66/100 while +1 is 15/10, so
        // 0.66 against 1.5 — a debuff and a buff of the same magnitude do not
        // cancel to 1. That asymmetry is why the panel reports the stage rather
        // than trying to reconstruct the unmodified value.
        let (dn, dd) = stat_stage_ratio(-1);
        let (un, ud) = stat_stage_ratio(1);
        assert_ne!(dn as u32 * un as u32, dd as u32 * ud as u32);
    }
}

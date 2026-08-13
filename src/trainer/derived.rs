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

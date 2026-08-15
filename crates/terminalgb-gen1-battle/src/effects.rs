//! Move effect ids, and the classification the engine branches on.
//!
//! `constants/move_effect_constants.asm` — every id below is that file's own
//! constant, in its own order, and the numbering is load-bearing: three of the
//! cartridge's checks are *range* comparisons over these ids rather than table
//! lookups. `MoveHitTest`'s Mist check is `cp ATTACK_DOWN1_EFFECT ... cp
//! HAZE_EFFECT + 1 ... cp ATTACK_DOWN2_EFFECT ... cp REFLECT_EFFECT + 1`, and
//! reordering the constants would silently change which moves Mist blocks.
//!
//! What is *modelled* is a deliberate subset, and [`Support`] is how the engine
//! says which. An effect it does not implement is executed as its damage and
//! nothing else, and the turn log records that — a battle that silently dropped
//! Disable would be a battle nobody could trust. See
//! [`crate::sim::TurnLog::unmodelled`].

/// Effect ids, from `constants/move_effect_constants.asm`.
pub mod id {
    pub const NO_ADDITIONAL_EFFECT: u8 = 0x00;
    pub const POISON_SIDE_EFFECT1: u8 = 0x02;
    pub const DRAIN_HP: u8 = 0x03;
    pub const BURN_SIDE_EFFECT1: u8 = 0x04;
    pub const FREEZE_SIDE_EFFECT1: u8 = 0x05;
    pub const PARALYZE_SIDE_EFFECT1: u8 = 0x06;
    pub const EXPLODE: u8 = 0x07;
    pub const DREAM_EATER: u8 = 0x08;
    pub const MIRROR_MOVE: u8 = 0x09;
    pub const ATTACK_UP1: u8 = 0x0A;
    pub const DEFENSE_UP1: u8 = 0x0B;
    pub const SPEED_UP1: u8 = 0x0C;
    pub const SPECIAL_UP1: u8 = 0x0D;
    pub const ACCURACY_UP1: u8 = 0x0E;
    pub const EVASION_UP1: u8 = 0x0F;
    pub const PAY_DAY: u8 = 0x10;
    pub const SWIFT: u8 = 0x11;
    pub const ATTACK_DOWN1: u8 = 0x12;
    pub const DEFENSE_DOWN1: u8 = 0x13;
    pub const SPEED_DOWN1: u8 = 0x14;
    pub const SPECIAL_DOWN1: u8 = 0x15;
    pub const ACCURACY_DOWN1: u8 = 0x16;
    pub const EVASION_DOWN1: u8 = 0x17;
    pub const CONVERSION: u8 = 0x18;
    pub const HAZE: u8 = 0x19;
    pub const BIDE: u8 = 0x1A;
    pub const THRASH_PETAL_DANCE: u8 = 0x1B;
    pub const SWITCH_AND_TELEPORT: u8 = 0x1C;
    pub const TWO_TO_FIVE_ATTACKS: u8 = 0x1D;
    pub const EFFECT_1E: u8 = 0x1E;
    pub const FLINCH_SIDE_EFFECT1: u8 = 0x1F;
    pub const SLEEP: u8 = 0x20;
    pub const POISON_SIDE_EFFECT2: u8 = 0x21;
    pub const BURN_SIDE_EFFECT2: u8 = 0x22;
    pub const FREEZE_SIDE_EFFECT2: u8 = 0x23;
    pub const PARALYZE_SIDE_EFFECT2: u8 = 0x24;
    pub const FLINCH_SIDE_EFFECT2: u8 = 0x25;
    pub const OHKO: u8 = 0x26;
    pub const CHARGE: u8 = 0x27;
    pub const SUPER_FANG: u8 = 0x28;
    pub const SPECIAL_DAMAGE: u8 = 0x29;
    pub const TRAPPING: u8 = 0x2A;
    pub const FLY: u8 = 0x2B;
    pub const ATTACK_TWICE: u8 = 0x2C;
    pub const JUMP_KICK: u8 = 0x2D;
    pub const MIST: u8 = 0x2E;
    pub const FOCUS_ENERGY: u8 = 0x2F;
    pub const RECOIL: u8 = 0x30;
    pub const CONFUSION: u8 = 0x31;
    pub const ATTACK_UP2: u8 = 0x32;
    pub const DEFENSE_UP2: u8 = 0x33;
    pub const SPEED_UP2: u8 = 0x34;
    pub const SPECIAL_UP2: u8 = 0x35;
    pub const ACCURACY_UP2: u8 = 0x36;
    pub const EVASION_UP2: u8 = 0x37;
    pub const HEAL: u8 = 0x38;
    pub const TRANSFORM: u8 = 0x39;
    pub const ATTACK_DOWN2: u8 = 0x3A;
    pub const DEFENSE_DOWN2: u8 = 0x3B;
    pub const SPEED_DOWN2: u8 = 0x3C;
    pub const SPECIAL_DOWN2: u8 = 0x3D;
    pub const ACCURACY_DOWN2: u8 = 0x3E;
    pub const EVASION_DOWN2: u8 = 0x3F;
    pub const LIGHT_SCREEN: u8 = 0x40;
    pub const REFLECT: u8 = 0x41;
    pub const POISON: u8 = 0x42;
    pub const PARALYZE: u8 = 0x43;
    pub const ATTACK_DOWN_SIDE: u8 = 0x44;
    pub const DEFENSE_DOWN_SIDE: u8 = 0x45;
    pub const SPEED_DOWN_SIDE: u8 = 0x46;
    pub const SPECIAL_DOWN_SIDE: u8 = 0x47;
    pub const CONFUSION_SIDE: u8 = 0x4C;
    pub const TWINEEDLE: u8 = 0x4D;
    pub const SUBSTITUTE: u8 = 0x4F;
    pub const HYPER_BEAM: u8 = 0x50;
    pub const RAGE: u8 = 0x51;
    pub const MIMIC: u8 = 0x52;
    pub const METRONOME: u8 = 0x53;
    pub const LEECH_SEED: u8 = 0x54;
    pub const SPLASH: u8 = 0x55;
    pub const DISABLE: u8 = 0x56;
}

use crate::state::Stat;

/// How completely the engine models an effect.
///
/// This exists so a result can never quietly overstate itself. An agent that
/// wins a battle in which Disable was silently ignored has not beaten the game,
/// and [`crate::sim::TurnLog::unmodelled`] is what makes that visible in the
/// report rather than in nobody's head.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Support {
    /// Implemented, including the Gen 1 quirks the tests pin.
    Full,
    /// Implemented as far as the damage goes; the secondary behaviour is not.
    /// Named so the log can say which.
    DamageOnly(&'static str),
    /// Not modelled at all. The move still deals its damage if it has power.
    None(&'static str),
}

impl Support {
    pub fn is_full(self) -> bool {
        self == Support::Full
    }
    pub fn note(self) -> Option<&'static str> {
        match self {
            Support::Full => None,
            Support::DamageOnly(n) | Support::None(n) => Some(n),
        }
    }
}

/// A stat-change effect, decoded: which stat, how far, and whether it targets
/// the user or the opponent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct StatChange {
    pub stat: Stat,
    pub by: i8,
    /// True for the `*_UP*` effects, false for the `*_DOWN*` ones.
    pub on_self: bool,
    /// True for the `*_SIDE_EFFECT` variants, which ride on a damaging move and
    /// roll separately.
    pub is_side_effect: bool,
}

/// Decode an effect id into a stat change, if it is one.
///
/// The six `UP1` ids are contiguous from `$0A` and the six `DOWN1` from `$12`,
/// and the `UP2`/`DOWN2` blocks from `$32` and `$3A`; that contiguity is the
/// cartridge's, and the range checks in `MoveHitTest`'s Mist test depend on it.
pub fn stat_change(effect: u8) -> Option<StatChange> {
    let stats = [
        Stat::Attack,
        Stat::Defense,
        Stat::Speed,
        Stat::Special,
        Stat::Accuracy,
        Stat::Evasion,
    ];
    let block =
        |base: u8, by: i8, on_self: bool, is_side_effect: bool| -> Option<StatChange> {
            let i = effect.checked_sub(base)? as usize;
            stats.get(i).map(|&stat| StatChange {
                stat,
                by,
                on_self,
                is_side_effect,
            })
        };
    match effect {
        id::ATTACK_UP1..=id::EVASION_UP1 => block(id::ATTACK_UP1, 1, true, false),
        id::ATTACK_DOWN1..=id::EVASION_DOWN1 => block(id::ATTACK_DOWN1, -1, false, false),
        id::ATTACK_UP2..=id::EVASION_UP2 => block(id::ATTACK_UP2, 2, true, false),
        id::ATTACK_DOWN2..=id::EVASION_DOWN2 => block(id::ATTACK_DOWN2, -2, false, false),
        // The four side-effect variants are only Attack/Defense/Speed/Special;
        // there is no accuracy or evasion side effect.
        id::ATTACK_DOWN_SIDE..=id::SPECIAL_DOWN_SIDE => {
            block(id::ATTACK_DOWN_SIDE, -1, false, true)
        }
        _ => None,
    }
}

/// Which non-volatile status a move inflicts as its *primary* effect, and its
/// side-effect twins.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Inflicts {
    Sleep,
    Poison,
    Paralysis,
    Burn,
    Freeze,
    Confusion,
    Flinch,
}

/// Decode an effect id into the status it inflicts and the byte the roll is
/// compared against — `None` for the primary-effect versions, which always land
/// if the move hits.
pub fn inflicts(effect: u8) -> Option<(Inflicts, Option<u8>)> {
    use crate::mechanics::side_effect as se;
    Some(match effect {
        id::SLEEP => (Inflicts::Sleep, None),
        id::POISON => (Inflicts::Poison, None),
        id::PARALYZE => (Inflicts::Paralysis, None),
        id::CONFUSION => (Inflicts::Confusion, None),
        id::POISON_SIDE_EFFECT1 => (Inflicts::Poison, Some(se::POISON_WEAK)),
        id::POISON_SIDE_EFFECT2 => (Inflicts::Poison, Some(se::POISON_STRONG)),
        id::BURN_SIDE_EFFECT1 => (Inflicts::Burn, Some(se::WEAK)),
        id::BURN_SIDE_EFFECT2 => (Inflicts::Burn, Some(se::STRONG)),
        id::FREEZE_SIDE_EFFECT1 => (Inflicts::Freeze, Some(se::WEAK)),
        id::FREEZE_SIDE_EFFECT2 => (Inflicts::Freeze, Some(se::STRONG)),
        id::PARALYZE_SIDE_EFFECT1 => (Inflicts::Paralysis, Some(se::WEAK)),
        id::PARALYZE_SIDE_EFFECT2 => (Inflicts::Paralysis, Some(se::STRONG)),
        id::FLINCH_SIDE_EFFECT1 => (Inflicts::Flinch, Some(se::WEAK)),
        id::FLINCH_SIDE_EFFECT2 => (Inflicts::Flinch, Some(se::STRONG)),
        id::CONFUSION_SIDE => (Inflicts::Confusion, Some(se::CONFUSION)),
        id::TWINEEDLE => (Inflicts::Poison, Some(se::POISON_WEAK)),
        _ => return None,
    })
}

/// How well the engine models each effect. Everything not named here is
/// [`Support::None`].
pub fn support(effect: u8) -> Support {
    match effect {
        id::NO_ADDITIONAL_EFFECT
        | id::SWIFT
        | id::EXPLODE
        | id::RECOIL
        | id::JUMP_KICK
        | id::DRAIN_HP
        | id::DREAM_EATER
        | id::HYPER_BEAM
        | id::TWO_TO_FIVE_ATTACKS
        | id::ATTACK_TWICE
        | id::TWINEEDLE
        | id::TRAPPING
        | id::OHKO
        | id::SUPER_FANG
        | id::SPECIAL_DAMAGE
        | id::HAZE
        | id::HEAL
        | id::LIGHT_SCREEN
        | id::REFLECT
        | id::MIST
        | id::FOCUS_ENERGY
        | id::LEECH_SEED
        | id::SUBSTITUTE
        | id::SPLASH
        | id::CHARGE
        | id::FLY
        | id::PAY_DAY
        | id::SLEEP
        | id::POISON
        | id::PARALYZE
        | id::CONFUSION
        | id::THRASH_PETAL_DANCE => Support::Full,

        e if stat_change(e).is_some() => Support::Full,
        e if inflicts(e).is_some() => Support::Full,

        id::BIDE => Support::DamageOnly("Bide's storing and release are not modelled"),
        id::DISABLE => Support::DamageOnly("Disable does not lock a move"),
        id::MIMIC => Support::DamageOnly("Mimic does not copy a move"),
        id::METRONOME => Support::DamageOnly("Metronome does not call another move"),
        id::MIRROR_MOVE => Support::DamageOnly("Mirror Move does not copy a move"),
        id::TRANSFORM => Support::DamageOnly("Transform does not copy the target"),
        id::CONVERSION => Support::DamageOnly("Conversion does not change types"),
        id::RAGE => Support::DamageOnly("Rage does not lock or build"),
        id::SWITCH_AND_TELEPORT => {
            Support::DamageOnly("Whirlwind/Teleport do not end a wild battle")
        }
        id::EFFECT_1E => Support::None("an unused effect id"),
        _ => Support::None("not modelled"),
    }
}

/// Effects that ignore the accuracy check because they never miss.
/// `MoveHitTest`: `cp SWIFT_EFFECT / ret z`.
pub fn never_misses(effect: u8) -> bool {
    effect == id::SWIFT
}

/// Effects that deal a fixed or computed amount rather than going through
/// `CalculateDamage` — `data/battle/set_damage_effects.asm`. They skip the
/// crit test, the stat setup, STAB, the type chart *and* the damage roll, and
/// jump straight to the accuracy check.
pub fn is_set_damage(effect: u8) -> bool {
    matches!(effect, id::SUPER_FANG | id::SPECIAL_DAMAGE)
}

/// Effects whose base power is allowed to be zero and still deal damage —
/// `CalculateDamage`'s `.skipbp` branch.
pub fn allows_zero_power(effect: u8) -> bool {
    matches!(effect, id::TWO_TO_FIVE_ATTACKS | id::EFFECT_1E)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stat_blocks_decode_in_the_cartridges_order() {
        assert_eq!(
            stat_change(id::ATTACK_UP1),
            Some(StatChange {
                stat: Stat::Attack,
                by: 1,
                on_self: true,
                is_side_effect: false
            })
        );
        assert_eq!(
            stat_change(id::EVASION_UP1),
            Some(StatChange {
                stat: Stat::Evasion,
                by: 1,
                on_self: true,
                is_side_effect: false
            })
        );
        assert_eq!(
            stat_change(id::SPECIAL_DOWN2),
            Some(StatChange {
                stat: Stat::Special,
                by: -2,
                on_self: false,
                is_side_effect: false
            })
        );
        assert_eq!(
            stat_change(id::SPEED_DOWN_SIDE),
            Some(StatChange {
                stat: Stat::Speed,
                by: -1,
                on_self: false,
                is_side_effect: true
            })
        );
    }

    /// There is no accuracy or evasion *side* effect — the block is only four
    /// long, and `$48` is a `const_skip`.
    #[test]
    fn the_side_effect_block_stops_at_special() {
        assert!(stat_change(0x48).is_none());
        assert!(stat_change(id::SPECIAL_DOWN_SIDE).is_some());
    }

    #[test]
    fn nothing_outside_the_four_blocks_decodes_as_a_stat_change() {
        for e in [
            id::SLEEP,
            id::HAZE,
            id::SUBSTITUTE,
            id::NO_ADDITIONAL_EFFECT,
        ] {
            assert!(stat_change(e).is_none(), "${e:02X}");
        }
    }

    /// A primary status effect always lands if the move hits; a side effect
    /// rolls.
    #[test]
    fn a_primary_status_has_no_roll_and_a_side_effect_does() {
        assert_eq!(inflicts(id::SLEEP), Some((Inflicts::Sleep, None)));
        assert_eq!(
            inflicts(id::PARALYZE_SIDE_EFFECT1),
            Some((Inflicts::Paralysis, Some(26)))
        );
        assert_eq!(
            inflicts(id::PARALYZE_SIDE_EFFECT2),
            Some((Inflicts::Paralysis, Some(77)))
        );
    }

    /// Effects the engine does not model must say so rather than be silently
    /// treated as ordinary damage.
    #[test]
    fn an_unmodelled_effect_carries_a_reason() {
        assert!(support(id::NO_ADDITIONAL_EFFECT).is_full());
        assert!(!support(id::BIDE).is_full());
        assert!(support(id::BIDE).note().is_some());
        assert!(support(id::METRONOME).note().unwrap().contains("Metronome"));
    }

    #[test]
    fn swift_is_the_only_effect_that_never_misses() {
        assert!(never_misses(id::SWIFT));
        assert!(!never_misses(id::NO_ADDITIONAL_EFFECT));
        assert!(!never_misses(id::OHKO), "OHKO misses in its own way");
    }

    #[test]
    fn the_set_damage_effects_are_the_two_the_table_names() {
        assert!(is_set_damage(id::SUPER_FANG));
        assert!(is_set_damage(id::SPECIAL_DAMAGE));
        assert!(!is_set_damage(id::RECOIL));
    }
}

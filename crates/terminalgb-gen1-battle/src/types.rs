//! Elemental types and the type chart.
//!
//! # What the cartridge does
//!
//! Type ids are not dense. `constants/type_constants.asm` splits them into a
//! **physical** block `$00-$08` and a **special** block `$14-$1A`, with twenty
//! unused ids in between (`UNUSED_TYPES` / `const_next 20`). That split is not
//! cosmetic: it is *the* rule that decides whether a move uses Attack/Defense or
//! Special, because `GetDamageVarsForPlayerAttack` compares the move's type
//! against `SPECIAL` and branches. Modelling types as a dense 0..17 enum and
//! carrying a separate "is special" flag would be a second source of truth that
//! could disagree with the cartridge; here the id *is* the flag.
//!
//! `BIRD` (`$06`) is a real id in the table that no released Pokémon or move
//! carries — it survives in the data and is reachable through Conversion and
//! through glitch moves, so it is modelled rather than elided.
//!
//! # The chart
//!
//! `data/types/type_matchups.asm` (`TypeEffects`) is a flat list of
//! `db attacker, defender, multiplier` triples terminated by `db -1`. The
//! multiplier is a percentage byte: `SUPER_EFFECTIVE` = 20, `NOT_VERY_EFFECTIVE`
//! = 5, `NO_EFFECT` = 0, and anything absent from the table is 10 (×1).
//! Everything not listed is neutral, which is why the file is short.
//!
//! **The table is not embedded in this repository.** It is read out of the
//! player's own cartridge by [`crate::data::load_from_rom`]; see the licence
//! boundary in the repository's `CLAUDE.md`. [`TypeChart`] is the decoded form,
//! and `TypeChart::neutral()` exists so tests can run with no ROM at all.

/// A Generation 1 elemental type, stored as the cartridge's own id byte.
///
/// Constructed from a raw byte so a value read out of ROM or WRAM round-trips
/// exactly, including ids the released game never uses.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Type(pub u8);

impl Type {
    pub const NORMAL: Type = Type(0x00);
    pub const FIGHTING: Type = Type(0x01);
    pub const FLYING: Type = Type(0x02);
    pub const POISON: Type = Type(0x03);
    pub const GROUND: Type = Type(0x04);
    pub const ROCK: Type = Type(0x05);
    /// `BIRD` — in the data, on nothing the released game ships.
    pub const BIRD: Type = Type(0x06);
    pub const BUG: Type = Type(0x07);
    pub const GHOST: Type = Type(0x08);
    pub const FIRE: Type = Type(0x14);
    pub const WATER: Type = Type(0x15);
    pub const GRASS: Type = Type(0x16);
    pub const ELECTRIC: Type = Type(0x17);
    pub const PSYCHIC: Type = Type(0x18);
    pub const ICE: Type = Type(0x19);
    pub const DRAGON: Type = Type(0x1A);

    /// The first special-block id (`SPECIAL` in `constants/type_constants.asm`).
    pub const SPECIAL_START: u8 = 0x14;

    /// Whether a move of this type is resolved with Special rather than
    /// Attack/Defense.
    ///
    /// `GetDamageVarsForPlayerAttack` (`engine/battle/core.asm`) does exactly
    /// this comparison — `cp SPECIAL; jr nc, .specialAttack` — so in Gen 1 the
    /// physical/special split is a property of the *type*, never of the move.
    #[inline]
    pub const fn is_special(self) -> bool {
        self.0 >= Self::SPECIAL_START
    }

    /// The name the cartridge would print, for the sixteen ids it ships.
    ///
    /// This is a debugging convenience only. The authoritative names live in
    /// `data/types/names.asm` in the player's ROM and are read from there by
    /// [`crate::data::load_from_rom`] when a caller wants display text.
    pub const fn debug_name(self) -> &'static str {
        match self.0 {
            0x00 => "NORMAL",
            0x01 => "FIGHTING",
            0x02 => "FLYING",
            0x03 => "POISON",
            0x04 => "GROUND",
            0x05 => "ROCK",
            0x06 => "BIRD",
            0x07 => "BUG",
            0x08 => "GHOST",
            0x14 => "FIRE",
            0x15 => "WATER",
            0x16 => "GRASS",
            0x17 => "ELECTRIC",
            0x18 => "PSYCHIC",
            0x19 => "ICE",
            0x1A => "DRAGON",
            _ => "?",
        }
    }

    /// Every id the released game actually uses, in cartridge order.
    pub const RELEASED: [Type; 15] = [
        Type::NORMAL,
        Type::FIGHTING,
        Type::FLYING,
        Type::POISON,
        Type::GROUND,
        Type::ROCK,
        Type::BUG,
        Type::GHOST,
        Type::FIRE,
        Type::WATER,
        Type::GRASS,
        Type::ELECTRIC,
        Type::PSYCHIC,
        Type::ICE,
        Type::DRAGON,
    ];
}

/// The `TypeEffects` multiplier byte: a percentage, ten meaning ×1.
pub const NEUTRAL: u8 = 10;

/// The decoded type chart.
///
/// Held as the cartridge holds it — a list of `(attacker, defender, percent)`
/// triples — rather than as a dense matrix, so that a chart read out of a ROM
/// with extra or reordered rows is represented faithfully and
/// [`TypeChart::multiplier`] reproduces the game's *first match wins* scan
/// exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TypeChart {
    entries: Vec<(u8, u8, u8)>,
}

impl TypeChart {
    /// A chart with no entries: everything is neutral. For tests, and for the
    /// honest answer when no cartridge has been supplied.
    pub fn neutral() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Build from decoded triples, in the cartridge's own order.
    pub fn from_entries(entries: Vec<(u8, u8, u8)>) -> Self {
        Self { entries }
    }

    pub fn entries(&self) -> &[(u8, u8, u8)] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The multiplier percentage for one attacking type against one defending
    /// type — `10` when the pair is absent from the table.
    ///
    /// `AdjustDamageForMoveType` (`engine/battle/core.asm`) walks `TypeEffects`
    /// from the top and takes the **first** row whose attacker and defender both
    /// match, so a duplicated pair in a hacked ROM resolves to whichever row
    /// comes first. That is reproduced here rather than normalised away.
    pub fn multiplier(&self, attacking: Type, defending: Type) -> u8 {
        for &(atk, def, pct) in &self.entries {
            if atk == attacking.0 && def == defending.0 {
                return pct;
            }
        }
        NEUTRAL
    }

    /// The combined multiplier against a defender's one or two types, as a
    /// numerator over 100 (so ×4 is `400`, ×0.25 is `25`).
    ///
    /// `AdjustDamageForMoveType` applies each matching row in turn as
    /// `damage = damage * pct / 10`, **truncating at each step**. That is why
    /// the two-type result is not simply the product of the two fractions, and
    /// why this returns the numerator rather than a float: the caller must apply
    /// the two multiplications separately to reproduce the truncation. Use
    /// [`TypeChart::apply`] for damage; use this for display and for an agent's
    /// evaluation function.
    pub fn effectiveness_percent(
        &self,
        attacking: Type,
        defender: (Type, Option<Type>),
    ) -> u16 {
        let mut pct = u16::from(self.multiplier(attacking, defender.0)) * 10;
        if let Some(second) = defender.1 {
            if second != defender.0 {
                pct = pct * u16::from(self.multiplier(attacking, second)) / 10;
            }
        }
        pct
    }

    /// Apply the chart to a damage figure exactly as the cartridge does: one
    /// `* pct / 10` per matching row, truncating in between.
    ///
    /// `AdjustDamageForMoveType`, `engine/battle/core.asm`. The dual-type case
    /// really is two sequential truncating multiplications — a Rock-type move
    /// against a Bug/Flying defender is `d*20/10` then `*20/10`, and the
    /// intermediate loses its remainder.
    pub fn apply(
        &self,
        damage: u16,
        attacking: Type,
        defender: (Type, Option<Type>),
    ) -> u16 {
        let step =
            |d: u16, pct: u8| -> u16 { (u32::from(d) * u32::from(pct) / 10) as u16 };
        let mut d = step(damage, self.multiplier(attacking, defender.0));
        if let Some(second) = defender.1 {
            if second != defender.0 {
                d = step(d, self.multiplier(attacking, second));
            }
        }
        d
    }

    /// Whether a defender is immune — used for move choice and for the "it
    /// doesn't affect" message path.
    pub fn is_immune(&self, attacking: Type, defender: (Type, Option<Type>)) -> bool {
        self.effectiveness_percent(attacking, defender) == 0
    }
}

/// Same-type attack bonus.
///
/// `AdjustDamageForMoveType` multiplies by 15 and divides by 10 *before* the
/// type chart is consulted, and it truncates. Applying STAB after the chart, or
/// as a float, is measurably different on small damage figures.
pub fn apply_stab(damage: u16) -> u16 {
    (u32::from(damage) * 15 / 10) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The physical/special split is a property of the type id, and the boundary
    /// is exactly `SPECIAL` = `$14`.
    #[test]
    fn the_special_split_is_the_type_id_and_nothing_else() {
        assert!(!Type::NORMAL.is_special());
        assert!(!Type::GHOST.is_special());
        assert!(Type::FIRE.is_special());
        assert!(Type::DRAGON.is_special());
        // The unused block between GHOST and FIRE is physical on the low side.
        assert!(!Type(0x09).is_special());
        assert!(Type(0x14).is_special());
    }

    fn chart() -> TypeChart {
        TypeChart::from_entries(vec![
            (Type::ROCK.0, Type::BUG.0, 20),
            (Type::ROCK.0, Type::FLYING.0, 20),
            (Type::NORMAL.0, Type::GHOST.0, 0),
            (Type::WATER.0, Type::WATER.0, 5),
        ])
    }

    #[test]
    fn an_absent_pair_is_neutral() {
        assert_eq!(chart().multiplier(Type::NORMAL, Type::NORMAL), NEUTRAL);
        assert_eq!(
            chart().effectiveness_percent(Type::NORMAL, (Type::NORMAL, None)),
            100
        );
    }

    #[test]
    fn a_dual_type_multiplies_twice() {
        let c = chart();
        assert_eq!(
            c.effectiveness_percent(Type::ROCK, (Type::BUG, Some(Type::FLYING))),
            400
        );
        assert_eq!(
            c.effectiveness_percent(Type::NORMAL, (Type::GHOST, None)),
            0
        );
    }

    /// **The two multiplications truncate separately**, so the answer is not the
    /// product of the two fractions.
    ///
    /// A ×0.5 then a ×2 on 5 damage gives `5*5/10 = 2` then `2*20/10 = 4`. The
    /// combined multiplier is ×1, so a model that applied it in one step would
    /// say 5. The order of the rows in `TypeEffects` therefore matters, and
    /// [`TypeChart::apply`] is the only thing that may be used for damage —
    /// [`TypeChart::effectiveness_percent`] is for display.
    #[test]
    fn the_chart_truncates_between_the_two_multiplications() {
        let c = TypeChart::from_entries(vec![
            (Type::WATER.0, Type::WATER.0, 5),
            (Type::WATER.0, Type::DRAGON.0, 20),
        ]);
        assert_eq!(
            c.apply(5, Type::WATER, (Type::WATER, Some(Type::DRAGON))),
            4
        );
        // The combined multiplier is exactly ×1, which would have given 5.
        assert_eq!(
            c.effectiveness_percent(Type::WATER, (Type::WATER, Some(Type::DRAGON))),
            100
        );
    }

    /// A Pokémon whose two type slots hold the same id is single-typed; the
    /// cartridge does not apply the row twice.
    #[test]
    fn a_repeated_type_slot_is_applied_once() {
        let c = chart();
        assert_eq!(
            c.effectiveness_percent(Type::WATER, (Type::WATER, Some(Type::WATER))),
            50
        );
    }

    /// STAB truncates too: 15/10 of 7 is 10, not 10.5 and not 11.
    #[test]
    fn stab_truncates() {
        assert_eq!(apply_stab(7), 10);
        assert_eq!(apply_stab(1), 1);
    }
}

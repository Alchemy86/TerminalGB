//! What a hunt is looking for, and what the odds of finding it actually are.
//!
//! # Gen 1 has no shiny Pokémon
//!
//! Shininess is a **Generation II** concept. What "shiny hunting in Gen 1"
//! means — and all it can mean — is hunting the specific DV spread that
//! *becomes* shiny the moment the Pokémon is traded forward through the Time
//! Capsule. The DVs travel across unchanged, and Gen 2 decides shininess from
//! them; the Gen 1 cartridge itself has no notion of any of it.
//!
//! That rule is already implemented once, in
//! [`terminalgb_plugin_trainer::derived::shiny_if_traded`], and this module calls
//! it rather than restating it. There is exactly one copy of the rule in the
//! tree, shared by the trainer panel's `★`, the wild-encounter banner and the
//! hunter — if it is ever wrong, it is wrong in one place, and
//! [`tests::the_shiny_target_is_the_trainer_panels_rule_and_nothing_else`]
//! fails the moment a second copy appears.
//!
//! # Why a general target and not just "shiny"
//!
//! Two reasons, both practical:
//!
//! * A hit at 1 in 8192 is rare enough that *proving the machinery works* by
//!   waiting for one is a bad test. Targeting a deliberately common spread
//!   (`spc=10`, 1 in 16) exercises the whole stop-and-preserve path in seconds,
//!   which is how the demonstration in `docs/shiny-hunt.md` is done.
//! * Real hunters want more than shininess: a flawless spread, a particular
//!   Hidden Power, a specific species out of a route's encounter table.
//!
//! Every target reports its **exact** probability, computed by enumerating all
//! 65,536 DV combinations rather than by multiplying assumed independent terms.
//! The HP DV is derived from the low bit of the other four (`CalcStat`,
//! `pokered/home/move_mon.asm`), so the terms are *not* independent and a closed
//! form is easy to get quietly wrong — see
//! [`tests::the_hp_dv_is_derived_so_odds_are_enumerated_not_multiplied`].

use terminalgb_plugin_trainer::derived::shiny_if_traded;
use terminalgb_plugin_trainer::gen1::Dvs;

/// Resolve a species *name* to its internal index and canonical spelling.
///
/// A function rather than a table because **we ship no species table**: the
/// names live in the player's own cartridge and the caller reads them from
/// there (`terminalgb_plugin_trainer::rom::RomTables::species_name`). See
/// `AGENTS.md` §"Licence boundary: format knowledge is ours, game assets are
/// not".
pub type SpeciesLookup<'a> = &'a dyn Fn(&str) -> Option<(u8, String)>;

/// One of the five DVs. Attack, defence, speed and special are stored as
/// nybbles in the two `wEnemyMonDVs` bytes; HP is assembled from their low bits,
/// so it is derived, not stored.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stat {
    Attack,
    Defense,
    Speed,
    Special,
    Hp,
}

impl Stat {
    pub fn parse(name: &str) -> Option<Stat> {
        Some(match name {
            "atk" | "attack" => Stat::Attack,
            "def" | "defense" | "defence" => Stat::Defense,
            "spd" | "speed" => Stat::Speed,
            "spc" | "special" => Stat::Special,
            "hp" => Stat::Hp,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Stat::Attack => "atk",
            Stat::Defense => "def",
            Stat::Speed => "spd",
            Stat::Special => "spc",
            Stat::Hp => "hp",
        }
    }

    pub fn of(self, dvs: Dvs) -> u8 {
        match self {
            Stat::Attack => dvs.attack,
            Stat::Defense => dvs.defense,
            Stat::Speed => dvs.speed,
            Stat::Special => dvs.special,
            Stat::Hp => dvs.hp(),
        }
    }
}

/// One condition on a DV spread. A [`Target`] is the conjunction of its rules.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Rule {
    /// Would be shiny if traded to Gen 2 — [`shiny_if_traded`].
    ShinyIfTraded,
    /// This DV is exactly `value`.
    Exact(Stat, u8),
    /// This DV is at least `value`.
    AtLeast(Stat, u8),
    /// Every bit in `mask` is set in this DV.
    Bits(Stat, u8),
    /// The five DVs sum to at least this (maximum 75).
    TotalAtLeast(u8),
}

impl Rule {
    fn holds(self, dvs: Dvs) -> bool {
        match self {
            Rule::ShinyIfTraded => shiny_if_traded(dvs),
            Rule::Exact(s, v) => s.of(dvs) == v,
            Rule::AtLeast(s, v) => s.of(dvs) >= v,
            Rule::Bits(s, m) => s.of(dvs) & m == m,
            Rule::TotalAtLeast(v) => dv_total(dvs) >= v,
        }
    }

    fn describe(self) -> String {
        match self {
            Rule::ShinyIfTraded => "shiny-if-traded".to_string(),
            Rule::Exact(s, v) => format!("{}={v}", s.name()),
            Rule::AtLeast(s, v) => format!("{}>={v}", s.name()),
            Rule::Bits(s, m) => format!("{}&{m}", s.name()),
            Rule::TotalAtLeast(v) => format!("total>={v}"),
        }
    }
}

/// The five DVs summed, out of 75.
pub fn dv_total(dvs: Dvs) -> u8 {
    dvs.attack + dvs.defense + dvs.speed + dvs.special + dvs.hp()
}

/// What the hunt stops for.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Target {
    rules: Vec<Rule>,
    /// Optional species filter, by **internal index** (`MonsterNames` order),
    /// not by Pokédex number — `wEnemyMonSpecies` holds the internal index.
    /// Only meaningful for a wild hunt; a static encounter is one species by
    /// definition.
    species: Option<u8>,
    species_label: Option<String>,
}

impl Target {
    /// The canonical hunt: a spread that turns shiny in the Time Capsule.
    pub fn shiny() -> Target {
        Target {
            rules: vec![Rule::ShinyIfTraded],
            species: None,
            species_label: None,
        }
    }

    /// Parse a target expression. Terms are separated by commas or whitespace
    /// and every term must hold.
    ///
    /// ```text
    /// shiny                 defence, speed and special all 10; attack bit 1 set
    /// perfect               every stored DV 15 (and therefore hp 15)
    /// spc=10                an exact DV
    /// spd>=14               a floor
    /// atk&2                 every bit of the mask set
    /// total>=70             the five DVs summed, out of 75
    /// species=NIDORAN       only this species counts as a hit (wild hunts)
    /// ```
    ///
    /// A species *name* is resolved through the player's own cartridge when
    /// `names` is supplied — we ship no species table, for the same reason the
    /// link partner does not (`AGENTS.md` §"Licence boundary"). A bare number is
    /// taken as the internal index.
    pub fn parse(spec: &str, names: Option<SpeciesLookup<'_>>) -> Result<Target, String> {
        let mut out = Target::default();
        for raw in spec.split([',', ' ', '\t', '\n']) {
            let term = raw.trim();
            if term.is_empty() {
                continue;
            }
            let lower = term.to_ascii_lowercase();
            match lower.as_str() {
                "shiny" => {
                    out.rules.push(Rule::ShinyIfTraded);
                    continue;
                }
                "perfect" | "flawless" => {
                    for s in [Stat::Attack, Stat::Defense, Stat::Speed, Stat::Special] {
                        out.rules.push(Rule::Exact(s, 15));
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(rest) = lower.strip_prefix("species=") {
                let (id, label) = resolve_species(rest, names)?;
                out.species = Some(id);
                out.species_label = Some(label);
                continue;
            }
            if let Some((lhs, rhs)) = lower.split_once(">=") {
                let v: u8 = rhs
                    .parse()
                    .map_err(|_| format!("`{term}`: `{rhs}` is not a number"))?;
                if lhs == "total" {
                    if v > 75 {
                        return Err(format!("`{term}`: the DV total maxes out at 75"));
                    }
                    out.rules.push(Rule::TotalAtLeast(v));
                } else {
                    let s = Stat::parse(lhs)
                        .ok_or_else(|| format!("`{term}`: `{lhs}` is not a DV"))?;
                    check_nybble(term, v)?;
                    out.rules.push(Rule::AtLeast(s, v));
                }
                continue;
            }
            if let Some((lhs, rhs)) = lower.split_once('&') {
                let s = Stat::parse(lhs)
                    .ok_or_else(|| format!("`{term}`: `{lhs}` is not a DV"))?;
                let v: u8 = rhs
                    .parse()
                    .map_err(|_| format!("`{term}`: `{rhs}` is not a number"))?;
                check_nybble(term, v)?;
                out.rules.push(Rule::Bits(s, v));
                continue;
            }
            if let Some((lhs, rhs)) = lower.split_once('=') {
                let s = Stat::parse(lhs)
                    .ok_or_else(|| format!("`{term}`: `{lhs}` is not a DV"))?;
                let v: u8 = rhs
                    .parse()
                    .map_err(|_| format!("`{term}`: `{rhs}` is not a number"))?;
                check_nybble(term, v)?;
                out.rules.push(Rule::Exact(s, v));
                continue;
            }
            return Err(format!(
                "`{term}` is not a target term (try `shiny`, `spc=10`, \
                 `total>=70`, `species=PIDGEY`)"
            ));
        }
        if out.rules.is_empty() && out.species.is_none() {
            return Err("an empty target would stop on the first encounter".into());
        }
        Ok(out)
    }

    /// Does this DV spread, on this species, end the hunt?
    pub fn matches(&self, dvs: Dvs, species: u8) -> bool {
        if let Some(want) = self.species {
            if species != want {
                return false;
            }
        }
        self.dvs_match(dvs)
    }

    /// Whether the DV rules alone match, ignoring the species filter. Used for
    /// reporting: "the spread was right but the species was not" is a thing a
    /// hunter wants to see rather than have hidden.
    pub fn dvs_match(&self, dvs: Dvs) -> bool {
        self.rules.iter().all(|r| r.holds(dvs))
    }

    /// Whether this target filters on species at all.
    pub fn species(&self) -> Option<u8> {
        self.species
    }

    /// Exact probability that a uniformly random DV pair satisfies the **DV**
    /// rules, computed by enumerating all 65,536 pairs.
    ///
    /// This deliberately does **not** fold in a species filter: how often a
    /// route yields a given species is a property of that route's encounter
    /// table and the slot roll, not of the DV space, and pretending otherwise
    /// would produce a confident wrong ETA. See [`Target::species_caveat`].
    pub fn dv_probability(&self) -> f64 {
        let mut hits = 0u32;
        for hi in 0..=255u8 {
            for lo in 0..=255u8 {
                if self.dvs_match(Dvs::from_bytes(hi, lo)) {
                    hits += 1;
                }
            }
        }
        f64::from(hits) / 65536.0
    }

    /// Mean number of encounters needed, `1/p`. Infinite for an impossible
    /// target — which is a legitimate thing to report rather than to hide.
    pub fn expected_encounters(&self) -> f64 {
        let p = self.dv_probability();
        if p <= 0.0 {
            f64::INFINITY
        } else {
            1.0 / p
        }
    }

    /// Encounters by which half of all hunts have finished,
    /// `ln 2 / -ln(1 - p)`. Worth reporting beside the mean: a geometric
    /// distribution's mean sits well above its median, so quoting only the mean
    /// reads pessimistically and makes people give up early.
    pub fn median_encounters(&self) -> f64 {
        let p = self.dv_probability();
        if p <= 0.0 {
            return f64::INFINITY;
        }
        if p >= 1.0 {
            return 1.0;
        }
        std::f64::consts::LN_2 / -(1.0 - p).ln()
    }

    /// Human-readable one-liner, e.g. `shiny-if-traded, species=NIDORAN`.
    pub fn label(&self) -> String {
        let mut parts: Vec<String> = self.rules.iter().map(|r| r.describe()).collect();
        if let Some(name) = &self.species_label {
            parts.push(format!("species={name}"));
        }
        if parts.is_empty() {
            "anything".to_string()
        } else {
            parts.join(", ")
        }
    }

    /// The honest caveat to print beside the odds whenever a species filter is
    /// set, since the odds ignore it.
    pub fn species_caveat(&self) -> Option<String> {
        self.species_label.as_ref().map(|n| {
            format!(
                "odds are per encounter OF {n}; the route's encounter table \
                 decides how many encounters that is"
            )
        })
    }
}

fn check_nybble(term: &str, v: u8) -> Result<(), String> {
    if v > 15 {
        return Err(format!("`{term}`: a DV is 0..=15"));
    }
    Ok(())
}

fn resolve_species(
    text: &str,
    names: Option<SpeciesLookup<'_>>,
) -> Result<(u8, String), String> {
    if let Ok(n) = text.parse::<u8>() {
        return Ok((n, format!("#{n}")));
    }
    let lookup = names.ok_or_else(|| {
        format!("`species={text}`: no cartridge to resolve the name against")
    })?;
    lookup(text).ok_or_else(|| format!("`species={text}`: no such species in this ROM"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dvs(a: u8, d: u8, s: u8, c: u8) -> Dvs {
        Dvs {
            attack: a,
            defense: d,
            speed: s,
            special: c,
        }
    }

    #[test]
    fn the_shiny_target_is_the_trainer_panels_rule_and_nothing_else() {
        // Every one of the 65,536 spreads must agree with the single
        // implementation in `terminalgb_plugin_trainer::derived`. If the hunter
        // ever grows its own copy of the rule, this fails.
        let t = Target::shiny();
        for hi in 0..=255u8 {
            for lo in 0..=255u8 {
                let d = Dvs::from_bytes(hi, lo);
                assert_eq!(
                    t.dvs_match(d),
                    shiny_if_traded(d),
                    "hi={hi:#04x} lo={lo:#04x}"
                );
            }
        }
    }

    #[test]
    fn the_shiny_odds_are_one_in_8192() {
        let t = Target::shiny();
        assert!((t.dv_probability() - 8.0 / 65536.0).abs() < 1e-12);
        assert!((t.expected_encounters() - 8192.0).abs() < 1e-6);
        // The median is well below the mean — that is the point of printing it.
        assert!(t.median_encounters() < 5700.0);
        assert!(t.median_encounters() > 5600.0);
    }

    #[test]
    fn a_contrived_target_is_common_enough_to_demonstrate_with() {
        let t = Target::parse("spc=10", None).unwrap();
        assert!((t.dv_probability() - 1.0 / 16.0).abs() < 1e-12);
        assert!(t.matches(dvs(0, 0, 0, 10), 1));
        assert!(!t.matches(dvs(10, 10, 10, 11), 1));
    }

    #[test]
    fn perfect_is_one_in_65536() {
        let t = Target::parse("perfect", None).unwrap();
        assert!((t.dv_probability() - 1.0 / 65536.0).abs() < 1e-12);
        assert!(t.matches(dvs(15, 15, 15, 15), 1));
    }

    #[test]
    fn the_hp_dv_is_derived_so_odds_are_enumerated_not_multiplied() {
        // `hp=15` requires the low bit of all four stored DVs, which is 1/16 of
        // the space. It is *not* independent of `atk=15`: naive multiplication
        // would say 1/16 * 1/16 = 1/256, but `atk=15` already supplies one of
        // the four bits `hp=15` needs, so the truth is 1/16 * 1/8 = 1/128.
        let t = Target::parse("hp=15,atk=15", None).unwrap();
        assert!((t.dv_probability() - 1.0 / 128.0).abs() < 1e-12);
    }

    #[test]
    fn total_and_mask_and_floor_terms_parse() {
        let t = Target::parse("total>=70", None).unwrap();
        assert!(t.matches(dvs(15, 15, 15, 15), 1)); // 15*4 + 15 = 75
        assert!(!t.matches(dvs(10, 10, 10, 10), 1));
        let t = Target::parse("atk&2", None).unwrap();
        assert!(t.matches(dvs(2, 0, 0, 0), 1));
        assert!(!t.matches(dvs(1, 0, 0, 0), 1));
        let t = Target::parse("spd>=14", None).unwrap();
        assert!(t.matches(dvs(0, 0, 14, 0), 1));
        assert!(!t.matches(dvs(0, 0, 13, 0), 1));
    }

    #[test]
    fn a_species_filter_narrows_hits_but_not_the_quoted_dv_odds() {
        let lookup = |name: &str| -> Option<(u8, String)> {
            name.eq_ignore_ascii_case("pidgey")
                .then(|| (0x24, "PIDGEY".into()))
        };
        let t = Target::parse("shiny,species=PIDGEY", Some(&lookup)).unwrap();
        let shiny = Dvs::from_bytes(0xEA, 0xAA);
        assert!(t.matches(shiny, 0x24));
        assert!(!t.matches(shiny, 0x25));
        // The quoted probability is per encounter *of that species*, and the
        // caveat that says so must be there.
        assert!((t.dv_probability() - 8.0 / 65536.0).abs() < 1e-12);
        assert!(t.species_caveat().is_some());
        assert_eq!(t.label(), "shiny-if-traded, species=PIDGEY");
    }

    #[test]
    fn nonsense_targets_are_refused_rather_than_silently_matching_everything() {
        assert!(Target::parse("", None).is_err());
        assert!(Target::parse("atk=16", None).is_err());
        assert!(Target::parse("wibble", None).is_err());
        assert!(Target::parse("total>=99", None).is_err());
        // A name with no cartridge behind it is an error, not a guess.
        assert!(Target::parse("species=PIDGEY", None).is_err());
    }

    #[test]
    fn dv_total_counts_the_derived_hp_dv_too() {
        assert_eq!(dv_total(dvs(15, 15, 15, 15)), 75);
        assert_eq!(dv_total(dvs(0, 0, 0, 0)), 0);
        // atk 1 -> hp bit 3 set (8), so 1 + 8 = 9.
        assert_eq!(dv_total(dvs(1, 0, 0, 0)), 9);
    }
}

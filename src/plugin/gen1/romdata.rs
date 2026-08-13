//! Game data read **out of the cartridge**, not embedded here.
//!
//! To hand Pokémon Blue a well-formed Pokémon we need real numbers: base stats
//! (to compute the stats the game will *not* recompute after a trade), the
//! internal-index↔Pokédex-number mapping, species names, and each move's PP.
//!
//! Two ways to get them: ship a table of all 151 species, or read the tables the
//! cartridge already contains. This module does the second, for three reasons:
//!
//! 1. **No game data lives in this repository.** The numbers are Nintendo's; the
//!    player already owns them, in the ROM they supplied.
//! 2. **It cannot drift.** Red, Blue and Yellow differ; a table read from *the
//!    running cartridge* is right for that cartridge by construction.
//! 3. **It is self-verifying.** Each table is found by a distinctive byte
//!    signature and then cross-checked against a second known entry, so a
//!    mislocated table is an error rather than a silently wrong Pokémon.
//!
//! The table *layouts* (28-byte base-stat records in Pokédex order, the
//! internal-index→dex list, 6-byte move records) are documented facts, taken
//! from the `pret/pokered` disassembly —
//! [`data/pokemon/base_stats/`](https://github.com/pret/pokered/tree/master/data/pokemon/base_stats),
//! [`data/pokemon/dex_order.asm`](https://github.com/pret/pokered/blob/master/data/pokemon/dex_order.asm),
//! [`data/moves/moves.asm`](https://github.com/pret/pokered/blob/master/data/moves/moves.asm).
//! No code from that project is used here.

use super::text;

/// One base-stat record's length in the ROM (`BaseStats` entries).
const BASE_STATS_ENTRY: usize = 28;
/// Offsets within a base-stat record.
const OFF_HP: usize = 1;
const OFF_TYPE1: usize = 6;
const OFF_TYPE2: usize = 7;
const OFF_CATCH_RATE: usize = 8;
const OFF_GROWTH_RATE: usize = 19;
/// One move record's length (`Moves` entries).
const MOVE_ENTRY: usize = 6;
const OFF_MOVE_PP: usize = 5;
/// Species-name records are fixed 10-byte fields in internal-index order.
const MON_NAME_LENGTH: usize = 10;

/// The five stats a Gen 1 Pokémon has, in the order the data structures use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Stats {
    pub hp: u16,
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub special: u16,
}

/// Everything the cartridge knows about one species.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeciesInfo {
    /// Internal index — what the party data structure actually stores.
    pub index: u8,
    /// Pokédex number, which is what a human means by "#151".
    pub dex: u8,
    pub name: String,
    pub base: Stats,
    pub type1: u8,
    pub type2: u8,
    pub catch_rate: u8,
    pub growth_rate: u8,
}

/// Why a lookup failed. These are user-facing: a wrong species name in a trade
/// file should say so, not produce a corrupt Pokémon.
#[derive(Debug, PartialEq, Eq)]
pub enum RomError {
    /// A table could not be located in this ROM (not a Gen 1 English cartridge).
    TableNotFound(&'static str),
    /// The table was found but a cross-check entry did not match.
    TableFailedVerification(&'static str),
    UnknownSpecies(String),
    UnknownMove(String),
    /// A species with no base-stat record (only possible for a malformed ROM).
    NoBaseStats(u8),
}

impl std::fmt::Display for RomError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RomError::TableNotFound(t) => write!(
                f,
                "could not find the '{t}' table in this ROM — the Gen 1 link \
                 partner needs an English Red/Blue/Yellow cartridge"
            ),
            RomError::TableFailedVerification(t) => {
                write!(f, "the '{t}' table did not verify — refusing to guess")
            }
            RomError::UnknownSpecies(s) => write!(f, "unknown species '{s}'"),
            RomError::UnknownMove(s) => write!(f, "unknown move '{s}'"),
            RomError::NoBaseStats(i) => {
                write!(f, "no base stats for species index {i}")
            }
        }
    }
}

/// Located offsets into the ROM image.
pub struct RomTables {
    rom: Vec<u8>,
    base_stats: usize,
    mew_base_stats: usize,
    dex_order: usize,
    mon_names: usize,
    moves: usize,
    move_names: usize,
    /// How many species the dex-order table lists (Gen 1 has 190 slots, 151 used
    /// and 39 "MissingNo." holes — those holes are exactly the invalid-species
    /// values that make Gen 1 famous, see [`SpeciesInfo`] callers).
    num_slots: usize,
}

/// Search `hay` for `needle`, requiring exactly one match. Ambiguity means the
/// signature is too weak; that is a bug to fix, not a coin to flip.
fn find_unique(hay: &[u8], needle: &[u8], what: &'static str) -> Result<usize, RomError> {
    let mut found = None;
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            if found.is_some() {
                return Err(RomError::TableFailedVerification(what));
            }
            found = Some(i);
        }
        i += 1;
    }
    found.ok_or(RomError::TableNotFound(what))
}

fn encode_name(s: &str) -> Vec<u8> {
    s.chars().filter_map(text::encode_char).collect()
}

impl RomTables {
    /// Locate every table we need in a ROM image, verifying each.
    ///
    /// The signatures are the *first* entry of each table, which is fixed and
    /// well known:
    ///
    /// | table | signature |
    /// |---|---|
    /// | `BaseStats` | Bulbasaur: dex 1, 45/49/49/45/65, GRASS/POISON, catch 45, exp 64 |
    /// | `MewBaseStats` | Mew is stored apart from the main table in Red/Blue |
    /// | `PokedexOrder` | internal 1..6 → dex 112, 115, 32, 35, 21, 100 |
    /// | `MonsterNames` | internal index 1 is `RHYDON` |
    /// | `Moves` | POUND then KARATE CHOP |
    /// | `MoveNames` | `POUND` + terminator |
    pub fn locate(rom: Vec<u8>) -> Result<RomTables, RomError> {
        let base_stats = find_unique(
            &rom,
            &[1, 45, 49, 49, 45, 65, 0x16, 0x03, 45, 64],
            "BaseStats",
        )?;
        let mew_base_stats = find_unique(
            &rom,
            &[151, 100, 100, 100, 100, 100, 0x18, 0x18, 45, 64],
            "MewBaseStats",
        )?;
        let dex_order = find_unique(&rom, &[112, 115, 32, 35, 21, 100], "PokedexOrder")?;
        let mon_names = find_unique(&rom, &encode_name("RHYDON"), "MonsterNames")?;
        let moves =
            find_unique(&rom, &[1, 0, 40, 0, 255, 35, 2, 0, 50, 0, 255, 25], "Moves")?;
        let mut sig = encode_name("POUND");
        sig.push(text::TERMINATOR);
        let move_names = find_unique(&rom, &sig, "MoveNames")?;

        let tables = RomTables {
            rom,
            base_stats,
            mew_base_stats,
            dex_order,
            mon_names,
            moves,
            move_names,
            num_slots: 190,
        };

        // Cross-checks: a table located by its first entry could still be the
        // wrong table if the signature happened to appear elsewhere. Verify a
        // *later* entry of each, chosen so a one-record misalignment fails.
        let pikachu = tables.species_by_index(0x54)?;
        if pikachu.name != "PIKACHU" || pikachu.dex != 25 || pikachu.base.speed != 90 {
            return Err(RomError::TableFailedVerification("BaseStats/MonsterNames"));
        }
        // Move 85 is THUNDERBOLT: 15 PP.
        if tables.move_pp(85) != 15 {
            return Err(RomError::TableFailedVerification("Moves"));
        }
        if tables.move_name(85).as_deref() != Some("THUNDERBOLT") {
            return Err(RomError::TableFailedVerification("MoveNames"));
        }
        Ok(tables)
    }

    /// Pokédex number for an internal index, or 0 for the unused slots.
    pub fn dex_of_index(&self, index: u8) -> u8 {
        if index == 0 {
            return 0;
        }
        self.rom
            .get(self.dex_order + index as usize - 1)
            .copied()
            .unwrap_or(0)
    }

    fn base_stats_record(&self, dex: u8) -> Option<&[u8]> {
        if dex == 0 {
            return None;
        }
        // Mew is not in the main table in Red/Blue — it lives on its own,
        // which is precisely why it is the species this plugin is most likely
        // to be asked for and most likely to get wrong.
        let start = if dex == 151 {
            self.mew_base_stats
        } else {
            self.base_stats + (dex as usize - 1) * BASE_STATS_ENTRY
        };
        self.rom.get(start..start + BASE_STATS_ENTRY)
    }

    /// Full species info by internal index (the value stored in party data).
    pub fn species_by_index(&self, index: u8) -> Result<SpeciesInfo, RomError> {
        let dex = self.dex_of_index(index);
        let rec = self
            .base_stats_record(dex)
            .ok_or(RomError::NoBaseStats(index))?;
        let name_at = self.mon_names + (index as usize - 1) * MON_NAME_LENGTH;
        let name = self
            .rom
            .get(name_at..name_at + MON_NAME_LENGTH)
            .map(text::decode)
            .unwrap_or_default();
        Ok(SpeciesInfo {
            index,
            dex,
            name,
            base: Stats {
                hp: rec[OFF_HP] as u16,
                attack: rec[OFF_HP + 1] as u16,
                defense: rec[OFF_HP + 2] as u16,
                speed: rec[OFF_HP + 3] as u16,
                special: rec[OFF_HP + 4] as u16,
            },
            type1: rec[OFF_TYPE1],
            type2: rec[OFF_TYPE2],
            catch_rate: rec[OFF_CATCH_RATE],
            growth_rate: rec[OFF_GROWTH_RATE],
        })
    }

    /// Resolve a user-supplied species: a name (`"Mew"`), a `#`-prefixed
    /// Pokédex number (`"#151"`), or a bare internal index (`"21"`).
    ///
    /// Names are matched case-insensitively and ignoring non-alphanumerics, so
    /// `"Mr. Mime"`, `"MR.MIME"` and `"mrmime"` all work.
    pub fn resolve_species(&self, s: &str) -> Result<SpeciesInfo, RomError> {
        let t = s.trim();
        if let Some(rest) = t.strip_prefix('#') {
            if let Ok(dex) = rest.trim().parse::<u8>() {
                for index in 1..=self.num_slots as u16 {
                    if self.dex_of_index(index as u8) == dex {
                        return self.species_by_index(index as u8);
                    }
                }
                return Err(RomError::UnknownSpecies(s.to_string()));
            }
        }
        if let Ok(index) = t.parse::<u8>() {
            return self.species_by_index(index);
        }
        let want = normalise(t);
        for index in 1..=self.num_slots as u16 {
            let index = index as u8;
            if self.dex_of_index(index) == 0 {
                continue;
            }
            if let Ok(info) = self.species_by_index(index) {
                if normalise(&info.name) == want {
                    return Ok(info);
                }
            }
        }
        Err(RomError::UnknownSpecies(s.to_string()))
    }

    /// Base PP of a move id (`0` for "no move").
    pub fn move_pp(&self, id: u8) -> u8 {
        if id == 0 {
            return 0;
        }
        self.rom
            .get(self.moves + (id as usize - 1) * MOVE_ENTRY + OFF_MOVE_PP)
            .copied()
            .unwrap_or(0)
    }

    /// Name of a move id. `MoveNames` is a terminator-separated list, so this
    /// walks it rather than indexing.
    pub fn move_name(&self, id: u8) -> Option<String> {
        if id == 0 {
            return None;
        }
        let mut p = self.move_names;
        for _ in 1..id {
            while *self.rom.get(p)? != text::TERMINATOR {
                p += 1;
            }
            p += 1;
        }
        let start = p;
        while *self.rom.get(p)? != text::TERMINATOR {
            p += 1;
        }
        Some(text::decode(&self.rom[start..p]))
    }

    /// Resolve a user-supplied move: a name or a bare id.
    pub fn resolve_move(&self, s: &str) -> Result<u8, RomError> {
        let t = s.trim();
        if t.is_empty() || t.eq_ignore_ascii_case("none") {
            return Ok(0);
        }
        if let Ok(id) = t.parse::<u8>() {
            return Ok(id);
        }
        let want = normalise(t);
        // Gen 1 has 165 moves.
        for id in 1..=165u8 {
            if let Some(n) = self.move_name(id) {
                if normalise(&n) == want {
                    return Ok(id);
                }
            }
        }
        Err(RomError::UnknownMove(s.to_string()))
    }
}

/// Fold a name to a comparison key: lowercase, alphanumerics only. Lets a user
/// write `Mr. Mime`, `MR.MIME` or `mrmime`, and `Double-Edge` or `DOUBLE-EDGE`.
fn normalise(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Total experience needed to *be* at `level`, from the species' growth rate.
///
/// A traded Pokémon whose experience does not match its level will jump or stall
/// the next time it gains any, so this has to be right. The four Gen 1 growth
/// curves (Pan Docs is silent on this; the formulas are the standard documented
/// ones, e.g. [Bulbapedia, "Experience"](https://bulbapedia.bulbagarden.net/wiki/Experience)):
///
/// * `0` medium fast — `n³`
/// * `3` medium slow — `(6/5)n³ − 15n² + 100n − 140`
/// * `4` fast — `(4/5)n³`
/// * `5` slow — `(5/4)n³`
pub fn experience_for_level(growth_rate: u8, level: u8) -> u32 {
    let n = level as i64;
    let exp: i64 = match growth_rate {
        4 => 4 * n * n * n / 5,
        5 => 5 * n * n * n / 4,
        3 => (6 * n * n * n) / 5 - 15 * n * n + 100 * n - 140,
        _ => n * n * n,
    };
    // Level 1 medium-slow evaluates to -49 by the formula; the games clamp.
    exp.clamp(0, 0x00FF_FFFF) as u32
}

/// Gen 1 stat formula with the given base stat, DV, stat experience and level.
///
/// `((base + DV) * 2 + floor(ceil(sqrt(statexp)) / 4)) * level / 100`, then
/// `+ level + 10` for HP or `+ 5` for everything else. Source: the same
/// well-documented formula the games implement
/// ([Bulbapedia, "Stat"](https://bulbapedia.bulbagarden.net/wiki/Stat)).
pub fn compute_stat(base: u16, dv: u8, stat_exp: u16, level: u8, is_hp: bool) -> u16 {
    let effort = isqrt_ceil(stat_exp as u32) / 4;
    let core = ((base as u32 + dv as u32) * 2 + effort) * level as u32 / 100;
    let v = if is_hp {
        core + level as u32 + 10
    } else {
        core + 5
    };
    v.min(999) as u16
}

/// `ceil(sqrt(n))`, integer-only (the games use a lookup; this matches it).
fn isqrt_ceil(n: u32) -> u32 {
    if n == 0 {
        return 0;
    }
    let mut r = (n as f64).sqrt() as u32;
    while r * r < n {
        r += 1;
    }
    while r > 0 && (r - 1) * (r - 1) >= n {
        r -= 1;
    }
    r
}

/// The HP DV is not stored: it is assembled from the low bit of each of the
/// other four DVs (Attack, Defense, Speed, Special — in that order, most
/// significant first). This is the quirk that makes Gen 1 shininess and
/// Hyper Beam-era stat spreads work the way they do.
pub fn hp_dv(attack: u8, defense: u8, speed: u8, special: u8) -> u8 {
    ((attack & 1) << 3) | ((defense & 1) << 2) | ((speed & 1) << 1) | (special & 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn experience_curves_match_known_values() {
        // Level 100 anchors, the standard published totals.
        assert_eq!(experience_for_level(0, 100), 1_000_000); // medium fast
        assert_eq!(experience_for_level(4, 100), 800_000); // fast
        assert_eq!(experience_for_level(5, 100), 1_250_000); // slow
        assert_eq!(experience_for_level(3, 100), 1_059_860); // medium slow
        assert_eq!(experience_for_level(0, 1), 1);
        // Medium slow is negative at level 1 by the raw formula; clamped.
        assert_eq!(experience_for_level(3, 1), 0);
    }

    #[test]
    fn stat_formula_matches_a_worked_example() {
        // A level 100 Mewtwo with perfect DVs and no stat experience:
        // Special base 154 → ((154+15)*2 + 0) * 100/100 + 5 = 343.
        assert_eq!(compute_stat(154, 15, 0, 100, false), 343);
        // HP base 106 → ((106+15)*2)*100/100 + 100 + 10 = 352.
        assert_eq!(compute_stat(106, 15, 0, 100, true), 352);
    }

    #[test]
    fn hp_dv_is_assembled_from_the_other_four() {
        assert_eq!(hp_dv(15, 15, 15, 15), 15);
        assert_eq!(hp_dv(0, 0, 0, 0), 0);
        assert_eq!(hp_dv(1, 0, 1, 0), 0b1010);
    }

    #[test]
    fn isqrt_ceil_is_exact() {
        assert_eq!(isqrt_ceil(0), 0);
        assert_eq!(isqrt_ceil(1), 1);
        assert_eq!(isqrt_ceil(2), 2);
        assert_eq!(isqrt_ceil(4), 2);
        assert_eq!(isqrt_ceil(65535), 256);
    }
}

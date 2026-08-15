//! Game data — base stats, moves, the type chart, names — read out of the
//! player's own cartridge.
//!
//! # Nothing here is embedded in this repository
//!
//! Every number this module hands the engine comes from a ROM image supplied at
//! run time. That is the repository's standing licence boundary (see
//! `CLAUDE.md`, "Licence boundary: format knowledge is ours, game assets are
//! not"): the *format* of a `BaseStats` record is a documented fact and is
//! written down below with its citation; the 151 records themselves are the
//! game's data and are the player's, not ours.
//!
//! It is also the more accurate arrangement. A table transcribed into Rust can
//! drift from the cartridge it claims to describe; a table read out of the
//! cartridge cannot. [`GameData::synthetic`] exists so the engine's own tests
//! run with no ROM at all.
//!
//! # Where the tables are
//!
//! Four of the five are at addresses from a verified build of
//! [`pret/pokered`](https://github.com/pret/pokered) at its pinned RGBDS
//! version, whose ROMs match `roms.sha1`; the same values serve Red and Blue.
//! A ROM offset is `bank * 0x4000 + (addr & 0x3FFF)`.
//!
//! | Table | Symbol | `bank:addr` | Source |
//! |---|---|---|---|
//! | Base stats | `BaseStats` | `0E:43DE` | symbol file |
//! | Mew's base stats | `MewBaseStats` | `01:425B` | symbol file |
//! | Dex order | `PokedexOrder` | `10:5024` | symbol file |
//! | Moves | `Moves` | `0E:4000` | symbol file |
//! | Species names | `MonsterNames` | `07:421E` | symbol file |
//! | Move names | `MoveNames` | `2C:4000` | symbol file |
//! | Type chart | `TypeEffects` | `0F:6474` | **read out of the cartridge** |
//!
//! `TypeEffects` is located by *searching the image for its own first four
//! rows* rather than by a transcribed address — the strongest of the three
//! grades in [`docs/gen1/README.md`](../../../docs/gen1/README.md), because it
//! needs no build, no symbol file and no arithmetic. Measured on the retail
//! Blue cartridge the signature occurs exactly once, at `0F:6474`, and the
//! table runs 82 rows to its `db -1` terminator.
//!
//! Every table is then **validated** before it is used ([`GameData::validate`]).
//! A wrong address does not produce subtly wrong battles here; it produces a
//! refusal that says which table failed and why.

use crate::types::{Type, TypeChart};

/// `BASE_DATA_SIZE` — `constants/pokemon_data_constants.asm`.
pub const BASE_DATA_SIZE: usize = 28;
/// `MOVE_LENGTH` — `constants/battle_constants.asm`.
pub const MOVE_LENGTH: usize = 6;
/// `MONSTER_NAME_LENGTH` — fixed-width entries in `MonsterNames`.
const MONSTER_NAME_LENGTH: usize = 10;
/// `NUM_POKEMON` — `constants/pokemon_constants.asm`. The number of *species*,
/// and the length of `BaseStats`.
pub const NUM_POKEMON: usize = 151;
/// The length of `BaseStats` — **150, not 151**.
///
/// `data/pokemon/base_stats.asm` ends `assert_table_length NUM_POKEMON - 1 ;
/// discount Mew`. Mew's record lives at `MewBaseStats` in a different bank, and
/// `GetMonHeader` (`home/pokemon.asm`) special-cases it. This too was wrong in
/// the first version of this file, and the cartridge is what caught it — the
/// validator refused a retail image with "record 151 says it is dex 17",
/// because index 150 was reading the bytes *after* the table.
pub const BASE_STATS_LEN: usize = NUM_POKEMON - 1;
/// `NUM_POKEMON_INDEXES` — `constants/pokemon_constants.asm`, and the length of
/// `PokedexOrder` (`assert_table_length NUM_POKEMON_INDEXES`).
///
/// **This is 190, not 151, and 39 of its entries are zero.** Internal index
/// numbers are not dense: the gaps are the slots the released game leaves empty
/// — the "MissingNo" indices — and `PokedexOrder` marks each of them with a `0`
/// rather than omitting it, because the table is indexed by internal number.
///
/// This constant was **wrong in the first version of this file**, which assumed
/// 151 dense entries, and the cartridge is what corrected it: the validator
/// refused a perfectly good retail Blue image with "0 is not a fresh Pokédex
/// number". That is the oracle rule working exactly as intended, and it is
/// recorded here so nobody re-derives 151 from the species count.
pub const NUM_POKEMON_INDEXES: usize = 190;
/// `NUM_ATTACKS` — `constants/move_constants.asm`.
pub const NUM_MOVES: usize = 165;
/// `MEW` — internal index `$15`. Its base stats live outside `BaseStats`;
/// `GetMonHeader` (`home/pokemon.asm`) special-cases it.
const MEW_INDEX: u8 = 0x15;
/// `STRUGGLE` — `constants/move_constants.asm`, move id 165.
pub const STRUGGLE: u8 = 165;

const fn flat(bank: usize, addr: u16) -> usize {
    bank * 0x4000 + (addr as usize & 0x3FFF)
}

const BASE_STATS: usize = flat(0x0E, 0x43DE);
const MEW_BASE_STATS: usize = flat(0x01, 0x425B);
const POKEDEX_ORDER: usize = flat(0x10, 0x5024);
const MOVES: usize = flat(0x0E, 0x4000);
const MONSTER_NAMES: usize = flat(0x07, 0x421E);
const MOVE_NAMES: usize = flat(0x2C, 0x4000);

/// The first four rows of `TypeEffects`, used to find it in an arbitrary image.
///
/// `data/types/type_matchups.asm`:
/// `db WATER, FIRE, SUPER_EFFECTIVE` / `db FIRE, GRASS, SUPER_EFFECTIVE` /
/// `db FIRE, ICE, SUPER_EFFECTIVE` / `db GRASS, WATER, SUPER_EFFECTIVE`.
const TYPE_EFFECTS_SIGNATURE: [u8; 12] = [
    0x15, 0x14, 20, // WATER  → FIRE   ×2
    0x14, 0x16, 20, // FIRE   → GRASS  ×2
    0x14, 0x19, 20, // FIRE   → ICE    ×2
    0x16, 0x15, 20, // GRASS  → WATER  ×2
];

/// A move's record in `Moves` (`data/moves/moves.asm`).
///
/// The `move` macro's six bytes, in order: animation id (interchangeable with
/// the move id), effect, power, type, accuracy, PP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MoveData {
    /// Move id, 1-based. `Moves` is indexed by `id - 1`.
    pub id: u8,
    /// Animation byte. Equal to the move id for every released move.
    pub animation: u8,
    /// `constants/move_effect_constants.asm`.
    pub effect: u8,
    pub power: u8,
    pub kind: Type,
    /// Accuracy as the cartridge stores it: `n percent` = `n * $FF / 100`, so
    /// 100 % is **255**, not 100. Never rescale this — [`crate::mechanics`]
    /// compares a random byte against it directly, and that comparison is the
    /// whole of the 1/256 miss.
    pub accuracy: u8,
    /// Base PP before PP Ups.
    pub pp: u8,
}

impl MoveData {
    /// A move that deals no damage of its own (status moves, and the multi-hit
    /// effects whose power the engine supplies).
    pub fn is_status(&self) -> bool {
        self.power == 0
    }

    /// Whether this move is resolved against Special rather than Attack/Defense
    /// — in Gen 1 a property of its type alone.
    pub fn is_special(&self) -> bool {
        self.kind.is_special()
    }
}

/// A species' `BaseStats` record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BaseStats {
    /// Pokédex number.
    pub dex: u8,
    pub hp: u8,
    pub attack: u8,
    pub defense: u8,
    pub speed: u8,
    pub special: u8,
    pub type1: Type,
    pub type2: Type,
    pub catch_rate: u8,
    pub base_exp: u8,
}

impl BaseStats {
    /// The defender's type pair, with the second slot dropped when it repeats
    /// the first — which is how the cartridge marks a single-typed species.
    pub fn types(&self) -> (Type, Option<Type>) {
        if self.type2 == self.type1 {
            (self.type1, None)
        } else {
            (self.type1, Some(self.type2))
        }
    }
}

/// Everything the engine needs to know about the cartridge it is modelling.
#[derive(Clone, Debug)]
pub struct GameData {
    /// Indexed by internal index number minus one.
    dex_order: Vec<u8>,
    /// Indexed by Pokédex number minus one; Mew's is stored separately.
    base_stats: Vec<BaseStats>,
    mew: Option<BaseStats>,
    /// Indexed by move id minus one.
    moves: Vec<MoveData>,
    species_names: Vec<String>,
    move_names: Vec<String>,
    chart: TypeChart,
}

/// Why a cartridge could not be read as Generation 1 battle data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DataError {
    /// The image is too short to hold a table at its documented offset.
    TooShort {
        table: &'static str,
        needed: usize,
        len: usize,
    },
    /// `TypeEffects` was not found by signature.
    NoTypeChart,
    /// A table was found but its contents are not plausible.
    Implausible { table: &'static str, why: String },
}

impl core::fmt::Display for DataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DataError::TooShort { table, needed, len } => write!(
                f,
                "cartridge is too short for `{table}`: needs {needed} bytes, image is {len}"
            ),
            DataError::NoTypeChart => write!(
                f,
                "no `TypeEffects` table found — this does not look like a Generation 1 \
                 Pokémon cartridge"
            ),
            DataError::Implausible { table, why } => {
                write!(f, "`{table}` does not look like a Generation 1 table: {why}")
            }
        }
    }
}

impl std::error::Error for DataError {}

impl GameData {
    /// Decode a cartridge image.
    ///
    /// Refuses rather than guesses: every table is validated before it is
    /// returned, so a wrong cartridge (Yellow, a translation, a hack with a
    /// shifted layout) produces a named error instead of a battle model that is
    /// subtly wrong. That is the same rule `plugins/trainer` applies to its
    /// panel — a graceful refusal is a feature.
    pub fn load_from_rom(rom: &[u8]) -> Result<GameData, DataError> {
        let chart = decode_type_chart(rom)?;
        let dex_order =
            read(rom, POKEDEX_ORDER, NUM_POKEMON_INDEXES, "PokedexOrder")?.to_vec();
        let mut base_stats = Vec::with_capacity(BASE_STATS_LEN);
        let stats_bytes = read(
            rom,
            BASE_STATS,
            BASE_STATS_LEN * BASE_DATA_SIZE,
            "BaseStats",
        )?;
        for i in 0..BASE_STATS_LEN {
            base_stats.push(decode_base_stats(
                &stats_bytes[i * BASE_DATA_SIZE..(i + 1) * BASE_DATA_SIZE],
            ));
        }
        // Mew's record is outside the table, in another bank.
        let mew = read(rom, MEW_BASE_STATS, BASE_DATA_SIZE, "MewBaseStats")
            .ok()
            .map(decode_base_stats);

        let move_bytes = read(rom, MOVES, NUM_MOVES * MOVE_LENGTH, "Moves")?;
        let mut moves = Vec::with_capacity(NUM_MOVES);
        for i in 0..NUM_MOVES {
            let r = &move_bytes[i * MOVE_LENGTH..(i + 1) * MOVE_LENGTH];
            moves.push(MoveData {
                id: (i + 1) as u8,
                animation: r[0],
                effect: r[1],
                power: r[2],
                kind: Type(r[3]),
                accuracy: r[4],
                pp: r[5],
            });
        }

        // `MonsterNames` is indexed by INTERNAL INDEX, so it is 190 entries
        // long like `PokedexOrder` — `assert_table_length NUM_POKEMON_INDEXES`
        // in `data/pokemon/names.asm`. Reading only 151 loses every species
        // whose internal index is above that, which is most of them: internal
        // index 1 is RHYDON, not BULBASAUR.
        let species_names = read(
            rom,
            MONSTER_NAMES,
            NUM_POKEMON_INDEXES * MONSTER_NAME_LENGTH,
            "MonsterNames",
        )?
        .chunks(MONSTER_NAME_LENGTH)
        .map(decode_text)
        .collect();
        let move_names = decode_move_names(rom);

        let data = GameData {
            dex_order,
            base_stats,
            mew,
            moves,
            species_names,
            move_names,
            chart,
        };
        data.validate()?;
        Ok(data)
    }

    /// Check that what was decoded really is Generation 1 battle data.
    ///
    /// Deliberately cheap, deliberately structural, and deliberately *not* a
    /// comparison against values baked in here — a check that pinned Bulbasaur's
    /// stats would be embedding the game's data to prove we had not embedded the
    /// game's data. Each rule below is a property of the *format*.
    pub fn validate(&self) -> Result<(), DataError> {
        let bad =
            |table: &'static str, why: String| DataError::Implausible { table, why };

        // Every move id is its own animation byte in the released game, PP is
        // capped at 40 by an `ASSERT` in `data/moves/moves.asm`, and a type id
        // must be in one of the two blocks.
        for m in &self.moves {
            if m.pp > 40 {
                return Err(bad(
                    "Moves",
                    format!("move {} has PP {} (> 40)", m.id, m.pp),
                ));
            }
            if m.kind.0 > 0x1A || (m.kind.0 > 0x08 && m.kind.0 < 0x14) {
                return Err(bad(
                    "Moves",
                    format!(
                        "move {} has type ${:02X}, which is not a type id",
                        m.id, m.kind.0
                    ),
                ));
            }
        }
        if self.moves.get(STRUGGLE as usize - 1).map(|m| m.power) != Some(50) {
            return Err(bad(
                "Moves",
                "STRUGGLE (id 165) does not have 50 power — the table is not aligned"
                    .into(),
            ));
        }

        // `PokedexOrder`'s 190 entries are the 151 Pokédex numbers, each exactly
        // once, plus 39 zeroes for the internal indices the released game leaves
        // empty. Both halves are checked: a table with a duplicate is misaligned,
        // and a table with the wrong number of holes is not this one.
        let mut seen = [false; NUM_POKEMON + 1];
        let mut holes = 0usize;
        for &d in &self.dex_order {
            if d == 0 {
                holes += 1;
                continue;
            }
            if d as usize > NUM_POKEMON || seen[d as usize] {
                return Err(bad(
                    "PokedexOrder",
                    format!("{d} is not a fresh Pokédex number 1..=151"),
                ));
            }
            seen[d as usize] = true;
        }
        let named = self.dex_order.len() - holes;
        if named != NUM_POKEMON {
            return Err(bad(
                "PokedexOrder",
                format!("{named} named entries, expected {NUM_POKEMON}"),
            ));
        }

        // Every `BaseStats` record self-identifies with its own index.
        for (i, b) in self.base_stats.iter().enumerate() {
            if b.dex as usize != i + 1 {
                return Err(bad(
                    "BaseStats",
                    format!(
                        "record {} says it is dex {} — the table is not aligned",
                        i + 1,
                        b.dex
                    ),
                ));
            }
            if b.hp == 0 || b.attack == 0 || b.speed == 0 {
                return Err(bad(
                    "BaseStats",
                    format!("dex {} has a zero base stat", b.dex),
                ));
            }
        }

        if self.chart.entries().len() < 20 {
            return Err(bad(
                "TypeEffects",
                format!(
                    "only {} rows — too short to be the chart",
                    self.chart.entries().len()
                ),
            ));
        }
        Ok(())
    }

    /// Data with no cartridge behind it: a neutral type chart, no names, and
    /// whatever species and moves the caller supplies.
    ///
    /// This is how every test in this crate runs. It exists so the engine's
    /// mechanics can be proved without a ROM on the machine — and so a failing
    /// test names an arithmetic bug rather than a missing file.
    pub fn synthetic(base_stats: Vec<BaseStats>, moves: Vec<MoveData>) -> GameData {
        // A dense fixture index space: index n is dex n. Real cartridges are not
        // dense (see [`NUM_POKEMON_INDEXES`]), which is exactly why the tests
        // that care about that use a real ROM and not this.
        let dex_order = (1..=base_stats.len() as u8).collect();
        GameData {
            dex_order,
            base_stats,
            mew: None,
            moves,
            species_names: Vec::new(),
            move_names: Vec::new(),
            chart: TypeChart::neutral(),
        }
    }

    /// Replace the type chart — for `synthetic` data, and for tests that want a
    /// specific matchup without a cartridge.
    pub fn with_chart(mut self, chart: TypeChart) -> Self {
        self.chart = chart;
        self
    }

    pub fn chart(&self) -> &TypeChart {
        &self.chart
    }

    /// Base stats for an **internal index** (what WRAM and the wire carry), the
    /// conversion `GetMonHeader` performs.
    ///
    /// `GetMonHeader` (`home/pokemon.asm`) converts the internal index to a
    /// Pokédex number via `IndexToPokedex` and then indexes `BaseStats` — except
    /// for Mew, whose record it loads from `MewBaseStats`. Indexing `BaseStats`
    /// with the internal index directly is the classic Gen 1 reader bug and
    /// yields a different species.
    pub fn base_stats_for_index(&self, index: u8) -> Option<&BaseStats> {
        if index == MEW_INDEX {
            return self.mew.as_ref();
        }
        let dex = *self.dex_order.get(index.checked_sub(1)? as usize)?;
        if dex == 0 {
            // A hole in `PokedexOrder` — an internal index with no Pokémon.
            return None;
        }
        self.base_stats_for_dex(dex)
    }

    /// Base stats by Pokédex number, for callers that already have one.
    pub fn base_stats_for_dex(&self, dex: u8) -> Option<&BaseStats> {
        // Dex 151 is Mew, and it is not in the table.
        if dex as usize == NUM_POKEMON {
            return self.mew.as_ref();
        }
        self.base_stats.get(dex.checked_sub(1)? as usize)
    }

    /// A move by id (1-based). `None` for id 0, which is `NO_MOVE`.
    pub fn move_data(&self, id: u8) -> Option<&MoveData> {
        self.moves.get(id.checked_sub(1)? as usize)
    }

    /// The species name the cartridge would print, if names were loaded.
    pub fn species_name(&self, index: u8) -> Option<&str> {
        self.species_names
            .get(index.checked_sub(1)? as usize)
            .map(String::as_str)
    }

    pub fn move_name(&self, id: u8) -> Option<&str> {
        self.move_names
            .get(id.checked_sub(1)? as usize)
            .map(String::as_str)
    }

    pub fn moves(&self) -> &[MoveData] {
        &self.moves
    }
}

fn read<'a>(
    rom: &'a [u8],
    at: usize,
    len: usize,
    table: &'static str,
) -> Result<&'a [u8], DataError> {
    rom.get(at..at + len).ok_or(DataError::TooShort {
        table,
        needed: at + len,
        len: rom.len(),
    })
}

fn decode_base_stats(r: &[u8]) -> BaseStats {
    BaseStats {
        dex: r[0],
        hp: r[1],
        attack: r[2],
        defense: r[3],
        speed: r[4],
        special: r[5],
        type1: Type(r[6]),
        type2: Type(r[7]),
        catch_rate: r[8],
        base_exp: r[9],
    }
}

/// Find and decode `TypeEffects`.
///
/// The signature is the table's own first four rows, and it must occur exactly
/// once — more than one hit means the image is not what we think it is, and
/// picking the first would be a guess. Rows run to the `db -1` terminator.
fn decode_type_chart(rom: &[u8]) -> Result<TypeChart, DataError> {
    let mut found: Option<usize> = None;
    let sig = &TYPE_EFFECTS_SIGNATURE;
    if rom.len() >= sig.len() {
        for i in 0..=rom.len() - sig.len() {
            if &rom[i..i + sig.len()] == sig {
                if found.is_some() {
                    // Two hits: refuse rather than pick one.
                    return Err(DataError::NoTypeChart);
                }
                found = Some(i);
            }
        }
    }
    let start = found.ok_or(DataError::NoTypeChart)?;

    let mut entries = Vec::new();
    let mut i = start;
    loop {
        match rom.get(i) {
            None => return Err(DataError::NoTypeChart),
            Some(0xFF) => break,
            Some(&atk) => {
                let def = *rom.get(i + 1).ok_or(DataError::NoTypeChart)?;
                let pct = *rom.get(i + 2).ok_or(DataError::NoTypeChart)?;
                entries.push((atk, def, pct));
                i += 3;
            }
        }
        // A chart longer than a few hundred rows means the terminator was
        // missed and we are reading code.
        if entries.len() > 512 {
            return Err(DataError::NoTypeChart);
        }
    }
    Ok(TypeChart::from_entries(entries))
}

/// Decode the game's own character map into ASCII.
///
/// `constants/charmap.asm`: `$80` is "A", `$A0` is "a", `$F6` is "0", and `$50`
/// terminates. Everything outside those runs is dropped rather than guessed at
/// — a name is for a human to read, and a wrong glyph is worse than a missing
/// one. (The full decoder, including the overlays a battle screen loads, is
/// `plugins/agent`'s; this is the small subset a data table needs.)
fn decode_text(bytes: &[u8]) -> String {
    let mut s = String::new();
    for &b in bytes {
        match b {
            0x50 | 0x00 => break,
            0x80..=0x99 => s.push((b - 0x80 + b'A') as char),
            0xA0..=0xB9 => s.push((b - 0xA0 + b'a') as char),
            0xF6..=0xFF => s.push((b - 0xF6 + b'0') as char),
            0x7F => s.push(' '),
            0xE1 => s.push_str("PK"),
            0xE2 => s.push_str("MN"),
            0xE3 => s.push('-'),
            0xE6 => s.push('?'),
            0xE7 => s.push('!'),
            0xEF => s.push('\u{2642}'), // ♂
            0xF5 => s.push('\u{2640}'), // ♀
            0xE8 => s.push('.'),
            _ => {}
        }
    }
    s
}

/// `MoveNames` is variable-length: each entry ends with `$50`.
fn decode_move_names(rom: &[u8]) -> Vec<String> {
    let mut names = Vec::with_capacity(NUM_MOVES);
    let mut i = MOVE_NAMES;
    for _ in 0..NUM_MOVES {
        let start = i;
        while matches!(rom.get(i), Some(&b) if b != 0x50) {
            i += 1;
            if i - start > 32 {
                break;
            }
        }
        names.push(decode_text(rom.get(start..i).unwrap_or(&[])));
        i += 1;
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ROM of zeroes is refused, not read as a cartridge with 151 identical
    /// Pokémon.
    #[test]
    fn an_image_that_is_not_a_cartridge_is_refused() {
        let rom = vec![0u8; 1024 * 1024];
        assert_eq!(
            GameData::load_from_rom(&rom).unwrap_err(),
            DataError::NoTypeChart
        );
    }

    #[test]
    fn a_short_image_is_refused_by_name() {
        let mut rom = vec![0u8; 0x40000];
        rom[0x3E474..0x3E474 + 12].copy_from_slice(&TYPE_EFFECTS_SIGNATURE);
        rom[0x3E474 + 12] = 0xFF;
        let err = GameData::load_from_rom(&rom).unwrap_err();
        // The chart is found, and the first table that runs past the end of the
        // image is named — a truncated cartridge produces an error that says
        // which table, not a battle model built from whatever was there.
        assert!(
            matches!(
                err,
                DataError::TooShort {
                    table: "PokedexOrder",
                    ..
                }
            ),
            "{err}"
        );
        assert!(err.to_string().contains("PokedexOrder"));
    }

    /// A duplicated signature means the image is not what we think it is, and
    /// taking the first hit would be a guess.
    #[test]
    fn two_type_charts_are_refused_rather_than_guessed_between() {
        let mut rom = vec![0u8; 0x80000];
        rom[0x1000..0x1000 + 12].copy_from_slice(&TYPE_EFFECTS_SIGNATURE);
        rom[0x2000..0x2000 + 12].copy_from_slice(&TYPE_EFFECTS_SIGNATURE);
        assert_eq!(decode_type_chart(&rom), Err(DataError::NoTypeChart));
    }

    /// **Three tables, three different lengths, and none of them is the species
    /// count.** `BaseStats` is 150 (Mew is elsewhere), `PokedexOrder` and
    /// `MonsterNames` are 190 (indexed by internal number, holes included).
    ///
    /// All three were wrong in the first version of this file and all three were
    /// caught by refusing a retail cartridge rather than by reasoning. That is
    /// the oracle rule earning its place three times in a row, and it is why the
    /// lengths are named constants with citations instead of `151` written out.
    #[test]
    fn the_three_tables_have_three_different_lengths() {
        assert_eq!(BASE_STATS_LEN, 150);
        assert_eq!(NUM_POKEMON_INDEXES, 190);
        assert_ne!(BASE_STATS_LEN, NUM_POKEMON);
        assert_ne!(NUM_POKEMON_INDEXES, NUM_POKEMON);
    }

    /// **`BaseStats` is 150 records, not 151** — its own
    /// `assert_table_length NUM_POKEMON - 1 ; discount Mew`.
    #[test]
    fn the_base_stats_table_discounts_mew() {
        assert_eq!(BASE_STATS_LEN, 150);
        assert_eq!(BASE_STATS_LEN, NUM_POKEMON - 1);
    }

    /// **Internal index numbers are not dense**, and `PokedexOrder` is 190
    /// entries with 39 zeroes rather than 151 dense ones.
    ///
    /// The first version of this module assumed 151, and a retail Blue image was
    /// refused because of it. The constant is checked here so the correction
    /// cannot be undone by someone re-deriving it from the species count.
    #[test]
    fn the_dex_order_table_is_longer_than_the_species_count() {
        assert_eq!(NUM_POKEMON, 151);
        assert_eq!(NUM_POKEMON_INDEXES, 190);
        assert!(NUM_POKEMON_INDEXES > NUM_POKEMON);
    }

    #[test]
    fn synthetic_data_needs_no_cartridge() {
        let d = GameData::synthetic(
            vec![BaseStats {
                dex: 1,
                hp: 45,
                attack: 49,
                defense: 49,
                speed: 45,
                special: 65,
                type1: Type::GRASS,
                type2: Type::POISON,
                catch_rate: 45,
                base_exp: 64,
            }],
            vec![MoveData {
                id: 1,
                animation: 1,
                effect: 0,
                power: 40,
                kind: Type::NORMAL,
                accuracy: 255,
                pp: 35,
            }],
        );
        assert_eq!(d.base_stats_for_dex(1).unwrap().hp, 45);
        assert_eq!(d.move_data(1).unwrap().power, 40);
        assert!(d.move_data(0).is_none());
    }

    #[test]
    fn a_species_with_two_equal_type_slots_is_single_typed() {
        let b = BaseStats {
            dex: 1,
            hp: 1,
            attack: 1,
            defense: 1,
            speed: 1,
            special: 1,
            type1: Type::NORMAL,
            type2: Type::NORMAL,
            catch_rate: 1,
            base_exp: 1,
        };
        assert_eq!(b.types(), (Type::NORMAL, None));
    }
}

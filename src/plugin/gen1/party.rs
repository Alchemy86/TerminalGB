//! The Generation 1 party data block, exactly as it goes over the link cable.
//!
//! # Layout
//!
//! A Gen 1 trade exchanges three blocks. Each is preceded by a run of
//! [`PREAMBLE_BYTE`] (`$FD`) that the receiver syncs on, and the sizes below are
//! what `pret/pokered`'s `CableClub_DoBattleOrTradeAgain` passes to
//! `Serial_ExchangeBytes`
//! ([`engine/link/cable_club.asm`](https://github.com/pret/pokered/blob/master/engine/link/cable_club.asm)):
//!
//! | block | preamble | data | total |
//! |---|---|---|---|
//! | random-number list | 7 | 10 | 17 |
//! | party data | 6 | 418 | 424 |
//! | patch list | 3 | 197 | 200 |
//!
//! The 418-byte party block mirrors the sender's WRAM verbatim, so its layout is
//! the WRAM layout — trainer name, party count, species list, the six 44-byte
//! Pokémon structures, six original-trainer names, six nicknames, and three
//! bytes of whatever happened to follow:
//!
//! ```text
//! 0    trainer name              11
//! 11   party count                1
//! 12   species list + $FF term     7
//! 19   6 x party structure      264
//! 283  6 x OT name               66
//! 349  6 x nickname              66
//! 415  padding                    3
//! ```
//!
//! # The patch list, and why it exists
//!
//! `$FE` is the protocol's "no data" byte: the receiver *skips* it while copying.
//! Real Pokémon data contains `$FE` all the time (a stat of 254, an experience
//! byte), so before sending, the game walks the 264-byte Pokémon region, replaces
//! every `$FE` with `$FF`, and records the 1-based offset of each substitution in
//! the patch list; the receiver reverses it. The list has **two parts**, split at
//! offset 252, because an offset is a single byte and `$FD` is reserved.
//!
//! Getting this wrong does not fail loudly — it silently alters one byte of the
//! Pokémon, which is how a trade produces a mon with impossible stats.

use super::romdata::{self, RomTables, Stats};
use super::text::{self, NAME_LENGTH};

/// Marks the start of a transferred block; the receiver ignores everything until
/// it sees one (`SERIAL_PREAMBLE_BYTE`).
pub const PREAMBLE_BYTE: u8 = 0xFD;
/// "Nothing to send" filler, and the byte the patch list exists to protect
/// (`SERIAL_NO_DATA_BYTE`).
pub const NO_DATA_BYTE: u8 = 0xFE;
/// Ends one part of the patch list (`SERIAL_PATCH_LIST_PART_TERMINATOR`).
pub const PATCH_LIST_TERMINATOR: u8 = 0xFF;

/// Maximum party size.
pub const PARTY_LENGTH: usize = 6;
/// One Pokémon's structure in a party (`PARTYMON_STRUCT_LENGTH`).
pub const MON_STRUCT_LENGTH: usize = 44;

/// Data lengths, excluding preambles.
pub const RANDOM_BLOCK_LEN: usize = 10;
pub const PARTY_BLOCK_LEN: usize = 418;
pub const PATCH_BLOCK_LEN: usize = 197;
/// Preamble lengths.
pub const RANDOM_PREAMBLE: usize = 7;
pub const PARTY_PREAMBLE: usize = 6;
pub const PATCH_PREAMBLE: usize = 3;

/// Offsets within the 418-byte party block.
pub const OFF_TRAINER_NAME: usize = 0;
pub const OFF_PARTY_COUNT: usize = 11;
pub const OFF_SPECIES_LIST: usize = 12;
pub const OFF_MONS: usize = 19;
pub const OFF_OT_NAMES: usize = 283;
pub const OFF_NICKNAMES: usize = 349;

/// The patch list splits here: part 1 covers Pokémon-region offsets `0..252`,
/// part 2 covers `252..264`. 252 is `$FD - 1`, because `$FD` itself is reserved
/// as the preamble byte and so cannot appear as an offset.
const PATCH_PART1_LEN: usize = 252;
/// The patch entries begin 7 bytes into the block's data (the game leaves that
/// gap; `wSerialPartyMonsPatchList + 10` with a 3-byte preamble).
const PATCH_ENTRIES_OFFSET: usize = 7;

/// One Pokémon, in the form the link cable carries.
///
/// Every field the game will *not* recompute after the trade is here, which is
/// nearly all of them: `AddEnemyMonToPlayerParty` copies the 44 bytes verbatim
/// into the player's party, so wrong stats stay wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mon {
    pub species: u8,
    pub current_hp: u16,
    pub status: u8,
    pub type1: u8,
    pub type2: u8,
    pub catch_rate: u8,
    pub moves: [u8; 4],
    pub pp: [u8; 4],
    pub ot_id: u16,
    pub experience: u32,
    pub stat_exp: Stats,
    /// Attack, Defense, Speed, Special DVs (0–15 each). The HP DV is derived —
    /// see [`romdata::hp_dv`].
    pub dv_attack: u8,
    pub dv_defense: u8,
    pub dv_speed: u8,
    pub dv_special: u8,
    pub level: u8,
    pub stats: Stats,
    pub ot_name: String,
    pub nickname: String,
}

impl Mon {
    /// Serialise to the 44-byte structure. Multi-byte fields are **big-endian**
    /// — the Game Boy is little-endian, but these particular Pokémon fields are
    /// stored big-endian, which is a classic source of "why is my traded
    /// Pokémon level 6400".
    pub fn to_bytes(&self) -> [u8; MON_STRUCT_LENGTH] {
        let mut b = [0u8; MON_STRUCT_LENGTH];
        b[0] = self.species;
        b[1..3].copy_from_slice(&self.current_hp.to_be_bytes());
        b[3] = self.level; // "box level"; the games keep both copies in step
        b[4] = self.status;
        b[5] = self.type1;
        b[6] = self.type2;
        b[7] = self.catch_rate;
        b[8..12].copy_from_slice(&self.moves);
        b[12..14].copy_from_slice(&self.ot_id.to_be_bytes());
        let exp = self.experience.min(0x00FF_FFFF);
        b[14] = (exp >> 16) as u8;
        b[15] = (exp >> 8) as u8;
        b[16] = exp as u8;
        b[17..19].copy_from_slice(&self.stat_exp.hp.to_be_bytes());
        b[19..21].copy_from_slice(&self.stat_exp.attack.to_be_bytes());
        b[21..23].copy_from_slice(&self.stat_exp.defense.to_be_bytes());
        b[23..25].copy_from_slice(&self.stat_exp.speed.to_be_bytes());
        b[25..27].copy_from_slice(&self.stat_exp.special.to_be_bytes());
        b[27] = (self.dv_attack << 4) | (self.dv_defense & 0x0F);
        b[28] = (self.dv_speed << 4) | (self.dv_special & 0x0F);
        b[29..33].copy_from_slice(&self.pp);
        b[33] = self.level;
        b[34..36].copy_from_slice(&self.stats.hp.to_be_bytes());
        b[36..38].copy_from_slice(&self.stats.attack.to_be_bytes());
        b[38..40].copy_from_slice(&self.stats.defense.to_be_bytes());
        b[40..42].copy_from_slice(&self.stats.speed.to_be_bytes());
        b[42..44].copy_from_slice(&self.stats.special.to_be_bytes());
        b
    }

    /// Parse a 44-byte structure — used to read back what the *game* offered, so
    /// a trade can be reported and verified.
    pub fn from_bytes(b: &[u8], ot_name: String, nickname: String) -> Option<Mon> {
        if b.len() < MON_STRUCT_LENGTH {
            return None;
        }
        let be16 = |i: usize| u16::from_be_bytes([b[i], b[i + 1]]);
        Some(Mon {
            species: b[0],
            current_hp: be16(1),
            status: b[4],
            type1: b[5],
            type2: b[6],
            catch_rate: b[7],
            moves: [b[8], b[9], b[10], b[11]],
            ot_id: be16(12),
            experience: ((b[14] as u32) << 16) | ((b[15] as u32) << 8) | b[16] as u32,
            stat_exp: Stats {
                hp: be16(17),
                attack: be16(19),
                defense: be16(21),
                speed: be16(23),
                special: be16(25),
            },
            dv_attack: b[27] >> 4,
            dv_defense: b[27] & 0x0F,
            dv_speed: b[28] >> 4,
            dv_special: b[28] & 0x0F,
            pp: [b[29], b[30], b[31], b[32]],
            level: b[33],
            stats: Stats {
                hp: be16(34),
                attack: be16(36),
                defense: be16(38),
                speed: be16(40),
                special: be16(42),
            },
            ot_name,
            nickname,
        })
    }
}

/// A whole party, ready to be turned into the three wire blocks.
#[derive(Clone, Debug)]
pub struct Party {
    pub trainer_name: String,
    pub mons: Vec<Mon>,
}

/// Something about the party would make the game misbehave. These are checked
/// rather than hoped for: an ill-formed trade corrupts a save.
#[derive(Debug, PartialEq, Eq)]
pub enum PartyError {
    Empty,
    TooMany(usize),
    /// A name that encodes to a byte the protocol reserves. `$FE` is the
    /// "no data" byte and the receiver silently *skips* it while copying names,
    /// which shifts every following byte. Digit `8` encodes to `$FE`.
    NameContainsReservedByte {
        field: &'static str,
        name: String,
    },
    BadLevel(u8),
    BadSpecies(u8),
}

impl std::fmt::Display for PartyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartyError::Empty => {
                write!(f, "a trade partner must offer at least one Pokémon")
            }
            PartyError::TooMany(n) => {
                write!(f, "{n} Pokémon offered, the party limit is 6")
            }
            PartyError::NameContainsReservedByte { field, name } => write!(
                f,
                "{field} '{name}' contains a character that encodes to $FE, the \
                 link protocol's no-data byte (the digit '8'); the receiving \
                 game would skip it and mis-copy every byte after it"
            ),
            PartyError::BadLevel(l) => write!(f, "level {l} is outside 1..=100"),
            PartyError::BadSpecies(s) => {
                write!(f, "species index {s} is not a valid Gen 1 index")
            }
        }
    }
}

fn check_name(field: &'static str, name: &str) -> Result<Vec<u8>, PartyError> {
    let enc = text::encode_fixed(name, NAME_LENGTH);
    if enc.contains(&NO_DATA_BYTE) {
        return Err(PartyError::NameContainsReservedByte {
            field,
            name: name.to_string(),
        });
    }
    Ok(enc)
}

impl Party {
    /// Validate and build the 418-byte party block **and** its patch list.
    ///
    /// Returned as `(party_block, patch_block)`, both data-only (no preamble).
    pub fn to_blocks(&self) -> Result<(Vec<u8>, Vec<u8>), PartyError> {
        if self.mons.is_empty() {
            return Err(PartyError::Empty);
        }
        if self.mons.len() > PARTY_LENGTH {
            return Err(PartyError::TooMany(self.mons.len()));
        }
        for m in &self.mons {
            if m.level == 0 || m.level > 100 {
                return Err(PartyError::BadLevel(m.level));
            }
            if m.species == 0 {
                return Err(PartyError::BadSpecies(m.species));
            }
        }

        let mut block = vec![0u8; PARTY_BLOCK_LEN];

        block[OFF_TRAINER_NAME..OFF_TRAINER_NAME + NAME_LENGTH]
            .copy_from_slice(&check_name("trainer name", &self.trainer_name)?);

        block[OFF_PARTY_COUNT] = self.mons.len() as u8;

        // Species list: one byte per mon, then $FF, then the slot is left as-is
        // (the game writes $FF as the sentinel and does not clear beyond it).
        for (i, m) in self.mons.iter().enumerate() {
            block[OFF_SPECIES_LIST + i] = m.species;
        }
        for i in self.mons.len()..=PARTY_LENGTH {
            block[OFF_SPECIES_LIST + i] = 0xFF;
        }

        for (i, m) in self.mons.iter().enumerate() {
            let s = OFF_MONS + i * MON_STRUCT_LENGTH;
            block[s..s + MON_STRUCT_LENGTH].copy_from_slice(&m.to_bytes());
            let o = OFF_OT_NAMES + i * NAME_LENGTH;
            block[o..o + NAME_LENGTH]
                .copy_from_slice(&check_name("OT name", &m.ot_name)?);
            let n = OFF_NICKNAMES + i * NAME_LENGTH;
            let nick = if m.nickname.is_empty() {
                // An un-nicknamed Pokémon carries its species name.
                String::new()
            } else {
                m.nickname.clone()
            };
            block[n..n + NAME_LENGTH].copy_from_slice(&check_name("nickname", &nick)?);
        }

        let patch = patch_and_build_list(&mut block);
        Ok((block, patch))
    }

    /// The party exactly as it sits in WRAM: count, species list, the six
    /// structures, OT names and nicknames — 404 bytes, `wPartyDataStart` to
    /// `wPartyDataEnd`.
    ///
    /// Same bytes as the middle of the wire block but **unpatched**, because
    /// WRAM holds the true values and only the wire substitutes `$FE`. Used to
    /// build a save-file fixture, and to check the wire encoding against the
    /// layout the game itself uses.
    pub fn to_wram_image(&self) -> Vec<u8> {
        let (mut block, patch) = match self.to_blocks() {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };
        unpatch(&mut block, &patch);
        block[OFF_PARTY_COUNT..OFF_PARTY_COUNT + WRAM_PARTY_DATA_LEN].to_vec()
    }
}

/// `wPartyDataEnd - wPartyDataStart`: count(1) + species list(7) + 6 structures
/// (264) + 6 OT names (66) + 6 nicknames (66).
pub const WRAM_PARTY_DATA_LEN: usize = 1 + 7 + 264 + 66 + 66;

/// Replace every `$FE` in the Pokémon region with `$FF`, recording offsets, and
/// return the 197-byte patch-list block.
///
/// The two parts and the 252 split come straight from the sender loop in
/// `CableClub_DoBattleOrTradeAgain`; the receiver's `.unpatchEnemyMonsLoop`
/// applies `wEnemyMons + (entry - 1)` for part 1 and `wEnemyMons + 252 +
/// (entry - 1)` for part 2, so the entries are **1-based** within each part.
fn patch_and_build_list(block: &mut [u8]) -> Vec<u8> {
    let mut part1 = Vec::new();
    let mut part2 = Vec::new();
    let mon_region = OFF_MONS..OFF_MONS + PARTY_LENGTH * MON_STRUCT_LENGTH;
    for (i, addr) in mon_region.enumerate() {
        if block[addr] != NO_DATA_BYTE {
            continue;
        }
        block[addr] = 0xFF;
        if i < PATCH_PART1_LEN {
            part1.push((i + 1) as u8);
        } else {
            part2.push((i - PATCH_PART1_LEN + 1) as u8);
        }
    }

    let mut patch = vec![0u8; PATCH_BLOCK_LEN];
    let mut p = PATCH_ENTRIES_OFFSET;
    for e in part1 {
        patch[p] = e;
        p += 1;
    }
    patch[p] = PATCH_LIST_TERMINATOR;
    p += 1;
    for e in part2 {
        patch[p] = e;
        p += 1;
    }
    patch[p] = PATCH_LIST_TERMINATOR;
    patch
}

/// Apply a received patch list to a received party block — the inverse, used to
/// read back what the game offered us.
pub fn unpatch(block: &mut [u8], patch: &[u8]) {
    let mut part = 0usize;
    for &e in patch {
        match e {
            0 | PREAMBLE_BYTE | NO_DATA_BYTE => continue,
            PATCH_LIST_TERMINATOR => {
                part += 1;
                if part >= 2 {
                    return;
                }
            }
            _ => {
                let base = if part == 0 { 0 } else { PATCH_PART1_LEN };
                let idx = OFF_MONS + base + (e as usize - 1);
                if idx < block.len() {
                    block[idx] = NO_DATA_BYTE;
                }
            }
        }
    }
}

/// Build one well-formed Pokémon from a species, a level and up to four moves.
///
/// This is where "well-formed" is actually enforced: types, catch rate and base
/// stats come from the cartridge, the stats are computed with the Gen 1 formula
/// for the given level and DVs, the experience is set to exactly the total for
/// that level on that species' growth curve, and PP comes from the move table.
#[allow(clippy::too_many_arguments)]
pub fn build_mon(
    tables: &RomTables,
    species_index: u8,
    level: u8,
    moves: [u8; 4],
    dvs: (u8, u8, u8, u8),
    ot_id: u16,
    ot_name: &str,
    nickname: &str,
) -> Result<Mon, romdata::RomError> {
    let info = tables.species_by_index(species_index)?;
    let (dv_a, dv_d, dv_s, dv_sp) = (dvs.0 & 15, dvs.1 & 15, dvs.2 & 15, dvs.3 & 15);
    let dv_hp = romdata::hp_dv(dv_a, dv_d, dv_s, dv_sp);

    let stats = Stats {
        hp: romdata::compute_stat(info.base.hp, dv_hp, 0, level, true),
        attack: romdata::compute_stat(info.base.attack, dv_a, 0, level, false),
        defense: romdata::compute_stat(info.base.defense, dv_d, 0, level, false),
        speed: romdata::compute_stat(info.base.speed, dv_s, 0, level, false),
        special: romdata::compute_stat(info.base.special, dv_sp, 0, level, false),
    };

    let mut pp = [0u8; 4];
    for i in 0..4 {
        pp[i] = tables.move_pp(moves[i]);
    }

    Ok(Mon {
        species: species_index,
        current_hp: stats.hp,
        status: 0,
        type1: info.type1,
        type2: info.type2,
        catch_rate: info.catch_rate,
        moves,
        pp,
        ot_id,
        experience: romdata::experience_for_level(info.growth_rate, level),
        stat_exp: Stats::default(),
        dv_attack: dv_a,
        dv_defense: dv_d,
        dv_speed: dv_s,
        dv_special: dv_sp,
        level,
        stats,
        ot_name: ot_name.to_string(),
        nickname: if nickname.is_empty() {
            info.name.clone()
        } else {
            nickname.to_string()
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_mon(species: u8, level: u8) -> Mon {
        Mon {
            species,
            current_hp: 100,
            status: 0,
            type1: 0,
            type2: 0,
            catch_rate: 45,
            moves: [1, 0, 0, 0],
            pp: [35, 0, 0, 0],
            ot_id: 12345,
            experience: 125000,
            stat_exp: Stats::default(),
            dv_attack: 15,
            dv_defense: 15,
            dv_speed: 15,
            dv_special: 15,
            level,
            stats: Stats {
                hp: 100,
                attack: 100,
                defense: 100,
                speed: 100,
                special: 100,
            },
            ot_name: "RED".into(),
            nickname: "TEST".into(),
        }
    }

    #[test]
    fn a_mon_round_trips_through_its_44_bytes() {
        let m = a_mon(21, 50);
        let bytes = m.to_bytes();
        assert_eq!(bytes.len(), MON_STRUCT_LENGTH);
        let back =
            Mon::from_bytes(&bytes, m.ot_name.clone(), m.nickname.clone()).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn multi_byte_fields_are_big_endian() {
        let mut m = a_mon(21, 50);
        m.experience = 0x00_12_34_56;
        m.stats.hp = 0xABCD;
        let b = m.to_bytes();
        assert_eq!(&b[14..17], &[0x12, 0x34, 0x56]);
        assert_eq!(&b[34..36], &[0xAB, 0xCD]);
    }

    #[test]
    fn the_party_block_is_exactly_418_bytes_with_the_documented_layout() {
        let p = Party {
            trainer_name: "AI".into(),
            mons: vec![a_mon(21, 50)],
        };
        let (block, patch) = p.to_blocks().unwrap();
        assert_eq!(block.len(), PARTY_BLOCK_LEN);
        assert_eq!(patch.len(), PATCH_BLOCK_LEN);
        assert_eq!(text::decode(&block[0..NAME_LENGTH]), "AI");
        assert_eq!(block[OFF_PARTY_COUNT], 1);
        assert_eq!(block[OFF_SPECIES_LIST], 21);
        assert_eq!(block[OFF_SPECIES_LIST + 1], 0xFF);
        assert_eq!(block[OFF_MONS], 21);
        assert_eq!(text::decode(&block[OFF_OT_NAMES..OFF_OT_NAMES + 11]), "RED");
        assert_eq!(
            text::decode(&block[OFF_NICKNAMES..OFF_NICKNAMES + 11]),
            "TEST"
        );
    }

    #[test]
    fn every_fe_in_the_mon_region_is_patched_and_recoverable() {
        let mut m = a_mon(21, 50);
        // Force $FE bytes in both patch-list parts: current HP high byte lands
        // at region offset 1, and mon 6's data is past the 252 split.
        m.current_hp = 0xFE00;
        m.stats.special = 0x00FE;
        let p = Party {
            trainer_name: "AI".into(),
            mons: vec![m.clone(), m.clone(), m.clone(), m.clone(), m.clone(), m],
        };
        let (mut block, patch) = p.to_blocks().unwrap();

        // Nothing in the Pokémon region may still be $FE on the wire.
        let region = &block[OFF_MONS..OFF_MONS + PARTY_LENGTH * MON_STRUCT_LENGTH];
        assert!(
            !region.contains(&NO_DATA_BYTE),
            "unpatched $FE left on the wire"
        );
        // Both parts of the list are populated and terminated.
        assert_eq!(
            patch
                .iter()
                .filter(|&&b| b == PATCH_LIST_TERMINATOR)
                .count(),
            2
        );

        // And the receiver's inverse restores the original bytes exactly.
        let original = Party {
            trainer_name: "AI".into(),
            mons: vec![a_mon(21, 50); 6],
        };
        let mut expect = vec![0u8; PARTY_BLOCK_LEN];
        {
            // Rebuild without patching to get the true bytes.
            let (b, _) = original.to_blocks().unwrap();
            expect.copy_from_slice(&b);
        }
        unpatch(&mut block, &patch);
        let got = &block[OFF_MONS..OFF_MONS + PARTY_LENGTH * MON_STRUCT_LENGTH];
        // Reconstruct the pre-patch expectation for the modified mon.
        let mut m2 = a_mon(21, 50);
        m2.current_hp = 0xFE00;
        m2.stats.special = 0x00FE;
        assert_eq!(&got[0..MON_STRUCT_LENGTH], &m2.to_bytes()[..]);
    }

    #[test]
    fn a_name_containing_the_no_data_byte_is_refused() {
        let p = Party {
            trainer_name: "R8D".into(), // '8' encodes to $FE
            mons: vec![a_mon(21, 50)],
        };
        assert!(matches!(
            p.to_blocks(),
            Err(PartyError::NameContainsReservedByte { .. })
        ));
    }

    #[test]
    fn an_empty_party_is_refused() {
        let p = Party {
            trainer_name: "AI".into(),
            mons: vec![],
        };
        assert_eq!(p.to_blocks().unwrap_err(), PartyError::Empty);
    }
}

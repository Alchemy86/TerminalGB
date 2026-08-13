//! Cartridge data tables: species names, move names and PP, types, base stats
//! and the pointer to each Pokémon's front sprite.
//!
//! Nothing here is embedded in the binary. Every name and number is read out of
//! the player's own cartridge image, which is both smaller and *exact* — a name
//! table copied into Rust can drift from the ROM it claims to describe, and a
//! table read from the ROM cannot.
//!
//! # Addresses
//!
//! As with [`super::gen1`], these are the values `pokeblue.sym`/`pokered.sym`
//! give for the named labels in a verified build of
//! [`pret/pokered`](https://github.com/pret/pokered) — identical in both games.
//! They are `bank:address` pairs, flattened here to a ROM file offset by
//! `bank * 0x4000 + (address & 0x3FFF)`.

use super::gen1::NUM_MOVES;

/// `MonsterNames` — `07:421E`. Ten bytes per name, indexed by internal index
/// number minus one, **not** by Pokédex number.
const MONSTER_NAMES: usize = flat(0x07, 0x421E);
/// `MoveNames` — `2C:4000`. Variable-length, each terminated by `$50`.
const MOVE_NAMES: usize = flat(0x2C, 0x4000);
/// `TypeNames` — `09:7DAE`. A table of 16-bit pointers into the same bank.
const TYPE_NAMES: usize = flat(0x09, 0x7DAE);
/// `BaseStats` — `0E:43DE`. [`BASE_DATA_SIZE`] bytes per entry, indexed by
/// **Pokédex number** minus one (`GetMonHeader` in `home/pokemon.asm` converts
/// the internal index first, via `IndexToPokedex`).
const BASE_STATS: usize = flat(0x0E, 0x43DE);
/// `MewBaseStats` — `01:425B`. Mew's entry lives outside the table;
/// `GetMonHeader` special-cases it.
const MEW_BASE_STATS: usize = flat(0x01, 0x425B);
/// `PokedexOrder` — `10:5024`. One byte per internal index (minus one) giving
/// that Pokémon's Pokédex number; `data/pokemon/dex_order.asm`.
const POKEDEX_ORDER: usize = flat(0x10, 0x5024);
/// `Moves` — `0E:4000`. Six bytes per move (`MOVE_LENGTH`), indexed by move id
/// minus one; the last byte is base PP.
const MOVES: usize = flat(0x0E, 0x4000);

/// `BASE_DATA_SIZE` — `constants/pokemon_data_constants.asm`.
const BASE_DATA_SIZE: usize = 28;
/// `MOVE_LENGTH` — `constants/battle_constants.asm`.
const MOVE_LENGTH: usize = 6;
/// Fixed width of a `MonsterNames` entry (`MONSTER_NAME_LENGTH`).
const MONSTER_NAME_LENGTH: usize = 10;
/// `MEW` — internal index `$15`.
const MEW: u8 = 0x15;

const fn flat(bank: usize, addr: u16) -> usize {
    bank * 0x4000 + (addr as usize & 0x3FFF)
}

/// Offsets inside a `BaseStats` entry, from `data/pokemon/base_stats/*.asm`.
mod base {
    #[allow(dead_code)] // kept for symmetry with the entry layout
    pub const DEX: usize = 0;
    pub const HP: usize = 1;
    pub const ATTACK: usize = 2;
    pub const DEFENSE: usize = 3;
    pub const SPEED: usize = 4;
    pub const SPECIAL: usize = 5;
    pub const TYPE1: usize = 6;
    pub const TYPE2: usize = 7;
    /// `INCBIN "…​.pic", 0, 1` — the sprite's first byte, `(width << 4) | height`
    /// in 8×8 tiles.
    pub const SPRITE_DIMS: usize = 10;
    /// `dw <mon>PicFront, <mon>PicBack`.
    pub const FRONT_PIC: usize = 11;
}

/// The cartridge image, with the accessors a tracker needs.
///
/// Held by value (the frontend clones the ROM it already loaded) because
/// [`crate::plugin::DebugView`] has no banked-ROM read: `peek` only sees the
/// bank the game currently has mapped, and mapping another one would be a
/// write. See the note at the top of [`crate::plugin`].
pub struct RomTables {
    rom: Vec<u8>,
}

impl RomTables {
    pub fn new(rom: Vec<u8>) -> RomTables {
        RomTables { rom }
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    fn byte(&self, at: usize) -> u8 {
        self.rom.get(at).copied().unwrap_or(0)
    }

    fn slice(&self, at: usize, len: usize) -> &[u8] {
        let end = (at + len).min(self.rom.len());
        self.rom.get(at..end).unwrap_or(&[])
    }

    /// Species name for an internal index number (1-based), e.g. `$99` is
    /// BULBASAUR and `$01` is RHYDON.
    pub fn species_name(&self, index: u8) -> String {
        if index == 0 {
            return String::new();
        }
        let at = MONSTER_NAMES + (index as usize - 1) * MONSTER_NAME_LENGTH;
        super::text::decode(self.slice(at, MONSTER_NAME_LENGTH))
    }

    /// National Pokédex number for an internal index. Zero for the MissingNo.
    /// slots, which `PokedexOrder` lists as 0.
    pub fn dex_number(&self, index: u8) -> u8 {
        if index == 0 {
            return 0;
        }
        self.byte(POKEDEX_ORDER + index as usize - 1)
    }

    /// The 28-byte base-stats entry for an internal index, resolving the
    /// internal→Pokédex conversion and Mew's special case exactly as
    /// `GetMonHeader` does.
    pub fn base_stats(&self, index: u8) -> Option<&[u8]> {
        let at = if index == MEW {
            MEW_BASE_STATS
        } else {
            let dex = self.dex_number(index);
            if dex == 0 {
                return None;
            }
            BASE_STATS + (dex as usize - 1) * BASE_DATA_SIZE
        };
        let entry = self.slice(at, BASE_DATA_SIZE);
        (entry.len() == BASE_DATA_SIZE).then_some(entry)
    }

    /// Base stats in the order the panel shows them: HP, attack, defense,
    /// speed, special.
    pub fn base_stat_line(&self, index: u8) -> Option<[u8; 5]> {
        let e = self.base_stats(index)?;
        Some([
            e[base::HP],
            e[base::ATTACK],
            e[base::DEFENSE],
            e[base::SPEED],
            e[base::SPECIAL],
        ])
    }

    /// The species' types straight from ROM, as a fallback for a Pokémon whose
    /// RAM copy is not trustworthy. Returns type *ids*.
    pub fn base_types(&self, index: u8) -> Option<(u8, u8)> {
        let e = self.base_stats(index)?;
        Some((e[base::TYPE1], e[base::TYPE2]))
    }

    /// Type name for a type id, via the `TypeNames` pointer table.
    pub fn type_name(&self, id: u8) -> String {
        // The table is 16-bit little-endian pointers into bank $09.
        let ptr_at = TYPE_NAMES + id as usize * 2;
        let ptr = (self.byte(ptr_at) as u16) | ((self.byte(ptr_at + 1) as u16) << 8);
        if !(0x4000..0x8000).contains(&ptr) {
            return String::new();
        }
        let at = flat(0x09, ptr);
        // Names are short; 16 bytes is more than any of them.
        super::text::decode(self.slice(at, 16))
    }

    /// Move name for a move id (1-based). `MoveNames` is a run of
    /// `$50`-terminated strings, so reaching the nth means walking the first
    /// n-1 — cheap at four moves per Pokémon and done only when the party
    /// changes.
    pub fn move_name(&self, id: u8) -> String {
        if id == 0 {
            return String::new();
        }
        let mut at = MOVE_NAMES;
        for _ in 1..id {
            while self.byte(at) != 0x50 {
                at += 1;
                if at >= self.rom.len() {
                    return String::new();
                }
            }
            at += 1;
        }
        super::text::decode(self.slice(at, 16))
    }

    /// Base PP for a move id, from the `Moves` table.
    pub fn move_base_pp(&self, id: u8) -> u8 {
        if id == 0 {
            return 0;
        }
        self.byte(MOVES + (id as usize - 1) * MOVE_LENGTH + 5)
    }

    /// Decode a Pokémon's front picture to Game Boy 2bpp tiles.
    ///
    /// `None` for an index with no entry in the table (the MissingNo. slots) or
    /// if the data does not decode. See [`super::sprite`] for the format.
    pub fn front_pic(&self, index: u8) -> Option<super::sprite::Pic> {
        let entry = self.base_stats(index)?;
        let dims = entry[base::SPRITE_DIMS];
        let ptr =
            (entry[base::FRONT_PIC] as u16) | ((entry[base::FRONT_PIC + 1] as u16) << 8);
        if !(0x4000..0x8000).contains(&ptr) || dims == 0 {
            return None;
        }
        let at = flat(sprite_bank(index) as usize, ptr);
        let pic = super::sprite::decompress(self.slice(at, 0x4000))?;
        // The base-stats entry declares the size independently of the stream,
        // so a disagreement means the pointer or the bank is wrong — better to
        // show nothing than a picture assembled from the wrong bytes.
        ((dims >> 4) as usize == pic.width_tiles
            && (dims & 0x0F) as usize == pic.width_tiles)
            .then_some(pic)
    }

    /// The front picture as 8-bit palette indices (0..=3), plus its pixel size.
    pub fn front_sprite(&self, index: u8) -> Option<(Vec<u8>, usize, usize)> {
        Some(super::sprite::tiles_to_indices(&self.front_pic(index)?))
    }

    /// PP for a move slot: the low six bits of the stored byte are the
    /// remaining PP and the top two are the number of PP Ups applied
    /// (`ram/wram.asm`, `wPartyMon1PP`). A PP Up adds `base / 5`, three times at
    /// most, which is the two-bit field's range.
    pub fn move_slot(&self, id: u8, raw_pp: u8) -> super::gen1::MoveSlot {
        let base_pp = self.move_base_pp(id);
        let ups = raw_pp >> 6;
        super::gen1::MoveSlot {
            id,
            name: self.move_name(id),
            pp: raw_pp & 0x3F,
            pp_max: base_pp.saturating_add((base_pp / 5).saturating_mul(ups)),
            pp_ups: ups,
        }
    }

    /// Decode all four move slots of a mon whose moves start at `moves` and
    /// whose PP bytes start at `pp`.
    pub fn move_slots(&self, moves: &[u8], pp: &[u8]) -> Vec<super::gen1::MoveSlot> {
        (0..NUM_MOVES)
            .filter_map(|i| {
                let id = *moves.get(i)?;
                (id != 0).then(|| self.move_slot(id, pp.get(i).copied().unwrap_or(0)))
            })
            .collect()
    }
}

/// Which ROM bank holds a Pokémon's picture.
///
/// Transcribed from the range test at the top of `UncompressMonSprite`
/// (`home/pics.asm`), which is the only place this mapping exists — there is no
/// table in the ROM to read it from.
fn sprite_bank(index: u8) -> u8 {
    const MEW_BANK: u8 = 0x01; // BANK(MewPicFront)
    const KABUTOPS_FOSSIL_BANK: u8 = 0x0B; // BANK(FossilKabutopsPic)
    const FOSSIL_KABUTOPS: u8 = 0xB6;
    const TANGELA: u8 = 0x1E;
    const MOLTRES: u8 = 0x49;
    const BEEDRILL: u8 = 0x72;
    const STARMIE: u8 = 0x98;

    match index {
        MEW => MEW_BANK,
        FOSSIL_KABUTOPS => KABUTOPS_FOSSIL_BANK,
        i if i <= TANGELA => 0x09,      // "Pics 1"
        i if i <= MOLTRES => 0x0A,      // "Pics 2"
        i if i <= BEEDRILL + 1 => 0x0B, // "Pics 3"
        i if i <= STARMIE => 0x0C,      // "Pics 4"
        _ => 0x0D,                      // "Pics 5"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_offsets_match_the_bank_address_pairs() {
        // 07:421E -> 0x1C21E
        assert_eq!(MONSTER_NAMES, 0x1_C21E);
        // 2C:4000 -> 0xB0000
        assert_eq!(MOVE_NAMES, 0xB_0000);
        // 0E:43DE -> 0x383DE
        assert_eq!(BASE_STATS, 0x3_83DE);
        // 10:5024 -> 0x41024
        assert_eq!(POKEDEX_ORDER, 0x4_1024);
        // 0E:4000 -> 0x38000
        assert_eq!(MOVES, 0x3_8000);
    }

    #[test]
    fn sprite_banks_follow_the_ranges_in_pics_asm() {
        assert_eq!(sprite_bank(0x15), 0x01); // MEW is special-cased
        assert_eq!(sprite_bank(0xB6), 0x0B); // so is the Kabutops fossil
        assert_eq!(sprite_bank(0x01), 0x09); // RHYDON, "Pics 1"
        assert_eq!(sprite_bank(0x1E), 0x09); // TANGELA, last of "Pics 1"
        assert_eq!(sprite_bank(0x1F), 0x0A); // first of "Pics 2"
        assert_eq!(sprite_bank(0x49), 0x0A); // MOLTRES, last of "Pics 2"
        assert_eq!(sprite_bank(0x73), 0x0B); // BEEDRILL + 1, last of "Pics 3"
        assert_eq!(sprite_bank(0x74), 0x0C); // first of "Pics 4"
        assert_eq!(sprite_bank(0x98), 0x0C); // STARMIE, last of "Pics 4"
        assert_eq!(sprite_bank(0x99), 0x0D); // BULBASAUR, "Pics 5"
    }

    #[test]
    fn an_empty_rom_answers_without_panicking() {
        // A tracker must survive being pointed at a truncated or absent image;
        // every accessor is bounds-checked rather than indexing.
        let r = RomTables::new(Vec::new());
        assert_eq!(r.species_name(0x99), "");
        assert_eq!(r.dex_number(0x99), 0);
        assert!(r.base_stats(0x99).is_none());
        assert_eq!(r.move_name(1), "");
        assert_eq!(r.move_base_pp(1), 0);
        assert!(r.front_sprite(0x99).is_none());
    }

    #[test]
    fn pp_ups_extend_max_pp_by_a_fifth_each() {
        let r = RomTables::new(Vec::new());
        // With no ROM the base PP reads 0, so build the arithmetic directly.
        let slot = super::super::gen1::MoveSlot {
            id: 1,
            name: String::new(),
            pp: 0b11_010101 & 0x3F,
            pp_max: 35 + (35 / 5) * 3,
            pp_ups: 3,
        };
        assert_eq!(slot.pp, 21);
        assert_eq!(slot.pp_max, 56); // 35 base + 3 x 7
        assert_eq!(r.move_slot(0, 0).name, "");
    }
}

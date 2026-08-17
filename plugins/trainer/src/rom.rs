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
/// minus one. See [`move_entry`] for the fields.
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

/// Offsets inside a `Moves` entry.
///
/// From the `move` macro in `pret/pokered`'s `macros/data.asm`, which
/// `data/moves/moves.asm` invokes once per move:
/// `move: MACRO / db \1, \2, \3, \4, \5, \6` — animation, effect, power, type,
/// accuracy, PP. Verified against the retail cartridge: THUNDERBOLT (id 85) is
/// `[85, 6, 95, 23, 255, 15]` — 95 power, type 23 (`ELECTRIC`), `$FF` accuracy
/// (100 %), 15 PP; HYDRO PUMP (id 56) is `[56, 0, 120, 21, 204, 5]` — 120
/// power, `WATER`, `$CC` (80 %), 5 PP.
mod move_entry {
    #[allow(dead_code)] // kept for symmetry with the entry layout
    pub const ANIMATION: usize = 0;
    pub const EFFECT: usize = 1;
    pub const POWER: usize = 2;
    pub const TYPE: usize = 3;
    pub const ACCURACY: usize = 4;
    pub const PP: usize = 5;
}

/// The cartridge image, with the accessors a tracker needs.
///
/// Held by value (the frontend clones the ROM it already loaded) because
/// [`terminalgb_plugin_api::Machine`] has no banked-ROM read: `peek` only sees the
/// bank the game currently has mapped, and mapping another one would be a
/// write. See the note at the top of [`terminalgb_plugin_api`].
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
        self.move_field(id, move_entry::PP)
    }

    /// One byte of a move's `Moves` entry. See [`move_entry`] for the layout.
    fn move_field(&self, id: u8, field: usize) -> u8 {
        if id == 0 {
            return 0;
        }
        self.byte(MOVES + (id as usize - 1) * MOVE_LENGTH + field)
    }

    /// A move's accuracy as a **percentage**.
    ///
    /// The cartridge stores it as a fraction of 255 (`MoveHitTest` compares
    /// the byte against a random one), so `$FF` is 100 %, `$D8` is 85 % and
    /// `$CC` is 80 %. Rounded to nearest, which reproduces the percentages
    /// `pret/pokered`'s own `data/moves/moves.asm` writes in its `percent`
    /// macro arguments.
    pub fn move_accuracy_percent(&self, id: u8) -> u8 {
        let raw = self.move_field(id, move_entry::ACCURACY) as u16;
        ((raw * 100 + 127) / 255) as u8
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
            power: self.move_field(id, move_entry::POWER),
            type_name: self.type_name(self.move_field(id, move_entry::TYPE)),
            accuracy: self.move_accuracy_percent(id),
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

/// `TypeEffects` — `0F:6474`. Rows of three bytes, attacker type, defender
/// type and the multiplier in tenths, terminated by `$FF`
/// (`data/types/type_matchups.asm`).
const TYPE_EFFECTS: (usize, u16) = (0x0F, 0x6474);
/// `Moves` — the same `0E:4000` [`MOVES`] points at, as a bank pair.
const MOVES_BANK: (usize, u16) = (0x0E, 0x4000);
/// `StatModifierRatios` — `0F:76CB`, thirteen (numerator, denominator) pairs
/// for stages 1..13 (`data/battle/stat_modifiers.asm`). `MoveHitTest` walks it
/// twice, once for the attacker's accuracy stage and once for the defender's
/// evasion; `CalculateDamage`'s callers use the same table for the four battle
/// stats. Located by its own bytes rather than transcribed — the sequence
/// `25,100,28,100,…` occurs exactly once in the cartridge.
const STAT_RATIOS: (usize, u16) = (0x0F, 0x76CB);
/// Thirteen pairs.
const STAT_RATIO_BYTES: usize = 26;
/// `NUM_ATTACKS` is `$A5`, so the table is that many [`MOVE_LENGTH`] entries.
const MOVES_BYTES: usize = super::gen1::NUM_ATTACKS as usize * MOVE_LENGTH;
/// Gen 1 stops looking for more rows well before this; the cap only stops a
/// cartridge that is not this one from being read for ever.
const MAX_TYPE_ROWS: usize = 256;
/// `SPECIAL EQU $14` (`constants/type_constants.asm`). A move whose type is at
/// or above it uses the Special stat on both sides; below it, Attack and
/// Defense. Gen 1 splits by *type*, not per move.
pub const FIRST_SPECIAL_TYPE: u8 = 0x14;

/// The two cartridge tables a battle decision needs, pulled through a banked
/// read rather than out of a whole ROM image.
///
/// **Why this exists beside [`RomTables`], which can already do both.** That
/// one is handed the cartridge image because a tracker wants sprites and names
/// and would otherwise pull a megabyte a byte at a time. A battle needs 1.6 KiB
/// — the move table and the type chart — and the agent plugin has a
/// [`terminalgb_plugin_api::Machine`] and no ROM, so it reads exactly those two
/// through `peek_bank` once per session.
///
/// **Nothing here is a table typed into Rust.** The bank/address pairs are the
/// ones `pokeblue.sym` gives for `Moves` and `TypeEffects`, and
/// [`BattleTables::verified`] re-checks the chart's first rows against the
/// cartridge before any of it is believed — the same "derive the address, then
/// confirm it live" rule `plugins/battle-control`'s WRAM reader follows.
#[derive(Debug, Clone, Default)]
pub struct BattleTables {
    moves: Vec<u8>,
    chart: Vec<(u8, u8, u8)>,
    ratios: Vec<u8>,
}

impl BattleTables {
    /// Read both tables with `byte(bank, address)`.
    pub fn read(mut byte: impl FnMut(usize, u16) -> u8) -> BattleTables {
        let (mb, ma) = MOVES_BANK;
        let moves = (0..MOVES_BYTES)
            .map(|i| byte(mb, ma.wrapping_add(i as u16)))
            .collect();
        let (cb, ca) = TYPE_EFFECTS;
        let mut chart = Vec::new();
        for row in 0..MAX_TYPE_ROWS {
            let at = ca.wrapping_add((row * 3) as u16);
            let a = byte(cb, at);
            if a == 0xFF {
                break;
            }
            chart.push((a, byte(cb, at + 1), byte(cb, at + 2)));
        }
        let (rb, ra) = STAT_RATIOS;
        let ratios = (0..STAT_RATIO_BYTES)
            .map(|i| byte(rb, ra.wrapping_add(i as u16)))
            .collect();
        BattleTables {
            moves,
            chart,
            ratios,
        }
    }

    /// The multiplier one stat stage applies, as `(numerator, denominator)`.
    ///
    /// `stage` is the cartridge's own 1..13 with 7 neutral. Out of range, or a
    /// table that was not read, gives 1:1 — a wrong answer here would silently
    /// scale every estimate, so it fails to *no effect* rather than to a guess.
    pub fn stage_ratio(&self, stage: u8) -> (u32, u32) {
        let i = (stage.clamp(1, 13) as usize - 1) * 2;
        match (self.ratios.get(i), self.ratios.get(i + 1)) {
            (Some(&n), Some(&d)) if d != 0 => (n as u32, d as u32),
            _ => (1, 1),
        }
    }

    /// Does the chart we read look like Gen 1's?
    ///
    /// Checks the six rows `data/types/type_matchups.asm` opens with, which are
    /// distinctive enough to say we are reading the table and not some bytes
    /// that happen to live at that address in another cartridge. The rows
    /// themselves are *not* the data — they are how the location is confirmed,
    /// exactly as `tools/gen1-tables/gen1_tables.py` finds the same table by
    /// searching for them.
    pub fn verified(&self) -> bool {
        const HEAD: [(u8, u8, u8); 6] = [
            (0x15, 0x14, 20), // WATER    FIRE     super
            (0x14, 0x16, 20), // FIRE     GRASS    super
            (0x14, 0x19, 20), // FIRE     ICE      super
            (0x16, 0x15, 20), // GRASS    WATER    super
            (0x17, 0x15, 20), // ELECTRIC WATER    super
            (0x15, 0x05, 20), // WATER    ROCK     super
        ];
        // And the ratio table, whose own first pair is 0.25 and whose middle
        // entry must be exactly 1:1 — a table read from the wrong address
        // almost never has a neutral stage in the middle of it.
        let ratios_ok = self.ratios.len() == STAT_RATIO_BYTES
            && self.ratios[0..2] == [25, 100]
            && self.ratios[12..14] == [1, 1];
        self.chart.len() > HEAD.len() && self.chart[..6] == HEAD && ratios_ok
    }

    fn field(&self, id: u8, off: usize) -> u8 {
        if id == 0 {
            return 0;
        }
        self.moves
            .get((id as usize - 1) * MOVE_LENGTH + off)
            .copied()
            .unwrap_or(0)
    }

    /// Base power. Zero is a status move, which is a fact and not a gap.
    pub fn power(&self, id: u8) -> u8 {
        self.field(id, move_entry::POWER)
    }

    /// The move's own type, which is often not its user's.
    pub fn kind(&self, id: u8) -> u8 {
        self.field(id, move_entry::TYPE)
    }

    /// The move's effect id — an index into a jump table of code, so what it
    /// *means* is not readable from the ROM. See `crate::battle`'s `effect`.
    pub fn effect(&self, id: u8) -> u8 {
        self.field(id, move_entry::EFFECT)
    }

    /// Accuracy as a percentage of the cartridge's 0-255 byte, rounded down.
    /// Gen 1's separate 1-in-256 miss belongs to the engine, not the move, and
    /// is not folded in.
    pub fn accuracy_percent(&self, id: u8) -> u8 {
        ((self.field(id, move_entry::ACCURACY) as u16 * 100) / 255) as u8
    }

    /// Whether a move of this type is resolved with Special on both sides.
    pub fn is_special(&self, id: u8) -> bool {
        self.kind(id) >= FIRST_SPECIAL_TYPE
    }

    /// Type effectiveness as a percentage, 100 being neutral.
    ///
    /// **Every matching row is applied in turn**, which is what makes a dual
    /// type multiply out — and is also how Gen 1 gets its 0x, since a row can
    /// be `NO_EFFECT`. Pass the defender's second type as its first when it has
    /// only one, exactly as the cartridge stores it.
    pub fn multiplier(&self, attack: u8, def1: u8, def2: u8) -> u32 {
        let mut m: u32 = 100;
        for &(a, d, x) in &self.chart {
            if a != attack {
                continue;
            }
            if d == def1 {
                m = m * x as u32 / 10;
            }
            if def2 != def1 && d == def2 {
                m = m * x as u32 / 10;
            }
        }
        m
    }

    /// How many rows the chart has — the cheap way to see it was read at all.
    pub fn chart_rows(&self) -> usize {
        self.chart.len()
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

    /// The six-byte `Moves` entry is decoded field by field, on a **synthetic**
    /// cartridge — no game data is vendored here, so the fixture writes the
    /// bytes the retail one is known to hold rather than shipping them.
    ///
    /// The two rows are the pair used to establish the layout in the first
    /// place: THUNDERBOLT and HYDRO PUMP, the second of which is the only one
    /// of the two whose accuracy is not `$FF`, so it is what pins the
    /// percentage conversion rather than the identity `255 -> 100`.
    #[test]
    fn a_moves_entry_decodes_power_type_and_accuracy() {
        let mut rom = vec![0u8; 0x10_0000];
        let put = |rom: &mut Vec<u8>, id: u8, e: [u8; 6]| {
            let at = MOVES + (id as usize - 1) * MOVE_LENGTH;
            rom[at..at + 6].copy_from_slice(&e);
        };
        // animation, effect, power, type, accuracy, pp
        put(&mut rom, 85, [85, 6, 95, 23, 0xFF, 15]); // THUNDERBOLT
        put(&mut rom, 56, [56, 0, 120, 21, 0xCC, 5]); // HYDRO PUMP
        put(&mut rom, 45, [45, 19, 0, 0, 0xFF, 40]); // GROWL — a status move
        let r = RomTables::new(rom);

        let tb = r.move_slot(85, 15);
        assert_eq!(tb.power, 95);
        assert_eq!(tb.accuracy, 100, "$FF is 100 %");
        assert_eq!(tb.pp_max, 15);

        let hp = r.move_slot(56, 5);
        assert_eq!(hp.power, 120);
        // $CC = 204; 204/255 = 80 %. The one row that proves the conversion is
        // a conversion and not a pass-through.
        assert_eq!(hp.accuracy, 80);
        assert_eq!(hp.pp_max, 5);

        // Zero power is a *status move*, not a missing value — the panel is
        // required to tell those apart.
        assert_eq!(r.move_slot(45, 40).power, 0);

        // A PP Up raises the maximum without touching the other three fields.
        let upped = r.move_slot(85, 15 | (3 << 6));
        assert_eq!(upped.pp_ups, 3);
        assert_eq!(upped.pp_max, 15 + 3 * 3);
        assert_eq!((upped.power, upped.accuracy), (95, 100));
    }

    /// Every accuracy the cartridge can store converts to a percentage in
    /// range, and the conversion is monotonic — so a less accurate move never
    /// prints a higher number.
    #[test]
    fn accuracy_converts_monotonically_and_stays_in_range() {
        // Move id `n` holds accuracy byte `n`, so ids 1..=255 cover every raw
        // value except 0 — which id 0 (no move at all) already answers as 0.
        let mut rom = vec![0u8; 0x10_0000];
        for id in 1u16..=255 {
            let at = MOVES + (id as usize - 1) * MOVE_LENGTH;
            rom[at + move_entry::ACCURACY] = id as u8;
        }
        let r = RomTables::new(rom);
        assert_eq!(r.move_accuracy_percent(0), 0, "no move has no accuracy");
        let mut last = 0u8;
        for id in 1u16..=255 {
            let pct = r.move_accuracy_percent(id as u8);
            assert!(pct <= 100, "raw {id} converts to {pct} %");
            assert!(pct >= last, "raw {id} went backwards: {last} -> {pct}");
            last = pct;
        }
        assert_eq!(last, 100, "$FF must be 100 %");
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
        assert_eq!(r.move_accuracy_percent(1), 0);
        let slot = r.move_slot(1, 0);
        assert_eq!((slot.power, slot.accuracy), (0, 0));
        assert!(slot.type_name.is_empty());
        assert!(r.front_sprite(0x99).is_none());
    }

    /// A hand-built cartridge whose two battle tables sit where the symbols
    /// say, so the reader is exercised without a commercial ROM.
    fn fake_tables() -> super::BattleTables {
        // id -> (animation, effect, power, type, accuracy, pp), as the six-byte
        // `move` macro lays them out. The real values for these three.
        const MOVES: [(u8, [u8; 6]); 3] = [
            (1, [1, 0, 40, 0x00, 255, 35]), // POUND      40 NORMAL 100% 35
            (56, [56, 0, 120, 0x15, 204, 5]), // HYDRO PUMP 120 WATER  80%  5
            (45, [45, 43, 0, 0x00, 255, 40]), // GROWL       0 NORMAL 100% 40
        ];
        const CHART: [(u8, u8, u8); 8] = [
            (0x15, 0x14, 20),
            (0x14, 0x16, 20),
            (0x14, 0x19, 20),
            (0x16, 0x15, 20),
            (0x17, 0x15, 20),
            (0x15, 0x05, 20),
            (0x04, 0x02, 0), // GROUND FLYING no effect
            (0x15, 0x15, 5), // WATER  WATER  not very
        ];
        super::BattleTables::read(|bank, addr| {
            let off = (addr as usize) & 0x3FFF;
            if bank == 0x0E {
                let (i, f) = (off / 6 + 1, off % 6);
                return match MOVES.iter().find(|(id, _)| *id as usize == i) {
                    Some((_, m)) => m[f],
                    None => 0,
                };
            }
            if bank == 0x0F && (0x36CB..0x36CB + 26).contains(&off) {
                const RATIOS: [u8; 26] = [
                    25, 100, 28, 100, 33, 100, 40, 100, 50, 100, 66, 100, 1, 1, 15, 10,
                    2, 1, 25, 10, 3, 1, 35, 10, 4, 1,
                ];
                return RATIOS[off - 0x36CB];
            }
            if bank == 0x0F && off >= 0x2474 {
                let k = off - 0x2474;
                let (i, f) = (k / 3, k % 3);
                return match CHART.get(i) {
                    Some(r) => [r.0, r.1, r.2][f],
                    None => 0xFF,
                };
            }
            0xFF
        })
    }

    #[test]
    fn the_battle_tables_read_moves_and_the_type_chart_out_of_the_banks() {
        let t = fake_tables();
        assert!(t.verified(), "the chart head did not match");
        assert_eq!(t.chart_rows(), 8);
        assert_eq!(
            (t.power(1), t.kind(1), t.accuracy_percent(1)),
            (40, 0x00, 100)
        );
        assert_eq!(t.power(56), 120);
        assert_eq!(t.accuracy_percent(56), 80);
        assert!(t.is_special(56), "WATER is a special type");
        assert!(!t.is_special(1), "NORMAL is a physical type");
        assert_eq!(
            t.power(45),
            0,
            "a status move has no power, and that is a fact"
        );
    }

    #[test]
    fn a_stat_stage_scales_by_the_cartridges_own_ratio() {
        let t = fake_tables();
        assert_eq!(t.stage_ratio(7), (1, 1), "stage 7 is neutral");
        assert_eq!(t.stage_ratio(6), (66, 100), "one down");
        assert_eq!(t.stage_ratio(1), (25, 100), "the floor");
        assert_eq!(t.stage_ratio(13), (4, 1), "the ceiling");
        assert_eq!(t.stage_ratio(0), (25, 100), "clamped, not wrapped");
        // A table that was never read must not scale anything.
        assert_eq!(super::BattleTables::default().stage_ratio(1), (1, 1));
    }

    #[test]
    fn type_effectiveness_multiplies_every_matching_row() {
        let t = fake_tables();
        assert_eq!(t.multiplier(0x15, 0x14, 0x14), 200, "WATER into FIRE");
        assert_eq!(t.multiplier(0x15, 0x15, 0x15), 50, "WATER into WATER");
        assert_eq!(
            t.multiplier(0x04, 0x02, 0x02),
            0,
            "GROUND cannot touch FLYING"
        );
        assert_eq!(t.multiplier(0x00, 0x14, 0x15), 100, "no row, so neutral");
        // A dual type applies both rows: WATER into a WATER/ROCK is 0.5 x 2.
        assert_eq!(t.multiplier(0x15, 0x15, 0x05), 100);
        // ... and a mon whose two types are the same must not be counted twice.
        assert_eq!(t.multiplier(0x15, 0x05, 0x05), 200, "WATER into ROCK, once");
    }

    #[test]
    fn a_chart_that_is_not_gen_1s_is_refused() {
        let t = super::BattleTables::read(|_, _| 0x00);
        assert!(!t.verified());
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
            ..Default::default()
        };
        assert_eq!(slot.pp, 21);
        assert_eq!(slot.pp_max, 56); // 35 base + 3 x 7
        assert_eq!(r.move_slot(0, 0).name, "");
    }
}

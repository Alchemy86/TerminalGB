//! Everything the player actually owns — the party and all twelve PC boxes —
//! read out of the running game, so an *offer* (to the trade portal, or to
//! anything else) can only ever name a Pokémon that really exists in the
//! loaded save.
//!
//! That is the point of this module: a free-text "describe your Pokémon" form
//! is unverifiable, and the emulator is the one component that can read the
//! actual save. See `docs/portal.md` ("Offering from the emulator").
//!
//! # Where the data lives
//!
//! The party is WRAM (`wPartyCount`/`wPartyMons`, [`crate::gen1::addr`]). The
//! boxes are split ([`crate::gen1::boxes`], derived from `pokeblue.sym` /
//! `pokered.sym` builds byte-identical to retail): the **current** box lives
//! in WRAM at `wBoxDataStart` — deposits and withdrawals operate on it there —
//! and the other eleven live in SRAM banks 2 and 3, six per bank
//! (`ram/sram.asm`). Three semantics from `engine/menus/save.asm` shape the
//! read:
//!
//! 1. **Until the player's first box change, SRAM boxes do not exist.** The
//!    first `ChangeBox` runs `EmptyAllSRAMBoxes`; before that, banks 2/3 are
//!    uninitialised garbage. `wCurrentBoxNum` bit 7 (`BIT_HAS_CHANGED_BOXES`)
//!    records it, so a clear bit means "current box only, the rest are empty".
//! 2. **The current box's SRAM slot is deliberately empty**: switching boxes
//!    marks the slot just copied *out* of SRAM as empty (`CopyBoxToOrFromSRAM`
//!    writes count 0, species `$FF`), because WRAM is authoritative for it.
//! 3. **Every SRAM box carries a checksum** (`CalcCheckSum`: the complement of
//!    the block's byte sum, one byte per box in
//!    `sBank{2,3}IndividualBoxChecksums`), refreshed on every box change. A
//!    box that fails its checksum was not written by the game and is refused,
//!    not decoded.
//!
//! # Reading SRAM at all
//!
//! Cartridge RAM is normally *disabled* (the game gates it open only around
//! its own saves), so `Machine::peek` cannot see it; the read goes through
//! [`Machine::peek_sram_bank`], which reads the MBC's storage directly and
//! touches no register.
//!
//! # The two-phase shape
//!
//! [`read_raw`] takes only a [`Machine`]; [`decode`] takes only the
//! [`RomTables`]. They are separate because a frontend borrows the machine
//! mutably (`Gameboy::with_machine`) and the plugin's tables immutably, and
//! cannot hold both at once.

use crate::derived;
use crate::gen1::{
    self, addr, boxes, boxmon, party, Dvs, BOX_MON_SIZE, MONS_PER_BOX, NUM_BOXES,
    PARTY_MON_SIZE,
};
use crate::rom::RomTables;
use crate::text;
use terminalgb_plugin_api::Machine;

/// Where a Pokémon sits in the player's storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    /// Party slot, 0-based.
    Party(u8),
    /// PC box (0-based box number, 0-based slot). `current` marks the box the
    /// PC has open, whose live copy is WRAM.
    Box { number: u8, slot: u8, current: bool },
}

impl Location {
    /// Short label for a picker row: `P1`-`P6`, `B1`-`B12` (1-based, the way
    /// the game numbers them on screen).
    pub fn label(&self) -> String {
        match self {
            Location::Party(i) => format!("P{}", i + 1),
            Location::Box { number, .. } => format!("B{}", number + 1),
        }
    }
}

/// One Pokémon's raw bytes, exactly as stored.
#[derive(Debug, Clone)]
pub struct RawMon {
    pub location: Location,
    /// The `party_struct` (44 bytes) or `box_struct` (33 bytes).
    pub data: Vec<u8>,
    pub nick: [u8; 11],
    pub ot: [u8; 11],
    /// The species byte from the count-prefixed species *list* beside the
    /// structs — a second, independently written copy of the species, used
    /// the way the enemy-team reader uses it: disagreement means a stale or
    /// corrupt slot, and the slot is refused rather than trusted.
    pub listed_species: u8,
}

/// Phase 1 output: raw bytes, no ROM tables involved.
#[derive(Debug, Clone, Default)]
pub struct RawCollection {
    pub player_name: String,
    pub player_id: u16,
    pub mons: Vec<RawMon>,
    /// Anything that kept part of the collection from being read (an
    /// unreadable box, storage the cartridge does not expose). Honest gaps,
    /// shown to the player rather than silently dropped.
    pub notes: Vec<String>,
}

/// A Pokémon as the offer picker shows it, plus the verdict on whether it can
/// legally be offered.
#[derive(Debug, Clone)]
pub struct Offerable {
    pub location: Location,
    /// Internal species index (not the dex number).
    pub species: u8,
    pub species_name: String,
    pub dex: u8,
    pub nickname: String,
    pub level: u8,
    pub ot_name: String,
    pub ot_id: u16,
    /// Names of the known moves (empty slots dropped).
    pub moves: Vec<String>,
    pub dvs: Dvs,
    /// Would be shiny if traded on to Gen 2 — same rule as the panel's `★`
    /// ([`derived::shiny_if_traded`]).
    pub shiny_if_traded: bool,
    /// Why this Pokémon cannot be offered, or `None` when it can. The rule is
    /// the wire's and the receiving game's, not taste — see [`verdict`].
    pub refusal: Option<String>,
}

impl Offerable {
    pub fn dv_total(&self) -> u8 {
        self.dvs.attack
            + self.dvs.defense
            + self.dvs.speed
            + self.dvs.special
            + self.dvs.hp()
    }
}

/// Phase 2 output.
#[derive(Debug, Clone, Default)]
pub struct Collection {
    pub player_name: String,
    pub player_id: u16,
    pub entries: Vec<Offerable>,
    pub notes: Vec<String>,
}

/// `CalcCheckSum` (`engine/menus/save.asm`): the complement of the byte sum.
pub fn checksum(bytes: &[u8]) -> u8 {
    !bytes.iter().fold(0u8, |d, b| d.wrapping_add(*b))
}

fn peek(m: &mut Machine<'_>, a: u16) -> Result<u8, String> {
    m.peek(a)
        .ok_or_else(|| "memory read was not granted".to_string())
}

fn peek_vec(m: &mut Machine<'_>, base: u16, len: usize) -> Result<Vec<u8>, String> {
    (0..len as u16).map(|i| peek(m, base + i)).collect()
}

fn peek_name(m: &mut Machine<'_>, base: u16) -> Result<[u8; 11], String> {
    let v = peek_vec(m, base, gen1::NAME_LENGTH as usize)?;
    let mut out = [0u8; 11];
    out.copy_from_slice(&v);
    Ok(out)
}

/// Read the party and every box out of the running machine. `Err` when the
/// game is not actually running a save (the same "uninitialised WRAM" test the
/// trainer panel uses) or the machine refused the reads.
pub fn read_raw(m: &mut Machine<'_>) -> Result<RawCollection, String> {
    let name = peek_name(m, addr::PLAYER_NAME)?;
    let count = peek(m, addr::PARTY_COUNT)? as usize;
    if !text::looks_like_a_name(&name) || count > gen1::PARTY_LENGTH {
        return Err(
            "no save is loaded yet — the party area still holds uninitialised bytes"
                .to_string(),
        );
    }

    let mut out = RawCollection {
        player_name: text::decode(&name),
        player_id: m
            .peek_be16(addr::PLAYER_ID)
            .ok_or_else(|| "memory read was not granted".to_string())?,
        ..RawCollection::default()
    };

    // The party: count, species list, 44-byte structs, OTs, nicknames.
    for i in 0..count as u16 {
        out.mons.push(RawMon {
            location: Location::Party(i as u8),
            data: peek_vec(m, addr::PARTY_MONS + i * PARTY_MON_SIZE, 44)?,
            nick: peek_name(m, addr::PARTY_MON_NICKS + i * gen1::NAME_LENGTH)?,
            ot: peek_name(m, addr::PARTY_MON_OT + i * gen1::NAME_LENGTH)?,
            listed_species: peek(m, addr::PARTY_SPECIES + i)?,
        });
    }

    // The current box, live in WRAM.
    let box_num_byte = peek(m, boxes::CURRENT_BOX_NUM)?;
    let current_box = (box_num_byte & boxes::BOX_NUM_MASK) as usize % NUM_BOXES;
    read_wram_box(m, current_box as u8, &mut out)?;

    // The other eleven, in SRAM — which only holds boxes at all once the
    // player has changed boxes (see the module doc, rule 1).
    if box_num_byte & boxes::HAS_CHANGED_BOXES == 0 {
        return Ok(out);
    }
    for n in 0..NUM_BOXES {
        if n == current_box {
            continue; // its SRAM slot is deliberately empty; WRAM was read.
        }
        let (bank, base) = boxes::sram_slot(n);
        let Some(block) = read_sram_block(m, bank, base, boxes::BOX_BLOCK as usize)
        else {
            out.notes.push(
                "box storage could not be read from this cartridge — only the \
                 party and the open box are listed"
                    .to_string(),
            );
            return Ok(out);
        };
        let stored = m
            .peek_sram_bank(bank, boxes::INDIVIDUAL_CHECKSUMS + (n % 6) as u16)
            .unwrap_or(0);
        if checksum(&block) != stored {
            out.notes.push(format!(
                "box {} failed its own checksum and was not read",
                n + 1
            ));
            continue;
        }
        decode_box_block(&block, n as u8, false, &mut out);
    }
    Ok(out)
}

/// The current box, from its WRAM block.
fn read_wram_box(
    m: &mut Machine<'_>,
    number: u8,
    out: &mut RawCollection,
) -> Result<(), String> {
    let block = peek_vec(m, boxes::BOX_DATA_START, boxes::BOX_BLOCK as usize)?;
    decode_box_block(&block, number, true, out);
    Ok(())
}

fn read_sram_block(
    m: &mut Machine<'_>,
    bank: usize,
    base: u16,
    len: usize,
) -> Option<Vec<u8>> {
    (0..len as u16)
        .map(|i| m.peek_sram_bank(bank, base + i))
        .collect()
}

/// Split one `BOX_BLOCK` into its mons. Offsets are the block-relative form of
/// the symbols pinned in [`gen1::boxes`] (`the_box_block_walks_onto_its_symbols`).
fn decode_box_block(block: &[u8], number: u8, current: bool, out: &mut RawCollection) {
    let count = block[0] as usize;
    if count == 0 {
        return;
    }
    if count > MONS_PER_BOX {
        out.notes.push(format!(
            "box {} claims {count} Pokémon (the game stores at most \
             {MONS_PER_BOX}) and was not read",
            number + 1
        ));
        return;
    }
    let species_list = (boxes::BOX_SPECIES - boxes::BOX_DATA_START) as usize;
    let mons = (boxes::BOX_MONS - boxes::BOX_DATA_START) as usize;
    let ots = (boxes::BOX_MON_OT - boxes::BOX_DATA_START) as usize;
    let nicks = (boxes::BOX_MON_NICKS - boxes::BOX_DATA_START) as usize;
    let name_len = gen1::NAME_LENGTH as usize;
    for i in 0..count {
        let at = mons + i * BOX_MON_SIZE as usize;
        let mut nick = [0u8; 11];
        nick.copy_from_slice(&block[nicks + i * name_len..nicks + (i + 1) * name_len]);
        let mut ot = [0u8; 11];
        ot.copy_from_slice(&block[ots + i * name_len..ots + (i + 1) * name_len]);
        out.mons.push(RawMon {
            location: Location::Box {
                number,
                slot: i as u8,
                current,
            },
            data: block[at..at + BOX_MON_SIZE as usize].to_vec(),
            nick,
            ot,
            listed_species: block[species_list + i],
        });
    }
}

/// Resolve raw bytes into picker rows, attaching each one's verdict.
pub fn decode(raw: &RawCollection, tables: &RomTables) -> Collection {
    let entries = raw
        .mons
        .iter()
        .map(|r| {
            let species = r.data[boxmon::SPECIES as usize];
            let level = match r.location {
                // A party Pokémon's live level; box_struct has no party level
                // byte, so a boxed one shows the level it was deposited at —
                // the same number the PC's own list shows.
                Location::Party(_) => r.data[party::LEVEL as usize],
                Location::Box { .. } => r.data[boxmon::BOX_LEVEL as usize],
            };
            let dvs = Dvs::from_bytes(
                r.data[boxmon::DVS as usize],
                r.data[boxmon::DVS as usize + 1],
            );
            let move_ids: Vec<u8> =
                r.data[boxmon::MOVES as usize..boxmon::MOVES as usize + 4].to_vec();
            Offerable {
                location: r.location,
                species,
                species_name: tables.species_name(species),
                dex: tables.dex_number(species),
                nickname: text::decode(&r.nick),
                level,
                ot_name: text::decode(&r.ot),
                ot_id: u16::from_be_bytes([
                    r.data[boxmon::OT_ID as usize],
                    r.data[boxmon::OT_ID as usize + 1],
                ]),
                moves: move_ids
                    .iter()
                    .filter(|&&id| id != 0)
                    .map(|&id| tables.move_name(id))
                    .collect(),
                dvs,
                shiny_if_traded: derived::shiny_if_traded(dvs),
                refusal: verdict(r, species, level, &move_ids, tables),
            }
        })
        .collect();
    Collection {
        player_name: raw.player_name.clone(),
        player_id: raw.player_id,
        entries,
        notes: raw.notes.clone(),
    }
}

/// Why a stored Pokémon cannot be offered, or `None` when it can.
///
/// Gen 1's own Trade Center refuses nothing per-Pokémon (`TradeCenter_SelectMon`,
/// `engine/link/cable_club.asm`, goes straight from selection to the exchange —
/// there are no held items, no mail and no eggs in Gen 1), so every rule here
/// is either the **wire's** or the **receiving game's**:
///
/// * the link protocol's `$FE` is "no data" and is patched around, so a name
///   containing `$FE` — the digit `8` — cannot cross a link cable (the same
///   rule `gen1-link`'s party builder enforces when the *other* side arms);
/// * a name byte outside the printable charmap cannot be re-encoded by the
///   receiving side, which works from the decoded string;
/// * a species with no Pokédex entry (MissingNo. and friends) or a move id
///   above `STRUGGLE` is glitch data the receiving cartridge cannot resolve;
/// * a level outside 1-100 cannot be expressed in a trade spec.
fn verdict(
    r: &RawMon,
    species: u8,
    level: u8,
    move_ids: &[u8],
    tables: &RomTables,
) -> Option<String> {
    if r.listed_species != species {
        return Some(
            "the species list disagrees with the stored Pokémon — stale or \
             corrupt slot"
                .to_string(),
        );
    }
    if species == 0 || species == 0xFF || tables.dex_number(species) == 0 {
        return Some(
            "glitch data, not a real Pokémon — the receiving game cannot \
             resolve it"
                .to_string(),
        );
    }
    if level == 0 || level > 100 {
        return Some(format!("level {level} cannot be expressed in a trade"));
    }
    if let Err(e) = name_wire_safe(&r.nick, "nickname") {
        return Some(e);
    }
    if let Err(e) = name_wire_safe(&r.ot, "OT name") {
        return Some(e);
    }
    if move_ids.iter().all(|&id| id == 0) {
        return Some("knows no moves at all — not a well-formed Pokémon".to_string());
    }
    for &id in move_ids.iter().filter(|&&id| id != 0) {
        if id > gen1::NUM_ATTACKS || tables.move_name(id).is_empty() {
            return Some(format!(
                "knows glitch move ${id:02X} — the receiving game cannot \
                 resolve it"
            ));
        }
    }
    None
}

/// Whether a stored name can survive the trip: decoded to text here, carried
/// as text, and re-encoded into a party block by the receiving side.
///
/// The byte set is the printable subset of `constants/charmap.asm` that the
/// gen1-link encoder implements (`plugins/gen1-link/src/text.rs`), minus
/// `$FE`: `$FE` *is* printable (the digit `8`) but is also the link
/// protocol's "no data" byte, and `Serial_ExchangeBytes` skips it, so it can
/// never travel inside a party block.
fn name_wire_safe(bytes: &[u8], what: &str) -> Result<(), String> {
    for &b in bytes {
        if b == 0x50 {
            return Ok(()); // terminator
        }
        if b == 0xFE {
            return Err(format!(
                "{what} contains the digit 8, which is the link protocol's \
                 no-data byte ($FE) and cannot cross a trade"
            ));
        }
        let printable = matches!(
            b,
            0x80..=0x99      // A-Z
            | 0xA0..=0xB9    // a-z
            | 0xF6..=0xFD | 0xFF // 0-7, 9 ($FE excluded above)
            | 0x7F           // space
            | 0x9A..=0x9F    // ( ) : ; [ ]
            | 0xE0 | 0xE3 | 0xE6 | 0xE7 | 0xE8 // ' - ? ! .
            | 0xF3 | 0xF4    // / ,
            | 0xEF | 0xF5    // ♂ ♀
            | 0xBA // é
        );
        if !printable {
            return Err(format!(
                "{what} contains byte ${b:02X}, which has no glyph the \
                 receiving side could re-encode"
            ));
        }
    }
    // No terminator inside the field: the game always terminates a name, so
    // eleven printable bytes with no $50 is not a name the game wrote.
    Err(format!("{what} is not terminated — corrupt name field"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_api::{Capability, KeypadKey, MachineAccess};

    /// A machine that is nothing but memory: flat WRAM plus four 8 KiB
    /// cartridge-RAM banks, or no cartridge RAM at all.
    struct FakeMachine {
        mem: Vec<u8>,
        sram: Option<Vec<u8>>,
    }

    impl FakeMachine {
        fn new(with_sram: bool) -> FakeMachine {
            FakeMachine {
                mem: vec![0; 0x10000],
                sram: with_sram.then(|| vec![0; 4 * 0x2000]),
            }
        }

        fn write(&mut self, at: u16, bytes: &[u8]) {
            self.mem[at as usize..at as usize + bytes.len()].copy_from_slice(bytes);
        }

        fn sram_write(&mut self, bank: usize, at: u16, bytes: &[u8]) {
            let base = bank * 0x2000 + (at as usize & 0x1FFF);
            self.sram.as_mut().unwrap()[base..base + bytes.len()].copy_from_slice(bytes);
        }
    }

    impl MachineAccess for FakeMachine {
        fn peek(&mut self, address: u16) -> u8 {
            self.mem[address as usize]
        }
        fn poke(&mut self, address: u16, value: u8) {
            self.mem[address as usize] = value;
        }
        fn peek_bank(&mut self, _bank: usize, _address: u16) -> u8 {
            0xFF
        }
        fn peek_sram_bank(&mut self, bank: usize, address: u16) -> Option<u8> {
            let s = self.sram.as_ref()?;
            s.get(bank * 0x2000 + (address as usize & 0x1FFF)).copied()
        }
        fn rom_title(&self) -> String {
            "TEST".into()
        }
        fn global_checksum(&self) -> u16 {
            0
        }
        fn rom_bank(&self) -> usize {
            1
        }
        fn set_button(&mut self, _key: KeypadKey, _down: bool) {}
        fn queue_press(&mut self, _key: KeypadKey, _hold: u16, _gap: u16) {}
        fn input_idle(&self) -> bool {
            true
        }
        fn clear_input(&mut self) {}
        fn frame_rgba(&self) -> &[u8] {
            &[]
        }
        fn frame_png(&self) -> Vec<u8> {
            Vec::new()
        }
        fn report_fault(&mut self, _reason: &str) {}
        fn fault(&self) -> Option<&str> {
            None
        }
    }

    /// Encode ASCII A-Z/0-9 into an 11-byte terminated charmap name field.
    fn name(s: &str) -> [u8; 11] {
        let mut out = [0x50u8; 11];
        for (i, c) in s.chars().take(10).enumerate() {
            out[i] = match c {
                'A'..='Z' => 0x80 + (c as u8 - b'A'),
                '0'..='9' => 0xF6 + (c as u8 - b'0'),
                ' ' => 0x7F,
                _ => panic!("test name helper only does A-Z, 0-9, space"),
            };
        }
        out
    }

    /// A well-formed `box_struct` (33 bytes) for `species`, holding one move.
    fn box_mon(species: u8, level: u8, dvs: (u8, u8), mv: u8) -> Vec<u8> {
        let mut b = vec![0u8; BOX_MON_SIZE as usize];
        b[boxmon::SPECIES as usize] = species;
        b[boxmon::BOX_LEVEL as usize] = level;
        b[boxmon::MOVES as usize] = mv;
        b[boxmon::OT_ID as usize] = 0x30; // 0x3039 = 12345, big-endian
        b[boxmon::OT_ID as usize + 1] = 0x39;
        b[boxmon::DVS as usize] = dvs.0;
        b[boxmon::DVS as usize + 1] = dvs.1;
        b
    }

    /// The party form: the same box_struct plus a live level and stats.
    fn party_mon(species: u8, level: u8, dvs: (u8, u8), mv: u8) -> Vec<u8> {
        let mut b = box_mon(species, level.wrapping_sub(1), dvs, mv);
        b.resize(PARTY_MON_SIZE as usize, 0);
        // The live level differs from the stale BoxLevel on purpose, so a
        // test that reads the wrong one fails.
        b[party::LEVEL as usize] = level;
        b
    }

    /// Put one Pokémon in the party and one in the (never-yet-changed) current
    /// box.
    fn minimal_save(m: &mut FakeMachine) {
        let player = name("RED");
        m.write(addr::PLAYER_NAME, &player);
        m.write(addr::PLAYER_ID, &[0xAB, 0xCD]);
        m.poke(addr::PARTY_COUNT, 1);
        m.write(addr::PARTY_SPECIES, &[0x99, 0xFF]); // Bulbasaur's index
        m.write(addr::PARTY_MONS, &party_mon(0x99, 14, (0x9A, 0x88), 1));
        m.write(addr::PARTY_MON_NICKS, &name("BULBA"));
        m.write(addr::PARTY_MON_OT, &name("RED"));

        m.poke(boxes::CURRENT_BOX_NUM, 0); // box 1 open, never changed boxes
        m.poke(boxes::BOX_DATA_START, 1);
        m.write(boxes::BOX_SPECIES, &[0xB0, 0xFF]);
        m.write(boxes::BOX_MONS, &box_mon(0xB0, 9, (0x00, 0x00), 2));
        m.write(boxes::BOX_MON_NICKS, &name("CATERPIE"));
        m.write(boxes::BOX_MON_OT, &name("RED"));
    }

    /// A tables stand-in: an image with a name at `MonsterNames` for internal
    /// index 1, a dex number for it in `PokedexOrder`, and "POUND" first in
    /// `MoveNames`. Enough for [`decode`] to resolve real entries and refuse
    /// glitch ones — the real cartridge is exact in exactly the same shape.
    fn tables() -> RomTables {
        let mut rom = vec![0u8; 0x2C * 0x4000 + 0x100];
        let write = |rom: &mut Vec<u8>, at: usize, bytes: &[u8]| {
            rom[at..at + bytes.len()].copy_from_slice(bytes);
        };
        // MonsterNames 07:421E, 10 bytes per entry, indexed by internal-1.
        let names = 0x07 * 0x4000 + 0x021E;
        let mut entry = |rom: &mut Vec<u8>, index: usize, n: &str| {
            let mut b = [0x50u8; 10];
            for (i, c) in n.chars().enumerate() {
                b[i] = 0x80 + (c as u8 - b'A');
            }
            write(rom, names + (index - 1) * 10, &b);
        };
        entry(&mut rom, 0x99, "BULBASAUR");
        entry(&mut rom, 0xB0, "CATERPIE");
        // PokedexOrder 10:5024, indexed by internal-1; index $1F stays 0 (a
        // MissingNo. slot).
        let dex = 0x10 * 0x4000 + 0x1024;
        rom[dex + 0x99 - 1] = 1;
        rom[dex + 0xB0 - 1] = 10;
        // MoveNames 2C:4000: move 1 then move 2, $50-terminated runs.
        let mv = 0x2C * 0x4000;
        write(&mut rom, mv, &[0x8F, 0x8E, 0x94, 0x8D, 0x83, 0x50]); // POUND
        write(
            &mut rom,
            mv + 6,
            &[0x8A, 0x80, 0x91, 0x80, 0x93, 0x84, 0x50],
        ); // KARATE(ish)
        RomTables::new(rom)
    }

    fn read(m: &mut FakeMachine) -> RawCollection {
        let mut machine =
            Machine::new(m, Capability::OBSERVE.union(Capability::READ_MEMORY));
        read_raw(&mut machine).unwrap()
    }

    #[test]
    fn checksum_is_the_complement_of_the_byte_sum() {
        // CalcCheckSum (engine/menus/save.asm): add every byte, complement.
        assert_eq!(checksum(&[0, 0, 0]), 0xFF);
        assert_eq!(checksum(&[1, 2, 3]), !6u8);
        assert_eq!(checksum(&[0xFF, 0x01]), 0xFF);
    }

    #[test]
    fn before_a_save_loads_the_read_is_refused() {
        let mut m = FakeMachine::new(true);
        let mut machine =
            Machine::new(&mut m, Capability::OBSERVE.union(Capability::READ_MEMORY));
        assert!(read_raw(&mut machine).is_err());
    }

    #[test]
    fn party_and_open_box_are_listed_and_sram_is_never_touched_before_a_box_change() {
        // Rule 1 of the module doc: with BIT_HAS_CHANGED_BOXES clear, SRAM
        // banks 2/3 are uninitialised garbage and must not be read. The fake
        // has *no* SRAM at all, so touching it would surface as a note.
        let mut m = FakeMachine::new(false);
        minimal_save(&mut m);
        let raw = read(&mut m);
        assert_eq!(raw.player_name, "RED");
        assert_eq!(raw.player_id, 0xABCD);
        assert_eq!(raw.mons.len(), 2);
        assert_eq!(raw.mons[0].location, Location::Party(0));
        assert_eq!(
            raw.mons[1].location,
            Location::Box {
                number: 0,
                slot: 0,
                current: true
            }
        );
        assert!(raw.notes.is_empty(), "{:?}", raw.notes);
    }

    #[test]
    fn a_decoded_entry_carries_what_the_picker_and_the_offer_need() {
        let mut m = FakeMachine::new(false);
        minimal_save(&mut m);
        let c = decode(&read(&mut m), &tables());
        let e = &c.entries[0];
        assert_eq!(e.species_name, "BULBASAUR");
        assert_eq!(e.nickname, "BULBA");
        assert_eq!(e.level, 14, "a party entry reads the live level");
        assert_eq!(e.ot_name, "RED");
        assert_eq!(e.ot_id, 0x3039);
        assert_eq!(e.moves, vec!["POUND".to_string()]);
        assert_eq!(
            (e.dvs.attack, e.dvs.defense, e.dvs.speed, e.dvs.special),
            (9, 10, 8, 8)
        );
        assert_eq!(e.refusal, None, "a well-formed Pokémon is offerable");

        // The boxed one shows its stored BoxLevel, the number the PC's own
        // list shows.
        assert_eq!(c.entries[1].level, 9);
        assert_eq!(c.entries[1].species_name, "CATERPIE");
    }

    #[test]
    fn sram_boxes_need_their_own_checksum_to_be_believed() {
        let mut m = FakeMachine::new(true);
        minimal_save(&mut m);
        m.poke(boxes::CURRENT_BOX_NUM, boxes::HAS_CHANGED_BOXES); // box 1 open
                                                                  // Build box 2 (n = 1) in SRAM bank 2: one CATERPIE.
        let mut block = vec![0u8; boxes::BOX_BLOCK as usize];
        block[0] = 1;
        block[1] = 0xB0;
        block[2] = 0xFF;
        let mons = (boxes::BOX_MONS - boxes::BOX_DATA_START) as usize;
        block[mons..mons + 33].copy_from_slice(&box_mon(0xB0, 30, (0, 0), 1));
        let nicks = (boxes::BOX_MON_NICKS - boxes::BOX_DATA_START) as usize;
        block[nicks..nicks + 11].copy_from_slice(&name("WORM"));
        let ots = (boxes::BOX_MON_OT - boxes::BOX_DATA_START) as usize;
        block[ots..ots + 11].copy_from_slice(&name("RED"));
        let (bank, base) = boxes::sram_slot(1);
        let sum = checksum(&block);
        m.sram_write(bank, base, &block);
        m.sram_write(bank, boxes::INDIVIDUAL_CHECKSUMS + 1, &[sum]);
        // Box 3 (n = 2) holds the same bytes but a wrong checksum: refused.
        let (bank3, base3) = boxes::sram_slot(2);
        m.sram_write(bank3, base3, &block);
        m.sram_write(bank3, boxes::INDIVIDUAL_CHECKSUMS + 2, &[sum ^ 0xFF]);

        let raw = read(&mut m);
        let boxed: Vec<_> = raw
            .mons
            .iter()
            .filter_map(|r| match r.location {
                Location::Box { number, .. } => Some(number),
                _ => None,
            })
            .collect();
        assert!(boxed.contains(&1), "the checksummed box is listed");
        assert!(!boxed.contains(&2), "the corrupt box is not");
        assert!(
            raw.notes.iter().any(|n| n.contains("box 3")),
            "and the refusal is reported, not silent: {:?}",
            raw.notes
        );
    }

    #[test]
    fn a_cartridge_with_no_readable_sram_says_so_instead_of_inventing_boxes() {
        let mut m = FakeMachine::new(false);
        minimal_save(&mut m);
        m.poke(boxes::CURRENT_BOX_NUM, boxes::HAS_CHANGED_BOXES);
        let raw = read(&mut m);
        assert!(
            raw.notes.iter().any(|n| n.contains("could not be read")),
            "{:?}",
            raw.notes
        );
        // The party and the open box are still there.
        assert_eq!(raw.mons.len(), 2);
    }

    #[test]
    fn the_verdict_refuses_what_the_wire_or_the_receiver_cannot_carry() {
        let t = tables();
        let mut m = FakeMachine::new(false);
        minimal_save(&mut m);

        // The digit 8 is charmap $FE — the link protocol's no-data byte.
        m.write(addr::PARTY_MON_NICKS, &name("MEW8"));
        let c = decode(&read(&mut m), &t);
        let r = c.entries[0].refusal.as_deref().unwrap();
        assert!(r.contains("digit 8"), "{r}");

        // A glitch species (no dex entry) is refused.
        minimal_save(&mut m);
        m.poke(addr::PARTY_SPECIES, 0x1F);
        m.poke(addr::PARTY_MONS, 0x1F);
        let c = decode(&read(&mut m), &t);
        assert!(
            c.entries[0].refusal.as_deref().unwrap().contains("glitch"),
            "{:?}",
            c.entries[0].refusal
        );

        // Two independently-written species copies disagreeing: stale slot.
        minimal_save(&mut m);
        m.poke(addr::PARTY_SPECIES, 0xB0);
        let c = decode(&read(&mut m), &t);
        assert!(c.entries[0]
            .refusal
            .as_deref()
            .unwrap()
            .contains("disagrees"));

        // A move id above STRUGGLE is glitch data.
        minimal_save(&mut m);
        m.poke(addr::PARTY_MONS + party::MOVES, 0xF0);
        let c = decode(&read(&mut m), &t);
        assert!(c.entries[0].refusal.as_deref().unwrap().contains("move"));

        // An unterminated name field is not a name the game wrote.
        minimal_save(&mut m);
        m.write(addr::PARTY_MON_OT, &[0x80u8; 11]);
        let c = decode(&read(&mut m), &t);
        assert!(c.entries[0]
            .refusal
            .as_deref()
            .unwrap()
            .contains("not terminated"));
    }

    #[test]
    fn the_shiny_marker_is_the_trainer_panels_rule() {
        let mut m = FakeMachine::new(false);
        minimal_save(&mut m);
        // def/spd/spc = 10 and attack bit 1 set: the Gen 2 shiny spread.
        m.write(addr::PARTY_MONS + party::DVS, &[0xFA, 0xAA]);
        let c = decode(&read(&mut m), &tables());
        assert!(c.entries[0].shiny_if_traded);
        assert!(!c.entries[1].shiny_if_traded);
    }
}

//! Where a Generation 1 battle lives in memory — and how each address is
//! *proved* against the cartridge actually loaded.
//!
//! # Two independent derivations, and why both
//!
//! Reading memory that turns out to be the wrong address shows a wrong number.
//! **Writing** it corrupts a running game, and can end up in a save file. So
//! every address this module hands to a write goes through two derivations that
//! have no source in common:
//!
//! 1. **The disassembly.** [`pret/pokered`](https://github.com/pret/pokered)'s
//!    `ram/wram.asm` laid out from a symbol this project has already verified
//!    (see [`gameboy_plugin_trainer::gen1`], which was produced by building the
//!    disassembly at its pinned RGBDS and matching `roms.sha1`). Each walk below
//!    *ends* on a second already-verified symbol, so the walk checks itself.
//! 2. **The cartridge.** [`verify`] finds a short, distinctive run of machine
//!    code in the ROM image and reads the address straight out of its operand
//!    bytes. `ld [nn],a` is `EA lo hi`; `ld a,[nn]` is `FA lo hi`; `ldh` is
//!    `E0 n` / `F0 n`. If the byte at the operand is not the constant below, the
//!    plugin refuses to attach rather than writing somewhere it does not know.
//!
//! Derivation 2 is the one that survives contact with a ROM nobody has checked:
//! it needs no symbol file, no build, and no trust in this file's arithmetic.
//! It is the same idea [`gameboy_plugin_gen1_link::romdata`] uses for *tables*,
//! applied to *addresses*.
//!
//! # The walks
//!
//! Starting from `wIsInBattle` `$D057`, verified in [`gameboy_plugin_trainer::gen1`]:
//!
//! ```text
//! wIsInBattle           $D057  db                      (verified)
//! wPartyGainExpFlags    $D058  flag_array PARTY_LENGTH -> 1 byte
//! wCurOpponent          $D059  db                      (verified independently)
//! wBattleType           $D05A  db                      (verified independently)
//! wDamageMultipliers    $D05B  db
//! wLoneAttackNo         $D05C  db  (= wGymLeaderNo)
//! wTrainerNo            $D05D  db
//! ```
//!
//! And from `wEventFlags` `$D747` (verified in `testharness/trainer.rs`, whose
//! own walk lands on `wGrassRate` `$D887`):
//!
//! ```text
//! wEventFlags           $D747  flag_array NUM_EVENTS (2560) -> 320 bytes
//! wGrassRate            $D887  UNION start            (verified)
//! wLinkEnemyTrainerName $D887  ds NAME_LENGTH (11)
//!                       $D892  ds 1
//! wSerialEnemyDataBlock $D893  label only
//!                       $D893  ds 9
//! wEnemyPartyCount      $D89C  db
//! wEnemyPartySpecies    $D89D  ds PARTY_LENGTH + 1 (7)
//! wEnemyMons            $D8A4  6 x party_struct (264)
//! wEnemyMonOT           $D9AC  6 x NAME_LENGTH (66)
//! wEnemyMonNicks        $D9EE  6 x NAME_LENGTH (66)
//! wTrainerHeaderPtr     $DA30  dw
//!                       $DA32  ds 6
//! wOpponentAfterWrong…  $DA38  db
//! wCurMapScript         $DA39  db
//!                       $DA3A  ds 7
//! wPlayTimeHours        $DA41                          (verified)
//! ```
//!
//! The walk arrives exactly on the independently verified `$DA41`, which is
//! what makes every address between the two anchors checked rather than
//! assumed. `$D89C` and `$D12B` are *also* read out of the cartridge by
//! [`verify`], so those two are proved twice over.

use gameboy_plugin_api::Machine;

/// Addresses. All of these live in WRAM bank 0 or in HRAM, so a plain
/// [`Machine::peek`] reads them correctly whatever the game has banked.
pub mod addr {
    // ---- battle setup: the four bytes that start a fight -------------------

    /// `wIsInBattle` — `-1` lost, 0 none, 1 wild, 2 trainer.
    pub const IS_IN_BATTLE: u16 = 0xD057;
    /// `wCurOpponent` — **the trigger.** Nonzero makes the overworld loop start
    /// a battle (`home/overworld.asm`, `.checkForOpponent`). Below
    /// [`super::OPP_ID_OFFSET`] it is a species; at or above it, a trainer
    /// class plus that offset.
    pub const CUR_OPPONENT: u16 = 0xD059;
    /// `wBattleType` — 0 normal, 1 old man tutorial, 2 Safari Zone.
    pub const BATTLE_TYPE: u16 = 0xD05A;
    /// `wTrainerNo` — which roster of the class to use (`ReadTrainer`).
    pub const TRAINER_NO: u16 = 0xD05D;
    /// `wCurEnemyLevel` — the level a wild opponent is generated at.
    pub const CUR_ENEMY_LEVEL: u16 = 0xD127;
    /// `wLinkState` — nonzero makes `ReadTrainer` return immediately and
    /// `LoadEnemyMonData` read the enemy party instead of generating it. We
    /// never write it; we read it to refuse to fire during a link session.
    pub const LINK_STATE: u16 = 0xD12B;
    /// `wCurPartySpecies`, written by `InitOpponent`.
    pub const CUR_PARTY_SPECIES: u16 = 0xCF91;
    /// `wEnemyMonSpecies2` — the value `InitBattleCommon` actually branches on.
    pub const ENEMY_MON_SPECIES2: u16 = 0xCFD8;

    // ---- the player's party (`sMainData` mirror in WRAM) -------------------

    /// `wPartyCount`.
    pub const PARTY_COUNT: u16 = 0xD163;
    /// `wPartySpecies` — 6 species plus an `$FF` terminator.
    pub const PARTY_SPECIES: u16 = 0xD164;
    /// `wPartyMons` — six 44-byte `party_struct`s.
    pub const PARTY_MONS: u16 = 0xD16B;
    /// `wPartyMonOT` — six 11-byte original-trainer names.
    pub const PARTY_MON_OT: u16 = 0xD273;
    /// `wPartyMonNicks` — six 11-byte nicknames.
    pub const PARTY_MON_NICKS: u16 = 0xD2B5;
    /// One past the last byte of the party block, `wPartyDataEnd`.
    pub const PARTY_DATA_END: u16 = PARTY_MON_NICKS + 6 * super::NAME_LENGTH as u16;

    // ---- the opposing party (trainer battles) ------------------------------

    /// `wEnemyPartyCount`.
    pub const ENEMY_PARTY_COUNT: u16 = 0xD89C;
    /// `wEnemyPartySpecies`.
    pub const ENEMY_PARTY_SPECIES: u16 = 0xD89D;
    /// `wEnemyMons` — six 44-byte `party_struct`s, same layout as the player's.
    pub const ENEMY_MONS: u16 = 0xD8A4;
    /// `wEnemyMonOT`.
    pub const ENEMY_MON_OT: u16 = 0xD9AC;
    /// `wEnemyMonNicks`.
    pub const ENEMY_MON_NICKS: u16 = 0xD9EE;
    /// One past the last byte of the enemy party block.
    pub const ENEMY_PARTY_END: u16 = ENEMY_MON_NICKS + 6 * super::NAME_LENGTH as u16;

    // ---- the active combatants (28/29-byte `battle_struct`) ----------------

    /// `wEnemyMonNick`.
    pub const ENEMY_MON_NICK: u16 = 0xCFDA;
    /// `wEnemyMon` — the opponent as the battle engine sees it.
    pub const ENEMY_MON: u16 = 0xCFE5;
    /// `wBattleMon` — the player's active Pokémon, same struct.
    pub const BATTLE_MON: u16 = 0xD014;

    // ---- reading the game's own screen -------------------------------------

    /// `wTileMap` — 20x18 charmap codes. Gen 1 loads its font so a tile id *is*
    /// the character it draws, so this reads back as text.
    pub const TILE_MAP: u16 = 0xC3A0;
    pub const TILE_MAP_WIDTH: usize = 20;
    pub const TILE_MAP_HEIGHT: usize = 18;

    // ---- the random number generator ---------------------------------------

    /// `hRandomAdd` — the high byte of the 16-bit seed `Random_` advances.
    pub const RANDOM_ADD: u16 = 0xFFD3;
    /// `hRandomSub` — the low byte.
    pub const RANDOM_SUB: u16 = 0xFFD4;
}

/// `OPP_ID_OFFSET` (`constants/trainer_constants.asm`): the boundary between
/// "this is a species" and "this is a trainer class".
pub const OPP_ID_OFFSET: u8 = 200;
/// `PARTYMON_STRUCT_LENGTH` — `macros/ram.asm`, `party_struct` = `box_struct`
/// plus level and five 16-bit stats.
pub const PARTY_STRUCT_LENGTH: usize = 44;
/// `battle_struct` — `macros/ram.asm`. **29 bytes, not 28**: species(1) HP(2)
/// partypos/boxlevel(1) status(1) types(2) catchrate(1) moves(4) DVs(2)
/// level(1) stats(10) PP(4).
pub const BATTLE_STRUCT_LENGTH: usize = 29;
/// `NAME_LENGTH` (`constants/text_constants.asm`).
pub const NAME_LENGTH: usize = 11;
/// `PARTY_LENGTH`.
pub const PARTY_LENGTH: usize = 6;

/// Field offsets inside a `battle_struct`, from `macros/ram.asm`.
pub mod battle_field {
    pub const SPECIES: usize = 0;
    pub const HP: usize = 1;
    pub const BOX_LEVEL: usize = 3;
    pub const STATUS: usize = 4;
    pub const TYPE1: usize = 5;
    pub const TYPE2: usize = 6;
    pub const CATCH_RATE: usize = 7;
    pub const MOVES: usize = 8;
    pub const DVS: usize = 12;
    pub const LEVEL: usize = 14;
    pub const MAX_HP: usize = 15;
    pub const ATTACK: usize = 17;
    pub const DEFENSE: usize = 19;
    pub const SPEED: usize = 21;
    pub const SPECIAL: usize = 23;
    pub const PP: usize = 25;
}

/// Anything about the loaded ROM that means this plugin must not write to it.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// A code signature this module needs was not in the ROM at all, or
    /// appeared more than once so no single reading is defensible.
    SignatureNotFound(&'static str),
    /// The signature was found but the address in its operand is not the one
    /// this module would have written to.
    AddressMismatch {
        what: &'static str,
        expected: u16,
        found: u16,
    },
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VerifyError::SignatureNotFound(s) => write!(
                f,
                "this ROM does not contain the '{s}' code sequence the battle \
                 plugin reads its addresses from — it is not an English \
                 Pokémon Red or Blue cartridge"
            ),
            VerifyError::AddressMismatch {
                what,
                expected,
                found,
            } => write!(
                f,
                "this ROM puts {what} at ${found:04X}, not ${expected:04X} — \
                 refusing to write to an address this build does not use"
            ),
        }
    }
}

/// Find the one place in `hay` where a `len`-byte window satisfies `shape`.
///
/// `None` covers both "not there" and "there twice": an ambiguous signature is
/// not evidence, and this is the whole basis for trusting an address that is
/// about to be written to. Note that a plain byte-run search is *not* good
/// enough — the fragments of Game Boy code that identify these routines are
/// short, and several of them (`ld a,$01 / ld [nn],a`, `ldh a,[rDIV] / ld b,a`)
/// occur hundreds of times in a megabyte of cartridge. Every signature here is
/// therefore matched as a whole shape including its operands' *positions*, and
/// only the operand values are read out.
fn find_shape(hay: &[u8], len: usize, shape: impl Fn(&[u8]) -> bool) -> Option<usize> {
    let mut found = None;
    if hay.len() < len {
        return None;
    }
    for i in 0..=hay.len() - len {
        if shape(&hay[i..i + len]) {
            if found.is_some() {
                return None;
            }
            found = Some(i);
        }
    }
    found
}

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

/// Read the addresses this plugin writes to straight out of the cartridge's
/// machine code, and refuse if any of them differ from the constants above.
///
/// Each signature is quoted with the `pokered` source it assembles from, so a
/// reviewer can check the bytes against the disassembly rather than trusting a
/// hex string.
pub fn verify(rom: &[u8]) -> Result<(), VerifyError> {
    // --- `InitOpponent` (engine/battle/core.asm) ---------------------------
    //     ld a, [wCurOpponent]      FA 59 D0
    //     ld [wCurPartySpecies], a  EA lo hi
    //     ld [wEnemyMonSpecies2], a EA lo hi
    //     jr InitBattleCommon       18 ..
    // Anchored on `wCurOpponent`, which `gameboy_plugin_trainer::gen1` already
    // verified, so this signature *starts* from known ground and reads two
    // unknowns out of it.
    let lo = (addr::CUR_OPPONENT & 0xFF) as u8;
    let hi = (addr::CUR_OPPONENT >> 8) as u8;
    let at = find_shape(rom, 11, |w| {
        w[0] == 0xFA
            && w[1] == lo
            && w[2] == hi
            && w[3] == 0xEA
            && w[6] == 0xEA
            && w[9] == 0x18
    })
    .ok_or(VerifyError::SignatureNotFound("InitOpponent"))?;
    check(
        "wCurPartySpecies",
        addr::CUR_PARTY_SPECIES,
        le16(rom, at + 4),
    )?;
    check(
        "wEnemyMonSpecies2",
        addr::ENEMY_MON_SPECIES2,
        le16(rom, at + 7),
    )?;

    // --- `ReadTrainer` (engine/battle/read_trainer_party.asm) --------------
    //     ld a, [wLinkState]        FA lo hi
    //     and a                     A7
    //     ret nz                    C0
    //     ld hl, wEnemyPartyCount   21 lo hi
    //     xor a                     AF
    //     ld [hli], a               22
    //     dec a                     3D
    //     ld [hl], a                77
    let head = find_shape(rom, 12, |w| {
        w[0] == 0xFA
            && w[3] == 0xA7
            && w[4] == 0xC0
            && w[5] == 0x21
            && w[8..12] == [0xAF, 0x22, 0x3D, 0x77]
    })
    .ok_or(VerifyError::SignatureNotFound("ReadTrainer"))?;
    check("wLinkState", addr::LINK_STATE, le16(rom, head + 1))?;
    check(
        "wEnemyPartyCount",
        addr::ENEMY_PARTY_COUNT,
        le16(rom, head + 6),
    )?;

    // Still inside `ReadTrainer`, within the next couple of hundred bytes:
    //     ld c,a / ld b,0 / add hl,bc / ld a,[hli] / ld h,[hl] / ld l,a
    //     ld a, [wTrainerNo]        FA lo hi
    // and then `.IterateTrainer`:
    //     ld a,[hli] / cp $FF / jr z,.. / ld [wCurEnemyLevel], a
    let rt = &rom[head..(head + 192).min(rom.len())];
    let no_at = find_shape(rt, 10, |w| {
        w[0..8] == [0x4F, 0x06, 0x00, 0x09, 0x2A, 0x66, 0x6F, 0xFA]
    })
    .ok_or(VerifyError::SignatureNotFound("ReadTrainer/wTrainerNo"))?;
    check("wTrainerNo", addr::TRAINER_NO, le16(rt, no_at + 8))?;
    let lvl_at = find_shape(rt, 8, |w| {
        w[0] == 0x2A && w[1] == 0xFE && w[2] == 0xFF && w[3] == 0x28 && w[5] == 0xEA
    })
    .ok_or(VerifyError::SignatureNotFound("ReadTrainer/wCurEnemyLevel"))?;
    check(
        "wCurEnemyLevel",
        addr::CUR_ENEMY_LEVEL,
        le16(rt, lvl_at + 6),
    )?;

    // --- `InitWildBattle` (engine/battle/core.asm) -------------------------
    //     ld a, $1                  3E 01
    //     ld [wIsInBattle], a       EA lo hi
    //     call LoadEnemyMonData     CD .. ..
    //
    // Searched **inside a window after `InitOpponent`**, not across the whole
    // cartridge: `ld a,$1 / ld [nn],a / call` is far too common a shape to be
    // unique in a megabyte, and the alternative — pinning the operand — would
    // make the check circular. `InitBattleCommon` falls through to
    // `InitWildBattle` a hundred-odd bytes below `InitOpponent`, so the window
    // is where the disassembly says the routine is.
    let window = &rom[at..(at + 256).min(rom.len())];
    let wild = find_shape(window, 6, |w| {
        w[0] == 0x3E && w[1] == 0x01 && w[2] == 0xEA && w[5] == 0xCD
    })
    .ok_or(VerifyError::SignatureNotFound("InitWildBattle"))?;
    check("wIsInBattle", addr::IS_IN_BATTLE, le16(window, wild + 3))?;

    // --- `Random_` (engine/math/random.asm) --------------------------------
    //     ldh a, [rDIV]     F0 04     ld b, a          47
    //     ldh a, [hRandomAdd] F0 n    adc b            88
    //     ldh [hRandomAdd], a E0 n
    //     ldh a, [rDIV]     F0 04     ld b, a          47
    //     ldh a, [hRandomSub] F0 n    sbc b            98
    //     ldh [hRandomSub], a E0 n    ret              C9
    //
    // `find_one` on the four-byte prefix would be ambiguous *by construction*
    // here — the routine contains `ldh a,[rDIV] / ld b,a / ldh a,[…]` twice, so
    // the prefix always matches at least twice inside the one routine it is
    // meant to identify. Every candidate is therefore checked against the whole
    // seventeen-byte shape first, and uniqueness is required of the survivors.
    let rnd = find_shape(rom, 17, |b| {
        b[0..4] == [0xF0, 0x04, 0x47, 0xF0]
            && b[5] == 0x88
            && b[6] == 0xE0
            && b[4] == b[7]
            && b[8..12] == [0xF0, 0x04, 0x47, 0xF0]
            && b[13] == 0x98
            && b[14] == 0xE0
            && b[12] == b[15]
            && b[16] == 0xC9
    });
    let hit = rnd.map(|i| (rom[i + 4], rom[i + 12]));
    let (add, sub) = hit.ok_or(VerifyError::SignatureNotFound("Random_"))?;
    check("hRandomAdd", addr::RANDOM_ADD, 0xFF00 | add as u16)?;
    check("hRandomSub", addr::RANDOM_SUB, 0xFF00 | sub as u16)?;

    Ok(())
}

fn check(what: &'static str, expected: u16, found: u16) -> Result<(), VerifyError> {
    if expected == found {
        Ok(())
    } else {
        Err(VerifyError::AddressMismatch {
            what,
            expected,
            found,
        })
    }
}

/// The addresses this plugin is allowed to write, as a single predicate.
///
/// **Cartridge RAM (`$A000-$BFFF`) is not in it.** That is the load-bearing
/// safety property of the whole plugin: a battery-backed save file is written
/// from cartridge RAM by the emulator’s battery module, so a plugin that never writes
/// cartridge RAM cannot put anything into a `.sav`, however wrong it is. The
/// party lives in WRAM; it only reaches the save file if the *player* then
/// walks into a Pokémon Center and saves.
pub fn is_writable(address: u16) -> bool {
    // WRAM (both banks and echo is excluded deliberately — we address WRAM0
    // directly) and HRAM. Not ROM, not VRAM, not cartridge RAM, not OAM, and
    // not the I/O range.
    (0xC000..=0xDFFF).contains(&address) || (0xFF80..=0xFFFE).contains(&address)
}

/// Read the game's own screen back as text.
///
/// Gen 1 writes character codes straight into `wTileMap` and loads its font so
/// that a tile id *is* the code of the glyph it draws, so twenty characters per
/// row of this buffer is exactly what a player sees. That is what lets a
/// cinematic script wait for `FIGHT` rather than for a frame number.
pub fn read_screen(machine: &mut Machine) -> String {
    let mut tiles = [0u8; addr::TILE_MAP_WIDTH * addr::TILE_MAP_HEIGHT];
    machine.peek_into(addr::TILE_MAP, &mut tiles);
    gameboy_plugin_trainer::text::decode_screen(&tiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The addresses this module shares with the (independently derived,
    /// symbol-file-verified) trainer view must agree. Two crews walking the
    /// same `wram.asm` from different anchors landing on the same byte is the
    /// cheapest possible check that neither walk slipped.
    #[test]
    fn the_shared_addresses_agree_with_the_trainer_views_table() {
        use gameboy_plugin_trainer::gen1::addr as t;
        assert_eq!(addr::IS_IN_BATTLE, t::IS_IN_BATTLE);
        assert_eq!(addr::CUR_OPPONENT, t::CUR_OPPONENT);
        assert_eq!(addr::BATTLE_TYPE, t::BATTLE_TYPE);
        assert_eq!(addr::PARTY_COUNT, t::PARTY_COUNT);
        assert_eq!(addr::PARTY_SPECIES, t::PARTY_SPECIES);
        assert_eq!(addr::PARTY_MONS, t::PARTY_MONS);
        assert_eq!(addr::PARTY_MON_OT, t::PARTY_MON_OT);
        assert_eq!(addr::PARTY_MON_NICKS, t::PARTY_MON_NICKS);
        assert_eq!(addr::ENEMY_MON, t::ENEMY_MON);
        assert_eq!(addr::ENEMY_MON_NICK, t::ENEMY_MON_NICK);
        assert_eq!(addr::BATTLE_MON, t::BATTLE_MON);
        // And the struct sizes the two modules use for the same structures.
        assert_eq!(
            PARTY_STRUCT_LENGTH as u16,
            gameboy_plugin_trainer::gen1::PARTY_MON_SIZE
        );
        assert_eq!(
            NAME_LENGTH as u16,
            gameboy_plugin_trainer::gen1::NAME_LENGTH
        );
    }

    /// The `wram.asm` walk documented at the top of this file, re-run as
    /// arithmetic. It starts on a verified symbol and must *end* on one.
    #[test]
    fn the_enemy_party_walk_lands_on_a_verified_symbol() {
        // wEventFlags $D747 + flag_array NUM_EVENTS(2560) = wGrassRate.
        let grass_rate = 0xD747u16 + (2560 + 7) / 8;
        assert_eq!(grass_rate, 0xD887, "wGrassRate");
        let mut p = grass_rate;
        p += NAME_LENGTH as u16; // wLinkEnemyTrainerName
        p += 1; // ds 1
        p += 9; // ds 9 after wSerialEnemyDataBlock
        assert_eq!(p, addr::ENEMY_PARTY_COUNT);
        p += 1;
        assert_eq!(p, addr::ENEMY_PARTY_SPECIES);
        p += PARTY_LENGTH as u16 + 1;
        assert_eq!(p, addr::ENEMY_MONS);
        p += (PARTY_LENGTH * PARTY_STRUCT_LENGTH) as u16;
        assert_eq!(p, addr::ENEMY_MON_OT);
        p += (PARTY_LENGTH * NAME_LENGTH) as u16;
        assert_eq!(p, addr::ENEMY_MON_NICKS);
        p += (PARTY_LENGTH * NAME_LENGTH) as u16;
        p += 2; // wTrainerHeaderPtr
        p += 6; // ds 6
        p += 1; // wOpponentAfterWrongAnswer
        p += 1; // wCurMapScript
        p += 7; // ds 7
        assert_eq!(
            p, 0xDA41,
            "the walk must land on wPlayTimeHours, which is verified elsewhere"
        );
    }

    #[test]
    fn the_battle_setup_walk_lands_on_trainer_no() {
        let mut p = addr::IS_IN_BATTLE;
        p += 1; // wPartyGainExpFlags: flag_array PARTY_LENGTH = 1 byte
        p += 1;
        assert_eq!(p, addr::CUR_OPPONENT);
        p += 1;
        assert_eq!(p, addr::BATTLE_TYPE);
        p += 1; // wDamageMultipliers
        p += 1; // wLoneAttackNo / wGymLeaderNo
        p += 1;
        assert_eq!(p, addr::TRAINER_NO);
    }

    #[test]
    fn cartridge_ram_is_never_writable() {
        for a in 0xA000u32..=0xBFFF {
            assert!(!is_writable(a as u16), "${a:04X} must not be writable");
        }
        // Nor ROM, VRAM, OAM or the I/O registers.
        assert!(!is_writable(0x0000));
        assert!(!is_writable(0x4000));
        assert!(!is_writable(0x8000));
        assert!(!is_writable(0xFE00));
        assert!(!is_writable(0xFF40));
        assert!(!is_writable(0xFFFF));
        // The two ranges we do use.
        assert!(is_writable(addr::PARTY_MONS));
        assert!(is_writable(addr::ENEMY_MONS));
        assert!(is_writable(addr::CUR_OPPONENT));
        assert!(is_writable(addr::RANDOM_ADD));
        assert!(is_writable(addr::RANDOM_SUB));
    }

    /// A ROM with none of the signatures is refused, and the message names the
    /// missing sequence rather than saying "unsupported".
    #[test]
    fn a_rom_without_the_signatures_is_refused() {
        let e = verify(&vec![0u8; 0x8000]).unwrap_err();
        assert!(matches!(e, VerifyError::SignatureNotFound(_)), "{e:?}");
        assert!(format!("{e}").contains("Red or Blue"));
    }

    /// The point of the check: a ROM that *does* contain the signature but with
    /// a different operand is rejected, naming both addresses. Build the
    /// `InitOpponent` shape with `wCurPartySpecies` moved.
    #[test]
    fn a_moved_address_is_reported_not_written_to() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x100..0x110].copy_from_slice(&[
            0xFA, 0x59, 0xD0, // ld a, [wCurOpponent]
            0xEA, 0x91, 0xCE, // ld [$CE91], a   <- moved by one page
            0xEA, 0xD8, 0xCF, // ld [wEnemyMonSpecies2], a
            0x18, 0x00, 0, 0, 0, 0, 0,
        ]);
        let e = verify(&rom).unwrap_err();
        assert_eq!(
            e,
            VerifyError::AddressMismatch {
                what: "wCurPartySpecies",
                expected: 0xCF91,
                found: 0xCE91,
            }
        );
    }
}

//! Gen 1 (English Red/Blue) RAM map and structure decoding.
//!
//! # Where every address in this file comes from
//!
//! Not from a wiki, and not from memory. The
//! [`pret/pokered`](https://github.com/pret/pokered) disassembly was built at
//! its pinned RGBDS version (1.0.3, `.rgbds-version`) and the resulting ROMs
//! were checked against `roms.sha1`:
//!
//! ```text
//! d7037c83e1ae5b39bde3c30787637ba1d4c48ce2  pokeblue.gbc
//! ea9bcae617fdf159b045185467ae58b2e4a48b9a  pokered.gbc
//! ```
//!
//! Those are the retail ROMs — the SHA-1 of the cartridge this was developed
//! against matches `pokeblue.gbc` byte for byte. Every constant below is then
//! the value the build's own symbol file (`pokeblue.sym`) gives for the named
//! `pokered` label, and **`pokered.sym` gives the identical address for every
//! one of them**, which is why one table serves both games. The label name is
//! quoted next to each constant so any of them can be re-derived by rebuilding
//! the disassembly; the layout itself is `pret/pokered`'s `ram/wram.asm`
//! combined with `layout.link`.
//!
//! # Extending beyond Red/Blue
//!
//! Yellow is `pret/pokeyellow`, the same structures at *different* addresses
//! (its WRAM shifted), so it needs its own symbol pass and its own
//! [`super::Game`] variant — a table swap, not new decoding. Gen 2
//! (`pret/pokecrystal`) changed the party structure itself (48-byte entries,
//! separate held item, different stat experience), so it needs a second decoder
//! as well as a second table. Neither is implemented here; the ROM check in
//! [`super::Game::detect`] refuses them rather than showing plausible garbage.

/// Every address here is `WRAM0`, i.e. it lives at a fixed place on the bus
/// regardless of banking, so a plain [`terminalgb_plugin_api::Machine::peek`] reads it
/// correctly at any time.
pub mod addr {
    /// `wPlayerName` — 11 bytes, terminated by `$50`.
    pub const PLAYER_NAME: u16 = 0xD158;
    /// `wPartyCount` — 0..=6.
    pub const PARTY_COUNT: u16 = 0xD163;
    /// `wPartySpecies` — 6 internal index numbers plus an `$FF` terminator.
    pub const PARTY_SPECIES: u16 = 0xD164;
    /// `wPartyMons` — six [`PARTY_MON_SIZE`]-byte `party_struct`s.
    pub const PARTY_MONS: u16 = 0xD16B;
    /// `wPartyMonOT` — six 11-byte original-trainer names.
    pub const PARTY_MON_OT: u16 = 0xD273;
    /// `wPartyMonNicks` — six 11-byte nicknames.
    pub const PARTY_MON_NICKS: u16 = 0xD2B5;
    /// `wPlayerMoney` — 3 bytes, big-endian **BCD**.
    pub const PLAYER_MONEY: u16 = 0xD347;
    /// `wObtainedBadges` — one bit per badge, see [`BADGE_NAMES`].
    pub const OBTAINED_BADGES: u16 = 0xD356;
    /// `wPlayerID` — 16-bit big-endian trainer ID.
    pub const PLAYER_ID: u16 = 0xD359;
    /// `wPlayTimeHours` (`wPlayTimeMaxed` at `$DA42` sits between hours and
    /// minutes, which is why these are not three consecutive bytes).
    pub const PLAY_TIME_HOURS: u16 = 0xDA41;
    /// `wPlayTimeMinutes`.
    pub const PLAY_TIME_MINUTES: u16 = 0xDA43;
    /// `wPlayTimeSeconds`.
    pub const PLAY_TIME_SECONDS: u16 = 0xDA44;

    /// `wIsInBattle` — `-1` lost, 0 none, 1 wild, 2 trainer. The four values
    /// and their meanings are the comment block above the label in
    /// `pokered/ram/wram.asm`; the only writers are `InitWildBattle` (1) and
    /// `InitBattleCommon` (2) in `engine/battle/core.asm`, `.allPokemonFainted`
    /// (`$FF`) in `home/overworld.asm`, and the two places that clear it.
    pub const IS_IN_BATTLE: u16 = 0xD057;
    /// `wCurOpponent` — in a wild battle the species; in a trainer battle the
    /// trainer class plus `OPP_ID_OFFSET`.
    pub const CUR_OPPONENT: u16 = 0xD059;
    /// `wBattleType` — 0 normal, 1 the old man's catching tutorial, 2 Safari
    /// Zone (`BATTLE_TYPE_*`, `constants/battle_constants.asm`).
    ///
    /// This one is not in `pokeblue.sym` under a name this project had already
    /// resolved, so it is derived by *walking* `ram/wram.asm` between two
    /// symbols that are — which is also the walk's own proof:
    /// `wIsInBattle` `$D057` (verified), then `wPartyGainExpFlags`
    /// (`flag_array PARTY_LENGTH` = 1 byte) `$D058`, then `wCurOpponent`
    /// `$D059` — and `$D059` is exactly [`CUR_OPPONENT`], verified
    /// independently. One more byte lands `wBattleType` at `$D05A`.
    pub const BATTLE_TYPE: u16 = 0xD05A;
    /// `wTrainerClass`.
    pub const TRAINER_CLASS: u16 = 0xD031;
    /// `wTrainerName` — 11 bytes, `$50`-terminated.
    pub const TRAINER_NAME: u16 = 0xD04A;
    /// `wEnemyMonNick` — 11 bytes.
    pub const ENEMY_MON_NICK: u16 = 0xCFDA;
    /// `wEnemyMon` — a [`BATTLE_MON_SIZE`]-byte `battle_struct`.
    pub const ENEMY_MON: u16 = 0xCFE5;
    /// `wBattleMonNick` — 11 bytes.
    pub const BATTLE_MON_NICK: u16 = 0xD009;
    /// `wBattleMon` — the player's active Pokémon, same `battle_struct`.
    pub const BATTLE_MON: u16 = 0xD014;
    /// `wEnemyPartyCount` — how many Pokémon the opposing trainer brought.
    ///
    /// The enemy block mirrors the player's byte for byte: count, then a
    /// species list with an `$FF` terminator, then the `party_struct`s — the
    /// same `+1` / `+8` offsets as [`PARTY_COUNT`] / [`PARTY_SPECIES`] /
    /// [`PARTY_MONS`], which `the_enemy_block_mirrors_the_player_block` pins.
    /// The values are only *used* after
    /// [`TrainerState::enemy_team`](crate::TrainerState::enemy_team)'s
    /// consistency check passes, so a wrong address shows `?` rather than a
    /// confident lie.
    pub const ENEMY_PARTY_COUNT: u16 = 0xD89C;
    /// `wEnemyPartyMons` — six internal index numbers plus an `$FF` terminator.
    pub const ENEMY_PARTY_SPECIES: u16 = 0xD89D;
    /// `wEnemyMons` — six [`PARTY_MON_SIZE`]-byte `party_struct`s, the same
    /// layout as the player's party.
    pub const ENEMY_MONS: u16 = 0xD8A4;
    /// `wEnemyMonActualCatchRate` — the byte Gen 1's capture routine actually
    /// compares against.
    ///
    /// **Not the species' base catch rate.** It is loaded from the base stats
    /// for an ordinary wild Pokémon, but bait and rocks move it in the Safari
    /// Zone, so reading it is the only way to be right in both places. Verified
    /// by disassembly rather than by symbol table: `ItemUsePokeBall` reads
    /// exactly this address at `03:5777` and `03:5797` on this cartridge — see
    /// [`super::capture`].
    pub const ENEMY_ACTUAL_CATCH_RATE: u16 = 0xD007;
    /// `wEnemyMonStatMods` — in-battle stat stages, `7` is neutral
    /// (attack, defense, speed, special, accuracy, evasion).
    pub const ENEMY_STAT_MODS: u16 = 0xCD2E;
    /// `wPlayerMonStatMods`.
    pub const PLAYER_STAT_MODS: u16 = 0xCD1A;
    /// `wCurrentMenuItem` — the live cursor index of whatever menu is open,
    /// zero-based. In a battle's move list that is the move slot.
    pub const CURRENT_MENU_ITEM: u16 = 0xCC26;
    /// `wNumBagItems` — how many distinct items the bag holds.
    pub const NUM_BAG_ITEMS: u16 = 0xD31D;
    /// `wBagItems` — id/quantity pairs, terminated by `$FF`.
    pub const BAG_ITEMS: u16 = 0xD31E;
    /// `BAG_ITEM_CAPACITY` (`constants/menu_constants.asm`), which is what
    /// `wBagItems: ds BAG_ITEM_CAPACITY * 2 + 1` is sized from.
    pub const MAX_BAG_ITEMS: usize = 20;
}

/// `party_struct`, `wPartyMon2 - wPartyMon1 = $D197 - $D16B`.
pub const PARTY_MON_SIZE: u16 = 44;
/// `box_struct` (`macros/ram.asm`) — the first 33 bytes of a `party_struct`
/// (`party_struct` is defined as `box_struct` + level + the six computed
/// stats), and the whole of a stored PC-box Pokémon.
/// `wBoxMon2 - wBoxMon1 = $DAB7 - $DA96`.
pub const BOX_MON_SIZE: u16 = 33;
/// `MONS_PER_BOX` (`constants/pokemon_data_constants.asm`).
pub const MONS_PER_BOX: usize = 20;
/// `NUM_BOXES`.
pub const NUM_BOXES: usize = 12;
/// `NUM_ATTACKS` (`constants/move_constants.asm`) — `STRUGGLE` is `$A5`, the
/// last real move. A stored move id above this is glitch data.
pub const NUM_ATTACKS: u8 = 0xA5;
/// `battle_struct`, the in-battle form: no experience or original-trainer ID,
/// so the level and stats sit at different offsets from the party form.
pub const BATTLE_MON_SIZE: u16 = 28;
/// `NAME_LENGTH` (`constants/text_constants.asm`).
pub const NAME_LENGTH: u16 = 11;
/// `PARTY_LENGTH`.
pub const PARTY_LENGTH: usize = 6;
/// `NUM_MOVES`.
pub const NUM_MOVES: usize = 4;

/// Field offsets inside a 44-byte `party_struct`, from `wPartyMon1*` symbols.
pub mod party {
    pub const SPECIES: u16 = 0; // wPartyMon1Species   $D16B
    pub const HP: u16 = 1; // wPartyMon1HP        $D16C (big-endian)
    pub const STATUS: u16 = 4; // wPartyMon1Status    $D16F
    pub const TYPE1: u16 = 5; // wPartyMon1Type      $D170
    pub const TYPE2: u16 = 6; // wPartyMon1Type + 1  $D171
    pub const MOVES: u16 = 8; // wPartyMon1Moves     $D173
    pub const HP_EXP: u16 = 17; // wPartyMon1HPExp     $D17C (big-endian)
    pub const ATTACK_EXP: u16 = 19; // wPartyMon1AttackExp  $D17E
    pub const DEFENSE_EXP: u16 = 21; // wPartyMon1DefenseExp $D180
    pub const SPEED_EXP: u16 = 23; // wPartyMon1SpeedExp   $D182
    pub const SPECIAL_EXP: u16 = 25; // wPartyMon1SpecialExp $D184
    pub const DVS: u16 = 27; // wPartyMon1DVs       $D186
    pub const PP: u16 = 29; // wPartyMon1PP        $D188
    pub const LEVEL: u16 = 33; // wPartyMon1Level     $D18C
    pub const MAX_HP: u16 = 34; // wPartyMon1MaxHP     $D18D
    pub const ATTACK: u16 = 36; // wPartyMon1Attack    $D18F
    pub const DEFENSE: u16 = 38; // wPartyMon1Defense   $D191
    pub const SPEED: u16 = 40; // wPartyMon1Speed     $D193
    pub const SPECIAL: u16 = 42; // wPartyMon1Special   $D195
}

/// Field offsets inside a 28-byte `battle_struct`, from `wEnemyMon*` symbols
/// (`wBattleMon*` repeats them exactly, 0x2F higher).
pub mod battle {
    pub const SPECIES: u16 = 0; // wEnemyMonSpecies   $CFE5
    pub const HP: u16 = 1; // wEnemyMonHP        $CFE6 (big-endian)
    pub const STATUS: u16 = 4; // wEnemyMonStatus    $CFE9
    pub const TYPE1: u16 = 5; // wEnemyMonType1     $CFEA
    pub const TYPE2: u16 = 6; // wEnemyMonType2     $CFEB
    pub const MOVES: u16 = 8; // wEnemyMonMoves     $CFED
    pub const DVS: u16 = 12; // wEnemyMonDVs       $CFF1
    pub const LEVEL: u16 = 14; // wEnemyMonLevel     $CFF3
    pub const MAX_HP: u16 = 15; // wEnemyMonMaxHP     $CFF4
    pub const ATTACK: u16 = 17; // wEnemyMonAttack    $CFF6
    pub const DEFENSE: u16 = 19; // wEnemyMonDefense   $CFF8
    pub const SPEED: u16 = 21; // wEnemyMonSpeed     $CFFA
    pub const SPECIAL: u16 = 23; // wEnemyMonSpecial   $CFFC
    pub const PP: u16 = 25; // wEnemyMonPP        $CFFE
}

/// Field offsets inside a 33-byte `box_struct`, from `wBoxMon1*` symbols.
///
/// `party_struct` *is* `box_struct` plus a trailing level byte and six
/// computed stats (`macros/ram.asm`), so every offset here equals the party
/// offset of the same field — pinned by
/// `a_party_struct_opens_with_a_box_struct`. The one field with no party twin
/// is [`boxmon::BOX_LEVEL`]: the level the Pokémon had when it was deposited.
/// On withdrawal the game recomputes the party level from the experience
/// field, but the stored `BoxLevel` is what the PC's own list displays, so it
/// is the honest number to show for a boxed Pokémon.
pub mod boxmon {
    pub const SPECIES: u16 = 0; // wBoxMon1Species    $DA96
    pub const HP: u16 = 1; // wBoxMon1HP         $DA97 (big-endian)
    pub const BOX_LEVEL: u16 = 3; // wBoxMon1BoxLevel   $DA99
    pub const STATUS: u16 = 4; // wBoxMon1Status     $DA9A
    pub const TYPE1: u16 = 5; // wBoxMon1Type1      $DA9B
    pub const TYPE2: u16 = 6; // wBoxMon1Type2      $DA9C
    pub const MOVES: u16 = 8; // wBoxMon1Moves      $DA9E
    pub const OT_ID: u16 = 12; // wBoxMon1OTID       $DAA2 (big-endian)
    pub const DVS: u16 = 27; // wBoxMon1DVs        $DAB1
    pub const PP: u16 = 29; // wBoxMon1PP         $DAB3
}

/// The PC box system: the **current** box lives in WRAM, the other eleven in
/// SRAM banks 2 and 3.
///
/// Layout (`ram/wram.asm` "Current Box Data" section, `ram/sram.asm`,
/// `engine/menus/save.asm`): a box block is a count byte, a species list of
/// `MONS_PER_BOX + 1` (an `$FF` terminator), `MONS_PER_BOX` `box_struct`s,
/// then the OT names and nicknames, 11 bytes each. Every address is the value
/// `pokeblue.sym` gives for the named label, from a build byte-identical to
/// the retail ROM (`roms.sha1`), and `pokered.sym` agrees on all of them.
pub mod boxes {
    /// `wCurrentBoxNum` — bits 0-6 the box number (0-11), bit 7
    /// `BIT_HAS_CHANGED_BOXES` (`constants/ram_constants.asm`).
    pub const CURRENT_BOX_NUM: u16 = 0xD5A0;
    /// `BOX_NUM_MASK`.
    pub const BOX_NUM_MASK: u8 = 0x7F;
    /// `BIT_HAS_CHANGED_BOXES` as a mask (bit 7). **Load-bearing**: the first
    /// ever box change runs `EmptyAllSRAMBoxes` (`engine/menus/save.asm`,
    /// `ChangeBox`), so until this bit is set SRAM banks 2/3 are
    /// uninitialised garbage and only the WRAM box exists.
    pub const HAS_CHANGED_BOXES: u8 = 0x80;
    /// `wBoxDataStart` == `wBoxCount` — the current box, in WRAM.
    pub const BOX_DATA_START: u16 = 0xDA80;
    /// `wBoxSpecies`.
    pub const BOX_SPECIES: u16 = 0xDA81;
    /// `wBoxMons` — `MONS_PER_BOX` 33-byte `box_struct`s.
    pub const BOX_MONS: u16 = 0xDA96;
    /// `wBoxMonOT`.
    pub const BOX_MON_OT: u16 = 0xDD2A;
    /// `wBoxMonNicks`.
    pub const BOX_MON_NICKS: u16 = 0xDE06;
    /// `wBoxDataEnd`.
    pub const BOX_DATA_END: u16 = 0xDEE2;
    /// One whole box block, `wBoxDataEnd - wBoxDataStart` — also the SRAM
    /// stride, `sBox2 - sBox1`.
    pub const BOX_BLOCK: u16 = 0x462;

    /// `sBox1`/`sBox7` — boxes 0-5 sit in SRAM bank 2, boxes 6-11 in bank 3,
    /// six per bank from the bank's start (`GetBoxSRAMLocation` +
    /// `BoxSRAMPointerTable`, `engine/menus/save.asm`).
    pub const SBOX_BASE: u16 = 0xA000;
    /// SRAM bank of boxes 0-5 (`ram/sram.asm` "Saved Boxes 1" = bank 2).
    pub const SRAM_BANK_LOW: usize = 2;
    /// SRAM bank of boxes 6-11.
    pub const SRAM_BANK_HIGH: usize = 3;
    /// `sBank2IndividualBoxChecksums` / `sBank3IndividualBoxChecksums` — six
    /// bytes per bank, one per box: `CalcCheckSum` over the box's `BOX_BLOCK`
    /// bytes (the complement of their byte sum). Recomputed on every box
    /// change and by `EmptyAllSRAMBoxes`, so a mismatch means the block is
    /// not a box the game wrote.
    pub const INDIVIDUAL_CHECKSUMS: u16 = 0xBA4D;

    /// Where box `n` (0-11) lives in SRAM: `(bank, base address)`.
    pub fn sram_slot(n: usize) -> (usize, u16) {
        let bank = if n < 6 { SRAM_BANK_LOW } else { SRAM_BANK_HIGH };
        (bank, SBOX_BASE + (n % 6) as u16 * BOX_BLOCK)
    }
}

/// `wObtainedBadges` bit order, from `constants/ram_constants.asm`
/// (`BIT_BOULDERBADGE` = 0 … `BIT_EARTHBADGE` = 7).
pub const BADGE_NAMES: [&str; 8] = [
    "Boulder", "Cascade", "Thunder", "Rainbow", "Soul", "Marsh", "Volcano", "Earth",
];

/// The gym leader who hands over each badge, in the same bit order. Not in the
/// disassembly as a table — it is the series' own naming, kept here only so the
/// panel can label the badges a player recognises.
pub const BADGE_LEADERS: [&str; 8] = [
    "Brock", "Misty", "Lt.Surge", "Erika", "Koga", "Sabrina", "Blaine", "Giovanni",
];

/// Non-volatile status, from `constants/battle_constants.asm`: the low three
/// bits are a sleep turn counter (`SLP_MASK`), then one bit each for poison
/// (3), burn (4), freeze (5) and paralysis (6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Status(pub u8);

impl Status {
    pub fn is_healthy(self) -> bool {
        self.0 == 0
    }

    /// Remaining sleep turns, 0 when awake.
    pub fn sleep_turns(self) -> u8 {
        self.0 & 0b111
    }

    /// Short label for the panel, or `None` when healthy.
    pub fn label(self) -> Option<&'static str> {
        if self.sleep_turns() > 0 {
            return Some("SLP");
        }
        match self.0 {
            v if v & (1 << 3) != 0 => Some("PSN"),
            v if v & (1 << 4) != 0 => Some("BRN"),
            v if v & (1 << 5) != 0 => Some("FRZ"),
            v if v & (1 << 6) != 0 => Some("PAR"),
            _ => None,
        }
    }
}

/// One move slot: the move's internal id, its current/maximum PP, and the
/// three facts a turn is actually decided on — what it hits for, what type it
/// is, and how often it lands.
///
/// PP is packed: the low six bits are the remaining PP, the top two are the
/// number of PP Ups applied (`ram/wram.asm`, `wPartyMon1PP`). Maximum PP
/// therefore has to come from the move's own data in ROM, which is why
/// [`super::rom`] resolves it — and once that table is being read, power, type
/// and accuracy are the next three bytes of the same entry.
#[derive(Debug, Clone, Default)]
pub struct MoveSlot {
    pub id: u8,
    pub name: String,
    pub pp: u8,
    pub pp_max: u8,
    pub pp_ups: u8,
    /// Base power. **Zero means the move does no direct damage** — a status
    /// move — which is a fact, not a missing value, so the panel prints a dash
    /// rather than a `0` or the unknown glyph.
    pub power: u8,
    /// The move's own type, which is often *not* the user's: a Pikachu's
    /// QUICK ATTACK is Normal, and whether the same-type bonus applies is the
    /// first thing a reader wants from this row.
    pub type_name: String,
    /// Accuracy as a percentage, converted from the cartridge's 0-255 byte.
    /// 100 for a move that cannot miss on its accuracy roll — Gen 1's separate
    /// 1-in-256 miss is a property of the engine, not of the move, and is not
    /// folded in here.
    pub accuracy: u8,
}

/// A Pokémon as the panel shows it, decoded from either struct form.
#[derive(Debug, Clone, Default)]
pub struct Mon {
    /// Internal index number (the `MonsterNames`/`PokedexOrder` index, *not*
    /// the Pokédex number).
    pub species: u8,
    /// National Pokédex number, resolved through `PokedexOrder`.
    pub dex: u8,
    pub name: String,
    pub nickname: String,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub status: Status,
    pub types: (String, String),
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub special: u16,
    pub moves: Vec<MoveSlot>,
    /// Individual values, unpacked from the two DV bytes.
    pub dvs: Dvs,
    /// Stat experience, present only for a party Pokémon — the in-battle
    /// structure does not carry it. See [`super::derived::StatExp`].
    pub stat_exp: Option<super::derived::StatExp>,
    /// In-battle stat stages, present only for the two Pokémon actually in a
    /// battle. **Never carried on a party copy**: stages do not survive a
    /// switch or the end of a fight, and a stale `-1` shown in the overworld
    /// would be worse than showing nothing. See [`super::derived::StatMods`].
    pub stat_mods: Option<super::derived::StatMods>,
}

impl Mon {
    /// Fraction of maximum HP remaining, clamped to `0.0..=1.0`.
    pub fn hp_fraction(&self) -> f32 {
        if self.max_hp == 0 {
            return 0.0;
        }
        (self.hp as f32 / self.max_hp as f32).clamp(0.0, 1.0)
    }

    pub fn is_fainted(&self) -> bool {
        self.hp == 0
    }

    /// Whether this Pokémon would be shiny if traded to Gen 2. The Gen 1 game
    /// has no such concept; see [`super::derived::shiny_if_traded`].
    pub fn would_be_shiny(&self) -> bool {
        super::derived::shiny_if_traded(self.dvs)
    }

    /// Hidden Power's type and power as Gen 2 would compute them.
    pub fn hidden_power(&self) -> super::derived::HiddenPower {
        super::derived::hidden_power(self.dvs)
    }

    /// The sum of the four stored DVs plus the derived HP DV, out of 75 — a
    /// single number for "how good is this one", the way a tracker wants it.
    pub fn dv_total(&self) -> u8 {
        self.dvs.attack
            + self.dvs.defense
            + self.dvs.speed
            + self.dvs.special
            + self.dvs.hp()
    }

    /// The two type names, deduplicated — a single-type Pokémon stores the same
    /// type in both slots.
    pub fn type_label(&self) -> String {
        if self.types.1.is_empty() || self.types.0 == self.types.1 {
            self.types.0.clone()
        } else {
            format!("{}/{}", self.types.0, self.types.1)
        }
    }
}

/// Individual values. Gen 1 packs five stats into two bytes: attack, defense,
/// speed and special are one nybble each, and the HP DV is assembled from the
/// low bit of the other four (`ram/wram.asm` `wPartyMon1DVs`; the HP rule is
/// `CalcStat` in `home/move_mon.asm`, which assembles it from bit 0 of the
/// attack, defense, speed and special IVs in that order).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dvs {
    pub attack: u8,
    pub defense: u8,
    pub speed: u8,
    pub special: u8,
}

impl Dvs {
    pub fn from_bytes(hi: u8, lo: u8) -> Dvs {
        Dvs {
            attack: hi >> 4,
            defense: hi & 0x0F,
            speed: lo >> 4,
            special: lo & 0x0F,
        }
    }

    /// The derived HP DV: bit 0 of attack, defense, speed and special, in that
    /// order from most to least significant.
    pub fn hp(self) -> u8 {
        ((self.attack & 1) << 3)
            | ((self.defense & 1) << 2)
            | ((self.speed & 1) << 1)
            | (self.special & 1)
    }
}

/// What the game is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BattleKind {
    #[default]
    None,
    Wild,
    Trainer,
}

impl BattleKind {
    /// From `wIsInBattle`. The label's own comment in `pokered/ram/wram.asm`
    /// lists **four** values, not three: `-1` lost, `0` none, `1` wild, `2`
    /// trainer. `$FF` is written by `.allPokemonFainted` in
    /// `home/overworld.asm` — the player blacked out, which is the *end* of a
    /// battle, not one in progress — so it maps to [`BattleKind::None`], as does
    /// any value the game never writes.
    pub fn from_flag(v: u8) -> BattleKind {
        match v {
            1 => BattleKind::Wild,
            2 => BattleKind::Trainer,
            _ => BattleKind::None,
        }
    }
}

/// `wBattleType` — which *kind* of wild battle this is.
///
/// `constants/battle_constants.asm`: `BATTLE_TYPE_NORMAL` 0,
/// `BATTLE_TYPE_OLD_MAN` 1, `BATTLE_TYPE_SAFARI` 2. It only means anything
/// while [`BattleKind::Wild`] is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BattleType {
    #[default]
    Normal,
    /// The old man's catching demonstration in Viridian City. The player never
    /// keeps this Pokémon — `_AddPartyMon` is never reached for it — so a
    /// "worth a Ball?" verdict there is noise.
    OldMan,
    Safari,
    /// A value the game does not define. Treated as not catchable, because
    /// guessing is the failure mode this whole plugin refuses.
    Unknown(u8),
}

impl BattleType {
    pub fn from_flag(v: u8) -> BattleType {
        match v {
            0 => BattleType::Normal,
            1 => BattleType::OldMan,
            2 => BattleType::Safari,
            other => BattleType::Unknown(other),
        }
    }

    /// Whether a Pokémon met under this battle type can actually become the
    /// player's. See [`super::encounter`] for the full argument and citations.
    pub fn is_catchable(self) -> bool {
        matches!(self, BattleType::Normal | BattleType::Safari)
    }

    pub fn label(self) -> &'static str {
        match self {
            BattleType::Normal => "WILD",
            BattleType::OldMan => "DEMO",
            BattleType::Safari => "SAFARI",
            BattleType::Unknown(_) => "?",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opposing trainer's roster is read out of a block laid out exactly
    /// like the player's — count, species list with terminator, then the
    /// structs. Holding that relationship here means the two cannot drift apart
    /// silently, and it is the structural half of the argument that the enemy
    /// addresses are right (the other half is the runtime cross-check in
    /// `TrainerPlugin::sample`).
    #[test]
    fn the_enemy_block_mirrors_the_player_block() {
        let d = |a: u16, b: u16| a - b;
        assert_eq!(
            d(addr::PARTY_SPECIES, addr::PARTY_COUNT),
            d(addr::ENEMY_PARTY_SPECIES, addr::ENEMY_PARTY_COUNT)
        );
        assert_eq!(
            d(addr::PARTY_MONS, addr::PARTY_COUNT),
            d(addr::ENEMY_MONS, addr::ENEMY_PARTY_COUNT)
        );
        // The species list is PARTY_LENGTH entries plus an $FF terminator.
        assert_eq!(
            addr::ENEMY_PARTY_SPECIES + PARTY_LENGTH as u16 + 1,
            addr::ENEMY_MONS
        );
    }

    #[test]
    fn party_struct_stride_matches_the_symbol_file() {
        // wPartyMon1 = $D16B, wPartyMon2 = $D197 in both pokered.sym and
        // pokeblue.sym; the difference IS the struct size.
        assert_eq!(addr::PARTY_MONS + PARTY_MON_SIZE, 0xD197);
        // wPartyMon1Nick = $D2B5, wPartyMon2Nick = $D2C0.
        assert_eq!(addr::PARTY_MON_NICKS + NAME_LENGTH, 0xD2C0);
    }

    #[test]
    fn party_field_offsets_land_on_their_symbols() {
        let base = addr::PARTY_MONS;
        assert_eq!(base + party::HP, 0xD16C); // wPartyMon1HP
        assert_eq!(base + party::STATUS, 0xD16F); // wPartyMon1Status
        assert_eq!(base + party::MOVES, 0xD173); // wPartyMon1Moves
        assert_eq!(base + party::DVS, 0xD186); // wPartyMon1DVs
        assert_eq!(base + party::PP, 0xD188); // wPartyMon1PP
        assert_eq!(base + party::LEVEL, 0xD18C); // wPartyMon1Level
        assert_eq!(base + party::MAX_HP, 0xD18D); // wPartyMon1MaxHP
        assert_eq!(base + party::SPECIAL, 0xD195); // wPartyMon1Special
    }

    #[test]
    fn battle_field_offsets_land_on_their_symbols() {
        let base = addr::ENEMY_MON;
        assert_eq!(base + battle::HP, 0xCFE6); // wEnemyMonHP
        assert_eq!(base + battle::MOVES, 0xCFED); // wEnemyMonMoves
        assert_eq!(base + battle::LEVEL, 0xCFF3); // wEnemyMonLevel
        assert_eq!(base + battle::MAX_HP, 0xCFF4); // wEnemyMonMaxHP
        assert_eq!(base + battle::PP, 0xCFFE); // wEnemyMonPP
                                               // The player's in-battle copy uses the identical struct.
        assert_eq!(addr::BATTLE_MON + battle::LEVEL, 0xD022); // wBattleMonLevel
        assert_eq!(addr::BATTLE_MON + battle::PP, 0xD02D); // wBattleMonPP
    }

    #[test]
    fn status_bits_decode_to_the_right_labels() {
        assert!(Status(0).is_healthy());
        assert_eq!(Status(0).label(), None);
        assert_eq!(Status(0b0000_0011).label(), Some("SLP")); // 3 turns left
        assert_eq!(Status(0b0000_1000).label(), Some("PSN"));
        assert_eq!(Status(0b0001_0000).label(), Some("BRN"));
        assert_eq!(Status(0b0010_0000).label(), Some("FRZ"));
        assert_eq!(Status(0b0100_0000).label(), Some("PAR"));
    }

    #[test]
    fn dvs_unpack_and_derive_hp() {
        // $9A88 is the standard trainer-Pokémon DV pair minus a nybble; use a
        // value whose four DVs are all distinct and whose low bits are known.
        let d = Dvs::from_bytes(0x9A, 0x88);
        assert_eq!((d.attack, d.defense, d.speed, d.special), (9, 10, 8, 8));
        // odd, even, even, even -> 0b1000
        assert_eq!(d.hp(), 0b1000);
        // All-15 DVs give an HP DV of 15.
        assert_eq!(Dvs::from_bytes(0xFF, 0xFF).hp(), 15);
    }

    #[test]
    fn battle_kind_maps_the_flag() {
        assert_eq!(BattleKind::from_flag(0), BattleKind::None);
        assert_eq!(BattleKind::from_flag(1), BattleKind::Wild);
        assert_eq!(BattleKind::from_flag(2), BattleKind::Trainer);
        // `-1` is "the player blacked out", not a battle in progress
        // (`home/overworld.asm`, `.allPokemonFainted`). Reading it as a trainer
        // battle would put a battle panel on screen in the overworld.
        assert_eq!(BattleKind::from_flag(0xFF), BattleKind::None);
        // Nothing else is ever written, and a stray value must not invent a
        // battle either.
        for v in 3u8..0xFF {
            assert_eq!(BattleKind::from_flag(v), BattleKind::None, "flag {v}");
        }
    }

    #[test]
    fn battle_type_maps_the_flag_and_knows_what_is_catchable() {
        assert_eq!(BattleType::from_flag(0), BattleType::Normal);
        assert_eq!(BattleType::from_flag(1), BattleType::OldMan);
        assert_eq!(BattleType::from_flag(2), BattleType::Safari);
        assert_eq!(BattleType::from_flag(9), BattleType::Unknown(9));
        assert!(BattleType::Normal.is_catchable());
        assert!(BattleType::Safari.is_catchable(), "Safari Balls are Balls");
        assert!(
            !BattleType::OldMan.is_catchable(),
            "the tutorial Pokémon is never the player's"
        );
        assert!(!BattleType::Unknown(7).is_catchable());
    }

    /// `party_struct` is `box_struct` + level + stats (`macros/ram.asm`), so
    /// the box offsets must equal the party offsets field for field. Holding
    /// the relationship here means the two tables cannot drift apart.
    #[test]
    fn a_party_struct_opens_with_a_box_struct() {
        assert_eq!(boxmon::SPECIES, party::SPECIES);
        assert_eq!(boxmon::HP, party::HP);
        assert_eq!(boxmon::STATUS, party::STATUS);
        assert_eq!(boxmon::TYPE1, party::TYPE1);
        assert_eq!(boxmon::TYPE2, party::TYPE2);
        assert_eq!(boxmon::MOVES, party::MOVES);
        assert_eq!(boxmon::DVS, party::DVS);
        assert_eq!(boxmon::PP, party::PP);
        // The party level byte is the first thing party_struct adds after the
        // 33 box_struct bytes.
        assert_eq!(party::LEVEL, BOX_MON_SIZE);
        assert_eq!(PARTY_MON_SIZE, BOX_MON_SIZE + 1 + 10);
        // wBoxMon1OTID $DAA2 = wBoxMons $DA96 + 12; the party twin is
        // wPartyMon1OTID $D177 = wPartyMons $D16B + 12.
        assert_eq!(boxes::BOX_MONS + boxmon::OT_ID, 0xDAA2);
        assert_eq!(addr::PARTY_MONS + boxmon::OT_ID, 0xD177);
    }

    /// The box block's internal layout, pinned against the symbol file the
    /// way `party_struct_stride_matches_the_symbol_file` pins the party's.
    #[test]
    fn the_box_block_walks_onto_its_symbols() {
        // wBoxSpecies = wBoxCount + 1.
        assert_eq!(boxes::BOX_DATA_START + 1, boxes::BOX_SPECIES);
        // wBoxMons follows the species list (MONS_PER_BOX + terminator).
        assert_eq!(
            boxes::BOX_SPECIES + MONS_PER_BOX as u16 + 1,
            boxes::BOX_MONS
        );
        // wBoxMon2 = $DAB7: the stride IS the struct size.
        assert_eq!(boxes::BOX_MONS + BOX_MON_SIZE, 0xDAB7);
        // 20 box_structs land exactly on wBoxMonOT, then the two name tables
        // land on wBoxMonNicks and wBoxDataEnd.
        assert_eq!(
            boxes::BOX_MONS + MONS_PER_BOX as u16 * BOX_MON_SIZE,
            boxes::BOX_MON_OT
        );
        assert_eq!(
            boxes::BOX_MON_OT + MONS_PER_BOX as u16 * NAME_LENGTH,
            boxes::BOX_MON_NICKS
        );
        assert_eq!(
            boxes::BOX_MON_NICKS + MONS_PER_BOX as u16 * NAME_LENGTH,
            boxes::BOX_DATA_END
        );
        // The block size is also the SRAM stride: sBox2 $A462 - sBox1 $A000.
        assert_eq!(
            boxes::BOX_DATA_END - boxes::BOX_DATA_START,
            boxes::BOX_BLOCK
        );
        assert_eq!(boxes::BOX_BLOCK, 0xA462 - 0xA000);
    }

    /// `GetBoxSRAMLocation`: boxes 0-5 → bank 2 from $A000, boxes 6-11 →
    /// bank 3 from $A000, `BOX_BLOCK` apart.
    #[test]
    fn sram_slots_follow_the_pointer_table() {
        assert_eq!(boxes::sram_slot(0), (2, 0xA000)); // sBox1
        assert_eq!(boxes::sram_slot(1), (2, 0xA462)); // sBox2
        assert_eq!(boxes::sram_slot(5), (2, 0xA000 + 5 * 0x462)); // sBox6
        assert_eq!(boxes::sram_slot(6), (3, 0xA000)); // sBox7
        assert_eq!(boxes::sram_slot(11), (3, 0xA000 + 5 * 0x462)); // sBox12
                                                                   // The checksum table sits right after the all-boxes checksum at
                                                                   // $BA4C, which itself follows the six boxes.
        assert_eq!(0xA000 + 6 * boxes::BOX_BLOCK, 0xBA4C);
        assert_eq!(boxes::INDIVIDUAL_CHECKSUMS, 0xBA4D);
    }

    #[test]
    fn the_battle_type_address_is_where_the_wram_walk_puts_it() {
        // wIsInBattle $D057 + wPartyGainExpFlags (1 byte) == wCurOpponent, and
        // wCurOpponent is independently verified — so the walk is checked, not
        // assumed, before it is used for the byte after.
        assert_eq!(addr::IS_IN_BATTLE + 2, addr::CUR_OPPONENT);
        assert_eq!(addr::CUR_OPPONENT + 1, addr::BATTLE_TYPE);
    }
}

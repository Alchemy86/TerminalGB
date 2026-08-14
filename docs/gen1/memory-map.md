# Where Pokémon Red and Blue keep everything

[← Gen 1 area](README.md)

Every address on this page is for **English Pokémon Red and Blue**, which share their WRAM
layout exactly. Yellow does not (its WRAM shifted); Generation 2 changed the structures
themselves. Both are named as extensions everywhere, never assumed.

Each entry carries its `pret/pokered` **symbol name** and how the address was established —
see the three grades in the [area standards](README.md#standards-for-this-area). The short
version: *symbol file* means the disassembly was built and its ROMs matched `roms.sha1`;
*walked* means added up through `ram/wram.asm` between two symbol-file addresses and landing
on a third; *cartridge* means read out of a run of machine code in the ROM image itself.

---

## 1. The player's party

The party is the single most useful block in the game: it is what the trainer panel reads,
what the link partner sends, and what battle control replaces.

| Symbol | Address | Size | Grade |
|---|---|---|---|
| `wPartyCount` | `$D163` | 1 | symbol file |
| `wPartySpecies` | `$D164` | 7 (6 + `$FF`) | symbol file |
| `wPartyMons` | `$D16B` | 6 × 44 | symbol file |
| `wPartyMonOT` | `$D273` | 6 × 11 | symbol file |
| `wPartyMonNicks` | `$D2B5` | 6 × 11 | symbol file |
| `wPlayerName` | `$D158` | 11 | symbol file |
| `wPlayerID` | `$D359` | 2, **big-endian** | symbol file |
| `wObtainedBadges` | `$D356` | 1 | symbol file |

`wPartyMon2 - wPartyMon1 = $D197 - $D16B = 44`, which *is* the structure size — the stride is
not a guess, it is the difference between two symbols.

### The 44-byte `party_struct`

From `macros/ram.asm`. `party_struct` is `box_struct` plus a level and five 16-bit stats:

```text
 0  Species        1     internal index, NOT a Pokédex number
 1  HP             2     big-endian
 3  BoxLevel       1     kept in step with Level
 4  Status         1
 5  Type1, Type2   2
 7  CatchRate      1
 8  Moves          4
12  OTID           2     big-endian
14  Exp            3     big-endian
17  HPExp          2 |
19  AttackExp      2 |
21  DefenseExp     2 |   stat experience, big-endian
23  SpeedExp       2 |
25  SpecialExp     2 |
27  DVs            2     4 nybbles: attack, defense, speed, special
29  PP             4     low 6 bits current PP, top 2 bits PP Ups applied
33  Level          1
34  MaxHP          2 |
36  Attack         2 |
38  Defense        2 |   big-endian
40  Speed          2 |
42  Special        2 |
```

**Three traps live in this table.** Every multi-byte field is **big-endian**, on a
little-endian console. The **HP DV is not stored** — it is assembled from bit 0 of the other
four, most significant first (`CalcStat`, `home/move_mon.asm`). And `Species` is the
*internal index*, which is not the Pokédex number: `MonsterNames` is indexed by internal
index, `BaseStats` by Pokédex number, and `PokedexOrder` converts (as `GetMonHeader` does,
with Mew special-cased because its base stats live outside the main table in Red/Blue).

Encoder and decoder: [`plugins/gen1-link/src/party.rs`](../../plugins/gen1-link/src/party.rs). There is
deliberately **one** implementation — the link partner and battle control share it, so a fix
to either fixes both.

---

## 2. The battle

### The four bytes that start one

| Symbol | Address | Meaning | Grade |
|---|---|---|---|
| `wIsInBattle` | `$D057` | −1 lost, 0 none, 1 wild, 2 trainer | symbol file + cartridge |
| `wCurOpponent` | `$D059` | **the trigger** — species, or class + `OPP_ID_OFFSET` | symbol file + cartridge |
| `wBattleType` | `$D05A` | 0 normal, 1 old man tutorial, 2 Safari | walked |
| `wTrainerNo` | `$D05D` | which roster of the class | walked + cartridge |
| `wCurEnemyLevel` | `$D127` | the level a wild opponent is generated at | cartridge |
| `wLinkState` | `$D12B` | nonzero changes the whole battle path | cartridge |
| `wCurPartySpecies` | `$CF91` | written by `InitOpponent` | cartridge |
| `wEnemyMonSpecies2` | `$CFD8` | the value `InitBattleCommon` branches on | cartridge |

`OPP_ID_OFFSET` is **200** (`constants/trainer_constants.asm`). Below it, `wCurOpponent` is a
species; at or above it, a trainer class plus that offset. What to do with these bytes is
[battle control](battle-control.md).

The walk for the middle three, from the symbol-file address of `wIsInBattle`:

```text
wIsInBattle         $D057  db
wPartyGainExpFlags  $D058  flag_array PARTY_LENGTH -> 1 byte
wCurOpponent        $D059  db                        <- independently verified
wBattleType         $D05A  db
wDamageMultipliers  $D05B  db
wLoneAttackNo       $D05C  db  (= wGymLeaderNo)
wTrainerNo          $D05D  db
```

It passes through `wCurOpponent`, which is known independently, so the walk is checked before
it is used for the byte after.

### The combatants: the 29-byte `battle_struct`

The in-battle form is **not** the party form. It drops original trainer, experience and stat
experience, so level and the five stats sit at different offsets — confusing the two is the
classic way to produce a level 6400 Pokémon.

| Symbol | Address |
|---|---|
| `wEnemyMonNick` | `$CFDA` (11 bytes) |
| `wEnemyMon` | `$CFE5` |
| `wBattleMon` | `$D014` |

```text
 0  Species  1     8  Moves     4    15  MaxHP    2
 1  HP       2    12  DVs       2    17  Attack   2
 3  PartyPos 1    14  Level     1    19  Defense  2
 4  Status   1                       21  Speed    2
 5  Type1/2  2                       23  Special  2
 7  CatchRate 1                      25  PP       4
```

29 bytes (`macros/ram.asm`, `battle_struct`). Note that
[`plugins/trainer/src/gen1.rs`](../../plugins/trainer/src/gen1.rs) carries a `BATTLE_MON_SIZE` of 28; that
constant is only ever used as a stride and there is only ever one enemy Pokémon in battle, so
it is harmless there — but do not copy it.

### The opposing party (trainer battles)

`ReadTrainer` builds a whole six-slot party for the opposing trainer, in the *same*
`party_struct` layout as the player's:

| Symbol | Address | Size | Grade |
|---|---|---|---|
| `wEnemyPartyCount` | `$D89C` | 1 | walked + cartridge |
| `wEnemyPartySpecies` | `$D89D` | 7 | walked |
| `wEnemyMons` | `$D8A4` | 6 × 44 | walked |
| `wEnemyMonOT` | `$D9AC` | 6 × 11 | walked |
| `wEnemyMonNicks` | `$D9EE` | 6 × 11 | walked |

The walk starts at `wEventFlags` `$D747` and ends on `wPlayTimeHours` `$DA41`, both known:

```text
wEventFlags           $D747  flag_array NUM_EVENTS (2560) -> 320 bytes
wGrassRate            $D887  <- the union's first member; independently verified
wLinkEnemyTrainerName $D887  ds NAME_LENGTH (11)
                      $D892  ds 1
wSerialEnemyDataBlock $D893  label only
                      $D893  ds 9
wEnemyPartyCount      $D89C
wEnemyPartySpecies    $D89D  ds PARTY_LENGTH + 1
wEnemyMons            $D8A4  6 x party_struct
wEnemyMonOT           $D9AC  6 x NAME_LENGTH
wEnemyMonNicks        $D9EE  6 x NAME_LENGTH
wTrainerHeaderPtr     $DA30  dw
                      $DA32  ds 6
wOpponentAfterWrongAnswer $DA38
wCurMapScript         $DA39
                      $DA3A  ds 7
wPlayTimeHours        $DA41  <- lands exactly on a verified symbol
```

Landing on `$DA41` is the proof. `$D89C` is *also* read out of the cartridge (it is the
operand of `ld hl, wEnemyPartyCount` at the top of `ReadTrainer`), so the walk has an
independent check at its start as well as its end.

**Note the union.** `wEnemyMons` shares its storage with `wGrassMons`/`wWaterMons` — the
map's wild encounter tables. That is not a bug; the game cannot be in a trainer battle and
consulting the wild tables at the same time. It does mean that reading `wEnemyMons` in the
overworld returns encounter data, so a reader must check `wIsInBattle` first.

---

## 3. The screen

| Symbol | Address | Size |
|---|---|---|
| `wTileMap` | `$C3A0` | 20 × 18 |

**This reads back as text.** Gen 1 loads its font so a tile id *is* the character code of the
glyph it draws, and `print_string` writes character codes straight into this buffer. So
decoding `wTileMap` with the Gen 1 charmap prints what a player can see. That single fact
underpins menu navigation, headless testing and the Mooneye debugging trick, and it has its
own page: [reading the screen](reading-the-screen.md).

The character map is in [`plugins/gen1-link/src/text.rs`](../../plugins/gen1-link/src/text.rs)
(`encode_char`/`decode_char`); the screen decoder that renders unmapped tiles as spaces is
[`plugins/trainer/src/text.rs`](../../plugins/trainer/src/text.rs) (`decode_screen`).

---

## 4. The map, the player, and the world

| Symbol | Address | Notes | Grade |
|---|---|---|---|
| `wCurMap` | `$D35E` | map id | symbol file |
| `wYCoord` | `$D361` | | symbol file |
| `wXCoord` | `$D362` | | symbol file |
| `wEventFlags` | `$D747` | 320-byte `flag_array NUM_EVENTS` | walked |
| `wWarpEntries` | — | the room's own warp table, part of the map header | symbol file |

**The map header is saved state, and that is the trap.** `LoadSAV`
(`engine/menus/save.asm`) sets `BIT_NO_PREVIOUS_MAP` on `wCurMapTileset` right after restoring
the main data, and `LoadMapHeader` (`home/overworld.asm`) sees that bit, clears it, and
*returns without loading the header*. The whole `wCurMapHeader` block lives in `sMainData`, so
a continued game trusts what the save holds. Change `wCurMap` without changing the header and
the game reads the new map's blocks through the old map's `wCurMapDataPtr`: observed as a jump
to `rst $38` or an illegal opcode within a second.

So **you cannot teleport the player by editing the save.** Two things that do work:

* **Walk.** Every header is then loaded by the game itself, exactly as in play. This is what
  `testharness/trainer.rs` does to reach Route 1's grass.
* **Redirect the room's own warp** (`wWarpEntries`) and let the first real warp do the full
  load. Move the world's pointers, not the player. This is how `examples/gen1_trade.rs` gets
  into the Cable Club.

`EVENT_FOLLOWED_OAK_INTO_LAB` is the first `const` after `const_def` in the event list, so it
is byte 0 bit 0 of `wEventFlags`. Setting it makes `PalletTownDefaultScript`
(`scripts/PalletTown.asm`) return immediately, which skips the whole introduction, the starter
choice and the rival battle — the cheapest way to get a brand-new game walking.

---

## 5. The dice

| Symbol | Address | Notes | Grade |
|---|---|---|---|
| `hRandomAdd` | `$FFD3` | high byte of the 16-bit seed | cartridge |
| `hRandomSub` | `$FFD4` | low byte | cartridge |
| `wLinkBattleRandomNumberList` | — | 9 shared seeds, link battles only | symbol file |

`Random_` (`engine/math/random.asm`) is four instructions:

```
ldh a, [rDIV]  /  ld b, a  /  ldh a, [hRandomAdd]  /  adc b  /  ldh [hRandomAdd], a
ldh a, [rDIV]  /  ld b, a  /  ldh a, [hRandomSub]  /  sbc b  /  ldh [hRandomSub], a
```

It mixes in **`rDIV`**, the hardware divider register, on every call. The consequences are the
whole of what "reproducible" can mean for this game, and they are worked through in
[battle control](battle-control.md#making-a-cinematic-reproducible).

`BattleRandom` (`engine/battle/core.asm`) is `Random` *except* in a link battle, where it
draws from a shared nine-entry list advanced by `x*5+1` — a pure PRNG with no `rDIV` in it at
all. Both consoles must roll identically for a link battle to stay in sync, which is exactly
why it cannot use the divider.

These two bytes are also the reason the `Random_` signature is worth reading out of the
cartridge rather than trusting: it is five bytes of code containing both addresses as
operands, and it appears exactly once.

---

## 6. The save file

Cartridge RAM, `$A000-$BFFF`, banked. A `.sav` is **raw cartridge RAM, bank 0 first, no
header** — so a file offset *is* an SRAM offset, and SRAM bank 1 starts at file offset
`0x2000`. See [`src/mbc/battery.rs`](../../src/mbc/battery.rs).

| Symbol | Notes |
|---|---|
| `sMainData` | the block a continue restores, including the map header |
| `sPartyData` | the saved party, same layout as `wPartyCount` onward |
| `sMainDataCheckSum` | `CalcCheckSum`, `engine/menus/save.asm` — edit the save, fix this |

**This is the boundary that makes battle control safe.** WRAM (`$C000-$DFFF`) is scratch; the
save file is made from cartridge RAM. A tool that writes only WRAM cannot corrupt a `.sav`
however wrong it is, because the bytes never reach the medium the save is written from — the
player has to walk into a Pokémon Center and choose SAVE for that to happen. Battle control
enforces exactly that boundary in code (`wram::is_writable`), and the enforcement is tested by
pointing its own write door at `$A000` and checking the byte does not move.

---

## 7. Reading these from a plugin

The [plugin contract](../plugins.md) gives three reads and one write, all capability-gated:

```rust
machine.peek(addr)                 // one byte, no bus tick, cannot perturb timing
machine.peek_range(addr, len)      // a run
machine.peek_be16(addr)            // big-endian, which most Pokémon fields are
machine.peek_bank(bank, addr)      // an explicitly numbered ROM bank, whatever is mapped
machine.poke(addr, value)          // needs WRITE_MEMORY, refused otherwise
```

`peek_bank` matters because cartridge *tables* — base stats, species names, move data,
trainer rosters — live in banks the game maps only while it is using them. A plugin polling
once a second would otherwise read whatever happened to be there.

And the tables themselves are located by **byte signature** rather than by hard-coded offset:
[`plugins/gen1-link/src/romdata.rs`](../../plugins/gen1-link/src/romdata.rs) finds `BaseStats` by
Bulbasaur's record, `PokedexOrder` by its first six entries, `MonsterNames` by `RHYDON`, and
then cross-checks a *later* entry so a one-record misalignment fails loudly. That is what
keeps game data out of this repository: the numbers are read from the player's own cartridge.

---

## What this suggests next

Three things this map makes obviously possible that nobody has built yet:

* **The box.** `wBoxCount`/`wBoxMons` are `box_struct`s — the same layout as a party entry
  minus level and stats. Everything party-shaped on this page applies to storage.
* **The item bag.** `wNumBagItems`/`wBagItems` is a terminated `(item, quantity)` list. The
  same "snapshot, replace, restore" discipline battle control uses would give a scripted
  scenario its items.
* **Map and warp editing as a *feature*.** Redirecting `wWarpEntries` is already proven
  ([`examples/gen1_trade.rs`](../../examples/gen1_trade.rs)); generalising it would let a
  script put the player anywhere the game can legitimately walk to.

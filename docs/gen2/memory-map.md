# Where Pokémon Gold keeps what we have looked at

[← Gen 2 area](README.md) ·
[AtlasGB: memory map](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/memory-map.md)

> **Every game fact below is now also stated, independently, on
> [AtlasGB's own Gen 2 memory map](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/memory-map.md)**
> — the same move the Gen 1 area's memory map made first. The evidence-grade vocabulary here
> (*symbol file* / *walked* / *measured*) is restated in AtlasGB's own looser prose rather than
> copied verbatim, since there is no `apply-evidence.py` run behind that atlas yet. This page's
> job is now which of these bytes **this project's own tooling** (`examples/gen2_timecapsule.rs`)
> touches, and why.

Every address on this page is for **English Pokémon Gold**, and is graded — see
[the four grades](README.md#the-grades-with-one-added-and-one-missing). The short version:
*symbol file* means `pokegold` was built at RGBDS 1.0.3 and matched its own `roms.sha1`, so the
label's address is exact; *walked* means added up between two known addresses and landing on a
third; *measured* means the byte was read or written in a running cartridge and the game
behaved as its name says. There is a fourth grade in the Gen 1 area — *read out of the
cartridge* — and **nothing here has it**.

**This page is deliberately short.** It contains what the Time Capsule work needed and nothing
else. The gaps are listed at the bottom, by name, so that an absent structure reads as *absent*
rather than as *simple*.

Every constant below is transcribed in
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) with its symbol name.

---

## 1. The player and the party

| Symbol | Address | Size | Grade |
|---|---|---|---|
| `wPlayerID` | `$D1A1` | 2, **big-endian** | walked (see below) |
| `wPlayerName` | `$D1A3` | 11 | symbol file |
| `wPartyCount` | `$DA22` | 1 | symbol file + measured |
| `wPartySpecies` | `$DA23` | 6 + `wPartyEnd` (`$FF`) | symbol file + measured |
| `wPartyMons` | `$DA2A` | 6 × 48 | symbol file + measured |
| `wPartyMonOTs` | `$DB4A` | 6 × 11 | symbol file |
| `wPartyMonNicks` | `$DB8C` | 6 × 11 | symbol file + measured |

**The walk for `wPlayerID` is two bytes and it has a landing check.** It is not in the
transcribed constant list; the code reaches it as `wPlayerName − 2`, which is also exactly the
first byte of `wPlayerData`, the start of the first saved run
([AtlasGB's save file](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md#three-runs-not-one-base-plus-an-offset)). So the walk lands
on a boundary that is independently established by the save map — which is the check, and it is
the only reason a two-byte walk is worth writing down instead of just doing.

`PARTYMON_STRUCT_LENGTH` is **48**, not Gen 1's 44 — from `rgbasm` rather than a hand count (see
[building the disassembly](building-the-disassembly.md#3-deriving-a-constant-rather-than-counting-one)).

**The last three addresses check each other**, which is worth doing because they were transcribed
by hand out of a symbol table: `wPartyMons + 6 × 48 = $DA2A + $120 = $DB4A = wPartyMonOTs`, and
`wPartyMonOTs + 6 × 11 = $DB4A + $42 = $DB8C = wPartyMonNicks`. Three independently transcribed
numbers and two struct sizes agree, so a slipped digit in any one of them would show. Do this
whenever a table is transcribed rather than derived; it costs a minute, and transcription gets no
other check.

### The 48-byte `party_struct`, as far as this repository has established it

**This is not a transcription of `macros/ram.asm`.** It is the set of offsets that
`examples/gen2_timecapsule.rs` writes and reads, each of which was exercised by a real trade —
so the listed ones are *measured*, and the unlisted ones are honestly blank rather than filled
in from memory of another game.

```text
 0  Species        1     Gen 2 dex number (not Gen 1's internal index)
 1  Item           1     held item — 0, or the Time Capsule refuses the party
 2  Moves          4
 6  OTID           2     big-endian
 8  Exp            3     big-endian
11  HPExp          2 |
13  AttackExp      2 |
15  DefenseExp     2 |   stat experience, big-endian
17  SpeedExp       2 |
19  SpecialExp     2 |
21  DVs            2     4 nybbles: attack, defense, speed, special
23  PP             4
27  Happiness      1
28  ...            3     NOT ESTABLISHED HERE
31  Level          1
32  ...            2     NOT ESTABLISHED HERE
34  HP             2 |
36  MaxHP          2 |
38  Attack         2 |
40  Defense        2 |   big-endian
42  Speed          2 |
44  SpclAtk        2 |
46  SpclDef        2 |
```

**Four differences from Gen 1's 44-byte struct matter to anything that touches both**, and all
four are visible above:

1. **A held item exists** and sits at offset 1 — a concept Gen 1 does not have at all.
2. **Special is split** into Special Attack and Special Defence, so the struct is four bytes
   longer even though it drops nothing.
3. **Happiness exists** at 27, another Gen 2 concept.
4. **Level is at 31, not 33.** This is the classic way to produce a nonsense Pokémon when
   porting code between generations, and it is the same class of mistake as confusing Gen 1's
   `party_struct` with its `battle_struct` ([Gen 1 sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md#reading-state)).

**Multi-byte fields are big-endian**, exactly as in Gen 1, on a little-endian console.

The **DV pair at 21** is the field the whole Time Capsule chain exists to follow: it crosses the
wire unchanged, and it is what Gen 2's own `CheckShininess` reads. See
[AtlasGB's Time Capsule](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md#the-shiny-rule).

---

## 2. The map, and where the player is

| Symbol | Address | Notes | Grade |
|---|---|---|---|
| `wMapGroup` | `$DA00` | **a map is `(group, number)` in Gen 2, not one id** | symbol file + measured |
| `wMapNumber` | `$DA01` | | symbol file + measured |
| `wYCoord` | `$DA02` | | symbol file + measured |
| `wXCoord` | `$DA03` | | symbol file + measured |
| `wObjectStructs` (= `wPlayerStruct`) | `$D1FD` | `OBJECT_MAP_X` = 16, `OBJECT_MAP_Y` = 17 | symbol file |
| `wMapObjects` | `$D445` | the player is object 0; `MAPOBJECT_Y_COORD` = 2, `MAPOBJECT_X_COORD` = 3 | symbol file |
| `wNextWarp` | `$D043` | +1 `wNextMapGroup`, +2 `wNextMapNumber` | symbol file + measured |

**`wNextWarp` is the one that makes anything possible**, and it is worth understanding rather
than copying. It is *not* in `wPlayerData`, so it is runtime-only and never saved; `EnterMapWarp`
(`home/map.asm`) loads it from the warp event the player stepped onto and reads it back a few
instructions later. Overwrite it in that window and the player goes somewhere else through the
game's own full `MAPSETUP_DOOR` path — objects, camera and scene scripts all loaded properly.

**The window is instruction-wide, not frame-wide.** A per-frame poke loses the race about half
the time; the working version writes it after *every instruction* while the player is stepping
onto the warp tile. Measured, and the reason it is done that way is written into the code.

This is the Gen 2 answer to the Gen 1 trick of redirecting `wWarpEntries`, and it exists for the
same reason: **move the world's pointers, not the player.** Why editing the save instead does
not work is [AtlasGB's save file](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md#you-cannot-teleport-the-player-by-editing-the-save).

### Map ids and coordinates that have actually been used

Established by driving them, so these are *symbol file + measured* — but note they are facts
about two particular maps, not a table of any kind:

| Constant | Value |
|---|---|
| `GROUP_POKECENTER_2F` / `MAP_POKECENTER_2F` | 20 / 1 |
| `GROUP_TIME_CAPSULE` / `MAP_TIME_CAPSULE` | 20 / 4 |

Pokémon Center 2F is **one shared map** for every Center in the game, 8 × 4 blocks (16 × 8
tiles). Its warp 1 is the staircase at (0, 7); the Time Capsule receptionist stands at (13, 3)
with the Time Capsule warp at (13, 2) directly behind her (`maps/Pokecenter2F.asm`).

Inside the Time Capsule room, the two consoles are at (4, 4) and (5, 4) and are
**direction-restricted** bg events — (4, 4) fires only when the player faces *right* into it and
(5, 4) only when they face *left* (`maps/TimeCapsule.asm`). So the two seats are (3, 4) facing
right and (6, 4) facing left. Which one is yours is decided by a byte, not by geometry —
see §4.

---

## 3. Flags

| Symbol | Address | Notes | Grade |
|---|---|---|---|
| `wEventFlags` | `$D7B7` | `flag_array NUM_EVENTS` = 2048 bits = 256 bytes | symbol file + measured |
| `wDailyFlags1` | `$D968` | | symbol file + measured |

Two flag positions have been established, both because the Time Capsule turns on them:

| Constant | Value | Where it lands |
|---|---|---|
| `EVENT_MET_BILL` | 1810 | `wEventFlags` byte 226, bit 2 |
| `DAILYFLAGS1_TIME_CAPSULE_F` | 3 | `wDailyFlags1` bit 3 |

Both values come from `rgbasm`, not from counting the `const` chain. `ENGINE_TIME_CAPSULE` is
mapped into `wDailyFlags1` by `data/events/engine_flags.asm`; what makes it *daily* is that
`CheckDailyResetTimer` (`engine/overworld/time.asm`) zeroes `wDailyFlags1` once a day has
elapsed on the cartridge clock. Both flags are read the "wrong way round" from what you would
guess — see [AtlasGB's Time Capsule](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md#the-in-game-prerequisites).

---

## 4. The link

| Symbol | Address | Notes | Grade |
|---|---|---|---|
| `hSerialConnectionStatus` | **`$FFCD`** | Gen 1's is `$FFAA` — **they are different bytes and this cost a whole run** | symbol file + measured |

The values are shared with Gen 1 (`constants/serial_constants.asm`):
`USING_EXTERNAL_CLOCK` = `$01`, `USING_INTERNAL_CLOCK` = `$02`.

Reading it is how you find out which Time Capsule seat is free:
`TimeCapsuleSetWhichChrisCallback` puts the partner's avatar on one of the two seat tiles via
`CableClubCheckWhichChris` (`engine/link/link.asm`), which branches on this byte. Guess from the
arrival coordinate instead and the bot walks into the NPC and stands there — measured, for a
whole 60,000-frame run.

`wLinkTimeoutFrames` is `$2FF` = **767 frames** — a *value* established from the disassembly,
used to reason about the ~13-second window `WaitForLinkedFriend` holds open. **Its address is not
established here**, because nothing needed to read it.

Two more link symbols are cited on these pages by name with **no address**, deliberately, because
nothing here reads them: `wOtherPlayerLinkMode` (what `LinkReceptionistScript_Trade` branches on
to refuse a Gen 1 partner) and `wLinkMode`.

---

## 5. The screen

| Symbol | Address | Size | Grade |
|---|---|---|---|
| `wTilemap` | `$C3A0` | 20 × 18 | symbol file + measured |

**Same address as Gen 1's `wTileMap`, and the same property: a tile id is a character code**, so
the buffer reads back as the text a player can see. The menu cursor is tile `$ED`, and
`wMenuCursorY` does not mean what its name suggests. All of that is
[reading the screen](reading-the-screen.md).

**`wMenuCursorY` deliberately has no address on this page.** It has been measured to read `2`
while the cursor was visibly on row 4, so it is a byte this area tells you not to use; publishing
its address would invite exactly the mistake.

---

## 6. The save file

Cartridge RAM, banked, 8 KiB per bank. A `.sav` is **raw cartridge RAM, bank 0 first, no
header** (`src/mbc/battery.rs`), so bank *N* starts at file offset *N* × `0x2000`.

| Symbol | Address | Grade |
|---|---|---|
| `sGameData` (= `sPlayerData`) | bank 1 `$A009` | symbol file + measured |
| `sCurMapData` | bank 1 `$A856` | symbol file + measured |
| `sPokemonData` | bank 1 `$A88A` | symbol file + measured |
| `sGameDataEnd` / `sChecksum` | bank 1 `$AD69` | symbol file + measured |
| `sBackupChecksum` | bank 3 `$BE6D` | symbol file, **not exercised** |

The three-run mapping, the arithmetic that closes it, and the checksum are
[the save file](the-save-file.md). It is a page rather than a table because the *shape* of the
mapping is the thing worth carrying, not the four numbers.

---

## 7. What is not on this page

Named, so that a gap reads as a gap:

* **Battle structures.** No Gen 2 equivalent of `wBattleMon` / `wEnemyMon` / `wIsInBattle` /
  `wCurOpponent` has been established here. Nothing in this repository starts, reads or scores a
  Gen 2 battle.
* **Storage and the bag.** Boxes and the item list are untouched.
* **The map system proper** — map attributes (Gen 2's map header), tileset pointers, the block
  map buffer, the collision table, and the warp / bg-event / object-event tables. The open
  questions and a cost estimate are in
  [the Gen 1 authoring page's §7](../gen1/authoring-an-area.md#7-generation-2--the-work-not-a-guess).
* **The RNG.** Gen 2's `Random` has not been read here. **Do not assume Gen 1's behaviour** —
  everything this repository says about `rDIV` and reproducibility is a Gen 1 claim.
* **Wild encounter tables**, and whatever Gen 2 does about DVs at capture.
* **A run-time signature check** for any of the above. See
  [the grades](README.md#the-grades-with-one-added-and-one-missing).

---

## What this suggests next

* **Finish the party struct.** Offsets 28-30 and 32-33 are three and two bytes of a structure
  that is otherwise fully mapped and already exercised. Transcribing them from `macros/ram.asm`
  during a `pokegold` build closes the only hole in the one Gen 2 structure this repository
  actually uses.
* **`wMapGroup` / `wMapNumber` make a two-byte "where am I" test**, and every Gen 1 technique
  that keys off `wCurMap` gains a second byte rather than a rewrite. That is the cheapest bridge
  from Gen 1 tooling to Gen 2.
* **The signature check is the unglamorous high-value one.** Gen 1's `wram::verify` matches whole
  code *shapes* rather than short byte runs, because a four-byte prefix occurs hundreds of times
  in a megabyte. The same method transfers directly; what it buys is a Gen 2 tool that refuses a
  cartridge it does not recognise instead of corrupting it.

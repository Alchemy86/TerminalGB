# Sharp edges

[← Gen 1 area](README.md)

Everything here cost somebody hours. They are grouped by what you were trying to do when they
bit, because that is how you will be looking for them.

---

## Reading state

**Gen 1 species indices are not Pokédex numbers.** `MonsterNames` is indexed by *internal
index*, `BaseStats` by *Pokédex number*, and `PokedexOrder` converts. Mixing them gives you a
plausible, wrong Pokémon. `GetMonHeader` is the game's own conversion, and Mew is special-cased
because its base stats live outside the main table in Red and Blue.

**Every HP and stat is big-endian**, on a little-endian console. Read one the wrong way round
and you get a level 6400 Pokémon.

**The party structure and the battle structure are different.** `party_struct` is 44 bytes;
`battle_struct` is 29 and drops original trainer, experience and stat experience — so level and
the five stats sit at *different offsets*. See
[memory-map.md](memory-map.md#the-combatants-the-29-byte-battle_struct).

**`wIsInBattle == $FF` means the player blacked out**, not "trainer battle". `ram/wram.asm`
documents four values above the label (`-1` lost, 0 none, 1 wild, 2 trainer); a catch-all `_ =>
Trainer` arm swallowed the sentinel and put a battle panel — opponent, level, HP, stats, moves,
all stale garbage from the lost battle — on screen *in the overworld*, with the same confidence
as live data. **No test ROM catches this and none can**: the cartridge was entirely correct, and
the defect was in a consumer of emulated state.

> **Standing lesson:** a catch-all arm over a documented enum is a defect until the values it
> swallows are enumerated. The test that catches it iterates *every* byte, not the named cases.

**`wEnemyMons` shares storage with the map's wild-encounter tables.** It is a `UNION` in
`ram/wram.asm` with `wGrassMons`/`wWaterMons`. Reading an enemy party in the overworld returns
encounter data. Check `wIsInBattle` first.

**A trainer battle's Pokémon have fixed DVs.** `LoadEnemyMonData` gives every trainer Pokémon
`ATKDEFDV_TRAINER`/`SPDSPCDV_TRAINER` = `$98`/`$88`. Anything that reads DVs and draws a
conclusion from them must exclude trainer battles, or it will report the same answer forever.

---

## Writing state

**Well-formed is not the same as accepted.** The game cross-checks between structures.
`CheckForDisobedience` compares a party entry's `OTID` with `wPlayerID` and treats a mismatch as
a *traded* Pokémon, which a badgeless player is only obeyed by up to level 10 — so an injected
level 50 Charizard, otherwise perfect, answers "CHARIZARD won't obey!" and wastes the turn.
Full write-up: [battle control](battle-control.md#the-disobedience-trap).

**Half-old, half-new is worse than either.** When replacing a party, write *all six* slots —
zeroing the unused ones — rather than only the ones you care about. A structure that is part
old data is exactly the incoherent state that glitches.

**You cannot teleport the player by editing `wCurMap`.** `LoadSAV` sets `BIT_NO_PREVIOUS_MAP` on
`wCurMapTileset` right after restoring the main data, and `LoadMapHeader` sees that bit, clears
it, and *returns without loading the header*. The header is saved state, so a continue trusts
it. Change the map without the header and the game reads the new map's blocks through the old
map's `wCurMapDataPtr` — observed as `rst $38` or an illegal opcode within a second. Clearing
the bit in the save does not help; `LoadSAV` sets it *after* the copy. **Walk instead**, or
redirect the room's own warp (`wWarpEntries`) and let the first real warp do the full load.
Move the world's pointers, not the player.

**Patching state the engine is still writing is a race, and a delay does not fix it.** The
engine fills `wEnemyMon`/`wEnemyMons` inside `InitBattle`, which is not synchronised to a frame
boundary. Keep writing until the bytes read back unchanged at the *start* of a frame — that is
an observation that the engine has stopped; a delay is a guess that gets tuned until the
failures stop being reproducible.

**A battle with an empty party is an instant black-out** and looks exactly like a crash. Check
`wPartyCount` before triggering.

---

## Driving the game

**`wCurOpponent` is polled only by the overworld loop.** Write it while a menu or a text box is
open and nothing happens until the player has control again. The first time this bit, a scripted
run had mashed START and A alternately to get through the title screen — and the last START
press left the START menu open, so an armed battle waited forever. `NewBattle` also refuses on a
dungeon warp, while a cutscene is moving the player, and where a map script has set
`BIT_NO_BATTLES`.

**A and B are not interchangeable.** Both advance a text box, but **B also closes a menu and can
never open one**. So B is right after a message and wrong at a menu you have not finished with —
a B press at the move list backs out to the FIGHT menu, which is invisible unless something is
strict about what it expected. And an A press one frame after a box closes opens the START menu,
which then eats every direction key for the rest of the run.

**Gen 1's menu code does not see a button that arrives too soon.** `HandleMenuInput` delays
several frames after accepting one. Two presses eight frames apart look, from outside, exactly
like the emulator dropping input. Space presses by around half a second, and press at the *end*
of a wait rather than the start.

**After fleeing a battle the "Got away safely!" box is still up and swallows every direction
key.** Press through it before walking, or the next hundred steps move nothing.

**Pace movement by coordinate, not by alternating directions.** A press that only turns the
character costs a step and moves nothing, so blind alternation drifts.

**Oak stops a new game at the north exit of Pallet Town.** `PalletTownDefaultScript` opens
`CheckEvent EVENT_FOLLOWED_OAK_INTO_LAB / ret nz`, so setting that one bit — byte 0, bit 0 of
`wEventFlags` — skips the entire introduction, the starter choice and the rival battle.

**`wCurMap` is populated by `TryLoadSaveFile` at the top of the main menu**, long before
CONTINUE is chosen. It cannot mean "we are in the game".

---

## Reading the screen

**Gen 1 draws through the WINDOW layer, not the BG map.** Reading `$9800` returns 20×18 spaces
and every menu looks empty. Read `wTileMap` (`$C3A0`) in WRAM.

**A decoded screen is not a fixed-length string.** Some tiles expand to whole words (`$4A` is
`PKMN`, `$54` is `POKé`, `$5D` is `TRAINER`), so 360 tiles are not 360 characters. Slicing by
byte index panics on a multi-byte character. Decode row by row.

**Match with whitespace collapsed.** Rows are padded to twenty columns and messages wrap, so a
phrase is split by runs of spaces.

---

## Files and saves

**A `.sav` is raw cartridge RAM with no header.** A file offset is an SRAM offset; SRAM bank 1
starts at `0x2000`. The legacy `.gbsave` format put an 8-byte RTC epoch *in front* of the RAM
and silently shifted every offset by 8 — including for cartridge types that have no clock, so
every foreign save loaded wrong.

**Never point the emulator at `~/roms` in a test.** The MBC writes its `.sav` next to the ROM
path. Copy the ROM *and* any save into `target/` first. A real save has been destroyed here
once already.

**A hand-built save hangs the game.** Map, tileset, sprite and warp state are mutually
consistent state the game maintains together. Let the *game* write the save — play to the
overworld and use its own SAVE — then edit only the fields you need and fix `sMainDataCheckSum`
(`CalcCheckSum`, `engine/menus/save.asm`).

---

## Timing and determinism

**`BattleRandom` is seeded from `rDIV` and the frame counter**, so the species and DVs a
scripted run produces are **not** reproducible across emulation-timing changes. The core-timing
work (real timer, 5-cycle interrupt dispatch, timed OAM DMA) moved every one of them. Never pin
a DV value in a test; pin the *agreement* between what a reader says and what memory holds.

**Even a 100 %-accuracy move misses about one time in 256.** The accuracy check compares a
random byte against the scaled accuracy; the disassembly says so in a comment above the
comparison. Design a scripted sequence so no roll can change its outcome, and pin the starting
state so the whole run is reproducible.

**Reading memory from a plugin must not perturb the run.** `Machine::peek` deliberately does not
tick the bus, so an observer cannot advance the timer or the APU and change the very run it is
watching.

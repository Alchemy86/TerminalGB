# The Gen 2 save file, and why you cannot move the player with it

[← Gen 2 area](README.md)

A `.sav` is **raw cartridge RAM, bank 0 first, no header** (`src/mbc/battery.rs`), so bank *N*
starts at file offset *N* × `0x2000` and a bank/address pair *is* a file offset. That much is
the same for both generations.

Everything after that is not. Gen 1 saves one contiguous block and checksums it with a
complemented byte sum; Gen 2 saves **three separate runs**, checksums them with a 16-bit word
sum, keeps a **second copy**, and — the part that bites hardest — deliberately does *not* reload
map objects when you continue.

---

## 1. Three runs, not one base plus an offset

`SavePlayerData` / `SavePokemonData` (`engine/menus/save.asm`) copy three WRAM regions into
three SRAM regions. Mapping a WRAM address to a save offset is therefore defined **run by run**:

| WRAM run | WRAM range | SRAM destination | bank 1 offset |
|---|---|---|---|
| `wPlayerData` | `$D1A1` – `$D9ED` | `sPlayerData` (= `sGameData`) | `$009` |
| `wCurMapData` | `$D9EE` – `$DA21` | `sCurMapData` | `$856` |
| `wPokemonData` | `$DA22` – `$DF00` | `sPokemonData` | `$88A` |

Implemented as `save_offset` in
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs), which **panics on an
address outside all three runs** rather than computing a plausible offset for it. That is the
right shape: the failure you are defending against is a write that lands in the middle of
somebody else's data, and *the checksum will still pass*, because the checksum covers the whole
region and does not care which byte moved.

### The map closes exactly, and that is the proof

A run map is only trustworthy if it *lands* on something known — the same rule as a walk in the
[Gen 1 memory map](../gen1/memory-map.md). This one does, at both ends:

```text
first saved WRAM byte   $D1A1  ->  bank 1 $A009  =  sGameData        (a known symbol)
last  saved WRAM byte   $DF00  ->  bank 1 $AD68
                                   bank 1 $AD69  =  sGameDataEnd     (a known symbol)
                                                 =  sChecksum
```

3,424 bytes of WRAM into 3,424 bytes of SRAM, starting on `sGameData` and ending one byte
before `sChecksum`. Neither endpoint was chosen to make it work; both are symbol-file addresses,
and the fact that the three runs tile the gap between them exactly is what says the map is
right.

### The honest footnote nobody should discover the hard way

**On Gold, these three runs are contiguous on both sides — so the arithmetic coincides with a
single base plus offset.** Check it: `$D9EE − $D1A1 = $84D` and `$009 + $84D = $856`;
`$DA22 − $D9EE = $34` and `$856 + $34 = $88A`. Both destinations fall out of the previous run's
length.

So the standing advice — *"a Gen 2 save offset is not one base plus an offset"* — is a claim
about the **structure**, not about today's arithmetic. Two consequences, and they point in
opposite directions:

* Do not "simplify" `save_offset` into one subtraction. The game copies three runs; if a future
  cartridge, revision or generation moves one of them, the three-run form is right and the
  single-base form is silently, checksum-passingly wrong.
* Equally, do not claim the three-run form has been *proved necessary here*. It has not — it has
  never diverged from the shortcut on this cartridge, which also means a bug in it would not
  have been caught. It is defensive, and saying so is more useful than implying it earned its
  place by catching something.

That distinction is exactly why this area grades things. "Structurally correct" and "measured to
differ" are different claims.

---

## 2. The checksum is verified, not tolerated

`SaveChecksum` / `Checksum` (`engine/menus/save.asm`): a **16-bit sum of every byte** of
`sGameData`, stored **little-endian** at `sChecksum` (bank 1 `$AD69`). Edit anything inside the
saved region and recompute it, or the game rejects the save outright — this is not a soft
warning that leaves you playing with slightly wrong data.

Note the contrast with Gen 1, which uses a *complemented 8-bit* sum (`CalcCheckSum`,
`pokered/engine/menus/save.asm`). Two generations, two algorithms, and the same trap: a tool
that carries one forward silently produces a file the game will not load.

### `sBackupChecksum` — established, and honestly not exercised

Gold keeps a **second copy of the save** and consults it when the first fails, with its own
checksum at `sBackupChecksum` (bank 3 `$BE6D`). The address is symbol-file grade and is declared
in the code.

**It is not written, and the crafted save loads anyway.** So what this repository has actually
established is narrower than the mechanism: patching `sGameData` and its own checksum is enough
for the game to accept the save. *When* the backup is consulted, and whether a mismatched backup
can bite later, is **not established here**. A Gen 2 save editor that goes further than this one
should find out before it ships, because "loaded fine in my test" is exactly what a stale backup
copy would look like right up until it is not.

---

## 3. You cannot teleport the player by editing the save

Both generations refuse this, for different reasons, and the Gen 2 reason is the more surprising
one.

**`MapSetupScript_Continue`'s second command is `LoadMapAttributes_SkipObjects`.** A continue
deliberately does not load map objects, because it assumes you are on the map you saved on. So
editing `wMapGroup` / `wMapNumber` in the save lands you on the right map with the *previous*
map's NPCs: nobody to talk to, a stale camera, and a room that draws correctly and does nothing.
Measured — exactly that.

And it does not help to move the player object as well, though it is tempting: `wObjectStructs`
and `wMapObjects` are **inside the saved region** ([memory map §2](memory-map.md#2-the-map-and-where-the-player-is)),
so you *can* reach them. Measured, the camera is still wrong afterwards. **Why** it is still
wrong — what else derives from the skipped object load — was not chased and is **not established
here**, because the correct route was already obvious by then. Chasing it is how you end up
hand-building a whole consistent world state, which is the Gen 1 lesson about hand-built saves in
a new costume.

**Do this instead: override the warp destination at run time.** Walk the player to a real
staircase or door and overwrite `wNextWarp` / `wNextMapGroup` / `wNextMapNumber` as they step
onto it. The game then takes the full `MAPSETUP_DOOR` path and sets up objects, camera and scene
scripts itself, because as far as it is concerned an ordinary warp fired.

**Write it between instructions, not once a frame.** `EnterMapWarp` (`home/map.asm`) loads
`wNextWarp` from the warp event and reads it back a handful of instructions later; a per-frame
poke loses that race roughly half the time, which presents as an override that "works
sometimes". The one-off tools here step the CPU instruction by instruction and rewrite the three
bytes after each one — expensive, and the right trade for certainty in a tool that runs once.

> **The general principle, and it is the same one as Gen 1's `wWarpEntries` trick:** move the
> world's pointers, not the player. A map transition is a large, mutually-consistent state
> change that the engine already knows how to perform. Make it perform one.

---

## 4. Craft by playing, then edit

**Let the game write the save, then patch it.** `--make-save` in
[`examples/gen2_timecapsule.rs`](../../examples/gen2_timecapsule.rs) plays the intro with a bot,
drives the START menu to SAVE, and only then edits the four things it needs on top. Hand-building
a whole save is how you get a file that looks right in a hex dump and hangs on load — Gen 1
taught this repository that twice, and Gen 2 has strictly more mutually-consistent state to get
wrong.

Two details of that bot worth keeping:

* **The naming screen needs START, not A.** Mashing A there types letters forever. Gen 1's agent
  work paid for this trap first; it is the same screen shape in Gen 2.
* **Find the SAVE entry by looking at the cursor on screen**, not by counting menu positions —
  `wMenuCursorY` lies. See [reading the screen](reading-the-screen.md#2-the-menu-cursor-is-a-tile-not-a-variable).

---

## 5. Safety, in one rule

The rule this repository applies everywhere: **stage the ROM and the save into `target/` before
touching either.** The MBC writes `<rom>.sav` beside the cartridge it was handed, so a tool
pointed at `~/roms` writes there. A real save has been destroyed here once already.

Note the extension is *replaced*, not appended: `game.gbc` saves to `game.sav`. Staging a save
as `game.gbc.sav` silently boots a fresh game — whose main menu has no CONTINUE, at which point
a screen-driven bot loops in the intro forever and looks like it is broken.

---

## What this suggests next

* **A Gen 2 save reader is now cheap.** The run map, the party struct and the checksum are all
  established; a read-only "what is in this Gold save" tool is an afternoon and needs no new
  derivation.
* **Settle the backup save.** One experiment — corrupt `sBackupChecksum`, load, and see whether
  anything notices — turns §2's honest gap into a fact, and any Gen 2 save editor needs the
  answer.
* **The three-run map is the pattern, not the numbers.** Any generation that copies WRAM into
  SRAM in pieces wants the same treatment: a per-run mapping that refuses an address it does not
  own, and an endpoint check that proves the runs tile the region exactly.

# Sharp edges

[← Gen 1 area](README.md) ·
[AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md)

**Most of this page moved.** Every trap here that was a fact about the *cartridge* — reading
state, writing state, driving the engine, the screen, the timing — now lives, adapted and with
more citation than the original had, on AtlasGB's own
[sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md)
page, alongside its
[discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md)
(the reasoning trail behind each one). AtlasGB owns documenting the game; this repository
produces the evidence. What is left below is specific to driving *this* emulator and its own
tooling, not to the cartridge.

---

## Writing state

**Half-old, half-new is worse than either.** When replacing a party through this emulator's own
injection tools, write *all six* slots — zeroing the unused ones — rather than only the ones you
care about. A structure that is part old data is exactly the incoherent state that glitches.

**Patching state the engine is still writing is a race, and a delay does not fix it.** The
engine fills `wEnemyMon`/`wEnemyMons` inside `InitBattle`, which is not synchronised to a frame
boundary. Keep writing until the bytes read back unchanged at the *start* of a frame — that is
an observation that the engine has stopped; a delay is a guess that gets tuned until the
failures stop being reproducible.

## Files and saves

**Never point the emulator at `~/roms` in a test.** The MBC writes its `.sav` next to the ROM
path. Copy the ROM *and* any save into `target/` first. A real save has been destroyed here
once already.

**The `.sav` checksum is over `sGameData..sGameDataEnd` (`$A598..$B522`), not over
`sMainData`.** Both ranges look plausible and only one is right. A wrong checksum is not
reported as one: the game simply offers **NEW GAME instead of CONTINUE**, which reads as a
corrupt save. `CalcCheckSum` (`pokered engine/menus/save.asm`) is the complement of the byte sum.
[`tools/gen1-save/gen1_save.py`](../../tools/gen1-save/gen1_save.py) reads a save — player,
Pokedex, story flags, checksum verdict — and moves a single event flag with the checksum repaired
for you; booting the result and looking for CONTINUE is the cheap confirmation.

**A save assembled by a harness can be in a state the game's own scripts assume is impossible,
and it loads fine.** Seen on a real save: `EVENT_GOT_POKEDEX` set while Oak's Parcel was still in
the bag, which `scripts/OaksLab.asm` sets on consecutive lines of one cutscene, so no playthrough
reaches it. The checksum was valid and Oak refused the parcel for ever. When an NPC "won't do the
thing", suspect the flag pair before the script.

**The legacy `.gbsave` format put an 8-byte RTC epoch *in front* of the RAM**, silently shifting
every offset by 8 — including for cartridge types that have no clock, so every foreign save
loaded wrong. `.sav` (raw cartridge RAM, no header) is the only format this emulator writes now;
`.gbsave` is read once, for migration, and never written.

## Timing and determinism

**Reading memory from a plugin must not perturb the run.** `Machine::peek` deliberately does not
tick the bus, so an observer cannot advance the timer or the APU and change the very run it is
watching.

---

## See also

- [AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md) —
  every trap that is a fact about the cartridge, and the discoveries each one links to.
- [Discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) —
  the reasoning trail behind the faults and corrected beliefs this project found in Pokémon Blue.

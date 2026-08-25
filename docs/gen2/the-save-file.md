# The Gen 2 save file, and driving it into a state worth testing

[← Gen 2 area](README.md) ·
[AtlasGB: the save file](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md)

> **The save-file format itself moved to AtlasGB** — the three-run map, the checksum algorithm
> and the reason you cannot teleport the player by editing the save are now on
> [AtlasGB's `the-save-file.md`](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md).
> What is left here is this repository's own recipe for getting a *real* save to test against.

A `.sav` is **raw cartridge RAM, bank 0 first, no header** (`src/mbc/battery.rs`), so bank *N*
starts at file offset *N* × `0x2000` and a bank/address pair *is* a file offset. That much is
the same for both generations, and the rest — the three-run map, the 16-bit checksum, the
map-object-skip trap — is AtlasGB's now.

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
  `wMenuCursorY` lies. See [reading the screen](reading-the-screen.md#do-not-trust-a-wram-variable-that-looks-like-it-should-hold-the-cursor).

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
  anything notices — turns
  [AtlasGB's own honest gap](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/the-save-file.md)
  into a fact, and any Gen 2 save editor needs the answer.
* **The three-run map is the pattern, not the numbers.** Any generation that copies WRAM into
  SRAM in pieces wants the same treatment: a per-run mapping that refuses an address it does not
  own, and an endpoint check that proves the runs tile the region exactly.

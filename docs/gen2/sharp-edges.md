# Sharp edges — Generation 2 (this repository's own tooling traps)

[← Gen 2 area](README.md) ·
[AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/sharp-edges.md)

> **Most of this page moved.** Every trap that is a fact about the cartridge — the save
> structure, the teleport prohibition, the dialog-dismissal inversion, the party-structure
> offsets, the Time Capsule's whole-party rejection, the catch-rate-becomes-item quirk, the "no
> Gen 1 shiny" rule — is now on
> [AtlasGB's `sharp-edges.md`](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/sharp-edges.md),
> split section by section from the tooling advice below. **Also read
> [the Gen 1 sharp edges](../gen1/sharp-edges.md)** if you are writing anything that touches both
> — two of the traps there are *inverted* in Gen 2.

---

## Deriving addresses

**A symbol's address is a claim about a build.** It is only exact for a cartridge if the
disassembly reproduced that cartridge byte for byte. Build `pokegold` at RGBDS **1.0.3** and
check it against its own `roms.sha1` before trusting a single number
([how](building-the-disassembly.md)). RGBDS 0.6.1 is too old and refuses; a *newer* toolchain is
the dangerous case, because it may assemble cleanly and shift a section.

**Ask the assembler for constants; do not count `const` chains.**
`rgbasm -Q8 -P includes.asm -D _GOLD` with `PRINTLN "{d:SYMBOL}"`. `EVENT_MET_BILL` is index
1810 in a chain hundreds of lines long, and an off-by-one there produces a flag edit that lands
on a different event and looks like nothing happened.

**Nothing in this repository verifies a Gen 2 address against the cartridge at run time.** Gen 1
tooling re-derives its write addresses out of the ROM image at attach time and refuses a
cartridge it does not recognise; Gen 2 has no equivalent. So a Gen 2 tool handed Silver, Crystal,
a translation or a hack will write to whatever the address happens to be. **Do not ship one
without adding the check** — it is named as the highest-value gap in
[the area README](README.md#what-is-not-established-about-gen-2-here).

---

## Editing a save — this repository's own discipline, not the format itself

**A hand-built save hangs the game.** Let the *game* write the save — play to a save point and use
its own SAVE menu — then edit only the fields you need and fix the checksum. Gen 2 has strictly
more mutually-consistent state to get wrong than Gen 1 does.

**The save filename replaces the extension, it does not append.** `game.gbc` saves to
`game.sav`. Staging a save as `game.gbc.sav` silently boots a fresh game, whose main menu has no
CONTINUE — at which point a screen-driven bot loops in the intro forever and looks broken.

**Never point the emulator at `~/roms` in a test.** The MBC writes `<rom>.sav` beside the ROM it
was handed. Stage the ROM *and* the save into `target/` first. A real save has been destroyed
here once already.

**Write a warp override between instructions, not once a frame.** `EnterMapWarp` reads it back a
handful of instructions after loading it, so a per-frame poke loses the race about half the time
— which presents as an override that "works sometimes". (What it overrides, and why editing the
save cannot do this job, is now on AtlasGB's own save-file and sharp-edges pages.)

---

## Driving the game

**`wMapGroup` reads noise on frame 0** (observed: map `127:211`), so any sanity test needs a
*stable, in-range* value across several frames, not a single sample. (Gen 1 has the identical
trap with `wCurMap`, populated by `TryLoadSaveFile` at the top of the main menu.)

**A word list is the wrong test for "does the game have control yet".** Continuing a real save
produced a `PLAYER <name>` box, then *"The clock's time may be wrong."*, then the entire clock-set
screen — three consecutive screens containing none of the words a reasonable list would hold.
Use the property instead: the Gen 2 overworld renders **no font glyphs at all**, so *any letter on
the decoded tilemap means a menu or dialog owns the joypad*.
[Why that is the right shape](reading-the-screen.md#a-word-list-is-the-wrong-test-and-the-overworld-proved-it).

**The naming screen needs START, not A.** Mashing A there types letters forever. Gen 1's agent
work paid for this trap first; it is the same screen shape in Gen 2.

**Re-derive navigation state every tick.** A stored "I am beside her, now face up" goes stale the
moment a press moves the player, and the bot then faces a wall forever. Compute the target from
the current position each time; walking back is one branch.

**Once a script has warped you somewhere, stop navigating.** A bot that keeps steering after the
Time Capsule receptionist sends it through the door walks straight back out of the link —
measured, mid-handshake.

---

## Linking

**Two separate processes cannot implement the Time Capsule's rendezvous gate**, because neither
can see the other's game state — [the table](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/time-capsule.md#which-seat-is-yours-is-a-byte-not-geometry)
covers what has to line up. The completed trade here is one process holding one bot back.

**The trade selection screen eats direction keys as cursor movement.** Any seat-facing or
walking logic must stop the moment `CANCEL` is on screen, or its presses walk the selection onto
the wrong Pokémon.

---

## What has *not* bitten yet, because nobody has been there

Stated so an empty section does not read as a safe one. Nothing in this repository starts,
reads or scores a **Gen 2 battle**; nothing touches the **box** or the **bag**; nothing has read
Gen 2's **RNG**, so no claim this repository makes about `rDIV` or reproducibility applies to it;
and nothing has established the **map system**. Expect this page to grow the first time somebody
goes there. See [the area README](README.md#what-is-not-established-about-gen-2-here) and
[AtlasGB's own gap list](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-gs/docs/sharp-edges.md).

# Sharp edges — Generation 2

[← Gen 2 area](README.md)

Everything here cost a run, and several cost a day. They are grouped by what you were trying to
do when they bit, because that is how you will be looking for them.

**Read [the Gen 1 sharp edges](../gen1/sharp-edges.md) as well** if you are writing anything that
touches both. Several traps are shared; two are *inverted* between the generations, and those are
called out below, because carrying a Gen 1 habit into Gen 2 is the specific way to get hurt.

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

**A Gold symbol is not a Crystal symbol.** Crystal is cited in this repository for *rules* — the
shiny test, Hidden Power, the type table — and never for addresses.

---

## Editing a save

**A Gen 2 save is three WRAM runs into three SRAM runs.** Map an address run by run, and make the
mapper *refuse* an address outside all three rather than compute a plausible offset for it. The
failure being defended against is a write landing in the middle of somebody else's data, and
**the checksum still passes** because it covers the whole region.

**On Gold the three runs happen to be contiguous on both sides**, so the arithmetic coincides with
a single base plus offset. That is a coincidence of this cartridge's numbers, not a licence to
simplify — and it also means a bug in the three-run form would not have been caught here.
[The full working](the-save-file.md#the-honest-footnote-nobody-should-discover-the-hard-way).

**The checksum is a 16-bit word sum, little-endian, and a stale one is rejected outright** — not
a warning, not a soft failure. **Gen 1's is a complemented 8-bit sum.** Carrying either algorithm
into the other generation produces a file the game refuses to load, which at least fails loudly.

**Gold keeps a second copy of the save with its own checksum** (`sBackupChecksum`). This
repository does not write it, and the crafted save loads anyway — so *when* the backup is
consulted is **not established here**. "Loaded fine in my test" is exactly what a stale backup
looks like until it is not.

**You cannot teleport the player by editing the save.** `MapSetupScript_Continue`'s second command
is `LoadMapAttributes_SkipObjects`: a continue does not load map objects, because it assumes you
are on the map you saved on. Editing `wMapGroup` lands you on the right map with the *previous*
map's NPCs, nobody to talk to and a stale camera. Moving the player object as well does not fix
it. **Override `wNextWarp` / `wNextMapGroup` / `wNextMapNumber` at run time instead** and let a
real warp do the full load. (Gen 1 has the same prohibition for a different reason — there it is
`LoadSAV` setting `BIT_NO_PREVIOUS_MAP`.)

**Write the warp override between instructions, not once a frame.** `EnterMapWarp` reads it back a
handful of instructions after loading it, so a per-frame poke loses the race about half the time
— which presents as an override that "works sometimes".

**A hand-built save hangs the game.** Let the *game* write the save — play to a save point and use
its own SAVE menu — then edit only the fields you need and fix the checksum. Gen 2 has strictly
more mutually-consistent state to get wrong than Gen 1 does.

**The save filename replaces the extension, it does not append.** `game.gbc` saves to
`game.sav`. Staging a save as `game.gbc.sav` silently boots a fresh game, whose main menu has no
CONTINUE — at which point a screen-driven bot loops in the intro forever and looks broken.

**Never point the emulator at `~/roms` in a test.** The MBC writes `<rom>.sav` beside the ROM it
was handed. Stage the ROM *and* the save into `target/` first. A real save has been destroyed
here once already.

---

## Driving the game

**⚠️ Inverted from Gen 1: dismiss dialogs with A, never B.** Measured — B backs out through the
continue menu to the title screen and the run is over. In Gen 1, B is the *safe* dismissal
because it closes menus and can never open one. The two generations disagree; this is a per-game
fact, not transferable knowledge.

**`wMapGroup` is loaded into WRAM *before* the continue panel is dismissed.** So it cannot mean
"we are in the game": stop pressing A at that panel because the map bytes look plausible and the
game sits there forever, looking exactly like a corrupt save. It also reads as noise on frame 0
(observed: map `127:211`), so any sanity test needs a *stable, in-range* value, not a single
sample. (Gen 1 has the identical trap with `wCurMap`, populated by `TryLoadSaveFile` at the top of
the main menu.)

**A word list is the wrong test for "does the game have control yet".** Continuing a real save
produced a `PLAYER <name>` box, then *"The clock's time may be wrong."*, then the entire clock-set
screen — three consecutive screens containing none of the words a reasonable list would hold.
Use the property instead: the Gen 2 overworld renders **no font glyphs at all**, so *any letter on
the decoded tilemap means a menu or dialog owns the joypad*.
[Why that is the right shape](reading-the-screen.md#1-the-overworld-contains-no-letters-at-all--and-that-is-a-better-test-than-a-word-list).

**`wMenuCursorY` does not tell you where the cursor is.** Measured: it read `2` for the save menu
while the cursor sat visibly on row 4. **Find tile `$ED` on the tilemap** — the cursor glyph
appears exactly once on an open menu.

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

**⚠️ `hSerialConnectionStatus` is `$FFCD` on Gen 2 and `$FFAA` on Gen 1.** Same name, same
purpose, different byte. Reading the Gen 1 address on a Gen 2 cartridge gives you an unrelated
value that is often plausible.

**Which Cable Club seat is free is not geometry.** The partner's avatar occupies one of the two
seat tiles, and which one is decided by `hSerialConnectionStatus` via
`CableClubCheckWhichChris`. Guessing from the arrival coordinate walked a bot into the NPC, where
it stood blocked for a full 60,000-frame run while the partner's screen said *"PLEASE WAIT!"* —
with no error printed anywhere. [The table](time-capsule.md#c-which-seat-is-yours-is-a-byte-not-geometry).

**A Gen 2 console answers handshake bytes from its overworld serial handler.** So a Gen 1 partner
that initiates while the Gen 2 player is still walking around completes a handshake against
nothing useful, enters the Trade Center alone, and desyncs. The Gen 2 side must be *at its
receptionist* first — the window is `wLinkTimeoutFrames` = 767 frames, about 13 seconds, and both
sides retry on a miss, so one gate is the entire choreography.

**Two separate processes cannot implement that gate**, because neither can see the other's game
state. The completed trade is one process holding one bot back.

**The trade selection screen eats direction keys as cursor movement.** Any seat-facing or
walking logic must stop the moment `CANCEL` is on screen, or its presses walk the selection onto
the wrong Pokémon.

**Do not take a Gen 1 partner to Gen 2's ordinary trade desk.** `LinkReceptionistScript_Trade`
reads `wOtherPlayerLinkMode` and answers *"You can't link to the past"*. The Time Capsule desk is
the only door.

---

## Reading and writing Pokémon

**⚠️ The Gen 2 party struct is 48 bytes and level is at offset 31.** Gen 1's is 44 bytes with
level at 33. Porting a reader between generations without moving that offset is the classic way
to produce a nonsense Pokémon — the same class of mistake as confusing Gen 1's `party_struct`
with its `battle_struct`.

**Special is split** into Special Attack and Special Defence, and **a held item exists** at offset
1. Neither concept is in Gen 1 at all.

**Multi-byte fields are big-endian** in both generations, on a little-endian console.

**The Time Capsule rejects the whole party, not the offending Pokémon.** Species ≥ 152, any move
above `STRUGGLE`, or any held mail, and the receptionist refuses the lot. **All three Johto
starters are Gen 2 species**, so a normally-played Gold save is very likely to be refused for its
party even when both Time Capsule flags are already open — which is exactly what a real cartridge
turned out to do here, and it is easy to misdiagnose as a flag problem.

**A Pokémon arriving from Gen 1 holds an item whose id is its Gen 1 catch rate.** The Abra came
across holding item 200; Abra's Gen 1 catch rate is 200. Documented Time Capsule behaviour, not a
conversion bug.

**Gen 1 has no shiny Pokémon.** Anything derived from DVs about shininess is *"would be shiny if
traded to Gen 2"*, never *"is shiny"* — and there is one implementation of that rule in this
repository, deliberately.

---

## What has *not* bitten yet, because nobody has been there

Stated so an empty section does not read as a safe one. Nothing in this repository starts,
reads or scores a **Gen 2 battle**; nothing touches the **box** or the **bag**; nothing has read
Gen 2's **RNG**, so no claim this repository makes about `rDIV` or reproducibility applies to it;
and nothing has established the **map system**. Expect this page to grow the first time somebody
goes there. See [the area README](README.md#what-is-not-established-about-gen-2-here).

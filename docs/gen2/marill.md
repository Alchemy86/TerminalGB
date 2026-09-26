# A Pokémon out of a Gold cartridge: picture, cry and entrance

[← Gen 2 area](README.md)

**What a Generation 2 Pokémon actually is, on the cartridge, and what its entrance actually
does — both read off a real Pokémon Gold and neither taken on trust.** The worked example is
Marill, `#183`, because that is what was asked for; nothing on this page is specific to it.

The tool is [`examples/gen2_marill.rs`](../../examples/gen2_marill.rs):

```bash
cargo run --release --features image,chiptune --example gen2_marill -- \
    --rom ~/roms/gold.gbc --save ~/roms/goldsave/gold.sav
```

Without `--save` it reads the cartridge and stops. With one, it puts a Marill in a **copy** of
that save, boots the game and drives it — the save on disk is never touched, and neither is the
directory the ROM lives in, because the MBC writes `<rom>.sav` beside whatever cartridge it is
handed.

---

## 1. The grade this area did not have

[The area README](README.md#the-grades-with-one-added-and-one-missing) grades every Gen 2
address and names the one grade nothing here had: **read out of the cartridge** — located in the
ROM image at run time instead of transcribed out of a symbol file. It called building it "the
highest-value piece of unglamorous work this area could gain", because it is the only grade that
needs no build and no trust, and the only one that lets a tool **refuse** a cartridge it does not
recognise.

This is that, for four tables. The method is
[`plugins/gen1-link/src/romdata.rs`](../../plugins/gen1-link/src/romdata.rs)'s, unchanged: find
each table by a distinctive byte signature, then cross-check it against something a
misalignment would break.

| table | found by | measured at |
|---|---|---|
| `BaseData` | Bulbasaur's record — dex 1, 45/49/49/45/65/65, GRASS/POISON, catch 45, exp 64 | `$051B0B` |
| `PokemonNames` | `BULBASAUR` *and* `IVYSAUR` together — one name alone also appears in the Pokédex text, but the 10-byte field padding makes the pair unmistakable | `$1B0B74` |
| `PokemonPicPointers` | **the hole Unown leaves in it** (below) | `$048000` |
| `PokemonPalettes` | Bulbasaur's whole 8-byte record; verified against colours measured off the running cartridge (§5) | `$00AD45` |
| the picture banks | derived, not assumed (below) | — |

### Unown is what finds the picture table

Unown's 26 forms live in their own table, so species 201's pair of entries in the main one is
`FF FF FF FF FF FF`. Finding six `$FF` bytes and asking whether a table whose blank pair sits
exactly `2 × 200` entries in is a table — every other entry a bank byte and a `$4000..$7FFF`
address — settles the base, the alignment *and* the front/back interleave in one question. It
matches once in the ROM.

### The bank byte is not the bank, and the correction is derived

An entry is `bank, address`. The address is a plain bank-window address; the bank byte is not
the ROM bank for every entry. Measured on Gold:

| bank byte | ROM bank |
|---|---|
| `$12`, `$15`–`$1E` | itself |
| `$13` | `$1F` |
| `$14` | `$20` |
| `$1F` | `$2E` |

The engine fixes the bank up before it pages it in — the Crystal disassembly calls the
equivalent routine `FixPicBank` — and **that routine has not been located in this cartridge**, so
nothing above is transcribed from anywhere. It is *derived*: for each bank byte the table uses,
every candidate ROM bank is tried, and the one that decompresses **every** picture carrying that
bank byte to exactly the size its own base-stat record predicts is taken. Anything short of
unanimous is an error, not a guess.

Crystal, for contrast, is tidy: its table is at `$120000` (bank `$48`) and every bank byte is
exactly `$36` under the real bank — `PICS_FIX`, the constant the Crystal disassembly names.
Gold's arrangement is not that, and why is an open question worth an hour to somebody with the
disassembly open.

### The check that makes it worth believing

**All 250 front pictures decompress to exactly the size their own base-stat record predicts, and
all 250 back pictures to exactly 6×6 tiles.** A mislocated table, a misaligned index, a wrong
bank or a wrong decompressor cannot survive 500 exact length agreements. The tool runs that
check every time it opens a cartridge, in about 40 ms, and refuses the cartridge if it fails —
which is exactly what it does when handed Crystal.

---

## 2. Generation 2 is not Generation 1 with different numbers

Everything a Gen 1 tool knows about a Pokémon is wrong here except the idea:

| | Red/Blue | Gold |
|---|---|---|
| species numbering | internal index ≠ Pokédex number, 190 slots with 39 holes | **internal index *is* the Pokédex number**, 251 of them |
| base-stat record | 28 bytes, Mew held apart from the table | **32 bytes**, no special case |
| picture size | a header byte at the head of the compressed stream | **a nibble pair in the base-stat record** |
| picture compression | `pkmncompress` — two delta-coded bitplanes | **a byte-oriented LZ**, a different algorithm |
| picture pointers | two pointers inside the base-stat record | **one table, front and back alternating** |
| colour | none — a Game Boy game has no per-species palette to have | **two RGB555 colours per species, and two more for its shiny** |

So [`plugins/trainer/src/sprite.rs`](../../plugins/trainer/src/sprite.rs), this repository's Gen 1
decompressor, cannot read a Gen 2 picture at all. The Gen 2 decompressor is a command stream:
`literal`, `iterate`, `alternate`, `zero`, `repeat`, `flip`, `reverse`, with a long form that
carries a 10-bit length, and `$FF` to end. `repeat` may read the bytes it is itself writing,
which is how the format expresses a run.

---

## 3. Marill, read off the cartridge

```
#183 MARILL — base [70, 20, 50, 40, 20, 50], type $15/$15, catch 190, base exp 58,
growth 4, egg groups $26, gender $7F, 20 hatch cycles, front picture 5x5 tiles
```

Both pictures come out as PNGs. The cartridge then confirms the reading itself: a level 10
Marill with perfect DVs is crafted into a copy of the save from *these* numbers, and the game's
own status page prints `37/ 37` HP, `800` experience and `LEVEL UP TO 11` in `264` more — which
is `4n³/5` at 10 and 11, the growth-rate byte read out of `BaseData`, checked by the machine
that owns it.

---

## 4. The cry is the cartridge's own

There is no interface on a Game Boy for "play a cry". There is a game that plays one, so the
tool drives it: boot the crafted save, open the start menu, choose `POKéMON`, choose `STATS`,
and record what the APU produces from the moment `STATS` is confirmed.

The clip is five seconds at 44.1 kHz. Measured on the run behind this page: the map music sits
at about 0.05–0.09 RMS, and Marill's cry is the burst from roughly 750 ms to 1350 ms that lifts
it to 0.275, peaking at 0.65. **The music is in the clip**, because the music is what the
cartridge was playing; nothing here mutes it, and a clip that claimed to be the cry alone would
be a claim this tool cannot make.

Two things the driver has to get right, both of them already written down here:

* A machine with **no** `AudioPlayer` has no sound chip rather than a quiet one, and real
  cartridges hang on that. The recorder is installed before the first frame and counts its own
  calls, so "the cartridge was silent" and "the APU never ran" stay different findings.
* It only records while **armed**. The run walks the player around for as long as it takes to
  find tall grass, and half an hour of emulated audio is hundreds of megabytes of sample pairs.

---

## 5. Colour: Generation 2 Pokémon have real palettes, and they are in the cartridge

`PokemonPalettes` at **`$00AD45`**. Eight bytes a species: two RGB555 colours for the normal
palette, two more for the shiny one. Only two, because the other two never vary — a Pokémon's
four shades are white, colour 1, colour 2, black.

This is a real difference from Generation 1, not a smaller version of it. A Game Boy game has no
per-species palette to have, so the Pokéwalker project's Generation 1 importer invents a
[ramp](https://github.com/Alchemy86/TerminalGB) per species and says so plainly. **A Generation 2
Pokémon needs no ramp invented for it.** Marill is `(86, 136, 186)` and `(173, 58, 73)` — a blue
body with red inner ears and a red tail ball — and those are the artist's numbers, not anyone's
guess.

**How it was found, and why it is not circular.** The colours were measured *first*, off the
emulator's own framebuffer with Marill on screen, and then used as the signature to find the
table. Inverting the Game Boy Color correction this emulator applies
([`Gpu::setrgb`](../../src/gpu/mod.rs), the Gambatte curve) gives exactly one RGB555 pair for
each: `$75A9` and `$1519`. Searching the ROM for those four bytes matches **once**, at
`$00B2F5` — and `$00B2F5 − 182 × 8 = $00AD45`, so Marill is entry 183, where every other table
in this cartridge also puts it.

Two checks then make it worth believing:

* **Every one of the table's 1004 words has bit 15 clear**, because an RGB555 colour leaves it
  unused. That is a 251-record-long constraint, and arbitrary bytes do not satisfy it.
* **A second species, never used in the search.** The wild Pokémon in the captured battle was a
  Hoothoot. The table says `#163` is `(165, 90, 80)` and `(40, 68, 100)`; the emulator's
  framebuffer in that battle contains exactly those two colours and no others besides white and
  black. The pixel counts agree too: Marill's extracted front picture has 374 pixels of shade 1
  and 67 of shade 2, and the status page draws 374 blue and 67 red.

So **shade 1 is the record's first colour and shade 2 its second**, and the tool now renders
Marill — and its shiny palette, `(80, 160, 95)` and `(219, 78, 214)` — in the colours the
cartridge holds.

---

## 6. The entrance: a decimation, which is why it looks like a squash

**This section corrected an earlier wrong answer, and the correction is the interesting part.**

The first version of this tool measured the send-out as "the Pokémon grows out of the ball at
24, 32, 40 and 48 pixels". That was wrong. It was measuring the bounding box of everything in
the player's quarter of the screen, and for the first several frames what is in that quarter is
mostly the **Poké Ball's poof sprite**, which is larger than the Pokémon and is not the
Pokémon. The numbers were real; the thing they described was not.

The real mechanism is in the disassembly, and it is not a scale at all.
[`BattleBGEffect_EnterMon`](https://github.com/pret/pokegold/tree/master/engine/battle_anims/bg_effects.asm),
reached from `ANIM_SEND_OUT_MON` in
[`data/moves/animations.asm`](https://github.com/pret/pokegold/tree/master/data/moves/animations.asm),
which `SendOutPlayerMon` plays immediately before `PlayStereoCry` — **the cry and the entrance
are one moment in the engine, not two.** Its `.PlayerData` is three steps, and each one fills the
background map from a fixed pattern of tile indices:

| step | tiles | columns and rows kept | at tilemap |
|---|---|---|---|
| 1 | 2×2 | `0, 5` | `(4, 10)` |
| 2 | 4×4 | `0, 2, 3, 5` | `(3, 8)` |
| 3 | 6×6 | all six | `(2, 6)` |

The picture's 36 tiles are in VRAM once, at their own size. The effect draws **fewer of them**,
throwing away middle columns and rows and keeping the outer ones. Nothing is resampled, so no
pixel is ever a colour the artist did not draw. The box's bottom edge stays on tilemap row 11
throughout, so the Pokémon grows upward out of the ball rather than outward from its middle.

**That is why it reads as a squash, and why what it looks like is species-dependent even though
the effect is not.** Marill is wide and low, so cutting the middle columns takes more width from
it than cutting the middle rows takes height — "squished thinner and taller, then released to
normal size" is an accurate description of what a player sees, produced by a mechanism with
nothing per-species in it. A tall, narrow Pokémon put through the identical three steps would
look squashed the other way.

**Verified, not argued.** With `--save`, the tool renders each step from the extracted back
picture in the extracted palette, places it where the engine's own tilemap coordinates put it,
and compares it pixel for pixel with every frame the emulator drew:

```text
step 1: 2x2 tiles at tilemap (4,10) — matches   36 of   43 inked pixels (83.7%)
step 2: 4x4 tiles at tilemap (3,8)  — matches  432 of  441 inked pixels (98.0%)
step 3: 6x6 tiles at tilemap (2,6)  — matches 1273 of 1273 inked pixels (100%)
```

Step 3 is exact. The shortfall in the first two is the poof drawn over the top of them, which is
also why only *inked* pixels are counted — the poof lands on the white ones. The numbers
reproduce across different encounters.

The front picture is 7×7 tiles in battle, so its entrance uses the same routine's `.EnemyData`
and the 3×3 / 5×5 / 7×7 patterns. That one is **transcribed, not verified here**: the wild
Pokémon is already on screen by the time this tool starts recording, so no frame of an enemy
entrance was captured to check it against. Capturing the encounter transition would close it.

`entrance-back-*.png` and `entrance-front-*.png` replay both.

### What this does *not* change

**Gold still holds no animation frames for any species.** Every front picture decompresses to
exactly the tiles its own dimensions call for, with nothing left over — that is the
500-agreement check in §1, read the other way round. The same measurement on Crystal finds **18
to 25 extra tiles per species** (Bulbasaur 43 tiles against a 25-tile picture, Pikachu 50,
Marill 44), because Crystal is where animated front sprites arrive. The entrance is a generic
engine effect over one static picture, and a tool offering to extract per-species animation from
Gold would be inventing it.

On the **status page** there is no effect at all: the picture appears — its bottom tile row two
frames before the rest, which is a transfer spread over frames — and then does not move again
for the five seconds the tool watches it.

---

## 7. Handing it to the Pokéwalker

`marill.pwpet1` is the 1952-byte pack the Pokéwalker pet software loads — that project keeps no
artwork of its own and takes a pet as a pack lifted out of the player's cartridge at run time.
Its Generation 1 producer leans on this repository's Generation 1 decompressor; this is the
Generation 2 half of the same arrangement, written here because this is where the Generation 2
decompressor now lives.

`marill.pwpet2` is the same 1952 bytes with the cartridge's own eight palette bytes on the end —
a new magic rather than a longer `PWPET1`, because the reader this is for checks the length
exactly and a silently longer pack is a pack it refuses. The first 1952 bytes are byte-identical,
so today's importer keeps working on the `.pwpet1` and a future one gets real colour for free.

Frame 2 of the pack is frame 1 nudged down one pixel. The Gen 1 producer already does that and
says plainly it is not what a shipping implementation would do. It is the right thing here for a
reason Gen 1's tool could only assume and this one has measured: **there is no second frame on
the cartridge to put there.**

**What is not done:** the Pokéwalker project's own importer is not taught to read `PWPET2`, and
its walking screen is not wired to play the cry or the entrance. That work is in that
repository, not this one.

---

## 8. What this does not establish

* **Silver and Crystal.** Every number here was measured on English Pokémon Gold, SHA-1
  `d8b8a3600a465308c9953dfa04f0081c05bdcb94` — the cartridge
  [building the disassembly](building-the-disassembly.md) records `pret/pokegold` reproducing
  byte for byte. Crystal is refused by the check, which is the correct behaviour and also the
  proof that the check works. Silver is untested.
* **The front picture's entrance.** Transcribed from `.EnemyData` in the same routine, not
  checked against a captured frame — see §6.
* **`FixPicBank`.** The bank correction is derived and verified; the routine that performs it
  has not been found in the cartridge.
* **The cry's data.** Nothing here reads Gen 2's `Cries` table or its audio engine. The cry is
  captured, not decoded, which means it is exactly what the hardware played and nothing at all
  about how.
* **Reaching a battle is a walk.** The driver backs out of the menus and explores with a
  pseudo-random walk until an encounter fires — measured: a walker that cycles four directions
  in order paces two tiles for ever, because it never stops moving and so never notices it is
  not going anywhere. Two things make it non-reproducible: the MBC3 clock
  cartridge reads the host's wall clock, so the in-game time — and the intro dialogue that
  depends on it — differs between runs, and Gen 2's RNG is not established here at all. A run
  that finds no grass says so and still produces everything in §1–§4.
* **Everything the area README already lists** as not established about Gen 2: battle
  structures, storage, the bag, the map system, the RNG.

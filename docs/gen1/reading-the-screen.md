# Reading the game's own screen as text

[← Gen 1 area](README.md)

**`wTileMap` (`$C3A0`, 20 × 18) reads back as the text a player can see.** Not "can be
OCR'd" — the bytes *are* the characters.

This is the single most useful property Generation 1 has for anything driving it from outside,
and it is not a coincidence of one game: it falls out of a design decision that a whole class
of Game Boy software shares.

---

## Why it works

Two facts, both about the same design:

1. **The font is loaded so that a tile id equals the character code of the glyph it draws.**
   Gen 1's charmap (`charmap.asm`) puts `A`–`Z` at `$80`–`$99`, `a`–`z` at `$A0`–`$B9`, `0`–`9`
   at `$F6`–`$FF`, space at `$7F`, and the terminator at `$50`.
2. **Text is drawn by writing character codes into the tilemap.** `PlaceString` /
   `print_string` copy the encoded string straight into `wTileMap`, and the PPU's normal
   background pipeline turns each byte into the glyph with that tile id.

There is no separate "text layer". The tilemap *is* the text buffer, and reading it back with
the charmap prints exactly what is on the screen.

A few tiles decode to more than one character, because the game packs common words into single
tiles: `$4A` is `PKMN`, `$54` is `POKé`, `$5D` is `TRAINER`, `$BD` is `'s`. A decoder should
expand them — and should render everything it does not recognise (borders, cursors, sprites
composed of background tiles) as a space, so a decoded screen is readable rather than noisy.

Implementations here:

* [`plugins/gen1-link/src/text.rs`](../../plugins/gen1-link/src/text.rs) — `encode_char` / `decode_char` /
  `encode_fixed`, the charmap itself, used for names going onto the link cable.
* [`plugins/trainer/src/text.rs`](../../plugins/trainer/src/text.rs) — `decode_screen`, which is the *screen*
  decoder: it does not stop at `$50` and it renders unmapped tiles as spaces.

---

## What it makes possible

### Navigating menus without counting frames

A script that presses A after 214 frames is recording one run. A script that presses A when the
screen says `FIGHT` is describing the sequence. The second survives a different save, a longer
Pokémon name, a slower cry, and a change in emulation timing; the first does not.

This is why the cinematic language in
[battle control](battle-control.md#why-the-script-is-written-against-the-screen) is built on
`await <text>` and `press-until <key> <text>` rather than on frame numbers. It is also how
[`examples/gen1_trade.rs`](../../examples/gen1_trade.rs) drives a real game through the Cable
Club: it presses A when the screen *says* `TRADE CENTER`.

### Testing against what the player sees

`testharness/trainer.rs` checks the trainer panel against **the game's own party menu**, by
decoding `wTileMap` and comparing. That is a fundamentally stronger check than comparing our
decoder against another copy of our own assumptions: the game is the oracle.

Any future test that wants to check emulator state against what the game displays can use the
same tool.

### Debugging test ROMs

The same trick works far outside Pokémon. Both of the big accuracy suites load a font the same
way:

* **Mooneye** — `print_string` writes char codes straight into the BG tilemap, so decoding
  `$9800` as 20×18 ASCII prints the assertion table (`C: FF!` and so on). Far faster than
  guessing from registers, and it is how this project debugged its way from 13/66 to 53/66.
* **Blargg** — the ROMs that never touch the link port still print their verdict. `halt_bug`
  and `interrupt_time` have no cartridge RAM at all, so `testharness/testroms.rs` reads the
  **screen** instead and records `Failed` rather than an opaque frame hash.

---

## The traps

**Gen 1 draws the whole screen through the WINDOW layer, not the BG map.** `LCDC` bit 5 is on
with `WY = 0`, `WX = 7`. Reading the BG tilemap at `$9800` therefore returns 20×18 spaces and
every menu looks empty. Read `wTileMap` in WRAM — the game's own buffer — not VRAM. (Mooneye
*does* use the BG map, hence `$9800` above; the lesson is to find out which buffer the program
writes rather than assuming.)

**A decoded screen is not a fixed-length string.** Because some tiles expand to whole words,
360 tiles do not produce 360 characters. Slicing a decoded screen by byte index panics on a
multi-byte character and gives the wrong column even when it does not. Decode **row by row** if
you want rows.

**Match with whitespace collapsed and case ignored.** The tilemap pads every row to twenty
columns and messages wrap across rows, so a phrase like `used FLAMETHROWER` is split by a run
of spaces. Squashing runs of whitespace to a single space before comparing makes the obvious
needle work; comparing raw does not.

**Text types out character by character.** A screen sampled mid-message contains a prefix of it.
Waiting for the full phrase is correct; waiting for a prefix will fire early.

---

## What this suggests next

* **An agent that plays the game** reads the screen, decides, and presses — the same loop, with
  a goal instead of a script. See [agent play](agent-play.md).
* **A regression test that asserts on dialogue.** Any scripted scenario can now check *what the
  game said*, which is a much stronger assertion than a framebuffer hash and a much more
  readable one when it fails.
* **The same question for other games.** "Which buffer does this program write text into, and is
  the font loaded at matching tile ids?" is worth asking of anything with a disassembly. When
  the answer is yes, everything on this page transfers.

# Reading Gold's screen as text

[← Gen 2 area](README.md)

**`wTilemap` (`$C3A0`, 20 × 18) reads back as the text a player can see** — the same address,
the same property and the same charmap base as Generation 1. `$80` is still `A`, `$A0` is still
`a`, `$F6`–`$FF` are still the digits.

So the whole of [the Gen 1 page on this](../gen1/reading-the-screen.md) transfers: the font is
loaded so a tile id *is* the character code of the glyph, text is drawn by writing character
codes into the tilemap, there is no separate text layer, and a script that presses A when the
screen *says* something survives a different save and a change in emulation timing where a
script that presses A on frame 214 does not.

**This page is only the differences.** There are three, all measured, and each one cost a run
before it was understood.

---

## 1. The overworld contains no letters at all — and that is a *better* test than a word list

Gen 2's map tiles are ids below `$80`, which the charmap decodes to spaces. So a decoded Gold
overworld screen is **entirely blank**.

That turns a weak test into a strong one. The obvious way to answer "does the game have control
yet, or is a menu still up?" is a word list — wait until the screen stops saying `CONTINUE` and
`NEW GAME` and `BADGES`. **Measured, that loses**, and it lost expensively:

> Continuing the captain's real Gold save put up a `PLAYER <name>` box, then *"The clock's time
> may be wrong."* (its RTC consistency check failing against the emulator-restored clock), then
> the entire clock-set screen (*"Set with the Control Pad."*, `engine/rtc/restart_clock.asm`) —
> three screens in a row containing **none** of the listed words, while `wMapGroup` and
> `wMapNumber` already read plausible values. Each word that escaped the list cost a full run
> walking a bot at nothing.

The robust test is the property, not an enumeration:

```
any ASCII letter on the decoded tilemap  ==  a menu or dialog still owns the joypad
```

A word list is a list of the screens somebody thought of. This is a statement about how the
renderer works, and it cannot be escaped by a screen nobody anticipated. **Wherever you can
replace an enumeration with a property, do.**

**Dismiss with A, never B.** Measured: B backs out through the continue menu to the title
screen and the run is over. (Gen 1 has the opposite habit — B is the *safe* dismissal there,
because it closes menus and can never open one. The two generations disagree, so this is not
transferable knowledge; it is a per-game fact.)

---

## 2. The menu cursor is a tile, not a variable

**Gen 2 draws the menu cursor as tile `$ED`**, and it appears exactly once on an open menu. Find
it by scanning the tilemap for that byte:

```rust
(0..18).find(|&r| (0..20).any(|c| peek(W_TILEMAP + r * 20 + c) == 0xED))
```

**Do not use `wMenuCursorY`.** Measured: it read `2` for the save menu while the cursor was
visibly sitting on row 4, and a bot trusting it pressed A on the wrong entry. Dumping the
tilemap is what showed the real cursor. Its address is deliberately not published in
[the memory map](memory-map.md#5-the-screen) so that nobody is tempted.

The generalisable form: **the screen is what the player sees, so the screen is what to drive
from.** A WRAM variable with a promising name is a claim about internals you have not verified;
the tilemap is the thing the game is actually showing. This is the same argument that makes
`await <text>` beat `wait 214 frames`, one level further down.

---

## 3. The Gen 2 tilemap is authoritative; Gen 1 needs care about *which* buffer

Both games write text into `wTileMap` / `wTilemap` in WRAM, and for Gen 2 that is the end of the
story — reading it directly is what drove every menu in this area.

Gen 1 has a wrinkle worth knowing if you are writing one tool for both: it draws the whole
screen through the **window layer** (`LCDC` bit 5, `WY = 0`, `WX = 7`), so a reader that goes to
VRAM has to pick the right map and account for scroll, and a reader that goes to the BG map at
`$9800` gets 20 × 18 spaces and concludes every menu is empty. Both approaches appear in this
repository for Gen 1 — `examples/gen2_timecapsule.rs` reads Gen 1 through VRAM (matching
`examples/link_two.rs`) and the trainer plugin reads `wTileMap` in WRAM.

The rule that covers both: **find out which buffer this program writes text into, rather than
assuming.** It is the same question the Gen 1 page asks about Mooneye, which writes into the BG
map and would defeat a reader that only knew about Pokémon.

---

## 4. Two decoding traps that carry over unchanged

Both are Gen 1 findings and both still apply, so they are named rather than re-argued:

* **Match with whitespace collapsed.** Rows are padded to twenty columns and messages wrap, so a
  phrase is split by runs of spaces.
* **Text types out character by character.** A screen sampled mid-message contains a prefix of
  it, so wait for the full phrase.

**One Gen 1 trap that has *not* been checked for Gen 2:** Gen 1 packs common words into single
tiles (`$4A` is `PKMN`, `$5D` is `TRAINER`), which is why a decoded Gen 1 screen is not a
fixed-length string. Whether Gen 2's charmap does the same, and for which tiles, is **not
established here** — the decoder used for this area maps letters, digits and a handful of
punctuation and renders everything else as a space, which is safe but lossy. A Gen 2 tool that
needs to match on `PKMN` should transcribe the charmap from `constants/charmap.asm` during a
[disassembly build](building-the-disassembly.md) rather than guess.

---

## What this suggests next

* **The property test generalises past Pokémon.** "This screen renders no glyphs, therefore any
  glyph means something else has the joypad" works for any game whose map tiles and font tiles
  live in disjoint id ranges — which is most of them, because that is how a tileset gets loaded.
* **A shared screen reader for both generations** is now a small piece of work: one charmap
  table, one buffer choice per game, one decode. Two crews have written one each.
* **A decoded screen is a much better test oracle than a framebuffer hash**, and Gen 2 now has
  the same access to it that Gen 1 does — so any future Gen 2 regression test can assert on
  *what the game said*.

# Techniques — the instruments that found the rest of this section

← [Pitfalls index](README.md)

Every entry elsewhere in this section was settled by a measurement, not by an argument. These
are the measurements. Most transfer to anything with a screen, a suite or a second process.

---

## Read the ROM's own verdict off the frame

**The problem.** Four of Blargg's test ROMs never touch the link port: they print `Passed` /
`Failed #N` **on screen** and nowhere else. Scoring those by comparing pixels means shipping a
reference frame produced by *your own emulator*, which is the
[self-judging harness](tests.md#a-harness-whose-expected-output-is-its-own-output) all over
again. So they get left out, and four real checks go unscored.

**The technique.** Blargg's print routine loads a font whose 8×8 glyphs sit **in the ROM under
test**, one glyph per character in ASCII order. So every glyph on the screen is a byte-for-byte
copy of eight bytes already in the cartridge you were handed:

1. cut the frame into 8×8 ink masks;
2. `rom.find` each mask — `offset // 8` is the character code, up to one constant;
3. pin the constant by keeping the alignment whose decoded words appear **verbatim in the
   ROM** (each ROM carries its own title plus `Passed`/`Failed` as plain ASCII).

**Why it is self-validating.** A wrong alignment decodes to noise, and noise is not in the
ROM. When nothing scores, the check reports **error** — never a guessed verdict. And the
measurement that says the block really is the font: all 19 non-blank glyphs of `halt_bug` are
found, uniquely, and every one lands on the same `offset % 8` class.

**What it cost, and what it did not.** No contract change: it is a new *method* riding the
existing screenshot capability, so every adapter with a screenshot flag gained four checks
with no edit at all — [binjgb](https://github.com/binji/binjgb) went 1/1 to 4/5 without being
touched. Two wrinkles real frames needed: `oam_bug` prints a failing sub-test's code in
**inverse video** (a cell that is the glyph's exact complement), and a console scrolled by a
non-multiple of eight puts the cell grid off the frame's lattice, so the sub-cell origin is
searched 0..7 in each axis. It reads a foreign emulator's frames whose shade ramp is
*inverted* relative to ours, because it never looks at a colour — only at which cells carry
ink.

**Where.** `tools/conformance/screentext.py`; the in-repo form is in
`testharness/testroms.rs`. The same trick — *the tilemap reads back as text* — drives menu
navigation in the Pokémon harnesses and prints a failing Mooneye test's own assertion table
(`GB_SCOUT_SCREEN=1`), which is far faster than guessing from registers.

> **The generalisation.** If the thing under test renders text, and the font it renders with
> is data you already have, you can read its verdict without an oracle of your own. Look for
> that before you pin a reference image.

---

## A picture is a better proof than a number for a video chip

**The problem.** A PPU regression reports as "N pixels differ". N is nearly useless: it does
not say what broke, and it is wildly non-linear.

**The technique.** Break exactly one behaviour, render one well-chosen ROM, and **look at
it**. dmg-acid2 draws a face, and each broken behaviour deforms it in a specific, diagnosable
way. Eight of ours, each produced by breaking one thing in `src/gpu/mod.rs`, rebuilding,
rendering 600 frames and comparing by shade rank:

| broken behaviour | pixels | what the face does |
|---|---:|---|
| object draw order by memory position only, ignoring X | 12 | one stray dot on the left cheek |
| the ten-objects-per-line limit removed | 28 | the `!` vanishes from "HELLO WORLD!" |
| object X-flip ignored | 119 | the nose becomes a double arrowhead; eye highlights swap sides |
| background-over-object priority ignored | 144 | white rectangles punch through both eyes |
| bit 0 of an 8×16 tile index not masked | 256 | the mouth collapses into a smirk |
| the window's offset of 7 dropped | 733 | the right of the face shifts 7 px left, notching the eye and chin |
| object Y-flip ignored | 868 | the nose becomes an upward arrow and **the smile becomes a straight line** |
| the `$8800` block indexed unsigned instead of signed | 1,044 | the top of the head tears off; the caption dissolves |

**The point the gallery makes and a pass/fail line cannot.** *Severity and pixel count are
barely related.* 12 pixels and 1,044 pixels are both total failures of a documented behaviour.
Anyone reading a pixel distance as a percentage is reading it wrong — and any row near the
frame's total (23,040) is usually a **rank artefact**, not a distance: one extra distinct shade
re-ranks the whole picture. One of ours read 22,070 and was four wrong pixels.

**Where.** [`docs/emulation/test-roms.md`](../emulation/test-roms.md), with the images in
`docs/emulation/faces/`. Committing renders of an MIT test ROM is deliberate; never do it with
a commercial cartridge.

---

## Characterise failures before you fix them

**The problem.** Fifty-seven red rows. Which one do you open?

**The technique.** None of them, yet. Compute each failure's **signature** — the offset, the
affected region, the direction, whether the residual is the same shape as its neighbour's —
and group them. Then:

* a group of eleven rows failing by *the same four-pixel shift, in both directions* is
  [one bug](accuracy.md#rows-that-move-together-are-one-bug);
* a group whose residual is a missing glyph is
  [not winnable at all](not-our-bug.md#most-of-a-suites-remaining-rows-can-be-unwinnable);
* what is left is the work.

The characterisation is cheap and it changes the scope of the project — in our case from
"62 rows to go" to "12 rows are in play". Do it before you scope, not after.

**A cheap sibling that has closed rows in minutes:** compare the suite's own reference sets
against each other. If the gap between two references equals your error, the emulator is right
and your runner booted the wrong machine.

---

## Trace against the ROM's own expectation table, not its pass/fail line

**The problem.** "The row is red." That is one bit of information from a test that computed
dozens.

**The technique.** Most good test ROMs ship their expectations in their source — a table of
offsets and expected bytes, or cycle-by-cycle comments in the handler. Print the same
quantities out of your emulator and lay the two side by side.

* `lcdon_timing-GS` ships a 24-entry cycle table beside four 24-entry expectation tables.
  Printing `(line, modeclock, mode)` at every `STAT` read turned "one row is red" into "every
  entry agrees except `modeclock == 252`" **in one run**.
* `m3_bgp_change`'s handler is annotated cycle by cycle. A trace showing BGP committing at
  dots 77/93/105/165/177/237/249 reconciles against the reference PNG's transition columns
  1/14/73/86/145/158 exactly under *pixel = dot − 92*. Reading a reference image's transition
  columns is a five-line script and turns "the row is red" into an arithmetic statement.
* Where a ROM writes its assertions to the screen, decode them (see the first technique on this
  page) rather than reading registers.

**And the corollary.** A one-dot change can be a whole M-cycle to the CPU, so always check
where the dot sits in the M-cycle grid before concluding a phase is wrong.

---

## Make identity, not similarity, the portability claim

**The problem.** "It runs on ARM" is not checkable. "It seems to work" is worse.

**The technique.** Require a **byte-identical frame hash** across architectures. A Game Boy
emulator has a hundred ways to be nearly right, and every one of them moves the frame — a
timer edge, an interrupt's phase, a sign extension, a shift. Five machines across three
architectures and two pointer widths producing the same 23,040 pixels means the port changed
the *host*, not the emulation.

Pair it with a second, independent identity: `state bytes` (the save-state length) matching
across pointer widths is what says no `usize` leaked into the file format.

**Where.** `tools/run-cross.sh` / `make test-cross`, and
[`docs/portability.md`](../portability.md). Read
[tests.md](tests.md#a-skip-is-not-a-pass) before quoting it: a target whose emulator is missing
is skipped, and a run where nothing ran still exits 0.

---

## Prove the test can fail

**The problem.** A green test proves nothing until you have seen it red for the right reason.

**The technique.** Break the thing deliberately and watch the specific test go red, then
restore it. Ours, on the record:

* clearing the HALT-bug latch turns `halt_bug` red;
* ignoring sprite X-flip turns dmg-acid2 red at **119 of 23,040** pixels — the same figure the
  measured gallery above independently reproduces, which is what says the method is sound;
* a one-line timer break (`TAP[3] 7→6`) reddens the external Mooneye gate; a one-line
  BG-scroll break moves 26 Mealybug rows;
* disabling the mid-M-cycle interrupt fix makes the end-to-end test read 456 dots on both
  paths instead of 460 against 456;
* pointing the battery-write door at cartridge RAM proves the write guard is not vacuous;
* removing a save-state field makes the round-trip harness desync.

Two rules that come out of doing this often:

* **Rebuild between the halves.** The scouts are `cargo build` targets and a stale binary looks
  exactly like a regression — see
  [tests.md](tests.md#a-stale-binary-looks-exactly-like-a-regression).
* **Say so when it does not work.** One of our fixes could not be reproduced red-to-green under
  synthetic load, and the commit says the diagnosis rests on the mechanism rather than on a
  reproduction. That sentence is worth more than a confident one.

**The fixture half of the same rule.** A test whose *fixture* cannot exhibit the bug is green
either way. A battery-safety test needs a cartridge that actually **writes** to cartridge RAM —
one that merely declares RAM passes with or without the fix, because the battery layer writes
nothing for a session that never touched it. That trap cost one of our tests its teeth.

---

## Make the check categorical, not a threshold

**The problem.** "Is the picture blurred?" and "did the zoom soften anything?" sound like
questions you answer with a tolerance. Pick one and you have invented a number that is too
tight on somebody's window size and too loose on somebody else's, and you will spend the
argument on the threshold rather than on the bug.

**The technique.** Find the property that is *categorically* true of correct output, and assert
that instead. A Game Boy emits exactly four shades and every palette maps them to four colours,
so **"a colour on screen that is not one of the four" cannot happen** — there is nothing to
tune, and it holds at every window size, palette and scaling ratio. No interpolating resampler
can satisfy it, which is precisely why it catches them.

Ours, twice, in two unrelated pictures:

* **The terminal half-block path.** `ratatui-image`'s half-block encoder hard-codes a `Triangle`
  filter, so every cell was an average of the pixels under it. Measured on dmg-acid2 at 80×24:
  **271 distinct colours in a four-shade picture.** The guard is
  `no_colour_appears_that_was_not_in_the_framebuffer` plus an end-to-end twin, and against the
  unfixed code the second one *reports the bug in words* — "the picture contains 268 colour(s)
  no Game Boy shade maps to". [display.md](../display.md#the-regression-guard).
* **The swarm view's zoom.** Every agent is a distinct colour and every colour on the frame
  means something, so a smoothed zoom inventing a shade between two agents is a bug rather than
  a softness. Each integer scaling step asserts that no colour in the output was absent from
  the input. [swarm-view.md](../swarm-view.md).

**Why it is worth reaching for.** A categorical check is *self-explaining* — it can name the
offending value, so the failure message is the diagnosis. A threshold check can only say the
number was too big.

**Where else it applies here.** Any point where something downstream might interpolate,
quantise or re-rank: a scaler, a terminal, a video sink, a screenshot path. The related trap is
the opposite direction — a **shade-rank** comparison is categorical about *order*, so one extra
distinct shade re-ranks the whole frame and reports ~23,040 pixels wrong for four bad ones. See
the pixel-count warning above.

---

## Two verdict methods disagreeing is the finding

**The problem.** You have two ways to judge a run and they disagree.

**The technique.** Do not pick the flattering one. It has happened twice here, in opposite
directions:

* a Color STAT/VBlank change made the **ROM's own report** say PASS and a hardware
  **photograph** say FAIL. The photograph was right — the change would have shipped an
  18,060-pixel regression as a win;
* the missing boot tilemap made two ROMs **report PASS** while failing an **image comparison**.
  The image was right — one shade fewer on screen re-ranks every white pixel under a shade-rank
  compare, so a content error read as a whole-screen failure.

**The corollary for tools.** Judge screens *and* verdicts. A suite that only reads serial
output cannot see either of those; a suite that only compares pixels cannot see a ROM
announcing its own reason. And an emulator's *own telemetry* is a third method that sees a
class neither can — `speed_pct`, `fps`, `tx_bytes`, `frames_dropped` expose presentation
defects that leave the framebuffer perfectly correct. That is the argument in
[`docs/conformance.md`](../conformance.md).

---

## Reproduce it in the shape a person actually uses

**The problem.** An in-process test proves two components agree in memory. That is not the
claim a person makes when they say a feature works.

**The technique.** Take the verdict at the outermost boundary you can reach.

The two-process link harness judges three ways, and the third is the one an in-process test
cannot make: each child checks its arrival **field by field** (a lost byte arrives as `$FF`,
`$FF` is not a character, and a decoder that skips it turns a corrupt `GLIMMER` into a
plausible `MMER` — so *check the bytes, not that something arrived*); the parent checks the
transport moved bytes, tore none and faulted neither side; and then the parent **reads both
save files back off disk** and requires each to hold the other's data and pass the game's own
checksum. Only the third one is what a person means.

It found things nothing else could — including a trait method that existed as an *inherent*
method and had never been added to the `impl`, so every clean disconnect was reported by the
peer as a broken link and the peer stopped writing its save. The unit test passed throughout,
because it called the inherent method on the concrete type.

**The cost, stated honestly.** That harness is load-dependent and deliberately out of CI. See
[tests.md](tests.md#a-reliability-percentage-that-is-really-a-load-measurement) — a technique
that finds real bugs and cannot be automated is still worth having, as long as you say which
it is.

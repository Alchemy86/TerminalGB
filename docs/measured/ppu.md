# The PPU — two engines, and what the difference is worth

← [The measured Game Boy](README.md)

The PPU is where this emulator's remaining accuracy work is. It is also the one component
where the *shape* of the implementation is a documented, measured choice rather than a
detail: there are **two renderers**, both shipped, both gated in CI, and the whole-scanline one
is still the default — **on cost alone now**, since the per-dot engine no longer loses a row on
any gate.

Hardware reference, not restated here:
[Rendering](https://gbdev.io/pandocs/Rendering.html) ·
[Pixel FIFO](https://gbdev.io/pandocs/pixel_fifo.html) ·
[LCD Status Registers](https://gbdev.io/pandocs/STAT.html) ·
[Accessing VRAM and OAM](https://gbdev.io/pandocs/Accessing_VRAM_and_OAM.html) ·
[OAM Corruption Bug](https://gbdev.io/pandocs/OAM_Corruption_Bug.html). gbdev's
[Timing of LYC/STAT Handlers](https://gbdev.io/guides/lyc_timing.html) is the guide most
directly relevant to the STAT section below. Component page:
[`docs/components/ppu.md`](../components/ppu.md). Full suite scoping:
[`docs/mealybug.md`](../mealybug.md).

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## Two picture modes

`RenderMode` ([`src/gpu/mod.rs:29`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L29)) picks which one runs — the terminal's `P` key,
`GB_PPU=standard|identical`, the `ppu` config key (the environment variable wins), or
`Gameboy::set_render_mode`. The pre-rename spellings `fast`/`accurate` still parse everywhere;
the whole user-facing story is [`docs/picture-modes.md`](../picture-modes.md). It is
deliberately **not** in the save state: it is a property of this session's host, not of the
emulated machine, so loading somebody else's state must not switch their renderer.

| | `standard` (default, `RenderMode::Fast`) | `identical` (`RenderMode::Accurate`) |
|---|---|---|
| Source | [`src/gpu/mod.rs:1027`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L1027) `renderscan` | [`src/gpu/fifo.rs:317`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/fifo.rs#L317) `do_cycle_accurate` |
| Granularity | one whole scanline at the mode 3 → 0 transition | one dot at a time |
| Mode 3 length | fixed 172 dots | variable, from the fetcher |
| Mid-scanline register writes | not reflected within the line | reflected at the pixel |
<<<<<<< HEAD
| Mooneye `acceptance` | <!--score:mooneye_acceptance-->67/75<!--/score--> | **<!--score:mooneye_acceptance_accurate-->75/75<!--/score-->** |
| Mealybug pixel-exact | <!--score:mealybug-->3/79<!--/score--> | **<!--score:mealybug_accurate-->27/79<!--/score-->** |
| Mealybug total distance | <!--score:mealybug:px-->1,212,282<!--/score--> px | **<!--score:mealybug_accurate:px-->498,150<!--/score--> px** |
=======
| Mooneye `acceptance` | <!--score:mooneye_acceptance-->67/75<!--/score--> | **<!--score:mooneye_acceptance_accurate-->75/75<!--/score-->** |
| Mealybug pixel-exact | <!--score:mealybug-->3/79<!--/score--> | **<!--score:mealybug_accurate-->27/79<!--/score-->** |
| Mealybug total distance | <!--score:mealybug:px-->1,212,282<!--/score--> px | **<!--score:mealybug_accurate:px-->498,150<!--/score--> px** |
>>>>>>> 7b3b9b3 (fix(ppu): an object's row index wraps inside the tile, it does not clamp)
| Cost (dmg-acid2) | **0.221 ms/frame** | 0.461 ms/frame |

**~6.3×** — 3.0 % of a 16.7427 ms frame, ~34× real time headless. Reproduce with
`cargo test --release --test ppuengines`, which also **fails if
`RenderMode::IDENTICAL_COST` — the figure printed to users on the Status row, in the `P` toast
and in the `?` overlay — stops matching what the two modes actually cost.**

**`identical` became the default on 2026-08-20.** That was not always available: it used to
lose `ppu/intr_1_2_timing-GS`, Blargg `oam_bug` sub-test 7 and — once the c-sp aggregation was
wired up — `cgb-acid2`, and while any of those stood, flipping the default would have made a
published score worse. All three are closed, and through the GB Emulator Shootout's own
264-row manifest the two engines are **223 and 238: fifteen gains, no losses**. `standard` is
now the *cheap* engine — small hardware, headless collectors, speed mode — and both are still
gated, each against its own baselines by a `make` target that names its engine. **Never
re-bless [`testharness/baseline.txt`](../../testharness/baseline.txt) to hide a regression**;
that rule did not move and never will.

### The trap that cost the most: `change_mode(0)` runs `renderscan()`

Entering HBlank draws the line that just finished. Under the accurate engine that overwrote,
*every scanline*, the picture the FIFO had just painted dot by dot — and almost invisibly,
because the two agree on ordinary content. What exposed it was a side effect: `draw_bg`
advances the window's line counter as it goes, so running both moved the window down two rows
per scanline.

**Anything else hung off a mode transition must ask which picture mode is running.**

---

## The OBJ penalty is charged explicitly, against Mooneye's own table

Pan Docs' [Rendering § OBJ penalty algorithm](https://gbdev.io/pandocs/Rendering.html) gives
it as six dots per object, plus `max(0, 5 - ((X + SCX) % 8))` for the **first** object landing
on each background tile. We charge that formula literally
([`src/gpu/fifo.rs:715`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/fifo.rs#L715)) rather than letting it emerge from the
fetcher's phase — and the reason is measured.

Deriving it from the pipeline's own phase **reproduced only the maximum**: ten objects at
`X = 0` and ten at `X = 7` both came out at 66 dots, where
`acceptance/ppu/intr_2_mode0_timing_sprites` states 65 and 60. That ROM gives the answer for
**39 arrangements** — every object count against every position within a tile — and is the
ground truth here. Its unit is one M-cycle, **truncated**, so 65 dots reads back as 16.

Pinned device-free by `the_obj_penalty_matches_mooneyes_table`.

**An object at `X < 8` draws nothing and still costs its six dots.** The old predicate required
`x > lx`, which made every object at `X = 0` free; the first ten rows of that Mooneye table
exist to catch exactly this.

### The window's takeover is decided at the *output*

Pan Docs: *"if the window is enabled and the current scanline reaches it, mode 3 is 6 dots
longer."* The comparison is made against the pixel about to be shifted out, so it **cannot
happen before there is one**. Checking it during the head-of-line fetch — where `lx` is already
0 — let `WX <= 7` take over a fetcher with nothing to discard, and cost nothing at all.

Caught by Mealybug `m3_window_timing`, which sets `WX = LY` on every line precisely to
photograph the difference between a window that started before a mid-line `BGP` write and one
that started after. Ours put the boundary at the same pixel for every `WX`. Pinned by
`the_window_costs_six_dots_wherever_it_starts`.

---

## The dot an object is detected is still a *fetcher* dot

Pan Docs gives the OBJ penalty as an amount of time mode 3 grows by. It does not say what the
**background** fetcher is doing while that time is being spent, and the obvious reading —
freeze it — is wrong by exactly one dot.

The object fetch begins from inside the background fetcher's own tick, so the tile fetch in
progress advances rather than stopping where it stood. This one came from **reading the
emulators ahead of us on the Shootout board** rather than from first principles — see
[README § reading other emulators](README.md#reading-other-emulators-what-the-licences-actually-allow)
for which ones may be read and what may be done with what you find. docboy states it in a
comment on the line that starts the object prefetcher (`src/docboy/docboy/ppu/ppu.cpp`,
`handle_pixel_slice_fetcher_push`: *"Note: the first obj prefetcher tick overlaps this tick"*),
with the *output* already stalled a stage earlier. We froze the fetcher on the spot, which put
the next tile-number read one dot late and let a mid-line `SCX` write land in front of it.

*Symptom* — one background tile per scanline is fetched from the wrong map column, **one tile
early**, but only on scanlines carrying an object that sits on a tile boundary, and only when
the game moves `SCX` (or anything else the tile-number fetch reads) mid-line. A player sees a
raster split misplaced by eight pixels on some lines and not others, in a pattern that follows
where the sprites are — which makes it look like a sprite bug and is a background one.

*Caught by* — Mealybug `m3_scx_high_5_bits`, and **nothing else in any suite**. Mooneye's
`intr_2_mode0_timing_sprites` measures the *length* of mode 3 with objects on the line, which
this does not change, and every other harness we run compares a settled frame with no mid-line
writes in it. **A defect that moves the fetcher by one dot is invisible to any test that does
not write a fetcher-read register inside mode 3.** Pinned device-free by
`the_background_fetcher_still_gets_the_dot_an_object_is_detected`
([`src/gpu/fifo.rs:498`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/fifo.rs#L498)), teeth proven by removing the tick.

> **Diagnosis worth reusing: read the failing frame, not the pixel count.**
> `m3_scx_high_5_bits` fills the background with `'A'`, `'B'`, `'C'`… one letter per tile
> column and sets `SCX = LY` part-way along every line, so **decoding the frame as text says
> which `SCX` each tile's fetch used**. We drew `'S'` (map column 18) where hardware drew `'C'`
> (column 2); the seven failing lines were seven rows of one glyph. The count alone could not
> have said that. Same family of trick as `GB_SCOUT_SCREEN=1` on a Mooneye failure.

**The full structural model is worse here, and that is the interesting part.** Letting the
fetcher run for *all* of the penalty's dots — docboy's actual shape — wins the same three rows
and costs 64,784 pixels elsewhere; a single tick on the trigger dot costs 151. The reason is
that our `obj_wait` is Pan Docs' *formula*, not the fetcher's real remaining time, so running
the fetcher through it double-counts. Measured, everything else held constant:

| | rows pixel-exact | suite pixel distance |
|---|---:|---:|
| before | 12 | 646,838 |
| fetcher ticks for the whole penalty | 15 | 711,773 |
| **fetcher ticks on the trigger dot only** | **15** | **646,989** |

**Do not "finish the job" without doing the per-tile calibration first.** Making the wait
structural means the penalty stops being a charged formula, which is what makes
`intr_2_mode0_timing_sprites` exact today.

---

## A PPU register write commits partway through its own M-cycle

*Symptom* — every mid-scanline write to `BGP` / `OBP` / `SCX` / `SCY` / `WX` / `LCDC` takes
effect up to a full M-cycle (four pixels) later than on hardware, so raster effects driven from
a STAT handler are smeared right by up to four pixels at every boundary.

The CPU flushes a write on the **second** T-cycle of its M-cycle, so most of that cycle's dots
already see the new value; on top of that sit per-register settling delays, including **the
DMG's one-dot window in which the PPU resolves colours through `old | new` rather than the new
palette**. Our bus advances the per-dot PPU a whole M-cycle *before* every access, which is
what put the commit at the end. `ppu_write_commit`
([`src/mmu/mod.rs:100`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L100)) is the model; each arm cites the Mealybug row
that measures it, and the whole thing is gated on the `identical` picture mode so the default
path is untouched.

*Caught by* — most of the Mealybug `m3_*` family. `m3_bgp_change`'s DMG column moved
**2,244 px → 22** on this fix alone; `m3_lcdc_obj_size_change_scx`'s DMG column moved
**22,070 → 298**. Nothing in Blargg, Mooneye or acid2 sees it: they judge a serial string or a
settled frame, never a mid-scanline boundary.

**The commit phases are at their optimum — do not re-sweep them.** Both remaining near-misses
were swept afterwards and `early: 3` wins on each by a wide margin: DMG `SCX` at 0/1/2/3/4 gives
84/77/47/**12**/53 px, and DMG `LCDC` at 1/2/3/4 gives a suite total of
671,348/669,696/**646,838**/712,414. `SCX` and `SCY` were split into separate arms to see
whether they wanted different phases; they do not, and the split was reverted.

---

## The Color console's PPU shipped in revisions too

`PpuRevision` ([`src/mode.rs:516`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mode.rs#L516)) — `CgbC` / `CgbD`, default `CgbD`.
Pan Docs describes *a* Game Boy Color; the fetcher changed between steppings, and Mealybug is
the suite that can tell.

**CPU CGB D and later latch the fetcher's `SCY + LY` at the tile-number fetch and reuse it for
both bitplanes; CPU CGB C and earlier re-read it, as every DMG does.** SameBoy says it in a
comment (`Core/display.c`, `GB_FETCHER_GET_TILE_T1`: *"This value is cached on the CGB-D and
newer, so it cannot be used to mix tiles together"*), and docboy states the same boundary from
the other side, modelling D-and-later only. **Two independent implementations agreeing on where
the boundary sits is what makes it a fact rather than a fit.**

*Symptom* — a game that changes `SCY` between a tile's two bitplane fetches draws a tile made
of two different rows on our Color console. On a real CPU CGB D that effect **cannot exist**.

Measured on the Color machine, everything else held:

| | `m3_scy_change@CGB-C` | `m3_scy_change@CGB-D` |
|---|---:|---:|
| `SCY` re-read (CGB C) | **560** | 15,085 |
| `SCY` latched (CGB D) | 15,129 | **116** |

That reproduces the two references' own 6,217-pixel disagreement from both ends.

The second difference — **a palette write is seen one dot later on a CPU CGB C** — is *measured
here* and is in neither reference emulator. The C and D photographs of `m3_bgp_change` differ
by exactly six pixels per line, at `x = 1, 13, 73, 85, 145, 157`: three 12-pixel bands each
shifted by one. Sweeping our Color palette-commit dot swaps the two columns exactly, on four
rows at once, and in no other configuration.

**Before this we were a machine that never shipped** — CGB-D palette timing with CGB-C `SCY`
behaviour. That is the shape of error to look for: not "wrong", but *incoherent*, passing half
of one console's rows and half of another's.

Deliberately **not** `sound::CgbRevision`: the APU's boundary is 0/B against C-and-later, the
PPU's is C-and-earlier against D-and-later, so one enum would name four steppings to express
two facts — and `src/sound.rs` is not compiled on `wasm32` while the PPU always is. Both read
`$GB_CGB_REVISION`, so a user still has one knob. It is machine *configuration*, not cartridge
state, and is **not serialised**. Pinned by
`only_a_cgb_c_can_desync_the_two_bitplanes_with_scy` and
`the_revision_tags_land_on_the_right_side_of_the_boundary`.

> ### When a suite ships more than one reference for a ROM, it is telling you there is more
> ### than one machine
>
> Mealybug ships `DMG-blob`, `CPU CGB C` **and** `CPU CGB D` reference photographs, and seven
> of its ROMs disagree between the two Color sets by 8,265 pixels in total. Scoring both Color
> columns against one Color machine is a **runner** bug that no emulator can pass;
> `examples/mealybug` runs three machines now, one per reference set.
>
> The same lesson is already recorded one suite over: SameSuite's `-cgb0` and `-cgbB` ROMs
> contradict each other, so a runner that ignores the stepping in the filename can never score
> more than four of the five. Two suites, two authors, one rule.
>
> **The diagnostic habit that follows: before grinding a row, compare the suite's own reference
> sets against each other.** `m3_window_timing@CGB-C` sat at 138 px and `m3_bgp_change@CGB-C` at
> 864 — and the C-vs-D *reference* gap is 138 and 864. Both were already pixel-exact on DMG and
> on CGB-D. They were never fetcher errors; they were the wrong machine. **If the gap between
> two references equals your error, the emulator is right and the runner is wrong.**

---

## The STAT interrupt is ONE LINE, not four triggers

The single most consequential PPU fix for real games, and the thing gbdev's
[LYC/STAT timing guide](https://gbdev.io/guides/lyc_timing.html) is about.

`stat_signal` ([`src/gpu/mod.rs:611`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L611)) ORs the enabled sources;
`refresh_stat_line` ([`src/gpu/mod.rs:719`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L719)) raises `IF.1` on the **0→1
transition only**. Every path that can move a term must call it: a mode change, an `LY`
increment, and writes to `STAT`, `LYC` and `LCDC`.

This is "STAT blocking", and **it affects real games**: a raster split driven off both the LYC
and the HBlank source ran at twice the intended rate when we fired per source.

Three further facts, each with the ROM that pins it:

| Behaviour | Pinned by |
|---|---|
| **`STAT` bit 2 is a latched flip-flop** (`lyc_match`, [`src/gpu/mod.rs:157`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L157)), not `line == lyc` computed on read. With the LCD off the comparison clock stops: the flag freezes, a write to `LYC` is invisible, and enabling the LCD re-latches it against the `LY` the PPU restarts from. | `acceptance/ppu/stat_lyc_onoff` |
| **Switching the LCD off must not force the STAT line low.** The coincidence flag keeps feeding the OR gate, so off-then-on with the flag already set produces no edge and no interrupt. Forcing it low *manufactures* one. | `stat_lyc_onoff` round 2 |
| **The line-144 OAM strobe is line 144 only** ([`src/gpu/mod.rs:679`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L679)), not "mode 1 honours the mode-2 bit". Held across all of VBlank it keeps the line high through lines 145–153, blocking the real mode 2 at `LY = 0` and delivering its interrupt a scanline late. | `intr_1_2_timing-GS` (reads `$31` instead of `$14`/`$15`) — **both the presence and the narrowness are wired** |

**The line the PPU restarts on has no OAM scan** (`skip_first_oam`,
[`src/gpu/mod.rs:162`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L162)): `STAT` reads mode 0 through the OAM slot and
goes straight to mode 3.

### The mode-2 STAT strobe rises one M-cycle *before* its scanline

On lines 1–143 only. Line 144 keeps dot 0, and **line 0 has no early window** — Mealybug's
`m3_bgp_change` says so in its own source (*"line 0 timing is different by 4 cycles"*) and
`intr_1_2_timing-GS` agrees.

**Only the per-dot engine emits it.** The whole-scanline renderer changes mode at instruction
granularity, so a 4-dot signal there is mixing resolutions — and it measures as such: Mooneye
unmoved, Mealybug *loses* `m2_win_en_toggle`.

> **Technique worth stealing:** `acceptance/ppu/intr_2_0_timing` times the mode-2 interrupt to
> the mode-0 interrupt. Dispatch latency is common to both ends, so it **cancels** — which
> proves the 4-cycle error was in the PPU signal and not in the CPU. That is what stopped this
> becoming "fudge the interrupt path".

### The strobe must ask `LY`, not `mode` — and the bug it hid was *two* interrupts a line

`oam_strobe` ([`src/gpu/mod.rs:771`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L771)) derived "the OAM scan is running"
from the `mode` field. The per-dot loop updates `mode` *after* `next_line` has advanced `LY`,
so `mode` lags `LY` by one dot — and since `IF` is folded once per M-cycle, one dot late is a
whole **M-cycle** late to the CPU. It asks `LY` and the dot counter now.

**Lines 1–143 were raising two STAT interrupts each, and that is what hid it for months.** The
strobe dropped at the line boundary and rose again a dot later. It is invisible to any program
that services the interrupt at once, because the second edge lands *inside* the first one's
dispatch, which clears `IF.1` at M4 after it — so the pair collapses into one.

> **A defect masked by the very thing that consumes it will not show up in a verdict. Count
> edges, not flags.** `a_visible_line_strobes_the_oam_source_exactly_once` counts them; teeth
> proven by reverting the arm, at which point it counts two. **Nothing in any public suite
> catches the doubled interrupt** — that is a coverage gap, and it is the interesting half.

Line 0 had nothing to collapse into, so it carried the whole error: its interrupt arrived
**8 dots** after the equivalent point on lines 1–143, where hardware wants **4**. Mealybug's
`m3_bgp_change.asm` encodes the requirement itself — one `jr` taken on lines 1–143 and not on
line 0, making the handler exactly 4 T shorter there, under the comment *"line 0 timing is
different by 4 cycles"*.

**The obvious fix is wrong, and was measured wrong.** Giving line 0 the same early window as
lines 1–143 (on line 153's tail) puts it at the *same* phase, where hardware wants it one
M-cycle later: Mealybug **6 → 3** and `intr_1_2_timing-GS` still red. At M-cycle granularity the
placements available are +0, +4 and +8 relative to lines 1–143; hardware is +4.

### The CGB strobes STAT one M-cycle before VBlank

`misc/ppu/vblank_stat_intr-C` and `acceptance/ppu/vblank_stat_intr-GS` are the same program
with one `nop` moved: on Color hardware the line-144 OAM strobe is one M-cycle **before** the
VBlank flag; on a DMG they are simultaneous (`VBLANK_STROBE_LEAD = 4`,
[`src/gpu/mod.rs:95`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L95)).

**Implementing that alone cost two hardware-photographed Mealybug rows, and that was the real
finding.** `m2_win_en_toggle` enables both interrupts, *sets* `LCDC.5` in one handler and
*toggles* it in the other, so the dispatch order inverts the window on every scanline —
18,060 of 23,040 pixels. Its `CPU CGB C`, `CPU CGB D` and `DMG-blob` reference photographs are
**identical in shade rank** (checked: 0 pixels differ), so real Color hardware does *not*
reorder them.

What reconciles the two: [interrupt dispatch decides its vector in the middle of
itself](timing.md#interrupt-dispatch-is-five-m-cycles-and-picks-its-vector-halfway-through).
VBlank's flag going up one M-cycle into STAT's dispatch **redirects** it to the
higher-priority vector. The gap is still measurable when only one of the two is enabled, which
is exactly how `vblank_stat_intr` enables them.

The defect that hid it was the batched PPU standing still for the whole dispatch: the per-dot
engine already advanced per bus access and got this right, so no PPU interrupt could ever
redirect anything under the fast one. Both engines agree now, Mealybug is byte-identical, and
frame cost is unmoved.

> **Two verdict methods disagreeing is the finding.** A ROM's own report said the change was
> right and a hardware photograph said it was wrong. Taking the flattering one would have
> shipped a regression as a win.

---

## Two real PPU bugs only a Mealybug ROM caught

Both fixed, both with unit tests proven to fail when reverted.

1. **The window's "Y condition" must latch independent of `LCDC` bit 5.** Pan Docs, [Window
   § rendering criteria](https://gbdev.io/pandocs/Window.html): at the start of each scanline,
   `WY == LY` latches it for the frame — nothing about the enable bit. We tested `win_on`
   first, so **a game that sets `WY` while the window is off and enables it later in the frame
   never showed the window at all.**
2. **The OAM strobe reaches `LY = 144`.** A program with only the mode-2 select bit still takes
   a STAT interrupt at line 144; missing it loses one STAT interrupt per frame and puts
   everything the handler toggles on the wrong parity of scanlines. Flipped
   `acceptance/ppu/vblank_stat_intr-GS` FAIL → PASS.

**A PPU test ROM caught an interrupt bug** — an argument for scoring screens, not only serial
verdicts.

---

## The OAM-corruption bug: a defect we had to *add*

Blargg `oam_bug@dmg` went **3/8 sub-tests → 8/8**. It is DMG/MGB/SGB silicon only, so
`oam_bug@cgb` **must stay red** — a real Game Boy Color fails that ROM too.

**The cause is the address bus, not a memory access.** The CPU's 16-bit increment/decrement
unit is wired to the address bus and asserts its operand even with no read or write behind it.
So `inc de` with `DE = $FE00` corrupts OAM and `add hl,bc` with the same value does not.
Hooking only `rb`/`wb` gets `2-causes` and `3-non_causes` wrong; the IDU hook
([`src/mmu/mod.rs:763`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L763)) is the load-bearing half.

### `OAM_BUG_PHASE = -12` is a calibration of *our* batching, not physics

[`src/mmu/mod.rs:34`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L34). The whole 12 dots are one `ldh [rLCDC],a`
enabling the LCD: the write sets `modeclock = 4` and the batched renderer then applies the
instruction's *entire* twelve dots on top, parking it three M-cycles past where hardware is.

**Measured, not assumed.** −16 and −8 both fail `4-scanline_timing`, at opposite ends of the
window; only the 4-dot band containing −12 passes. It also assumes the 3-M-cycle
`ldh [rLCDC],a` every test ROM uses — a 4-M-cycle `ld [$FF40],a` would land one row off.

The accurate engine needs **no correction at all** (`ACCURATE_OAM_BUG_PHASE = 0`,
[`src/mmu/mod.rs:37`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L37)): it has already spent those dots access by
access, so its remainder is zero and it lands on the real 4. Measured both ways — with −12 the
accurate engine fails `oam_bug` 4/5/6/8; with 0 it passes them. Swept −12..+12; nothing
recovers sub-test 7, so that is a real finding and not a tuning knob.

Cost of the whole bug: ~1.5 % on the test-ROM suite (2.78 s → 2.82 s, alternating A/B, three
rounds). The hot path gained one `address & 0xFF00 == 0xFE00` test.

---

## Reading a Mealybug number

Everything in [scoreboard.md § Mealybug](scoreboard.md#mealybug-tearoom--and-the-number-is-not-a-percentage)
applies. The two things that most often produce a wrong conclusion:

1. **The metric is a shade *rank* distance.** One extra distinct shade in the frame re-ranks
   every pixel and reports ~23,040 of 23,040. `m3_lcdc_obj_size_change_scx` is **four wrong
   pixels** and scores 22,070.
2. **Score each column on the machine its reference was photographed on.** These cartridges
   carry no CGB flag, so `Model::Auto` gives a DMG — right for `DMG-blob`, wrong for
   `CPU CGB C`/`CPU CGB D`, which are a Color console running the same cartridge in
   compatibility mode. The two Color columns are **also two different machines**, not one:
   `examples/mealybug` now runs three, one per reference set, with the
   [PPU stepping](#the-color-consoles-ppu-shipped-in-revisions-too) selected per column. (An
   earlier version of this page said the Color columns "render identically today"; that
   stopped being true the moment `PpuRevision` landed, and the suite shipping three sets is
   why.)

**Before believing a suite score, check the ROMs are actually running.** Most of the per-dot
engine's *original* three-of-seventy-nine (a historical figure, not the gated one above) was
measuring a stall, not accuracy: it never left mode 0 after the LCD
came on, so every Mealybug ROM — all of which arm on the mode-2 STAT source — ran off the end
of its code before drawing anything. `m3_bgp_change` hit its ten-frame breakpoint in 80,668
cycles instead of ~767,000 and made exactly **one** `BGP` write. Both facts were visible in a
five-minute trace.

> **A fast-forward past a mode is only safe if nothing inside it changes state.** That is the
> general form of that bug: a "mode 0 has no per-dot behaviour, skip to end of line" shortcut
> swallowed the one dot that clears `skip_first_oam`.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| A game's window never appears, though it sets `WY` correctly | the Y condition latched behind `LCDC` bit 5 |
| A raster split runs at double rate, or a screen tears where it should not | STAT fired per source rather than on the line's 0→1 edge |
| Everything a program times from the mode-2 interrupt is one M-cycle late, all frame | the mode-2 strobe not leading its scanline |
| Mid-scanline palette or scroll changes have no effect (bands are missing) | a whole-scanline renderer — expected on the `fast` engine, by design |
| Objects on a line cost the wrong amount of mode 3, so mid-line writes land on the wrong pixel | the OBJ penalty derived from fetcher phase rather than charged |
| Nothing draws at all after the LCD is switched on, and the program runs off the end of its code | the restart line's mode-3 entry skipped, so no mode-2 STAT interrupt ever arrives |
| A parallax layer wobbles by one tile, on the scanlines that carry sprites and no others | the background fetcher frozen on the dot an object is detected instead of finishing its tile |
| Every mid-scanline raster boundary is smeared up to four pixels right | a register write committed at the end of its M-cycle rather than partway through it |
| The top scanline of a split-screen effect is four pixels out of step with the 143 below it | the mode-2 strobe derived from `mode`, which lags `LY` by a dot; line 0 is the only line where the resulting spurious edge has nothing to hide inside |
| A Color game's `SCY` trick produces a tile made of two different rows — or refuses to | the CGB PPU stepping: CPU CGB D latches `SCY + LY` at the tile fetch, CPU CGB C re-reads it |

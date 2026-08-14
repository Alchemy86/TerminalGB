# The PPU — two engines, and what the difference is worth

← [The measured Game Boy](README.md)

The PPU is where this emulator's remaining accuracy work is. It is also the one component
where the *shape* of the implementation is a documented, measured choice rather than a
detail: there are **two renderers**, both shipped, both gated in CI, and the fast one is still
the default because the accurate one does not yet strictly dominate it.

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
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`f69fff9`](https://github.com/Alchemy86/TerminalGB/tree/f69fff9889fe64e40dc194949e820c2b6ead6b52) · regenerate with `make docs-links`, verify with `make docs-check`.

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
| Mooneye `acceptance` | 67/75 | **69/75** |
| Mealybug pixel-exact | 3/79 | **12/79** |
| Mealybug total distance | 1,212,282 px | **646,838 px** |
| Cost (dmg-acid2) | **0.221 ms/frame** | 0.461 ms/frame |

**~2.1×** — 2.8 % of a 16.7427 ms frame, ~36× real time headless. Reproduce with
`cargo test --release --test ppuengines`, which also **fails if
`RenderMode::IDENTICAL_COST` — the figure printed to users on the Status row, in the `P` toast
and in the `?` overlay — stops matching what the two modes actually cost.**

**Cost is now the only reason `standard` is still the default.** That was not always true:
`identical` used to lose `ppu/intr_1_2_timing-GS` and Blargg `oam_bug` sub-test 7, and while it
did, flipping the default would have made a published score worse. Both are closed, so
`identical` loses no row on any gate. Whichever way the default eventually goes, never re-bless
[`testharness/baseline.txt`](../../testharness/baseline.txt) to hide a regression.

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

Everything in [scoreboard.md § Mealybug](scoreboard.md#mealybug-tearoom--3-of-79-and-the-number-is-not-a-percentage)
applies. The two things that most often produce a wrong conclusion:

1. **The metric is a shade *rank* distance.** One extra distinct shade in the frame re-ranks
   every pixel and reports ~23,040 of 23,040. `m3_lcdc_obj_size_change_scx` is **four wrong
   pixels** and scores 22,070.
2. **Score each column on the machine its reference was photographed on.** These cartridges
   carry no CGB flag, so `Model::Auto` gives a DMG — right for `DMG-blob`, wrong for
   `CPU CGB C`/`CPU CGB D`, which are a Color console running the same cartridge in
   compatibility mode. They render identically *today*, and that must stop being true once
   mode 3 is fully modelled — which is exactly why the suite ships three sets.

**Before believing a suite score, check the ROMs are actually running.** Most of the original
"3/79" was measuring a stall, not accuracy: the accurate engine never left mode 0 after the LCD
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

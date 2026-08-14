# Mealybug Tearoom: what it measures, where we stand, and what the rest would cost

[Mealybug Tearoom Tests](https://github.com/mattcurrie/mealybug-tearoom-tests) (Matt Currie, MIT)
is the PPU suite that writes to LCD registers **while a scanline is being drawn** and compares the
result against screenshots taken from real DMG, CPU CGB C and CPU CGB D hardware. Its own README
states the purpose plainly: *"verify correct timing of the background tile and sprite data fetches
as each scanline is rendered."*

We render **one whole scanline at a time**, at the end of mode 3, using whatever the registers hold
at that instant ([`src/gpu/mod.rs`](../src/gpu/mod.rs), `renderscan`). Scoring near-zero on a suite
built to catch exactly that is the expected outcome, not a surprise.

This page is the honest accounting: what we actually score, which failures are winnable without a
structural change, which are not, and what the structural change would really involve. It is
written to be read *before* anyone starts a pixel-FIFO rewrite.

**Reproduce every number here:**

```sh
git clone https://github.com/mattcurrie/mealybug-tearoom-tests
cd mealybug-tearoom-tests && unzip mealybug-tearoom-tests.zip -d build && cd -
cargo run --release --example mealybug -- --dir <that checkout>
```

The ROMs and reference screenshots are **not vendored** — same rule as `POKERED_GFX` in
[`testharness/trainer.rs`](../testharness/trainer.rs): assets come from a path the user supplies.

---

## 1. Score

| | before | after |
|---|---:|---:|
| DMG-blob references | 0 / 24 | **1 / 24** |
| CPU CGB C references | 0 / 31 | **1 / 31** |
| CPU CGB D references | 0 / 24 | **1 / 24** |
| **total rows** | **0 / 79** | **3 / 79** |

`m2_win_en_toggle` is now **pixel-exact against all three hardware references** — 0 of 23,040
pixels differ, from 17,362 before. Every other ROM's frame is byte-for-byte what it was, so the
change bought exactly what it claims and nothing else.

In the [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)'s accounting — which lists 71
Mealybug rows across the three models — this is **0/71 → 3/71**.

> **Each column is scored on the machine its reference was photographed on.** These cartridges
> have no CGB flag, so `Model::Auto` puts them in a DMG — right for `DMG-blob`, wrong for the two
> CGB sets, where the reference is a *Color console running the same DMG cartridge in
> compatibility mode*. `examples/mealybug` therefore runs every ROM on both (`Model::Dmg` and
> `Model::Cgb`) and compares each column against its own frame. Today the two produce identical
> pictures — `GbMode::Classic` and `GbMode::ColorAsClassic` take the same path through
> `draw_bg`/`draw_sprites`, and the consoles differ in boot registers and CGB register
> availability, not in the DMG-mode renderer. That is expected to change the moment mode-3
> behaviour is modelled, since **Mealybug's whole reason for shipping three reference sets is that
> DMG and CGB mode 3 differ**.

Comparison is by **shade rank** (luminance order), the same rule
[`tools/conformance/image.py`](../tools/conformance/image.py) uses, so a palette difference cannot
mask or manufacture a result. A CGB running a DMG cartridge colourises it from the boot ROM's
title-hash palette; we paint greyscale. Ranked, that is the same picture.

---

## 2. The fix that landed, and why it is a PPU fix and not a fudge

`m2_win_en_toggle` is the only ROM in the suite whose register writes all happen in **mode 2** —
before the line is drawn. It is therefore the only one a scanline renderer can get right at all,
and it failed for two separate, citable reasons.

### 2.1 The window's "Y condition" was gated on the window being enabled

*Symptom in user terms:* a game that sets `WY` on a line where the window is disabled and turns the
window on later in the same frame never sees the window at all — for the rest of the frame. On
screen: a status bar or text box that simply does not appear. Mealybug renders this as a **blank
white screen** where the reference is full of text.

*Caught by:* `mealybug-tearoom-tests/ppu/m2_win_en_toggle` (all three references). Nothing else in
our suite catches it — dmg-acid2 enables the window before `WY` matches, so it never exercises the
ordering.

*Hardware behaviour violated:* Pan Docs,
[Window → Window rendering criteria](https://gbdev.io/pandocs/Scrolling.html):

> The PPU keeps track of a "**Y condition**" throughout a frame.
> - On each VBlank, the *Y condition* is cleared (becomes false).
> - **At the beginning of each scanline, if the value of `WY` is equal to `LY`, the *Y condition*
>   becomes true** (and remains so for subsequent scanlines).

No mention of `LCDC` bit 5 — the condition latches whether or not the window is enabled. Pan Docs
illustrates the consequence with this very ROM's expected screenshot. Our `change_mode(3)` arm
tested `self.win_on` before latching, so for `m2_win_en_toggle` (which disables the window during
mode 2 of line 0, the only line where `WY == LY`) the condition could never latch.

*Fix:* `check_wy_trigger()`, called from the **mode 2** arm of `change_mode`, with no `win_on` test.

*New test:* `gpu::tests::the_wy_condition_latches_while_the_window_is_disabled` — plain `cargo test`,
no ROM. Proven to have teeth by restoring the `win_on` guard (it fails).

### 2.2 Entering VBlank did not raise STAT for the mode-2 source

*Symptom in user terms:* a program that enables **only** the mode-2 (OAM) STAT source misses one
interrupt per frame — the one at `LY=144`. Anything that counts STAT interrupts per frame, or
toggles state on each one, ends up on the wrong parity for the whole frame. Mealybug renders this
as a picture that is structurally right but **shifted by exactly one scanline**.

*Caught by:* `m2_win_en_toggle`, and independently by `mooneye/acceptance/ppu/vblank_stat_intr-GS`
(§2.3) — which was red before this and is green after. No Blargg ROM catches it. Note that a *PPU*
screen test found an **interrupt** bug: an argument for scoring frames as well as serial verdicts.

*Hardware behaviour violated:* AntonioND, *The Cycle-Accurate Game Boy Docs* (TCAGBD) §8.7 "STAT
Interrupt" — the internal STAT signal is set by

```text
( (LY = LYC)        AND (STAT.ENABLE_LYC_COMPARE = 1) ) OR
( (ScreenMode = 0)  AND (STAT.ENABLE_HBL        = 1) ) OR
( (ScreenMode = 2)  AND (STAT.ENABLE_OAM        = 1) ) OR
( (ScreenMode = 1)  AND (STAT.ENABLE_VBL || STAT.ENABLE_OAM) )   <- "Not only VBL!!"
```

*Fix:* the mode-1 arm of `change_mode` now returns `m1_inte || m2_inte`.

*New test:* `gpu::tests::entering_vblank_raises_stat_for_the_mode_2_source_too`, positive **and**
negative case. Teeth proven the same way.

*Conformance-tool candidates* (per the [standing convention](../CLAUDE.md)): **"a latched frame
condition must not be gated on the enable bit that consumes it"** and **"the mode-1 STAT source is
the OR of the mode-1 and mode-2 select bits."** Both are one-frame screen checks against a
reference image — exactly the shape `tools/conformance/checks.json` already carries.

### 2.3 A second test agrees, independently

`mooneye/acceptance/ppu/vblank_stat_intr-GS` — whose whole subject is the STAT interrupt at the
VBlank boundary — goes **FAIL → PASS** with §2.2, taking Mooneye acceptance from 52 to 53 and the
remaining PPU reds from 11 to 10. Two suites written by different authors against different
hardware, agreeing on the same rule, is much better evidence than either alone.

### 2.4 What did not move

Verified after the change, all green and all unchanged:

| check | result |
|---|---|
| `cargo test --release --test testroms` | 32/35 ROM/model rows, matching the baseline exactly; **dmg-acid2 still 0/23,040 pixels differ from the reference on both DMG and CGB** |
| `cargo test --release --test savestate` | all round-trip and negative checks pass |
| `cargo test --release --test battery` | 7/7 |
| `cargo test --release --test audiocapture` | matches baseline, `dmg_sound` 12/12 |
| cgb-acid2 (shootout's own ROM + reference + ≤50 greyscale rule) | passes, worst delta 13 |
| `cargo run --release --example mooneye_scout` (75 acceptance ROMs) | **52 → 53**: `ppu/vblank_stat_intr-GS` flips FAIL → PASS, nothing else moves |
| the other 30 Mealybug ROMs | frames byte-for-byte identical, on both consoles |

---

## 3. The remaining 68 rows: what each one actually needs

Every other ROM writes a PPU register **during mode 3** and the reference shows the change taking
effect part-way along the line. Three independent things stood between us and those, and **all
three must be true at once** for a row to pass. One of the three has since been closed by another
crew's core-timing work, which changes the recommendation in §5.

### 3.1 Blocker A — sub-scanline rendering (the structural one)

`renderscan` runs once, at the mode 3 → mode 0 transition, and reads each register once. A test
that sets a register and restores it 12 dots later is invisible to us by construction: we see only
the restored value. This is the blocker on **every** remaining row.

### 3.2 Blocker B — interrupt-dispatch timing. **Now closed, and this is the good news.**

Every Mealybug handler budgets "20 cycles interrupt dispatch" in its own comments, and until the
core-timing work landed (`fix(core): Mooneye acceptance 13/66 -> 50/66 — real timer, 5-cycle
interrupt dispatch, timed OAM DMA`) `Cpu::handleinterrupt` spent 16. Measured on this branch by
tracing every `BGP` write on line 1 of `m3_bgp_change`:

| write | the ROM's own cycle arithmetic | before the 5-cycle dispatch | now |
|---|---:|---:|---:|
| 1 | dot 80 | 76 | **80** |
| 2 | 96 | 92 | **96** |
| 3 | 108 | 104 | **108** |
| 4 | 168 | 164 | **168** |
| 5 | 180 | 176 | **180** |
| 6 | 240 | 236 | **240** |
| 7 | 252 | 248 | **252** |

(Raw `modeclock` at the write is 4 lower in each column; `ld [c],a` puts its bus write on the
instruction's last M-cycle. The uniform 4-dot error, and its disappearance, are what matter.)

**Our CPU now lands mid-mode-3 register writes on exactly the dot the test intends, to the
cycle.** That is the single most encouraging fact on this page: the timing substrate a
fetcher-accurate PPU would sit on is already correct, so the remaining work is genuinely the PPU's
and not a hunt for a phase error somewhere else. It also means a mode-3 row that fails after the
rewrite fails for a reason inside the rewrite.

### 3.3 Blocker C — we do not reproduce post-boot VRAM

We start with VRAM zeroed. Real hardware hands the cartridge a VRAM that still contains the boot
ROM's decompressed Nintendo logo tiles (`$01`–`$18`) and the `®` glyph at tile `$19`, plus the
tile-map entries that place them. **18 of the 31 ROMs depend on that state:**

* **12 write no tile data of their own at all** — their entire picture *is* post-boot VRAM:
  `m3_bgp_change`, `m3_lcdc_bg_en_change2`, `m3_lcdc_bg_map_change2`, `m3_lcdc_obj_en_change`,
  `m3_lcdc_obj_en_change_variant`, `m3_lcdc_tile_sel_change2`, `m3_lcdc_tile_sel_win_change2`,
  `m3_lcdc_win_map_change2`, `m3_obp0_change`, `m3_scx_high_5_bits_change2`, `m3_scx_low_3_bits`,
  `m3_scy_change2`.
* **6 more write their own BG tiles but place every sprite with tile `$19`** — verified in their
  `sprite_data` tables (`DB $10, 00, $19, 0`): `m3_lcdc_bg_map_change`, `m3_lcdc_tile_sel_change`,
  `m3_lcdc_tile_sel_win_change`, `m3_lcdc_win_map_change`, `m3_bgp_change_sprites`,
  `m3_wx_4_change_sprites`.

Our screens for these are blank or missing a column of glyphs *before* any timing question arises.
The logo tile data is derivable from the cartridge's own header bytes `$0104`–`$0133`, but the `®`
glyph is boot-ROM data. **That is a licensing/policy call for the captain, not a PPU decision** —
see the licence-boundary section of [`CLAUDE.md`](../CLAUDE.md). Flagged, not done.

### 3.4 Which rows each level of accuracy would actually buy

| tier | what it needs | ROMs |
|---|---|---|
| **scanline renderer** (today) | correct mode-2 register semantics | `m2_win_en_toggle` — **done** |
| **dot-accurate catch-up renderer** — no FIFO: an exact dot→pixel-x mapping and a flush of the pixels drawn so far on every mode-3 register write | + Blocker B, + Blocker C | `m3_bgp_change`, `m3_obp0_change`, `m3_bgp_change_sprites` — the registers consumed at the *output* stage, where "applies from pixel x on" is what hardware does |
| **fetcher-accurate PPU** (pixel FIFO or equivalent) | + a per-dot BG fetcher state machine, an 8-pixel FIFO, sprite-fetch stalls, window restart, SCX discard, WX penalty, variable mode-3 length | the remaining ~26 |

The middle tier is real but small, and it is not a stepping stone that survives: once you have an
exact dot→x mapping you are two thirds of the way to the fetcher, and the catch-up code gets
thrown away.

Why the fetcher is genuinely required for the rest, in one example: `m3_lcdc_tile_sel_change`
flips `LCDC` bit 4 for about one M-cycle. On hardware exactly one *tile* is affected — whichever
tile's data fetch coincides with the write — and which tile that is depends on the fetcher's 6-dot
cadence and phase. A catch-up renderer applies the change to every pixel from x onward, which is
the wrong shape, not merely the wrong offset. Pan Docs' own mid-frame notes say the same thing from
the other side: SCX/SCY are "re-read on each tile fetch, except for the low 3 bits of `SCX`, which
are only read at the beginning of the scanline"
([Scrolling → Mid-frame behavior](https://gbdev.io/pandocs/Scrolling.html)). You cannot honour a
rule phrased in terms of tile fetches without modelling tile fetches.

---

## 4. Calibration: how hard is this, really

From the shootout's published per-test data (its 24 DMG Mealybug rows):

| emulator | Mealybug |
|---|---:|
| SameBoy | 15 / 24 |
| bgb | 2 / 24 |
| PyBoy | 0 / 24 |
| us | 1 / 24 (was 0) |

This is not a suite where a competent cycle-stepped PPU scores 24/24. It is the suite that
separates the two or three most accurate emulators in the world from everyone else, and the top
scorer on the board still fails a third of it.

---

## 5. Recommendation

### 5.1 The rewrite is now worth doing — but do Blocker C first

The rewrite is the only route to Mealybug's mode-3 rows, to Mooneye's remaining
`acceptance/ppu/*` (10, all failing), to `cgb-acid-hell` and to `daid/ppu_scanline_bgp`. When this
task began the recommendation was "not yet", because two of the three blockers lived outside
`src/gpu/` and the FIFO would have bought zero rows on its own. **Blocker B is now closed**, which
changes that answer: the substrate is correct and the remaining work is genuinely the PPU's.

It is still the wrong thing to start *this minute*, for one cheap reason: 18 of the 31 ROMs render
the boot ROM's `®` glyph, so their frames are wrong before timing is even considered. Doing
Blocker C first costs little, is verifiable on its own, and is what makes a green test after the
rewrite mean what it says. Doing it afterwards means every failure has two candidate causes.

**Order of work, cheapest-first, each independently verifiable:**

1. ~~**Interrupt dispatch 16 → 20 T-cycles**~~ — **done**, by the core-timing crew. Verified here:
   mid-mode-3 writes now land on the exact dot (§3.2).
2. **Post-boot VRAM state.** Needs a policy decision on the `®` glyph first. Verifiable by the
   *static* content of a dozen Mealybug frames snapping into place while the timing bands stay
   wrong — a clean, partial, honest signal, and one nobody has to guess at.
3. **Then** the fetcher-accurate PPU, with 1 and 2 already in place so that each test flipping green
   is real evidence rather than a coincidence of independent errors.

### 5.2 Scoping the rewrite, for when it is time

*Shape.* Step the PPU per dot rather than in ≤80-dot chunks; replace `renderscan` with a mode-3
state machine — BG fetcher (tile number / low byte / high byte / push, 2 dots each), an 8-pixel BG
FIFO, sprite fetch stalls, window restart with its own fetcher reset, the `SCX & 7` discard, the WX
penalty, and a mode-3 length that falls out of the machine instead of being the constant 172 it is
today.

*Cost.* Realistically a full rewrite of `src/gpu/mod.rs` (785 lines today, likely 1,200–1,600
after), plus per-dot stepping in `MemoryManagementUnit::do_cycle`. The hard part is not the code,
it is the calibration: each of the ~26 remaining ROMs pins a different constant in the pipeline, and
they are mutually constraining — the same experience as the DMG wave-RAM window in
[`docs/audio.md`](audio.md), where a ±2 change flipped three CRCs at once. Budget in weeks of
measurement, not days of typing.

*Performance.* Headless we run cpu_instrs at **3,226 fps (54× real time, 0.31 ms/frame)** today. A
per-dot PPU adds work on the order of 29k mode-3 dots per frame on top of the 23k pixels
`renderscan` already writes; the honest estimate is **1.3–2× frame cost**, i.e. roughly 27–40×
real time headless. That leaves the 16.74 ms frame budget untouched for every frontend, but it
would visibly lengthen the test-ROM harness runs. It must be **measured, not assumed**, before the
work is accepted — `render_bench --json` already reports the right number.

*Risk.* This is the highest-risk change on the board:
- dmg-acid2 is **pixel-perfect** today and cgb-acid2 passes; both are whole-screen compositions of
  BG + window + sprites and would be re-derived from scratch by a new renderer.
- `testharness/baseline.txt` now stores each ROM's own printed report, so a mode-3 rewrite that
  shifts one character of a register dump is caught — good, but it means every row must be read,
  not re-blessed.
- `Gpu::save_state` serialises `mode`/`modeclock` directly; a new state machine means new fields and
  a `savestate::FORMAT_VERSION` bump, and `testharness/savestate.rs` asserts byte-identical state
  snapshots at every frame boundary.
- `may_hdma()` drives HBlank DMA off `hblanking`; a variable-length mode 3 moves when HDMA fires,
  which is game-visible.
- STAT mode and `LY` become visible at dot granularity, which can move Blargg and Mooneye results in
  either direction.

*Mitigation that should be a precondition, not an afterthought:* keep the scanline renderer behind a
switch during the rewrite so every existing baseline can be re-run against both, and land the
fetcher only when the new path reproduces all 18 `testroms` results **and** improves Mealybug.

### 5.3 One more thing worth knowing before anyone quotes a number

The shootout's **published** table scores all 24 of its Mealybug rows against **DMG-blob**
references. That was unreachable until `Model::Dmg` landed on `main`
(`fix(testroms): a harness that can actually fail, and a real DMG mode`); it is reachable now, and
this page's DMG column is measured on a real DMG boot rather than on the Color console's
compatibility mode. The DMG and CGB columns still describe two different machines, though — do not
merge them into one number.

The corollary for whoever builds the fetcher: **the mode-3 model will have to be two models.**
Mealybug ships separate DMG, CPU CGB C and CPU CGB D references precisely because the pipelines
differ (Pan Docs on `SCY`: "all models before the CGB-D read the Y coordinate once for each
bitplane … but CGB-D and later use the same Y coordinate for both"). A single shared mode-3 path
can pass at most one of the three columns.

---

## 6. Conformance-tool notes

Per the [standing convention](../CLAUDE.md), for the two fixes:

1. **Window Y condition gated on window-enable.** *Symptom:* the window never appears for a game
   that sets `WY` while the window is off and enables it later in the frame. *Caught by:*
   Mealybug `m2_win_en_toggle`; **nothing else in our suite**, and that is itself the finding —
   dmg-acid2, Blargg and Mooneye all miss it. *Hardware:* Pan Docs "Window rendering criteria".
   *New test:* `gpu::tests::the_wy_condition_latches_while_the_window_is_disabled` +
   `examples/mealybug` pin.
2. **Mode-1 STAT source ignores the mode-2 select bit.** *Symptom:* one STAT interrupt per frame is
   lost; anything toggling on STAT lands on the wrong parity of scanlines. *Caught by:* Mealybug
   `m2_win_en_toggle` **and** `mooneye/acceptance/ppu/vblank_stat_intr-GS` — a PPU ROM catching an
   *interrupt* bug, which is an argument for scoring screens as well as serial verdicts.
   *Hardware:* TCAGBD §8.7. *New test:*
   `gpu::tests::entering_vblank_raises_stat_for_the_mode_2_source_too`.

And a coverage gap worth recording on its own: **no ROM-driven check we run detects that our VRAM
starts zeroed instead of holding the boot ROM's handoff state.** Twelve Mealybug ROMs depend on it,
so the suite exposes it — but only as part of a timing failure, which is exactly the kind of
conflated verdict [`docs/conformance.md`](conformance.md) argues a good tool should separate.

---

## 7. The pixel-FIFO engine, measured (14 Aug 2026)

The rewrite §5 scoped now exists, as a **second** engine beside the whole-scanline one rather
than a replacement for it: `RenderMode::Accurate` (`src/gpu/fifo.rs`), selected with
`GB_PPU=accurate`, the `ppu` config key, or `Gpu::set_render_mode`. `RenderMode::Fast` — the
renderer this page has been about — is unchanged and is still the default. Reproduce everything
below by putting `GB_PPU=accurate` in front of the command in the header of this page.

### 7.1 What it does

Per-dot mode 2 and mode 3: an OAM scan spread one sprite per two dots, a background/window
fetcher on its 6-dot cycle, an 8-pixel FIFO, sprite fetches that suspend the output, the window
restart with its own fetcher reset, the `SCX & 7` discard, and a **mode 3 length that falls out
of the machine**. Colour *indices* go into the FIFO and the palette is applied at the output,
which is the mechanism a mid-line `BGP` write needs. Every register is read where hardware reads
it: the tile-map select at the tile-number fetch, `LCDC.4` and `SCY` at each bitplane fetch,
`SCX`'s low three bits latched once per line and its high five re-read per tile, `LCDC.0` at the
output.

### 7.2 Where it stands, honestly

**It is structurally right and not yet calibrated.** Three claims it now carries:

| claim | fast | accurate |
|---|---:|---:|
| dmg-acid2 vs its author's reference | 0/23040 | **0/23040** |
| mode 3 length, plain BG line | 172 (constant) | **172** (derived) |
| mode 3 length, `SCX & 7 = n` | 172 | **172 + n** |
| Mooneye `acceptance/ppu` | 4/12 | 4/12, **not the same 4** |
| Mooneye `acceptance` overall | 58/75 | 58/75 |
| cost, dmg-acid2 | 0.219 ms/frame | 0.463 ms/frame (**2.1x**) |

The variable mode-3 length wins `ppu/hblank_ly_scx_timing-GS` outright and loses
`ppu/intr_1_2_timing-GS`; `oam_bug` sub-test 7 also regresses (see §7.4).

**Mealybug rows: still 3/79 — the same three.** That is the number that matters and it has not
moved. What *has* moved is the distance to the rest, and it has moved in both directions:

| test (DMG column) | fast | accurate |
|---|---:|---:|
| `m3_scx_high_5_bits` | 9,766 | **141** |
| `m3_lcdc_win_en_change_multiple` | 8,982 | **485** |
| `m3_scy_change` | 15,879 | **11,569** |
| `m3_wx_5_change` | 11,471 | **9,521** |
| `m3_window_timing` | 1,215 | 2,611 |
| `m3_bgp_change` | 5,296 | 17,392 |
| `m3_lcdc_obj_size_change_scx` | 190 | 22,032 |
| **total, all 79 rows** | **1,212,282** | **1,268,382** |

So the *aggregate* is marginally worse while the closest tests are one to two orders of
magnitude closer. Both halves of that are informative and neither should be quoted alone:

* The tests that collapsed (`m3_bgp_change`, `m3_lcdc_obj_size_change_scx`) render an almost
  **blank frame**, not a mistimed one. These ROMs synchronise themselves against mode 3 —
  they wait on `STAT` and then write on a counted dot — so **a variable mode-3 length that is
  wrong is worse for them than a constant one that is honestly wrong**, because a fixed 172
  at least keeps the ROM's own loop in step. This is the mechanism §5.2 warned about ("each of
  the remaining ROMs pins a different constant, and they are mutually constraining"), showing
  up on schedule.
* The tests that improved are the ones whose result is a direct function of *where* a register
  is read rather than of the length of the mode.

### 7.3 The remaining work, and what it is not

The remaining work is **calibration, not architecture**: the sprite penalty (Pan Docs' OBJ
penalty algorithm, 6-11 dots, currently a flat 6 plus the fetcher's wait), the window's WX
penalty, and the exact dot on which a CPU write becomes visible to the fetcher. Evidence that
this is genuinely the hard part rather than a missing feature: sweeping the tile-number fetch
from dot 2 to dot 4 of the 6-dot cycle moves `m3_scx_high_5_bits` from 141 to 125 pixels and
the whole-suite total by 114 pixels out of 1.27 million, and flips **no** row. There is no
single constant left to find; there are many, and they constrain each other.

§4's calibration stands and is worth re-reading before budgeting more: **SameBoy scores 15 of
the 24 published DMG rows and bgb scores 2.** A cycle-stepped PPU is the price of entry, not
the prize.

### 7.4 What the accurate engine is behind on

Recorded rather than smoothed over — both are the accurate engine being *worse* than the
default, and both block making it the default:

1. **Blargg `oam_bug` sub-test 7** ("verifies corruption for each timing"), 8/8 → 7/8. Not a
   phase problem: the two engines' corruption rows agree dot for dot, and no value of the phase
   constant recovers it (`-12` through `+12` swept; only `0` passes 4/5/6/8, and none passes 7).
   The engines diverge only after the ROM re-enables the LCD mid-test.
2. **Mooneye `ppu/intr_1_2_timing-GS`**, pass → fail, traded for `hblank_ly_scx_timing-GS`.

### 7.4a It is gated in CI

`make test-external-accurate` scores Mooneye and Mealybug on the accurate
engine against `testharness/mooneye_accurate_baseline.txt` and
`testharness/mealybug_accurate_baseline.txt`, and CI runs it beside
`make test-external` (which scores the default engine). Those baselines carry
**pixel counts, not just PASS/FAIL**, so a change to the pipeline is a visible
diff whether or not it flips a row — which, on a suite this far from green, is
the only way progress and regression are both legible.

Every baseline file now opens with the engine that produced it and the `make`
target that re-blesses it, because the two engines score the same ROMs
differently and the obvious command would otherwise rewrite a file with the
other machine's numbers.

### 7.5 Recommendation

**Keep both engines for now; ship one later.** The captain's condition for deleting the fast
path was that the accurate one be fast enough on ordinary hardware, and on cost it plainly is —
2.1x, 2.8 % of a 16.7427 ms frame, 36x real time headless, inside the 1.3-2x band §5.2
predicted. Cost is no longer the argument for two renderers. The argument that remains is the
two rows in §7.4: until the accurate engine strictly dominates, a default flip would make a
published score worse, and `testharness/baseline.txt` must never be re-blessed to hide that.
The order is: close §7.4, flip the default, run the conformance suite against the accurate
engine, then delete `renderscan`.

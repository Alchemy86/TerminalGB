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

---

## 8. Four fixes later: 3/79 → 6/79, and where the rest of the distance actually is

§7 measured the pixel FIFO on its first outing. This section is the second pass over it. Every
figure below is `GB_PPU=accurate` against the ROMs pinned in
[`tools/fetch-test-suites.sh`](../tools/fetch-test-suites.sh), and **the default renderer is
byte-identical to its checked-in baselines throughout** — `make test-external` was green after
each of the four changes.

| | §7 (first outing) | now |
|---|---:|---:|
| rows pixel-exact | 3 / 79 | **6 / 79** |
| DMG column | 1 / 24 | **2 / 24** |
| whole-suite pixel distance | 1,268,382 | **693,023** |
| Mooneye `acceptance` | 58 / 75 | **61 / 75** |
| Mooneye overall (`acceptance` + `emulator-only`) | 86 / 103 | **89 / 103** |
| cost, dmg-acid2 | 2.1x fast | 2.13x fast (0.263 → 0.559 ms/frame) |

`m3_lcdc_win_en_change_multiple` joins `m2_win_en_toggle` as pixel-exact against all three
hardware reference sets. On the shootout's published DMG denominator that is **2/24 — level
with bgb** ([§4](#4-calibration-how-hard-is-this-really)), with SameBoy still on 15.

### 8.1 The one that was not a PPU accuracy problem at all

**The accurate engine never left mode 0 after the LCD came on, for the rest of the frame.**

The line the PPU restarts on after `LCDC.7` goes 0→1 reads mode 0 *through* its OAM slot and
leaves it only by entering mode 3 at dot 80 — which is the one place that clears the
"no OAM scan" latch. `do_cycle_accurate`'s "mode 0 has no per-dot behaviour, skip to the end of
the line" shortcut fired there, so dot 80 was never reached, the latch survived to the next
line, and the next: `LY` ticked over with the PPU parked in mode 0 and **no mode 2, no mode 3
and no STAT interrupt** until the VBlank at line 144.

Every Mealybug ROM arms itself on the mode-2 STAT source and then falls through a field of
`nop`s waiting for it. With no strobe the program counter ran off the end of the slide (1200
`nop`s ≈ 10 scanlines) long before line 144, `reti`'d on a stack that had never been pushed,
and looped through address `$0000` into the VBlank vector. Traced on `m3_bgp_change`: ten
"frames" counted in 80,668 cycles, **one** `BGP` write in the whole run, a blank single-shade
frame. That — not mid-scanline accuracy — is what most of "3/79" was measuring.

Pinned by `gpu::fifo::lcd_restart`, whose first assertion on the old code reads *"the PPU never
left mode 0 on the line the LCD came on"*.

### 8.2 The OAM strobe rises one M-cycle before its scanline

Mooneye's `intr_2_0_timing`, `intr_2_mode0_timing` and `intr_2_mode3_timing` each measure the
gap from the mode-2 STAT interrupt to the next PPU event, and each came out exactly 4 T-cycles
short. Dispatch latency is common to *both* ends of `intr_2_0_timing`'s measurement, so it
cancels there — the error was in the signal, not in the CPU. `intr_2_mode0_timing` goes
FAIL → PASS.

Two exclusions, each demanded by a test: line 144 keeps its strobe at dot 0, simultaneous with
the VBlank interrupt (`vblank_stat_intr-GS`), and **line 0 gets no early window** — which
Mealybug states in its own source, `m3_bgp_change` commenting *"line 0 timing is different by
4 cycles"* and padding its handler by one taken `jr` on every line except line 0 to compensate.

Only the per-dot engine emits it. The whole-scanline renderer changes `mode` at instruction
granularity, so it has no "one M-cycle before" to represent, and asserting a 4-dot signal
inside it measures as mixing resolutions: Mooneye does not move (it cannot resolve 4 dots)
while Mealybug loses `m2_win_en_toggle`, the one row that engine has pixel-exact, and
`m3_window_timing` goes 1,215 → 21,671 pixels.

### 8.3 Pan Docs' OBJ penalty algorithm, calibrated against Mooneye's own table

The engine used to derive the object penalty from whatever phase its fetcher happened to be in.
That reproduced the *maximum* and nothing else — ten objects at `X = 0` and ten at `X = 7` both
cost 66 dots where hardware wants 65 and 60 — and an object at `X < 8`, which hangs off the
left edge and draws no pixels, cost nothing at all because it was never fetched.

`acceptance/ppu/intr_2_mode0_timing_sprites` states the answer for 39 arrangements. Read back
as dots (its unit is one M-cycle, truncated) they are exactly **6 dots per object, plus
`max(0, 5 - ((X + SCX) % 8))` for the first object on each background tile** — Pan Docs,
[Rendering § OBJ penalty algorithm](https://gbdev.io/pandocs/Rendering.html). All 24
arrangements sampled by `the_obj_penalty_matches_mooneyes_table` now match to the dot.

This is the fetcher's phase, so it moves the rows that photograph *which tile* a mid-mode-3
write caught: `m3_lcdc_tile_sel_change2` @CGB-C 22,214 → 1,008, `m3_scy_change2` 18,129 →
2,924, `m3_lcdc_tile_sel_win_change2` 22,511 → 946, `m3_scx_high_5_bits` @CGB 150 → **12**.

### 8.4 The window's takeover is decided at the output

`check_window_start` ran at the top of every mode-3 dot, including the twelve dots of the
head-of-line fetch — where `lx` is already 0 and the FIFO is still empty, so a window with
`WX <= 7` took over a fetcher that had nothing to throw away and cost nothing. The comparison
is made at the **output**, against the pixel about to be shifted out, so it cannot happen
before there is one; gating it on a non-empty FIFO is the whole change, and it restores Pan
Docs' "if the window is enabled and the current scanline reaches it, mode 3 is 6 dots longer"
for every `WX`, not just for `WX > 7`.

`m3_window_timing` is built entirely out of that difference: it sets `WX = LY` on every line and
photographs a window that started before a mid-line `BGP` write against one that started after.
Our boundary used to sit at pixel 12 for **every** `WX`; hardware's sits at 3 and 9, and ours
now splits 6 and 12 — the right shape, three pixels late.

### 8.5 Where the remaining 693,023 pixels are — and how much of it is winnable

This matters more than the total, because three quarters of it is **not** mid-scanline timing.

| block | rows | pixels | what it is |
|---|---:|---:|---|
| post-boot VRAM (the `®` glyph) | 9 | **210,018** | measured, see §8.6 — a policy call, not a PPU problem |
| CGB references that are colour photographs | 8 | ~182,500 | structurally uncomparable, see §8.7 |
| `m3_lcdc_obj_size_change_scx`, `m3_wx_4_change_sprites` | 6 | ~135,000 | real errors, magnified ~500x by the metric (§8.8) |
| genuine mid-scanline calibration | 56 | ~165,500 | the actual remaining work |

**Rows now within a few hundred pixels of exact** (DMG / CGB-C / CGB-D):
`m3_scx_high_5_bits` **77 / 12 / 12**, `m3_window_timing` 418 / 280 / 418,
`m3_lcdc_obj_en_change_variant` 582 / 498 / 642, `m3_lcdc_obj_size_change` 641 / 611 / 611,
`m3_window_timing_wx_0` 705 / 595 / 705, `m3_lcdc_bg_en_change` 866 / 463 / 463,
`m3_lcdc_win_map_change` 892 / 710 / 710, `m3_lcdc_bg_map_change` 922 / 538 / 538, plus
`m3_lcdc_tile_sel_win_change2` 946 and `m3_lcdc_tile_sel_change2` 1,008 on CGB-C.

### 8.6 Blocker C is now measured, and it is **eight bytes**

[§3.3](#33-blocker-c--we-do-not-reproduce-post-boot-vram) scoped this by counting ROMs. Having
run the experiment, the shape is much narrower than that section implies:

* The **logo tiles `$01`-`$18` buy nothing.** Every ROM that would show them either calls the
  suite's own `copy_font` or selects the `$8800` signed tile block, where the logo lives at
  addresses that are still zero. Not one Mealybug row depends on them — which also means the
  licence-free half of Blocker C (the logo is decompressed from the *cartridge's own* header
  bytes `$0104`-`$0133`) has no payoff at all.
* **Ten ROMs use tile `$19` — the `®` glyph — and nothing else.** Four of them draw *only* that
  (`m3_obp0_change`, `m3_lcdc_obj_en_change`, `m3_scx_low_3_bits`, `m3_wx_4_change_sprites`);
  the other six place every sprite with it.

Measured by writing the boot ROM's eight-byte `®` bitmap to `$8190` (low bitplane, high plane
zero, exactly as the DMG boot ROM leaves it) on both DMG and CGB-compat machines, then
reverting:

| row | now | with the `®` |
|---|---:|---:|
| `m3_obp0_change` @DMG/@CGB-C/@CGB-D | 22,750 / 22,792 / 22,750 | **108 / 66 / 108** |
| `m3_lcdc_obj_en_change` | 22,894 / 22,834 / 22,834 | **256 / 196 / 196** |
| `m3_scx_low_3_bits` | 22,500 / 22,500 / 22,500 | **324 / 324 / 324** |
| `m3_lcdc_bg_map_change` | 922 / 538 / 538 | 524 / 140 / 140 |
| `m3_lcdc_win_map_change` | 892 / 710 / 710 | 494 / 312 / 312 |
| `m3_bgp_change_sprites` | 2,772 / 2,444 / 3,160 | 1,800 / 1,472 / 2,188 |
| **whole suite** | **693,023** | **483,005** |

So the `®` is worth **210,018 pixels, 30 % of everything left**, and it takes the three worst
rows in the suite to double figures — closer to exact than anything except
`m3_scx_high_5_bits`. It is the single largest lever on this board and it is not a PPU change.

**It remains the captain's call and has not been taken.** The case for it, stated plainly so
the decision can be made once: this repository already reproduces the boot ROM's entire I/O
register map, its system-counter value, its APU register writes and the tilemap entry that
*points at tile `$19`* (`MemoryManagementUnit::set_boot_logo_tilemap` writes `$19` to `$9910`
today). The glyph's pixels are the same category of thing — machine state at `PC=$0100`,
transcribed — differing only in that eight of those bytes happen to draw a standard
typographic symbol. The case against is the standing rule in
[`CLAUDE.md`](../CLAUDE.md#licence-boundary-format-knowledge-is-ours-game-assets-are-not): it
is data out of somebody else's ROM, and we do not ship those. Nothing here decides it.

### 8.7 Eight rows compare a greyscale frame against a colour photograph

Four ROMs' CGB references — `m3_wx_4_change`, `m3_wx_5_change`, `m3_wx_6_change` and
`m3_lcdc_win_en_change_multiple_wx` — carry **125 distinct luminances**, against four or fewer
in every other reference in the suite. They are photographs of a Color console that has
colourised a DMG cartridge from the boot ROM's title-hash palette, and that palette happens to
be a coloured one for those four titles. We render `ColorAsClassic` in greyscale, so
`examples/mealybug` falls back to bucketing the reference's luminance span evenly into four —
which cannot line up with what the palette actually did.

Those eight rows are ~182,500 pixels, **26 % of the remaining total, and no amount of PPU work
moves them.** Closing them needs the CGB boot ROM's compatibility-palette selection, which is
a different feature *and* the same licensing question as §8.6 (the palette table is boot ROM
data). Do not read their pixel counts as PPU progress in either direction.

Counted, so nobody has to re-derive it: DMG-blob has 0 references with more than four shades;
CPU CGB C has 7 (the four above plus three at exactly five shades, which do compare usefully);
CPU CGB D has 4.

### 8.8 The pixel count is a **rank** distance, and it is wildly non-linear

The comparison ranks each frame's distinct luminances and compares ranks, which is right for
pass/fail (a palette difference cannot manufacture or mask a result) and badly misleading as a
magnitude. **If our frame has one more distinct shade than the reference, every rank shifts and
essentially every pixel is reported as different.**

Worked example, `m3_lcdc_obj_size_change_scx` @DMG: our frame contains **four** pixels of a
shade hardware does not produce there. The frame is otherwise structurally right. It scores
**22,070 / 23,040**. `m3_wx_4_change_sprites` is the same story at 23,039.

Consequences, both of which apply to anyone reading
[`testharness/mealybug_accurate_baseline.txt`](../testharness/mealybug_accurate_baseline.txt):
a row near 22,500 means "wrong shade count", not "nothing works", and a change that fixes a
handful of pixels can move the total by 60,000. The metric is deliberately left alone — it is
the same shade-rank rule as `tools/conformance/image.py` and changing it mid-flight would
invalidate every comparison against the committed baselines — but quote it with this caveat.

### 8.9 What the next pass should do, in order

1. **Settle the `®`** (§8.6). 210,018 pixels and three rows within ~100 of exact, for eight
   bytes and a decision. Nothing else on this board has that ratio.
2. **Find the residual head-of-line offset.** After §8.2 and §8.4 the two output-stage ROMs
   agree with hardware in *shape* and sit a constant **3 pixels late** (`m3_bgp_change`'s bands
   start at 4 / 76 / 148 against hardware's 1 / 73 / 145; `m3_window_timing`'s boundary is 6
   and 12 against 3 and 9). `m3_lcdc_bg_en_change` says **2**, and only for the first
   transition of each pair — so it is not one constant, and it is not the interrupt path
   (that is M-cycle quantised, and 3 is not a multiple of 4). It is somewhere in the twelve
   dots between mode 3's start and the first pixel. Worth one focused session; it is the last
   *uniform* error visible in the suite.
3. **Then the per-tile work** — `m3_lcdc_obj_size_change_scx`'s four stray pixels, the
   `m3_wx_*` family, `m3_scy_change` on CGB-D (15,091 against 562 on CGB-C, which is the
   DMG-vs-CGB-D bitplane difference [§5.3](#53-one-more-thing-worth-knowing-before-anyone-quotes-a-number)
   predicted). This is genuinely the mutually-constraining calibration §5.2 warned about;
   budget it in weeks, not days.

### 8.10 What is still true from §7

The two rows in [§7.4](#74-what-the-accurate-engine-is-behind-on) still block flipping the
default: Blargg `oam_bug` sub-test 7, and `intr_1_2_timing-GS`. Both engines remain gated in
CI ([§7.4a](#74a-it-is-gated-in-ci)), and the accurate engine's baselines carry pixel counts
so a pipeline change is a visible diff whether or not it moves a row.

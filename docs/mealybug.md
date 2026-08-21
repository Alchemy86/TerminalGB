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
> `Model::Cgb`) and compares each column against its own frame. It is **three** machines now,
> not two: the two Color reference sets are different CPU steppings and seven ROMs disagree
> between them — see [§10.2](#102-m3_window_timingcgb-c-and-m3_bgp_changecgb-c-were-never-a-fetcher-bug). Today the two produce identical
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

> **Corrected, 2026-08-21 — this section's "18 of the 31" undercounted the correction it needed
> by half.** §11.1/§11.3 below (15 Aug 2026) already fixed the arithmetic properly and this
> section was never brought in line with it; a third, independent pass re-derived the same split
> from the pinned test ROMs' own `.asm` source and confirms it exactly. Of the 18 ROMs named
> below, only **9 are genuinely blocked by the missing `®` glyph**: `m3_obp0_change`,
> `m3_lcdc_obj_en_change`, `m3_lcdc_obj_en_change_variant`, `m3_scx_low_3_bits`,
> `m3_lcdc_bg_map_change`, `m3_lcdc_tile_sel_change`, `m3_lcdc_tile_sel_win_change`,
> `m3_lcdc_win_map_change`, `m3_bgp_change_sprites`. Of the other 9: **7** (the `*2` suffix
> ROMs) fail for the unrelated ROM/reference mismatch §11.3 diagnoses and never reference `$19`
> at all; **1** (`m3_wx_4_change_sprites`) places sprites at `$19` and then overwrites the whole
> of `$8000`-`$8FFF` with `$FF` before the LCD ever turns on, so the glyph is gone before it can
> be seen and the row is pixel-exact already; and **1** (`m3_bgp_change`) never touches OAM or a
> BG tile at all — its `$8800` signed addressing puts its whole picture at an address neither the
> boot ROM nor anything else here ever writes, and it too is pixel-exact already. **Zero of the
> 18 need only the licence-clean logo tiles (`$01`-`$18`)** — every row that needs anything from
> post-boot VRAM needs the `®` glyph specifically, which is why decompressing the logo tiles
> (now done, licence-clean, see `AGENTS.md` "The captain's decision confirmed") moves none of
> them. **The captain's decision is now settled policy, not an open question**: skip the
> Nintendo boot text entirely (`docs/mealybug.md` §8.6, confirmed 2026-08-21) — these 9 rows
> stay red under the default configuration, permanently, by deliberate choice, and licence-clean
> regression coverage of the *mechanism* they exercise lives in GBSelfTest (`GB-BOOT-07`,
> `GB-PPU-23`) instead. Do not re-litigate this count; the paragraphs below are kept for the
> reasoning trail that led here, not as the current figure.

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
glyph is boot-ROM data. **That was a licensing/policy call for the captain, not a PPU decision** —
see the licence-boundary section of [`AGENTS.md`](../AGENTS.md). **Both halves are now settled: the
logo tiles are decompressed (licence-clean, §8.6's "captain's decision confirmed" entry) and the
glyph stays undone, permanently, by deliberate choice — not one row of this section moved either
way, because (per the correction above) none of the 18 needed the logo tiles at all.**

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
than a replacement for it. It is the user-facing **`identical` picture mode**
(`RenderMode::Accurate`, `src/gpu/fifo.rs`), selected with the terminal's `P` key,
`GB_PPU=identical`, the `ppu` config key, or `Gpu::set_render_mode`. The **`standard`** mode
(`RenderMode::Fast`) — the renderer this page has been about — is unchanged and is still the
default; [`docs/picture-modes.md`](picture-modes.md) is the reader-facing page for the choice.
Reproduce everything below by putting `GB_PPU=identical` in front of the command in the header
of this page (the older spelling `GB_PPU=accurate` still parses, and is what the `make` targets
use).

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

> **Superseded — both rows are closed. See
> [§9](#9-the-line-0-stat-phase-679--1279-and-the-last-blocking-row).** Item 1 below was
> already stale when it was written down: `oam_bug` sub-test 7 was fixed by §8's phase work
> and this note was never amended. Item 2 was real and is fixed in §9. Kept for the record.

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

### 7.5 Recommendation — **carried out, 2026-08-20**

> **The default is `identical` now.** This section's order was: close §7.4, flip the default,
> run the conformance suite against the accurate engine, then delete `renderscan`. The first
> three are done; the fourth is **not**, and deliberately — see the update at the end.

**Keep both engines for now; ship one later.** The captain's condition for deleting the fast
path was that the accurate one be fast enough on ordinary hardware, and on cost it plainly is —
2.1x, 2.8 % of a 16.7427 ms frame, 36x real time headless, inside the 1.3-2x band §5.2
predicted. Cost is no longer the argument for two renderers. The argument that remains is the
two rows in §7.4: until the accurate engine strictly dominates, a default flip would make a
published score worse, and `testharness/baseline.txt` must never be re-blessed to hide that.
The order is: close §7.4, flip the default, run the conformance suite against the accurate
engine, then delete `renderscan`.

#### What actually happened

§7.4's two rows closed (§9.1), and a third counter-example turned up in between that this
section did not know about: the c-sp aggregation, wired up after §7 was written, found
`cgb-acid2` 158 pixels wrong on the per-dot engine where the batched one is pixel-exact. That
closed too — the window pixel-insertion glitch needs the window to be *drawing* on Color
silicon (`docs/conformance-notes.md`, 2026-08-20) — and with it went 63 Gambatte rows.

Measured through the GB Emulator Shootout's own 264-row manifest, same build, `GB_PPU` the only
difference: **`standard` 223, `identical` 238 — fifteen gains, zero losses.** Eight Mooneye
`acceptance/ppu` rows and seven Mealybug DMG rows. The flip re-blessed **no baseline**: both
`make` targets name their engine now, so each file set stayed with the engine that produced it.

**`renderscan` was NOT deleted, and the reason is not caution.** `standard` stopped being "the
default" and became "the cheap engine", which is a job it is the only thing that can do:

* the PSP frontend pins it, because that console spends 9.4-9.9 ms of a 16.743 ms frame already
  and four times the PPU there is unplayable rather than 2 %;
* a headless collector's `set_render_pixels(false)` is a `standard`-only saving — under
  `identical` the fetcher *is* the timing — and it is worth **6.7x** on a real cartridge
  (0.057 against 0.382 ms/frame, Pokémon Blue, one P-core);
* speed mode refuses to run under `identical` at all, so every shortcut in `src/speed.rs` needs
  it;
* and it is the second opinion the `ppuengines` gate is built on: 600 frames of dmg-acid2 and
  of `cpu_instrs` through both engines, required byte-identical, every `cargo test --release`.

Deleting it would cost all four to save a file nobody is paying for. If it is ever revisited,
revisit it as *"can the per-dot engine be made cheap enough for a PSP"*, not as tidying.

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
| Mooneye `acceptance` | 58 / 75 | **59 / 75** (+`intr_2_mode0_timing`) |
| cost, dmg-acid2 | 2.1x fast | 2.13x fast (0.263 → 0.559 ms/frame) |

> **Those Mooneye figures are the before/after of *this* work in isolation, measured on the
> base it started from.** `main` has since gained the nine-machine model work, so the absolute
> numbers are higher: re-measured at `e58b651`, `GB_PPU=accurate` scores **68/75 acceptance,
> 28/28 emulator-only, 8/8 misc** and the default engine scores 67/75 — the +1 for
> `intr_2_mode0_timing` (§8.2) survives on top. The Mealybug figures are unchanged by that
> work, re-measured at the same commit.

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

> **Later correction (2026-08-15):** this lead is right, but it is not the whole story, and for
> a while it was recorded as a knob that could not satisfy both suites at once. What was
> missing is that being asserted *inside* an M-cycle is the point: a halted CPU is one M-cycle
> slower to act on such a request than a running one, which is the only difference between
> Mooneye's five `intr_2_*` (`ei / halt`) and Mealybug's ROMs (a `nop` slide). With that
> modelled, `acceptance` is 75/75 **and** Mealybug is 17/79 with the lead untouched — see
> [`conformance-notes.md`](conformance-notes.md), "a mid-M-cycle interrupt reaches a halted CPU
> late".

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

### 8.6 Blocker C: measured, decided, and now a **user-supplied boot ROM**

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

So the whole of Blocker C is eight bytes of bitmap in somebody else's boot ROM.

**The decision (captain, 2026-08-14): do not embed them — support an optional user-supplied
boot ROM instead.** A register value or a counter phase is a fact about hardware and not
copyrightable expression, which is why this emulator transcribes the entire `$FF00-$FF7F` map
by hand; a glyph bitmap is expressive content, and this repository is public, MPL-2.0 and has a
standing requirement to be freely distributable. Anyone may lawfully dump the boot ROM of
hardware they own, so the accurate route is the one SameBoy and others take: run the user's own
image when there is one, and fall back to today's high-level emulation when there is not.

That is implemented ([`src/boot.rs`](../src/boot.rs)). Nothing is bundled, nothing is fetched,
and every failure — no file, unreadable, wrong length, wrong console — is silently the HLE
machine:

```sh
${XDG_CONFIG_HOME:-~/.config}/terminalgb/dmg_boot.bin    # 256 bytes
${XDG_CONFIG_HOME:-~/.config}/terminalgb/cgb_boot.bin    # 2304 bytes
```

`$GB_BOOT_ROM=<path>` overrides both. The terminal frontend picks one up automatically and says
so; the **suite runners do not** — `examples/mealybug` takes `--boot-rom <path>` explicitly and
refuses to be combined with `--baseline`, because a gated score that depended on what happens to
be in somebody's `~/.config` would not be a gate.

#### Both numbers

Every checked-in baseline, every CI job and every published score in this repository is the
**left** column. Reproduce the right one with your own dump:

```sh
GB_PPU=accurate cargo run --release --example mealybug -- \
    --dir <mealybug checkout> --boot-rom ~/.config/terminalgb/dmg_boot.bin
```

| | HLE (default, and what CI gates) | with a boot ROM |
|---|---:|---:|
| rows pixel-exact | 6 / 79 | 6 / 79 |
| DMG column | 168,453 px | **98,555 px** |
| CPU CGB C column | 292,612 px | **222,510 px** |
| CPU CGB D column | 231,958 px | **161,940 px** |
| **whole suite** | **693,023 px** | **483,005 px** (−30 %) |

Per row, DMG column:

| row | HLE | with a boot ROM |
|---|---:|---:|
| `m3_obp0_change` | 22,750 | **108** |
| `m3_lcdc_obj_en_change` | 22,894 | **256** |
| `m3_scx_low_3_bits` | 22,500 | **324** |
| `m3_lcdc_win_map_change` | 892 | 494 |
| `m3_lcdc_bg_map_change` | 922 | 524 |
| `m3_lcdc_tile_sel_win_change` | 1,576 | 1,184 |
| `m3_lcdc_tile_sel_change` | 1,778 | 1,386 |
| `m3_bgp_change_sprites` | 2,772 | 1,800 |

**No row flips to green**, and that is the honest headline: the boot ROM removes a *content*
error the emulator could never have fixed, and what is left in those rows is mid-scanline
timing, which is ours. Three of them land inside a few hundred pixels of exact, which is where
`m3_scx_high_5_bits` (12 px) already sits — so they are now real targets rather than blanks.

**How these figures were produced, stated exactly.** There is no boot ROM on the machine this
was measured on and the rule is never to fetch one, so the right-hand column was measured
through the shipped `--boot-rom` path using a 256-byte and a 2304-byte stand-in written for the
purpose in `/tmp` — programs of our own that reproduce the post-boot state the real image
leaves (the tile at `$8190`, the tilemap, `BGP`, `LCDC` and the documented register handoff) and
then unmap themselves with `ldh [$50],a`, exactly as a real boot ROM's last instruction does.
Nothing from either stand-in is in this repository. The figures agree to the pixel with an
earlier experiment that injected the same handoff state directly into the MMU, which is the
cross-check that they are measuring the state and not the mechanism.

A real dump will additionally supply the `DIV` phase and the register file, which the HLE path
already gets exactly right (`acceptance/boot_div-dmgABCmgb`, `boot_hwio-dmgABCmgb` and
`serial/boot_sclk_align-dmgABCmgb` all pass without one), so the right-hand column should be
what a real image scores.

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

1. ~~**Settle the `®`**~~ — **done** (§8.6): a user-supplied boot ROM, never a bundled one.
   Worth 210,018 pixels to anyone who supplies a dump, and it takes three rows from ~22,750 to
   108/256/324 — which turns them from blanks into the same kind of mid-scanline target the
   rest of this list is about. It flips no row on its own.
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

~~The two rows in [§7.4](#74-what-the-accurate-engine-is-behind-on) still block flipping the
default~~ — **both are closed; see
[§9](#9-the-line-0-stat-phase-679--1279-and-the-last-blocking-row).** Both engines remain
gated in CI ([§7.4a](#74a-it-is-gated-in-ci)), and the accurate engine's baselines carry pixel
counts so a pipeline change is a visible diff whether or not it moves a row.

---

## 9. The line-0 STAT phase: 6/79 → 12/79, and the last blocking row

**Measured with `make test-external` / `make test-external-accurate` on the ROMs pinned in
`tools/fetch-test-suites.sh`, before and after, same build.** Mealybug on the accurate engine
**6 → 12 of 79 rows pixel-exact**, total distance **693,023 → 646,838 px**; Mooneye
`acceptance` **68 → 69/75** (`acceptance` + `emulator-only` 96 → 97/103) with
`ppu/intr_1_2_timing-GS` **FAIL → PASS**. The default engine is **byte-identical** throughout
— `testharness/baseline.txt`, `mealybug_baseline.txt`, `mooneye_baseline.txt`,
`mooneye_cgb_baseline.txt`, `samesuite_baseline.txt` and `cpp_baseline.txt` are all untouched.

Six rows gained: `m3_bgp_change@DMG`, `@CGB-D`; `m3_lcdc_bg_en_change@CGB-C`, `@CGB-D`;
`m3_window_timing@DMG`, `@CGB-D`.

### 9.1 §7.4 is closed, and one half of it was already stale

[§7.4](#74-what-the-accurate-engine-is-behind-on) named two rows as blocking a default flip.
Both are gone, and **only one of them was ever fixed here**:

1. **Blargg `oam_bug` sub-test 7 was already passing.** Measured at the pristine parent commit
   with `GB_PPU=accurate cargo test --release --test testroms`:
   `oam_bug 01:ok 02:ok 03:ok 04:ok 05:ok 06:ok 07:ok 08:ok Passed`. The
   `ACCURATE_OAM_BUG_PHASE = 0` work described in §8 fixed it and §7.4 was never amended. The
   standing lesson from the APU campaign applies exactly: **check the baseline, not a prose
   note, before starting on a named gap.** (`oam_bug@cgb` still fails, and must — Color
   silicon has no OAM-corruption defect, so a real CGB fails that ROM too.)
2. **`intr_1_2_timing-GS`** is what this section fixes.

### 9.2 The OAM strobe asked `mode`, which lags `LY` by a dot

`Gpu::oam_strobe` derived "the OAM scan is running" from `mode == 2`. `mode` is moved by the
dot loop, which runs *after* `next_line` has advanced `LY` — so the dot that starts a scanline
still reads mode 0. And because `MemoryManagementUnit::bus_tick` folds `IF` **once per
M-cycle**, "a dot late" is a whole M-cycle late to the CPU.

Two consequences, and the first is why it survived so long:

* **Lines 1-143 raised two STAT interrupts each.** The strobe stopped at the line boundary and
  restarted a dot later, so there was a second rising edge. It is invisible to any program that
  services the interrupt immediately, because the second edge lands *inside* the first one's
  dispatch, which clears `IF.1` at M4 after it — the pair collapses into one.
* **Line 0 had nothing to collapse into.** By design it gets no early window (lines 1-143 rise
  4 dots early, on the previous line's tail; line 144 keeps dot 0; line 0 has neither). So on
  line 0 the whole defect was exposed: the interrupt arrived **8 dots** after the equivalent
  point on lines 1-143 where hardware wants **4**.

`m3_bgp_change`'s own source says what hardware wants, and pads its handler to prove it:

```
    ; line 0 timing is different by 4 cycles, so jump only
    ; when on line 0
    ldh a, [rLY]
    and a
    jr nz, .line_0      ; taken (12 T) on lines 1-143, not taken (8 T) on line 0
.line_0:
```

The handler is deliberately **4 T shorter on line 0**, to cancel a line-0 interrupt that
hardware delivers 4 T later. Ours was 8 late, so the 4-T pad left exactly 4 over — and the
residual was line 0, and only line 0, four pixels to the right on every ROM that syncs off the
mode-2 source.

**Measured on both sides.** Tracing the handler's own `ldh a,[rLY]`: dot **44** on lines 1 and
2, dot **52** on line 0 — before. Tracing its `BGP` writes: `77, 93, 105, 165, 177, 237, 249`
on lines 1-2 against `81, 97, 109, 169, 181, 241, 253` on line 0, a uniform +4. Afterwards the
reference's line 0 and line 1 are identical, as they are on hardware, and so are ours: the edge
positions on both lines read `[1, 14, 73, 86, 145, 158]`, matching Matt Currie's photograph
pixel for pixel.

The fix asks `LY` and the dot counter instead of `mode`, on the accurate engine only —
`self.line < VBLANK_LINE && self.modeclock < OAM_DOTS && !self.skip_first_oam`. Pinned by
`gpu::tests::a_visible_line_strobes_the_oam_source_exactly_once`, whose teeth are proven by
reverting the arm (it then counts two raises on line 3).

### 9.3 A dead end worth recording

The obvious first move — *give line 0 the early window too*, on line 153's tail — is wrong and
was measured wrong: Mealybug **6 → 3** rows, and `intr_1_2_timing-GS` still red. It moves line
0 to the *same* phase as lines 1-143, where hardware wants it one M-cycle later; the ROM's
4-T pad then over-corrects in the other direction. The three placements available at M-cycle
granularity are +0, +4 and +8 relative to lines 1-143; hardware is +4, we were on +8, and that
experiment is +0.

### 9.4 The write-commit model underneath it

The line-0 fix sits on top of a second piece of work in the same change: a PPU-bound register
write now commits **partway through its own M-cycle** rather than at the end of it
(`mmu::ppu_write_commit`, `MemoryManagementUnit::gpu_defer`). `bus_tick` advances the per-dot
PPU a whole M-cycle *before* every access, which put every mid-scanline register boundary most
of an M-cycle late. The physical model — the CPU flushes a write on the second T-cycle of the
M-cycle, plus per-register settling delays including the DMG's one-dot `old | new` palette
glitch — is read from [docboy](https://github.com/Docheinstein/docboy) and reimplemented per
`CONTRIBUTING.md`; each arm carries its citation and the row that measures it. On its own it
took the suite **693,023 → 669,162 px** and turned the DMG column of `m3_bgp_change` from 2,244
pixels to 22; the line-0 fix took those last 22 to zero.

### 9.5 Is the accurate engine ready to be the default?

**On accuracy, yes — it lost no row to the default on any gate this table had.** Measured
engine-for-engine on the same build:

> **Correction, 2026-08-20.** "Any gate" meant the six below. The c-sp aggregation, wired up
> the day after, found three places `identical` scored *worse* — `cgb-acid2` and Gambatte's
> `sprites` and `window` groups. `cgb-acid2` and `window` are closed; `sprites` and three
> smaller groups are still net-negative by **eight rows between them, against 891 gains**, and
> that is stated on [`docs/picture-modes.md` §3](picture-modes.md#3-what-each-mode-scores)
> rather than rounded away. The flip went ahead on the evidence that matters for a published
> score: through the shootout's own manifest, fifteen gains and no losses.

| gate | `fast` | `accurate` |
|---|---:|---:|
| in-repo harness (`testroms`) | 46/47 | 46/47 — *identical verdicts, row for row* |
| Mooneye `acceptance` + `emulator-only` | 95/103 | **97/103** |
| Mooneye `misc` (CGB) | 8/8 | 8/8 |
| SameSuite | 72/76 | 72/76 |
| CasualPokePlayer (`cpp`) | 4/4 | 4/4 |
| Mealybug Tearoom | 3/79 | **12/79** |

**What remains is a cost decision, not an accuracy one.** `testharness/ppuengines.rs` measures
0.2426 ms/frame fast against 0.5270 accurate — **2.17x**, 3.1 % of a 16.7427 ms frame, still
31.8x real time headless. That is inside the band [§5.2](#5-recommendation) predicted and the
captain's stated condition for retiring `renderscan`.

Flipping the default is nonetheless a product decision with two consequences worth stating
plainly, and it has **not** been taken here:

* every user pays 2.17x on the PPU for accuracy only test ROMs can see;
* `testharness/{baseline,mealybug_baseline,mooneye_baseline,mooneye_cgb_baseline}.txt` become
  the accurate engine's numbers and the `*_accurate_baseline.txt` files become the fast one's —
  the two sets swap, in one commit, and `make status` must move with them.

Worth roughly **+5 rows on the GB Emulator Shootout's published 264** if taken: its Mealybug
population is DMG-only (our DMG column goes 1/24 → 4/24) and `hblank_ly_scx_timing-GS` and
`intr_2_mode0_timing` are both inside its 66-row Mooneye population.

> **Taken on 2026-08-20, and both bullets came out differently — which is why they are left
> standing rather than edited.**
>
> * The estimate of **+5** shootout rows was low by a factor of three: it is **+15**, because
>   the DMG Mealybug column went 1/24 → **8/24** rather than 4/24 and *all eight* of the
>   shootout's red Mooneye `acceptance/ppu` rows flipped, not two.
> * **The two baseline sets did not swap and nothing was re-blessed.** Making both `make`
>   targets name their engine (`GB_PPU=standard` / `GB_PPU=accurate` on every line) leaves each
>   file with the engine that produced it and gates both engines as before. Swapping the
>   contents would have left a file called `_accurate_` holding the other machine's numbers.
> * The cost is worth re-quoting from a real cartridge rather than from dmg-acid2, because it
>   is what a player actually pays: Pokémon Blue on one P-core, **0.094 → 0.373 ms/frame**,
>   which is 2.2 % of a 16.743 ms frame. Headless with no pixels wanted it is 0.057 → 0.382,
>   i.e. **6.7x**, and that is the case `standard` now exists for.

### 9.6 What is left, and where

The residual is no longer uniform — [§8.9](#89-what-the-next-pass-should-do-in-order)'s "2-3 pixels at the
head of the line" was the line-0 phase and is gone. What remains, in order of how close it is:

1. **`m3_lcdc_bg_en_change@DMG` — 4 px**, one pixel at `x=0` on lines 19-22, DMG only (both CGB
   columns are exact). A head-of-line interaction with the DMG LCDC stage schedule.
2. **`m3_scx_high_5_bits` — 12 px on all three columns**, seven lines around `x=16..23`. The
   single biggest prize left: closing it is **+3 rows**.
3. **`m3_window_timing@CGB-C` — 138 px** and **`m3_bgp_change@CGB-C` — 864 px**, where the DMG
   and CGB-D columns of both are already exact. A CGB-specific window/palette phase.
4. **`m3_lcdc_obj_size_change_scx@DMG` — 298 px**, newly out of the rank-artefact zone and
   worth re-reading now that its number means something.
**The commit phases are already at their optimum — do not re-sweep them.** Both remaining
near-misses were swept against `mmu::ppu_write_commit` after the fact, and `early: 3` is the
best value for each by a wide margin:

| DMG `SCX` (`$FF43`) `early` | 0 | 1 | 2 | **3** | 4 |
|---|---:|---:|---:|---:|---:|
| `m3_scx_high_5_bits@DMG` | 84 | 77 | 47 | **12** | 53 |

| DMG `LCDC` (`$FF40`) `early` | 1 | 2 | **3** | 4 |
|---|---:|---:|---:|---:|
| `m3_lcdc_bg_en_change@DMG` | 855 | 403 | **4** | 376 |
| suite total | 671,348 | 669,696 | **646,838** | 712,414 |

`SCX` and `SCY` were also split into separate arms to see whether they wanted different
phases: they do not, both sit at 3, and the split was reverted. `m3_scx_high_5_bits` reads 12
on **all three** columns while only the DMG one has an early commit at all, which is the same
conclusion from the other side. **What is left in these two rows is inside the fetcher, not in
when the write becomes visible** — and that is the mutually-constraining per-tile calibration
[§8.9](#89-what-the-next-pass-should-do-in-order) says to budget in weeks.

4. Everything else is either the boot-ROM content rows
   ([§8.6](#86-blocker-c-measured-decided-and-now-a-user-supplied-boot-rom)) or the eight CGB rows whose
   references carry 125 luminances and cannot be compared at all
   ([§8.7](#87-eight-rows-compare-a-greyscale-frame-against-a-colour-photograph)).

**Read any row near 22,500 with care — it is a rank artefact, not a distance.** The metric is
a shade-*rank* compare, so one extra distinct shade in our frame re-ranks the whole picture and
reports ~23,040. `m3_lcdc_obj_size_change_scx@DMG` is the worked example in both directions: it
sat at 22,070 on the parent commit and reads **298** now, because our shade count finally
matches the reference — the picture did not improve by 21,772 pixels' worth, it stopped being
mis-ranked. Its two CGB columns are still in that zone (22,076 → 22,154) and that movement is
meaningless. Overall: **5 rows numerically worse, 41 better, none lost pixel-exactness.**

---

## 10. Reading the leaders: 12/79 → 17/79 (15 Aug 2026)

Measured with `make test-external-accurate` on the ROMs pinned in
[`tools/fetch-test-suites.sh`](../tools/fetch-test-suites.sh), before and after, on the
`identical` picture mode. **Rows 12 → 17; total distance 646,838 → 630,266 pixels; five rows
`FAIL → PASS` and none lost.** Every `standard`-engine baseline is byte-identical, as are
Mooneye (95/103 standard, 97/103 identical), Mooneye CGB `misc` (8/8 both), SameSuite (72/76
both), `cpp` (4/4), the in-repo `testroms` suite (46/47), `audiocapture`, `savestate`,
`battery`, `bootrom`, `chiptune` and the `ppuengines` gate (accurate still 0/23,040 on
dmg-acid2, cost ratio 2.10×).

This pass did not grind the rows from first principles. Two emulators ahead of us on the
[GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/) board are open source, and both
already answer the two questions §9.6 left standing. **Licences checked before anything was
read**, and recorded here so nobody has to check again:

| repo | licence | what was taken |
|---|---|---|
| [Docheinstein/docboy](https://github.com/Docheinstein/docboy) | MIT (`LICENSE`, "Copyright (c) 2025 Docheinstein") | the fetcher's behaviour at an object hit; the CGB `SCY` latch |
| [LIJI32/SameBoy](https://github.com/LIJI32/SameBoy) | Expat, "all files and directories … except for the iOS and HexFiend directories" | the CGB-C/CGB-D PPU boundary |
| [mattcurrie/mealybug-tearoom-tests](https://github.com/mattcurrie/mealybug-tearoom-tests) | MIT (`LICENSE`, "Copyright (c) 2018 Matt Currie") | `src/ppu/*.asm`, to read what each ROM actually measures |

All three are MPL-2.0-compatible, so nothing here was blocked. **No code was copied**: each
behaviour was read as a hardware statement and reimplemented in this codebase's idiom, the same
rule `CONTRIBUTING.md` states and the same one the `ppu_write_commit` model already followed.
Every claim below carries its file and symbol.

### 10.1 The dot an object is detected is still a fetcher dot (`m3_scx_high_5_bits`, +3 rows)

`m3_scx_high_5_bits` sat at **12 pixels on all three columns** — §9.6 called it the biggest
single prize left and read the fact that it is 12 on all three, while only the DMG has an early
register commit, as "what is left is inside the fetcher". That reading was right.

Where the 12 pixels are: lines 128-134 only, all inside the single tile at `x = 16..23`. The
ROM fills the background with `'A'`, `'B'`, `'C'`… one letter per tile column and sets
`SCX = LY` part-way along every line, so which letter a tile holds says which `SCX` its
tile-number fetch used. On those lines we drew **`'S'` (map column 18, the new `SCX >> 3` of
16) where hardware drew `'C'` (column 2, the old `SCX` of 0)** — one tile of the line took the
new value a fetch too early. The seven failing lines are seven rows of one glyph; line 135 is
the eighth and happens to agree.

Lines 128-135 are the ones carrying the ROM's object at `X = 16`. `16 & 7 == 0`, so Pan Docs'
[OBJ penalty](https://gbdev.io/pandocs/Rendering.html) charges its maximum five dots there,
where the object at `X = 15` on lines 120-127 costs nothing — and only the lines with the
five-dot penalty were wrong. So the defect is in what happens to the **background fetcher**
while an object is being served.

docboy answers it structurally. Its object fetch is started from inside the background
fetcher's own tick — `handle_pixel_slice_fetcher_push` in
`src/docboy/docboy/ppu/ppu.cpp` ends with `if (is_obj_ready_to_be_fetched()) { … is_fetching_sprite = true; obj_prefetcher_get_tile_0(); }`,
under the comment *"Note: the first obj prefetcher tick overlaps this tick"* — and the output
stalls earlier than that, at `is_bg_fifo_ready_to_be_popped()`. So between "an object is due"
and "the object fetch owns the fetcher" the background fetcher **keeps running**, finishing the
tile it is in the middle of. Ours froze it on the spot, which put the next tile-number read one
dot late and let the `SCX` write land in front of it.

`Gpu::pipeline_dot` now gives the fetcher that dot. The charged penalty is kept exactly as it
was — it is what makes `acceptance/ppu/intr_2_mode0_timing_sprites` exact and it is unmoved —
so this is one dot of fetcher progress, not a change to mode 3's length.

**The full structural model was tried and is worse here, which is worth writing down.** Letting
the fetcher run for *all* `obj_wait` dots (docboy's actual shape) also wins the three rows but
costs 64,784 pixels elsewhere; a single tick at the trigger costs 151. The reason is that our
`obj_wait` is Pan Docs' *formula*, not the fetcher's real remaining time, so running the
fetcher through it double-counts. Making the wait structural instead is the per-tile
calibration §8.9 budgets in weeks, and [`src/gpu/fifo.rs`](../src/gpu/fifo.rs)'s
`start_obj_fetch` already records why the formula is charged rather than emerging.

Measured, everything else held constant:

| | rows | suite pixels |
|---|---:|---:|
| before | 12 | 646,838 |
| fetcher ticks for the whole penalty | 15 | 711,773 |
| **fetcher ticks on the trigger dot only** | **15** | **646,989** |

Pinned by `gpu::fifo::fetcher_details::the_background_fetcher_still_gets_the_dot_an_object_is_detected`,
teeth proven by removing the tick.

### 10.2 `m3_window_timing@CGB-C` and `m3_bgp_change@CGB-C` were never a fetcher bug

Both were named as close rows: 138 and 864 pixels, **already pixel-exact on DMG and on CGB-D**.
Comparing the suite's own two Color reference sets against each other explains why:

```
m3_window_timing:   CPU CGB C vs CPU CGB D references differ in 138 pixels
m3_bgp_change:      CPU CGB C vs CPU CGB D references differ in 864 pixels
```

The numbers are ours exactly. We matched CGB-D to the pixel, and the C column is the *other
machine*. Seven of the suite's ROMs disagree between the two Color reference sets, by 8,265
pixels in total:

| ROM | C vs D reference |
|---|---:|
| `m3_scy_change` | 6,217 |
| `m3_bgp_change` | 864 |
| `m3_bgp_change_sprites` | 716 |
| `m3_window_timing_wx_0` | 144 |
| `m3_lcdc_obj_en_change_variant` | 144 |
| `m3_window_timing` | 138 |
| `m3_obp0_change` | 42 |

`examples/mealybug` was already running two machines because the DMG and CGB references are
different consoles. It was scoring both Color columns against **one** Color machine, and no
emulator can pass both that way — the same lesson `examples/samesuite.rs::revision_for`
records about SameSuite's mutually-contradictory `-cgb0` and `-cgbB` ROMs.

### 10.3 What actually differs between CPU CGB C and CPU CGB D

Two facts, and between them they account for all seven rows.

**(a) CGB-D and later latch the fetcher's `SCY + LY`.** SameBoy says it outright, and it is the
only CGB stepping difference in its whole PPU — `Core/display.c`, `GB_FETCHER_GET_TILE_T1`:

> `if (gb->model > GB_MODEL_CGB_C) { /* This value is cached on the CGB-D and newer, so it cannot be used to mix tiles together */ gb->fetcher_y = y; }`

with the bitplane fetch reading `gb->model > GB_MODEL_CGB_C ? gb->fetcher_y : fetcher_y(gb)`.
docboy states the same boundary from the other side and models D-and-later only
(`bg_prefetcher_get_tile_0`: *"Differently from DMG, on CGB there's no way to create a bitplane
desync effect by changing SCY between the tile data low/high fetches. That is, the same SCY is
used for both low and high byte of the tile, and it's computed exactly here.
[mealybug/m3_scy_change]"*). Our fetcher re-read `SCY` at every bitplane on both consoles, so
we were a CGB-C for this. Measured on the Color machine, everything else held:

| | `m3_scy_change@CGB-C` | `m3_scy_change@CGB-D` |
|---|---:|---:|
| `SCY` re-read (CGB-C) | **560** | 15,085 |
| `SCY` latched (CGB-D) | 15,129 | **116** |

That is the two references' 6,217-pixel disagreement reproduced from both ends, which is what
says the mechanism is right rather than fitted.

**(b) A palette write is seen one dot later on a CPU CGB C.** This one is *measured here*, not
borrowed — neither reference emulator models it. The C and D reference photographs differ by
exactly six pixels per line on `m3_bgp_change`, at `x = 1, 13, 73, 85, 145, 157`: three
12-pixel bands each shifted by one pixel. Sweeping our Color palette-commit dot reproduces the
swap exactly and in no other configuration:

| `$FF47-$FF49` `early` on colour hardware | 4 | **3** | **2** | 1 |
|---|---:|---:|---:|---:|
| `m3_bgp_change` C / D | 1728 / 864 | 864 / **0** | **0** / 864 | 864 / 1728 |
| `m3_window_timing` C / D | 276 / 138 | 138 / **0** | **0** / 138 | 138 / 276 |
| `m3_bgp_change_sprites` C / D | 2400 / 1684 | 1688 / **972** | **972** / 1688 | 1682 / 2398 |
| `m3_lcdc_obj_en_change_variant` C / D | 350 / 494 | 350 / **206** | **206** / 350 | 350 / 494 |

Three rows land on their C reference at one setting and on their D reference at the setting one
dot later, and nothing else in the suite moves. `m3_window_timing_wx_0` is the one row that
fits neither (263/371 against a 144-pixel reference gap), so it has a second cause.

### 10.4 How it is modelled

[`PpuRevision`](../src/mode.rs) — `CgbC` and `CgbD`, default `CgbD`, in `src/mode.rs` because
`src/sound.rs` is not compiled on `wasm32` and the PPU always is. It is deliberately **not**
`sound::CgbRevision`: that type's boundary is 0/B against C-and-later, and the PPU's is
C-and-earlier against D-and-later, so one enum would have to name four steppings to express two
facts. Both read the same `$GB_CGB_REVISION`, and every tag one accepts the other accepts, so a
user still has one knob.

It is machine **configuration**, not cartridge state, so it is not serialised — the same call
`Sound::cgb_revision` makes, and for the same reason: a save state should restore into whatever
machine is running. (`savestate::FORMAT_VERSION` did move, 17 → 18, but for the fetcher's
latched `SCY + LY`, which *is* live pipeline state.)

The default is `CgbD` because that is the machine this emulator already claims elsewhere: the
APU targets CGB-E (`docs/components/apu.md` §8), SameBoy's default Color console is
`GB_MODEL_CGB_E`, and docboy models D-and-later. Before this we were neither — CGB-D palette
timing with CGB-C `SCY` behaviour, a machine that never shipped.

`examples/mealybug` now runs **three** machines, one per reference set, and `REFS` carries the
stepping beside the console. Only the runner selects a non-default stepping; nothing in the
product does, and no other suite runner or harness looks at it.

### 10.5 Every row that moved, both directions

Five `FAIL → PASS`, none lost:

| row | before | after |
|---|---:|---:|
| `m3_scx_high_5_bits@DMG` | 12 | **PASS** |
| `m3_scx_high_5_bits@CGB-C` | 12 | **PASS** |
| `m3_scx_high_5_bits@CGB-D` | 12 | **PASS** |
| `m3_bgp_change@CGB-C` | 864 | **PASS** |
| `m3_window_timing@CGB-C` | 138 | **PASS** |

Better but still failing:

| row | before | after |
|---|---:|---:|
| `m3_scy_change@CGB-D` | 15,101 | 116 |
| `m3_bgp_change_sprites@CGB-C` | 1,688 | 972 |
| `m3_lcdc_tile_sel_change@CGB-C` / `@CGB-D` | 1,094 | 802 |
| `m3_lcdc_obj_en_change_variant@CGB-C` | 350 | 206 |
| `m3_lcdc_win_map_change@CGB-C` / `@CGB-D` | 710 | 590 |

Worse, and all of it is the object-trigger dot in §10.1 — these rows all place objects, so the
fetcher genuinely moves one dot at each of them and the mid-line write they photograph lands
somewhere else:

| row | before | after |
|---|---:|---:|
| `m3_lcdc_win_map_change@DMG` | 644 | 1,028 |
| `m3_lcdc_tile_sel_win_change@CGB-C` / `@CGB-D` | 1,178 | 1,366 |
| `m3_lcdc_bg_map_change@DMG` | 466 | 594 |
| `m3_window_timing_wx_0@CGB-C` | 263 | 371 |
| `m3_lcdc_tile_sel_win_change@DMG` | 1,222 | 1,300 |
| `m3_lcdc_bg_map_change@CGB-C` / `@CGB-D` | 522 | 530 |
| `m3_lcdc_tile_sel_change@DMG` | 802 | 822 |
| `m3_scy_change@CGB-C` | 546 | 560 |
| `m3_scy_change@DMG` | 246 | 257 |

Net: **−16,572 pixels, +5 rows, 0 rows lost.** Trading ~800 pixels spread over eight
mid-line-write rows for three exact rows and a 15,000-pixel correction is the right trade, and
the eight are all in the same place — *when the fetcher resumes after an object*, which is the
per-tile calibration §8.9 already scopes.

### 10.6 The rows named in the brief that this pass did **not** close

* **`m3_lcdc_bg_en_change@DMG`, 4 pixels — open, and not for want of trying.** The four are one
  pixel each at `x = 0` on LY 19-22: we blank the background where hardware still draws it, on
  the very first visible pixel of four lines. docboy has a rule that looks made for it
  (`pixel_transfer_lx8`: *"On DMG, it seems that there is a 1 T-cycle delay between the real
  LCDC and the LCDC the PPU sees, both for LCDC.OBJ_ENABLE and LCDC.BG_WIN_ENABLE. **LX == 8
  seems to be an exception to this rule.**"* — their `LX == 8` is our `lx == 0`). We already
  model that one-dot delay, on the *write* side, as `LCDC_DMG_STAGE_BITS`. Moving it to the read
  side with the exception was implemented and measured: **the suite total is identical to the
  digit, 646,989 either way**, and these four pixels do not move. So the exception is
  unobservable in this engine and the write-side model is equivalent; the four pixels are a
  different cause and the change was reverted rather than carried for nothing.

  > **Re-measured 2026-08-20, three more ways, all negative — do not spend a fourth hour on
  > this row.** The write-side stage was swept rather than reasoned about:
  >
  > | `LCDC.0` settles… | `m3_lcdc_bg_en_change@DMG` | other rows |
  > |---|---:|---|
  > | one dot after commit (**shipped**) | **4** | — |
  > | two dots after commit (a second stage value) | 403 | `m3_lcdc_win_en_change_multiple` 0 → 522 |
  > | at the cycle's close, on the window enable's schedule | 855 | none |
  > | one dot after commit **except at `lx == 0`** (docboy's exception, write-side) | **6** | none |
  >
  > The direction is now settled too, and it is the opposite of what the pixel says at first
  > glance. The ROM starts with the background **off** and the mid-line write turns it **on**;
  > the left column is a staircase that grows one pixel per band as the write lands later. Our
  > extra pixel means we turn the background on *one dot late* in the third band and exactly
  > right in every band after it — so any uniform shift is wrong by construction, which is what
  > the first three rows of that table say. It is a conditional rule and the obvious condition
  > (the head of the line) is the fourth row, which makes it worse. Come back with a model.
* **`m3_lcdc_obj_size_change_scx@DMG`, 298 pixels — not attempted.** Its two CGB columns sit at
  22,154, which is the shade-rank artefact zone, so the row cannot go green without the DMG
  column and both CGB columns moving together.

  > **Update, 2026-08-20: the CGB columns came out of the artefact zone and the row is now the
  > second-cheapest thing in the suite.** It reads **30 / 0 / 0** — both Color columns are
  > pixel-exact and only the DMG one is left, thirty pixels in columns 27-31 of lines 2-7 and
  > 130-135. Those columns are the **second** sprite block, `X = $20` and attribute `$40`
  > (Y-flipped), from the ROM's own `sprite_data`; the handler writes `LCDC` four times in
  > eight cycles, alternating `$83` (8x8) and `$87` (8x16), so what is being measured is which
  > dot of an object fetch reads `LCDC.2`. Its sibling `m3_lcdc_obj_size_change` is **51 / 10 /
  > 10** — the same question on all three machines. Closing them is **two shootout rows** and
  > they are group F of §11.4.


## 11. The denominator: 45 of the failing rows cannot be won by PPU work (15 Aug 2026)

Everything before this section counts progress against **79**, and every recommendation in it
is scoped as though the whole 79 were reachable. It is not, and this section is the correction.
A characterisation of all 62 failures at 17/79 — the *shape* of each difference, not its pixel
count — sorted them into four blocks, three of which no amount of mid-scanline accuracy can
move.

Reproduce the classification with the two instruments this pass added:

```sh
python3 tools/mealybug/characterise.py                       # every failure's signature
python3 tools/mealybug/rowview.py <rom> [DMG|CGB-C|CGB-D] --cols A B --lines A B
```

`characterise.py` re-renders before it reads, deliberately: `target/mealybug/*.png` holds
whichever engine ran last, and scoring `identical`'s frames against `standard`'s numbers invents
both regressions and gains.

### 11.1 The four blocks

| block | rows | winnable? |
|---|---:|---|
| **A** — the boot ROM's `®` glyph | 27 | **no**, and not ours to decide — see §8.6 |
| **B** — CGB references that are colour photographs | 8 | **no** — see §8.7 |
| **C** — the `*2` ROMs: a DMG build against a CGB-mode reference | 7 | **no** — new, §11.3 |
| **D/E/F/G** — genuine mid-scanline error | **11** | yes — §11.4 |

So with the suite pinned as it is, and with the licensing decision of §8.6 standing, the
reachable ceiling is **79 − 42 = 37 rows.** At <!--score:mealybug_accurate:pass-->26<!--/score-->
that leaves **eleven rows in play**, not fifty-three. Anyone planning work against this suite
should plan against eleven.

**`grep -l '\$19'` finds ten ROMs and only nine of them are blocked.**
`m3_wx_4_change_sprites` places its objects with tile `$19` *and then fills all tile character
data with `$FF`* before it starts, so the glyph never reaches the screen; its three rows are
pixel-exact now. Referencing the tile is a first pass, not a verdict — check whether the ROM
overwrites it. The nine that genuinely depend on it write nothing at `$8190`:
`m3_bgp_change_sprites`, `m3_lcdc_bg_map_change`, `m3_lcdc_obj_en_change`,
`m3_lcdc_obj_en_change_variant`, `m3_lcdc_tile_sel_change`, `m3_lcdc_tile_sel_win_change`,
`m3_lcdc_win_map_change`, `m3_obp0_change`, `m3_scx_low_3_bits`.

**Block A still repays effort even though it cannot reach zero.** What is left in those ten
ROMs after the missing glyph is ordinary mid-scanline error of exactly the kinds D/E/F name, so
a fix in those groups improves them; they simply stop short. A crew with its own boot-ROM dump
can measure the true residual through the shipped `--boot-rom` path (§8.6); the gated score must
never use one.

### 11.2 A correction: the references are the author's *emulator*, not photographs

This page, the brief that commissioned this work and the headers of both Mealybug baselines all
described `expected/` as hardware photographs. The suite's own README says otherwise:

> You can check in the `expected` directory for screenshots from **my Game Boy emulator** (which
> I believe to be correct), and the `photos` directory contains blurry photos of the ROMs
> running on real devices.

They are [Beaten Dying Moon](https://mattcurrie.com/bdm-demo/)'s output — the test author's own
emulator, cross-checked by him against the hardware photographs he ships in `photos/`. That is
still an excellent oracle and nothing about the campaign changes because of it. But it is a
different claim from "this is what the silicon did", it is the claim the baselines should carry,
and §11.3 only makes sense once you know it.

### 11.3 Block C — the seven `*2` rows are a ROM/reference mismatch in the suite

`m3_lcdc_bg_en_change2`, `m3_lcdc_bg_map_change2`, `m3_lcdc_tile_sel_change2`,
`m3_lcdc_tile_sel_win_change2`, `m3_lcdc_win_map_change2`, `m3_scx_high_5_bits_change2` and
`m3_scy_change2` exist only in the `CPU CGB C` column. Their sources program **CGB attributes
and CGB palettes** — `rVBK = 1`, then a green/blue/red palette 1 through `BCPS`/`BCPD`. The
prebuilt ROMs in `mealybug-tearoom-tests.zip` carry **`$0143 = $00`**, a DMG cartridge, so on a
Color console they run in compatibility mode where `VBK` is locked and the palette data ports
are closed and none of that has any effect.

**But their references are in colour.** `m3_lcdc_bg_map_change2`'s CGB-C reference contains
`(123, 255, 49)` and `(255, 132, 132)`. Our frame is *structurally identical* to it and differs
only in colour:

```
ours  (0,0,0)×4792   (192,192,192)×2869                    (255,255,255)×15379
hw    (0,0,0)×4792   (123,255,49)×2729  (255,132,132)×140  (255,255,255)×15379
```

`2729 + 140 = 2869`, and the black and white counts agree to the pixel. Patch `$0143` to `$80`
in a scratch copy of the ROM and the row falls **18,108 → 2,869**, the remainder being nothing
but our CGB colour conversion (we apply Gambatte's correction matrix; the suite's README asks
for `(r << 3) | (r >> 2)` — a display choice the shade-rank compare is designed to be immune to,
and not a defect).

So the author scored these against a CGB-mode build and shipped a DMG build in the zip.

**Do not close them by forcing the CGB flag in the runner.** That is the runner measuring a
machine the cartridge does not ask for — the mistake `examples/mealybug`'s own header warns
against, and the same one that made the DMG/CGB columns worth separating in the first place. The
legitimate route is to build the suite from source with its CGB define and pin *that* in
`tools/fetch-test-suites.sh`, which is a suite-plumbing change and a whole-baseline re-bless.

### 11.4 The twelve winnable rows, grouped by the code they live in

Measured on the `identical` engine at the head of this campaign.

| group | rows | code | signature |
|---|---|---|---|
| **D** window / `WX` | `m3_wx_5_change@DMG` 13,660 · `m3_wx_6_change@DMG` 4,792 · `m3_lcdc_win_en_change_multiple_wx@DMG` 1,273 | `check_window_start`, `check_window_stop`, `prologue_pos` | `LCDC.5` toggled and `WX` moved *after* the window has started |
| **E** `SCY` / bitplane mix | `m3_scy_change` 257 / 560 / 116 | **not `SCY` at all — see §14**, it is `pipeline_dot`'s object-overlap dot | **tile 1 only**, and it renders the previous scanline's `SCY` |
| **F** the object fetch | `m3_lcdc_obj_size_change` 81 / 20 / 20 (`_scx` closed 21 Aug, §12) | `due_sprite`, `start_obj_fetch`, `obj_fetch_dot`, `obj_plane`, `merge_sprite` | the tall extension, and objects at `X < 8` |
| **G** | `m3_lcdc_bg_en_change@DMG` 4 | `paint`, `LCDC_DMG_STAGE_BITS` | one pixel at `x = 0`; §10.6 records **four** measured dead ends |

> **`acid/cgb-acid-hell` was the cheapest measurement of the `tile_sel` question
> there is, and it is CLOSED — §13.** The note below is kept for its negative
> result, which is still true and still worth not repeating.
>
> **And `acid/cgb-acid-hell` is now the cheapest measurement of the `tile_sel`
> question there is.** `m3_lcdc_tile_sel_change` sits at 802/802/822 px and is
> in block A, so its number says nothing about the behaviour — it is dominated
> by the missing boot-ROM glyph. `cgb-acid-hell` measures the same thing,
> `LCDC.4` flipping between a tile's two bitplane fetches, in **two pixels** and
> with no glyph anywhere near it: [`shootout.md` §1c](shootout.md#1c-the-remaining-25-measured-rather-than-assumed-21-aug-2026)
> has the byte-level derivation — **and the negative result that goes with it**,
> which is that sweeping the Color `LCDC` arm of `ppu_write_commit` over all
> eight of its reachable configurations leaves that row at 2 px. Whatever it is,
> it is not the commit schedule, so do not spend a fifth sweep there.

> **Group F, the trace and its two traps (21 Aug 2026).** The instrument is one
> `eprintln!` in `obj_plane` printing `(line, modeclock, oam, x, y, plane,
> sprite_size)` and one in `Gpu::wb`'s `$FF40` arm printing the dot the write
> lands on, both gated on an env var and on `line` — that is enough to read the
> race straight off. Measured on the final frame of `m3_lcdc_obj_size_change`,
> line 8: the four `LCDC` writes land at dots ~97 / ~121 / ~133 / ~145, and the
> three objects that line finds are fetched at 99+101 (`X = 0`), 118+120
> (`X = $10`) and 145+147 (`X = $20`). The middle object is the one that must
> mix its bitplanes on hardware and does not here, so the question is a couple
> of dots between the `$FF40` commit and the plane reads — not the plane
> spacing, which already matches SameBoy at +3 and +5.
>
> **Trap one: `examples/mealybug` runs every ROM on three machines**, so a trace
> keyed only on the line prints three interleaved runs and the dots differ by a
> dot or two between them. Stage the one ROM into its own directory
> (`build/<rom>.gb` plus a copy of `expected/`) and point `--dir` at that.
> **Trap two: the screenshot is frame 10**, taken at the ROM's own `ld b,b`
> breakpoint, and the handler's return through the `nop_slide` drifts the write
> dots between frames — read the *last* group in the trace, not the first.
>
> Only sprites whose `Y` matches the line are fetched, and the ROM's 27 objects
> walk `Y` from `$10` to `$90` in each of its three groups, so a given line has
> exactly **three** objects: one per `X` group. Lines 8-13 are the tall
> extension of the `Y = $10` row and are where the mix belongs; columns 0-8 are
> the second half of group F's signature, the objects at `X < 8`.

**Start with group F — and note that group E is now part of it** (§14: `m3_scy_change`'s DMG
error is an object stall, not a `SCY` one, so the two rows share a cause and are **two shootout
rows** between them). Group G is four pixels and has now refused four different models; leave it
until something else in the fetcher moves.

These are four disjoint regions of `src/gpu/fifo.rs`, so they can be worked in parallel. **The
coupling is not the code, it is the baseline**: `testharness/mealybug_accurate_baseline.txt`
carries a pixel count for all 79 rows, so any PPU change rewrites lines belonging to rows nobody
is working on, and `docs/status*` is regenerated from it. The rule that makes that painless is
**rebase first, re-measure second, re-bless third** — never carry a re-blessed baseline through
a rebase.

### 11.5 What group D still needs, with the arithmetic already done

The window's tile grid is anchored at `WX - 7` even when that is left of the screen. Ground
truth, from `m3_lcdc_win_en_change_multiple_wx@DMG` where `WX = LY`: with `LCDC.5` taken low
around pixel 3, the background resumes at **9, 10, 3, 4, 5, 6, 7, 8** for `WX = 0..7` — exactly
the first `WX - 7 + 8n` at or after pixel 3. Our grid is anchored at `max(0, WX - 7)`, so we
resume at 16 where hardware resumes at 9.

Implementing it by dropping the first tile's off-screen pixels out of the FIFO **does not work**
and was measured: it brings the next push forward and shortens the line, costing
`m3_window_timing` (0 → 3/6/3) and `m3_window_timing_wx_0` (0 → 666/702/666).

The model that fits every measurement is that for `WX < 7` **the window takes over `7 - WX` dots
earlier than it does now** and the leading pixels are shifted out and dropped during dots that
are already being spent. Working backwards from the two rows that pass, the takeover belongs at
mode-3 dot `5 + WX`:

| `WX` | takeover | window fetch | push | off-screen shifts | pixel 0 |
|---:|---:|---|---:|---|---:|
| 0 | dot 5 | 5-10 | 10 | 11-17 (seven) | **18** |
| 6 | dot 11 | 11-16 | 16 | 17 (one) | **18** |
| 7 | dot 12 | 12-17 | 17 | none | **18** |

Pixel 0 lands on dot 18 in every case, which is what `m3_window_timing_wx_0` measures and what
we already get. What blocks it is that our pipeline has no output position left of pixel 0: the
takeover is gated on the FIFO being non-empty, which cannot be true before dot 12. Both
reference emulators carry a signed one (SameBoy's `position_in_line` runs from −16, docboy's
`LX` from 0 with the screen at 8), and adopting one would also let the `SCX & 7` discard, the
`WX = 0` extra dot and this all fall out of one rule instead of three. That is the refactor this
group wants; it is not a special case away.

> **Confirmed independently, 21 Aug 2026: `check_window_stop`'s resume is the same bug from the
> stop side, and it is NOT a one-line fix either.** `m3_lcdc_win_en_change_multiple_wx@DMG` sets
> `WX = LY` and toggles `LCDC.5` off around pixel 3; hardware's background resumes at 9, 10, 3,
> 4, 5, 6, 7, 8 for `WX = 0..7` — exactly the smallest `(WX - 7) + 8n` at or after pixel 3. Ours
> (`resume = lx / 8 + 1`) resumes on the **screen's** tile grid, not the **window's** — it throws
> away the phase the window's own `fetch_x` was counting in. Patching the formula alone does not
> close it: `(WX - 7)` is not generally a multiple of 8, so converting a window-grid boundary
> back into a background `fetch_x` needs a fractional carry-over — exactly the discard mechanism
> `win_skip` already gives the *start* side, not yet given to the *stop* side. That is the same
> unification the signed-position note above is asking for, from the opposite end of the line;
> do not attempt this half separately from that refactor.


## 12. The object's high bitplane reads `LCDC.2` a dot later (21 Aug 2026)

`m3_lcdc_obj_size_change_scx` is **pixel-exact on all three machines** — 30 / 0 / 0 → **0 / 0 /
0** — which is the DMG column, one Mealybug row and **one shootout row** (239 → 240 of 264).
Group F of [§11.4](#114-the-twelve-winnable-rows-grouped-by-the-code-they-live-in) is half
closed; its sibling `m3_lcdc_obj_size_change` is still red and §12.3 says why, with the
arithmetic, so nobody repeats the search.

The whole change is one line of `Gpu::obj_fetch_dot`: the high bitplane is read on the object
fetch's **sixth** dot, not its fifth.

### 12.1 What the two ROMs measure, read off the reference photographs

Both toggle `LCDC.2` (object size) four times inside mode 3, from a `nop` slide entered on the
mode-2 STAT interrupt, over 8×16 objects whose two halves hold different tile rows. So a plane
read landing either side of a write picks a **different tile row**, and the picture says which.

The reference frames were solved back to the bytes they were drawn from. Every object is tile
`$4C`, and with `OBP0 = $E4` and `BGP = $00` each pixel's colour gives one bit of each plane, so
eight pixels give both bytes exactly. Against the tile in VRAM:

| ROM | band | our low | our high | **hardware** low | **hardware** high |
|---|---|---|---|---|---|
| `m3_lcdc_obj_size_change` | LY 8-13, object at screen `x = 8` | 8×8 | 8×8 | 8×8 | **8×16** |
| `m3_lcdc_obj_size_change_scx` | LY 2-7 and 130-135, object at `x = 24` | 8×16 | 8×16 | 8×16 | **8×8** |

The two disagree in opposite directions and say the same thing: **hardware's high-plane read
sits later on the `LCDC` timeline than ours.** One crosses a `$83 → $87` write, the other a
`$87 → $83` one, and in both cases hardware has crossed it and we have not.

Worked example, `m3_lcdc_obj_size_change@DMG` line 9. Our fetch log reads the low plane at dot
118 and the high at dot 120; the `$87` write commits at dot 121. We produce `lo = $F0`
(`$84C2`, the 8×8 row) and `hi = $90` (`$84C3`). The photograph's eight pixels decode to
`lo = $F0` — the same byte — and `hi = $9D`, which is **`$84D3`, the 8×16 row's high byte**. The
low plane agrees to the bit and only the high plane moved, so the fetch is right about
everything except *when* it looked at `LCDC.2`.

### 12.2 Six is docboy's own phase, and the low read is left alone on measurement

docboy runs the object fetch as six phases and computes the address **"just before access it
from VRAM"** in `obj_pixel_slice_fetcher_get_tile_data_low_1` (its fourth) and
`obj_pixel_slice_fetcher_get_tile_data_high_1_and_merge_with_obj_fifo` (its **sixth**) — and its
comment there names both of these ROMs and calls the effect *"bitplane desync"*
([Docheinstein/docboy](https://github.com/Docheinstein/docboy), `src/docboy/docboy/ppu/ppu.cpp`;
MIT, read and reimplemented per `CONTRIBUTING.md`). SameBoy has the same two-address structure
and marks the timing *"Todo: Which T-cycle actually reads this?"* / *"TODO: timing not
verified"* (`Core/display.c`), so it is not an oracle for this row.

So our high read moves to six and matches docboy. **Our low read stays at three, and that is a
measurement rather than an omission** — docboy's low read is its fourth, and taking it costs
more than it buys. The whole 3 × 4 sweep, every cell the full 79-row suite:

| low / high dot | `obj_size_change` DMG/C/D | `obj_size_change_scx` DMG/C/D | rows DMG/C/D |
|---|---|---|---|
| 3 / 5 (was) | 51 / 10 / 10 | 30 / 0 / 0 | 8 / 9 / 9 |
| **3 / 6 (shipped)** | 81 / 20 / 20 | **0 / 0 / 0** | **9 / 9 / 9** |
| 4 / 6 (docboy) | 85 / 20 / 20 | 0 / **15 / 15** | 9 / 8 / 8 |
| 3 / 4 | 21,844 / 21,839 / 21,839 | 30 / 45 / 45 | 8 / 8 / 8 |
| 2 / anything | ≥ 838 everywhere | ≥ 21,898 everywhere | 7-8 / 7-8 / 7-8 |

`4 / 6` loses the row's own two Color columns. It was also swept against the Color `LCDC`
commit (`ppu_write_commit`'s `early`, 0-3) in case the two were compensating: at `early = 0` the
scx row is green on all three **and `m3_lcdc_bg_en_change`'s Color columns go 0 → 452**, which
is two rows for one. The DMG `LCDC` commit was swept the same way with the shipped 3 / 6 and its
present value of 3 is the best of the four (`scx` DMG 150 / 90 / 75 / **0** for `early`
0 / 1 / 2 / 3). **Do not re-run either sweep.**

`OBJ_FETCH_DOTS` is unchanged at six, so mode 3's length does not move — which is why nothing
outside these two rows changed. Verified: `make test-external` green on all 6,222 `standard`
rows (Mealybug's 79 among them, byte-identical), and on `identical` Mooneye 103/103 + 8/8,
SameSuite 76, `cpp` 4/4 and **all thirteen c-sp suites including Gambatte's 5,320 rows and
`strikethrough`** — the last of which measures object-fetch dots against a running OAM DMA and
so is the row most likely to have noticed. In-repo `testroms` 46/47 with the same verdict text,
dmg-acid2 0/23,040, 892 library tests.

### 12.3 Why `m3_lcdc_obj_size_change` cannot follow, with the arithmetic

Its remaining 81 pixels are **not** the same question, and the constraint it imposes is
contradictory under Pan Docs' OBJ penalty — so no commit schedule and no plane-dot pair can
close it. That is worth writing down, because both are the obvious next thing to try.

Its first sprite block sits at `X = 1 … 8`, one object per 16-line band, all due at `lx = 0`.
Pan Docs' `max(0, 5 - ((X + SCX) mod 8))` therefore fetches them at dots `94 + (5 - X)`: 98, 97,
96, 95, 94, 94, 94 for `X = 1 … 7`, and **99** for `X = 8`, which lands on the next background
tile and pays the full five. Solved out of the photographs, hardware wants:

* `X = 1 … 7` — **both** planes 8×16, so both reads are before the `$83` write becomes visible;
* `X = 8` — **both** planes 8×8, so both reads are after it.

Let `A` be the dot from which a read sees `$83`. The first line needs `A > 100` (`X = 1`'s high
read) and the second needs `A ≤ 99` (`X = 8`'s low read). **There is no such `A`**, at any plane
spacing, because `X = 8` is fetched *earlier* than `X = 1`'s high read and must nevertheless see
the later value. So what is wrong is the head-of-line fetch order itself, not the register.

The obvious repair — charge no tile penalty to an object that is off the left edge, so all of
`X ≤ 7` fetch together and `X = 8` five dots later — was implemented and measured: it changes
mode 3's length, moves every write against every fetch, and takes the row to **21,836** pixels
and the suite to 7 / 7 / 7. Come back with a model of the head-of-line object fetch, not a
constant.


## 13. `TILE_SEL` reset on a bitplane read: the tile index is the data (21 Aug 2026)

**`acid/cgb-acid-hell.gbc` is pixel-exact** — the two-pixel row [`docs/shootout.md`
§1c](shootout.md#1c-the-remaining-25-measured-rather-than-assumed-21-aug-2026) called the
cheapest on the board, open since 2026-08-18 and measured down to the byte by the crew before
this one. Shootout **240 → 241 of 264**, and the `acid` group is 5/5. AGE's `m3-bg-lcdc` and
`m3-bg-lcdc-nocgb` go green with it and `m3-bg-lcdc-ds` improves 3,072 → 2,304 px; Mealybug's
two CPU CGB C `tile_sel` rows improve 820 → 782 and 809 → 771. Nothing anywhere got worse.

### 13.1 The rule, and it is the Mealybug author's own

> *On all CGB revisions, excluding CPU CGB D, resetting `TILE_SEL` on the same T-cycle as a
> bitplane data read will cause the **tile index** to be instead used as the data for that
> bitplane.*
>
> — Matt Currie, *The Comprehensive Game Boy PPU Documentation*, `LCDC` § `TILE_SEL` (bit 4).
> It ships **inside the Mealybug repository this project already fetches**
> (`the-comprehensive-game-boy-ppu-documentation.md`), so the same person wrote the rule and
> photographed the references we score against. SameBoy models the same split and cites the
> same page (`Core/display.c`, `data_for_tile_sel_glitch`).

That is `Gpu::tile_sel_glitch`: a `$FF40` write taking `LCDC.4` from 1 to 0 on Color silicon
arms one dot, and a bitplane fetch on that dot takes `Pipeline::tile_num` as its byte instead
of reading VRAM. SameBoy's extra clause is kept — an index with **bit 7 set** falls through to
the ordinary read.

### 13.2 The two pixels, and why they were unreachable from the fetcher

The previous pass proved the row was *not* a scheduling question: it swept the Color `LCDC`
commit over all eight of its configurations and the row stayed at two pixels in every one, and
concluded — correctly — that the fetch resolves to the same address whatever the schedule and
that the byte at that address is not the byte the reference device had. **This pass added the
other half of that sweep and it says the same thing**: the Color fetcher's three read dots
(tile number, low, high) were swept over all eight combinations of 1/2 × 3/4 × 5/6 and the row
is **two pixels in every one**. Neither *when the write lands* nor *when the read happens* can
reach it, because the byte hardware used is not in the tile at all.

What it is instead is written in the earlier note without being recognised: hardware's high
bitplane is `$55` on line 68 and `$49` on line 69, and the same paragraph records the tilemap
holding **`$98A0 = $55`** and **`$98C0 = $49`** on those two lines. The bytes were the tile
numbers all along.

### 13.3 `PpuRevision` gained `CgbE`, and this is the only thing that splits it from `CgbD`

The boundary here is **D against everything else** — 0, B, C *and E* substitute the index; D
alone does something different. That is a third boundary, so `CgbD` could no longer mean "D and
later":

| predicate | `CgbC` | `CgbD` | `CgbE` (default) |
|---|:-:|:-:|:-:|
| `caches_fetcher_y` — `SCY + LY` latched at the tile-number fetch | ✗ | ✓ | ✓ |
| `late_palette_commit` — a palette write seen a dot later | ✗ | ✓ | ✓ |
| `tile_sel_glitch_reads_tile_index` | ✓ | **✗** | ✓ |

`CgbE` is the default because CGB-E is the stepping this emulator targets everywhere else, and
because it behaves as `CgbD` did on the first two rows **nothing that existed before this change
moved**. `examples/mealybug` still runs its two Color columns as `CgbC` and `CgbD` explicitly,
so the CPU CGB D photographs are still scored on a CPU CGB D. `cgbDE` resolves to `CgbE`.

The gate on the split is a second suite that names the revisions in its own file names:
**AGE ships `m3-bg-lcdc-cgbBCE.png`** — B, C and E, and by omission not D — and we pass it now.
Two independent references, one from each author, agreeing on where the boundary falls.

### 13.4 CPU CGB D's own rule is NOT modelled, and both readings were measured

> *On CPU CGB D, resetting `TILE_SEL` on the same T-cycle as the bitplane 1 data read will cause
> the PPU to instead read the bitplane data from the address for bitplane 0.*

"The address for bitplane 0" has two readings — the same row in the `$8000` block
(`addr & !0x1000`) or the low plane's own address (SameBoy's `(addr & !0x1000) - 1`). **Both
were implemented and both give the identical, worse answer** against the suite's own CPU CGB D
photographs: `m3_lcdc_tile_sel_change@CGB-D` 802 → 1,186 px and
`m3_lcdc_tile_sel_win_change@CGB-D` 1,366 → 1,424. So `CgbD` falls through to the ordinary read
and the rule is left open. Both those rows are block A — they need the boot ROM's `®` glyph
before their absolute distance means anything — so this is a direction, not a verdict, and a
third reading is what it wants.

Also unmodelled, deliberately: the **set** direction (`TILE_SEL` 0 → 1 on a read dot), whose
documented answer is "bitplane 1 data from the most recently drawn sprite, or from the most
recently drawn tile, or from the read in progress at pixel 159/160 of the previous row" — three
alternatives with a revision-dependent choice between them, and no row here measures it; and
SameBoy's separate **double-speed** variant, which glitches on *either* direction over two
cycles and is what AGE's `m3-bg-lcdc-ds` still measures at 2,304 px.

### 13.5 Cost and blast radius

`Gpu::tile_sel_glitch` is one `bool`, set by a `$FF40` write and cleared by the dot that
consumes it. **Not serialised** — published and consumed inside one M-cycle, the same argument
`idu_share` and the OAM DMA's bus fields already make — so `savestate::FORMAT_VERSION` did not
move. `lo_hi` gains one test on a flag that is false on every dot but the handful a Color game
resets `LCDC.4` on, and a DMG never sets it at all.


## 14. `m3_scy_change`'s DMG error is an OBJECT stall, not a `SCY` one (21 Aug 2026)

Traced but **not fixed** — recorded because it moves the row out of group E and into group F,
and because the obvious repair was measured and costs more than it buys.

§11.4 files `m3_scy_change@DMG` (257 px) under "`SCY` / bitplane mix" and points at
`fetch_tile_number`, `lo_hi` and the `$FF42`/`$FF43` arms of `ppu_write_commit`. **None of those
is the fault.** The DMG `SCY` commit was swept over all four values it can take and the shipped
`early: 3` is the best of them, monotonically: 6,313 / 5,128 / 965 / **257** px for `early`
0 / 1 / 2 / 3.

What the fetch trace says instead, on line 5 of the final frame:

```
BGN dot= 92 fx=1 scy=1 map=9801 tile=42     <- our tile-number read
SCY dot= 93 v=2
SCY dot=101 v=3
BG  dot=104 t=3 fx=1 scy=3 row=0 addr=8420  <- our bitplanes, ten dots later
BG  dot=106 t=5 fx=1 scy=3 row=0 addr=8421
```

Hardware's eight pixels at `x = 8` decode to `$7E`/`$7E`, which is **tile `$43` row 0**
(`$8430`), where we draw tile `$42` (`$8420` = `$FE`/`$FE`). The ROM fills each tilemap row with
`ABC…` from a different letter, so `$43` is `$9821` — **map row 1** — and `$42` is `$9801`, map
row 0. Map row is `((SCY + LY) >> 3) & 31`, so hardware read the tile number with `SCY >= 3`,
i.e. **at dot 101 or later**. We read it at dot 92.

The ten dots between them are **an object fetch**. `m3_scy_change` is not an object test but it
places eighteen of them (`sprite_data`, `X = 0 … 17`), and the one at `X = 0` is due at the head
of every line it covers; Pan Docs' penalty charges it `5 - (0 & 7) = 5`, so its fetch spans
eleven dots. `pipeline_dot` starts it on the dot *after* the previous tile's push and hands the
background fetcher that same dot — which is the dot the tile number is read on. Hardware's tile
number lands *after* the stall, so the stall belongs **before** the tile-number read, not
between it and the bitplanes.

**The narrow repair was measured and rejected.** Withholding the overlap dot from the background
fetcher when it is at `fetch_t == 0` (about to start a tile) does move this row the right way —
`m3_scy_change` 257 / 560 / 116 → **215 / 510 / 66** and the DMG suite total 93,169 → 92,435 px
— and it **costs `m3_scx_high_5_bits`, which is pixel-exact on all three machines today and goes
to 12 / 12 / 12**. Net one row lost. That is the same trade `bg_fetch_dot`'s own comment already
records from the other side, and it says the overlap dot is real; what is wrong is *where the
object fetch begins* relative to the push, which is the per-tile calibration §8.9 scopes. Come
back with a model of that, not with a condition on `fetch_t`.

## 15. The signed-position refactor: `check_window_start`/`check_window_stop` share one grid (21 Aug 2026)

`m3_lcdc_win_en_change_multiple_wx@DMG` **1,273 → 437** px; its CGB-C/CGB-D columns (colour
photographs, dominated by the boot-logo blocker in §11.3) move 22,813 → 22,812, noise. Zero rows
lost anywhere: full record of the verification is in `AGENTS.md` under "The window fetcher's
addressing and its visible flag are two different questions" — Mooneye acceptance/emulator-only/
misc, SameSuite, `cpp`, all thirteen c-sp suites (Gambatte's 5,320 rows included), the in-repo
`testroms`, dmg-acid2, `savestate`/`battery`/`bootrom`/`ppuengines`/`gbselftest` and every library
test match their checked-in baselines exactly under both picture modes, and the three moved rows
are the only diff `UPDATE_EXTERNAL_BASELINE=1` produced.

**`m3_wx_5_change`/`m3_wx_6_change` are UNCHANGED (13,660 / 4,792, identical to the digit).**
§11.4's own table already separated them from this row: they need the window to *re-trigger*
while already active on the same line, which `check_window_start`'s guard still refuses
unconditionally. That is a second mechanism, not exercised by the fix here, and is not attempted
in this pass — do not read their unmoved counts as this row's fix falling short.

### 15.1 The fix, in one sentence

`check_window_start` still checks `pipe.window || !win_on || !wy_trigger`, unchanged, and still
sets `wy_pos`/`win_skip`/`win_fresh` exactly as before. What changed is *when the fetcher itself
is allowed to re-address the background mid-tile* versus *when the externally-visible "window is
still on screen" flag comes down* — the two are no longer the same read of the same field, which
was the bug §11.5 named ("Ours resumes on the **screen's** tile grid, not the **window's**").

### 15.2 Three fields where there was one

`Pipeline::window` used to serve three purposes at once: the fetcher's own addressing branch
(`fetch_tile_number`, `lo_hi`), the guard against re-triggering, and the externally-observed "is
window content still showing" flag `the_window_tile_grid_is_anchored_at_wx_minus_7` reads
directly. Splitting it is what closes the row:

- **`window`** keeps its name and its *deferred* role: true from activation, false only once the
  FIFO reaches a natural tile boundary with nothing window-addressed still queued — i.e. only
  reachable past the `bg_len > 0` early return in `bg_fetch_dot`, exactly where the deleted
  `check_window_stop` used to run. This is what `check_window_start`'s guard and the test both
  read, and it is why the background always resumes on a whole tile with no arithmetic: the FIFO
  is never interrupted mid-tile, only redirected for the *next* one.
- **`win_active`** is the *immediate* fetcher-addressing flag docboy's per-stage checks model
  (`src/docboy/docboy/ppu/ppu.cpp`, cited already in `bg_fetch_dot`'s doc comment): it flips the
  instant `LCDC.5` reads low, mid-fetch or not, and `fetch_tile_number` addresses off it, not
  `window`. The existing restart-on-early-abort logic (`fetch_t < 4 + late`) is unchanged; it now
  resets in response to `win_active`, not `window`.
- **`tile_is_window`** is `win_active`'s value frozen once, at `fetch_tile_number`'s own call —
  read back by `lo_hi` (the row) and by the push-time `fetch_x` guard, because a *late* abort
  (caught after the tile number was already read, too late to restart) must not let those two
  stages disagree about which side the tile they are completing was fetched as.

### 15.3 The reset that cost the first attempt an afternoon

The first version of the split passed every `fifo` unit test, including
`the_window_tile_grid_is_anchored_at_wx_minus_7`, and then made this exact ROM's own score
*worse* than the pre-refactor baseline (1,973 px, not an improvement). Traced on line 48 (of
`WX = LY`): `win_active` read **true** at the very first dot of the line's mode 3, before
`check_window_start` had fired at all for that line — carried over from the *previous* scanline,
whose window was still active near its own end (`WX = 47`). `start_mode3` already reset `window`
to `false` per line; the new `win_active`/`tile_is_window` fields needed the identical per-line
reset and did not have it. One line each in `start_mode3` closed it. The lesson worth keeping:
splitting one field into several means finding *every* place the old one was reset, not only
where it was read.

### 15.4 How it was found: the tests measure `lx`, not the arithmetic

`the_window_tile_grid_is_anchored_at_wx_minus_7` and `check_window_stop`'s deleted resume formula
both talk about a *position*, which invites reasoning about it in the abstract. The test itself
does nothing of the sort — it drives the real per-dot engine (`gpu.wb(0xFF40, …)` directly, no
`ppu_write_commit` staging) and reads `gpu.pipe.lx` at the exact dot `gpu.pipe.window` next reads
false. Comparing that trace against the *same* test run on an unmodified pre-refactor checkout
(`git checkout <parent> -- src/gpu/fifo.rs`, add one `eprintln!`, run, revert) is what found that
old code's `check_window_stop` was gated behind the identical `bg_len > 0` early return that
blocks ordinary fetching — so it only ever fires at a natural tile boundary, which is *why* the
resume position was always on-grid without any grid arithmetic in the old formula at all. That
gating is the one property worth preserving exactly, and `window`'s new deferred role above is a
direct port of it.

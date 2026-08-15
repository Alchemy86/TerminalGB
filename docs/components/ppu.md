# PPU (GPU) — scanline pixel engine

> Source: [`src/gpu/mod.rs`](../../src/gpu/mod.rs) · Related: [MMU](mmu.md) · [CPU](cpu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The PPU ("Picture Processing Unit"; called `Gpu` in this codebase) is the part that turns
memory into an image. The game writes tile graphics and a map of which tile goes where
into video RAM (VRAM), plus a list of moving objects ("sprites") into OAM. Sixty times a
second the PPU walks the screen **one horizontal line at a time**, top to bottom, and for
each line works out the colour of all 160 pixels from that data. The result is a
160×144 framebuffer the frontend draws to a window, terminal, or canvas.

| What this PPU renders | What the test's author says it should be |
|:---:|:---:|
| <img src="../shots/screenshot-key-acid2.png" alt="dmg-acid2 rendered by this emulator: a 160 by 144 image of a face made of background tiles, window and sprites" width="280" /> | <img src="../../third_party/dmg-acid2/reference-dmg.png" alt="The dmg-acid2 author's reference image of the same face, for a hardware-accurate DMG" width="280" /> |
| our framebuffer, straight out of `renderscan` | [`third_party/dmg-acid2/reference-dmg.png`](../../third_party/dmg-acid2/reference-dmg.png) |

<p align="center"><em><a href="https://github.com/mattcurrie/dmg-acid2">dmg-acid2</a> draws a face
in which every feature is a different PPU behaviour — sprite priority, the window, tile
flipping, the 10-sprites-per-line limit — so a wrong face names the bug. <strong>0 of 23,040
pixels differ</strong> between these two, checked on every CI run by
<code>testharness/testroms.rs</code>, comparing by shade rank rather than RGB (§7). The two
images look slightly different in tone because they are: our shades are
<code>000000/606060/c0c0c0/ffffff</code> and the reference's are
<code>000000/555555/aaaaaa/ffffff</code>, which is exactly why the comparison ranks shades
instead of matching colours.</em></p>

Crucially, the PPU is a *clock*, not just a renderer. Its progress across the screen is
observable to the running game (through the `LY`, `STAT` registers and interrupts), and
games rely on that timing to change graphics mid-frame. Getting the timing right is what
separates a toy from an accurate emulator.

## 2. The real hardware

The Game Boy PPU is a **scanline renderer with a strict, cyclic timing budget**. It does
not draw a whole frame at once; it cycles through four **modes** on every line:

- **Mode 2 — OAM scan (80 dots):** find which sprites intersect this line (max 10).
- **Mode 3 — drawing (172–289 dots, variable):** push pixels to the LCD. Longer when
  there is scrolling, a window, or many sprites on the line.
- **Mode 0 — HBlank (the rest, so `376 − mode3` dots):** idle until the line ends.
- **Mode 1 — VBlank (lines 144–153):** ten lines of idle after the visible screen,
  during which the whole of VRAM/OAM is safe to touch.

The numbers (Pan Docs, [Rendering](https://gbdev.io/pandocs/Rendering.html)):

- **456 dots per scanline** (a "dot" = one 4.19 MHz tick at normal speed).
- **154 scanlines per frame** (144 visible + 10 VBlank) = **70,224 dots/frame** ≈ 59.7 Hz.
- **`LY`** (`0xFF44`) is the current line 0–153. **`LYC`** (`0xFF45`) is a compare value;
  a match can raise a STAT interrupt.
- **`STAT`** (`0xFF41`) reports the current mode (bits 0–1) and LYC=LY (bit 2), and its
  bits 3–6 enable the four STAT interrupt sources.
- **10 sprites per line, 40 sprites total** in OAM; sprites are 8×8 or 8×16.
- **VRAM/OAM access is blocked from the CPU during certain modes** (OAM during 2/3, VRAM
  during 3): reads return `0xFF`.

## 3. History and versions

| Model | PPU capability |
|---|---|
| **DMG** | 4 shades of grey. One background palette (`BGP`) + two object palettes (`OBP0/1`), each a 2-bit → shade lookup. |
| **MGB (Pocket)** | Same PPU; the LCD has a truer black/grey response but the register model is identical. |
| **SGB** | DMG PPU output; the SNES side *recolours* it (border + up to 4-colour palettes selected by the game) — colour lives in the SNES, not the Game Boy PPU. |
| **CGB** | Real colour: **8 background + 8 object palettes of 4 colours each**, stored as 5-bit-per-channel RGB in dedicated palette RAM (`BCPS/BCPD`, `OCPS/OCPD`). Adds a second VRAM bank, per-tile attribute bytes (palette, bank, flip, priority), and HDMA to stream VRAM during HBlank. |

**This emulator implements both DMG and CGB paths** (`Gpu::new` vs `Gpu::new_cgb`) and is
driven in CGB mode by default. SGB border/recolour is **not** implemented.

**And the Color console itself shipped in steppings whose PPUs differ.**
[`PpuRevision`](../../src/mode.rs) (`CgbC` / `CgbD`, default `CgbD`, selected by the same
`$GB_CGB_REVISION` the APU reads) carries the two known differences: CPU CGB D and later latch
the fetcher's `SCY + LY` at the tile-number fetch, so the bitplane desync a DMG and a CPU CGB C
can produce is unavailable on them; and a palette write is seen one dot later on a CPU CGB C.
Both only reach the [`identical` picture mode](../picture-modes.md). Mealybug ships a separate
reference set per stepping and seven of its ROMs disagree between them — see
[`docs/mealybug.md`](../mealybug.md) §10 for the evidence and the citations.

## 4. How it works here

### 4.0 There are two picture modes

`RenderMode` ([`src/gpu/mod.rs`](../../src/gpu/mod.rs)) selects which one runs — a **user-facing
mode**, not an internal switch: see [`docs/picture-modes.md`](../picture-modes.md). Everything
else in this section describes **`standard`** (`RenderMode::Fast`), the default: it draws a whole
scanline in one go at the mode 3 → 0 transition and gives mode 3 a fixed 172-dot length, so every
register is read exactly once per line.

**`identical`** (`RenderMode::Accurate`, [`src/gpu/fifo.rs`](../../src/gpu/fifo.rs)) is the real
pixel pipeline, stepped one dot at a time — BG/window fetcher, 8-pixel FIFO, sprite fetch stalls,
`SCX & 7` discard, and a mode 3 length derived rather than assumed. Colour indices go into the
FIFO and the palette is applied at the output, which is what makes a mid-line `BGP` write move a
boundary along the scanline instead of recolouring what is already drawn. Chosen with the `P` key,
`GB_PPU=identical`, the `ppu` config key, or `Gpu::set_render_mode`, and shown in the terminal
Status panel's `PPU` row. The pre-rename spellings `fast`/`accurate` still parse everywhere.

The two are **not** an appearance choice — palettes and upscaling live in `src/screen/`, above
the PPU, and are identical either way. The axis is exactness against speed: measured 0.221 vs
0.461 ms/frame on dmg-acid2, and both are pixel-exact against its reference image. `identical`
now **loses no row to `standard` on any gate**; what keeps it opt-in is the ~2.1x cost. Where it
stands against the suite that motivates it is [`docs/mealybug.md`](../mealybug.md) §7. Both modes
are held against each other in CI by
[`testharness/ppuengines.rs`](../../testharness/ppuengines.rs).

### 4.1 The whole-scanline engine

Structure closely follows rboy's PPU (per-pixel `PrioType` priority tracking, the Gambatte
RGB-correction curve, `dmg_sprite_order`/`cgb_sprite_order`).

**The `Gpu` struct** — [`src/gpu/mod.rs:17`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L17). Key fields: `mode`,
`modeclock`, `line` (LY), `lyc`, the RGBA framebuffer `data`
([`mod.rs:55`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L55), `160*144*4` bytes), the `updated` flag the frame
loop polls, `vram` (`0x4000` = two banks), `voam` (OAM), DMG palettes `palb/pal0/pal1`,
CGB palette RAM `cbgpal`/`csprit`, and per-pixel `bgprio` for BG-over-sprite priority.

**The mode cycle** — `do_cycle` ([`src/gpu/mod.rs:244`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L244)) accumulates
ticks into `modeclock` and steps modes with these thresholds:

```
line advances every 456 ticks     (mod.rs:261)   line = (line + 1) % 154   (mod.rs:263)
mode 2 (OAM):  modeclock <= 80                    (mod.rs:276)
mode 3 (draw): modeclock <= 80 + 172  // 252      (mod.rs:280-281)
mode 0 (HBlank): remaining 204                    (mod.rs:286-287)
line >= 144  → mode 1 (VBlank)                    (mod.rs:267)
```

`renderscan` ([`mod.rs:570`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L570)) runs when the line's drawing completes;
it calls `draw_bg` ([`mod.rs:600`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L600), window folded in) then
`draw_sprites` ([`mod.rs:702`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L702)). VBlank sets the `0x01` interrupt and
`updated = true` ([`mod.rs:323`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L323)).

**The 10-sprite limit IS modelled** — `draw_sprites` collects into a 10-slot array and
breaks once full ([`mod.rs:710`,`721`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L710)). 8×16 mode masks the tile
LSB ([`mod.rs:737`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L737)).

**STAT/LYC** — there is **one interrupt line**, not four triggers. `stat_signal`
([`mod.rs:344`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L344)) ORs the enabled sources together and
`refresh_stat_line` ([`mod.rs:400`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L400)) raises `IF.1` on its
**0→1 transition only**; every path that can move a term (mode change, `LY` increment,
writes to `STAT`/`LYC`/`LCDC`) calls it. `STAT` bit 2 is a **latched** coincidence
flip-flop (`lyc_match`), not a live `line == lyc` comparison — with the LCD off the
comparison clock stops, so the flag freezes and a new `LYC` is invisible until the PPU
runs again ([`mod.rs:410`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L410)).

**Palettes** — DMG `get_monochrome_pal_val` maps to greys 255/192/96/0
([`mod.rs:561`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L561)); CGB packs 5-bit RGB in `BCPD`/`OCPD` writes
([`mod.rs:502`,`522`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L502)) and outputs via `setrgb` with a Gambatte-style
colour-correction curve ([`mod.rs:585`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L585)).

### Honest approximations

> **These describe the default `standard` picture mode.** The per-dot pixel FIFO of §4.0
> (`identical`, `GB_PPU=identical`, [`src/gpu/fifo.rs`](../../src/gpu/fifo.rs)) does all three of
> the first bullets properly — variable mode 3, per-dot stepping, per-pixel output — and is gated
> in CI by `make test-external-accurate`. It is not the default because it costs about 2.1x the
> CPU, not because it is behind on anything; the measured trade is in
> [`docs/picture-modes.md`](../picture-modes.md) and
> [`docs/measured/ppu.md`](../measured/ppu.md). Read what follows as *"what you get by
> default"*, not *"what this emulator can do"*.

- **Mode 3 is fixed at 172 dots.** Real mode 3 is **variable (172–289)** — it lengthens with
  `SCX` fine-scroll, window activation, and sprite count, stealing time from HBlank. The fast
  engine uses a flat `80 / 172 / 204` split
  ([`mod.rs:276–286`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L276)) with no penalty model, so games that time raster
  effects to mode-3 length will drift. *(No longer a non-goal: the accurate engine derives it
  from the fetcher and charges Pan Docs' OBJ penalty explicitly. dmg-acid2 cannot prove either
  way — it is a static single-frame composition and **already renders pixel-correct** on both
  engines, see §6.12 — which is why Mealybug Tearoom is wired as the yardstick instead.)*
- **Line stepping is coarse (≤80-tick chunks), not per-dot.** `do_cycle` consumes ticks in
  blocks ([`mod.rs:255`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L255)); a mid-scanline register read may see a
  slightly stale mode/`LY`.
- **The whole scanline is rendered in one shot** at the end of mode 3, so a mid-line write
  to `SCX`/palette during mode 3 is not reflected within that line (no per-pixel FIFO).
- **VRAM/OAM read- and write-blocking during modes 2/3 is not enforced on this engine** (the
  CPU can read and write VRAM any time) — see MMU. That is deliberate and must stay:
  enforcing blocking on top of the fixed-length mode 3 above would be *worse* than not
  enforcing it, because a write the game placed legally in HBlank could land in our
  approximate mode 3 and be silently dropped. **The `identical` engine does enforce it**,
  because it has both halves the rule needs — a mode-3 length its fetcher produces, and a PPU
  standing on the exact dot of the access — and with it
  `acceptance/ppu/lcdon_timing-GS`, `lcdon_write_timing-GS` and `intr_2_oam_ok_timing` pass
  there. Both sample OAM/VRAM accessibility at **1 M-cycle** resolution across the first
  three scanlines after the LCD is switched on. See §4.0 and
  [`docs/conformance-notes.md`](../conformance-notes.md) (2026-08-15).
- The old `// Full line takes 114 ticks` comment (a stale M-cycle count that predated the
  456-dot loop) has been corrected in place to an accurate 456-dot note
  ([`mod.rs:259`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L259)), which links back to this page.

## 5. Known quirks and edge cases

- **STAT interrupt blocking / the "STAT IRQ line" — MODELLED.** The four STAT sources feed
  **one** shared interrupt line and the interrupt fires on its *rising edge*, so a second
  source going active while the line is already high does **not** re-trigger. Raster-effect
  titles that run a handler off both the HBlank and the LYC source depend on it: firing per
  source runs their split twice as fast as the game intended. Pinned by
  `gpu::tests::a_source_going_high_behind_another_raises_nothing` and by Mooneye
  `acceptance/ppu/stat_irq_blocking` (FAIL→PASS). (Pan Docs — [STAT](https://gbdev.io/pandocs/STAT.html);
  AntonioND's TCAGBD §8.7.)
- **The DMG STAT write bug — NOT modelled.** On DMG, writing *any* value to `STAT`
  (even `0x00`) during OAM scan / HBlank / VBlank / LY=LYC can spuriously fire the STAT
  interrupt, as if `0xFF` were written for one M-cycle. Two commercial games depend on it.
- **Mid-scanline `SCX`/`SCY`/`WX`/`WY`/palette writes** are a legitimate technique
  (parallax, status bars). With whole-line rendering (§4) these land at line granularity,
  not pixel granularity. This is what the **Mealybug Tearoom** suite measures, and
  [`docs/mealybug.md`](../mealybug.md) is the scoping of what fixing it would take.
- **The window's frame-long "Y condition"** is latched at the *start of a scanline* when
  `WY == LY`, and **does not depend on `LCDC` bit 5** — a game may latch it on a line where
  the window is disabled and enable the window later in the same frame (Pan Docs, "Window
  rendering criteria"). Modelled in the mode-2 arm of `change_mode` via
  `check_wy_trigger`; cleared on VBlank. *Not* modelled: Pan Docs' GBC-only note that
  clearing `LCDC` bit 5 resets the condition.
- **The window's internal line counter** only advances on lines where the window is
  visible — modelled via `wy_pos` ([`mod.rs:605`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L605)).
- **The OAM strobe also happens on line 144 — and *only* on line 144.** A program that
  enables only the mode-2 source still takes a STAT interrupt at `LY = 144`, simultaneous
  with the VBlank interrupt (Mooneye `acceptance/ppu/vblank_stat_intr-GS` measures the two
  as coincident); missing it puts a whole frame's worth of per-STAT state on the wrong
  parity of lines. But it is a **strobe on that one line**, not "mode 1 honours the mode-2
  bit": modelling it as the latter holds the STAT line high across lines 145–153, which
  *blocks* the real mode 2 at `LY = 0` and delivers its interrupt a whole scanline late —
  which is exactly the gap `acceptance/ppu/intr_1_2_timing-GS` counts. Modelled in
  `oam_strobe`; pinned by `gpu::tests::the_vblank_oam_strobe_is_line_144_only` and
  `entering_vblank_raises_stat_for_the_mode_2_source_too`.
- **On Color hardware that strobe starts one M-cycle earlier** (`VBLANK_STROBE_LEAD`).
  Mooneye ships the same program twice with one `nop` moved —
  `acceptance/ppu/vblank_stat_intr-GS` (DMG/MGB/SGB, coincident) and
  `misc/ppu/vblank_stat_intr-C` (CGB/AGB/AGS, STAT one cycle first) — and each fails on the
  other machine, so the gap is pinned in both directions.
  **It must not reorder the two interrupts when both are enabled**, and that is not a
  contradiction: interrupt dispatch takes its vector from `IE & IF` at M4, so VBlank's flag
  going up one M-cycle into STAT's dispatch *redirects* it to the higher-priority vector.
  Mealybug's `m2_win_en_toggle` is the check — its DMG and CGB reference photographs are
  identical in shade rank — and satisfying it needs the batched PPU to keep running through
  a dispatch (`MemoryManagementUnit::set_ppu_bus_ticking`; the per-dot engine already did).
  Pinned by `gpu::tests::a_colour_console_strobes_one_m_cycle_before_the_vblank_flag`.
- **Where the PPU is when the boot ROM hands over is a per-console constant**
  (`Gpu::set_boot_position`). The LCD is switched on by the boot ROM, not the cartridge, so
  the PPU is already free-running at `PC=$0100`; a DMG ABC arrives at line 0 and a DMG 0 —
  which skips the later revisions' `CheckLogo` — arrives nine scanlines earlier, still in
  VBlank. `acceptance/boot_hwio-dmg0` reads `STAT = $83` and `LY = $01` where
  `boot_hwio-dmgABCmgb` reads `$80`/`$0A`. Only DMG 0 gets a position: `boot_hwio-S` and
  `misc/boot_hwio-C` skip `$FF40`/`$FF41`/`$FF44` in their own masks, so nothing measures it
  on those machines and nothing is invented for them.
- **The line the PPU restarts on has no OAM scan.** After `LCDC` bit 7 goes 0→1, `STAT`
  reads mode 0 through what would be the OAM slot and then goes straight to mode 3
  (`acceptance/ppu/lcdon_timing-GS`: "line 0 starts with mode 0 and goes straight to
  mode 3"). Modelled via `skip_first_oam`; pinned by
  `gpu::tests::the_line_the_ppu_restarts_on_has_no_oam_scan`. What is **still not**
  modelled is that line's exact *phase* — see §5's `lcdon_*` note below.
- **OAM DMA restricted-bus interaction** — see [MMU](mmu.md); OAM writes during a real DMA
  are blocked, which this emulator does not enforce.

## 6. Expected behaviours as tests

> **Coverage today:** the framebuffer itself is pinned (dmg-acid2 frame hash in
> `testharness/baseline.txt`, #12), the whole STAT interrupt line has unit tests (#5, #11,
> #13, #14, #16), and the mid-mode-3 gap and the LCD-on phase are *measured* rather than
> merely asserted (#15, #17). The mode-timing items below are still uncovered.

1. **Frame geometry.** A full frame is 154 lines × 456 dots = **70,224 dots**; `LY` cycles
   `0→153→0`. *(Not covered.)*
2. **Mode sequence on a visible line.** For `LY < 144`, modes advance 2 → 3 → 0 with the
   emulator's `80 / 172 / 204` boundaries; `STAT` bits 0–1 reflect the current mode.
   *(Not covered. Note: hardware mode 3 is variable — see #3.)*
3. **Mode 3 length is variable on hardware.** With `SCX & 7 != 0`, a window enabled, or
   sprites present, mode 3 exceeds 172 dots and HBlank shortens. **Expected to FAIL —
   fixed-length approximation.** *(Not covered; known-divergent.)*
4. **VBlank.** Entering `LY == 144` sets `IF` bit 0 (VBlank) once, and `STAT` mode = 1 for
   lines 144–153. *(Not covered.)*
5. **LYC=LY.** Setting `LYC = 40` and enabling STAT bit 6 raises `IF` bit 1 exactly when
   `LY` reaches 40; `STAT` bit 2 reads 1 only while `LY == LYC` — and bit 2 is a *latched*
   flip-flop, so switching the LCD off freezes it and a new `LYC` written while off is
   invisible until the PPU restarts. **COVERED for the latch** —
   `gpu::tests::the_lyc_comparison_clock_stops_with_the_lcd` and
   `enabling_the_lcd_raises_stat_only_if_the_coincidence_flag_flips_on`, plus Mooneye
   `acceptance/ppu/stat_lyc_onoff`, which this turned from red to green.
6. **10-sprites-per-line limit.** With 11 sprites on one line, only the first 10 (by
   selection order) are drawn. *(Not covered.)*
7. **Sprite priority.** On DMG, among overlapping sprites the lower X (then lower OAM
   index) wins; a BG pixel of colour 0 is always behind a sprite, colours 1–3 respect the
   OBJ-behind-BG attribute. *(Not covered.)*
8. **8×16 sprites** ignore the tile-number LSB (top tile even, bottom odd). *(Not covered.)*
9. **DMG palette mapping.** With `BGP = 0xE4`, BG colour indices 0/1/2/3 map to shades
   255/192/96/0. *(Not covered.)*
10. **CGB palette RAM auto-increment.** Writing `BCPS` with the auto-increment bit set, then
    eight `BCPD` writes, fills one 4-colour palette and advances the index. *(Not covered.)*
11. **STAT interrupt blocking.** Two STAT sources active in the same window raise the STAT
    interrupt only once (rising edge). **COVERED** —
    `gpu::tests::a_source_going_high_behind_another_raises_nothing`, plus Mooneye
    `acceptance/ppu/stat_irq_blocking`, which this turned from red to green.
12. **dmg-acid2 / cgb-acid2 render.** The reference "face" test image renders pixel-correct
    (see [testing](../testing.md)). **COVERED for dmg-acid2** — the harness pins its frame
    hash, and a pixel diff against `third_party/dmg-acid2/reference-dmg.png` shows **zero
    differing pixels** across the four gray levels (only the exact shade constants differ,
    a palette choice). The PPU already draws the acid2 face correctly.
13. **The window Y condition latches with the window disabled.** With `WY = 5` and `LCDC`
    bit 5 clear, reaching `LY = 5` must latch the condition so the window appears if it is
    enabled later in the frame. **COVERED** —
    `gpu::tests::the_wy_condition_latches_while_the_window_is_disabled`, and Mealybug
    `m2_win_en_toggle` is pixel-exact against all three hardware references.
14. **The OAM strobe reaches line 144, and stops there.** With `STAT = $20` (mode-2 source
    only) and mode-1 select clear, entering `LY = 144` must still request a STAT interrupt —
    and lines 145–153 must request none, so the real mode 2 at `LY = 0` is not blocked.
    **COVERED** — `gpu::tests::entering_vblank_raises_stat_for_the_mode_2_source_too` and
    `the_vblank_oam_strobe_is_line_144_only`, plus
    `mooneye/acceptance/ppu/vblank_stat_intr-GS` and `intr_1_2_timing-GS`.
16. **The line the PPU restarts on has no OAM scan.** Immediately after `LCDC` bit 7 goes
    0→1, `STAT` must read mode 0, not mode 2, and go straight to mode 3. **COVERED** —
    `gpu::tests::the_line_the_ppu_restarts_on_has_no_oam_scan`.
17. **OAM/VRAM accessibility after the LCD is switched on.** Reads and writes to OAM and
    VRAM must be blocked or pass according to the mode the PPU is in, at 1 M-cycle
    resolution, over the first three scanlines — and **a read is blocked one dot before a
    write is**, at each dot the PPU takes a bus. **COVERED on the `identical` engine**
    (`Gpu::cpu_can_read_vram` and its three siblings;
    `gpu::tests::a_write_still_lands_on_the_dot_a_read_is_already_blocked`), and
    deliberately **not** on the default one — a fixed-length mode 3 would drop writes a
    game placed legally in HBlank. *(Mooneye `acceptance/ppu/lcdon_timing-GS`,
    `lcdon_write_timing-GS` and `intr_2_oam_ok_timing`, all green on `identical`.)*
15. **Mid-mode-3 register writes take effect part-way along the line.** Writing `BGP`,
    `LCDC`, `SCX`, `SCY` or `WX` during mode 3 must change only the pixels drawn after the
    write, at the fetcher's own granularity. **Expected to FAIL — whole-line rendering.**
    *(Covered as a measurement by `cargo run --release --example mealybug`: 1 of 31 ROMs
    pixel-exact. Scoping in [`docs/mealybug.md`](../mealybug.md).)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **dmg-acid2** (Matt Currie) | A single-frame BG/window/sprite composition that draws a face; any PPU feature bug distorts it | The best cheap visual PPU regression check — see [testing](../testing.md). |
| **cgb-acid2** | The CGB equivalent (colour palettes, attributes, priority) | CGB attribute/palette/priority bugs. |
| **Mooneye `acceptance/ppu/`** (`intr_1_2_timing`, `intr_2_0_timing`, `lcdon_timing`, `stat_irq_blocking`, `vblank_stat_intr`, …) | Exact mode/STAT/LYC interrupt timing | Directly targets §4/§5 approximations. **Quote these per engine**: on `identical` only the five `intr_2_*` rows are red, and all five are one M-cycle long because of `OAM_STROBE_LEAD` (see [`docs/conformance-notes.md`](../conformance-notes.md), 2026-08-15); on the default engine eight are red. Run with `GB_PPU=identical cargo run --release --example mooneye_scout -- <suite>/acceptance`. |
| **Mealybug Tearoom** (Matt Currie) | PPU register writes *during* mode 3, against DMG / CPU CGB C / CPU CGB D reference screenshots | Directly targets the whole-line renderer. **1 of 31 ROMs pixel-exact**; run it with `cargo run --release --example mealybug -- --dir <checkout>`. Full accounting and the cost of the rest: [`docs/mealybug.md`](../mealybug.md). |
| **Blargg `oam_bug`** | The OAM corruption bug (DMG) | **8/8 sub-tests pass on `@dmg`** — the bug is modelled in [`src/gpu/oambug.rs`](../../src/gpu/oambug.rs), driven by `Gpu::oam_scan_row_in`. A failure means the mode-2 dot window, the row the PPU is on, or a corruption pattern has moved. `@cgb` is expected to fail: CGB silicon has no such defect. See [MMU §5](mmu.md). |
| **Blargg `cgb_sound`** shares no PPU code, but its screenshot is in the repo as "failing" | — | (APU, listed under [APU](apu.md).) |

The declared submodule `retrio/gb-test-roms` is Blargg-only and **uninitialised**; acid2
and Mooneye are separate repos not yet referenced. See [testing](../testing.md) for the
proposed headless harness.

# PPU (GPU) — scanline pixel engine

> Source: [`src/gpu/mod.rs`](../../src/gpu/mod.rs) · Related: [MMU](mmu.md) · [CPU](cpu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`151dd79`](https://github.com/Alchemy86/gameboy/tree/151dd797c4b34712a38edf423efbc041649bfa35) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The PPU ("Picture Processing Unit"; called `Gpu` in this codebase) is the part that turns
memory into an image. The game writes tile graphics and a map of which tile goes where
into video RAM (VRAM), plus a list of moving objects ("sprites") into OAM. Sixty times a
second the PPU walks the screen **one horizontal line at a time**, top to bottom, and for
each line works out the colour of all 160 pixels from that data. The result is a
160×144 framebuffer the frontend draws to a window, terminal, or canvas.

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

## 4. How it works here

Structure closely follows rboy's PPU (per-pixel `PrioType` priority tracking, the Gambatte
RGB-correction curve, `dmg_sprite_order`/`cgb_sprite_order`).

**The `Gpu` struct** — [`src/gpu/mod.rs:17`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L17). Key fields: `mode`,
`modeclock`, `line` (LY), `lyc`, the RGBA framebuffer `data`
([`mod.rs:55`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L55), `160*144*4` bytes), the `updated` flag the frame
loop polls, `vram` (`0x4000` = two banks), `voam` (OAM), DMG palettes `palb/pal0/pal1`,
CGB palette RAM `cbgpal`/`csprit`, and per-pixel `bgprio` for BG-over-sprite priority.

**The mode cycle** — `do_cycle` ([`src/gpu/mod.rs:244`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L244)) accumulates
ticks into `modeclock` and steps modes with these thresholds:

```
line advances every 456 ticks     (mod.rs:261)   line = (line + 1) % 154   (mod.rs:263)
mode 2 (OAM):  modeclock <= 80                    (mod.rs:276)
mode 3 (draw): modeclock <= 80 + 172  // 252      (mod.rs:280-281)
mode 0 (HBlank): remaining 204                    (mod.rs:286-287)
line >= 144  → mode 1 (VBlank)                    (mod.rs:267)
```

`renderscan` ([`mod.rs:535`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L535)) runs when the line's drawing completes;
it calls `draw_bg` ([`mod.rs:565`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L565), window folded in) then
`draw_sprites` ([`mod.rs:667`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L667)). VBlank sets the `0x01` interrupt and
`updated = true` ([`mod.rs:313`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L313)).

**The 10-sprite limit IS modelled** — `draw_sprites` collects into a 10-slot array and
breaks once full ([`mod.rs:675`,`686`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L675)). 8×16 mode masks the tile
LSB ([`mod.rs:702`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L702)).

**STAT/LYC** — `check_interrupt_lyc` ([`mod.rs:295`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L295)) raises `0x02`
when `line == lyc` and enabled; mode-entry STAT interrupts fire in `change_mode`
([`mod.rs:301`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L301)).

**Palettes** — DMG `get_monochrome_pal_val` maps to greys 255/192/96/0
([`mod.rs:526`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L526)); CGB packs 5-bit RGB in `BCPD`/`OCPD` writes
([`mod.rs:467`,`487`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L467)) and outputs via `setrgb` with a Gambatte-style
colour-correction curve ([`mod.rs:550`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L550)).

### Honest approximations

- **Mode 3 is fixed at 172 dots (documented non-goal).** Real mode 3 is **variable
  (172–289)** — it lengthens with `SCX` fine-scroll, window activation, and sprite count,
  stealing time from HBlank. This emulator uses a flat `80 / 172 / 204` split
  ([`mod.rs:276–286`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L276)) with no penalty model. Games that time raster
  effects to mode-3 length will drift. **Deliberately not fixed here:** a faithful
  variable mode 3 (and the VRAM/OAM bus-blocking that rides on it) requires a per-dot,
  cycle-stepped PPU sharing the bus with a cycle-stepped CPU — the structural restructure
  `docs/debugging.md` flags as justified *only* if the project commits to full cycle
  accuracy. No wired ROM proves it (dmg-acid2 is a static single-frame composition and
  **already renders pixel-correct** — see §6.12), so it stays an approximation.
- **Line stepping is coarse (≤80-tick chunks), not per-dot.** `do_cycle` consumes ticks in
  blocks ([`mod.rs:255`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L255)); a mid-scanline register read may see a
  slightly stale mode/`LY`.
- **The whole scanline is rendered in one shot** at the end of mode 3, so a mid-line write
  to `SCX`/palette during mode 3 is not reflected within that line (no per-pixel FIFO).
- **VRAM/OAM read-blocking during modes 2/3 is not enforced** (the CPU can read VRAM any
  time) — see MMU.
- The old `// Full line takes 114 ticks` comment (a stale M-cycle count that predated the
  456-dot loop) has been corrected in place to an accurate 456-dot note
  ([`mod.rs:259`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L259)), which links back to this page.

## 5. Known quirks and edge cases

- **STAT interrupt blocking / the "STAT IRQ line" — NOT modelled.** On hardware the four
  STAT sources feed **one** shared interrupt line; an interrupt only fires on its
  *rising edge*, so a second source going active while the line is already high does
  **not** re-trigger. Here each source ORs `0x02` independently
  ([`mod.rs:297`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L297)) — no edge detection. (Pan Docs — [STAT](https://gbdev.io/pandocs/STAT.html).)
- **The DMG STAT write bug — NOT modelled.** On DMG, writing *any* value to `STAT`
  (even `0x00`) during OAM scan / HBlank / VBlank / LY=LYC can spuriously fire the STAT
  interrupt, as if `0xFF` were written for one M-cycle. Two commercial games depend on it.
- **Mid-scanline `SCX`/`SCY`/`WX`/`WY`/palette writes** are a legitimate technique
  (parallax, status bars). With whole-line rendering (§4) these land at line granularity,
  not pixel granularity.
- **The window's internal line counter** only advances on lines where the window is
  visible — modelled via `wy_pos` ([`mod.rs:570`](https://github.com/Alchemy86/gameboy/blob/main/src/gpu/mod.rs#L570)).
- **OAM DMA restricted-bus interaction** — see [MMU](mmu.md); OAM writes during a real DMA
  are blocked, which this emulator does not enforce.

## 6. Expected behaviours as tests

> **Coverage today: none.** Framebuffer/timing tests do not exist yet.

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
   `LY` reaches 40; `STAT` bit 2 reads 1 only while `LY == LYC`. *(Not covered.)*
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
    interrupt only once (rising edge). **Expected to FAIL — not modelled.** *(Not covered.)*
12. **dmg-acid2 / cgb-acid2 render.** The reference "face" test image renders pixel-correct
    (see [testing](../testing.md)). **COVERED for dmg-acid2** — the harness pins its frame
    hash, and a pixel diff against `third_party/dmg-acid2/reference-dmg.png` shows **zero
    differing pixels** across the four gray levels (only the exact shade constants differ,
    a palette choice). The PPU already draws the acid2 face correctly.

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **dmg-acid2** (Matt Currie) | A single-frame BG/window/sprite composition that draws a face; any PPU feature bug distorts it | The best cheap visual PPU regression check — see [testing](../testing.md). |
| **cgb-acid2** | The CGB equivalent (colour palettes, attributes, priority) | CGB attribute/palette/priority bugs. |
| **Mooneye `acceptance/ppu/`** (`intr_1_2_timing`, `intr_2_0_timing`, `lcdon_timing`, `stat_irq_blocking`, `vblank_stat_intr`, …) | Exact mode/STAT/LYC interrupt timing | Directly targets §4/§5 approximations — expected reds. **Not the declared submodule; not wired.** |
| **Blargg `oam_bug`** | The OAM corruption bug (DMG) | Fails — the bug isn't modelled. |
| **Blargg `cgb_sound`** shares no PPU code, but its screenshot is in the repo as "failing" | — | (APU, listed under [APU](apu.md).) |

The declared submodule `retrio/gb-test-roms` is Blargg-only and **uninitialised**; acid2
and Mooneye are separate repos not yet referenced. See [testing](../testing.md) for the
proposed headless harness.

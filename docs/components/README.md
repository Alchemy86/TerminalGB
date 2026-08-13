# Component reference

> Per-component deep dives for the [gameboy](../../README.md) emulator. Each page covers, in
> the same order: **what it is** (in plain terms) · **the real hardware** · **history across
> DMG/MGB/SGB/CGB** (and MBC types) · **how it works here** with `path:line` references and
> honest approximations · **known quirks** · **expected behaviours phrased as future tests**
> (marked *covered* / *not covered*) · **the Blargg/Mooneye ROMs that exercise it**.
>
> The expected-behaviour lists are the specification we test *against* — written **before**
> any emulation code is changed. The shared yardstick they draw from is on the
> **[accuracy testing](../testing.md)** page.

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`6ec57e1`](https://github.com/Alchemy86/gameboy/tree/6ec57e1) · regenerate with `make docs-links`, verify with `make docs-check`.

## The components

The list below is derived from the code (`src/cpu`, `src/gpu`, `src/mmu`, `src/mbc`,
`src/sound.rs`, `src/mmu/timer.rs`, `src/mmu/serial.rs`, `src/input.rs`, and the
frontends), split where the hardware is genuinely distinct — Timer and Serial get their own
pages because they are separate chips with their own registers, quirks, and test suites,
even though they live under `src/mmu/`.

| Component | Source | What it does |
|---|---|---|
| **[CPU](cpu.md)** | [`src/cpu/`](../../src/cpu) | The Sharp **LR35902 (SM83)** core — registers, flags, the full opcode + `CB` tables, interrupts. *Not a Z80.* |
| **[PPU (GPU)](ppu.md)** | [`src/gpu/mod.rs`](../../src/gpu/mod.rs) | Scanline pixel engine — the mode cycle, background/window/sprites, DMG + CGB palettes, STAT/LYC. |
| **[MMU](mmu.md)** | [`src/mmu/mod.rs`](../../src/mmu/mod.rs) | The 64 KB memory bus — address routing, OAM DMA, CGB HDMA/GDMA, and the `IF`/`IE` interrupt wiring. |
| **[Cartridges (MBC)](mbc.md)** | [`src/mbc/`](../../src/mbc) | Bank switching for MBC0/1/2/3/5, MBC3's real-time clock, and battery saves. |
| **[APU (Sound)](apu.md)** | [`src/sound.rs`](../../src/sound.rs) | Four-channel audio (2 square + wave + noise), the 512 Hz frame sequencer, `blip_buf` → `cpal`. *Native only.* |
| **[Timer & Divider](timer.md)** | [`src/mmu/timer.rs`](../../src/mmu/timer.rs) | `DIV`/`TIMA`/`TMA`/`TAC` — the programmable interrupt heartbeat. |
| **[Serial](serial.md)** | [`src/mmu/serial.rs`](../../src/mmu/serial.rs) | The link cable — **and the channel Blargg test ROMs report results through.** |
| **[Joypad (Input)](input.md)** | [`src/input.rs`](../../src/input.rs) | The 2×4 button matrix at `0xFF00` and the joypad interrupt. |
| **[Frontends](frontends.md)** | [`src/screen/`](../../src/screen), [`examples/`](../../examples), [`web/`](../../web) | Desktop (OpenGL), terminal (ratatui), web (canvas), and the C/Go FFI. |

## How they fit together

```
frontend ──▶ Gameboy ──▶ CPU ──▶ MMU ──┬─▶ PPU   ─▶ framebuffer ─▶ frontend
   │                       │             ├─▶ MBC   (cartridge)
 keys                   interrupts       ├─▶ APU   (native)
                        (IF / IE)        ├─▶ Timer / Serial
                                         └─▶ Joypad
```

The **MMU is the hub**: the CPU only reads/writes addresses, and the MMU routes each to a
component. Components raise interrupts by setting a bit the MMU folds into `IF`; the CPU
dispatches them (priority: VBlank → STAT → Timer → Serial → Joypad).

## Read this first

- **[🗺️ Code map](../codemap.md)** — the top-level orientation page: how a frame flows, how a
  memory access travels CPU → MMU → device, how interrupts are delivered, and where the
  frontends attach — with clickable links into the source and the linking strategy that keeps
  them honest.
- **[Accuracy testing / the yardstick](../testing.md)** — the Blargg, Mooneye, and
  dmg-acid2/cgb-acid2 suites every component page's test list refers to, how a headless
  harness would run them, and an honest note on the "Nintendo's emulator scored 100%" claim.
- **[Debug-tooling design](../debugging.md)** — the inspect / manipulate / capture surface
  (registers, disassembly, tile & OAM viewers, stepping, breakpoints, watchpoints,
  screenshots, and the deterministic frame-hashing that feeds the accuracy tests), plus
  Rust breakpoint handlers with a declarative runtime-configurable condition format and a
  public extension surface (emit / control / transport) external tools can plug into — with a
  core/frontend boundary that keeps debug cost out of release builds.

## Provenance & honesty

The emulation core is closely modelled on **[mvdnes/rboy](https://github.com/mvdnes/rboy)**
(the one literal `rboy` credit in the tree is [`src/gameboy.rs:50`](https://github.com/Alchemy86/gameboy/blob/main/src/gameboy.rs#L50),
but the structure runs throughout). Every page's **"honest approximations"** section names
where this emulator is *not* cycle-perfect — the fixed PPU mode-3 length, instantaneous
unguarded OAM DMA, separate timer counters, the unmodelled HALT and OAM-corruption bugs, the
16-vs-20 T-cycle interrupt dispatch, and the MBC3-for-everything web/FFI bug. Those are not
apologies; they are the map of exactly what the test suites above will turn red, and the
work list for making the emulation more accurate.

Hardware behaviour throughout is grounded in **[Pan Docs](https://gbdev.io/pandocs/)** and
the gbdev community docs, cited inline. Where a specific timing number was uncertain, the
page says so rather than inventing one.

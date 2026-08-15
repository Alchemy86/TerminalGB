# The measured Game Boy

> **Pan Docs says X. Here is what we measured. Here is the test that catches it. Here is what
> it looks like when an emulator gets it wrong.**

[gbdev.io](https://gbdev.io) is where the Game Boy's hardware is *documented*. This section is
where it is **measured** — the constants we calibrated by sweeping them until a test ROM
agreed, the suite scores before and after each fix, and the failures we left red on evidence.

That material does not exist anywhere else, because it is a by-product of building a
particular emulator and writing down every step. Everything here is a number this repository
produced, on a machine, with the command that reproduces it printed beside it.

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

## What this section is *not*

It is not a hardware reference, and it will never become one. **Pan Docs already is that**, it
is maintained by people who verified it against real silicon, and duplicating it here would
only give you a second copy that goes stale. Every page below cites Pan Docs by section and
links out.

The rule this section is written under — the same one in
[`CONTRIBUTING.md`](../../CONTRIBUTING.md) § *Third-party material & licences*:

> **Facts and formats, yes — copies, no.** An address, a timing, a register behaviour is a
> fact; citing it is what this repo already does on every accuracy fix. Wholesale
> re-explaining somebody else's reference is duplication whatever its licence permits.

### A correction worth recording: Pan Docs is CC0, not CC BY-SA

This was checked rather than assumed, because the two licences imply very different
obligations and the wrong belief was in circulation while this section was being written.
The `LICENSE` file at the root of [`gbdev/pandocs`](https://github.com/gbdev/pandocs) is
**CC0 1.0 Universal** — a public-domain dedication — added in a single commit in January 2020
and never changed since. The project's README states it plainly: *"Pan Docs is released into
the public domain."*

So there is **no copyleft hazard** for an MPL-2.0 repository, and no legal barrier to quoting
a line of it. Our reason for not reproducing it is editorial, not legal: a second copy of a
maintained reference is a liability. **Cite and link generously; still don't fork it.**

---

## The territory: what gbdev.io publishes

Read this first if you are new. Everything below is somebody else's work, linked rather than
repeated.

| Project | What it is | Why this repo depends on it |
|---|---|---|
| **[Pan Docs](https://gbdev.io/pandocs/)** | The single most comprehensive public technical reference to the Game Boy. | The hardware citation behind essentially every accuracy fix here. |
| **[awesome-gbdev](https://gbdev.io/resources.html)** | Curated index of tools, docs, tutorials, emulators and open-source ROMs. | The map for anything not in Pan Docs. |
| **[RGBDS](https://rgbds.gbdev.io/)** | The de-facto Game Boy assembler toolkit. | What [`pret/pokered`](https://github.com/pret/pokered) builds with — how our Gen 1 addresses are *derived* rather than guessed. |
| **[hardware.inc](https://github.com/gbdev/hardware.inc)** | Standard register/constant definitions for RGBDS projects. | The canonical spelling of register names used throughout these pages. |
| **[GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)** | Automated accuracy comparison across 18 emulators, every verdict a screenshot comparison. | The population our published score is measured on — see [scoreboard](scoreboard.md) and [`docs/shootout.md`](../shootout.md). |
| **[Game Boy CPU opcode tables](https://gbdev.io/gb-opcodes/optables)** | Every opcode, its length, cycles and flags. | The reference the CPU core is checked against. |
| **[GB ASM Tutorial](https://gbdev.io/gb-asm-tutorial/)** | Learning to write Game Boy assembly from zero. | Where to start if you want to *write* the test ROM rather than pass it. |
| **[GBDK 2020](https://github.com/gbdk-2020/gbdk-2020)** | C toolchain for the Game Boy. | — (not used here; listed for completeness) |
| **[Homebrew Hub](https://hh.gbdev.io/)** | Hundreds of freely distributable homebrew ROMs, playable in-browser. | Legally shareable cartridges to test against, where commercial ROMs cannot be. |
| **[The Game Boy Archive](https://github.com/gb-archive)** | Preservation mirror of scene software, hardware docs and literature. | Provenance for older, fragmented references. |
| **[rgbds-live](https://gbdev.io/rgbds-live/)** | In-browser RGBDS assembler and emulator. | Quickest way to build a one-off probe ROM. |

**gbdev guides** — short, focused articles, several of which name behaviour this repo went on
to measure:

| Guide | Measured here |
|---|---|
| [The Timing of LYC/STAT Handlers](https://gbdev.io/guides/lyc_timing.html) | [ppu.md § the STAT line](ppu.md#the-stat-interrupt-is-one-line-not-four-triggers) |
| [Interrupts tutorial](https://evie.gbdev.io/blog/interrupts.html) | [timing.md § interrupt dispatch](timing.md#interrupt-dispatch-is-five-m-cycles-and-picks-its-vector-halfway-through) |
| [DMA Hijacking](https://gbdev.io/guides/dma_hijacking.html) | [timing.md § OAM DMA](timing.md#oam-dma-is-a-timed-160-m-cycle-transfer) |
| [Adding a custom SGB border](https://gbdev.io/guides/sgb_border.html) | Deliberately **not** implemented — [`docs/components/sgb.md`](../components/sgb.md) argues why |
| [Dead C Scroll](https://gbdev.io/guides/deadcscroll.html) · [ASM style](https://gbdev.io/guides/asmstyle.html) · [Choosing tools](https://gbdev.io/guides/tools.html) | — |

Community: [Discord/IRC](https://gbdev.io/chat.html) ·
[GitHub](https://github.com/gbdev) · [Mastodon](https://fedi.gbdev.io/@gbdev) ·
[donate](https://opencollective.com/gbdev). For Game Boy *Advance*,
[gbadev.net](https://gbadev.net/).

---

## Reading other emulators: what the licences actually allow

The other reference nobody publishes as a reference: **the emulators ahead of you are open
source, and several of them answer a question the documentation does not.** Two of the fixes on
the [PPU page](ppu.md) came from reading them rather than from first principles, and cost hours
instead of weeks.

The blocker is never technical, it is a licence question — so it is answered here once, with
the file checked in each case rather than assumed. This is the same discipline
[`CONTRIBUTING.md`](../../CONTRIBUTING.md) applies to Scale2x and to FlashGBX, and it works in
both directions: it tells you what you *may* do as well as what you may not.

| Project | Licence, as checked | What that permits an MPL-2.0 repo |
|---|---|---|
| [mgba-emu/mGBA](https://github.com/mgba-emu/mgba) | **MPL-2.0** — root `LICENSE` is the Mozilla Public License Version 2.0 verbatim; the README states *"distributed under the Mozilla Public License version 2.0"*. Bundled third-party components are separately licensed and permissive (inih BSD-3, LZMA SDK / MurmurHash3 / getopt-for-MSVC / SQLite3 public domain) | **The same licence as this repository.** File-for-file compatible, so its code is not merely readable for behaviour but *reusable* with file-level attribution and the MPL notice retained. Still prefer reimplementing in this codebase's idiom, and cite repo + file + symbol for anything taken. |
| [Docheinstein/docboy](https://github.com/Docheinstein/docboy) | MIT (`LICENSE`, "Copyright (c) 2025 Docheinstein") | Compatible. Read for behaviour; reimplement and cite. |
| [LIJI32/SameBoy](https://github.com/LIJI32/SameBoy) | Expat — "all files and directories in this repository, **except for the iOS and HexFiend directories**" | Compatible, with that carve-out. Read for behaviour; reimplement and cite. |
| [mattcurrie/mealybug-tearoom-tests](https://github.com/mattcurrie/mealybug-tearoom-tests) | MIT (`LICENSE`, "Copyright (c) 2018 Matt Currie") | The `src/ppu/*.asm` sources say what each ROM measures — read them before grinding a row. |
| [LIJI32/SameSuite](https://github.com/LIJI32/SameSuite) | see the repository | `apu/README.md` states which CGB revisions pass what, and names the two tests SameBoy fails. Read it first ([apu.md](apu.md)). |

**Two rules that survive whatever the licence says.** Learn the *behaviour*, not the
implementation, and reimplement it here — a borrowed structure that does not fit this
codebase's shape produces the failure recorded on [ppu.md](ppu.md#the-dot-an-object-is-detected-is-still-a-fetcher-dot),
where docboy's own model of the fetcher-during-an-object-hit is **worse in this engine** than a
one-dot approximation of it, because our neighbouring code charges a formula where theirs counts
real time. And **cite by repo, file and symbol**, so a reviewer can check the claim: every
hardware statement taken from one of these is quoted with the function it came from.

> **Check the `LICENSE` yourself and write down what it said.** These entries were verified
> rather than recalled — the same reason [this page corrects the Pan Docs licence](#a-correction-worth-recording-pan-docs-is-cc0-not-cc-by-sa)
> a paragraph above. A wrong belief in either direction costs something: refusing what is
> permitted, or taking what is not.

---

## The pages

Each one follows the same shape: **what the reference says** (cited, linked, not restated) →
**what we measured** → **the test that pins it** → **what a wrong emulator does**.

| Page | What it answers |
|---|---|
| **[Scoreboard](scoreboard.md)** | Every suite, scored today, group by group — and a named, evidenced reason for **every** row still red. Says which of its own numbers are gated and which are not. |
| **[Timing](timing.md)** | Timer, `DIV`, interrupt dispatch, OAM DMA. The four defects that cost 37 Mooneye rows, and the one glitch a single test can see. |
| **[The PPU](ppu.md)** | Two engines and the measured trade between them; the OBJ penalty calibrated against Mooneye's own table; the Color console's PPU steppings; why a pixel count is not a percentage. |
| **[The APU](apu.md)** | Nine calibrated constants, each with the ROM that pins it and the measured cost of moving it one tick. |
| **[Boot state](boot.md)** | Nine consoles, not four. Every byte a boot ROM leaves behind, and where the numbers came from. |
| **[Cartridges](cartridges.md)** | MBC wiring, the real-time clock, save-file formats — and the class of bug that destroys a player's save. |
| **[The link port](serial.md)** | The half of the Game Boy no ROM-driven suite can reach, measured two cores at a time — and what a torn byte carries. |
| **[The Super Game Boy](sgb.md)** | Scope as the interesting question: only the ICD2 command channel is observable, and `MLT_REQ 2` is three players. |
| **[The host boundary](host.md)** | Defects that leave the framebuffer perfect: the clock, the build, the audio device, the terminal. |
| **[Symptoms](symptoms.md)** | Reverse index: *"my emulator does X"* → the cause, the test that catches it, the citation. Plus what **no** suite catches. |

### Where the rest of it lives

This section is the *measured* layer. It sits on top of, and links into, work that already had
a home:

- **[`docs/components/`](../components/README.md)** — one deep-dive page per component:
  hardware background, our implementation with `path:line` references, honest approximations,
  and expected behaviours phrased as future tests.
- **[`docs/testing.md`](../testing.md)** — the *mechanics* of the yardstick: which suites
  exist, how each publishes a verdict, how the harness detects one. This section gives the
  numbers; that page explains how a number is obtained.
- **[`docs/conformance-notes.md`](../conformance-notes.md)** — the raw per-fix corpus,
  newest-first, ~30 entries. Every page here is a distillation of part of it; when a page says
  "measured", that file is where the working is.
- **[`docs/codemap.md`](../codemap.md)** — orientation: a frame through the system, a memory
  access from CPU to device.

---

## Reading the numbers honestly

Five rules, each of which this project learned by getting it wrong first. They are the reason
the numbers on these pages can be trusted.

1. **Say which picture mode.** The PPU has two (`GB_PPU=standard|identical`, formerly
   `fast`/`accurate`) and they score the same ROMs differently. A figure without a mode is
   ambiguous — see [ppu.md](ppu.md) and [picture-modes.md](../picture-modes.md).
2. **Say which population.** "Mooneye acceptance" is 75 rows here, **66** in the community's
   usual denominator, and **66** again in the Shootout's manifest — for two different reasons.
   Comparing across them silently is how a real gain measures as zero.
   ([scoreboard.md § populations](scoreboard.md#three-different-denominators-all-called-mooneye))
3. **A pixel count is a rank distance, not a percentage.** One extra distinct shade re-ranks
   every pixel in the frame and reports ~23,040 out of 23,040. Four wrong pixels can score
   22,070. Never convert it to a percentage.
4. **Never record a hash of your own output as the expected result.** Every expectation in this
   repo is somebody else's: the ROM's printed verdict, its author's reference photograph, or a
   figure from Pan Docs. A harness that judges itself stays green through any regression —
   which ours did, for four ROMs, until it was caught.
   ([symptoms.md § methodology](symptoms.md#methodology-defects--the-harness-was-wrong-not-the-emulator))
5. **Re-measure before quoting.** These scores moved on several consecutive days. Every page
   states the commit it was measured at, and [scoreboard.md](scoreboard.md) carries a
   ten-line script that re-derives all of them from the checked-in baselines in about a second.

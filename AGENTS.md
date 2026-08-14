# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

## Standing convention: every emulation fix feeds the conformance tool

**When you fix an emulation bug, capture it as a conformance-tool test case *at the moment of
the fix*.** Every real accuracy bug we fix here is a ready-made check for the standalone
[emulator conformance tool](docs/conformance.md) we plan to extract — and the *reasoning* behind
the fix is exactly the "why did this fail / what behaviour is missing" explanation that makes
such a tool worth using instead of a bare pass/fail runner. That knowledge is most valuable now
and evaporates afterwards, so write it down in the same change (commit message + the relevant
[`docs/components/`](docs/components/README.md) page, or `docs/audio.md` / `docs/display.md`).

For each emulation-bug fix, record four things:

1. **The observable symptom in user terms** — what a player would *see or hear*, or what a test
   would show, not just the code defect. That is what a foreign emulator with the same bug
   exhibits, so it is how the tool would detect it.
2. **Which test ROM or check catches it** — name it (e.g. Blargg `mem_timing`, dmg-acid2, a
   `dmg_sound` subtest). **If nothing in the suite catches it, say so explicitly** — a coverage
   gap is itself a finding worth writing down and a candidate check to add.
3. **The hardware behaviour being violated, with a citation** (Pan Docs / gbdev / a test-ROM
   readme). This is the explanation the tool hands its users.
4. **Any new test the fix produced** — flag it as a candidate to carry into the standalone tool
   (a reference frame hash, a serial verdict, an audio property, a unit assertion).

*Example (the HALT bug):* **symptom** — some games freeze or mis-time interrupts after a `HALT`
with interrupts disabled and one pending; **caught by** Blargg `halt_bug` (frame-hash pinned in
`testharness/baseline.txt`); **hardware** — `HALT` with `IME=0` and `IE & IF & 0x1F != 0`
re-reads the byte after `HALT` without incrementing `PC` (Pan Docs "Halt bug"); **new test** —
`halt_bug` moved FAIL→PASS in the baseline, a ready conformance check.

**Seed set already carrying this material** (the starting corpus for the tool): the per-access
timer / `mem_timing` fix, the HALT-bug fix, the eight `dmg_sound` accuracy fixes (1/12→9/12), the
HiDPI `cell_scale` fix, and the audio pacing + latency work. Keep it proportionate — a few honest
lines per fix, not a process document.

## Build & test

- **Audio is behind the `audio` cargo feature, OFF by default.** `cpal` is an *optional*
  dependency gated by `audio` (`Cargo.toml`), and the `cpal_audio` module
  (`src/sound.rs`) + the audio-init blocks in **both** frontends —
  `Gameboy::render_desktop` (`src/gameboy.rs`) and `tui::run` (`src/screen/tui.rs`, the
  terminal frontend) — are `#[cfg(feature = "audio")]`; each holds the cpal stream in a
  local `_audio_stream` for the whole run (drop it and sound stops). So
  `cargo build`/`cargo test`/the examples build **with
  no system audio library** — a fresh clone works out of the box. The APU itself
  (`blip_buf`, still non-optional) is always emulated; only host playback is gated. To hear
  sound, build with `--features audio`, which pulls `cpal` → on Linux needs
  `libasound2-dev` / `alsa-lib-devel` (`could not find system library 'alsa'` otherwise).
  CI installs `libasound2-dev` (`.github/workflows/ci.yml`). Chosen default: audio **off**,
  because the whole point was that a fresh clone must not need ALSA; documented in README.
- **Native builds require ALSA dev headers.** `cpal` is a non-optional dependency
  (`Cargo.toml`), so `cargo build`, `cargo test`, the examples, and the FFI build **all**
  fail on a Linux box without `libasound2-dev` / `alsa-lib-devel` — not just the audio path.
  CI installs `libasound2-dev` (`.github/workflows/ci.yml`). A fresh clone on a box without
  it fails with `could not find system library 'alsa'`. Feature-gating audio is a known
  roadmap item.
  - **Local workaround without the `-devel` package** (if only the `libasound.so.2`
    *runtime* lib is present): make a private `alsa.pc` pointing `libdir` at a dir holding a
    `libasound.so -> libasound.so.2` symlink, then `PKG_CONFIG_PATH=<that dir> cargo …`.
    `alsa-sys` only needs pkg-config metadata + the lib to link (no headers). Touches
    nothing in the repo — do not add ALSA to `Cargo.toml`.
- Toolchain is pinned to Rust **1.83.0** via `rust-toolchain`. `make lint` = `cargo fmt
  --check` + `cargo clippy --all-targets -D warnings`.
- WASM build: `cargo build --release --target wasm32-unknown-unknown` (audio is compiled
  out on wasm).

## Documentation

- **Per-component docs live in [`docs/components/`](docs/components/README.md)** — one page
  per component (CPU, PPU, MMU, MBC, APU, Timer, Serial, Joypad, Frontends), each with real
  hardware background, `path:line` implementation references, honest approximations, and a
  numbered list of **expected hardware behaviours phrased as future tests** (marked
  covered/not-covered). These test lists are the spec to implement *before* changing
  emulation code.
- **[`docs/testing.md`](docs/testing.md)** is the accuracy yardstick: Blargg + Mooneye GB +
  dmg-acid2/cgb-acid2, how a headless harness runs each, and how pass/fail is detected
  (Blargg → serial string; Mooneye → `LD B,B` + Fibonacci registers; acid2 → reference PNG).
- **[`docs/debugging.md`](docs/debugging.md)** is the debug-tooling design (inspect /
  manipulate / capture), grounded in a reading of `ioncodes/ayyboy-advance`. Key
  architectural facts it establishes, worth knowing before adding debug features:
  `Cpu::do_cycle()` already executes exactly **one instruction**, so instruction/scanline/
  frame stepping and PC-breakpoints are cheap *driver-loop* work with **zero hot-path cost**;
  decode and execute are **fused** in `Cpu::operation()` so a disassembler must be written
  from scratch (it also feeds Game Boy Doctor logging); the **only** feature needing a
  hot-path hook is memory watchpoints in `mmu::rb/wb`, which must be behind a `debug` cargo
  feature so release builds pay nothing. First thing to build: deterministic frame-hash +
  reference compare wired into the acid2 harness.
- When touching emulation code, keep the relevant component page's "how it works here"
  references and "honest approximations" in sync.

## Sharp edges (documented in the component pages)

- The core is closely modelled on [mvdnes/rboy](https://github.com/mvdnes/rboy).
- **Timer is stepped per CPU memory access** (`MemoryManagementUnit::bus_tick`, gated by
  `bus_tick_enabled` around `Cpu::operation`): each `rb`/`wb` advances the timer one
  M-cycle *before* the access, and `do_cycle` applies only the instruction's remaining
  internal cycles. This makes memory reads/writes land on the correct sub-instruction cycle
  → **Blargg `mem_timing` + `mem_timing-2` PASS**. The **APU clock is per-access too**
  (same hook: `bus_tick` calls `Sound::do_cycle(4)`, remainder at instruction end), which
  is what lets `catch_up()` sync the APU to the exact access cycle → dmg_sound 09/10/12.
  APU *synthesis* stays event-driven and the PPU stays batched, so frame rendering is
  unchanged; per-access APU stepping costs ~25 µs/frame with audio enabled, zero without
  (sound is `None` headless). Timer stepping costs ~15% on a pure-CPU benchmark
  (headless still ~60× realtime). The **HALT bug IS modelled** now (`halt_bug` flag; PC
  rewind after the post-HALT fetch) → **Blargg `halt_bug` PASSES**.
- Known-inaccurate **on purpose (documented non-goals — see component pages)**: the Super
  Game Boy's **SNES side** — border, palettes and SGB sound (`docs/components/sgb.md`; the
  ICD2 command channel a cartridge *can* read back **is** modelled and passes all three
  published SGB rows); PPU mode 3 fixed-length (needs a cycle-stepped PPU — it costs us Mealybug, **3/79** measured and gated,
  see "Gating the EXTERNAL suites" below). *(The "CGB double-speed switch does not take effect" entry that
  used to sit here was a misdiagnosis and is gone — the switch was working; the APU was
  clocked at CPU speed, so a ROM that synchronises through the APU could not see it. See
  the SameSuite APU section.)*
- **The timer, interrupt dispatch and OAM DMA are no longer non-goals** — all three are
  modelled now; see the "Core timing" section below. Two claims that used to live in this
  list are gone for good: the timer divergence did *not* have "no wired test" (Mooneye
  `acceptance/timer/*` wires it, and we were failing 10 of 13 — now 13/13), and OAM DMA
  being instantaneous was not merely a DMA inaccuracy, it made 14 Mooneye *instruction*-
  timing tests unmeasurable. **`interrupt_time` now PASSES** — and it took both halves of
  that day's work: the 5-cycle dispatch fix supplies the correct 13 cycles, the APU
  base-clock fix lets the ROM see the double-speed switch it makes halfway through.
  Neither turns it green alone.
- `get_mbc` (`src/mbc/mod.rs`) returns MBC3 for **any** cartridge when built without a
  filepath — always the case for the web and FFI builds, so non-MBC3 games are mis-driven.
- `src/gpu/old_gpu.rs` (961 lines) is dead/unreferenced; `src/cpu/z80` is a non-compiled
  reference file, not a module.
- The `retrio/gb-test-roms` submodule (`.gitmodules`) holds the Blargg ROMs; it is now
  initialised and wired into CI (see the test-ROM harness below).

## Core timing: timer, interrupt dispatch, OAM DMA — measured against Mooneye

**Mooneye `acceptance`: 13/66 → 52/66**, under *both* criteria — Mooneye's own (`LD B,B` +
Fibonacci registers) and the GBEmulatorShootout's screenshot compare, which used to score
us 6/66. **They now agree exactly**, because with a real DMG mode the boot-register dump
Mooneye prints beside its verdict finally matches the DMG reference images. Before DMG mode
landed this work alone scored 50/66 on registers but only 32/66 on screenshots, and that
whole gap was the dump — not behaviour. Attribution, measured all three ways:

| | register | screenshot |
|---|---:|---:|
| before either | 13/66 | 6/66 |
| DMG mode alone (`origin/main`) | 15/66 | — |
| this work alone | 50/66 | 32/66 |
| **both** | **52/66** | **52/66** |

So of the 39 tests gained, **37 are this work and 2 are DMG mode**
(`bits/unused_hwio-GS`, `boot_regs-dmgABC`); no regressions in either direction. The "+9
from forcing DMG" measured earlier was against the *old* core, where DMG boot rescued
tests that pass on registers but failed the screenshot — that gain is real but it lands in
the screenshot column and overlaps almost entirely with this work.
Full per-fix record with citations: [`docs/conformance-notes.md`](docs/conformance-notes.md).

- **Runner: `cargo run --release --example mooneye_scout -- <mooneye-test-suite>/acceptance`**
  (`examples/mooneye_scout.rs`). Dependency-free, stages each ROM into a temp dir (the MBC
  writes `.sav` next to the ROM), never looks at a reference image. Mooneye is **not**
  vendored — clone [Gekkio/mooneye-test-suite](https://github.com/Gekkio/mooneye-test-suite)
  or use the shootout's `testroms/mooneye/`.
- **Debugging a Mooneye failure: read the screen.** The suite loads its font so
  `tile id == ASCII code` and `print_string` writes char codes straight into the BG tilemap,
  so decoding `$9800` as 20×18 ASCII prints the assertion table (`C: FF!` etc.) — far
  faster than guessing from registers. Same trick as the Gen 1 trainer's `wTileMap` reader.
- **Timer is the hardware circuit now** (`src/mmu/timer.rs`): ONE 16-bit system counter,
  `DIV` = its top byte, a `DIV` write clears all 16 bits, and `TIMA` counts **falling
  edges** of `div[TAP[tac & 3]] AND (tac & 4)` with `TAP = [9,3,5,7]`. Every path that can
  move that expression (clock, `DIV` write, `TAC` write) goes through one `set_div`, so the
  glitches fall out instead of being special-cased. `TIMA` overflow reads `$00` for 4
  T-cycles before `TMA` lands, with the write-cancel / write-on-reload rules. **13/13
  `acceptance/timer/*`.**
- **The one genuinely subtle bit — `TAC` latches BEFORE the counter's increment.** Our
  `bus_tick` applies the M-cycle increment *before* the access, so a `TAC` write arrives
  "late" by one increment. `Timer::wb` therefore re-runs the edge across that increment
  with the newly latched `TAC` (using `prev_div`). Only `timer/rapid_toggle` detects it,
  and only via one enable landing exactly on the tap's own 1→0 boundary; without it the
  test overflows one loop iteration late (`BC = $FFD8` instead of `$FFD9`). **Do not
  "simplify" this away.**
- **Interrupt dispatch is 5 M-cycles laid out as hardware does it** (`Cpu::handleinterrupt`):
  two idle cycles, `PC`-high push, `PC`-low push, vector fetch — all ticking the bus
  (`MemoryManagementUnit::internal_tick`) so the timer keeps phase through dispatch. **The
  vector is decided between the two pushes**, from `IE & IF` as they stand at the start of
  M4: a high-byte push onto `IE` (`SP = $0000`) can cancel the dispatch (`PC = $0000`, no
  `IF` bit acknowledged, `IME` still cleared) or redirect it; the low-byte push's own write
  to `IE` is too late. `acceptance/intr_timing` + `interrupts/ie_push`.
- **`DI` is immediate and cancels a pending `EI`; `EI`'s delay is a latch, not a
  restartable counter.** `0xFB` seeds `setei = 2` *only when it is 0* — otherwise a run of
  `EI`s never enables interrupts at all (`acceptance/ei_sequence`, 18 in a row). `RETI`
  enables `IME` with no delay. The old `setdi` counter is gone.
- **OAM DMA is a timed 160 M-cycle transfer** (`src/mmu/oamdma.rs`): write at M=0, transfer
  starts at M=2, one byte per M-cycle; while it runs CPU reads of OAM return `$FF` and
  writes are dropped. `$FF46` reads back the last page written. A restart lets the previous
  transfer run through M=1. **`OamDma::pos` is advanced at the TOP of `step()`, not after
  the copy** — that is what keeps `active()` true for the CPU access sharing the M-cycle
  with the last byte, which is the single cycle `oam_dma_timing` measures.
- **The lesson worth keeping: an unmodelled peripheral hid a whole dimension of CPU
  accuracy.** 14 Mooneye *instruction-timing* tests measure which M-cycle an access lands
  on by DMAing a known byte into OAM and pointing `SP` there. With an instantaneous DMA
  none of them could pass however correct the CPU was.
- **Two CPU corrections fell out of that** (`src/cpu/stack.rs`): a 16-bit push writes the
  **high** byte first at `SP-1`, and the *internal* M-cycles of `PUSH`/`RST`/`CALL`/`RET cc`
  sit **before** their memory accesses, not as end-of-instruction slack. `CALL` also has to
  fetch its operand before pushing the return address. Conditional `JP`/`JR`/`CALL` fetch
  their operand even when not taken.
- **Cost:** ~6 % on a pure-CPU headless benchmark (0.303 → 0.321 ms/frame, still ~52×
  realtime, 2 % of a 16.74 ms frame). Verified transparent to real games: 2400 frames of
  Pokémon Blue produce byte-identical framebuffers before and after.
- **Save-state `FORMAT_VERSION` was bumped to 4 here** (timer became `div`/`prev_div`/reload latches;
  `Cpu::setdi` removed; `OamDma` added).
- **What is still red is not ours:** 11 PPU tests (mode-3 timing, STAT blocking, LCD-on)
  and 5 boot-state tests.
- **DMG mode flips two of the five boot-state tests**, not five — my earlier estimate was
  optimistic. The other three (`boot_div-dmgABCmgb`, `boot_hwio-dmgABCmgb`,
  `serial/boot_sclk_align-dmgABCmgb`) needed the exact machine state the *boot ROM* leaves
  behind at `PC=$0100`, which is a strictly harder claim than the boot registers.
  **All three pass now** — see "Exact post-boot machine state" below.
- **`blargg/interrupt_time` moves `$0C` → `$0D` and `$0D` is CORRECT** —
  `tests/interrupt_time/interrupt_time.s` says "Should take 13 cycles at either speed". Any
  baseline note blaming "16-vs-20 T-cycle dispatch" is stale; re-bless it. It then still
  said `Failed` because rounds 3 and 4 printed speed `00` after `cpu_fast` — which read as
  "the CGB double-speed switch is not taking effect" and **was wrong about the component**.
  The switch worked; the ROM syncs through the APU (`apu.s` spins on `NR52`) and the APU
  was clocked at CPU speed, so at double speed it ran twice as fast and the sync missed.
  With the APU on the base clock the ROM prints `01` and **`Passed`**. Worth remembering as
  a class: *when a test synchronises through one subsystem to measure another, its failure
  names the wrong one.*
- **Do not read wall-clock timings from a run with the `audio` feature enabled** — that
  slowdown (17.9 ms/frame vs 0.29 ms) was the *player pacing the emulator*, since fixed;
  the ~6 % figure above was measured with no APU installed, so it isolates this change.

## CGB VRAM DMA (HDMA/GDMA) — and "never panic on guest input" (`src/mmu/mod.rs`)

**SameSuite `dma/` 0/4 → 4/4** (suite 50/76 → 54/76). Re-scored on the audit's full
311-row shootout manifest: **187 → 191 PASS, zero regressions**; on the published 264,
**188 → 192**. `testroms` 33/35 matching the baseline, savestate/battery/audiocapture/
lint/lint-audio/audioperf/docs-check/wasm/conformance-runner all green. Full per-fix
record with citations: [`docs/conformance-notes.md`](docs/conformance-notes.md).

- **The rule this belongs to, and it is not optional: cartridge-supplied input must never
  be able to kill the process.** This repo builds `panic = 'abort'`, so a `panic!` is
  process death *and* the loss of any cartridge RAM not yet written — the worst outcome an
  emulator has. It was already established when the MBC divide-by-zero crashes were fixed
  (`src/mbc/battery.rs`); **three more aborts of the same class lived on until now**, two
  of them reproduced from a blank hand-built CGB cartridge. When you touch code that
  `panic!`s on a guest-controlled value, fix it to behave as hardware does, whatever else
  you are doing.
- **HDMA1-4 ARE the address counters, not a shadow of them.** The old code kept a
  `hdma: [u8; 4]` register copy and re-latched `hdma_src`/`hdma_dst` from it on every
  `$FF55` write, so a second transfer repeated the first block instead of continuing
  (`dma/gbc_dma_cont`). The array is gone; `hdma_src`/`hdma_dst` are written directly by
  `$FF51-54`, masked on the way in, and advanced by the transfer. **HDMA1-4 are write-only
  and read back `$FF`** (Pan Docs).
- **The destination is 13 bits and wraps inside VRAM.** `perform_vramdma_row` used to
  increment `hdma_dst` unbounded, so a transfer that reached `$9FFF` walked into
  `Gpu::wb`'s catch-all `panic!` — abort, from nothing worse than a guest pointing a DMA
  near the end of VRAM. It is `0x8000 | (dst & 0x1FFF)` now, which is also exactly what
  `dma/gdma_addr_mask` measures. The out-of-range-source `panic!` is gone too: hardware has
  no such trap, it just reads the bus.
- **An HBlank transfer moves a block when the PPU IS in mode 0, not only on the transition
  into it.** `Gpu::may_hdma()` (the `hblanking` edge) is still the per-scanline trigger, but
  `$FF55` now also arms a one-shot (`hdma_run_block`) if `Gpu::in_hblank()` — which is true
  with the LCD off, where STAT's mode bits read 0 forever. That is the whole of
  `dma/hdma_lcd_off` and `dma/hdma_mode0`: **exactly one block moves and then it stalls**,
  because with the LCD off there is never another mode-0 *entry*. Games do their bulk VRAM
  loading with the LCD off, so before this such a transfer copied **nothing at all**.
- **`Gpu::do_cycle` returns early with the LCD off and so never clears `hblanking`.**
  Turning the LCD off now clears it explicitly — leave it set and a pending HBlank DMA
  moves a block on *every* `do_cycle`.
- **Cancelling with a bit-7-clear `$FF55` write also writes the length register**, so
  `$FF55` reads back `$80`, not `$80 | the blocks that were left`. `dma/hdma_lcd_off`'s
  expected bytes are `$02, $80` and that second one is the only thing that pins it.
- **Undefined opcodes lock the CPU up; they no longer abort.** `$D3 $DB $DD $E3 $E4 $EB
  $EC $ED $F4 $FC $FD` are not instructions — hardware hangs until reset (Pan Docs,
  "Unused opcodes"). `Cpu::exec` returns 1 M-cycle while `stop != 0`, so the PPU/timer/APU
  keep running and the frontend stays alive with `PC` parked on the offending byte. A
  runaway `PC` in any game reaches this. `$FC` had been special-cased as a `NOP`, which is
  neither hardware's behaviour nor its ten siblings'; it locks up now too.
- **Save-state `FORMAT_VERSION` is now 6** (the four HDMA1-4 bytes removed, `hdma_run_block`
  added).
- **No public suite catches an emulator aborting on legal-but-odd guest input** — the same
  gap the battery work recorded. That is why the four `mmu::vramdma_tests` crash/behaviour
  cases and `cpu::core::lockup_tests` are Rust tests, not test-ROM rows.
- **SameSuite is not in CI** (audit item ⑪). Re-run by hand:
  `cargo run --release --example samesuite -- <samesuite-testroms-dir>`.
- **Pre-existing flake, not ours:** `screen::tui::tests::the_stream_survives_ten_on_off_cycles`
  counts *process-global* file descriptors, so it fails intermittently when the rest of
  `cargo test --lib` opens files in parallel. Reproduced on unmodified `main` (1 of 2 full
  runs failed; it passes in isolation on both `main` and this branch).


## `STOP`, the CGB speed switch, and undefined opcodes (`src/cpu/core.rs`)

- **`STOP` is the whole Pan Docs decision chart now** (`Cpu::stop_instruction`), not
  "call `switch_speed()` and carry on". Four inputs select the behaviour: a **selected
  button already held** (STOP does not stop, `DIV` is not reset, HALT is entered
  instead), a **speed switch requested** via `KEY1` bit 0, an **interrupt pending**
  (`IE & IF != 0`, which makes `STOP` a **1-byte** opcode so the `$00` after it executes),
  and otherwise **STOP mode**. Source: Pan Docs *"Using the STOP Instruction"* + the
  decision chart (Lior Halphon) at `src/imgs/stop_diagram.svg` in the pandocs repo.
- **STOP mode is not HALT — an interrupt does not end it**, only one of `P10`-`P13` going
  low does. It is therefore tested at the top of `Cpu::exec`, *before* interrupt dispatch.
- **The speed switch is `SPEED_SWITCH_HALT_TICKS` = `0x20000` T-cycles of the NEW clock**,
  spent in HALT, with `DIV` reset on the way in. Switching into double speed that is
  65536 cycles of the PPU's unchanged 4.194304 MHz clock = **143.7 scanlines**, i.e. the
  display collapse Pan Docs warns about. `0x20000` is also exactly two wraps of the 16-bit
  system counter, which is why `DIV` reads ~0 the moment the CPU resumes
  (`daid/speed_switch_timing_div`). SameBoy uses a measured `0x20008`; on this core the
  ROM passes for anything in `0x1FFF4..=0x20000`, so the eight cycles are below our
  resolution — **do not "correct" the constant to make `_ly`/`_stat` pass** (see below).
- **`speed_switch_timing_ly` and `_stat` are still red, and it is NOT a STOP bug.** A
  duration sweep gives three incompatible optima (`div` ≤ `0x20000`, `stat` ≈ `0x20030`,
  `ly` ≈ `0x20036`): `div` measures the switch alone (the counter is reset by it) while
  `ly`/`stat` measure the switch *plus* where the PPU already was, and the two disagree by
  ~21 dots. That is a pre-existing PPU-vs-counter phase error, almost certainly the phase
  the PPU starts at when `LCDC` bit 7 is set — the thing Mooneye's `ppu/lcdon_timing-GS`
  measures, also red. Those two rows belong with the STAT/LCD-on work, not here.
- **A stopped machine's screen** (Pan Docs, same page): DMG → blank/white, Color → black,
  **except** if the PPU is in mode 3 when the CPU stops, where it keeps drawing. That is
  one rule, not two: STOP takes the PPU's VRAM access away *unless it already holds it*,
  and mode 3 is when it holds it. Implemented as `Gpu::stop_fill: Option<u8>` used by
  `renderscan`. **The PPU keeps counting while stopped even on a DMG (where hardware
  stops it outright)** — deliberately, so a frame-paced frontend still gets frames instead
  of spinning forever; the stopped CPU cannot observe the difference.
- **Writing `0` to `KEY1` withdraws the request** — the old `wb` only ever set the flag.
- **Save-state `FORMAT_VERSION` is 8** (`Cpu::speed_switch_halt`, `Gpu::stop_fill`; the
  VRAM-DMA work took it to 6 and the APU corner cases to 7).
- Score: **daid 1/9 → 5/9** — four rows (`stop_instr` DMG+CGB,
  `stop_instr_gbc_mode3`, `speed_switch_timing_div`); `rom_and_ram` is the fifth and was
  already green. Measured against `main` at `c10da12` with the shootout's own ROMs and
  reference images: Mooneye 53/75, SameSuite 54/76, Mealybug and
  `testharness/baseline.txt` **byte-identical** before and after, and 1200 frames of
  Pokémon Blue produce a byte-identical framebuffer. Full record:
  [`docs/conformance-notes.md`](docs/conformance-notes.md).
- **Nothing in the in-repo harness executes a `STOP` at all** — Blargg, Mooneye and acid2
  between them never do. The daid suite is the only public source of these checks and it
  is not in CI. That gap is a finding, not an aside.

## PPU: Mealybug Tearoom and the mid-scanline question (`src/gpu/`)

- **Full scoping, with the split of what is winnable now versus what needs the rewrite:
  [`docs/mealybug.md`](docs/mealybug.md).** Score today **1 of 31 ROMs** pixel-exact against
  all three hardware reference sets (was 0). Measure it with
  `cargo run --release --example mealybug -- --dir <mealybug checkout>` — ROMs and reference
  screenshots are **not vendored**, they come from a run-time path like `POKERED_GFX` does.
- **Score each column on the machine its reference was photographed on.** These cartridges have
  no CGB flag, so `Model::Auto` gives a DMG — right for `DMG-blob`, wrong for `CPU CGB C`/`CPU
  CGB D`, which are a Color console running the same DMG cartridge in compat mode.
  `examples/mealybug` runs both and compares each column to its own frame. They render
  identically *today* (`GbMode::Classic` and `ColorAsClassic` share the renderer) — that must
  stop being true once mode 3 is modelled, which is exactly why the suite ships three sets.
- **Three independent blockers stood between us and the remaining rows, and all three must be
  true at once for ANY row to pass. One is now closed:**
  1. **Whole-line rendering.** `renderscan` runs once at the mode 3 → 0 transition. Open; this
     is the structural one.
  2. ~~Interrupt dispatch 16 vs 20 T-cycles~~ — **closed** by the core-timing work. *Measured*
     on both sides: tracing every `BGP` write on line 1 of `m3_bgp_change` gave dots
     76/92/104/164/176/236/248 before and **80/96/108/168/180/240/252 now**, which is exactly
     what the ROM's own cycle comments intend. **Mid-mode-3 writes now land on the right dot to
     the cycle** — the substrate a fetcher-accurate PPU needs is already correct.
  3. **VRAM starts zeroed; hardware hands over the boot ROM's logo tiles + the `®` at tile
     `$19`.** 18 of the 31 ROMs depend on it (12 write no tile data of their own at all), so
     their frames are wrong before timing is considered. **Do this before the FIFO**, or every
     failure after the rewrite has two candidate causes. Shipping the `®` glyph is a
     licence/policy call — flagged, not taken.
- **Two real PPU bugs that only a Mealybug ROM caught** (both fixed, both with unit tests in
  `src/gpu/mod.rs` proven to fail when reverted):
  - **The window "Y condition" must latch independent of LCDC bit 5.** Pan Docs, "Window
    rendering criteria": at the start of each scanline, `WY == LY` latches it for the frame;
    nothing about the enable bit. We tested `win_on` first, so a game that sets `WY` while the
    window is off and enables it later in the frame **never showed the window at all**.
  - **The OAM strobe reaches LY=144.** A program with only the mode-2 select bit still takes
    a STAT interrupt at `LY = 144`; missing it loses one STAT interrupt per frame and puts
    everything the handler toggles on the wrong parity of scanlines. Flips
    `mooneye/acceptance/ppu/vblank_stat_intr-GS` FAIL → PASS (**acceptance 52 → 53**, PPU reds
    11 → 10). **A PPU test ROM caught an interrupt bug** — an argument for scoring screens, not
    only serial verdicts. *(This used to be recorded as TCAGBD §8.7's "mode 1 honours the
    mode-2 bit". That framing is wrong for the rest of VBlank and cost `intr_1_2_timing-GS` —
    see the STAT-line section below.)*
- **Calibration before anyone promises a number:** on the shootout's 24 published Mealybug
  rows, SameBoy scores 15, **bgb scores 2**, PyBoy 0. This is not a suite a competent
  cycle-stepped PPU sweeps.
- **After editing `src/gpu/mod.rs`, re-anchor `docs/components/ppu.md` by content.**
  `make docs-links` reported "rewrote 0 references" for a change that shifted every line after
  295 by +9 and after 535 by +35 — `docs-check` only catches out-of-range rot. Fix the
  `path:line` spans by hand, then re-run `make docs-check`.

## The STAT interrupt is ONE LINE, not four triggers (`src/gpu/mod.rs`)

**Mooneye `acceptance` 53 → 55/75; shootout published rows 192 → 194/264 (full suite
194 → 196/311), zero regressions, measured before and after through the same manifest
against `main` at `c10da12`.**
Full record with citations: [`docs/conformance-notes.md`](docs/conformance-notes.md).

- **`stat_signal` ORs the enabled sources; `refresh_stat_line` raises `IF.1` on the 0→1
  transition ONLY.** Every path that can move a term must call it: a mode change, an `LY`
  increment, and writes to `STAT`, `LYC` and `LCDC`. `do_cycle` also re-samples once per
  chunk, because the line-144 OAM strobe expires on a plain dot count with no event of its
  own. This is "STAT blocking" and it **affects real games** — a raster split driven off
  both the LYC and the HBlank source ran twice as fast when we fired per source.
- **`STAT` bit 2 is a LATCHED flip-flop (`lyc_match`), not `line == lyc` computed on read.**
  With the LCD off the comparison clock stops: the flag freezes, a write to `LYC` is
  invisible, and enabling the LCD re-latches it against the `LY` the PPU restarts from.
  `acceptance/ppu/stat_lyc_onoff` reads `STAT` back after every step to check exactly this.
- **Switching the LCD off must NOT force the STAT line low.** The coincidence flag keeps
  feeding the OR gate, so off-then-on with the flag already set produces no edge and no
  interrupt. Forcing it low manufactures one and fails `stat_lyc_onoff` round 2. The *mode*
  terms are gated on `lcd_on` — a deliberate choice, not a hardware reading: `STAT` really
  does read mode 0 with the LCD off, but an ungated `m0_inte` would fire every time a game
  blanks the screen, and no test covers it. Documented at `stat_signal`.
- **The line-144 OAM strobe is line 144 ONLY** (`oam_strobe`), not "mode 1 honours the
  mode-2 bit". Held across all of VBlank it keeps the line high through lines 145-153, which
  *blocks* the real mode 2 at `LY = 0` and delivers its interrupt a scanline late —
  `intr_1_2_timing-GS` counts precisely that and reads `$31` instead of `$14`/`$15`.
  **This is a two-sided constraint: both the presence and the narrowness are wired.**
- **The line the PPU restarts on has no OAM scan** (`skip_first_oam`): `STAT` reads mode 0
  through the OAM slot and goes straight to mode 3.
- **`lcdon_timing-GS` and `lcdon_write_timing-GS` are NOT separable from the cycle-stepped
  PPU** — this was attempted and the audit's separability call was an inference that turned
  out wrong for half the item. They sample OAM/VRAM **accessibility** at 1 M-cycle
  resolution over three scanlines, so they need (a) mode-based VRAM/OAM access blocking and
  (b) the PPU stepped per bus access. **Do not add (a) alone**: with mode 3 fixed at 172
  dots where hardware varies it 172-289, blocking would silently drop writes a game placed
  legally in HBlank — strictly worse than not blocking. Schedule both with the FIFO rewrite.
- **Save-state `FORMAT_VERSION` is now 6** (`stat_line`, `lyc_match`, `skip_first_oam`).
- **Debugging tool worth knowing: `GB_SCOUT_SCREEN=1 mooneye_scout <rom>`** prints the ROM's
  own assertion table by decoding the BG tilemap as ASCII. `stat_lyc_onoff` says
  `Fail: r1 step 3` and `lcdon_timing-GS` prints the cycle, expected and actual byte — the
  Fibonacci register dump says none of that. **And the Mooneye sources are the real
  documentation**: `git clone --depth 1 https://github.com/Gekkio/mooneye-test-suite` and
  read the `.s` file. Every behaviour above was derived from the assembly plus its expected
  table, not guessed from the test's name.

## The DMG OAM-corruption bug (`src/gpu/oambug.rs`) — a defect we had to ADD

- **`blargg/oam_bug@dmg` 3/8 sub-tests → 8/8 PASS**; the in-repo suite went **33/35 →
  34/35 rows**. It is no longer a non-goal. Full reasoning + the conformance corpus entry:
  [`docs/conformance-notes.md`](docs/conformance-notes.md) (newest entry).
- **`oam_bug@cgb` MUST STAY RED.** The bug is DMG/MGB/SGB silicon only — Pan Docs says CGB
  and Advance are unaffected *even running monochrome software* — so a real CGB fails this
  ROM too. `Gpu::oam_bug` returns early unless `gbmode == GbMode::Classic`, which
  deliberately excludes `ColorAsClassic` (that is CGB hardware in compat mode). The
  baseline header says all this so nobody "fixes" the row.
- **The cause is the ADDRESS BUS, not a memory access.** The CPU's 16-bit
  increment/decrement unit is wired to the address bus and asserts its operand even with
  no read or write behind it. So `inc de` with `DE = $FE00` corrupts OAM and `add hl,bc`
  with the same value does not. Hooking only `rb`/`wb` gets `2-causes` and `3-non_causes`
  wrong — the IDU hook (`MemoryManagementUnit::idu_tick`, called from `cpu::data::idu`,
  `push_rr`, `call`, `rst`) is the load-bearing half.
- **`share_idu(address)` is how the "read and IDU in the same M-cycle" pattern is spelled**
  (`ld a,[hl±]`, and a `pop`'s *first* read only — Pan Docs records `pop` as triggering
  three times, not four). It takes the address so it can only ever flag an access that is
  itself in OAM, which is what stops a stale flag being consumed by a later access. A
  *write* sharing its M-cycle with an IDU is just a write, so `wb` never consults it.
- **`OAM_BUG_PHASE = -12` is calibrated, not chosen.** The PPU is stepped once per
  instruction, so which OAM row a mid-instruction bus cycle hits is *predicted*
  arithmetically (`Gpu::oam_scan_row_in`) from the free-running dot counter. The one
  systematic error is that enabling the LCD mid-instruction over-advances the batched PPU
  by the ticks that instruction had already spent; this constant absorbs it. **Measured:
  −16 and −8 both fail `4-scanline_timing`, at opposite ends of the window; only the
  4-dot band containing −12 passes.** It assumes the 3-M-cycle `ldh [rLCDC],a` every test
  ROM and essentially every game uses — a 4-M-cycle `ld [$FF40],a` would land one row off.
- **Cost: ~1.5 % on the test-ROM suite** (2.78 s → 2.82 s, alternating A/B, 3 rounds).
  The hot path gained one `address & 0xFF00 == 0xFE00` test in `rb`/`wb` and a range test
  on the IDU value; everything else is behind those cold branches.
- **No save-state format bump.** `idu_share` is set and consumed inside one instruction,
  so it is never live at a frame boundary and is deliberately not serialised.
- Verified no regression anywhere: `--lib` 589/589, savestate, battery 8/8, audiocapture
  (`dmg_sound` 12/12, all hashes unchanged), dmg-acid2 0/23,040 px, and the external
  suites byte-identical before/after (Mooneye acceptance 55/75, SameSuite 61/76, Mealybug
  3 — measured on this branch's parent and again with the change).

## Super Game Boy (`src/sgb.rs`) — the ICD2, and nothing else

- **Full page: [`docs/components/sgb.md`](docs/components/sgb.md); conformance record in
  `docs/conformance-notes.md`.** `Model::Sgb` / `Model::Sgb2` (`GB_MODEL=sgb|sgb2`) →
  `GbMode::Super`/`Super2`. `GbMode::is_dmg()` is now what every "this is DMG hardware"
  branch asks (PPU OAM-bug, undocumented CGB registers, boot counter, `P1` boot state, `STOP`
  fill) — Classic **and** both SGBs, never `ColorAsClassic`.
- **The audit called this "a new subsystem" and that was wrong.** All three published
  shootout SGB references are plain 160×144 DMG-grey screens: no border, no palette, no SNES
  side. Every one of the three tests measures the packet receiver on `$FF00` plus `MLT_REQ`,
  because that is the **only** part of an SGB a Game Boy program can read back. ~300 lines.
- **Border, SGB palettes and SGB sound are a documented non-goal**, and the reason is the
  argument above, not laziness: they are one-way traffic to a screen the cartridge cannot
  see. If they are ever wanted they are a *display* feature.
- **`Model::Auto` must never pick an SGB.** The cartridge SGB flag (`$0146 == 0x03`) says the
  game *offers* a border, not that the player owns the peripheral; auto-booting one would
  change every SGB-enhanced game's boot registers (Pokémon R/B included) for no visible gain.
  The suite runners select it the way the shootout's own manifest does: `samesuite.rs` from
  the header flag, `mooneye_scout.rs` from the `-sgb`/`-sgb2` filename suffix.
- **The four protocol rules the tests pin down** (prose docs are ambiguous on all four):
  `$00` arms and aborts; a bare `$00` **not** followed by `$30` starts nothing; a bit is
  latched on the write of `$30` and its value is **the last line held low** (`$20 → $10 → $30`
  is a `1`); the command runs on the **last data bit**, not the stop bit, so a corrupt stop
  cannot cancel it.
- **`MLT_REQ 2` is THREE players, not four.** `player_count = (param & 3) + 1` with no
  correction → mask 2 → the index alternates between players 1 and **3**. SameBoy's
  `if (player_count == 3) player_count++;` is exactly why **SameBoy fails
  `samesuite/sgb/command_mlt_req`**. Only GSE passes all three SGB rows among the 18 published
  emulators. Do not "fix" our version to match SameBoy.
- **The index advances on the rising edge of P15 and on nothing else** (so `$10 → $30` moves
  it, `$20 → $30` does not), masked with `player_count - 1` every step; and **applying
  `MLT_REQ` advances it `player_count` more times** — invisible for 1/2/4 players, and the
  entire point for the illegal 3.
- **The model was derived, not guessed.** 59 ground-truth constraints (24 + 8 expected bytes
  from the two SameSuite ROMs' own `CorrectResults` tables, 27 player counts decoded out of
  `sgb-ext-test`'s reference PNG) and the rules above are the only model that satisfies all
  59. If you change any rule here, re-check it against all three ROMs, not one.
- `savestate::FORMAT_VERSION` is **12**: a length-prefixed, present-flagged SGB block sits
  between the joypad and the PPU. Plugin-style skippable, so an SGB state loads on a DMG build.

## Boot state: the DMG boot ROM leaves a tilemap behind

- **`MemoryManagementUnit::set_boot_logo_tilemap`** writes `$01`-`$0C` to `$9904`-`$990F`,
  `$0D`-`$18` to `$9924`-`$992F` and `$19` (®) to `$9910`, for every DMG-class machine,
  **before** `LCDC` is enabled so the write cannot land in a blocked PPU mode.
- **Why it matters far more than it sounds.** A ROM that draws on part of the screen and
  never clears the tilemap renders its *own* tiles in rows 8 and 9 on hardware. We rendered
  blanks. Under a shade-rank image compare that is a **whole-screen** mismatch, because one
  shade fewer on screen re-ranks every white pixel. Both SameSuite `sgb/` ROMs reported their
  own verdict as PASS while failing the shootout's screenshot compare for this reason alone.
- The tile **pixels** are still not written (on hardware they are the logo decompressed out of
  the cartridge header, and every ROM has replaced them by the time anything looks). Pinned by
  `the_boot_rom_leaves_its_logo_in_the_tilemap`. **Two verdict methods disagreeing is the
  finding** — if a ROM's own report and an image compare differ, do not pick the flattering one.

## Test-ROM correctness harness

- **Runner:** `testharness/testroms.rs`, a `harness = false` Cargo integration test
  (`[[test]] name = "testroms"`). Run it with `cargo test --release --test testroms`. It
  lives in `testharness/` (not `tests/`) because **`tests/` is the gb-test-roms submodule** —
  Cargo cannot track `.rs` files inside a submodule path.
- **NEVER record a hash of our own output as the expected result.** That was the harness's
  central defect: `mem_timing-2`, `interrupt_time`, `halt_bug` and `oam_bug` were pinned as
  FNV hashes of *our* framebuffer, and the documented response to a changed hash is to
  re-bless it — so CI would have stayed green straight through any regression in those four,
  and `oam_bug` (really **3 of 8 sub-tests**) was recorded as an opaque number nobody could
  read. Every expectation is now somebody else's: the ROM's serial string, the text the ROM
  writes to cartridge RAM, or its author's reference image.
- **Three mechanisms, all judged by the ROM or its author:**
  1. *Serial* — Blargg's ROMs write "Passed"/"Failed" to the link port
     (`Serial::set_callback`), capped at 250M cycles.
  2. *Verdict* — the ROMs that never touch the link port still print their result. Prefer
     cartridge RAM (`$A000` status byte behind the `$DE $B0 $61` signature, text from
     `$A004`); **`halt_bug` and `interrupt_time` have no cartridge RAM at all** (type 0x02,
     RAM size 0x00), so the **screen** is read instead. Blargg's font is loaded so a tile id
     **is** the ASCII code of the glyph it draws and characters go straight into the BG
     tilemap, so the tilemap reads back as text — the same trick the trainer harness uses on
     Pokémon's `wTileMap`. That is how `oam_bug` reports `01:ok 02:02 …` and `interrupt_time`
     reports `Failed` instead of a hash.
  3. *Image* — dmg-acid2 is compared against `third_party/dmg-acid2/reference-dmg.png` by
     **shade rank**, not RGB (`testharness/pngdec.rs`, same rule as
     `tools/conformance/image.py`), because our shades are `000000/606060/c0c0c0/ffffff` and
     the reference's are `000000/555555/aaaaaa/ffffff`. Result: **0 of 23,040 pixels differ**
     — the claim is now CI-enforced instead of manually spot-checked.
- **The harness runs with an APU installed.** It used to leave `MMU.sound` at `None`, which is
  not a real machine and silently changed results: `interrupt_time` stalled on its title
  screen (it spins on `NR52`) and was written off as "cannot run headless", and
  `dmg_sound/01` reported Failed. Never build a test machine without an APU. **Measured cost:
  +0.02 ms/frame (1.1x), whole suite 2.9 s** — do not confuse this with the ~17.9 ms/frame
  the **`audio` cargo feature** costs (the ~5 fps the captain sees). Different path: `cargo
  test` does not compile `cpal` or `sample_queue`, the harness' `play()` is a no-op, and the
  producer-side high-watermark throttle that paces emulation against the audio clock is
  `#[cfg(feature = "audio")]`. If the suite's wall-clock ever jumps, re-measure the APU
  install here — do not remove it, the verdicts depend on it.
- **Every ROM runs on BOTH consoles** (`<id>@dmg`, `<id>@cgb`), skipping `@dmg` for a
  cartridge whose header says `0x0143 == 0xC0`. This is also the answer to "retrio's Blargg
  copies are patched to `0x143 = 0x80`, so we test them as CGB games": forcing the model runs
  both machines without mutating or vendoring a ROM.
- **Baseline:** `testharness/baseline.txt` records the expected verdict of every ROM/model row
  **and the ROM's own report text** — so a diff is human-readable and a change in any
  sub-test, register dump or checksum line is caught, not just a flipped PASS/FAIL. CI is red
  on any change in either direction. Update deliberately with
  `UPDATE_BASELINE=1 cargo test --release --test testroms`. Full explanation in
  `docs/testing.md`.
- **The harness self-checks that it can fail**, at startup: it decodes the reference image,
  requires it to be 160×144 with >1 shade, requires it to compare equal to itself, and
  requires a single flipped pixel to be *detected*. Both mechanisms were also proved by
  deliberately breaking the emulator — clearing the HALT-bug latch turns `halt_bug` red, and
  ignoring sprite X-flip turns dmg-acid2 red at 119/23,040 pixels. Re-run those two if you
  ever doubt the harness.
- **Sharp edges learned:** (1) always drive test ROMs via `Cpu::new(data, Some(path))` — with
  `None`, `get_mbc` forces MBC3 and mis-drives them. (2) The MBC writes a `.sav` next to
  the ROM path, so the harness **copies each ROM into `target/` first** to keep the submodule
  checkout clean. (3) Emulation is deterministic (WRAM seeded with a fixed value), so a frame
  reproduces exactly — the harness asserts this at startup.
- **Vendored ROM:** `third_party/dmg-acid2/` holds the prebuilt MIT acid2 ROM + reference PNG
  (upstream ships source only); `.gitignore`'s `**/*.gb` rule is negated for that one file.
- No new crate dependencies: the hash, the PNG writer and the PNG **reader** (a full RFC 1951
  inflater — the upstream reference is 2-bit greyscale) are hand-rolled.
- `src/cpu/z80` is a non-compiled reference file, not a module. (`src/gpu/old_gpu.rs`,
  961 lines of dead/unreferenced code, was deleted — no `mod old_gpu` ever existed.)
- The `retrio/gb-test-roms` submodule (`.gitmodules`) is declared but uninitialised.

## Gating the EXTERNAL suites — Mooneye / SameSuite / Mealybug (`make test-external`)

- **The problem this solves: 266 rows that CI never looked at.** Mooneye GB, SameSuite and
  the Mealybug Tearoom tests were scored only when somebody ran a scout by hand. In the
  twenty-four hours before this landed those suites moved **+96 rows**, and every one of them
  could have been lost again to an unrelated commit with nobody the wiser. `make test-external`
  fetches all three at pinned versions and scores **266 rows in ~1.5 s**; CI job
  `external-suites` in `.github/workflows/ci.yml`.
- **Fetch at run time, NEVER vendor — and pin in exactly one place.** These are other people's
  ROMs under other people's licences, the same reason `conformance.yml` downloads Blargg's.
  Every pin lives in `tools/fetch-test-suites.sh` and nowhere else, so a laptop, a CI job and a
  published score measure identical bytes: Mooneye from Gekkio's own prebuilt archive
  (sha256-checked), Mealybug from the author's repo at a pinned commit (his prebuilt zip **and**
  his hardware-photographed reference PNGs), SameSuite from the copies vendored by
  GBEmulatorShootout at a pinned commit — LIJI32 ships source only, and that is also the exact
  corpus our published 264-row score is measured on. Bumping a pin will move baselines; the two
  must land in the same commit.
- **Baselines are `testharness/{mooneye,mooneye_cgb,samesuite,mealybug}_baseline.txt`**, under
  `testharness/baseline.txt`'s rules: every value is the ROM's **own** judgement (Mooneye's
  Fibonacci registers, SameSuite's `RESULT_CODE` at `$CFFE`, Mealybug's author-photographed
  frame), never a hash of our output; **any change is red in either direction** — a row that
  starts *passing* is a baseline change too; re-bless deliberately with
  `UPDATE_EXTERNAL_BASELINE=1 make test-external`. Shared gating code:
  `examples/support/suite_baseline.rs`, included by every scout with `#[path]`.
- **A fifth group: the shootout's `cpp`** (`examples/cpp_suite.rs`,
  `testharness/cpp_baseline.txt`, **4/4**) — CasualPokePlayer's three MBC3/RTC tests and
  `sgb-ext-test`, judged against *his* reference screenshots by shade rank. Four published
  shootout rows that nothing was watching; the same shootout sparse checkout that brings
  SameSuite brings them, so it costs no extra fetch.
- **The Mealybug baseline pins the PIXEL COUNT, not just PASS/FAIL.** At 3/79 a PASS/FAIL file
  is a flat wall that cannot show a cycle-stepped-PPU rewrite getting closer, so each row
  records *how many of the 23040 pixels differ*. Consequence the PPU crew must expect: **any**
  PPU change turns that file red. Read the diff, re-bless with the new counts in the same
  commit. That is the point — progress and regression both become visible from commit one.
- **Measured on `main` @ `b51ce9f`, 2026-08-14** (measure, never estimate): Mooneye
  `acceptance` **58/75** (**58/66** on the community denominator, which drops nine `-S`/`-sgb`/
  `-sgb2`/`-mgb`/`-dmg0` ROMs for machines we do not model — pinned FAIL on purpose),
  `emulator-only` **28/28**, `misc` (`GB_MODEL=cgb`) **4/8**, SameSuite **61/76**, Mealybug
  **3/79** (DMG 1/24, CGB-C 1/31, CGB-D 1/24). These are **above** the suite-audit report's
  figures (53/66, 50/76) because `main` advanced past the commit it measured — always
  re-measure rather than copying a report's numbers into a baseline.
- **A missing baseline file is not a silent pass**, and a ROM present in the baseline but
  absent from the run is a failure too (it means the fetched suite is not the pinned one).
- **Teeth proven, not assumed:** a deliberate one-line timer break (`TAP[3] 7→6`) turns the
  Mooneye gate red, and a one-line BG-scroll break moves 26 Mealybug rows. Re-run those two if
  you ever doubt the gate. **Sharp edge that cost real time here: the scouts are `cargo build`
  targets, so after breaking source for a teeth test you MUST rebuild before trusting any
  later measurement** — a stale binary reported `timer 11/13` and looked exactly like a
  regression on `main`.
- **`cgb_sound` is wired into `testharness/testroms.rs`** (12 CGB-only cartridges, `0x0143 ==
  0xC0`, so one row each, all green) — the in-repo harness is **46/47 rows**, ~3.6 s. The one
  red is `oam_bug@cgb` and it must stay red: Color silicon has no OAM-corruption defect, so a
  real CGB fails that ROM too.
- **Wiring the suite immediately found a save-destroying bug — that is the argument for doing
  it.** Three Mooneye ROMs that **pass** (`add_sp_e_timing`, `ld_hl_sp_e_timing`,
  `oam_dma_start`) execute code from OAM on purpose to time an access onto a known byte.
  `Cpu::operation`'s runaway-PC heuristic covered `$FE00-$FF7F`, so it called that a crash and
  `report_fault` **poisoned the battery — a game legitimately running a routine out of OAM
  silently stopped saving for the rest of the session.** OAM is real RAM the CPU may read
  outside PPU modes 2 and 3 (Pan Docs, "VRAM/OAM accessibility"). The window is now
  **`$FEA0-$FF7F`** — the unusable block and I/O, which genuinely cannot hold code. A runaway
  PC that walks *into* OAM still faults: it keeps walking and reaches `$FEA0`. Pinned by
  `running_from_oam_is_not_a_fault` beside the existing `running_from_hram_is_not_a_fault`, and
  `testharness/battery.rs`'s crash cartridge now ends with `JP $FEA0` instead of spinning in
  OAM (that test's premise — "PC in OAM means crashed" — was the same wrong belief).

## Exact post-boot machine state (`MemoryManagementUnit::set_initial`, `Sound::set_post_boot_state`)

We do not run a boot ROM, so **every byte of the state one leaves at `PC=$0100` has to be
written by hand**, and most of it was not being written at all. Mooneye
`acceptance` **55/75 → 58/75** and `misc` (`GB_MODEL=cgb`) **2/8 → 4/8**, measured on the
same build before and after: `boot_div-dmgABCmgb`, `boot_hwio-dmgABCmgb`,
`serial/boot_sclk_align-dmgABCmgb`, `misc/boot_div-cgbABCDE` and `misc/boot_hwio-C` all
FAIL → PASS. Full write-up with citations: `docs/conformance-notes.md`.

- **It is per console, and Mooneye's own tables are the authority** (they were verified on
  real DMG/MGB and CGB units): `acceptance/boot_hwio-dmgABCmgb.s` and `misc/boot_hwio-C.s`
  hold the whole `$FF00-$FF7F` map, transcribed into `mmu::boot_state_tests`. The two
  consoles genuinely disagree — a DMG leaves both joypad select lines pulled low
  (`P1 = $CF`), a CGB leaves them released (`$FF`). Both leave `IF = $E1` (the boot ROM's
  VBlank is still pending). Note `boot_hwio-C`'s cartridge is **DMG**-flagged, so what it
  measures is `ColorAsClassic`, which is why `VBK`/`BCPS`/`OCPS` read back (`$FE`/`$C8`/`$D0`)
  while the palette *data* ports stay closed.
- **`set_initial`'s APU block was dead code** and had been since audio became optional:
  `self.sound` is `None` at construction, so all twenty-two `NR*` writes were dropped and
  the chip installed afterwards started from zeroed defaults (`NR52` read `$70` — powered
  **off**). It now lives in `Sound::set_post_boot_state`, applied from `enable_audio` and
  only on a freshly created chip (re-attaching a player mid-run must not rewind it).
- **NEVER write the read-back values into `NR11`/`NR14`.** They read `$BF`, but that is
  `$80` and `$87` ORed with their write-only bits. Writing `$BF` loads a length counter of
  **1 with the length counter enabled**, so channel 1 switches itself off within 4 ms and
  `NR52` reads `$F0`; `boot_hwio` reads `NR52` ~2.8 ms in and requires `$F1`. Replay the
  writes the boot ROM actually makes (`NR52=$80, NR11=$80, NR12=$F3, NR51=$F3, NR50=$77,
  NR13=$C1, NR14=$87`) — that is the only formulation that gets both the read-back and the
  channel-enable flip-flop right, and it falls out for free that `NR10`/`NR2x`/`NR3x`/`NR4x`
  are never written at all (their read-back masks supply every expected bit).
- **The boot beep is deliberately NOT reproduced.** On hardware the second "ding" is still
  ringing at hand-off, but its envelope volume is not readable from any register, so how far
  it has decayed cannot be derived — and we never played it. The channel-enable flip-flop is
  set (as hardware has it); the envelope is left at zero.
- **The system counter does not start at zero.** `$ABCC` (DMG) / `$2678` (Color), as a read
  observes it on the first M-cycle. `boot_div` is much stronger than "`DIV` reads `$AB`": it
  reads `DIV` six times at offsets chosen to fall either side of an increment, so the
  counter's **low bits** are pinned too. `Timer::set_boot_counter` stores the value 4 T lower
  because `bus_tick` advances the counter *before* the access that observes it — the
  constant in the source is the hardware number and that adjustment happens in one place.
- **A link transfer is timed now, and an unanswered one still completes.** Eight falling
  edges of system-counter bit 8 (8192 Hz, ~1 ms/byte), fed to `Serial::shift_clock` by
  `Timer::take_serial_edges`; then `SB` takes the byte, `SC` bit 7 clears and `IF` bit 3 is
  set. With nothing attached the byte is `$FF` — the console drives the clock, so the
  transfer runs regardless; dropping it was what made `boot_sclk_align` print "No serial
  intr". The callback/plugin is still offered the byte at the `SC` write (so the Blargg
  capture and the Gen 1 partner are unchanged); only *when it lands* moved.
- **The one-M-cycle subtlety, and only one ROM can see it:** the serial edge must be taken
  from `Timer::observed_counter()` (`div + 4`), not from `div`. Use `div` and the interrupt
  arrives one instruction late — `boot_sclk_align` reports `C: 90!` with `$91`. Verified by
  making that change deliberately. **No unit test can catch it**: every other measurement of
  the serial clock is relative to an `SC` write, and a constant phase offset cancels; only
  alignment against a boot-time counter exposes it.
- **Save-state `FORMAT_VERSION` bumped to 10** (the shift register's `bits_left`/`incoming`).
- **Cost:** `render_bench --frames 600` on `cpu_instrs`, three runs each, **0.339 → 0.359
  ms/frame** (+6 % of a pure-emulation figure, 0.1 % of the 16.743 ms budget, ~46× real
  time). About half is the serial edge test in `Timer::do_mcycle`; about half is channel 1
  genuinely running from boot.
- **One baseline moved — `testharness/audio_baseline.txt`'s `tone440`, and only that line**
  (hand-edited, not `UPDATE_AUDIO_BASELINE=1`, so nothing else could move silently).
  `testharness/baseline.txt` is untouched and matches exactly. Cause: with channel 1 enabled
  at hand-off, the tone ROM's own `NR14` write is a *re*-trigger and takes the shorter
  `SQUARE_RESTART_SPEEDUP` path. Confirmed by comparing the WAVs, not by assertion — RMS
  `0.2467` and peak `0.267` identical to four decimals, every zero crossing displaced by the
  same `−0.047` samples (one M-cycle). The printed "dominant" moved 440.0 → 438.0 Hz only
  because that estimator counts crossings in a fixed 0.5 s window and the shift drops one.
  `dmg_sound` hashes unchanged, verdict still 12/12. **This is the shape of evidence to
  demand before any audio hash is re-blessed.**
- **Still red and not this work:** the `-S`/`-sgb`/`-sgb2`/`-mgb`/`-dmg0`/`-A`/`-cgb0`
  model variants (other consoles we do not emulate — 9 rows in `acceptance`, 2 in `misc`)
  and the PPU mid-scanline tests.

## Console model — DMG vs CGB (`src/mode.rs`, `Cpu::new_with_model`)

- **There are three machines, not two.** `GbMode::Classic` (a DMG), `GbMode::Color` (a CGB
  running a CGB cartridge) and `GbMode::ColorAsClassic` (**CGB hardware in DMG-compat mode**).
  The last two are what this emulator has always booted: `Cpu::new` called
  `MemoryManagementUnit::new_cgb` unconditionally, so a DMG-only cartridge got compat mode.
  `MemoryManagementUnit::new` (Classic) existed but **nothing in the product ever called it**.
- **`Model { Auto, Dmg, Cgb }`** is the user-facing choice, resolved to a `GbMode` by
  `Model::resolve(cgb_flag)`. `Cpu::new` = `Model::Auto` = "CGB flag bit 7 set → Color, else
  DMG", i.e. what a person with both consoles would do. `Cpu::new_with_model` forces one.
  `$GB_MODEL=auto|dmg|cgb` overrides `Auto` only (an explicit caller choice always wins), so
  every frontend honours it without growing a flag; a typo warns and is ignored rather than
  silently switching console. `Model::Dmg` is honoured even for a `0x143 == 0xC0` cartridge —
  forcing means forcing — whereas `MemoryManagementUnit::new` still refuses one.
- **Classic and ColorAsClassic render identically** (`Gpu::new_cgb() == Gpu::new()`; the
  Color-only branches in `renderscan` test `== GbMode::Color`). The differences are the boot
  registers (`A=0x01 HL=0x014D` vs `A=0x11 HL=0x007C`) and the undocumented CGB registers
  `FF72`/`FF73`/`FF75`-`FF77`, which `Classic` reads as `0xFF`. That is enough for nine
  Mooneye acceptance tests, which print their boot registers, to change verdict — measured
  **+9 with zero regressions**. It is also a hard prerequisite for being listed in the
  [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/), whose runner passes a model.
- **Cost: one branch at boot.** Nothing on any hot path; `set_mode` replaced `determine_mode`
  and all three constructors now share `MemoryManagementUnit::raw`, so the DMG and CGB paths
  cannot drift apart.
- **Auto-detect changes existing behaviour for DMG-only cartridges**, which is the point but
  is worth knowing when a baseline moves: Blargg's `dmg_sound` ROMs (`0x143 == 0x00`) now boot
  a DMG, and the audio baseline's hashes shifted by 256 samples because the boot state differs.
  The verdicts stayed 12/12 and every analysed property (RMS, peak, silence, dominant pitch)
  held. **`GB_MODEL=cgb` reproduces the pre-change hashes exactly** — that is the cheap way to
  prove a baseline move is the console and not a synthesis regression, and it is the first
  thing to try before re-blessing any moved baseline. Written down where the next person
  looks: `docs/audio.md` §8, `docs/testing.md`, `testharness/README.md` and the generated
  header of `testharness/audio_baseline.txt`. `savestate` needed no format bump (`gbmode` was
  already serialised).

## Audio quality & pacing

- **Full audio breakdown: [`docs/audio.md`](docs/audio.md)** — the signal path from APU
  synthesis → `blip_buf` resample → `cpal` queue → speaker, plus the "rough & fast"
  investigation and fixes. Complements the register-level [`docs/components/apu.md`](docs/components/apu.md).
- **Pitch is decoupled from device rate.** `blip.set_rates(4_194_304, sample_rate)` means the
  APU emits exactly `sample_rate` samples per *emulated* second; `Sound::set_player` retunes
  blip to whatever the player reports. So pitch is correct for any rate **iff the reported
  rate equals the rate the stream truly runs at**. A mismatch is the only thing that makes a
  *tone* sharp/flat; verified by `src/sound.rs` `mod tests` (`tone_pitch_is_correct`,
  `output_matches_reported_rate` — device-free, run under plain `cargo test`).
- **"Fast" was the frame pacing, not the audio.** The DMG runs at 70224 cyc / 4.194304 MHz =
  **59.7275 Hz = 16.7427 ms/frame**. `render_desktop` paced at **16 ms (62.5 fps, +4.64 %)`,
  so the emulator — and thus APU sample production — ran 4.64 % fast: the queue overflowed and
  dropped samples (rough) while the game ran fast (tempo). Fixed to `from_micros(16_743)`.
- **"Blocky and broken" was the pacing again, plus queue policy.** Both frontends reset their
  frame timer with `last = Instant::now()` *after* stepping — the period silently included
  draw/poll overhead, so the emulator ran a few % **slow**, the audio queue drained, and each
  underrun stepped to 0.0 (a click) then played the next 45 ms block: machine-gun gaps. Both
  loops now use a **deadline accumulator** (`next += FRAME_TIME`, bounded catch-up) — keep it
  that way; never reset a pacing timer to `now()`. The queue is `sample_queue::SampleQueue`
  (`src/sound.rs`): prime before draining, hold-and-fade + re-prime on underrun, and above
  the high watermark **discard the oldest samples down to `start_fill()` — never wait**
  (the producer runs inside `Cpu::do_cycle`; see the "producer must never block" entry
  below). Sizes come from the simulation in `mod queue_tests` + real soaks
  (`examples/audio_soak.rs`, `GB_AUDIO_STATS=1`).
- **"Sounds play just AFTER they should" — latency. Measure the WHOLE path, not our queue.**
  Killing the stutter by holding more samples bought delay: measured **299 ms end-to-end**.
  `examples/audio_latency.rs` reports queue latency, device latency and underruns from one
  real-device run — always judge them together, they trade off. Three things mattered, in
  order of size:
  1. **The cpal device buffer was never pinned.** `BufferSize::Default` gave 1882 frames/callback
     and **99.5 ms** of host latency. `BufferSize::Fixed(256)` → **5.9 ms**. cpal reports this
     directly: `info.timestamp().playback.duration_since(&callback)`. Always measure it.
  2. **`OUTPUT_SAMPLE_COUNT` is the floor under all audio latency.** At 2000 (45 ms/block) no
     queue can be small. Now **256** (5.8 ms) — shorter than a frame, which is the real
     production granularity. `play_samples` reuses `Sound::mixdown` scratch (no per-block allocs).
  3. **The clip target sets steady-state latency, and it is `start_fill()`.** Opening a device
     always overfills the queue, so wherever the latency bound leaves it is where it stays.
     Now 1792/2816 samples (41/64 ms): `start_fill` is the cushion and the clip target,
     `high_water` is only the ceiling. Sized to a stated promise: *absorb one dropped frame*
     (a dropped frame costs the frame **plus** the normal 16.7 ms period, since the queue
     drains continuously). (Historical note: this used to read "`high_water()`, not
     `start_fill()`" because the producer *blocked* at the watermark. It no longer does —
     that was the 6 fps bug.)
  Result, 5-min real soak: **52.7 ms end-to-end (46–60), 0 underruns, 0 drops, drift −0.0005 %**.
  Measured stall reality on this box: 4 of 17899 frames >1 ms late, p99.9 = 0.0 ms — so
  `queue_tests`' profile is derived from that, not from an assumed 90 ms stall.
- **AN `AudioPlayer` MUST NEVER BLOCK — it runs inside the emulated frame.** `Sound::play_samples`
  is called from `Cpu::do_cycle`, so anything the player does is charged to `emulate_ms`. The
  cpal player used to *wait* at the queue's high watermark to make the sound card the master
  clock. Result, measured on the captain's session: `emulate_ms` **17.93** against a 16.743 ms
  budget with the no-audio build at **0.29** — and **97.6 % of that was blocked, not
  executing** (`examples/audio_frame_cost.rs` prints wall time beside
  `CLOCK_THREAD_CPUTIME_ID`; if they diverge, emulation is not what is slow). It was
  misdiagnosed four times as the T-cycle-stepped APU, which in fact costs ~0.16 ms/frame.
  Two compounding effects, both worth remembering: the wait's equilibrium is **slower than
  real time** (1 ms poll granularity, resumes only at/below the watermark → 57.85 fps
  unpaced), and the frontends pace emulation themselves, so a frame over budget puts the loop
  into catch-up, which fills the queue faster, which blocks every frame — pinned at
  `MAX_CATCHUP` = 12 emulated frames per draw, i.e. 59.73/12 ≈ **5 fps**, with no escape.
  **Never put a second clock inside the emulator's inner loop.** The hand-off is now the whole
  of `sample_queue::hand_off`: push, then bound latency by *discarding*, which is the policy
  fast-forward already documented ("drop samples rather than pitch-shift"), so
  `set_fast_forward` needs no special case. After: `emulate_ms` **0.34** with a real device,
  soak **PASS** (0 underruns, 0 drops, 59.7264 Hz), end-to-end latency **44.8 → 36.3 ms**,
  `dmg_sound` still **12/12**, all audio hashes unchanged. Gated by
  `testharness/audioperf.rs` (`make test-audio-perf`, in CI) — needs the `audio` feature but
  **no sound card**: it attaches the real producer half with no consumer so the queue
  saturates, and fails above 4 ms/frame or below 4x real time. Teeth proven: 29.6 ms and
  0.57x against the old policy. Full write-up + conformance record: `docs/audio.md` §4c.
- **A frame-paced loop ran the machine unbounded while the LCD was off — FIXED** by
  `FRAME_TICKS_CAP` in `Gameboy::frame` (see its own section below). `Gpu::do_cycle` returns
  immediately when `!lcd_on`, so `updated` never fires and the loop had no other exit;
  measured on Pokémon Blue's boot, one "frame" ran **845,836** cycles instead of 70,224 and
  the APU emitted the matching burst, which the queue's latency bound then discarded. It was a
  **frontend pacing** defect, not an audio one, and it was the only remaining source of
  playback discards. It was never to be "fixed" in `sound.rs`, and it was not.
- **Small blocks broke the consumer-stall detector (fixed; don't reintroduce).** It compared
  the callback counter against the previous `play()`. Fine at 45 ms/block, but at 5.8 ms one
  frame pushes several blocks back-to-back with no callback between them, so every block after
  the first was misread as "consumer stalled" and **dropped**: 330 ms of healthy audio per 90 s.
  Judge a stall by **elapsed time** (50 ms with no callback), never by counter equality.
- **cpal negotiation gotcha:** `device.supported_output_configs()` often lists the first `F32`
  range as `ch=1, min=1, max=384000` (mono!). Never break on the first F32 range. Prefer
  `device.default_output_config()` when it's F32 (real channel count + a rate the HW actually
  runs); else scan all F32 ranges preferring span-44100 then max channels. `examples/audio_probe.rs`
  dumps a machine's configs + negotiated rate (`cargo run --features audio --example audio_probe`).
- **Offline audio harness:** `cargo test --release --test audiocapture`
  (`testharness/audiocapture.rs`) captures the APU's sample stream device-free, writes WAVs to
  `target/testrom-artifacts/audio/`, checks rate/pitch/level/continuity (built-in 440 Hz tone
  ROM), reads Blargg `dmg_sound`'s memory verdict at `$A000` (**12/12 pass**, and the
  expected verdict is pinned per fixture so a pass→fail regression is named, not just a
  hash change), and pins an FNV hash in
  `testharness/audio_baseline.txt` (re-bless: `UPDATE_AUDIO_BASELINE=1`). Single-ROM TDD
  mode: `AUDIO_ROM=<path> [AUDIO_FRAMES=n]` runs one ROM and prints its verdict. Any change
  to `src/sound.rs` synthesis will move these hashes — listen to the WAVs before re-blessing.
  **Gotcha: single-ROM mode runs the ROM in place** (no `target/` staging like the fixture
  path), so pointing it at `tests/…` drops `.gbsave` files into the submodule checkout —
  delete them (`/usr/bin/find tests -name '*.gbsave' -delete`) before committing.
- **APU accuracy model (dmg_sound 1/12 → 9/12 → 12/12) — things to preserve:** register read-back is
  `regs[i] | NR_READBACK_MASKS[i]` (raw-byte shadow; NRx1 length loads mask duty `v & 0x3F`);
  the MMU calls `Sound::catch_up()` before every APU register access (lazy ~45 ms blocks
  otherwise leave NR52/length/sweep stale — this is what makes all the timing subtests pass);
  length expiry clears the channel-enable flip-flop; NRx4 extra-length-clock quirks live in
  `apply_nrx4_length`; sweep state fully reloads on trigger + negate-exit disable quirk;
  NR52 power-off zeroes NR10–NR51 *while still on* (clearing after `on=false` is a no-op),
  DMG preserves length counters across power and allows NRx1 length writes while off, and
  power-on resets the frame sequencer to step 0 (DIV divider keeps running — but see the
  SameSuite section below: with the DIV-APU bit already set, the first event is *skipped*);
  a powered-off APU still emits silence at full sample rate (never stall the stream). Save-state format is
  v4 (v2 added APU shadow regs + flags; v3 added `WaveChannel::since_fetch`; v4 added the
  double-speed clock divider, the duty latch, the 1 MHz grid phase and the DIV-APU skip).
- **DMG wave-RAM window (dmg_sound 9/12 → 12/12) — things to preserve:** the APU clock is
  stepped per memory access (see the timer sharp-edge above) — without it no wave-window
  constant can be right, since `catch_up()` would sync to instruction boundaries only.
  `WaveChannel::since_fetch` (T-cycles since the channel's last wave-RAM byte fetch, valid
  at every catch-up point) drives `dmg_access_byte`: while CH3 plays, DMG reads/writes of
  `$FF30-$FF3F` hit `waveram[wave_pos/2]` iff `since_fetch <= 2`, else read `$FF` / are
  dropped; CGB always hits the current byte. Trigger reloads the fetch timer with
  `period + WAVE_TRIGGER_DELAY(=4)`; NR34-trigger with `delay == 0` (fetch imminent this
  cycle) corrupts wave RAM (byte 0 or aligned 4-byte block). Constants are calibrated by
  dmg_sound 09's 2-cycle phase sweep — a ±2 change flips all three CRCs. `since_fetch` is
  serialised (format v3); `sound::tests::wave_window_state_survives_save_load` fails if it
  is dropped (teeth proven by dropping it deliberately). Conformance-tool notes for these
  fixes: `docs/conformance-notes.md`.
- **The sweep shadow reload takes three M-cycles, and that window is a distinct state**
  (`SWEEP_RESTART_HOLD = 12` T, `src/sound.rs`; SameSuite `channel_1_sweep_restart_2`
  FAIL → PASS, 68/76 → 69/76, APU half 63/69 → 64/69, measured on `a5eb0af` with the CH4
  work already in). A CH1 trigger reloads the sweep unit's shadow register from
  the frequency registers and for 12 T afterwards the **overflow check reads that shadow as
  zero**. Two consequences that must both stay: a 128 Hz sweep event landing inside the
  window latches no addend, and the delayed check it arms compares `0 + 0`, so the channel
  survives an overflow that would kill it a cycle later. The addend is latched when the
  check is **armed**, not when it fires (`sweep_addend`) — an NR10 shift written in between
  does not change it. Both fields are live machine state (`savestate::FORMAT_VERSION` 12 →
  13). Pinned by `sound::tests::a_sweep_event_inside_the_restart_hold_cannot_kill_the_channel`,
  teeth proven by removing the hold.
- **`channel_1_sweep_restart`'s last red cell is NOT a sweep bug — do not "fix" it in the
  sweep unit.** 143 of its 144 cells match; the one that differs is round 1's last subtest,
  where the first duty step after a **re-trigger** lands at 66 T and hardware's is at ≥ 69.
  That is `SQUARE_TRIGGER_DELAY` / `SQUARE_RESTART_SPEEDUP`, both already pinned by rows
  that pass. **Measured, so nobody re-runs it:** `SPEEDUP = 0` (what round 1 wants) →
  62 → **58** (loses `channel_{1,2}_restart`, `channel_{1,2}_restart_nrx2_glitch`);
  `TRIGGER_DELAY = 5` → 56; `= 7` → 62 (grid rounding absorbs it); `= 8` → **39**. A real
  fix needs a *conditional* re-trigger reload rule and the suite carries no evidence for the
  condition. `docs/conformance-notes.md` §15.
- **PLAYBACK VOLUME IS NOT NR50, AND MUST NEVER BECOME IT** (`sound::output_volume`,
  `docs/audio.md` §6b). The obvious way to make the speakers quieter is to turn down the
  Game Boy's own master volume — and it is wrong: NR50 is *emulated behaviour* the accuracy
  suites measure (Blargg `dmg_sound`, SameSuite), a game reads it back from `$FF24`, it is in
  save states, and it is baked into `testharness/audio_baseline.txt`. The attenuation is
  therefore applied **past the end of emulation**, in the cpal device callback
  (`cpal_audio::cpal_thread`), to samples the APU has already handed over. That buys three
  things worth keeping: emulation is bit-identical at every level (no baseline, no suite and
  no capture harness can see it — `cargo test` does not even compile `cpal`), the **frame
  path is untouched** (`play_samples` runs inside `Cpu::do_cycle`, where a blocking hand-off
  once cost 17.9 ms/frame — the volume costs two relaxed atomic loads *per device callback*,
  on the audio thread), and a change is heard immediately rather than after the ~40 ms already
  queued. Scale in **`f32` before the device-format conversion**; attenuating an integer
  sample throws away its low bits and is exactly where "crunchy when quiet" comes from.
  Taper is `(level/100)²` (100 % = unity = the pre-control output), default **50 %** because
  full scale was the complaint. Verified end-to-end by recording the sink monitor with
  `parec` while `examples/audio_volume.rs` walks the ladder: measured ratios match the taper
  to 3 dp, crest factor and 3rd-harmonic ratio are **constant** across the ladder (the
  waveform is scaled, not degraded), mute is digital silence. `emulate_ms` with a real device
  0.236 → 0.235 (unchanged); dmg_sound 12/12 and SameSuite 61/76 both sides.
- **The level is a process-wide global on purpose** — the cpal callback reads it from cpal's
  own thread, the same reason `host_status` is one. Consequence for tests: anything that moves
  it must take `output_volume::test_lock()`, and `App::build` deliberately does **not** apply
  it (the render tests construct `build` in parallel; the production path applies it in
  `App::with_warnings`). The TUI tests also point `$XDG_STATE_HOME` at a scratch dir
  (`isolate_state_dir`) — a `cargo test` run must not rewrite the user's saved volume.
- **The remembered volume carries the config value it was adjusted from**
  (`VolumeState.from_config`, `${XDG_STATE_HOME}/gameboy/volume`). Without that fingerprint one
  of the two intents is always silently ignored: either the live keys never persist, or an
  edited `volume =` in `config.toml` does nothing. The rule is "config unchanged → the level
  you last set wins; config edited → the config wins".
- **Audio knobs for re-measuring (and for A/B by ear):** `GB_AUDIO_DEVICE_FRAMES` (0 = host
  default), `GB_AUDIO_START_FILL`, `GB_AUDIO_HIGH_WATER`, `GB_AUDIO_STATS=1`. The old,
  laggy configuration is `GB_AUDIO_DEVICE_FRAMES=0 GB_AUDIO_START_FILL=7000
  GB_AUDIO_HIGH_WATER=12000`.
- **Next structural step for latency:** production is still bursty at frame granularity —
  a frame emits 16.7 ms of audio in one go, and that burst dominates the cushion. Stepping
  emulation in sub-frame chunks would roughly halve the required cushion again (towards
  ~30 ms end-to-end). That changes the frontends, not the audio code.


## SameSuite APU accuracy (2/61 → 49/61) — `src/sound.rs`

**Full write-up: [`docs/components/apu.md`](docs/components/apu.md) §8; per-fix conformance
records in [`docs/conformance-notes.md`](docs/conformance-notes.md).** Read those before
touching APU timing — every constant below is *measured* against a named test, not chosen.

- **Run the suite:** `cargo run --release --example samesuite -- <dir> [--dump] [--frames N]`.
  It reads SameSuite's own `RESULT_CODE` at `$CFFE` (`"P"`/`"F"`) — the ROM's verdict, not a
  screenshot. `--dump` prints the measurement table for debugging against the test's `.asm`.
  ROMs: `LIJI32/SameSuite` (source only) or the prebuilt copies in `gbdev/GBEmulatorShootout`
  under `testroms/samesuite/`. The 8 `*-cgb0B`/`-cgbB`/`-cgb0BC`/`-A` variants encode *older*
  CGB revisions and are deliberately not chased (we target CGB-E) — quote the whole-directory
  number (76 ROMs) and say which of the failures are revision-locked, rather than quietly
  scoring against a smaller denominator. See the corner-cases section below.
- **The APU only runs when a player is installed.** `Sound::do_cycle` returns immediately
  when `player.is_none()`, so a harness with no `enable_audio` call has *no APU at all* —
  not a quiet one. Any APU measurement without a player is measuring nothing.
- **PCM12/PCM34 (`$FF76`/`$FF77`) are the whole interface.** 57 of the 61 tests are
  unreachable without them. The MMU routes them to `Sound::rb` with a `catch_up()` first,
  exactly like NRxx. Low nibble CH1/CH3, high nibble CH2/CH4; zero when the channel or its
  DAC is off.
- **The frame sequencer is DIV-driven.** Falling edge of system-counter bit 12 (bit 13 at
  double speed) — so `Sound::div_write()` (called from the MMU's `$FF04` path, after
  `catch_up`) fires an event when the bit was set, and resets the phase. `div_apu_bit_set()`
  derives the bit from `next_time - time` rather than keeping a counter copy; that identity
  only holds because `div_write` resets both. **`Timer::wb($FF04)` must zero `internaldiv`
  as well as `divider`** — leaving the sub-DIV counter running desynchronises everything
  clocked off the same counter and made the whole `div_*` group non-deterministic.
- **Power-on while the DIV-APU bit is set skips the first event** (`frame_seq_skip`), and a
  pending skip counts as `first_half()` for the NRx4 extra-length-clock quirk. Both halves
  are needed: without the skip `div_trigger_volume_10` fails, without the `first_half`
  clause `div_write_trigger_10` does.
- **The APU runs off the base clock.** `Sound::set_double_speed` (from
  `MemoryManagementUnit::switch_speed`) halves the CPU T-cycles it is handed. Before this,
  CGB double-speed games played an octave high *and* — because the audio queue paces the
  emulator — ran at about half speed: Blargg `cpu_instrs` measured **33.7 fps / 48% of
  frames late before, 59.70 fps / 0% after**.
- **`grid_phase`** tracks the APU's position on its free-running 1 MHz (4 T) grid, kept
  separately from `time` because `do_output` rebases `time` by `output_period` (24344 T,
  not a multiple of 4). **APU power-on resets it** — that is what makes SameSuite's
  measurements repeatable, since every subtest power-cycles the APU first.
- **Measured trigger constants — do not "tidy" these:** `SQUARE_TRIGGER_DELAY = 6` T
  (`channel_N_delay`), `SQUARE_TRIGGER_ALIGN = 0` (`channel_N_align`, double speed),
  `SQUARE_RESTART_SPEEDUP = 4` T (`channel_N_restart` — a re-trigger is one tick shorter and
  does *not* reset the duty position), `WAVE_TRIGGER_DELAY = 4` T (`channel_3_delay` **and**
  Blargg `dmg_sound` 09/10/12 — this one is doubly constrained), `NOISE_TRIGGER_DELAY = 2`
  with `NOISE_TRIGGER_ALIGN = 0`.
- **Square duty output is a LATCH** (`duty_out`), not `DUTY_PATTERN[duty][duty_pos]`. It
  updates only at a frequency-timer expiry, so a freshly powered channel reads 0 for its
  whole first position. Power-off resets `duty_pos` *and* `duty_out`. The duty table is the
  canonical Pan Docs one — ours used to be rotated, which sounds identical and is measurable.
- **CH3 advances its position before fetching**, so a triggered wave channel starts at
  sample **1**, never 0; and its `sample_latch` empties on power-off (a trigger does not
  refill it). The stale latch is what `channel_3_first_sample` actually measures.
- **CH4's trigger reloads only the NR43 divisor**, not the shift stage above it:
  `delay = divisor + (period - divisor)/2`. Getting this right fixed all four LFSR tests at
  once, because they reconstruct the LFSR from 128 consecutive samples and rotate wholesale
  if the first sample is off.
- **NRx2 does not reload the live volume** (`VolumeEnvelope::write_nrx2`) — only a trigger
  does. Writing it while the channel plays is "zombie mode", and the rule is **CGB revision
  E's**, derived from `channel_N_volume`'s twelve old/new direction+period combinations. The
  gg8-wiki DMG formula does not fit and must not be substituted.
- **Cost and audio quality re-measured after the work** (both required, both green):
  APU cost `32.6 µs/frame` vs `31.4 µs` before — inside the noise of the measurement
  (`cargo run --release --example apu_bench -- <rom> [frames]`, best-of-3 with/without a
  player). Real-device latency, re-measured after rebasing onto the audio crew's
  player-pacing fix (which is what moved the absolute numbers, not this work): three
  consecutive 25 s runs on a single-speed ROM give **28.0–39.4 ms end to end, 0 underruns,
  59.686 fps**, and `main` measured the same way in the same session gives 28.0–41.2 ms
  with the identical run-for-run drop/held pattern (down to the same 2373 held and 4809
  dropped samples). **Unchanged by the APU work** — which is the claim that matters here;
  the absolute figure now belongs to the player, not to us.
- **`testharness/audio_baseline.txt` was re-blessed** for this work. Judge it by the
  properties, not the hash: sample counts identical (so the rate did not move), tone440 RMS
  identical at 0.2467, and its measured pitch *improved* from 438.0 Hz to 440.0 Hz against a
  ROM programming 439.84 Hz. dmg_sound still reads `01:ok..12:ok Passed`.

## SameSuite APU corner cases (50/76 → 57/76) — sweep, envelope, noise restart

**Per-fix conformance records: [`docs/conformance-notes.md`](docs/conformance-notes.md)
§10-13; the failing list with reasons is [`docs/components/apu.md`](docs/components/apu.md) §8.**
Measured before and after with
`cargo run --release --example samesuite -- <shootout>/testroms/samesuite` over the whole
76-ROM directory. **On `main` after this merged the number reads 61/76**, because the VRAM
DMA crew's `dma/*` fix (4 rows) landed alongside it; 50 → 57 is this work in isolation,
measured against the base it started from. The APU half is **56/69** either way. Nothing else moved: Mooneye `acceptance` 53/75, Mealybug 3/79,
`cargo test --release --test testroms` 33/35 "All results match the baseline", savestate
and battery harnesses green, and the audio-perf gate is inside its own noise
(0.265-0.270 ms/frame before, 0.266-0.278 after, three runs each).

- **A sweep's overflow check happens `shift + 1` M-cycles AFTER it writes the new
  frequency, not with it** (`SquareChannel::sweep_calc_delay`, `sweep_calc_delay_for`).
  `channel_1_sweep`'s own source states the mechanism: *"8 cycles after trigger, the APU
  checks if the NEXT trigger overflows the frequency."* A trigger's own calculation starts
  one M-cycle later still. Clearing NR10's shift field **parks** an in-flight calculation
  rather than cancelling it — the channel that was about to die keeps playing
  (`channel_1_sweep_restart` round 3), which is why `sweep_calc_unshifted` exists: a
  calculation armed with shift 0 must not be parked by that same rule.
  `SquareChannel::run` splits its window at the pending check so the channel goes quiet on
  the exact cycle. **The old note blaming these on "a 4 T race, timer territory" was
  wrong** — it is APU behaviour and it is now modelled.
- **The volume envelope is TWO things, and separating them is the whole fix**: a 3-bit
  period counter clocked at 64 Hz, and a *pending-step latch* (`VolumeEnvelope::clock`)
  set when that counter reads zero and acted on at the next **512 Hz** DIV-APU event.
  The counter is free-running — it reloads from NRx2 when it reads zero rather than
  stopping — so an envelope parked at period 0 resumes the instant a non-zero period is
  written, with no trigger. That is what every real music driver's mid-note volume ramp
  depends on, and it is what `channel_N_nrx2_speed_change` and the long half of
  `channel_N_nrx2_glitch` measure. `VolumeEnvelope::step` is gone; `div_event` is called
  for CH1/CH2/CH4 on **every** frame-sequencer step.
- **The envelope's period counter runs on frame-sequencer step 6, not 7.** Length (even
  steps) and sweep (step 2 of 4) were already one step ahead of the event that acts on
  them; the envelope was not, and that inconsistency is exactly what an extra 512 Hz of
  latency looked like. With the count on 6 the *volume change* still lands on step 7 where
  `channel_N_volume_div` measures it — putting it back on 7 costs six rows
  (`channel_{1,2,4}_volume_div`, `div_trigger_volume_10`, `div_write_trigger_volume{,_10}`)
  and reddens `sound::tests::a_triggered_envelope_still_steps_on_the_64hz_boundary`.
- **Re-triggering a *running* CH4 costs one whole extra sample** (`+ self.period` in
  `NoiseChannel::trigger` when it was already enabled). The divider that clocks the LFSR is
  free-running: a trigger reloads the countdown to its next tick, not the divider.
  `channel_4_lfsr_restart` / `_fast` reconstruct 128 consecutive outputs, so a one-sample
  slip shifts the whole sequence; removing this drops the suite 57 → 55.
- **Five of the remaining failures are ROMs we should NOT pass.** *(Superseded — all
  five pass now; see the CH4/CGB-revision section below. The hardware reading here is
  right, the conclusion was not: the revision is selectable per ROM.)*
  `channel_{1,2,4}_extra_length_clocking-cgb0B`, `channel_3_extra_length_clocking-cgb0` and
  `-cgbB` assert the length behaviour of **CPU CGB revisions ≤ B**, which the ROM source
  itself calls a bug that *"breaks at least one game (Prehistorik Man)"* and that was fixed
  in later revisions. We model CGB-E. `-cgb0` and `-cgbB` also contradict each other, so no
  single machine passes both. Likewise only one of the three
  `channel_1_freq_change_timing-{A,cgb0BC,cgbDE}` variants is winnable at all.
- **The three remaining CH4 failures have ONE cause, and it is scoped not attempted**:
  *(Superseded — two of the three are green; the cause was the two-stage counter, as
  described here. See below.)* the
  noise frequency comes from a free-running 14-bit counter clocked every `(NR43 & 7) * 8`
  T-cycles (4 if zero), with the LFSR stepping on the 0→1 edge of bit `NR43 >> 4`. We model
  a per-channel period timer reloaded at trigger — right rate, wrong phase, which is
  precisely what `channel_4_equivalent_frequencies` exists to catch. The counter is four
  lines; what blocks it is the value the *trigger* reloads its countdown with, a table of
  revision- and instance-specific quirks SameBoy's own source annotates "this quirk seems
  way too specific" and "non-deterministic". Do not attempt it without a way to calibrate.
- **`testharness/audio_baseline.txt` was re-blessed again** — deliberately, and only the
  `dmg_sound` hash. Judge it by the properties, which are byte-identical before and after:
  1,625,088 samples, RMS 0.0265, peak 0.283, 96.8 % silent, dominant 480 Hz, and the ROM's
  own verdict still `01:ok..12:ok Passed`. `testharness/baseline.txt` was **not** touched.
- **`savestate::FORMAT_VERSION` is now 7** (`SquareChannel::sweep_calc_delay` +
  `sweep_calc_unshifted`, `VolumeEnvelope::clock`); v6 is the VRAM-DMA crew's bump, which
  landed on `main` first.
- **SameBoy's `Core/apu.c` was read as a hardware reference, not copied** — its
  `envelope_clock`/`volume_countdown` split and its free-running noise counter are what
  named the two structures above. Everything here is reimplemented in this codebase's
  idiom, per the licence-hygiene rule in `CONTRIBUTING.md`.

## CH4 noise timing + CGB CPU revisions (SameSuite 61/76 → 68/76)

**Per-fix conformance records: [`docs/conformance-notes.md`](docs/conformance-notes.md)
§14-16.** Third pass over item ② of the suite audit. Measured with
`make test-external` (the CI gate) before and after, on the ROMs pinned in
`tools/fetch-test-suites.sh`: SameSuite **61/76 → 68/76**, its APU half
**56/69 → 63/69**. Mooneye 86/103 and 4/8, Mealybug 3 pixel-exact, `testroms`
46/47, `audiocapture` (dmg_sound 12/12 + every hash), `savestate`, `battery` and
672 library tests all **byte-identical before and after**.

- **Do NOT restate this as a published-shootout gain.** Only two of the seven rows
  (`channel_4_equivalent_frequencies`, `channel_4_frequency_alignment`) are in the GB
  Emulator Shootout's 311-row manifest at all — the five `extra_length_clocking-*` ROMs
  are not in that corpus. The published-264 delta is **at most +2** and is
  screenshot-judged, which this work did not measure. Check
  `gameboy-shootout-verify-k5/manifest.json` before quoting a 264 number for any
  SameSuite row; the CI-gated 76-row count and the published 264 are different
  populations.
- **A CH4 trigger reloads NEITHER frequency counter — it halves both.** The noise
  frequency is NR43's divisor stage feeding a `>> shift` stage, and a trigger leaves
  both mid-count: the first LFSR step lands at `(period - divisor)/2 + divisor/2`, not
  at a whole period. The one exception is that **the divisor stage cannot be left holding
  less than one of its own ticks** (16 T-cycles, or 8 for code 0, itself a half tick), so
  codes 0 and 1 are floored and code 3 is not. That floor alone is why the four encodings
  of one 16 M-cycle sample (`$0C`/`$1A`/`$29`/`$38`) start 0/0/+2/+1 M-cycles apart —
  `channel_4_frequency_alignment` + `channel_4_equivalent_frequencies`, both green now.
  `NoiseChannel::trigger_divisor`; gated device-free by
  `sound::tests::the_noise_trigger_delay_matches_samesuite` (eleven `(NR43, delay)` pairs
  read off the ROMs, **not** off our output — nothing in `cargo test` runs SameSuite).
- **`Sound::grid_phase` is now 0..=15, not 0..=3.** Bits 0-1 are the 1 MHz phase the
  square channels align to; **bit 2 is the half-tick the CH4 divisor stage races
  against**. `SquareChannel::trigger` masks `& 3` itself, so squares are untouched — but
  anything new reading `grid_phase` must mask deliberately.
- **THE CGB SHIPPED IN REVISIONS AND ITS APU CHANGED BETWEEN THEM** — `CgbRevision`
  (`src/sound.rs`), default `CgbC` (= C, D, E, and what every DMG does). On CPU CGB 0 and
  B, extra length clocking drops the *"and now enabled"* half of its condition: an
  `NRx4 = $00` write clocks length anyway. That is a silicon bug — it breaks Prehistorik
  Man — fixed in CPU CGB C. CH3 differs again between 0 and B: on B the clock that reaches
  zero leaves the channel playing and the *next* one kills it.
- **Read the machine off the ROM filename — `-cgb0` and `-cgbB` contradict each other, so
  a runner that ignores it can never score more than four of the five.**
  `examples/samesuite.rs::revision_for` maps the suffix to a `CgbRevision`, the same move
  the `testroms` harness makes with `@dmg`/`@cgb`. The ROM still judges itself; the knob
  only picks which machine it is judging. `$GB_CGB_REVISION=cgb0|cgbB|cgbC` exposes it to
  a frontend (a typo warns and is ignored). **A DMG is never affected** —
  `Sound::extra_length_rule` pins the modern rule whenever `dmg_mode` is set, pinned by
  `the_revision_quirk_never_reaches_a_dmg`.
- **`CgbRevision` deliberately lumps C/D/E.** That is exactly right for length clocking,
  but `channel_1_freq_change_timing` ships as `-cgb0BC` / `-cgbDE` / `-A`, which is a
  *different* revision axis and would need `CgbC` split. Whoever takes those three rows
  gets the plumbing for free; `from_tag` returning `None` falls back to the default, so
  adding a tag is additive.
- **`cgb_revision` is machine configuration, NOT cartridge state, so it is not
  serialised** — no `savestate::FORMAT_VERSION` bump. A save state restores into whatever
  machine is running, which is the behaviour you want.
- **`channel_4_freq_change` is left red on evidence, not fatigue.** SameSuite's own
  `apu/README.md`: *"SameBoy – when emulating CGB-CPU-E, passes all tests except
  `channel_4_freq_change` and `channel_1_sweep_restart_2`."* The ROM's own header: *"the
  logic behind it is still unclear."* A row the suite's author does not pass has no
  reference implementation. Diagnosis for whoever tries anyway: our single `period`
  countdown cannot change the *next* step after an NR43 write (it fires ≤16 T later); the
  ROM measures ~56-70 T, which is a two-stage counter keeping its position `p` and counting
  on to the new `2^shift`. That means splitting `NoiseChannel`'s countdown, a format bump,
  and re-calibrating the ten green CH4 rows — for one row nothing passes.
- **`apu/README.md` inside SameSuite is the first thing to read before grinding a row.**
  It states which revisions pass what, and that SameBoy fails two named tests. Clone the
  source (`github.com/LIJI32/SameSuite`) — the shootout ships prebuilt ROMs only, and the
  `.asm` `CorrectResults` tables plus the header comments are where every expectation in
  this section came from.


## Terminal frontend (TUI)

- The terminal frontend is a proper `ratatui` TUI split across three files in `src/screen/`:
  `config.rs` (config + recent-list, hand-rolled `key = value` parser — **no config crate**),
  `romlib.rs` (cartridge-header title/MBC reading, directory scan, fuzzy filter — pure,
  file-system-only, heavily unit-tested), and `tui.rs` (the App: picker screen + game screen,
  one single-threaded event loop, help/quit overlays, centred display).
- **Entry point is `gameboy::run_terminal(Option<PathBuf>)`** — `Some(path)` launches straight
  into the game, `None` opens the ROM picker. The TUI **owns ROM loading** (so the picker and
  in-app ROM switching can build `Gameboy`s); the old `render(Terminal)` on a pre-built
  `Gameboy` is a legacy shim that just opens the picker. `load_session()` in `tui.rs` is the
  single place a `Gameboy` is constructed; it also wires **per-session audio** there
  (`init_audio`, feature-gated on `audio`, silent+graceful with no device) so sound follows
  the active game across in-app ROM switches — the cpal stream lives in `Session` for the
  session's lifetime. Non-kitty-keyboard terminals **degrade, they do not fail** — see
  "Graceful degradation" below.
  Protocol selection uses `build_picker()` (prefer the advertised protocol, e.g. Kitty over an
  env-guessed iTerm2; never panic on query timeout), then a config `protocol` may override.
  Frame pacing targets **59.7275 Hz** (`FRAME_TIME = 16_743µs`), not a rounded 60.

## Graceful degradation — the frontend runs in ANY terminal (`src/screen/tui.rs`)

- **NEVER refuse to start over a missing capability.** `8357546` added a hard
  `return Err(…)` at the top of `run()` when `supports_keyboard_enhancement()` reported
  `Ok(false)`, so on GNOME Terminal / Ptyxis (VTE) — a very common default — the emulator
  printed an error and exited having drawn nothing. Reproduced here: pre-fix binary in
  Ptyxis exits **1**; post-fix it runs at **99.98 % speed, 55.7 display fps, halfblocks**.
  The information was worth having; the *response* was wrong. **Detect, adapt, inform.**
- **The only legitimate refusal is `terminal_is_usable(stdout_tty, stdin_tty)` = both false**
  — nothing to draw on *and* no keys to read. Either half alone still runs.
- **`input_mode_for` maps the handshake to `InputMode::{Enhanced,Basic}`, and `Err` is
  treated as `Ok(false)`** — an unanswered question is not a reason to refuse. The
  enhancement flags are pushed (and popped) **only** in `Enhanced`.
- **Basic mode: auto-repeat *is* the "still held" signal, and that is a hard limit.** A plain
  terminal sends only presses. `Session::hold_window` releases a button `3 ×` the observed
  auto-repeat interval after its last press, clamped to 90–350 ms, starting at 160 ms
  (`KEY_HOLD_BASIC`) before any repeat has been seen. `last_press` is kept **separately from
  `held`** on purpose: on a slow repeat rate the button has already auto-released when the
  next repeat lands, so learning from `held` alone never converges. Gaps above
  `KEY_REPEAT_LEARN_MAX` (400 ms) are the auto-repeat *delay* or a double tap and teach
  nothing — learn one and a tap walks for a second.
- **What is honestly lost, measured in a real terminal** (WezTerm with
  `enable_kitty_keyboard=false`, driven by `wezterm cli send-text`): a *tap* is still exactly
  one tile and menus still step once per press; *holding* a direction takes one step, pauses
  for the OS auto-repeat delay (~0.5 s), then walks continuously; **diagonals and sustained
  two-button holds are not available** because a terminal auto-repeats only the last key
  held. Say that plainly rather than implying parity.
- **A one-shot control key must not strobe.** In basic mode the auto-repeat stream would fire
  `[`/`]`/`v`/`f` ~30×/s. `CONTROL_REPEAT_LOCKOUT` (200 ms, keyed on the *previous event* for
  the same code, not the previous fire) suppresses it — verified live: 12 `[` presses 52 ms
  apart moved the palette exactly one step. Game *buttons* deliberately bypass the lockout;
  a repeat of a button is the evidence the fallback runs on.
- **Say it once, in the four places `config_warnings` already reaches** (stderr scrollback,
  picker footer, a toast on entering a game, the `?` overlay) — that is why
  `BASIC_INPUT_NOTICE` is pushed onto the warnings list rather than printed by hand. Plus an
  `Input` row in the Status panel, because a mode nobody can see is a mode nobody can debug.
- **The graphics path already degraded correctly** and still does: `build_picker` falls back
  to the env-guess query and then to a fixed font size, and `ProtocolType::Halfblocks` is a
  real answer, not a failure. Nothing needed changing there.
- **The Status box is now sized to its contents** (`status.len() + 2`). The old fixed
  `13 + plugins.len()` was two rows short of the twelve rows it already drew, so `Audio` and
  `FPS` were clipped, and with two plugins attached the second plugin row vanished —
  observed live, exactly the bug `docs/plugins.md` §4 says must not happen. A hard-coded
  height beside a `push` is a standing invitation to it.
- **The regression test is `testharness/basicterm.rs` — a pty, not a window.**
  `cargo test --release --test basicterm` (19 s, runs under a plain `cargo test`, CI
  included). It allocates a pty, **is** the terminal on the far end, and runs the real
  `gameboy::run_terminal` as a child in its own session (`setsid` + `TIOCSCTTY`, so even the
  child's `/dev/tty` probes land on the pty and nothing can reach the developer's desktop).
  Two scenarios differing in **one reply**: answer `CSI ? u` and the frontend must report
  `kitty`, ignore it and it must report `basic`. Teeth proved by restoring the old
  `return Err` — `basic` goes red with "never drew a Status panel" while `kitty` stays green,
  which is the bug's exact signature. **Do not replace this with a windowed test**: spawning
  terminal windows steals desktop focus, and synthetic key injection (`xdotool`/`wezterm cli
  send-text`) goes to whatever has focus — a `wezterm cli send-text` whose socket had gone
  stale typed a dozen characters into the captain's own session.
- **Three things that make a fake terminal faithful, all learned the hard way:**
  1. **Answer everything a real terminal answers.** Silence is not neutral — every
     unanswered query costs the frontend a multi-second timeout in a blocking path. Ignoring
     `CSI 5 n`, `CSI 14 t` and `CSI 16 t` drove the emulator at **1 % of real speed** and made
     input look lost when it was merely queued behind blocked loop iterations.
  2. **Never assert on a word in the raw byte stream.** `ratatui` diffs **per cell**, so
     `grayscale` → `green` puts `een` on the wire and `green` → `amber` puts `amb`, a skip,
     `r`. Resize the pty (`TIOCSWINSZ`) to force a full repaint, then read.
  3. **Mind the modulus when counting suppressed repeats.** The palette cycle is three long
     and the first press of a burst is a legitimate press, so 10 repeats land on the same
     colour whether the lockout works or not. The harness sends **12**.
- **Doing it by eye instead (only when a picture is the point):** Ptyxis is VTE and has no
  kitty keyboard protocol; WezTerm gives a clean A/B on one binary with
  `--config enable_kitty_keyboard=false|true`. Ptyxis is GTK4 and stays on native Wayland
  whatever `DISPLAY` says, so it cannot be screenshotted here — use WezTerm on XWayland.
  Verified this way once: Ptyxis **99.65 % speed, 55.7 display fps, half-blocks**; WezTerm
  without the protocol plays Pokémon Blue — a tap is one tile, menus step once per press, a
  held direction walks the map — and with it negotiates `kitty shm` unchanged.

### Conformance record

Per the [standing convention](#standing-convention-every-emulation-fix-feeds-the-conformance-tool),
and like the three render bugs, this is a **presentation** defect, so point 3 cites a
protocol/library contract rather than Pan Docs.

*Symptom* — the emulator prints one line and exits 1 in an ordinary terminal; the player sees
no picture at all and is told to install a different terminal.
*Caught by* — **no test ROM, and no headless harness either.** Every ROM-driven check runs
against a `Gameboy` the harness constructs directly and never enters `screen::tui::run`, so
the whole entry path — capability handshakes, refusals, input mapping — is invisible to them.
That was a real coverage gap, and `testharness/basicterm.rs` is the answer to it: a pty
harness that drives `run_terminal` itself and would have caught this on the commit that
introduced it. `a_terminal_without_keyboard_enhancement_is_not_a_refusal` and
`refusal_only_when_there_is_neither_a_screen_nor_a_keyboard` pin the same decisions at unit
speed, where the *why* is legible.
*Contract violated* — the kitty keyboard protocol is documented as **progressive
enhancement**: a terminal that does not understand `CSI > 1 u` ignores it and keeps sending
legacy keys ([kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)),
and `crossterm::terminal::supports_keyboard_enhancement` returning `Ok(false)` means exactly
"you get legacy keys", not "input is impossible".
*New tests* — `testharness/basicterm.rs`, which closes that gap by driving the real entry
point over a pty, plus `a_basic_terminal_holds_a_button_across_auto_repeat`,
`the_hold_window_adapts_to_a_slow_auto_repeat_rate`,
`auto_repeat_cannot_strobe_a_one_shot_control_key`,
`the_kitty_path_is_unchanged_by_the_fallback` and
`the_status_panel_says_which_input_mode_is_live`. Conformance candidate: **an absent optional
capability must cost only that capability** — the same principle as the adapter contract's
"declining a capability is `exit 0`, not an error"
([`docs/conformance-action.md`](docs/conformance-action.md)).
- **Scaling gotcha (the "limited to small" bug):** `ratatui_image::Resize::Fit` **refuses to
  upscale** an image smaller than the area (it caps the target at the source size), so a
  160×144 source always rendered tiny; the old code faked enlargement with a capped integer
  pre-scale (`MAX_SCALE=4`) → the coarse "3–4 tiers". Use **`Resize::Scale`**, which upscales
  to fill at pixel resolution (nearest-neighbour, aspect-preserved). The image is drawn into a
  centred rect from `fit_metrics()` (uncapped, aspect-correct for non-square cells, one tier
  per terminal row); it re-fits on resize with no keypress. The display is **borderless** so
  height (the binding constraint for a 10:9 image in tall cells) isn't wasted; status/controls
  are the framed panels.
- **HiDPI "tiny picture" gotcha (the *real* one, proven with `GB_DEBUG_LAYOUT`):** the earlier
  `Resize::Scale` fix is correct, but on a **scaled Wayland display** the picture still rendered
  tiny. Cause, measured on a 192-DPI (2×) box: WezTerm answers the cell-size query (`CSI 16 t`)
  and `TIOCGWINSZ` in **logical** pixels (cell `8×16`, so `ratatui-image` sizes the iTerm2/sixel
  image in logical px), but **renders that inline image in *physical* device pixels** — so on
  scale *S* the picture comes out *S*× too small (784 logical px asked → ~306 px shown ≈ the
  "310×275" report). Under **XWayland** WezTerm instead reports the *physical* cell (`17×41`) and
  renders correctly, which is why it only bites native Wayland. Fix: a **`cell_scale`** config
  key (`config.rs` `CellScale`; default `Fixed(1.0)` = untouched) that multiplies the cell size
  the frontend feeds the picker — the picker is *rebuilt* via `Picker::from_fontsize(scaled)`
  (there is no `set_font_size`) so **both** `fit_metrics` and the encoded image become
  physical-pixel sized. `cell_scale = auto` reads `Xft.dpi/96` (via `xrdb`) / `GDK_SCALE`;
  `$GAMEBOY_CELL_SCALE` overrides for testing. Proven: at `142×46` cells, `disp_px` 784×704 →
  1568×1408 with `cell_scale=2`, i.e. it now fills the physical window. **Screenshots are blocked
  (GNOME/Wayland denies the D-Bus screenshot and `import -window root` is empty), so this was
  verified by the logged numbers, not by eye** — a human on a HiDPI display must confirm the
  picture visibly fills. `GB_DEBUG_LAYOUT=<file>` dumps term cells, queried/effective font,
  area, fit rect, scale and `disp_px` (gated, zero-cost when unset).
- **Note for HiDPI users:** the correction is **opt-in** (default `1.0` never regresses low-DPI /
  XWayland). A native-Wayland HiDPI user must add `cell_scale = auto` (or an explicit `2.0`) to
  their `config.toml` for the picture to fill.
- Config lives at `${XDG_CONFIG_HOME:-~/.config}/gameboy/config.toml`; recents at
  `${XDG_STATE_HOME:-~/.local/state}/gameboy/recent`. Missing config = defaults, never an
  error. `rom_dir` defaults to `~/roms`. Keys: `rom_dir`, `protocol`, `palette`, `upscale`,
  `upscale_factor`, `cell_scale`, `key_a/b/start/select` — all documented in the README's
  "Terminal config file" table.
- **Config values may be BARE or double-quoted — both parse.** `parse_kv` (`config.rs`)
  strips *optional* surrounding quotes (`.strip_prefix('"').and_then(strip_suffix('"'))
  .unwrap_or(value)`), so `palette = green` and `palette = "green"` are equivalent; the
  `parses_all_fields` unit test pins the bare form. (Historical note: a belief that "unquoted
  values silently do nothing" does **not** reproduce against the current parser — if you must
  document a quoting requirement, verify it against `config.rs` first. The one thing bare form
  can't carry is a literal `#`, which starts a comment.)
- The picker opens with the most-recently-played ROM pre-selected (recents sorted first), so
  **Enter loads the last game**; `.` resumes the last-played ROM directly even if it lives
  outside `rom_dir`.
- **ALSA is only needed with `--features audio`** (default builds/tests need none). For that
  feature without system dev headers, a shim exists at `~/.local/lib/alsashim/libasound.so` +
  `~/.local/lib/pkgconfig/alsa.pc`; build with `PKG_CONFIG_PATH=~/.local/lib/pkgconfig
  LIBRARY_PATH=~/.local/lib/alsashim LD_LIBRARY_PATH=~/.local/lib/alsashim cargo build
  --features audio`.

## Battery saves (in-game saves) — `src/mbc/battery.rs`

- **NEVER persist anything in a `Drop` impl. `Drop` is not a shutdown hook here.** The
  desktop frontend's `glutin`/`winit` `event_loop.run()` **never returns** — winit 0.26
  calls `process::exit(0)` on `ControlFlow::Exit` — so the `Gameboy` moved into that closure
  is never dropped. `[profile.release] panic = 'abort'` skips destructors too, as do a
  closed terminal window, SIGTERM and `kill`. Cartridge RAM used to be written **only** in
  `impl Drop for MBCx`; that is why in-game saves vanished. This applies to anything else
  that must survive a shutdown.
- **How it works now:** `writeram` marks the battery dirty; `Gameboy::frame` calls
  `MemoryBankController::autosave` once per frame, which writes the save out after 60 quiet
  frames (~1 s) or 600 dirty frames. `Gameboy::flush_battery()` forces it (called from the
  desktop `CloseRequested` handler, since nothing there is ever dropped). `Drop` still
  flushes as a last chance only. All four battery MBCs (1, 2, 3, 5) share `Battery`.
- **Writes are atomic** (`<file>.tmp` → `fsync` → `rename`) — a truncated save is worse
  than no save. And a session that never wrote cartridge RAM **writes nothing**: the old
  code unconditionally dumped its pristine all-zero RAM on exit, so one launch that failed
  to load permanently erased a real save. That is exactly what happened to the captain's
  Pokémon Blue `.gbsave` (32,776 bytes, all zeros).
- **Formats (settled by the FlashGBX study — this is the ecosystem's format, not ours):**
  `<rom>.sav` = raw cartridge RAM, bank 0 first, no header, exactly `ramsize` bytes. An MBC3
  clock is a **48-byte footer appended to that same file**, all little-endian: 5×`u32`
  registers (S, M, H, day-low, day-high where bit0=day bit 8, bit6=HALT, bit7=CARRY), the
  same five again as the latched copy, then a `u64` UNIX timestamp. Written **only for
  header types 0x0F/0x10** — 0x13 has no clock and must get nothing extra. Load detection is
  by size (`filesize - ramsize` == 48, or 44 for an older 32-bit-timestamp variant);
  `rtc_zero = timestamp - regs_total`, which is what makes the clock advance while the
  emulator is closed. `.srm` also read. Legacy `<rom>.gbsave` (8-byte **big-endian** RTC
  anchor **prepended** to the RAM) is still **read** when no `.sav` exists, migrated, and
  **left on disk untouched as a backup**. That prefix was applied even to type 0x13, so
  every foreign save used to load 8 bytes shifted — never reintroduce a header.
- **Crashes fixed here (all were aborts under `panic = 'abort'`, which also loses the save):**
  RAM-less MBC5+RUMBLE (types 0x19/0x1C) `% rambanks` divide-by-zero on the bank register —
  and bit 3 of that register is the rumble motor, so every rumble game hits it;
  `rom_banks()` returning 0 for unofficial `0x148` codes (0x52/0x53/0x54 are real) making
  every `% rombanks` divide by zero — it now falls back to `rom.len()/0x4000`, minimum 2;
  and unchecked RAM indexing on a short or foreign `.sav`. Cart types 0x08/0x09 (no mapper,
  with RAM/battery) are now supported rather than rejected — MBC0's save-state block stays
  byte-identical for 0x00 carts, so **no `savestate::FORMAT_VERSION` bump**.
- **MBC2 stores the low nibble (`v & 0x0F`), reads OR in `0xF0`.** Storing `v | 0xF0` plays
  identically but makes our `.sav` differ byte-for-byte from a hardware dump.
- **MBC1M multicarts are a different *wiring*, not a different mapper** (`src/mbc/mbc1.rs`).
  BANK1's top bit is not connected and BANK2 drives bank bits **4-5**, so mode 1 shows
  `$00/$10/$20/$30` in `$0000-$3FFF` — that is how a compilation cart switches games. The
  `00 → 01` translation still uses all five *written* bits before bit 4 is dropped, so
  writing `$10` selects bank `$00` and writing `$00` selects `$01`; that single rule is what
  Mooneye's expected-bank table turns on. Both differences live in three one-line helpers
  (`bank2_shift`/`bank1_mask`/`bank2_mask`), so the ordinary path is unchanged and the state
  layout did not move — **no `savestate::FORMAT_VERSION` bump**. **Detection is a guess and
  has to be**: such a cart is indistinguishable from a plain MBC1 by its own header, so we
  require exactly 1 MiB **and** a second Nintendo header at `$40104` (bank `$10`) — Pan Docs'
  own identification rule — with `GB_MBC1_MULTICART=1|0` to force it either way. **The
  false-positive direction is the dangerous one**: misreading an ordinary 1 MiB cart as a
  multicart moves BANK2 down two bits and strands half the ROM, which is why the heuristic is
  narrow and both directions are unit-tested. **Mooneye `emulator-only` is 28/28**
  (`cargo run --release --example mooneye_scout -- <suite>/emulator-only`).
- **A save that cannot be read is never overwritten** (the battery poisons itself on a read
  error). Loading fails loudly instead of silently starting a new game over an existing save.
- **Loaded saves are normalised to the declared RAM size** (pad/truncate) and every MBC
  RAM access is bounds-checked — a foreign or odd-sized `.sav` must not panic (with
  `panic = 'abort'` a panic is an instant crash *and* loses the save).
- **Tests:** `cargo test --release --test battery` (`testharness/battery.rs`) builds a
  hand-assembled MBC3+RAM+BATTERY ROM in-process — no commercial ROM, no vendored binary —
  and proves the save lands with **no destructor run** (`std::mem::forget`), reloads bank by
  bank, migrates the legacy format, and accepts foreign `.sav`s. Validate it is not vacuous
  by disabling the `autosave()` call in `Gameboy::frame`. For a real cartridge use
  `examples/rom_script.rs` (scripted headless input, `EXIT=kill` aborts with no destructors);
  the Pokémon Blue save script is in `docs/testing.md`.
- **Conformance-tool notes:** no public suite (Blargg / Mooneye / acid2) covers persistence —
  they run in one process and never look at the filesystem. **That gap is itself a finding.**
  Per-fix records:
  - *Drop-only persistence.* Symptom: save in-game, quit, relaunch → "NEW GAME" only.
    Caught by: nothing before; now `battery.rs` case 1. Hardware: a battery-backed cart holds
    SRAM from the instant the game writes it (Pan Docs, MBC3) — no "quit cleanly" step exists.
  - *8-byte RTC prefix on every save.* Symptom: a save from another emulator or a real
    cartridge loads shifted by 8 bytes and its last 8 bytes are gone; with a save checksum
    that is indistinguishable from "NEW GAME". Caught by: nothing before; now case 6.
    Contract: `.sav` is raw RAM, and type 0x13 has no clock.
  - *RAM-less MBC5+RUMBLE bank write.* Symptom: any rumble game crashes the emulator on its
    first motor poke. Caught by: nothing — no rumble or RAM-less MBC5 ROM in the suite, a
    coverage gap; now case 7. Hardware: bit 3 of 0x4000-0x5FFF is the motor (Pan Docs, MBC5).
  - *Unofficial ROM-size code.* Symptom: a bootleg, corrupt or 1.1/1.2/1.5 MiB cart crashes
    on its first bank switch. Caught by: nothing; now case 7. Real dumpers never size a cart
    from byte 0x148 either.
  - *Undersized cartridge RAM does not mirror* (we allocate a full 8 KiB bank for RAM code
    0x01 = 2 KiB). Not fixed, deliberately — recorded as a conformance check, since mirroring
    is how FlashGBX recovers a cartridge's true save size.
  - ~~*MBC3 latches on any write to 0x6000-0x7FFF*, not on the 0x00→0x01 sequence.~~
    **Withdrawn — this was never a deviation.** See the MBC3 RTC section below; latching on
    any single write is what hardware does, and CasualPokePlayer's `latch-rtc-test` is built
    to prove it. Do not "fix" it and do not add the conformance check.
- **A real cartridge is a reference oracle for saves** now that our format matches: play in
  the emulator, write the `.sav`, restore it to the cart with a dumper, read it back, compare.
  No emulator-only suite can make that check.
- **Do not vendor FlashGBX's `db_DMG.json`** (GPL-3.0 compiled database, and our repo is
  MPL-2.0). Formats, header constants and hardware behaviour are facts and were adopted; no
  code was taken.

## MBC3 real-time clock — `src/mbc/rtc.rs` (ax6 `rtc3test` 0/3 → 3/3)

- **The clock is driven by EMULATED time, never by the host's.** `Rtc::tick` counts T-cycles
  of *real console* time into a sub-second divider and ticks the counter chain once every
  `CYCLES_PER_SECOND` (4,194,304) of them; `MemoryManagementUnit::do_cycle` calls it once per
  instruction with `gputicks`, which is already divided by the CGB speed multiplier (the
  crystal is on the cartridge and does not know the console changed gear). The old code
  derived every register from `SystemTime::now()`, so the in-game clock ran at whatever
  multiple of real time the emulator did — 60× too slow relative to the game headless, frozen
  while paused — and there was no sub-second phase at all. It also **panicked** if the host
  clock predated the UNIX epoch. **The host clock is now used for exactly one thing: the gap
  between a save being written and reloaded.**
- **The registers are counters, not a time.** Seconds/minutes are 6-bit, hours 5-bit, the day
  counter 9-bit (day-low + `DH` bit 0). Each carries into the next **only** when it rolls over
  from its natural maximum (59/59/23); a value loaded *above* that maximum counts up to the
  top of its physical width and wraps to 0 **without carrying** — 62 minutes → 63 → 0, hours
  untouched. One helper (`advance_counter`) implements this and is used by both the per-second
  tick and the shutdown catch-up, so the two cannot disagree. Deriving from a normalised
  `SystemTime` cannot express any of it, which is the whole of `rtc3test`'s "range tests".
- **`DH` bit 6 (HALT) freezes the sub-second divider too**, so a clock stopped 600 ms into a
  second and restarted later ticks 400 ms after the restart, not a full second
  (`rtc3test` "RTC off/400"). **Writing the seconds register restarts the divider; writing any
  other RTC register does not** ("Sub-second writes", measured to 1.5 ms). `DH` bit 7 is the
  day-counter overflow flag past 511 days and is sticky.
- **ANY single write to `$6000-$7FFF` latches — this is correct, not an approximation.**
  The repo used to record the `$00`→`$01` sequence as the hardware rule and our behaviour as a
  known deviation. It is the other way round: CasualPokePlayer's `latch-rtc-test` writes five
  random register values and then **one random byte** to `$6000` per row, and its reference
  dump has every row reading back what was just written. Implementing the strict pair was
  tried here and turned that ROM into a screen of zeroes (FAIL, measured); any-write restored
  it. Pan Docs describes the idiom games use, not the decode.
- **Measured against `main` (`b386a15`), both directions:** ax6 `rtc3test` **0/3 → 3/3**
  (shootout's `rtc3test-1/2/3`, `GB_MODEL=cgb`, 930/810/1560 frames); CasualPokePlayer
  `latch-rtc-test`, `rtc-invalid-banks-test`, `ramg-mbc3-test` **3/3 before and after**;
  `testroms` 33/35 "all results match the baseline"; Mooneye acceptance 55/75,
  `emulator-only` 28/28 and `misc` 2/8 all unchanged; 607 lib tests, `savestate`, `battery`
  8/8, `audiocapture` (`dmg_sound` 12/12) and the audio-perf gate (0.243 ms/frame, 68.9×)
  all green.
- **Cost: it is not free on a clock cartridge.** 400 M cycles headless, min of 3: a cartridge
  **without** a clock 1.526 → 1.545 s (+1.3 %, at the level of code-layout noise — that path
  is one bool test), a cartridge **with** one 1.410 → 1.484 s (**+5.2 %**, a `dyn` call plus
  an add and a compare per instruction), still ~64× real time. Batching the call was
  considered and rejected: the accumulator would be machine state needing serialisation, and
  it would race with the seconds-write divider reset, for ~0.3 ms per emulated frame.
- **None of this is in CI.** `rtc3test` is judged by a reference screenshot and neither the
  ROM nor the reference is ours to vendor, so the in-repo gate is the unit tests in
  `src/mbc/rtc.rs`, one per `rtc3test` sub-behaviour. If you touch the RTC, re-run `rtc3test`
  by hand against a shootout checkout — the unit tests cover the behaviours but not the ROM.
- **`savestate::FORMAT_VERSION` bumped** (the MBC3 block is now counters + latch + sub-second
  divider instead of `rtc_ram`/`rtc_ram_latch`/`rtc_zero`). The sub-second divider **is**
  machine state: drop it and every load moves the next tick by up to a second.
- The `.sav` footer format is unchanged — 48 bytes, FlashGBX/VBA-M/BGB layout. On load
  `Rtc::restore` masks the stored registers and advances them by `now - timestamp`; a halted
  clock gains nothing and a timestamp from the future advances nothing rather than going
  backwards.

## Save states

- **Core API, frontend-agnostic:** `Gameboy::save_state() -> Vec<u8>` /
  `Gameboy::load_state(&[u8]) -> Result<(), savestate::StateError>` (delegating to the same
  pair on `Cpu`). The core serialises to bytes and makes **no filesystem assumptions** — each
  frontend decides where the bytes go. Full design + block layout + the BESS verdict in
  [`docs/savestates.md`](docs/savestates.md).
- **Format `GBSS`, hand-rolled, versioned, native (NOT BESS).** `src/savestate.rs` holds the
  no-dependency little-endian `Writer`/`Reader`/`StateError` codec (same no-dep ethos as the
  test harness's hash/PNG). Every stateful component got an **additive** `save_state`/
  `load_state` pair (no struct restructured, nothing added to the `rb`/`wb`/`do_cycle` hot
  path). BESS was rejected for now because it can't represent this core's internal counters
  (PPU `modeclock`/mode, timer `internaldiv`/`internalcnt`, HDMA bookkeeping, APU
  frame-seq phase) without desync — proposed as an additive follow-up (`bess.rs`).
- **Bump `savestate::FORMAT_VERSION` on ANY layout change** — loads reject a version mismatch
  rather than corrupting. The ROM is never stored; states carry the title + global checksum
  and refuse a mismatched cartridge (`StateError::RomMismatch`).
- **Sharp edges:** (1) The GPU framebuffer `data` IS saved (so the visible frame is right on
  load) but `bgprio` is NOT (it is rebuilt at the top of every `renderscan`). (2) The APU
  section is written **length-prefixed and present-flagged** so it is skippable — that is what
  lets a native-with-audio state load on `wasm32`, where `MMU.sound` doesn't exist. The
  `blip_buf` buffers and host player are runtime plumbing, never serialised. (3) MBC RAM is
  length-checked on load. (4) MBC3 RTC captures `rtc_ram`, `rtc_ram_latch`, and the
  `rtc_zero` epoch anchor.
- **Round-trip proof:** `testharness/savestate.rs` (`cargo test --release --test savestate`,
  `harness = false`). Runs N frames → save → run reference forward, load into a fresh machine,
  and asserts **byte-identical framebuffers AND byte-identical full-state snapshots every
  frame**, plus negative checks (magic/version/truncation/wrong-game). Uses the vendored
  dmg-acid2 always, and Blargg cpu_instrs (scrolling screen ⇒ non-trivial frame variation)
  when the submodule is present.

## Terminal frontend: stats / speed / save-slots (`src/screen/tui.rs`)

- Integrated into the **shell crew's single-threaded `App` state machine** (Screen/Overlay,
  one event loop — no separate input thread). Added `App` fields `speed`/`stats`/`slot`/`toast`
  and handled in `handle_game_key`. In-game keys: **`t`** stats overlay (off by default, does
  NOT pause), **`+`/`-`** speed ladder (0.5x/1x/2x/4x/MAX), **`1`-`3`** select save slot,
  **`F5`/`F8`** quick-save / quick-load the active slot (F-keys because their default game
  buttons are `a`/`s`/`x`/`z`, so `s`/`l` were taken). All listed in the Status/Controls sidebar
  and the `?` help overlay.
- Speed is a wall-clock **interval** derived from `FRAME_TIME` (`SPEED_LADDER`); `1x` = the true
  16.743 ms DMG frame, so it is real time. `MAX` runs an uncapped per-iteration burst. Save/load
  and all emulator mutation happen on the single loop thread (no locking needed here). After a
  load, `Session.image` is rebuilt so the restored frame shows immediately.
- Stats overlay reuses the shell crew's per-second `Session.fps`; emulated speed % =
  `fps/59.7275*100`, frame-ms = `1000/fps`, plus `gb.debug_pc()`/`gb.debug_rom_bank()`. Rendered
  only when toggled, so it costs no emulation time.
- **Audio + fast-forward:** the APU emits at a fixed rate; faster-than-real play overruns the
  cpal queue (bounded to ~180 ms by the audio crew), which discards the oldest — i.e.
  fast-forward **drops samples** rather than pitch-shifting; slow-mo under-runs to brief
  silence. No resampling, by choice.
- Save states live under `$XDG_STATE_HOME` (or `$HOME/.local/state`, else temp) in
  `gameboy/states/<sanitised-title>.slot<N>.gbss`.
- **Save-state capture must track new machine state.** After the accuracy crew's fixes it now
  also covers `Cpu.halt_bug` (HALT-bug latch — can be live at a frame boundary) and the MMU's
  `bus_tick_enabled`/`bus_ticked` (per-access timer stepping; reset between instructions but
  captured for completeness). Any future CPU/MMU/PPU field that survives across a `do_cycle`
  boundary MUST be added to the `save_state`/`load_state` pair or `savestate` round-trip desyncs.
- **Terminal pacing:** the speed ladder gates `frame()` on the true 16.743 ms DMG frame time,
  so `1x` is real-time (fixing the old 5 ms/~200 fps terminal bug the audio crew flagged);
  `2x`/`4x` shorten the interval and `MAX` runs an uncapped burst.

## Documentation code map & clickable links

- **[`docs/codemap.md`](docs/codemap.md)** is the top-level orientation page (frame flow,
  memory-access routing CPU→MMU→device, interrupt delivery, frontend attach points) with
  Mermaid diagrams. Linked from the README and `docs/components/README.md`.
- **Every `path:line` reference in the docs is a generated GitHub link**, not hand-typed.
  The source of truth is the human-readable ``path:line`` code span; the URL is derived.
  - Strategy: **branch links** (`blob/main/…#Lnn`) so they track moving code, plus a
    per-page **"documented as of `<sha>`" marker** (an HTML sentinel `<!-- codemap:asof -->`
    the tool refreshes). File/dir links and doc-to-doc links stay **relative** (editor- and
    GitHub-friendly); only references carrying a `:line` become absolute GitHub links.
  - Tool: `docs/tools/codemap_links.py`. `make docs-links` rewrites every reference + restamps
    the marker (idempotent); `make docs-check` fails on a missing file/dir or an out-of-range
    line number (CI-suitable). Repo slug/branch via `CODEMAP_REPO`/`CODEMAP_REF` env vars
    (default `Alchemy86/gameboy` / `main`). `docs/display.md` is in the tool's `EXCLUDE` set
    (whole doc, terminal-scaling refs track `src/screen/tui.rs`); refs whose target is in
    `SRC_EXCLUDE` (`src/sound.rs`, `src/screen/tui.rs` — both being rewritten live) are kept
    as file-level relative links with a flux note (apu.md, frontends.md) rather than churned
    every rebase. Drop a file from `SRC_EXCLUDE`/`EXCLUDE` and re-run `make docs-links` once it settles.
  - **After editing source that shifts line numbers, run `make docs-links`** then
    `make docs-check`. `docs-check` catches out-of-range rot but *not* in-range wrong-content
    rot — when another crew rewrites a file, re-anchor the affected pages' references by
    content, not just by range. Fenced ASCII-diagram cites (e.g. ppu.md's mode-cycle box) are
    plain text the tool does not touch — update them by hand.
- **Code↔docs cross-links:** hard-part functions carry a short comment pointing at their
  component page and the Pan Docs section (CPU interrupt dispatch, PPU mode cycle, MMU
  dispatch/echo-RAM/OAM-DMA, MBC1 banking). The APU "why" lives in `docs/components/apu.md`
  prose, not in `src/sound.rs`, to avoid conflicting with the active audio rewrites.

## Pixel-art upscaler (`src/upscale/`)

- **Full design + survey + measured costs: [`docs/upscaling.md`](docs/upscaling.md).**
  Core module, dependency-free, wasm/ffi-safe: `Upscaler::apply(rgba, w, h)` → upscaled
  RGBA; algorithms `nearest`(×1–6) / `scale2x`(×2–4) / `mmpx`(×2,×4) / `lcd`(×2–6), all
  runtime-switchable. Worst case measured ~1.6 ms (MMPX ×4) vs the 16.7 ms frame — CPU
  pre-pass everywhere by choice (one code path for desktop GL, TUI and web canvas).
- **Licensing was the design constraint:** Scale2x reimplemented from the published rule
  (reference impl is GPL-2.0 — do not copy it); MMPX ported from the **MIT** reference
  (attribution comment above `fn mmpx` must stay); xBRZ (GPLv3) and hqx (LGPL tables)
  deliberately excluded — do not vendor them into this MPL-2.0 repo. The MMPX port is
  locked by a unit test pinned to the reference's exact output (verified byte-identical
  against the authors' C reference; note that the ITotalJustice c99 port has an
  out-of-bounds `clamp()` bug — ours clamps correctly).
- Wiring: config keys `upscale` / `upscale_factor` (parsed in `screen/config.rs`, clamped
  via `Algorithm::clamp_factor`); TUI keys `]`/`}` + Status line (`screen/tui.rs`,
  upscale runs **after** palette mapping or the LCD grid would be quantised away);
  desktop keys `U`/`I` (`gameboy.rs::render_desktop`, texture is uploaded at upscaled
  size so `out_w/out_h` replace `self.width/height` in draw/resize); web keys `]`/`\`
  (`screen/web.rs`, canvas auto-resizes). Default is `nearest` ×1 = exact old behaviour.
- **Evidence pack:** `cargo run --release --example upscale_gallery [-- rom --frames N]`
  writes per-algorithm PNGs + side-by-side strips + `bench.txt` (headless, deterministic);
  committed samples from the MIT dmg-acid2 ROM live in `docs/upscale-samples/` (never
  commit galleries from commercial ROMs — generate those into `target/upscale-gallery/`).

- **Project goal — standalone conformance tool: [`docs/conformance.md`](docs/conformance.md).**
  The captain wants the test harness eventually extracted into a tool anyone can point at their
  *own* emulator (serial-capture / framebuffer-compare / audio-capture adapters, reporting *why*
  a test fails). The key design constraint recorded there: our own machine-readable telemetry
  stream (render crew, EMIT half of `docs/debugging.md`) should be the reference adapter, so the
  adapter contract and the telemetry format must be designed to match. Also linked from the
  README "Roadmap & goals" section.

## Terminal render path — capped transmission (`src/termimage.rs`)

- **Full analysis + measured numbers: [`docs/terminal-render.md`](docs/terminal-render.md).**
  The rule: **never size the transmitted image by the window.** `ratatui-image`'s
  `Resize::Scale` resizes the frame to the *display rectangle* and re-encodes it every frame —
  measured at a 1568×1408 rectangle that is **39.7 ms / 11.6 MB per frame on Kitty** and
  **86.4 ms on iTerm2** (25 fps / 12 fps). Transmitting 160×144 and letting the terminal scale
  costs **0.10 ms / 160 KB** and does not move when the window grows.
- `src/termimage.rs` is dependency-free (hand-rolled base64 + a **stored-deflate** PNG — no
  compression, hence no compression cost at 60 Hz) and speaks two protocols that can place an
  image over a rectangle measured in **cells**: Kitty (`a=T,U=1,c=,r=` + Unicode placeholders)
  and iTerm2 (`width=`/`height=` default to cells; `Npx` would mean pixels). **Sixel and
  halfblocks have no cell placement** and keep the `ratatui-image` path — their cost really
  does scale with the window.
- **Never put a large escape sequence in a `ratatui` cell symbol.** `Buffer::diff` computes
  `invalidated = max(symbol.width(), invalidated)` and re-emits every following cell while it
  is non-zero; a 92 KB payload has width ~92 000, so the whole screen is retransmitted every
  frame (measured: 84 KB for an *unchanged* picture). The image sequence is therefore written
  straight to stdout **after** `terminal.draw` returns, and the cells it covers are marked
  `skip`.
- **Nothing may be drawn on top of the picture.** A terminal image owns whole cells: text there
  is not *over* the image, it is *instead of* it, and for Kitty it removes the placeholders
  that make the picture update at all. This was a live bug — the stats overlay froze the game
  on screen. The stats panel and toast now get their own rectangles (sidebar + footer; a left
  column + reserved row in fullscreen), asserted across eight window sizes by
  `the_stats_panel_never_overlaps_the_picture_at_any_size` (it fails on the old layout).
- **`Picker::new_resize_protocol` picks a random Kitty image id (`rand::random()`).** Calling
  it per frame — which the old `step_frame` did — hands the terminal 60 new images a second,
  8.8 MB each at fullscreen, never deleted, and blows its image-storage quota (Kitty's default
  is 320 MB) in under a second, after which the picture stops updating. Reuse one id.
- **Emulation is decoupled from rendering.** `step_frame` is emulation *only*; palette,
  upscale and encode happen in `render_display`, at most once per displayed frame. `frames_due`
  advances the deadline by exact periods (capped at `MAX_CATCHUP`, then resyncs);
  `next_draw_at` paces drawing at `max(FRAME_TIME, last_draw_cost)` from the draw's *start*.
  **Display frames are dropped; emulated frames never are** — the audio clock is slaved to
  emulation. Both rules are unit-tested over a virtual clock.
- `transmit_scale` config (default `auto`): `nearest` → ×1 (the terminal's own stretch does
  exactly the same thing for 1/16th the bytes), a real filter → its own factor. `render_path =
  legacy` restores the old behaviour as an escape hatch.
- Reproduce any of it: `cargo run --release --example render_bench -- --frames 120 --motion
  --rom tests/cpu_instrs/cpu_instrs.gb` (add `--json` for one machine-readable line).

### Conformance record for the three render bugs

Per the [standing convention](#standing-convention-every-emulation-fix-feeds-the-conformance-tool).
These are **presentation** bugs, not emulation ones, so point 3 cites the *protocol or library
contract* violated rather than Pan Docs — and point 2 is the same answer for all three, which is
itself the finding: **no test ROM catches any of them.** A ROM-driven suite compares
framebuffers, and all three leave the framebuffer perfectly correct while the player sees a
frozen or crawling picture. That is a real gap in what a conformance tool built only on test
ROMs can see, and the argument for the telemetry stream being the reference adapter
([`docs/conformance.md`](docs/conformance.md)): `speed_pct`, `fps`, `tx_bytes` and
`frames_dropped` expose all three, a framebuffer hash exposes none.

1. **Transmission sized by the window.**
   *Symptom* — the game runs slowly and the picture stutters, progressively worse as the window
   is enlarged; ~18 fps at fullscreen with the emulator itself 60× faster than needed.
   *Caught by* — no test ROM. `examples/render_bench.rs` (`--json`) measures it; the telemetry
   invariant is `tx_bytes` independent of `display_px_*`.
   *Contract violated* — `ratatui-image`'s `Resize::Scale` is documented as resizing to the
   render area, which is correct for a photo and wrong for a 60 Hz video source; the terminal
   protocols both offer cell-sized placement precisely so the client need not
   ([kitty graphics protocol, Unicode placeholders](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders);
   [iTerm2 inline images](https://iterm2.com/documentation-images.html) — `width`/`height`
   default to cells).
   *New test* — `transmitted_bytes_do_not_grow_with_the_window`,
   `image_payload_is_independent_of_the_display_rectangle`. Conformance candidate: **cost per
   displayed frame must not be a function of output size.**

2. **A large escape sequence in a `ratatui` cell symbol.**
   *Symptom* — the whole UI flickers/redraws constantly and a *still* game screen still costs
   84 KB/frame on the wire.
   *Caught by* — no test ROM.
   *Contract violated* — `ratatui`'s `Buffer::diff` computes
   `invalidated = max(symbol.width(), invalidated)` and re-emits every subsequent cell while it
   is non-zero (`ratatui-0.29.0/src/buffer/buffer.rs`); a cell symbol is assumed to be a
   *glyph*, so a 92 KB payload reports a display width of ~92 000 and invalidates the screen.
   *New test* — `an_unchanged_frame_transmits_nothing`. Conformance candidate: **an unchanged
   frame must produce zero output.**

3. **A new Kitty image id per frame.**
   *Symptom* — the picture freezes after roughly a second while sound and input keep working;
   it recovers only when something forces a full redraw.
   *Caught by* — no test ROM.
   *Contract violated* — Kitty images persist until deleted and the terminal enforces a storage
   quota (320 MB by default), so a client must reuse an id or delete what it uploads
   ([kitty graphics protocol, "Image persistence and storage quotas"](https://sw.kovidgoyal.net/kitty/graphics-protocol/)).
   `Picker::new_resize_protocol` picks `rand::random()`, so calling it per frame uploads 60
   undeleted images/second (8.8 MB each at fullscreen).
   *New test* — `the_picture_keeps_updating_with_the_stats_overlay_on` covers the same
   user-visible symptom. Conformance candidate: **a long run must not grow terminal-side
   resource use without bound.**

4. **A capability answer thrown away because one *unrelated* query went unanswered.**
   *Symptom* — a terminal that can draw real pixels shows a coarse two-pixels-per-cell picture
   instead, for the whole session, with nothing in the UI to say why. Silent: the frontend
   reports `halfblocks` perfectly honestly, having genuinely chosen it.
   *Caught by* — no test ROM, and no test of ours before this: the framebuffer is identical
   whichever protocol draws it, which is the same blind spot as the three above.
   *Contract violated* — `CSI 16 t` is an **optional** XTWINOPS report
   ([XTerm Control Sequences, "Window manipulation"](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html));
   nothing licenses treating its absence as "this terminal has no graphics protocol". Primary
   DA and the kitty `a=q` handshake are the capability questions, and they had already been
   answered. `ratatui-image`'s `Picker::from_query_stdio` returns `Err(NoFontSize)` and drops
   the lot.
   *New test* — `a_cell_size_is_recovered_when_csi_16t_is_unanswered`,
   `a_terminal_that_advertises_sixel_is_chosen_over_halfblocks`,
   `the_picker_prefers_sixel_over_halfblocks_and_kitty_over_sixel`; reproduced end-to-end over a
   PTY (old code `halfblocks`, new code `sixel`, same simulated terminal). Conformance
   candidate: **a missing optional report must degrade only the thing it reports.**

## Graphics-protocol selection (`src/screen/termcaps.rs`, `tui::probe_graphics`, `tui::build_picker`)

- **The ladder is kitty → sixel → iTerm2 → half-blocks, and half-blocks is the LAST resort**,
  never the default for "not kitty". Kitty is cell-addressed *and* does shared memory; sixel
  draws real pixels but re-sends the whole picture; iTerm2 is cell-addressed but **cannot be
  probed** (env-var guess only), so it ranks below the two that can be. Full write-up with the
  measured numbers: [`docs/rendering.md`](docs/rendering.md) §4a.
- **Do NOT use `Picker::from_query_stdio()` — it discards a good capability answer.** It returns
  `Err(NoFontSize)` when the terminal reports no cell size, and the caller is then left with
  `Picker::from_fontsize((10,20))`, whose protocol is `Halfblocks`. So a terminal that
  advertised sixel *or kitty* but does not answer `CSI 16 t` silently loses its protocol for the
  whole session. It also queries stdout/stdin, which `crossterm` owns. We ask on `/dev/tty` in
  the same round trip as the shm handshake (`probe_graphics`, called from `probe_terminal`
  **before** the shm probe, which returns early on `GB_TERM_KITTY=noshm`).
- **`CSI 14 t` ÷ `CSI 18 t` recovers the cell size when `CSI 16 t` is silent.** VTE answers
  neither `CSI 16 t` nor `TIOCGWINSZ`'s pixel fields (`xpixel=0 ypixel=0`), so both of
  ratatui-image's sources fail; window-px ÷ window-cells gives its real 9×20 cell instead of a
  fabricated 10×20.
- **Fedora's VTE is built WITHOUT sixel — this is not a detection bug.** Measured on VTE 0.84.1
  (Ptyxis 50.1, Fedora 44): `vte_get_features()` = `+BIDI +GNUTLS +ICU +SYSTEMD` (no `+SIXEL`)
  and DA1 = `ESC[?61;1;21;22;28c`, which has no `4`. GNOME Terminal / GNOME Console / Ptyxis
  therefore genuinely get half-blocks and there is nothing the emulator can do about it. **The
  authoritative check is `vte_get_feature_flags()` via `dlopen`, not the DA1 alone** — the
  `vte_terminal_*_enable_sixel` symbols exist either way.
- **Sixel's cost is 91.5 % a `ratatui` defect, not sixel.** Measured 125 KB/s (2,182 B/displayed
  frame) against half-blocks' 5.2 KB/s, but only 8.5 % of that is the picture (8.5 KB/image,
  ~1 image/s after dedup). The rest is the whole UI re-emitted every frame because
  `ratatui-image` puts the entire sixel payload in **one cell symbol** — bug #2 of the render
  conformance record above, still live for sixel because it has no cell-addressed placement to
  write out-of-buffer. Giving sixel the `TermImage` treatment would remove ~92 % of its cost.
- **`Sent` in the status row used to be a hard-coded `0 B` on the sixel/half-blocks path.**
  Those protocols are drawn by `ratatui-image` through the buffer, so their bytes only exist at
  the backend's writer — hence `CountingWriter` around stdout (`WROTE`/`wrote_total`). Never
  report a transport figure the code cannot actually observe.
- **`transport_label` takes the protocol** because "no `TermImage`" has two causes: sixel and
  half-blocks have no cell placement (`full` — the whole picture every frame), versus the
  `render_path = legacy` escape hatch (`legacy`). Calling both `legacy` misreported one.
- **Config `protocol` is the ONE override** (`auto|kitty|sixel|iterm2|iterm|halfblocks|blocks`);
  do not add a second knob. It now *validates* — a typo used to be stored and then silently
  ignored by the frontend, which reads as "forcing a protocol does nothing".
  `config::PROTOCOLS` and `tui::protocol_from_str` are pinned together by
  `every_configurable_protocol_is_understood_by_the_frontend`. **Pinning any value disables
  detection**, which is how a terminal ends up worse than it can do.
- **How to test protocol selection without a terminal: drive the frontend over a PTY you create
  and *be* the terminal.** A ~150-line Python harness (`pty.fork`, answer `CSI c` / `CSI 16 t` /
  `CSI 14 t` / `CSI 18 t` / kitty `a=q` / `CSI 6 n` / `CSI 5 n` from a captured profile, read
  `GB_DEBUG_LAYOUT`) reproduces every case deterministically. Three traps: **answer `CSI 5 n`**
  or ratatui-image's own query times out and every terminal looks capability-less; make the
  master **non-blocking** or a keystroke feed parks the harness in `write()`; and **`p` is
  PAUSE**, `[` is the palette key — sending `p` stops the frontend drawing and the run measures
  nothing.
- **A real VTE terminal can be driven headlessly, with no window on anybody's desktop:**
  `gtk4-broadwayd :7` then `GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 ptyxis -s -- <cmd>`
  (`-s` = standalone, so it never attaches to the user's running instance), and screenshot it by
  pointing a browser at `http://localhost:8087`. This is how the VTE captures above were made.
- **Sixel output can be verified without a sixel terminal**: decode the DCS bytes the binary
  actually wrote (sixel is deterministic) and look at the PNG. Done for dmg-acid2 — 738×660,
  12.8 KB of sixel, text crisply legible where half-blocks smears it.

## Frame transport — how a frame reaches the terminal (`src/termimage.rs`, `src/shmframe.rs`, `src/deflate.rs`)

- **Full write-up: [`docs/rendering.md`](docs/rendering.md)** — the ladder, the measured
  numbers, the terminal-compatibility table, and the conformance record. Read it before
  changing anything here. The earlier [`docs/terminal-render.md`](docs/terminal-render.md)
  fixed the *size* of the transmitted image; this fixed the *transport*.
- **The cost that matters is the terminal's, not ours.** Capping resolution left 92 KB of raw
  RGBA (~123 KB base64) per frame = **5.5 MB/s** for WezTerm to parse — **6 fps** while the
  core held 59.73. Our own path measured 2,395 fps because the bench wrote to `/dev/null`.
  Never conclude "the render path is fast" from a headless number again; use
  `render_bench --tty`, which drives the real terminal and uses a **`CSI 6 n` round-trip per
  frame as a barrier** (WezTerm does *not* answer `CSI 5 n`; CPR is the portable fence).
- **Measured ladder** (213×55 window, `--motion` so nothing is deduped):
  kitty raw RGB + placeholders **118,788 B** → iTerm2 uncompressed PNG **93,093** → kitty
  indexed PNG + placeholders **27,725** → iTerm2 indexed+deflate PNG **2,022** → kitty
  indexed PNG + direct **1,448** → **kitty shared memory + direct 81 B** (encode
  0.103 ms → **0.057 ms**, i.e. compressing/handing over is *cheaper* than base64-ing).
- **Kitty `t=s` (shared memory) is the real fix, and WezTerm supports it** — confirmed by
  handshake, not by guessing (`render_bench --probe`). The frame is `memcpy`d into a POSIX
  shm object as `f=32` RGBA (our framebuffer's exact layout) and only the *name* goes on the
  wire. **The terminal unlinks the object after reading**, so an object can never be reused
  across frames — a fresh one per frame is required, not a wasteful choice.
- **`Drop` is not cleanup — again.** A `SIGKILL`/`panic = 'abort'`/crash leaves a 92 KB
  object in `/dev/shm` forever, so `shmframe` **sweeps stale `gb-<pid>-*` objects on first
  use**. Liveness is `kill(pid, 0)`; **`EPERM` means alive**, only `ESRCH` means gone — get
  that wrong and the sweep deletes a running emulator's frames.
- **THE WEZTERM TRAP, do not rediscover it:** WezTerm implements the Kitty graphics protocol
  but **not** its Unicode placeholders, so the placeholder grid renders as missing-glyph
  boxes and *no picture* — which is why this project was on the iTerm2 fallback at all.
  **Direct placement is now the Kitty default** (`GB_TERM_KITTY=placeholder` restores the
  grid). It is also far cheaper: placements are deleted whenever image data is re-transmitted
  under an id, so the grid costs 4 UTF-8 bytes per covered cell **every frame** (25 KB at
  122×55) and can never be cached on screen.
- **Ask the terminal, never infer from `$TERM`.** `termimage::shm_query`/`shm_reply_ok` build
  and read the protocol's `a=q` handshake; `tui::probe_shm_support` runs it **once, after
  `enable_raw_mode` and before the event loop owns stdin** (crossterm parses input into
  `Event`s and drops the APC reply, so this reads the fd directly — nothing else may). A
  **CPR fence is appended** so silence is a conclusive "no". Anything not an explicit `OK`
  means no; guessing yes shows a blank rectangle.
- **Indexed colour is most of the win even without shm.** A DMG frame is 4 shades: 2-bit
  palette PNG is 5,904 bytes before compression vs 69,120 as RGB. `PngEncoder` picks 1/2/4/8
  bpp from the frame's actual colour count (cap `MAX_PALETTE`, then truecolour fallback);
  `deflate.rs` is a hand-rolled fixed-Huffman LZ77 (no new dependency, wasm-safe).
- **Unchanged-frame dedup was always working**; the *benchmark* hard-coded
  `frames_identical: 0` into its telemetry sample.
- **NEVER quote a transport figure from a static-image ROM.** dmg-acid2 is a still picture, so
  it dedups 299/300 and reports ~6 B/frame — that measures the deduplicator, not the
  transport, and it is the best possible case. Always report **two numbers separately**:
  *bytes per SENT frame* (compression; roughly constant per transport) and the *dedup hit
  rate* (entirely scene-dependent). Real Pokémon Blue figures, kitty PNG: overworld standing
  still 2 B/frame @ 99.8 % dedup; **walking 652 B/frame @ 53.2 %**; Oak intro 430 B @ 85 %;
  `--motion` 1,448 B @ 0 %. The per-sent-frame column barely moves (1.3–2.9 KB), so
  `--motion` is a fair worst case. Drive real gameplay with `rom_script`'s `state` command +
  `render_bench --state … --walk` (states for Pokémon Blue take ~4,500 scripted frames of
  menu navigation to reach the overworld).
- **Do not overclaim the fix.** In *real* gameplay the old path was often already coping
  (155 fps walking, because dedup skipped 55 % of frames); it collapses only under **sustained
  every-frame motion**, which is what the captain's report was (his overlay showed 91 KB being
  *sent* per frame, i.e. zero dedup). Honest claim: bytes/wire fell ~1,000× unconditionally;
  the terminal no longer saturates; lighter scenes gain headroom, not a rescue.
- **The status panel must show small numbers, averaged.** `{:.0}K` renders 87 bytes as `0K`;
  and the *instantaneous* per-frame value reads `0B` on every deduplicated frame (almost
  always on a static screen). `Sent`/`tx` are now a **one-second average over displayed
  frames** via `human_bytes_f`, and `Proto` names the live transport (`shm`/`png`/`legacy`) —
  a transport negotiated at runtime that nobody can see is one nobody can debug.
- **Capability probes are fragile in three specific ways — all found only by screenshotting
  the real frontend, all silent (they just cost you the good transport):** (1) never probe on
  stdout/stdin, `crossterm` owns them and the reply never arrives — use `/dev/tty`;
  (2) **drain** stale input first, or crossterm's own startup keyboard query's reply is read
  as the answer to yours; (3) **knock first** with `CSI 6 n` until the terminal answers, since
  a terminal launched *for* this process is not yet parsing input. With all three: 6/6 cold
  starts negotiate. Missing any one: intermittent silent fallback to PNG.
- **SCREENSHOTS ARE POSSIBLE — the old "screenshots are blocked" note is wrong.** It only
  tried the X *root* window, which is empty under Wayland. Run the frontend on **XWayland**
  (`env -u WAYLAND_DISPLAY DISPLAY=:0 wezterm-gui …`) and capture the window **by id**:
  `WID=$(DISPLAY=:0 xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1);
  DISPLAY=:0 import -window $WID shot.png`. `import`/`magick`/`xprop` are installed; there is
  no `xwininfo`/`xdotool`/`grim`. This is how the probe bugs above were found.
- **Never point the emulator at `~/roms` in a test** — the MBC writes `.sav` next to the ROM.
  Copy ROM *and* save into `target/` first (the captain's Pokémon Blue save was destroyed
  once already; the `.gbsave` files there are still all-zero).
- **`timeout` breaks terminal tests.** GNU `timeout` runs the child in its own process group,
  so any `tcsetattr` (i.e. crossterm's `enable_raw_mode`) raises SIGTTOU and **stops** the
  process — which then ignores SIGTERM, so the harness hangs and the app never renders. Use
  `timeout --foreground`, or no timeout.
- **Escape hatches:** `GB_TERM_KITTY=shm|noshm|placeholder`, `GB_TERM_IMAGE=rgb` (the
  *pre-compression* payload, so one binary can measure before/after on the same terminal).
- **Half-block rendering is NOT implemented** — scoped with arithmetic in
  [`docs/rendering.md`](docs/rendering.md) §2. Note the trap: naive half-blocks are ~484 KB
  per frame, *worse* than what we started with; all of the value is in cell-level
  differencing against a retained back buffer, which `TermImage`'s whole-frame hash cannot
  provide.
- **Measuring against a real terminal, hard-won mechanics:** `tcsetattr` from outside the
  foreground process group raises SIGTTOU and **stops** the process (a stopped process
  ignores SIGTERM, so even `timeout` cannot clear it); merely ignoring SIGTTIN makes reads
  fail with **EIO**, so every reply looks like a timeout. Ignore SIGTTOU and `tcsetpgrp` to
  claim the terminal. Also set the tty **non-blocking**: a terminal that has stopped draining
  its pty parks a blocking 91 KB write beyond the reach of `timeout`, and reporting
  `terminal_saturated` is the measurement we actually want.

## Telemetry — NDJSON run data (`src/telemetry.rs`)

- **Schema contract: [`docs/telemetry.md`](docs/telemetry.md).** Everything the stats overlay
  shows a human is also emitted as newline-delimited JSON, so a change can be verified without
  anyone watching the screen: fps (displayed *and* emulated), `speed_pct`, the per-stage
  millisecond breakdown, `tx_bytes`, dropped/identical frame counts, ROM/MBC/protocol/palette/
  upscale/scale, and audio queue depth + underruns.
- **Off by default**, one `Option` test per frame when disabled. `GB_TELEMETRY=<path>` (or the
  `telemetry` config key; the env var wins), `GB_TELEMETRY_EVERY=<n>` to sample. **The TUI
  refuses `stdout`** — it would corrupt the alternate screen — and says so.
- One-shot mode for CI: `render_bench --json` runs N frames headless and prints a single
  summary object. The invariant worth asserting: **`speed_pct` stays at 100 whatever `fps`
  does.** Falling `fps` with `speed_pct` 100 is correct (display frames dropped); `speed_pct`
  below 100 is a real bug and would break audio too.
- Audio stats come from `CpalPlayer::queue_handle()` with a **non-blocking `try_lock`** —
  `sound.rs` is not modified, and telemetry never stalls the render loop.
- This is the first implementation of the *emit* half of
  [`docs/debugging.md`](docs/debugging.md)'s extension surface (the "frame rendered" event).
  Bump `telemetry::SCHEMA_VERSION` if a field is renamed, retyped or removed; additions are free.

## What the TERMINAL costs per frame — and why `fps` used to read 59 forever (`src/screen/termcost.rs`)

- **Full write-up, the measured ladder and the conformance record:
  [`docs/rendering.md` §11](docs/rendering.md).** Read it before touching the TUI's pacing.
- **`fps` counted draws WE issued, and nothing we do can make that number fall.** With the
  kitty shm transport a frame is ~87 bytes and ~0.2 ms of our CPU, and 87 bytes never fills a
  pty buffer — so the whole of the displayed frame a *user* experiences happens after our
  timers stop. That is how "reports 59 fps but is clearly rendering slower at fullscreen" went
  unexplained. **A counter that cannot go down is not a counter.**
- **THERE IS NO PRESENT COUNT. Do not go looking for one again.** The kitty graphics
  protocol's response acknowledges the *command* (received + decoded); the spec defines no
  display acknowledgement. Write back-pressure measures nothing at 87 bytes. A DSR/CPR round
  trip **is** a fence — the terminal answers only after working through everything written
  before it, so it includes decode + upload + composite — but it still cannot report a frame
  the terminal *coalesced* and never painted. So the honest number is a **ceiling** (how long
  it takes to accept one frame), it is labelled as one everywhere it is shown, and the fix is
  to stop issuing above it so `fps` becomes *true* rather than merely lower.
- **The cost is the composited AREA, not the bytes.** Measured on WezTerm/5120×2880, kitty
  `t=s`, every frame changed: wire flat at 86–88 B while the terminal goes 0.04 ms (0.29 Mpx)
  → **2.97 ms at fullscreen (7.2 Mpx)** — **≈0.42 ms per megapixel**. The fixed round-trip
  floor is 3.35 ms at 80×24 and 3.39 ms at fullscreen (flat, so subtracting a constant is the
  right correction) and is measured once at startup by `probe_terminal`. **On this box the
  terminal is not the bottleneck at any size that fits the screen** (~337 fps at fullscreen;
  reaching a 16.74 ms budget would take ~40 Mpx). The captain's report did not reproduce here
  — the deliverable is the instrument that answers it on *his* box, and paces if the answer is
  below 59.7.
- **THE MEASUREMENT TRAP, do not repeat it: read `TIOCGWINSZ` AFTER the terminal has
  answered, never before.** A window asked for more cells than the screen holds is clamped,
  and a terminal launched *for* this process reports the size it was asked for until it has
  done so. Sizing the placement from that early value asks the terminal to composite an image
  mostly off-screen, and the cost then rises with a rectangle nobody can see. That produced a
  confident, wrong "30 ms/frame, ceiling 29 fps" first time round; a "640×176" window on this
  display is really **268×60**. `render_bench --tty` now reads the size after its first CPR.
- **The fence is SAMPLED, never per frame** (`TerminalCost::observe`): on a layout change and
  every `RESAMPLE_AFTER` (3 s) thereafter, minimum of 3 fences, samples over 500 ms discarded
  as an occluded window. A fence blocks, and blocking the loop every frame would starve the
  audio queue (cushion ~41 ms) *and* add its own 3.4 ms round trip to every displayed frame —
  fencing per frame is exactly why `render_bench --tty` tops out at 143 fps at fullscreen when
  the terminal itself can do ~337. Consequence to remember: a sample taken while the screen
  was static measures the *idle* marginal cost, which is optimistic; the 3 s re-check is what
  makes it follow the scene.
- **Use `crossterm::cursor::position()` for the fence, NOT `/dev/tty`.** It reads through
  crossterm's own event source, which requeues non-matching events (`skipped_events` in
  `crossterm/src/event/read.rs`), so a keypress arriving during the wait is delivered
  afterwards. Reading `/dev/tty` directly is right at *startup* — before the loop owns stdin,
  which is what `probe_terminal` does — and would silently eat keys anywhere after that.
- Pacing is one line: `next_draw_at(t, our_cost.max(termcost.draw_interval(FRAME_TIME)))`.
  Display frames are dropped; **emulated frames never are**. `GB_TERM_FENCE=off` opts out.
- Status panel gains a **`Term`** row (`1.8ms ok` / `30.3ms ≤33fps` / **`n/a no CPR`**);
  telemetry gains `terminal_ms`, `terminal_ceiling_fps` (**`null`, never `0`** when
  unmeasured — "we did not ask" and "it is free" are opposite conclusions) and
  `frames_issued`. Additions only, so no `SCHEMA_VERSION` bump.
- **`cell_scale` does NOT enlarge anything that reaches the terminal**, despite what the HiDPI
  note further up reads like. It multiplies the font size fed to the picker and the fit
  divides by the same font, so the cell rectangle is *identical*; the payload is 160×144 at
  every window size. All it moves is the pixel figure the status panel reports (and the legacy
  `ratatui-image` path, where the encoded image really is sized in pixels). Pinned by
  `cell_scale_does_not_change_what_the_terminal_is_asked_to_composite`.
- Kitty offers **no cheaper placement for an unchanged-but-rescaled image**: re-transmitting
  data under an id deletes that id's placements, and a frame that is *not* re-transmitted is
  already free (zero bytes, zero terminal work). There is nothing between the two cases.
- Adding the `Term` row is exactly the pressure the trainer crew's note further down warns
  about: **the Status box must stay sized from its own line list** (`status.len() + 2`), never
  a hand-counted constant, or the last row — a *plugin's* row — is clipped and a plugin is
  active and invisible. Two crews reached that fix independently on the same day from
  different rows; that is a sign it is the right one.

## `Gameboy::frame` is capped at one frame's worth of cycles (`FRAME_TICKS_CAP`)

- `Gpu::do_cycle` returns immediately while the LCD is off, so `gpu.updated` never fires and
  `frame`'s loop had **no other exit**. For a ROM that leaves the screen off it did not return
  *at all* — proven by deleting the cap, at which point
  `a_frame_with_the_lcd_off_still_lasts_exactly_one_frame` times out. For a game that switches
  the screen back on later it returned after **845,836** cycles instead of 70,224 — twelve
  frames of emulation *and twelve frames of audio* charged to one 16.743 ms slot, which is why
  playback discards ran to ~10,000 in the first 20 s of a cold boot (`docs/audio.md` §4c).
- The cap is `70224 + 64`. The margin means that with the LCD **on** the `updated` break
  always arrives first and the cap is unreachable, so it is byte-for-byte the old behaviour —
  which is what makes it safe to apply unconditionally rather than peeking at the LCD. Only
  the LCD-off case changes. Verified: `testroms`, `audiocapture`, `savestate`, `battery` and
  `trainer` baselines all unchanged.

## Config parsing — fail loudly

- **Every path-valued key must expand `~`.** There is no shell between `config.toml` and the
  program, so a `~` a user types is a literal. Only `rom_dir` expanded it, which is why the
  `gen1_trade = ~/mewtwo.trade` in `docs/plugins.md` failed with "cannot read
  ~/mewtwo.trade". `expand_tilde_str` now covers `gen1_trade`, `telemetry`, `screenshot_dir`,
  `video_dir`, `video_device` and `video_out`; `every_path_key_expands_a_leading_tilde` pins
  the lot. Add a new path key to that test as well as to the `get(...)` list.

- `config.toml` invites TOML syntax and TOML has **two** string forms. Single-quoted values
  (`palette = 'green'`) used to be dropped silently while a numeric key beside them applied —
  which reads exactly like "the feature is broken". `unquote` now strips both, and
  `Config::parse_reporting` returns a complaint for **every** key that is unknown or whose
  value could not be applied. Warnings go to stderr before the alternate screen opens, to the
  picker footer, to a toast on entering a game, and to the `?` help overlay.
- When adding a config key, add it to the `get(...)` list in `Config::parse_reporting` — keys
  not read there are reported as unknown.

## Repo furniture & attribution (fork of raphamorim/gameboy)

- **This is a fork of [raphamorim/gameboy](https://github.com/raphamorim/gameboy)** (itself
  modelled on [rboy](https://github.com/mvdnes/rboy)), now at **Alchemy86/gameboy**, MPL-2.0.
  Attribution is a **standing requirement, not decoration**: keep the original author credited
  in `Cargo.toml` `authors` (alongside "gameboy contributors") and in the README's
  **"Credits & lineage"** section + the top-of-README fork note. `repository` points at
  Alchemy86/gameboy. Do not strip upstream copyright or the MPL-2.0 `LICENSE` text.
- **Versioning:** pre-1.0 SemVer, minor may break. Today's big accuracy/audio/tooling day is
  `0.2.0` (was `0.1.5`). `CHANGELOG.md` (Keep-a-Changelog) starts at the fork divergence and
  does **not** reconstruct upstream history; its preamble names the starting point.
- **`.github/FUNDING.yml` was removed** — it pointed sponsorship at the upstream author from
  this fork (misleading), and inventing the fork owner's sponsor account would be worse. If a
  real GitHub Sponsors account exists for this fork, add it back deliberately.
- **No `CODE_OF_CONDUCT.md` (deliberate).** A Contributor Covenant is only meaningful with a
  real enforcement contact/process; on a personal fork with no community yet it would be
  performative filler. Add one when there's an actual moderation commitment behind it.
- **Third-party licence hygiene (in `CONTRIBUTING.md`): facts & formats yes, incompatible code
  no.** Learn hardware behaviour, file/packet **formats**, protocol knowledge and header
  constants from any source and reimplement cleanly; never copy source under an MPL-2.0-
  incompatible licence. Concrete cases: Scale2x reimplemented from its rule (GPL-2.0 ref),
  xBRZ/hqx excluded; GPL tooling like **FlashGBX** (GPL-3.0) — take formats/constants only,
  never its code or bundled data (e.g. `db_DMG.json`). Same for link-tool/protocol repos being
  studied (PokemonGB_Online_Trades, GB-Link, Poke_Transporter_GB): protocol knowledge is fair,
  implementations are not ours to lift.
- **Verified fresh-clone quick start (2026-08-13):** with the ALSA env shims unset,
  `cargo build --release --example terminal`, `--example desktop`, and
  `cargo build --release --target wasm32-unknown-unknown` all build with **no** system audio
  library (audio is off by default). CI now also builds wasm and runs `make docs-check`.

## Raw video-out + screenshots (`src/video.rs`)

- **Full guide, measured costs, platform limits and the captured proof:
  [`docs/video-out.md`](docs/video-out.md).** The pixels half of the EMIT surface —
  `telemetry.rs` publishes a frame's *numbers*, `video.rs` publishes the frame. Off by default;
  `video_tick` returns on one `Option` test and does not even read the clock.
- **The stream is fed from `step_frame` (emulated frames), never from the draw.** The terminal
  deliberately drops *display* frames on a big window (`docs/terminal-render.md`); a capture may
  not lose those frames with it. Test:
  `the_video_stream_is_fed_by_emulated_frames_not_displayed_ones`.
- **`rawvideo` has no header and no timestamps**, which dictates two rules that must not be
  broken: (1) **geometry is fixed when the stream opens** — a differently-sized frame (the user
  cycled the upscaler) is *letterboxed* into it, never resized underneath the consumer;
  (2) **frame number is the clock** — `realtime` pacing emits one frame per 16.743 ms of wall
  time, so fast-forward *drops* and pause/slow-mo *duplicate*. `emulated` pacing (one frame per
  emulated frame) is for headless capture and is deterministic.
- **The TUI cannot stream to stdout** — `ratatui` owns it. It refuses with a pointer to a FIFO
  or to `examples/video_out.rs`. The **desktop** frontend can (`--video-out -`), and that is the
  headline pipe command.
- **THE CAMERA IS 16:9 (`v4l2::DEFAULT_SIZE = 1280x720`) AND THAT IS LOAD-BEARING.** Reported as
  "the feed crops the game — the dialogue box is cut mid-line", and diagnosed on arrival as our
  scaler filling instead of fitting. **It is not.** `fit_size`/`fit_maps` have letterboxed
  correctly since the first video commit; at 640×480 they put the game at exactly 480×432 with
  80/24 bars — pixel for pixel the `scale=480:432,pad=640:480:80:24` the old ffmpeg bridge did,
  and an `ffmpeg -f v4l2` grab off the device shows the whole picture. **The crop is the
  consumer's**: a video-call client lays the camera out in a 16:9 tile and fills it, losing
  12.5 % off the top and 12.5 % off the bottom. Measured in real Chrome against the real device:
  `getUserMedia` reports **`resizeMode: "none"`** and hands the page the whole 640×480 frame, so
  it is not a track-level crop and *nothing this end can detect it*. Letterboxing "better"
  cannot help either — the consumer crops the bars and the picture with them. **The only fix a
  camera has is to be the shape of the tile.** 1280×720 is also the exact fit: 144 × 5 = 720.
  Cost 1.83 ms/frame vs 0.77 at 640×480, paid only while live; `768x432` (×3) is the cheap 16:9
  fallback. An explicit `video_size` is still honoured — `v4l2::aspect_warning` names the
  consequence instead of overruling it, on the `v` toast and in `?`.
  **Do not "restore" 640×480 as the default, and do not go looking for a fill-vs-fit bug in
  `fit_size` — it is not there.** Reproduce either direction with the two `video_out` commands
  in `docs/video-out.md` §4a.
- **`fit_maps` samples at pixel CENTRES** (`(2i+1)*s / 2t`), not left edges. Identical for
  integer magnification, so the crisp ×3/×5 path is untouched; it matters when the frame is
  *smaller* than the source, where edge sampling never reaches the last source column/row and
  leans the whole picture towards the top-left. `fit_never_crops_at_any_size_or_aspect` pins it
  by symmetry (mirroring the output must mirror the source) across sixteen size combinations —
  a band/extent check alone cannot tell edge from centre sampling and gives a test with no teeth.
- **The idle card is drawn into the game's rectangle, not edge to edge.** It used to fill the
  frame while the game was letterboxed, so pressing `v` made the picture appear to grow and
  shrink. `VideoOut::card_size` re-fits from `game_fit` (the last game frame's *source* size, so
  a driver re-negotiating the geometry is picked up), and the ordinary letterbox path centres it
  — `fit_size` applied to an already-fitted size is always ×1, so it lands in exactly the same
  box. Pinned by `the_idle_card_lands_in_the_same_rectangle_as_the_game`.
- **`examples/idle_card.rs` renders through the real stream path** (`VideoOut::in_memory`), with
  `--game` for the comparison frame. It used to call `card::draw_idle` into a full-size buffer
  directly, which is *not* what ships and is exactly why the full-bleed card went unnoticed.
- **The feed is the game picture only, and the product now says so.** No panels, by design —
  they are text the terminal rasterises and the terminal is not in this pipeline. The `?`
  overlay says it, `docs/video-out.md` opens with it, and the TUI sets the window title to
  **`TerminalGB`** (`window_title`, stable prefix + cartridge) so an OBS Window Capture source
  can find it. Route decision and the scene recipe: `docs/stream-panel-routes.md` — re-checked
  and still OBS window capture → OBS Virtual Camera, **do not build the text compositor**.
- **Careful when inserting a function above another one in `tui.rs`:** `load_session` carries a
  `#[cfg_attr(not(feature = "audio"), allow(clippy::let_unit_value))]`, and an item dropped
  between the attribute and `fn load_session` silently moves the allow onto the new function.
  `cargo clippy --all-targets` (no audio) is what catches it.
- **The emulator DRIVES `/dev/videoN` natively now** (`video::v4l2::sink`) — the old "we refuse
  a V4L2 node and hand you an `ffmpeg` recipe" note is dead. `video_out = "/dev/video10"`, press
  `v`, pick the camera in Chrome/Meet/Discord/OBS. `tools/virtual-camera.sh` still works and is
  still the answer on macOS/Windows or a box with no v4l2loopback, but it is not the Linux route.
  **This box DOES have v4l2loopback** (`/dev/video10`, `card_label="Game Boy"`, `exclusive_caps=1`,
  made permanent via `/etc/modules-load.d` + `/etc/modprobe.d`); the old "no v4l2loopback here"
  note was stale. `/dev/video1` and `/dev/video2` are a real UVC webcam.
- **A write that returns success proves NOTHING on a V4L2 node.** An un-negotiated `write(2)` is
  accepted and produces nothing a consumer can read — this exact mistake was made and reported
  as a working camera. The only acceptable proof is *a consumer decoding the device*
  (`ffplay -f v4l2 -i /dev/video10`, or `ffmpeg -f v4l2 -i … -frames:v N out_%02d.png` and
  actually looking at the PNGs). Same discipline as the terminal-render screenshots.
- **The sequence is QUERYCAP → S_FMT → write**, and **what `S_FMT` returns is authoritative**:
  the driver may change the geometry, and `bytesperline` is the row stride which is *not*
  necessarily `width * 2` (frames are re-strided if it differs), `sizeimage` is the exact
  write length.
- **Raw `ioctl`s through the existing `libc` feature, no new crate — deliberate.** `v` and
  `v4l2-sys-mit` are permissive but link **libv4l2**, a *system* library: precisely the
  fresh-clone-fails trap that ALSA/`cpal` already cost this repo. Hand-written UAPI structs are
  made safe by a **compile-time** check: an `ioctl` number encodes `sizeof` its struct, so
  `VIDIOC_QUERYCAP`/`VIDIOC_S_FMT` are computed from our definitions and `const`-asserted
  against the kernel's published `0x80685600`/`0xc0d05605`. Get a field wrong and the build
  stops. Gated `#[cfg(all(target_os = "linux", feature = "libc"))]`.
- **`exclusive_caps=1` makes a loopback node report CAPTURE while a producer is attached.** So
  "this is a capture device" means two different things and must not be one error: on a *real*
  webcam it is permanent (wrong node); on a loopback node it means **someone else is already
  streaming to it** and clears on its own. Told apart via sysfs — a loopback node canonicalises
  under `/sys/devices/virtual/`, a USB/PCI camera never does (`v4l2::is_loopback`). The same
  sysfs fact fixed `tools/virtual-camera.sh`, whose device search checked `device/driver` — a
  symlink loopback nodes do not have, so it reported "no loopback device" on a box that had one.
- **A v4l2loopback device cannot be renamed while it exists, and this is now checked, not
  assumed**: `/sys/class/video4linux/videoN/name` is `-r--r--r--` root-owned, the only writable
  sysfs attributes are `dev_debug`/`format`/`max_openers`, `modinfo` lists no other route, and
  upstream `v4l2loopback-ctl` has `add`/`delete`/`set-fps`/`set-caps`/`set-timeout-image` and no
  rename. `VIDIOC_S_FMT` negotiates a format, never an identity. **So the camera cannot be named
  after the game from inside the emulator** — it needs delete+add as root, which drops every
  consumer, and a per-launch privilege escalation is not something to ship. Pick a static
  `card_label` at setup; `video_device = /dev/videoN` still *verifies* label vs cartridge title.
- **Naming:** `clean_title` (human/device label) and `file_safe_title` (filename) live in
  `video.rs` — cartridge titles are 16 padded bytes and can hold anything. A recording is named
  `TITLE-stamp-WxH-fmt.raw` so the consuming `ffmpeg` arguments can be read off the filename;
  `%t`/`%s` expand in any configured path; a directory destination gets the same treatment.
- **`v` toggles the stream/camera live, `V` records a timestamped file**, `c`/`C` (and `F2`) take screenshots — displayed look and raw
  framebuffer. Screenshots use the **selected** upscale factor, not `transmit_factor` (with
  `transmit_scale = auto` those differ on purpose), and go through the `image` crate, not
  `termimage::png_rgb_into` — the latter is *stored*-deflate (uncompressed), right for a 60 Hz
  wire and wrong for a file someone keeps.
- Costs measured over 3000 frames (emulation alone 0.216 ms/frame): rgba 160×144 **+0.007 ms**,
  yuyv 640×480 +0.55 ms, yuyv 640×480 with the LCD upscaler +1.58 ms — all inside 16.74 ms.

### NO MEDIA SINK MAY EVER BLOCK THE EMULATOR (the defect this cost us)

- **`video_out` pointing at a FIFO hung the emulator at startup**, in `open(2)`, *before the
  terminal UI existed* — one line of output and a process only `kill` could end (`wchan` =
  `wait_for_partner`). It was configured exactly as our own docs told people to. The old note
  here said writes are "blocking: a stalled consumer back-pressures the emulator" as if that
  were a design; it is the same mistake the audio player made (`docs/audio.md` §4c), and it is
  now forbidden for every sink.
- **Three rules, all enforced in `src/video.rs`:** (1) the open never blocks — a FIFO is
  `O_NONBLOCK` and `ENXIO` is the *state* `StreamState::Waiting`, and a camera's
  open/`S_FMT` are bounded driver calls; (2) it is **retried every `RETRY_EVERY` (250 ms)**
  while armed, so emulator-first and consumer-first are equally fine, and a consumer that quits
  (`EPIPE`) drops back to `Waiting` and the next one is picked up; (3) writes are non-blocking
  and a slow consumer **loses frames** (`VideoStats::stalled`), never frame time.
- **`SIGPIPE` is set to `SIG_IGN` before the first byte.** Rust's runtime does this for `bin`
  targets but the FFI build is a `cdylib` where it has not run, and a consumer quitting is a
  *routine* event here.
- **`rawvideo` is positional, so a partial write must be finished before the next frame starts**
  — otherwise a consumer that got 40 % of frame N and then frame N+1 is desynchronised for the
  rest of the run. `Fifo::tail` holds the remainder; `VideoOut::idle` drains it whatever the
  pacing mode.
- **Chosen non-blocking over "defer the open until `v` is pressed"** because deferring only
  moves the hang: a configured stream auto-arms at startup, and pressing `v` inside the
  alternate screen would hang with the UI up and no way to cancel. Non-blocking also makes late
  attach fall out for free.
- **`Intent` (`Stream` vs `Record`) is derived from what the destination *is*** — a FIFO or a
  V4L2 node is a stream, a regular file is a recording — and the Status row now says **which**
  and **where** (`off` · `◌ wait video10` · `● LIVE 12M video10` · `● REC 12M …raw`). The row
  used to say `LIVE` for a recording, which is how "the camera is broken" turned out to be
  "frames were going to `~/Videos` in the wrong format all along". **`v` streams, `V` records** —
  two jobs, two keys.
- **A FIFO stream publishes a sidecar descriptor `<fifo>.gbvideo`** (`video::descriptor`) with
  width/height/format/fps/title, because `rawvideo` carries none of it in-band and the numbers
  were otherwise configured twice in two programs and silently decoded as garbage when they
  drifted. `tools/virtual-camera.sh` reads it. A V4L2 destination needs none of this: it
  negotiates, and it **ignores `video_format`/`video_size`** (YUYV, 640×480 default) rather than
  keeping a default that cannot work.
- Regression tests to keep: `a_configured_fifo_with_no_consumer_does_not_stop_the_emulator_starting`
  (tui), `a_fifo_with_no_reader_does_not_block_startup`,
  `a_consumer_that_arrives_late_is_picked_up`,
  `a_consumer_that_leaves_drops_back_to_waiting_and_can_be_replaced`,
  `a_stalled_consumer_costs_frames_never_emulation_time`,
  `a_partial_write_is_finished_before_the_next_frame_starts`,
  `a_missing_camera_does_not_stop_the_emulator_and_says_what_to_install`,
  `the_v4l2_ioctl_numbers_match_the_kernel_headers`.
- Telemetry gained a `"video"` object (`frames`/`duplicated`/`dropped`/`bytes`/`ms`) — an
  addition, so no `SCHEMA_VERSION` bump. The invariant to assert: `video.frames ≈ elapsed ×
  59.7275` under `realtime` pacing, whatever `fps` does.

### The camera is a persistent output; streaming the game into it is a MODE

Four faults, one root cause — *configuring a destination*, *sending frames to it* and *holding it
open* were all the same flag. Full write-up in [`docs/video-out.md`](docs/video-out.md) §5b/§6.

- **A configured `video_out` must NOT stream at launch.** It used to open the sink in
  `tui::open_video` before the terminal existed, so a terminal with a camera configured came up
  already feeding it (`Video ◌ wait gameboy-cam.…` before the user pressed anything). Nothing is
  opened now — not even created, since opening a plain file *truncates* it and would silently
  destroy the last recording. `v` goes live; `video_autostart` / `--video-start` /
  `GB_VIDEO_START` is the separate opt-in. The desktop frontend has no `v` key, so it prints
  what to add rather than arming something unreachable.
- **NEVER close a `v4l2loopback` device to stop streaming — you cannot get it back.** Every
  distribution builds it with `exclusive_caps=1`, which makes the node advertise
  `V4L2_CAP_VIDEO_OUTPUT` only while it has no consumer. Let go of a device someone is watching
  and `VIDIOC_QUERYCAP` answers CAPTURE and `VIDIOC_S_FMT` answers **`EINVAL`** for as long as
  the viewer stays open. Measured: on/off/on worked *once*, then the camera was gone. So
  `VideoOut::go_idle`/`resume` hold the fd and switch what is *sent*. `must_stay_open()` is
  `intent() == Stream`: close a recording (that is what makes it a finished file), hold anything
  live. A FIFO counts as live — its consumer is often `virtual-camera.sh`, whose `ffmpeg` owns
  the camera, so an EOF kills the device one process further out.
- **Idle must be a trickle, never silence.** With nothing sent, a consumer displays the **last
  frame forever** — the captain's Meet call sat on a title screen while the game was in a Rattata
  battle. A dead feed that looks live is worse than a lost one. `IDLE_PERIOD` (2 Hz) pushes a
  generated card instead; it is converted once and cached, so a tick is one `write(2)`.
- **`video::card` is a hand-written 5×7 bitmap font + `Canvas`, deliberately ours.** No vendored
  asset and therefore no third-party font licence to audit in an MPL-2.0 repo. Draw at the
  frame's own geometry, not at 160×144 magnified. It is also the groundwork for compositing the
  status panel — see [`docs/stream-panel-routes.md`](docs/stream-panel-routes.md), which
  recommends **OBS window capture instead of building that renderer**.
- **`v` is the live feed and only ever the live feed.** It used to fall back to *recording a
  file* when nothing was configured, so "stop the stream" answered `REC stopped: 268 frames →
  ~/Videos/gameboy` at someone thinking about his camera. `v` now says what to set; `V` records.
- **A camera is never "waiting for a consumer".** A V4L2 output node takes writes whether or not
  anyone is watching, so it is never blocked on a reader the way a FIFO genuinely is — but the
  FIFO's *words* were used for every `Waiting` stream, which sent the captain hunting for a
  consumer problem when the real one was a second emulator holding `/dev/video10`. `waiting_toast`
  reports the device's own reason for a camera.
- **`convert()`'s scaling tables are keyed on the SOURCE size too**, not just the output: the
  idle card and a `display`-source game frame are different sizes going to the same output.
- **Verify this path with a real consumer or not at all.** `examples/video_toggle_soak.rs` mirrors
  the frontend's decisions through the public API; `GB_SOAK_NO_PARK=1` restores the old
  always-release policy, which is how it is shown to have teeth (every cycle after the first
  fails). Decode what the consumer *received* — `ffmpeg -f v4l2 -i /dev/video10 -pix_fmt gray -f
  rawvideo`, then classify frames — rather than trusting a return code. Measured: 1,028 frames
  over 34.3 s alternating game/card across ten cycles, every transition inside one 30 fps frame,
  `stalled=0`, fds flat, and `fuser /dev/video10` empty after exit.
- **This box HAS v4l2loopback** (`/dev/video10`, card label `Game Boy`) — the older note saying
  it does not is stale. `/dev/video1`/`video2` are a real webcam. Beware: the captain's own
  session and Chrome's `VideoCaptureService` both hold `/dev/video10` at times, and a second
  producer is exactly what makes our sink report `Waiting`. Check `fuser -v /dev/video10` before
  concluding anything about the device.

- Add durable project-specific notes here as they are discovered through real work.

## Licence boundary: format knowledge is ours, game assets are not

**Facts about a game — addresses, structure layouts, character maps, compression formats,
constants — are usable freely and must be cited. The game's *assets* are not ours at any
price.** Concretely, for anything that reads or decodes a commercial cartridge:

- **Never vendor sprite data, pixel arrays, tile sheets, music, text corpora or any other game
  asset into this repo — including as a test fixture.** Extract them at runtime from the user's
  own ROM (or from VRAM), never from a copy we ship. This applies to golden files and hashes of
  assets used as fixtures just as much as to the assets themselves.
- **Disassemblies are reference material.** [`pret/pokered`](https://github.com/pret/pokered)
  and friends are the authority for addresses and formats; cite the *file and symbol* for every
  address used, which is what turns a magic number into something a reviewer can check. That is
  the same standard as the conformance convention at the top of this file.
- **Reference implementations are read, not copied.** Reimplement from the documented format in
  our own idiom, the way Scale2x was (see the upscaler section) — and check the licence before
  drawing on anything concrete. The Ironmon Tracker (MIT, Lua, Gen 3) informed *which data
  earns screen space*; none of its code is here.
- When a check genuinely needs a real asset to compare against, take it from a path the user
  supplies at run time (`POKERED_GFX=<dir>` in `testharness/trainer.rs`) and skip the check when
  it is absent. Never commit the asset to make the test hermetic.

(The FlashGBX GPL note and the licence-hygiene section in `CONTRIBUTING.md` are being added on
another branch; when those land, this belongs beside them.)

### Inherited pictures in `resources/` are third-party and mostly unlicensed

The same rule applies to **images we inherited with the fork**, and it caught two that were
being embedded in the README. Both are gone; the finding is that `resources/` is upstream's
attic, not ours.

- **`jsgb-mmu-map.png`** (removed) — the memory-map figure from [Imran Nazar's *GameBoy
  Emulation in JavaScript*](http://imrannazar.com/GameBoy-Emulation-in-JavaScript:-Memory).
  That page's footer reads *"Content and design by Imran Nazar, 2006 – 2026"* and states no
  licence. Replaced by **[`docs/diagrams/memory-map.svg`](docs/diagrams/memory-map.svg)**, ours,
  used by the README and [`docs/components/mmu.md`](docs/components/mmu.md).
- **`internals-cpu-section.gif`** (removed) — Jeff Frohwein's DMG schematic sheet from
  [devrs.com](http://www.devrs.com/gb/hardware.php#hardgb), credit burned into the image, no
  licence stated. Now a link, beside the [gbdev wiki
  mirror](https://gbdev.gg8.se/wiki/articles/DMG_Schematics).
- **Still in the tree, unreferenced by anything, same question unanswered:**
  `dmg-main-board-schematic-circuit.png`, `zilog-Z80.jpg` (same provenance) and upstream's own
  screenshots `demo-pokemon.gif` (2.5 MB of Pokémon gameplay), `demo-web.png`, `demo-*.png`,
  `gbproject.png`, `resources/tests/*.png`. Deleting upstream's assets is the captain's call,
  not a side effect of a README change — but **do not embed any of them** without settling it.
- **Attribution is not a licence.** The README already carried "(c) Jeff Frohwein" *as the
  caption of the image it was reproducing*. Naming the owner is not permission from them.
  A **link** has never needed a licence; an embedded copy does.

### Verifying that a README or doc actually renders

**Render it through GitHub's own renderer and look at it** — this is how the wrong caption in
`docs/components/frontends.md` was caught, and a unit test on prose could not have.

```bash
python3 -c "import json;print(json.dumps({'mode':'gfm','context':'Alchemy86/gameboy','text':open('README.md').read()}))" > /tmp/md.json
gh api -X POST /markdown --input /tmp/md.json > /tmp/body.html
# wrap in <article class="markdown-body">, stylesheet from
# cdn.jsdelivr.net/npm/github-markdown-css@5/github-markdown-{light,dark}.css
# serve from the REPO ROOT so relative image paths resolve as they do on GitHub,
# then screenshot with chrome-devtools-axi and read the picture.
```

- Put the preview HTML at the **same directory depth** as the doc, or `../shots/` hops resolve
  against the wrong directory and everything looks fine when it is not.
- Count broken images programmatically as well as looking:
  `[...document.images].filter(x => x.naturalWidth === 0)`.
- **Caveat: `mode=gfm` renders the input as a GitHub *comment*, so newlines become `<br>`.**
  Paragraphs look hard-broken in the preview and reflow on the real README. `mode=markdown`
  reflows correctly but drops tables, which this README needs. Judge images and layout from the
  preview, not line wrapping.
- **Theme:** a `prefers-color-scheme` media query *inside* an SVG loaded via `<img>` resolves
  against the **OS**, not GitHub's theme setting — except that GitHub's dark stylesheet sets
  `color-scheme: dark`, which Chrome propagates into the embedded SVG. So both paths happen in
  the wild. `memory-map.svg` is therefore drawn on its **own explicit panel** with a light
  default plus a dark override, and all three combinations (light/light, light/dark,
  dark/dark) were rendered and confirmed legible. Never leave an SVG's background transparent
  and its text black.
- Delete the preview artefacts before committing (`.preview/`, `readme-*.html`,
  `*.preview.html`) — none of them belong in the repo.

## Live trainer view — the read-only plugin (`plugins/trainer/src/`)

- **Full design, address table, verification and follow-ups: [`docs/trainer.md`](docs/trainer.md).**
  A read-only plugin that reads a running Pokémon Red/Blue cartridge and shows the party,
  badges, the battle opponent's stats and a decompressed sprite beside the game. `T` toggles it
  in the TUI, `d` is the per-Pokémon detail view, `Tab` cycles the party.
- **It implements the same `Plugin` trait as the link partner** — see the plugin-contract
  section below. There is deliberately **one** contract, not a read-only one beside a
  read-write one: this plugin declares `OBSERVE | READ_MEMORY` and nothing else, so
  `Machine::poke` refuses it. The read-only guarantee is the *host's*, enforced per call,
  rather than a borrow-checker consequence of handing out `&Gameboy`.
- **The two gaps this consumer reported are now closed**, which is the main reason the
  contract has the shape it does. (1) `on_event(&mut self, &Event, &mut Machine)` passes the
  machine alongside the event, so a plugin can read state *in response to* an event — the
  original `DebugSink::on_events` sketch could not. (2) `Machine::peek_bank(bank, addr)` reads
  an explicitly numbered ROM bank whatever the game has mapped, backed by
  `MemoryBankController::readrom_bank` (one line per MBC over the shared `banked_rom_byte`
  helper); it does **not** touch the bank registers, so it cannot perturb the running game.
  The trainer still takes the cartridge image in `TrainerPlugin::new` because `rom::RomTables`
  *scans every bank* to locate tables by signature — a different need from reading a known
  table out of a known bank, which is what `peek_bank` is for.
- **`Gameboy::peek` is the `&mut self`, exact read** (`MemoryManagementUnit::debug_peek`:
  reuses the normal `rb` dispatch with the bus tick suppressed, so banking, echo RAM and I/O
  read-back are all correct). `MemoryManagementUnit::peek` is a separate `&self` variant that
  cannot reach the I/O range and returns `0xFF` for `$FF00-$FF7F`; prefer `debug_peek`.
- **Addresses are derived, not looked up.** `pret/pokered` was built at its pinned RGBDS 1.0.3
  and the ROMs checked against its `roms.sha1`; the captain's cartridge is byte-identical to
  `pokeblue.gbc`, so the build's own `pokeblue.sym` is exact for it. Every constant in
  `plugins/trainer/src/gen1.rs` carries its symbol name. `pokered.sym` gives identical addresses, which
  is why one table serves Red and Blue. **Re-derive rather than edit by hand.**
- **Three traps that produce plausible-but-wrong data** (all handled, all worth remembering):
  Gen 1 species indices are **not** Pokédex numbers (`MonsterNames` is indexed by internal
  index, `BaseStats` by dex number, converted via `PokedexOrder` — as `GetMonHeader` does, Mew
  special-cased); every HP and stat is **big-endian**; and `party_struct` (44 bytes) and
  `battle_struct` (28) put level and stats at **different offsets**.
- **Sprites come from the ROM, not VRAM**, because VRAM only holds what is on screen and a
  party panel would be blank in the overworld. `plugins/trainer/src/sprite.rs` implements Gen 1's
  picture compression from the format documented in `pokered`. Proven **byte-identical** to
  `pokered`'s own 2bpp output for 13 species covering every "Pics" bank and both special cases.
- **The sprite is a SECOND terminal image, and the enemy team strip is a THIRD.** Ids
  `0x00676202` and `0x00676203` against the game's `0x00676201`; each has its own cell
  rectangle and its cells marked `skip`. All three rules from `docs/terminal-render.md` apply
  to both. They are drawn **after** `render_display`, because `render_display` clears
  `pending_wire` for the frame.
- **ONE focus drives the picture and the numbers** (`trainer_view::Focus`, `focus_ring`,
  `focus_at`). The sprite used to switch to the opponent on its own while the stat block kept
  rendering the player's party, so during a battle the panel showed one Pokémon's face over
  another's statistics — and the opponent's own stats were unreachable, which is most of what a
  tracker is for. `Tab` walks a ring that **starts at the opponent in a battle**;
  `App::trainer_sel` indexes the ring, not the party, and snaps to 0 on a battle transition
  (`App::trainer_fighting`) because the ring changes length underneath a stale index.
- **`skip` cells are never ERASED, which is a ghosting trap.** A cell marked `skip` is left
  alone by ratatui's diff, so when a layout moves an image the text that used to be underneath
  is still on the terminal. An opaque sprite hid this; a **transparent** one shows it straight
  through as half-eaten words. Rule: mark `skip` only on frames where the picture is *not*
  being retransmitted (`!placement.changed`) — on a frame where it is, `pending_wire` is
  flushed after `terminal.draw` and covers the text anyway. And a picture that stops being
  drawn must be explicitly deleted (`TermImage::erase`, Kitty `a=d,d=i`) plus a full repaint:
  **a Kitty placement lives until it is deleted** and redrawing the cells under it does not
  remove it.
- **Sprite backgrounds are transparent via a FLOOD FILL FROM THE EDGES, never a colour key**
  (`sprite::background_mask`). Gen 1 art uses the lightest shade *inside* the figure — eyes,
  teeth, Charmander's belly, the core of its flame — so keying on the colour punches holes
  through the middle of the Pokémon. Measured: Charmander keeps 98 interior lightest-shade
  pixels, Squirtle 100, Pikachu 281. Four-connected, not eight, or the fill leaks diagonally
  between touching outline pixels. Alpha reaches the terminal as `f=32` RGBA on the shm path
  and as a **`tRNS` chunk** on both PNG paths; `PngEncoder` keys its palette on RGBA now, and
  an all-opaque frame emits no `tRNS` so the game picture's bytes on the wire are unchanged.
  Transparent pixels keep their opaque RGB and only carry `A = 0`, so a transport that drops
  alpha degrades to today's opaque box rather than to garbage.
- **The opposing trainer's roster** is read from the enemy party block, which mirrors the
  player's byte for byte (`ENEMY_PARTY_COUNT` `$D89C` / `ENEMY_PARTY_SPECIES` `$D89D` /
  `ENEMY_MONS` `$D8A4`, the same `+1` / `+8` offsets — pinned by
  `the_enemy_block_mirrors_the_player_block`). **Nothing is displayed unless the block is
  self-consistent**: the species list must agree with the `party_struct` species beside it, two
  independent copies the game writes at different times. Verified live against the captain's
  cartridge in the Oak's-Lab rival battle. Drawn as **one** wide strip image, not six — six ids
  and six PNG encodes for six-cell-wide pictures is not a trade worth making. The trainer's own
  *portrait* is deliberately NOT shipped: `TrainerPicAndMoneyPointers` is not among the derived
  symbols and guessing an address is exactly what the refusal machinery exists to prevent.
- **Stat stages: 7 is neutral, and an all-zero block is NOT "-6 on everything".** The block is
  zeroed WRAM until `InitBattleVariables` writes the 7s, so a sample on the first frames of a
  battle reads `0` everywhere — observed live printing `YOU ATK-6 DEF-6 SPD-6 SPC-6` for a
  Pokémon nothing had touched. `StatMods::is_valid` refuses anything outside
  `MIN_STAT_MOD..=MAX_STAT_MOD` (1..=13). The panel shows the **stage**, not a reconstructed
  "before" value: the multipliers are truncating integer ratios (`-1` is 66/100, `+1` is 15/10,
  so up and down do not cancel) and the unmodified value is not recoverable by dividing back
  out. Stages are only ever read from the battle structures, never from a party copy — they do
  not survive a switch.
- **Gen 1 capture odds were DISASSEMBLED from the cartridge, not looked up**
  (`plugins/trainer/src/capture.rs`). `ItemUsePokeBall` sits in bank 3 and is found by its
  `ld a,200` / `ld a,150` threshold pair, which occurs exactly once in the ROM (`03:570B`).
  Three things a modern formula gets wrong: (1) **`jr c, .loop` at `03:570E` jumps BACKWARDS
  to `call Random`** — the ball threshold is a *resampling loop*, not a failure, so a Great
  Ball makes the first byte uniform on `0..=200` and that is help, not a hurdle; (2) the
  status bonus is **subtracted** (`sub c` / `jp c` at `03:5729`) and a byte below it catches
  outright — 25 for sleep/freeze, 12 for anything else; (3) the shake count from `03:578D` is
  **cosmetic**, computed after the outcome is decided. Consequences: a Great Ball beats an
  Ultra Ball at full HP (divisor 8 vs 12) and loses to it at low HP (bound 200 vs 150), and
  catch rate 255 at 1 HP with a Poké Ball is a *guaranteed* catch. The quoted probability is a
  closed form checked against a literal transcription by enumerating every reachable random
  pair. `wEnemyMonActualCatchRate` is `$D007` — **not** the species' base catch rate, because
  Safari bait and rocks move it; harness check [5] asserts the two agree in a real wild
  encounter, which is the one address claim the feature rests on.
- **`GB_TRAINER_STATE=<path>` on `cargo test --release --test trainer`** writes a save state at
  the first real wild encounter. A wild encounter costs a scripted walk to reach, so that is
  far cheaper than building a second route when one is wanted for a screenshot.
- **The picture and the trainer panel are ONE composition** (`compose_block`), not two
  independently-placed columns: adjacent, one shared vertical extent, centred together. Three
  separate captain reports came from that not holding. `fit_metrics` centres the picture in
  its own column, so pinning the panel to the window's right edge put *all* the horizontal
  slack in a band **between** them; and centring only the panel inverted the vertical
  mismatch rather than fixing it. The block height is the taller of what each half needs, so
  neither is cropped for the other, and `PanelLayout::centre_in` centres the panel's content
  in its half exactly as `fit_metrics` centres the picture in its own. **When the panel wants
  more rows than the pane has it stays top-aligned** — centring an over-height panel pushes
  its header off the ceiling, and the header is the part that says whose panel it is.
- **"The panel should scale with the game" — text cannot, so three things do.** The panel
  takes **two fifths of the width** rather than a fixed `PANEL_W`; `sprite_rows` grows the
  sprite band with that width (the sprite is an image, so it genuinely scales); and
  `party_cols` grows the name/bar/type columns above the design width instead of stranding
  the space. A fixed 40-column panel beside a picture that grows with the window reads as "a
  footnote pinned next to a poster", which is exactly how it was reported.
- **The controls and plugin state live on a ONE-ROW BAR across the top** (`render_top_bar`),
  not along the bottom and not inside the trainer panel. The bottom row is the row a terminal
  clips when its pixel height is not a whole number of cells, which is how the controls were
  being lost; and plugin state inside the panel competes with the party for the panel's actual
  job. **It costs no rows**: it replaces the title row, and the bottom row it freed is now
  taken only while a toast is up. `fit_bar` shrinks each segment through its own ladder of
  complete shorter forms before dropping any of it — never truncate a control hint, because a
  clipped hint is indistinguishable from a complete one.
- **`trainer = on` in `config.toml`** opens the panel at startup. Also the only reliable way
  to get the panel up when screenshotting through `wezterm cli send-text`, which does not
  deliver every key.
- **Screenshot geometry is not what you asked for unless you check.** `wezterm-gui --config
  initial_cols=N --config initial_rows=M` is honoured *intermittently* (the window manager
  remembers geometry), and a stale window from an earlier run is easy to capture by mistake.
  Read `term_cells=` back out of `GB_DEBUG_LAYOUT` and retry until it matches; `CSI 8 t` sent
  through `wezterm cli send-text` does **not** resize, because that writes to the pane's
  stdin, not to the emulator.
- **Driving a real cartridge into a real battle, headlessly:**
  `cargo run --release --example trainer_layout -- --rom <rom> --to-battle`. Mashing Start/A
  plays Gen 1's intro on its own; a **per-map-biased** random walk then finds the doorways
  (Red's house 2F wants Up for the stairs, 1F wants Down for the door, outside wants Up), and
  in Oak's Lab **never press B** — it answers NO to "do you want it?" and loops the scene
  forever. That reaches the rival's trainer battle in ~760 rounds. The same example renders the
  panel from a fixture at any width (`-- 40 34 28 24`) and dumps the sprites with their alpha
  plus an ASCII mask (`--sprites <dir>`).
- **`wezterm cli send-text` does not deliver every key** to a TUI that has pushed the kitty
  keyboard-enhancement flags — `T`, digits, `Tab` and `F8` arrive, `d` and `t` do not. That is
  the injection method, not the product: `the_trainer_keys_toggle_detail_and_walk_the_focus_ring`
  drives the real dispatcher and passes. Verify key handling in-process; use the terminal for
  what only it can show (images, colour, alignment).
- **Cost discipline:** nothing is read per frame — the plugin samples every 6 emulated frames
  (~10 Hz) and **only while its panel is visible** (`set_active`). Sprites decompress once per
  species and cache. A session with no plugin attached pays one `is_empty` test per frame.
- **Graceful refusal is a feature.** The cartridge is identified by title **and header global
  checksum**, so a hack, translation or randomiser with a shifted layout is refused with an
  explanation rather than displayed wrongly. Yellow (`pret/pokeyellow`, same structures at
  different addresses) and Gen 2 (`pret/pokecrystal`, different structures) are named as
  extensions, not pretended.
- **Derived values are the point** (`plugins/trainer/src/derived.rs`): shiny-if-traded-to-Gen-2 from the
  DVs a Gen 1 Pokémon already has (`CheckShininess`, pokecrystal `engine/gfx/color.asm` —
  def/spd/spc all exactly 10, attack DV with bit 1 set; note perfect 15s are **not** shiny),
  Hidden Power type and power, stat experience, DV total. **Gen 1 has no Poké Ball field at
  all** — that concept arrives in Gen 2 and must never be inferred. Gender needs a Gen 2
  gender-ratio table that is not in the cartridge; named as a follow-up.
- **How the harness proves it** (`cargo test --release --test trainer`): it plays a **new** game
  to the overworld, drives the START menu to SAVE so the *game* writes an internally consistent
  battery file, edits a known party into `sPartyData` and fixes `sMainDataCheckSum`
  (`CalcCheckSum`, `engine/menus/save.asm`; the battery file is `<rom>.sav`, **raw cartridge
  RAM with no header** — `src/mbc/battery.rs` — so SRAM bank 1 starts at file offset
  `0x2000`), boots into it, and then compares the panel against **the game's own party
  menu** — `wTileMap` (`$C3A0`, 20×18) holds charmap codes directly, so decoding it reads back
  what a player sees. A hand-built save **hangs** the game: its map/tileset/sprite state is not
  consistent. The harness copies the ROM *and* its save into `target/` first, because it
  presses buttons at menus.
- **Reading the game's own screen (`text::decode_screen` over `wTileMap`) is a general tool** —
  any future test that wants to check emulator state against what the game displays can use it.

## Plugin system — the workspace layout, and why the API is its own crate

- **Layout.** The contract is [`crates/gameboy-plugin-api/`](crates/gameboy-plugin-api/)
  (no dependencies); each plugin is its own crate under [`plugins/`](plugins/) —
  `trainer`, `gen1-link`, `battle-control`, `agent`, plus `example-tracer`, the one a
  stranger copies. The emulator's side of the contract is `src/plugin.rs`. Stranger-facing
  guide: [`docs/writing-a-plugin.md`](docs/writing-a-plugin.md); design rationale stays in
  [`docs/plugins.md`](docs/plugins.md).
- **The API crate is NOT tidiness — it is the only acyclic arrangement.** The TUI *draws
  panels* for the plugins it ships with, so `gameboy` depends on the plugin crates. If the
  `Plugin` trait lived in `gameboy`, plugins would depend back on it and Cargo would refuse
  the cycle. `MachineAccess` is the seam that made the split possible: the API crate declares
  what a machine must provide, `src/plugin.rs` implements it for `MemoryManagementUnit`, and
  `Machine` (the capability-checked handle) is defined over the trait, not over the MMU.
  **Never move the trait back into `gameboy`, and never let a plugin crate depend on
  `gameboy` outside `[dev-dependencies]`.**
- **A dev-dependency back-edge IS allowed and we use it.** `battle-control`'s tests need a real
  MBC, so it dev-depends on `gameboy` with the `testing` feature and uses
  `gameboy::testing::TestMachine` (`src/testing.rs`) — a real machine exposed only as
  `MachineAccess`, plus `bus_read`/`bus_write`/`advance_input`. Cargo permits a cycle whose
  back-edge is a dev-dependency; that is the escape hatch, and the only one.
- **`make no-plugins` is the gate, and it is in CI.** Builds core + both frontends with every
  plugin off, runs the core tests, and greps `cargo tree` to prove no plugin crate is in the
  graph — a `cfg` that silently stopped compiling something would pass a build and fail this.
  **Proven once by hand:** with all four plugin crates deleted and every mention removed from
  `Cargo.toml`, the emulator builds and runs a ROM.
- **Frontend integration is `cfg`-gated per plugin, with stubs.** `src/screen/tui.rs` gates ~20
  functions on `plugin-trainer` / `plugin-agent` / `plugin-battle-control` and supplies
  `#[cfg(not(...))]` stubs so callers stay untouched — a key whose plugin is absent toasts
  "not built in", it does not silently do nothing. `trainer_view.rs` / `agent_view.rs` are
  gated whole; `tui.rs` carries tiny stub `mod tv` / `mod av` for the layout constants the
  always-compiled code asks for.
- **Adding a plugin is three edits:** an optional dependency + a `plugin-*` feature in
  `Cargo.toml`, and one `if` in `attach_named` (`src/screen/tui.rs`). Add a row to `REGISTRY`
  beside it so a build without the feature says "not built into this binary — rebuild with
  --features X" rather than "unknown plugin".
- **CI's "the core has exactly one dependency" gate now reads "one THIRD-PARTY dependency"**
  and excludes first-party `gameboy-plugin-api`. That is not a loosening: the crate has no
  dependencies of its own and is the code that used to be `src/plugin/mod.rs` (now `crates/gameboy-plugin-api/`), so an embedder
  gains no new third-party code. Do not let anything else onto that list.
- **Trust model, decided: plugins are TRUSTED code the user compiled in.** Capabilities are an
  interlock on the *emulated machine* — they make declared intent checkable and keep "which
  plugin wrote to my save?" answerable — **not a sandbox**. A plugin can do anything a Rust
  crate in-process can. Sandboxing would need a wasm/subprocess boundary and a serialised
  protocol on the hot path; the wire-shaped `Event` keeps that possible later.
- **A panicking plugin is quarantined, not fatal** — `catch_unwind` around every call, marked
  `x` on the name, reported once via `PluginHost::take_failures`, never called again; a
  panicking link partner becomes an unplugged cable. **Honest caveat: `[profile.release]
  panic = 'abort'` means no `catch_unwind` catches anything in a release build.** Real in dev
  and test; do not panic in release.
- **Version boundary:** `API_VERSION` + `Plugin::api_version` (default body, never override),
  refused at `register` with `RegisterError::ApiVersionMismatch`. Under build-time linking
  Cargo already makes a mismatch impossible — the check is how compatibility is *declared* and
  the gate that matters if anything is ever loaded at runtime. Say that plainly rather than
  overclaiming. Real forward-compat comes from every `Plugin` method having a default body and
  `Event` being `#[non_exhaustive]` (so external matches need a `_` arm).
- **Runtime `dlopen` loading was considered and REJECTED** — Rust has no stable ABI, so a
  `dyn Plugin` across the boundary is UB without an all-`extern "C"` redesign that would make
  the API worse for the compile-time case everyone uses, and `wasm32` has no `dlopen`. If
  cross-language plugins are ever needed the answer is a socket/subprocess transport, not a
  dylib. Do not half-build it.
- **Separate repos: deliberately NOT yet.** The boundary that matters (own crate, own tests,
  depends on the published contract not on internals) is already there, so splitting one out
  later is `git filter-repo` plus a version bump — not a redesign. Doing it now costs every
  cross-cutting change four pull requests. The signal to split is a plugin wanting its own
  release cadence or maintainer.
- **`TrainerPlugin::set_encounter_for_test` is public API now**, not `#[cfg(test)]` — a
  `cfg(test)` item is invisible across a crate boundary, and the frontend that renders the
  panel is a different crate. That is the ordinary cost of a real boundary; the alternatives
  (a `testing` feature for one method, or deleting the frontend's test) were worse.

## Plugins & the Gen 1 virtual link partner

- **Full contract: [`docs/plugins.md`](docs/plugins.md).** A plugin is an **in-process Rust
  trait** (`Plugin`), not a dylib or a script — that is the only mechanism that works
  identically on `wasm32` (no sockets, no `dlopen`) and needs no new dependencies. Messages are
  wire-shaped anyway (`Event::write_json` emits the same NDJSON telemetry uses, and
  `GB_PLUGIN_TRACE=<path>` writes that stream), so a socket transport is additive. **Be honest
  about it: you cannot write a plugin in Python today.**
- **Capabilities are enforced, not advisory.** `OBSERVE / READ_MEMORY / WRITE_MEMORY /
  SERIAL_LINK`, declared by `Plugin::capabilities`; `Machine::peek` returns `None` and
  `Machine::poke` returns `false` for anything undeclared. Pinned by
  `capabilities_are_enforced_not_trusted`. At most **one** plugin may hold `SERIAL_LINK` (a
  cable has two ends). Nothing loads implicitly — only `plugins = …` / `GB_PLUGINS`.
- **The user must be able to see it.** The TUI Status panel gains a `Plugin` row whenever one is
  attached; `!` = may write memory, `!!` = has written (sticky). Attaching also raises a toast.
  The `?` help overlay lists **what plugins exist and how to arm them** — opt-in is only a
  feature if the options are findable, and before that the only place `gen1-link` was named was
  `docs/plugins.md`.
- **ONE contract, two very different plugins — do not add a second trait.** The trainer view is
  read-only and the link partner answers the cable, and both implement the same `Plugin`. The
  read-only guarantee is the *host's* (an undeclared `poke` returns `false`), not a
  borrow-checker artefact of handing out `&Gameboy`. There is one `PluginHost` and it lives on
  the `Cpu`, so `Gameboy::frame` dispatches the `Frame` event itself — **a frontend must not
  drive plugins**; `Gameboy::plugin::<T>()` / `plugin_mut::<T>()` are how it reaches a concrete
  plugin to render or configure it.
- **`Machine::peek_bank(bank, addr)` reads a numbered ROM bank whatever is mapped**, backed by
  `MemoryBankController::readrom_bank` (one line per MBC over the shared `banked_rom_byte`).
  It does **not** touch the bank registers, so it cannot perturb the running game. This closed
  the second gap the trainer crew reported; the first (`on_events` had no view) is closed by
  `on_event(&mut self, &Event, &mut Machine)`. Pinned by
  `a_plugin_can_read_a_rom_bank_the_game_has_not_mapped`.
- **`on_attach` runs on the first *dispatch*, not in `register`** — that is the first moment a
  `Machine` exists. Both `on_frame` and `on_serial_transfer` complete a pending attach, because
  a game can clock the link before its first frame boundary.
- **The trade is proved by a number we never sent.** The partner puts a level 5 Mew on the wire
  with **exp 125**; the party afterwards reads **135**. Gen 1's boosted experience for traded
  Pokémon, applied by the *game*, on the receiving side — a memory poke would have left 125.
  That single number is the difference between arriving *through* the trade path and *beside*
  it, and it is checked on every run of `examples/gen1_trade.rs`.
- **Two techniques worth stealing for any test that drives a real game** (both in
  `examples/gen1_trade.rs`): (1) **navigate by reading the screen**, not by counting frames —
  `wTileMap` (`$C3A0`, 20×18) holds charmap codes, so pressing A when the screen *says*
  `TRADE CENTER` is robust against animation length and emulation speed; (2) **redirect the
  room's own warp (`wWarpEntries`), never relocate the player** — map, tileset, sprite and warp
  state are mutually consistent state the game maintains together, so writing coordinates
  produces a save that looks right and hangs. Move the world's pointers, not the player.
- **`CableClubLeftGameboy` needs the player on the LEFT console tile facing RIGHT**
  (`engine/link/cable_club.asm`); the right tile is the other Game Boy's seat. That player also
  becomes clock master, which is the role the partner wants the game to have. A run that hangs
  at "Please wait…" is almost always the wrong seat.
- **Screenshots of our UI are documentation; game data is not. The line, decided deliberately:**
  `docs/link-trade/` **does** commit four screenshots of the terminal frontend mid-trade, and
  that is not a hole in the licence-boundary rule above — it is what that rule is for. The rule
  exists to stop us redistributing game *data*: bytes that substitute for owning the cartridge
  or that a program could consume as input (sprite tables, tile sheets, text corpora, fixtures,
  golden hashes of assets). A screenshot of our own chrome with a game visible inside it is
  none of those — it cannot be decoded back into assets and is of use only to a human reading
  docs. **The test when adding more: our interface, doing something, with the game incidental.**
  Not a gallery of game frames, and never anything a program could read back as data.
  `docs/upscale-samples/` stays MIT-ROM-only because those genuinely *are* asset comparisons.
- **`--shots <dir>` captures a PNG per stage transition of the headless run**, through the same
  `image`-crate path the `c` / `C` / `F2` keys use — `termimage`'s writer is *stored* deflate
  (right for a 60 Hz wire, wrong for a file someone keeps), so a screenshot must never use it.
  `--state <path>` writes a save state at the console the instant before the trade, which is how
  the frontend screenshots are reproducible: drop it in save slot 1, press F8, press A.
- **Driving the real TUI for screenshots is possible and worth it** (this is how the two status
  bugs below were found). `wezterm-gui --config enable_wayland=false start -- <cmd>` on
  XWayland, then `wezterm cli send-text --no-paste --pane-id 0` against **that instance's**
  socket (`WEZTERM_UNIX_SOCKET=/run/user/1000/wezterm/gui-sock-<gui-pid>` — the default socket
  belongs to the captain's session), and capture with
  `import -window $(xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1)`.
  Beware: `pgrep -f <pattern>` matches **your own command line**, so it reports processes that
  are already dead — check with `ps -eo pid,args | grep` instead.
- **Two "a plugin cannot be active and invisible" bugs, both only findable by screenshotting
  the real frontend** — a unit test on the row's *text* passes while the user sees nothing:
  (1) the Status box was a fixed `Constraint::Length(13)`, which fits exactly eleven content
  rows, and the plugin row is pushed **last** — so it was clipped away; the box is now sized to
  its contents. (2) `status_line()` joins all plugins with " | " and the row truncates to 18
  columns, so with the always-attached trainer view first, the *link partner* vanished behind
  the ellipsis. It is now **one row per plugin**, and the `!` / `!!` write marker lives on the
  **name** (`PluginStatus.name`) precisely so truncation of the detail can never eat the
  warning. Pinned by `an_attached_plugin_is_visible_in_the_rendered_status_panel`, which
  asserts against **rendered output** at three window sizes.
- **Plugin state is NOT machine state** and is not serialised. `Gameboy::load_state` calls
  `PluginHost::reset` — a link partner holding half a trade would otherwise desync silently.
- **Where it hooks:** `PluginHost` lives on `Cpu`, *not* in the MMU, so dispatch can pass
  `&mut MemoryManagementUnit` without a self-borrow. `Serial::wb` parks a declined transfer in
  `pending`; `Cpu::do_cycle` answers it **before** `MMU::do_cycle` folds `serial.interrupt` into
  `IF`, so timing is unchanged. An unanswered transfer is dropped (an unplugged cable), never
  answered late. `Plugin: Send` because the FFI build parks a `Gameboy` in a `static Mutex`.
- **`MemoryManagementUnit::debug_peek/debug_poke`** reuse the normal dispatch with `bus_tick`
  suppressed, so an observer cannot advance the timer/APU and change the run it is watching.

### Gen 1 link trade — things to preserve

- **Answer `$01` with `$02` and the game takes the clock.** `ESTABLISH_CONNECTION_WITH_*` and
  `USING_*_CLOCK` share values (`$01`/`$02`) so the reply *is* the role assignment. Being the
  slave is why this needs **no timing tolerance at all**: the partner answers synchronously
  inside the `SC = $81` write. Every hardware/network bridge has to solve the clocked-timing
  problem; we sidestep it by choosing the other role.
- **Each block costs `N + 1` transfers.** `Serial_ExchangeBytes` re-sends its first byte until
  it receives `$FD`, so our stream leads every block with one extra preamble byte. Stream is
  **644 bytes** (18 + 425 + 201) for blocks of 17 / 424 / 200. Off by one ⇒ the party block
  shifts and the game receives garbage.
- **`AddEnemyMonToPlayerParty` copies the 44 bytes verbatim** — nothing is recomputed after a
  trade. Stats, experience-for-level, types, catch rate and PP must all be right *on the wire*.
  Multi-byte fields are **big-endian**. The HP DV is assembled from the low bits of the other
  four.
- **The patch list is not optional.** `$FE` is the protocol's "no data" byte and the receiver
  *skips* it, so every `$FE` in the 264-byte mon region is swapped for `$FF` and its 1-based
  offset recorded in a two-part list (split at 252). Silent corruption otherwise. Names cannot
  contain `$FE` either — the digit `8` — and that is refused with a clear error.
- **No game data is embedded.** Base stats, dex↔index mapping, species names and move PP are
  located in the player's own ROM by byte signature and cross-checked (Pikachu, THUNDERBOLT)
  before use. Mew's base stats live *outside* the main table in Red/Blue — handled explicitly.
- **`PokemonSource` keeps the *source* of the Pokémon separate from the protocol.** That is the
  load-bearing abstraction: a file, a converter, a remote peer and a live partner all become
  interchangeable, which is what makes cross-generation and remote trading possible later.
- **`$6F` — the cancel nybble — MUST be echoed from EVERY phase, and it is the only way out of
  the Cable Club.** The Trade Center has **no warp events at all**
  (`data/maps/objects/TradeCenter.asm`; `wNumberOfWarps` reads 0 at run time), so backing out of
  the trade-selection screen *is* leaving the room. `.cancelMenuItem_APressed`
  (`engine/link/cable_club.asm`) only reaches `ReturnToCableClubRoom` when the round comes back
  `$F`, and three things make this a whole-state-machine obligation rather than one branch of
  the selection round: `wSerialExchangeNybbleSendData` is a **latch** (later syncs keep sending
  `$6F` until `TradeCenter_SelectMon` resets it to `1`); a round is **11–12 transfers of the same
  byte** and the game keeps the *last* one; and a wrongly-answered round is **retried from
  whatever phase the partner has since moved into** — so answering it in one phase only makes the
  *second* attempt unanswerable. Symptom of getting it wrong: the player presses A on CANCEL for
  ever (measured: 2,475 presses, `wLinkState` pinned at `$32`), game rendering and clock running.
  `LinkPartner::feed_cancel` handles it uniformly; **`Exchanging` is the one exclusion** — inside
  a block `$6F` is party data and must come from our stream. A cancel also must **not** count as
  a trade (in `Confirm` it used to fall into the accept arm and answer `$62`).
- **`CloseLinkConnection` never reaches the plugin — do not go looking for it there.** It puts
  `$02` in `SB` and starts an **externally** clocked transfer (`SC = $80`), and `Serial::wb`
  only offers a byte to a plugin when `SC & $81 == $81`. That is correct: a real partner that is
  not clocking transfers nothing, and the game's status stays `CONNECTION_NOT_ESTABLISHED`.
- **`GB_GEN1_LINK_TRACE=1` records the tail of the wire** (`Gen1LinkPartner::trace()`, last 4096
  `(sent, replied, phase)` triples). Every interesting link failure is a *reply to one byte*, so
  reason from the trace, not from the state diagram.
- **A quick-load is a legitimate mid-session desync, and the cancel is the escape from it.**
  F5/F8 in the TUI call `PluginHost::reset`, dropping the partner to `Phase::Handshake` while the
  game is still linked (by design — plugin state is not machine state). That is exactly why the
  cancel must work from an unexpected phase. Reproduce with
  `examples/gen1_trade.rs --quick-load`.
- **A player who is stuck in the Cable Club can safely reset.** `TradeCenter_Trade` calls
  `predef SavePartyAndDexData` right after "Trade completed!" — pokered's own comment is *"this
  allows reset into Pokecenter"* — so the received Pokémon is already in SRAM and a reset returns
  the player to the Pokémon Center with it.

### Verifying a trade (`examples/gen1_trade.rs`) — hard-won gotchas

- **Gen 1 draws the whole screen through the WINDOW layer** (`LCDC` bit 5, `WY=0`, `WX=7`), not
  the BG map. Reading `$9800` returns 20×18 spaces and every menu looks empty. Because the game
  writes character codes straight into the tilemap and loads the font so tile id == char code,
  **the tilemap is readable as text** — that is what lets a headless bot navigate menus instead
  of replaying a brittle button script.
- **Save files are `<rom>.sav`, raw cartridge RAM with no header** (`src/mbc/battery.rs`), so a
  file offset is an SRAM offset. Cost a lot of debugging when it was the older `.gbsave`, which
  put an 8-byte RTC epoch *in front* of the RAM and silently shifted every offset by 8.
- **Continuing a save deliberately skips most of `LoadMapHeader`** (it returns early while
  `BIT_NO_PREVIOUS_MAP` is set), so the map header, the block-map view pointer, the border block
  and the warp/sign/NPC tables are all *saved* state. Writing a different `wCurMap` over the top
  renders one map through another's tileset and stride, and walking onto a stale warp coordinate
  crashes the game. **Don't relocate the player by hand** — redirect the room's own warp instead
  and let the first real warp do the full load.
- **`wCurMap` is populated by `TryLoadSaveFile` at the top of the main menu**, long before
  CONTINUE is chosen, so it cannot mean "we are in the game". Same trap during Oak's speech.
- **`CableClubLeftGameboy` requires the player to be facing RIGHT** and to be the clock master.

## Battle control — starting a Gen 1 battle from outside (`plugins/battle-control/src/`)

**Full technique + guarantees: [`docs/gen1/battle-control.md`](docs/gen1/battle-control.md);
the whole area index is [`docs/gen1/`](docs/gen1/README.md).** Read those before touching this.

- **The whole trigger is one byte.** `OverworldLoop` (`home/overworld.asm`,
  `.checkForOpponent`) polls `wCurOpponent` (`$D059`); nonzero → `NewBattle` → `InitBattle`.
  Below `OPP_ID_OFFSET` (200) it is a species (wild, level from `wCurEnemyLevel` `$D127`), at or
  above it a trainer class + 200 with the roster in `wTrainerNo` (`$D05D`). **This is what the
  game's own debug menu does** (`engine/debug/debug_menu.asm`) — not a discovered trick.
- **It is polled ONLY by the overworld loop.** Writing it inside a menu or a text box queues
  the battle silently. The first time this bit, a scripted run had mashed START/A through the
  title screen and the last START left the START menu open. `NewBattle` also refuses on a
  dungeon warp, during a cutscene, and where a map script set `BIT_NO_BATTLES`.
- **THE LEGITIMACY TRAP, and the reason "well-formed" is not enough:**
  `CheckForDisobedience` (`engine/battle/core.asm`) compares `wPartyMon1OTID` with `wPlayerID`
  and treats a mismatch as a **traded** Pokémon — a badgeless player is obeyed only to level 10.
  An injected level 50 Charizard with a made-up trainer id answers *"CHARIZARD won't obey!"* and
  wastes the turn. The injected party must inherit the running game's `wPlayerID` (`$D359`,
  big-endian) and `wPlayerName` (`$D158`). Generalise it: the game cross-checks between
  structures, so injecting into one without the other is correct for the state you created and
  wrong for the state you meant.
- **Patching state the engine is still writing is a race, and a delay does not fix it.**
  `LoadEnemyMonData`/`ReadTrainer` fill `wEnemyMon`/`wEnemyMons` inside `InitBattle`, not on a
  frame boundary. Keep writing until the bytes **read back unchanged at the start of a frame** —
  an observation, not a guess. Capped at 40 frames so a game that never settles reports it.
- **Save safety, enforced not promised:** every write goes through one door that refuses
  anything outside WRAM/HRAM (`wram::is_writable`) and *counts* refusals, so cartridge RAM —
  which is what a `.sav` is written from — is unreachable; the exact byte ranges written are
  snapshotted and restored when the battle ends; a preset with no `player =` line never touches the
  party — nothing to restore. Pinned by pointing the write door at `$A000` and checking the byte does not move.
- **Addresses are re-derived from the cartridge before the plugin is constructed**
  (`wram::verify`): `ld [nn],a` is `EA lo hi`, so the address is the operand. **Match whole
  code shapes, never short byte runs** — `3E 01 EA ?? ?? CD` and `F0 04 47 F0` each occur
  hundreds of times in a megabyte, and `Random_` contains its own four-byte prefix twice, so
  requiring global uniqueness of a prefix fails on the real ROM. `InitWildBattle` is searched in
  a 256-byte window after `InitOpponent` rather than cartridge-wide.
- **Cinematics are screen-driven, not frame-driven.** `await`/`press-until` compare against
  `wTileMap` decoded as text. `press-until` presses at the **end** of each period: Gen 1's
  `HandleMenuInput` delays several frames after accepting a button and does not see one that
  arrives inside that delay — which looks exactly like dropped input. **A and B are not
  interchangeable**: B closes menus and can never open one, so B before a move is chosen backs
  out of the move list.
- **Reproducibility, stated precisely.** `Random_` mixes `rDIV` in per call, so writing the seed
  cannot fix an individual roll; but emulation is deterministic, so *same starting state + same
  script ⇒ bit-identical frames*. **Measured: three runs, 1219 frames, FNV hash
  `dac74ad86f8e1854`, identical.** Design the clip so no roll can change it — and know that
  Gen 1's accuracy check gives even a 100 %-accuracy move a **1-in-256 miss**
  (`MoveHitTest`, and the disassembly says so in its own comment).
- **Run it:** `cargo run --release --example gen1_battle -- --check` (every preset resolves
  against the cartridge — worth having in CI), `--preset <name> --shots <dir>`, `--repeat 3`,
  `--video -`, `--save-state <file> --save-at <frame>`. It starts a **new game** (bedroom, no
  party — which is why presets supply one), so it needs no save file at all, and it copies the
  ROM into `target/` first. Verified: it leaves `~/roms` byte-identical.
- TUI: `plugins = gen1-battle`, then **`B`** fires the selected preset and **`N`** cycles.
  `battles = <path>` / `GB_BATTLES` for your own file.
- **`Capability::CONTROL_INPUT` is separate from `WRITE_MEMORY` on purpose** — pressing a button
  is something the game sanctions (every byte it changes, it changes through its own code); a
  poke is not. A plugin scripting a sequence should not have to ask for the second.

## The Gen 1 mechanics & hacking area (`docs/gen1/`)

- **[`docs/gen1/README.md`](docs/gen1/README.md) is the index and it owns the standards.** Linked
  from the README so it is discoverable. Pages: memory map, battle control, reading the screen,
  driving the link cable, sharp edges, and a stub for agent play.
- **Standards, because the value is that a reader can trust it:** cite `pret/pokered` by **symbol
  name** (a symbol survives; a bare hex address is wrong for Yellow and unsearchable); grade every
  address as *symbol file* / *walked* / *read out of the cartridge* and say which; explain the
  **mechanism** so a reader can invent the next technique rather than repeat ours; and never put
  game *data* in the repository (facts and formats yes, assets never — not even as a fixture).
- **A walk through `ram/wram.asm` is only usable if it ENDS on a second known symbol.** The
  enemy-party walk starts at `wEventFlags` `$D747` and lands exactly on `wPlayTimeHours` `$DA41`;
  that landing is the proof, and it must be written down beside the walk.
- **Two live investigations have homes waiting, named in the index**: the Colosseum link-battle
  scout → `link-from-outside.md` §"The Colosseum"; the button-pressing agent → `agent-play.md`.
  The Colosseum section already carries the disassembly's answer — `CableClubLeftGameboy`
  (`engine/pokemon/bills_pc.asm`) branches on **`wCurMap`** alone (`TRADE_CENTER` → trade, any
  other map → battle), so the partner is never consulted — flagged as *derived, not yet observed*.

## Conformance GitHub Action — scoring *foreign* emulators (`tools/conformance/`, `.github/actions/conformance/`)

- **Full contract, badge recipes, licensing position and honest gaps:
  [`docs/conformance-action.md`](docs/conformance-action.md).** This is the first shippable form
  of the standalone-tool goal in [`docs/conformance.md`](docs/conformance.md); that page is now
  design rationale, this one is what exists.
- **The adapter contract is `gbconform/1` and it must stay tiny.** The runner executes
  `<adapter> <rom>` with `GBCONFORM_{ROM,MODEL,FRAMES,TIMEOUT_S}` plus **at most two** output
  paths — `GBCONFORM_SERIAL_OUT` (raw link-port bytes) and `GBCONFORM_FRAME_OUT` (one frame).
  No protocol, nothing parsed from stdout, and **deliberately no way for an adapter to report a
  verdict** — verdicts come from the ROM. Every demand added here is a project that will not
  adopt it; resist growing it. Our reference adapter is
  [`examples/conformance_adapter.rs`](examples/conformance_adapter.rs) and is written to be read
  as documentation of the whole ask.
- **The two outputs are independent CAPABILITIES, and declining is `exit 0` + write nothing.**
  A crash or timeout during the capability probe means *assume the capability exists*, so every
  check runs and records a real error. Get that backwards and an emulator that segfaults on
  every ROM produces no files, reads as "no capabilities", and scores **0 out of 0** — which a
  badge renders as a clean slate. `test_an_adapter_that_crashes_is_an_error_everywhere_not_a_skip`
  pins it; it caught this exact bug during development.
- **Screen checks compare SHADE RANK (luminance order), never RGB.** Not optional politeness:
  our own frames are `000000/606060/c0c0c0/ffffff` and the upstream dmg-acid2 reference is
  `000000/555555/aaaaaa/ffffff`, so a literal compare fails on every run. `image.quantise_shades`.
- **Accept permissive frame formats.** PNG (colour types 0/2/3/4/6, bit depth 1–16), Netpbm
  `P6`/`P5` **and the ASCII `P3`/`P2`**, and raw RGBA/RGB/8-bit dumps. The `P3` support exists
  because binjgb writes it — assuming binary PPM rejected a perfectly good frame. The upstream
  acid2 reference is **2-bit greyscale** PNG, an uncommon shape a naive decoder gets wrong.
- **Guiding principle, worth defending: no check's expected output is defined by our emulator.**
  Serial verdicts are the ROM's own printed judgement; the acid2 verdict is its author's
  reference image. This is why Blargg's *screen-only* ROMs (`mem_timing-2`, `halt_bug`,
  `interrupt_time`, `oam_bug`) are NOT in the action — including them means shipping reference
  frames produced by us, which is a weaker claim. Read the verdict text off the frame instead.
- **ROM licensing (the constraint that shaped the design):** dmg-acid2 is MIT and vendored
  (`third_party/`), so the action already has it. **Blargg's ROMs carry no explicit licence** —
  freely distributed but not relicensed — so they are **fetched at run time** from
  `retrio/gb-test-roms` pinned to `c240dd7d…`, the same commit our baselines were made against.
  Never vendor them here. Commercial ROMs never, in any form.
- **Runner is Python stdlib only** (`tools/conformance/gbconform.py`, `image.py`) — hand-rolled
  PNG decode/encode and badge SVG, same dependency-light ethos as `testharness/`. Adding a
  dependency means every adopting project pays for it. Its own tests:
  `python3 -m unittest discover -s tools/conformance -p 'test_*.py'` (fake adapters, no
  emulator involved) and they run in CI.
- **Our own score is enforced, not asserted.** `.github/workflows/conformance-self.yml` scores us
  through the *same composite action* an external project calls, fails below 100%, and fails if
  `docs/conformance-badge.json` (which the README badge renders) has drifted from the real
  result. Regenerate it deliberately — the command is in that workflow's header comment. A
  second job runs the **fetch** path with no submodule, because that is what every external
  caller takes and it is the one that breaks silently when a mirror moves.
- **Proven foreign against [binjgb](https://github.com/binji/binjgb)** (`gcc -O2 -Isrc -o
  binjgb-tester src/{tester,emulator,joypad,options,common,memory}.c -lm`): its shipped headless
  tester covers the screen capability in a four-line adapter and **passes dmg-acid2 unmodified**.
  It has **no serial output** (no flag, and `emulator.h` publishes no serial hook), so the serial
  half is *not* proven foreign — say so rather than implying it is.
- **`.github/actions/conformance` is the primary product**; the reusable workflow
  (`.github/workflows/conformance.yml`) is a wrapper. A called workflow gets a bare runner and
  must `actions/checkout` the caller itself; the composite action instead gets this whole repo
  checked out for it, which is why `${{ github.action_path }}/../../..` reaches the vendored ROM.
- **Where the "why" comes from:** `tools/conformance/checks.json`, fed by the standing
  per-fix convention at the top of this file and `docs/conformance-notes.md`. A check with no
  `behaviour`, no `explain` or no `hardware` citation **fails the runner's test suite** — the
  explanation is the product, so it is enforced.
## Wild-encounter shiny verdict (`plugins/trainer/src/encounter.rs`)

- **Full write-up: [`docs/trainer.md`](docs/trainer.md#the-wild-encounter-banner).** The claim is
  strictly *"would be shiny if traded to Gen 2"* — **Gen 1 has no shiny Pokémon at all**, and
  overclaiming would make the captain skip a real one or waste a Ball. Every banner string at
  every terminal width carries the qualifier, asserted by
  `the_banner_never_says_a_gen_1_pokemon_is_shiny` (renders widths 8..200 and greps).
- **Why the answer is available at the encounter, and survives capture** (the three citations
  that make the feature honest): wild DVs are two `BattleRandom` bytes written to
  `wEnemyMonDVs` by `LoadEnemyMonData` (`pokered/engine/battle/core.asm`) before the battle is
  drawn; `_AddPartyMon`'s `.copyEnemyMonData` branch (`engine/pokemon/add_mon.asm`) copies them
  **verbatim** into the party entry on capture — nothing is re-rolled; and the Gen 2 rule
  (`CheckShininess`, pokecrystal `engine/gfx/color.asm`) reads only DVs.
- **A trainer battle gets no verdict, and that hides nothing**: `LoadEnemyMonData` gives every
  trainer Pokémon the *fixed* pair `ATKDEFDV_TRAINER`/`SPDSPCDV_TRAINER` = `$98`/`$88`, which is
  not the shiny pattern. The old man's tutorial (`wBattleType == BATTLE_TYPE_OLD_MAN`) is a real
  wild battle with real rolled DVs but the Weedle is never added to the party, so it is excluded
  too. `encounter::applies(is_in_battle, battle_type)` is one pure function, tested over all
  65,536 flag pairs.
- **Static/scripted encounters (Snorlax, the birds, Mewtwo, Voltorbs, tower ghosts) are covered
  for free** — `InitOpponent` falls through to `InitWildBattle`. **Gift/starter/fossil/trade
  Pokémon are not, and cannot be**: they never enter a battle, and `_AddPartyMon` rolls their
  DVs (`; Not wild.` branch) at the instant they become yours. The party panel's `★` is the
  retrospective answer; there is no earlier moment to warn at.
- `wBattleType` is `$D05A`, derived by *walking* `ram/wram.asm` from `wIsInBattle` `$D057`
  through `wPartyGainExpFlags` (1 byte) to `wCurOpponent` `$D059` — and `$D059` is
  independently verified, which is the walk's own proof.
- **`wIsInBattle == $FF` means the player blacked out** (`home/overworld.asm`,
  `.allPokemonFainted`), not a trainer battle. `BattleKind::from_flag` now maps it (and every
  value the game never writes) to `None`. **Symptom before the fix:** a battle panel — opponent,
  level, HP, stats, moves, all stale garbage from the lost battle — displayed *in the
  overworld*, with the same confidence as live data. `ram/wram.asm` documents **four** values
  above the label ("lost battle, this is -1"); the reader had a catch-all `_ => Trainer` arm
  that swallowed the sentinel. **No test ROM catches it and none can** — the cartridge was
  entirely correct; the defect was in a *consumer* of emulated state, which is the same blind
  spot as the render bugs. The test that catches it iterates **every** byte `3..=$FF`, not the
  named cases. Full record: [`docs/conformance-notes.md`](docs/conformance-notes.md)
  (2026-08-14). Standing lesson for any adapter: a catch-all arm over a documented enum is a
  defect until the values it swallows are enumerated.
- **Cost:** the encounter watch runs with the panel **closed** — that is the point, an encounter
  is when nobody has a stats panel up — and costs **one `peek` of `wIsInBattle` at ~10 Hz** in
  the overworld. The verdict is *latched* on the first valid read and held until the battle
  ends, so it never re-reads and cannot flicker. `looks_populated` guards the window where
  `InitWildBattle` has set the flag but `LoadEnemyMonData` has not run yet.
- **Independent of the boot model** (checked on both, not assumed — and note `Model::Auto`
  resolves Red/Blue to `GbMode::Classic`, so DMG is now the *default* for this cartridge):
  `wEnemyMon` `$CFE5` is in the *fixed* WRAM bank; `wIsInBattle`/`wBattleType`/the party live in
  `$D000-$DFFF`, whose `wrambank` is initialised to **1 in both `MemoryManagementUnit::new` and
  `new_cgb`** and only moves on an `SVBK` write a DMG game never makes; `Game::detect` reads the
  cartridge header, not the model; and Red/Blue has no CGB code path. Verified by running the
  wild-encounter harness under **both `GB_MODEL=dmg` and `GB_MODEL=cgb`** — 3/3 verdicts agreed
  each time. **But `BattleRandom` is seeded from `DIV` and the frame counter, so the particular
  species and DVs a scripted run produces are NOT reproducible** — not across models, and not
  across *emulation-timing* changes either: the Mooneye core-timing work (real timer, 5-cycle
  interrupt dispatch, timed OAM DMA) moved every one of them again. Never pin a DV value in a
  test; pin the agreement between the verdict and memory.
- **The verdict costs the Status sidebar no row.** It rides in the trainer plugin's existing
  `Plugin` row via `Plugin::status()` — a second place it is visible with the panel closed —
  and is held to the same wording rule there (`the_plugin_status_line_carries_the_verdict`).
  The Status box itself is now sized from its own line list rather than a hand-counted
  constant: it was `13 + plugins.len()` against a twelve-line list, so with two plugins
  attached the second one's row was clipped and a plugin was *active and invisible*, which
  `docs/plugins.md` §4 promises cannot happen (its own test was failing on `main`). Rows keep
  being added to that panel — `Audio` most recently — so
  `every_status_row_survives_an_audio_row_and_a_live_encounter` pins the worst case: two
  plugins **and** a live encounter, every row still present at three window sizes. **Adding a
  Status row must never silently cost a plugin its row again.**
- The banner gets **its own terminal row** (top row in fullscreen, under the header otherwise)
  and is in `ui_signature`, because a row appearing moves the picture — all the
  `docs/terminal-render.md` rules apply.

### Driving a real wild encounter headlessly — two traps

Both cost hours; `testharness/trainer.rs` check [5] documents them in place.

1. **You cannot teleport the player by editing `wCurMap` in the save.** `LoadSAV`
   (`pokered/engine/menus/save.asm`) sets `BIT_NO_PREVIOUS_MAP` (bit 7) on `wCurMapTileset`
   right after restoring the main data, and `LoadMapHeader` (`home/overworld.asm`) sees that
   bit, clears it and **returns without loading the header** — the whole `wCurMapHeader` block
   lives in `sMainData`, so a continue trusts it. Change the map without the header and the game
   reads the new map's blocks through the old map's `wCurMapDataPtr`: observed as a jump to
   `rst $38` or an illegal opcode within a second. Clearing the bit in the save does not help —
   `LoadSAV` sets it *after* the copy. **Walk instead**; every header is then loaded by the game.
2. **Oak stops a new game at the north exit of Pallet Town.** `PalletTownDefaultScript`
   (`scripts/PalletTown.asm`) opens `CheckEvent EVENT_FOLLOWED_OAK_INTO_LAB / ret nz`, so
   setting that one bit skips the whole introduction, starter choice and rival battle.
   `wEventFlags` is **`$D747`**, derived by walking `ram/wram.asm` backwards from the verified
   `wPlayTimeHours` `$DA41` — the walk lands exactly on `wGrassRate` `$D887` (the union's first
   member, an independent check) and `wEventFlags` is the 320-byte `flag_array NUM_EVENTS`
   before it (NUM_EVENTS = 2560, counting `const`/`const_skip`/`const_next`).
   `EVENT_FOLLOWED_OAK_INTO_LAB` is the first `const` after `const_def`: byte 0, bit 0.
- Route: bedroom `(1,7)` stairs → 1F `(7,2)` door → step **off** the Pallet Town doorstep
  (standing on it, pressing Up walks back inside) → east along `y=7` → north at tile `x=10`
  (block `0B`, the only gap in row 0 of `maps/PalletTown.blk`) → Route 1, arriving at `(34,10)`
  in grass. Pace **by coordinate**, not by alternating Up/Down: a press that only turns the
  character costs a step and moves nothing, so blind alternation drifts north out of the grass.
- After fleeing a battle the "Got away safely!" box is still up and **swallows every direction
  key** — press through it before walking, or the next hundred steps move nothing. Use **B, not
  A**: both advance a text box, but B also closes a menu and can never open one, and an A one
  frame after the box closes opens the START menu, which then eats every direction key for the
  rest of the run. That failure is invisible unless the check is strict about how many
  encounters it wanted — **it is** (`seen < want` is a FAIL), and the pacing loop nudges itself
  free after eight motionless steps and reports how often it had to.

## The save guard: a crashed game must never reach the battery file

**Full story, with the crash that motivated it: [`docs/link-battle.md`](docs/link-battle.md).**

- **`MemoryManagementUnit::report_fault(reason)` is the one entry point, and it does exactly
  one thing: it withdraws the emulator's right to write the save file.** It poisons the
  battery (`MemoryBankController::poison_battery` → `Battery::poison`), records why, warns
  loudly on stderr, and is sticky for the session with the *first* reason kept. It
  deliberately does **not** stop, reset or "fix" the machine — detectors are heuristics, and
  only the irreversible act is taken away. `Gameboy::fault()` exposes it; the TUI shows a
  `Fault: SAVE NOT WRITTEN` row and toasts once.
- **Why it exists:** a Game Boy has no fault handler, so a program counter that walks off its
  own code keeps executing and reliably stores bytes into cartridge RAM. Measured on Pokémon
  Blue crashing out of a Cable Club link battle: **4,378 bytes of cartridge RAM changed within
  six frames**, all persisted by the autosave over a real save. The game is recoverable; the
  file is not.
- **Three detectors, and the third is the reason plugins may report faults at all:**
  (1) an **undefined opcode** — this used to `panic!` (an abort, under `panic = 'abort'`);
  hardware *locks the CPU up*, so it now does that and faults. `$FC` was silently a NOP.
  (The lock-up half also arrived independently on the CGB-DMA/abort branch; the two were
  merged, and `illegal_opcode` keeps that branch's PC rewind so a debugger shows the
  offending byte.)
  (2) **fetching an opcode from `$FE00-$FF7F`** (OAM / unusable / I-O) — one subtract and one
  compare per instruction; HRAM is deliberately excluded, real games run OAM DMA code there.
  (3) **`Machine::report_fault`, which needs NO capability** — every other call there lets a
  plugin *do* something, this one only takes a right away, so fail-safe is to allow it.
  **Neither CPU-level detector fires on the Colosseum crash** (measured over 600 post-crash
  frames: the runaway code stays in executable memory), which is the whole argument for (3).
- **`Gameboy::frame` is bounded by `FRAME_TICKS_CAP`.** It looped until the PPU produced a
  frame and `Gpu::do_cycle` returns immediately with the LCD off, so a crashed game that
  switches the LCD off made it **unbounded** — six post-crash frames took a minute, forty
  never finished; 600 of them now take four seconds. (Two crews landed this cap
  independently, from opposite ends — the CGB-DMA/abort work and this one. `FRAME_TICKS + 64`
  is the tighter and better-reasoned of the two and is what survived.)
- **Test with teeth:** `testharness/battery.rs` case 8 boots a hand-assembled cartridge that
  switches the LCD off, copies a routine into **OAM** and jumps to it, then fills cartridge
  RAM with `$5A`. It asserts the crash was detected, that cartridge RAM really was rewritten
  (so the case is not vacuous), and that the save file is byte-identical. Disable the poison
  call and 8,192 of 32,768 bytes change.
- **Sizing a TUI panel from a hard-coded row count drifts.** The Status box was `Length(13)`,
  one short of its twelve fixed rows plus borders, so the last plugin row was clipped whenever
  a plugin was attached — the exact bug that box's own comment says was fixed, reintroduced
  when a row was added without moving the constant. Two crews found and fixed it the same way
  in the same week; it is derived from the content now, and the `Fault` row is a live example
  of the addition that used to cost a plugin its row.

## Cable Club Colosseum — the link partner takes part in battles (`plugins/gen1-link/src/battle.rs`)

- **One function drives both Cable Club rooms.** `CableClub_DoBattleOrTrade` exchanges the
  identical 644 transfers for a trade and for a battle and diverges only afterwards, on
  `wLinkState` — which **never crosses the cable**. `LinkBattleExchangeData` and
  `Serial_SyncAndExchangeNybble` also share one encoding (`$60 | n`). So the trade-only
  partner answered battle turns with trade answers; "confirm" reads as *move slot 2*, an
  offered Pokémon with fewer than three moves has nothing there, and `GetCurrentMove` +
  `_JumpMoveEffect` `jp hl` out of the program. **The shipped default offer (level 5 Mew, one
  move) crashed deterministically at frame 4038.**
- **Declining the Colosseum is not available**, and it is worth knowing why before anyone
  proposes it again: `LinkMenu` resolves a disagreement in favour of whoever drives the clock,
  and this partner deliberately makes the *game* the clock master. When the player presses A
  on COLOSSEUM the game wins whatever we send; refusing to answer hangs it at "PLEASE WAIT!".
- **The wire carries four bits per turn and nothing else** (`constants/serial_constants.asm`):
  `0-3` move slot, `4+n` send out party slot n, `$D` cannot act, `$E` Struggle, `$F` run. No
  result ever crosses — the player's console computes everything.
- **The partner reads the host's battle state rather than simulating one.** `wEnemyMon` is
  *us* from the host's point of view, so moves, PP, HP and party position are a lookup, not a
  battle-engine reimplementation (which is the classic source of link desyncs). Honest label:
  not a faithful second console, but a real opponent choosing real actions through the genuine
  link path. `Host` is a **trait** so nothing is read during the 644-transfer exchange and the
  protocol stays testable without a ROM (`NoHost`).
- **`wEnemyMons = $D8A4`, located by measurement** — a WRAM search for a `party_struct`
  matching the active Pokémon's species, level and max HP returned exactly one candidate.
  `Host::party_hp` re-checks the species at run time, so a cartridge that disagrees is
  declined rather than misread. `battle_struct` is **29 bytes**, not 28
  (`plugins/trainer/src/gen1.rs`'s `BATTLE_MON_SIZE` is wrong, but it is only used in a doc comment).
- **Without a recognised cartridge the partner is blind but still safe:** it names a move slot
  that is populated in the party *it* sent. That floor is what makes the crash impossible on
  any cartridge; the host reads only make it a good opponent.
- **`seen_party()` had never once worked against a real cartridge.** It indexed the received
  stream at fixed offsets, and no fixed offset can be right — `Serial_ExchangeBytes` re-sends
  its block's first byte until it *receives* a preamble and then once more, so the leading
  copy count depends on the other console. It now finds each block the way the hardware does
  (skip to the `$FD` run, skip all of it, take the length) and tolerates the last block
  arriving one byte short, which it always does. **The old unit test missed it because it fed
  the partner our own framing back** — the regression test now builds the stream the way
  `Serial_ExchangeBytes` does.
- **A link battle is not a trade and must not be reported as one.** `trades_completed` used to
  increment during every battle because turn 2 landed on `Phase::Confirm`.
- **Harness:** `POKE_ROM=<blue.gb> cargo test --release --test colosseum`
  (`testharness/colosseum.rs`, `harness = false`). Skips with exit 0 without a cartridge and a
  cable-club save (`GB_CABLECLUB_SAVE`, or `gen1_trade --make-save`'s fixture). Two cases: the
  shipped default offer, and a two-Pokémon party for the faint/replacement rendezvous — which
  reads the answer off **the game's own screen** (the second Pokémon must come out by name; a
  glitch Pokémon never is). Asserts `wLinkState` stays valid, no fault, no phantom trade, and
  that **SRAM banks 1-3 are byte-identical** — not the whole file: bank 0 is `layout.link`'s
  "Sprite Buffers", scratch the game legitimately rewrites all through a battle's animations.
- **`gen1_trade --make-save` does not work on this box** (it stalls in Oak's speech, on the
  default branch too). The fixture at `/tmp/gameboy-gen1-trade/cableclub.sav` was produced
  earlier and is what the harness finds; regenerating it is an open problem.

## AI agent substrate — "Claude plays Pokémon" (`plugins/agent/src/`, `src/screen/agent_view.rs`)

- **Full design, OBS/Twitch recipe, honest gaps and the recorded run:
  [`docs/claude-plays.md`](docs/claude-plays.md).** Read it before touching any of this.
- **There is ONE way to drive the buttons from outside the machine, and it is
  `crate::input::InputQueue`** — a plugin reaches it through `Machine::press` under the new
  `Capability::CONTROL_INPUT`, a frontend through `Gameboy::queue_press`/`tap`. A press is
  scheduled **in frames** because a frame is the granularity the game samples input at, and
  `Gameboy::frame` advances the queue exactly once, *before* the frame runs. Anything else that
  wants to drive the game (a battle-control plugin, a scripted test) uses this; three private
  frame timers would be three subtly different definitions of "a press". The queue is **not**
  machine state — same category as plugin state — so it is not serialised and `load_state`
  clears it (releasing anything held).
- **A plugin that presses buttons is visible.** `PluginStatus` gained `has_pressed`, and the
  status-bar marker is now name + `!`/`!!` (may write / has written) + `>`/`>>` (may press / has
  pressed). The marker lives on the *name* so panel truncation can never eat the warning.
- **The agent declares `OBSERVE | READ_MEMORY | CONTROL_INPUT` and deliberately NOT
  `WRITE_MEMORY`.** An agent that can poke RAM is not playing the game, and the host enforces
  that rather than the author promising it.
- **The architecture is plan-then-execute, and the numbers are the justification.** The model
  takes seconds; the game runs at 59.73 Hz. The driver returns a goal, one line of commentary and
  ≤12 actions; cheap local code carries them out over hundreds of frames and only returns to the
  driver when the plan finishes, fails, or the world changes unexpectedly (battle started, map
  changed). Measured on Pokémon Blue's opening: **~1 model call per 1,100 emulated frames**.
  `advance_text` alone is most of that saving.
- **The emulator NEVER blocks on the agent.** `Driver::submit`/`poll` must not block;
  `CommandDriver` runs the child on its own thread and the emulator thread only does a
  `try_recv`. Gated in CI by `testharness/agent.rs`: **0.24 ms/frame with a driver that never
  answers**, against a 16.74 ms budget. Never "just await it here" — that is the same mistake as
  putting a second clock inside the audio loop (see the audio section).
- **Budget, breaker and stuck detection live in the HOST, never in the driver or the prompt**
  (`plugins/agent/src/governor.rs`). Four independent ceilings — calls, tokens, spend, **wall clock**.
  The wall clock is the one that matters: token and cost figures are *self-reported by the
  driver*, so they are only ever a floor. A failure reported with **no call in flight is
  ignored**, or a driver that misreports one trips the breaker in milliseconds.
- **Drivers are swappable and the deterministic one came first.** `ScriptedDriver` (canned
  replies + fake latency in frames) goes through the identical parse/clamp/execute path a real
  model does, which is what makes every other part testable for free. Also `RandomDriver` and
  `CommandDriver` (subprocess: state on stdin, plan on stdout). **No HTTP client in the crate** —
  that would be a large new dependency in every build including wasm and FFI; the network lives
  in a stdlib-only adapter under `tools/claude_plays/`.
- **The `claude` CLI adapter is convenient and expensive: ~$0.078 for the first call and
  ~$0.0086 after** (the 1-hour prompt cache), 9–22 s per call, because the CLI loads a whole
  coding-agent system prompt every time. `claude_api_adapter.py` sends ~1.5 KB instead of ~18 KB.
  Use the API adapter for a real broadcast.
- **The commentary panel is designed for a stranger who arrived nine seconds ago**, not as a
  log: a standing `GOAL` line, a `NOW` line, then a short dimmed history. Twitch/OBS specifics
  (1920×1080, 30 fps, 4500 kbps, **terminal font 26–30 px ≈ 106×30 cells**, no mirror) are in
  `docs/claude-plays.md` §6, and the layout is asserted at exactly those cell grids.
- **Everything the panel can render is treated as public and permanent.** Two defences: driver
  errors reach it only as a *category* (never the message, the command, the path or stderr — those
  go to `GB_AGENT_LOG`), and `commentary::sanitise` scrubs credentials, paths, URLs, emails, IPs,
  ANSI escapes and bidi overrides, dropping the line entirely if what is left is not prose.
- **Gen-1 facts that cost real model calls to learn, now encoded in the prompt and in host
  notes** (also written up in [`docs/gen1/agent-play.md`](docs/gen1/agent-play.md), the page the
  battle-control crew reserved for this work): the naming screen needs **START** (mashing A types
  `AAAAA` for ever); a door fires its warp when you step **through** it, so standing on the mat
  and pressing A does nothing; during the opening cutscene `wCurMap` already reads the *bedroom*
  with plausible coordinates while no map header is loaded, so **`wCurMapHeight`/`wCurMapWidth`
  (`$D368`/`$D369`) are the real "am I in control?" test** — `0` until `LoadMapHeader` runs, and
  no real map is 0x0.
- **A map's width/height are in 32x32 BLOCKS; `wXCoord`/`wYCoord` and every warp coordinate are
  in 16x16 tiles — two per block.** Report the raw numbers to an agent and it is told Pallet
  Town is 10 wide while its x runs 0..19, so it decides it is standing outside the map. Doubled
  on the way out; checked against the two maps whose exits are known (Red's house 2F 4x4 with a
  staircase at x=7; Pallet Town 10x9 with Oak's Lab at (12,11)).
- **`Machine::press` is the level above the battle crew's `Machine::set_button`, on the same
  capability and the same latch.** `set_button` is the raw hold/release for something thinking
  frame by frame; `press` schedules in frames. Do not add a third.
- **Testing: `cargo test --release --test agent`** builds a **hand-assembled Gen-1-shaped
  cartridge** — same RAM layout, a player that walks round a walled 8×8 room, no game data at all
  — so the whole stack is proved in CI with no commercial ROM. WRAM boots full of noise, so the
  fixture must zero `wIsInBattle`/`wObtainedBadges`/`wPlayerMoney` or the observer reports a
  level-201 opponent and four badges. (It did.)
- **Never point a run at `~/roms`** — `examples/claude_plays.rs` stages the ROM into `target/`
  first, because the MBC writes a `.sav` beside it.
- **`examples/claude_plays.rs` gotchas:** `--command` swallows every remaining argument, so it
  must come last; and a "print what is new" display must key on a **monotonic counter**
  (`Commentary::accepted()`), never on the length of the bounded history buffer — that bug hid
  28,000 frames of a real run.

## Embeddability — the core stands alone (`--no-default-features`)

The captain's requirement: someone must be able to pull this crate into their own UI, tool
or hardware project and get the emulator, with nothing about this machine or this owner in
it. That is now a real, CI-enforced build; it did not compile before.

- **`--no-default-features` is THE EMBEDDING BUILD, and the dependency count is the contract:
  `gameboy -> blip_buf`, one crate, against 193 on the default build.** `make embed` and the
  **`embed` CI job** assert exactly that (the job installs no system libraries at all). If a
  frontend crate ever appears in that tree, something has leaked into the core — fix the
  layering, do not relax the check.
- **Frontends live behind `desktop` and `tui` cargo features** (both still in `default`, so
  every existing binary is unchanged). Before this, `screen::desktop` and `screen::tui` were
  unconditional `mod`s, so glutin/gl/ratatui/ratatui-image/image came with the emulator
  whether you wanted them or not. **New rule: anything a frontend needs goes behind a
  feature.** A new example or test that reaches for `image`/`ratatui`/`cpal`/`libc` needs an
  explicit `[[example]]`/`[[test]]` entry with `required-features` — otherwise the minimal
  build goes red on code that was never meant to be in it.
- **`examples/headless.rs` is the proof, not a demo.** It runs a cartridge, takes input and
  round-trips a save state with **no window, no terminal, no audio device, no image codec and
  no `$HOME`** — `make embed` runs it with `HOME`, `DISPLAY` and `WAYLAND_DISPLAY` unset. Keep
  it dependency-free (its FNV hash and ASCII renderer are hand-rolled on purpose).
- **The core's whole host surface, enumerated so it can be defended:** one environment read
  (`$GB_MODEL`, and only for `Model::Auto` — `Gameboy::new_with_model` bypasses it), one wall
  clock read (only writing/reading an MBC3 clock cartridge's battery file), and `<rom>.sav`
  **only** if the caller passed `Some(path)`. Everything else — config files, `$XDG_*`,
  `/dev/video*`, terminals, displays — is in `screen`/`video`/`sound::cpal_audio`. Adding a
  host dependency to `cpu`/`mmu`/`gpu`/`mbc`/`sound`/`savestate`/`mode`/`upscale` breaks the
  claim; put it in a frontend.
- **`std` is required and `no_std` is not a flag away** (`Vec`, `Box<dyn>`, `std::sync::Mutex`
  in the FFI shim, `PathBuf` in the battery layer). Say so rather than implying otherwise;
  embedded *Linux* is fine, bare metal is a rework of the MBC/battery layer.
- **`get_mbc` used to force an MBC3 whenever no save path was given** — i.e. for the wasm
  build, the FFI build and every embedder — so a non-MBC3 game was mis-banked from its first
  bank switch. Persistence and mapper selection are now separate: every `MBCx::new` takes
  `Option<PathBuf>`. *Conformance record:* **symptom** — a ROM-only or MBC1/MBC5 game runs
  wrong (garbage code, freeze) in the browser or in any embedder, while the identical ROM is
  fine in the desktop build; **caught by** — nothing in the suite, because every harness
  passes a path, which is itself the finding; now
  `mbc::test::the_mapper_is_chosen_from_the_header_with_or_without_a_save_path` (proved to
  fail against the old code); **hardware** — the mapper is cartridge header byte `0x0147`
  (Pan Docs, The Cartridge Header), and nothing about a save file can change it.
- **Arbitrary bytes must not abort the host.** `[profile.release] panic = 'abort'` means a
  panic in the core takes the embedder's process down with no unwinding. A cartridge shorter
  than its header used to index out of bounds; `get_mbc` refuses it, and
  `Gameboy::try_new_with_model` / `Cpu::try_new_with_model` return the reason. **Use the
  `try_` form for bytes you did not produce.**
- **Cross-target reality:** `cargo check --no-default-features` is clean for `wasm32`,
  `x86_64-pc-windows-gnu`, `aarch64-unknown-linux-gnu` and `x86_64-apple-darwin` (in CI for
  the first three). Those are **type checks, not linked-and-run** — x86-64 Linux is still the
  only platform anything has been executed on. Do not upgrade that claim without running
  something.
- **Licensing position, per suite, for redistributors** (README "Embedding the core" carries
  the user-facing version): every resolved dependency in every feature combination is
  MIT/Apache-2.0/BSD/Zlib/ISC/0BSD/Unlicense — **nothing GPL**; `icy_sixel` ships MIT text
  with no `license` field in its manifest, which is a metadata gap, not a licence problem.
  The only vendored ROM is `third_party/dmg-acid2` (MIT, redistribution permitted, licence
  included). **Blargg's ROMs carry no explicit licence and are a submodule, never vendored;
  Mooneye is not vendored either.** Re-run the check with
  `cargo metadata --all-features` when a dependency is added.
- **Personal identifiers:** the tree is clean of them (`git grep -iE 'aaron|codebyexample'`
  returns nothing outside the `tests/` submodule). Sanitiser fixtures in
  `plugins/agent/src/commentary.rs` deliberately feed private-looking shapes through `sanitise` —
  keep them **obviously synthetic** (`/home/example-user`, `someone@example.com`), never a
  real address or path. Commit *author* emails in git history are a separate thing and are
  not editable without rewriting history — a captain decision, not a crew one.

## Two PPU engines — the pixel FIFO and the whole-scanline renderer (`src/gpu/fifo.rs`)

- **Full state, per-test numbers and the recommendation: [`docs/mealybug.md`](docs/mealybug.md) §7.**
  Component-level orientation: [`docs/components/ppu.md`](docs/components/ppu.md) §4.0.
- **`RenderMode { Fast, Accurate }` picks the engine**; `GB_PPU=fast|accurate`, the `ppu` config
  key (env wins), or `Gpu::set_render_mode`, and it is shown in the TUI Status panel's `PPU` row.
  It is deliberately **not** in the save state — it is a property of this session's host, not of
  the emulated machine, so loading someone else's state must not switch their renderer.
  `Gameboy::set_render_mode` takes `gameboy::ppu::RenderMode`; the `gpu` module itself stays
  private, and the crate root's `gameboy::RenderMode` is a *different* enum (which frontend).
- **`Fast` is still the default and every baseline is measured on it.** The accurate engine is
  behind on exactly two rows — Blargg `oam_bug` #7 and Mooneye `ppu/intr_1_2_timing-GS` — and
  until it strictly dominates, flipping the default would make a published score worse.
  **Never re-bless `testharness/baseline.txt` to hide that.**
- **THE TRAP THAT COST THE MOST: `change_mode(0)` runs `renderscan()`.** Entering HBlank draws
  the line that just finished. Under the accurate engine that overwrote, every scanline, the
  picture the FIFO had just painted dot by dot — and *almost* invisibly, because the two agree
  on ordinary content. What exposed it was a side effect: `draw_bg` advances the window's line
  counter as it goes, so running both moved the window down two rows per scanline. **Anything
  else hung off a mode transition must ask which engine is running.**
- **`OAM_BUG_PHASE = -12` is not physics, it is the batched PPU's error**, and it is exactly one
  `ldh [LCDC],a`: the write sets `modeclock = 4` and the whole-scanline renderer then applies
  the instruction's entire twelve dots on top. The accurate engine has already spent those dots
  access by access, so it lands on the real 4 and needs **no** correction
  (`ACCURATE_OAM_BUG_PHASE = 0`). Measured: with `-12` the accurate engine fails `oam_bug`
  4/5/6/8; with `0` it passes them. Swept `-12..+12`; nothing recovers #7, so #7 is a real
  finding and not a tuning knob.
- **A wrong *variable* mode-3 length is worse than an honest constant one** for a ROM that
  synchronises itself against `STAT`. `m3_bgp_change` and `m3_lcdc_obj_size_change_scx` render
  a near-blank frame under the accurate engine and a nearly-right one under the fast engine,
  because a fixed 172 keeps their own counted-dot loop in step. Expect aggregate Mealybug pixel
  distance to get slightly *worse* before it gets better, and never quote the aggregate alone:
  the closest tests improved by one to two orders of magnitude (`m3_scx_high_5_bits`
  9,766 → 141) while the total moved 1,212,282 → 1,268,382 and no row flipped.
- **Cost, measured, since the whole two-renderer decision rests on it:** dmg-acid2 0.219
  ms/frame fast vs 0.463 accurate — **2.1x, 2.8 % of a 16.7427 ms frame, 36x real time
  headless**. Cost is *not* a reason to keep two renderers; the two red rows are.
  Reproduce with `cargo test --release --test ppuengines`.
- **`testharness/ppuengines.rs` is the only thing in CI that runs the non-default engine.** It
  pins three hermetic claims from the vendored MIT dmg-acid2 ROM: pixel-exact against its
  author's reference, frame-identical to the fast engine on well-behaved ROMs (which makes
  `testroms`' whole baseline protect the accurate engine transitively), and a 4 ms/frame
  collapse detector. Teeth proven by putting `renderscan` back on top of the FIFO: 762/23040.
- **`modeclock` is a 0-based dot index in the accurate engine** — dot 0 is the first dot of
  mode 2, dot 80 the first of mode 3, as Pan Docs numbers them. `accurate_dot` therefore does
  the work first and increments after.
- **BOTH engines are gated in CI.** `make test-external` scores the default,
  `make test-external-accurate` scores the pixel FIFO against its own
  `testharness/{mealybug,mooneye}_accurate_baseline.txt`. Those carry **pixel counts**, so a
  pipeline change is a visible diff even when it flips no row — which is the only way a
  rewrite this far from green shows progress *and* regression. **Every baseline file opens
  with the engine that produced it and the target that re-blesses it**: the two engines score
  the same ROMs differently, so the obvious command rewrites the file with the other machine's
  numbers.
- **Reproduce the suites by hand with**
  `cargo run --release --example mealybug -- --dir <checkout>` and
  `cargo run --release --example mooneye_scout -- <checkout>/acceptance`, both with `GB_PPU`
  set. `GB_SCOUT_SCREEN=1` prints a failing Mooneye test's own assertion table, which is far
  faster than reading registers.

## MCP server (`src/mcp/`, `gameboy-mcp` binary, `mcp` cargo feature)

- **Full contract, install steps for Claude Desktop and Claude Code, every flag, the measured
  numbers and the honest limitations: [`docs/mcp.md`](docs/mcp.md).** Behind the `mcp` cargo
  feature, adds **no third-party crate**: the JSON codec is the agent plugin's, PNG and
  base64 are `termimage`, and every crate it pulls in is in this repository.
- **It is a thin wrapper over the agent plugin, and must stay one.** `observe` calls
  `gameboy_plugin_agent::observe::{Gen1Observer, Screen}`; `run_actions` drives
  `gameboy_plugin_agent::action::Executor` unchanged; the JSON codec is that crate's too.
  Nothing here reads game state or times a button press. If a change starts re-deriving
  either, it belongs in `plugins/agent/`. That is also why the feature is
  `mcp = ["plugin-agent"]` — all first-party, still no third-party crate.
- **A frontend, NOT a plugin — decided deliberately, and here is the argument.** A `Plugin` is
  handed a `Machine` *during a frame the emulator has already decided to run*. Three tools
  cannot be expressed that way: the server must **drive** the clock (`step`, `press_buttons`,
  `run_actions` decide how many frames run and when the call returns); half the surface is
  **above** the machine (`load_rom` replaces the cartridge, `reset` rebuilds it, `load_state`
  swaps its contents); and it owns host resources (stdio, a sound device, a video sink), which
  is where `screen::tui` sits. It *uses* the plugin contract instead of joining it.
- **`Gameboy::with_machine(caps, f)` is the new core hook** (`src/gameboy.rs`), giving a host
  the same capability-checked `Machine` a plugin gets; `Machine::new` became `pub(crate)`.
  That is what lets `write_memory` be *provably* absent without `--allow-write-memory`
  rather than absent by convention — and `read_memory`/`write_memory` go through it rather
  than through `Gameboy::peek_range`/`debug_write`, even though the server owns the
  `Gameboy` and could reach either. Filtering the tool out of `tools/list` is what a
  *client* sees; the grant is what makes the write impossible. Pinned twice:
  `a_machine_handed_out_by_the_host_still_enforces_its_grant` (core) and
  `a_write_without_the_grant_does_not_land_and_says_so` (server).
- **The agent plugin is refused with an explanation, not attached.** An MCP client already *is*
  the agent; two of them share one controller and each undoes the other's plan.
- **`write_memory` is filtered out of `tools/list`, not merely refused when called.** A client
  must never be shown a tool that will say no. Pinned by
  `write_memory_is_absent_from_the_listing_unless_it_was_enabled`.
- **A failing tool is a *successful* JSON-RPC call carrying `isError`**, per the MCP spec — a
  transport-level error is a fault most clients never show the model, so it cannot adapt.
  Likewise a **notification (no `id`) gets no reply**; answering `notifications/initialized`
  is the classic way to break a client.
- **There is no `--tui`, and that is the honest answer.** The MCP transport *is* stdin/stdout;
  a terminal frontend owns both. `--video-out` is the way a human watches. `--video-out` to a
  **FIFO** needs the `libc` feature (`--features mcp,libc`); a plain file works everywhere.
- **Measured, through the protocol, on a real Pokémon Blue save:** headless / no audio /
  uncapped (the default) **0.238 ms/frame = 70x real time**; `--plugins trainer` 0.239;
  `--video-out` 0.238; `--audio` **0.264** (~11%); `--speed realtime` 16.74 exactly.
  **The "~8 ms/frame with audio" figure in circulation is STALE** — it predates the audio
  crew's non-blocking hand-off (`docs/audio.md` §4c). Audio stays off by default because
  nobody is listening at 70x, not because it is ruinous.
- **Harness: `make test-mcp`** (`testharness/mcp.rs`, `harness = false`, in CI) spawns the
  **real binary** and speaks JSON-RPC down its stdin — a server that starts is not a server
  that works. Needs only the vendored dmg-acid2 ROM. `make lint-mcp` exists for the same
  reason `make lint-audio` does: plain `make lint` never compiles the feature.
- **A trap this harness fell into and now documents: dmg-acid2 draws a STATIC picture.** The
  first save-state round-trip check compared `observe` output before and after, which is
  byte-identical at any two frames — so a `load_state` that did nothing passed. Proved by
  making it a no-op and watching the check stay green. It now probes `$FF04` (DIV), which
  advances ~274 counts every frame. **Never build a "did the state change?" check on a still
  screen.**
- **Verified with a real client**, not only the harness: Claude Code via
  `claude -p --mcp-config` loaded a save state, and `observe` returned Viridian Pokémon
  Center at (11,3) with SPARK lv12 / MEWTWO lv70; `read_memory` at `$D35E` independently
  returned map 41; `run_actions` walked and `observe` confirmed y 3→4; `goto (3,7)` routed
  round the room's furniture; and the model described the screenshot as the Center's lobby
  with a nurse and potted plants.
- **Gen 1 menu timing is the game's, not the server's.** `press_buttons` defaults to a
  24-frame gap (`agent::action::MENU_GAP`) because `HandleMenuInput` delays several frames
  after accepting a press. Several presses in one call still land faster than a *screen
  transition*, so a driver must `step` for the screen to settle after one. Rediscovering this
  as "A does not register" costs an hour — it is real emulation, proved by driving the same
  sequence through the library directly.
## The README's screenshots — where they come from, and the licence line

- **`docs/shots/` holds the README's own pictures, and they are ours to show.** Every one is
  either the emulator's own chrome or the output of a ROM we may redistribute or run freely:
  the MIT dmg-acid2 ROM vendored in `third_party/`, and Blargg's `cpu_instrs` from the `tests/`
  submodule. **Never commit a screenshot of a commercial game** — that is the same
  licence-boundary rule as the rest of this file, applied to pixels. The narrow exception
  already in the tree is `docs/link-trade/`, argued for explicitly in the plugin section above
  ("our interface, doing something, with the game incidental"); do not widen it casually.
- **The recipe that produced them** (the screenshot mechanics in the frame-transport section
  above are the same, this is the README-specific application):
  ```bash
  env -u WAYLAND_DISPLAY DISPLAY=:0 wezterm-gui --config enable_wayland=false \
      --config initial_cols=150 --config initial_rows=45 --config font_size=13 \
      --config enable_kitty_keyboard=true start -- ./target/release/examples/terminal <rom>
  WID=$(DISPLAY=:0 xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1)
  DISPLAY=:0 import -window $WID shot.png
  magick shot.png -resize 1600x -strip -colors 128 PNG8:docs/shots/<name>.png
  ```
  Drive keys with `wezterm cli send-text --no-paste --pane-id 0` against **that instance's**
  socket (`WEZTERM_UNIX_SOCKET=/run/user/1000/wezterm/gui-sock-<gui-pid>`). Point
  `XDG_CONFIG_HOME`/`XDG_STATE_HOME`/`XDG_PICTURES_DIR` at a scratch dir so a capture run
  cannot touch the captain's config, recents or Pictures. **PNG8 with 128 colours is
  free**: a terminal screenshot quantises to a quarter of the size with no visible loss
  (382 KB → 105 KB), and a README that costs a megabyte to open is a README nobody scrolls.
- **`pkill -f '<pattern>'` matches your own shell's command line** and will kill the pipeline
  you are running it from — the same trap as `pgrep -f` recorded above. Kill by the PID you
  captured at launch.

## Re-measure the suite scores before quoting them — they move most days

Several crews landing APU, PPU and SGB work in the same week means any absolute suite number
in a brief is probably a few days stale. In one afternoon SameSuite moved 68 → 69 (the CH1
sweep-shadow fix `52d98f5`) → 71 (SGB), and Mooneye 86 → 88. **Measured on `main` at
`8747e7d`:** testroms **46/47**, Mooneye **88/103** (acceptance 60/75, emulator-only 28/28),
Mooneye CGB misc **4/8**, SameSuite **71/76** (apu 64/69, dma 4/4, ppu 1/1, sgb 2/2),
`cpp` **4/4**, Mealybug **3/79**. The one-liner that produces all of them without a full run:

```python
# python3, from the repo root — reads the checked-in baselines, which ARE the expectations
import collections
for f in ['mooneye','mooneye_cgb','samesuite','mealybug']:
    rows=[l.split('=') for l in open(f'testharness/{f}_baseline.txt')
          if l.strip() and not l.startswith('#')]
    print(f, sum(v.strip().startswith('PASS') for _,v in rows), '/', len(rows))
```

**Both engines' baselines must move together when a non-PPU change flips a row.** Boot
registers, the timer and the APU do not depend on `GB_PPU`, so a change that flips a Mooneye
row flips it under *both* engines and **both** `mooneye_baseline.txt` and
`mooneye_accurate_baseline.txt` need re-blessing in the same commit — otherwise
`make test-external-accurate` goes red on merge while `make test-external` is green, which
reads as a PPU regression and is not one. The SGB work is the worked example: +2 acceptance
rows (`boot_regs-sgb`, `-sgb2`) under both engines, both files updated, both gates green.

## The audio-cost figure: quote the measurement, not the folklore

"Audio costs ~8 ms/frame" and "17.9 ms/frame with sound" are **stale** and keep resurfacing in
briefs. They describe the blocking cpal player fixed in `8b4207d`. Measured on this box today,
with a real device attached: **0.256 ms/frame** (`make test-audio-perf`) and **0.267 ms/frame**
(`cargo run --release --features audio,libc --example audio_frame_cost -- <rom>`), against
**0.219** for the same run with no audio at all. The single biggest performance lever in the
emulator today is the **PPU engine** (0.219 → 0.467, 2.13x), not audio. Re-measure before
repeating either number.

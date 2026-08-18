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
- WASM build: `cargo build --release --target wasm32-unknown-unknown`. **The APU is
  emulated on wasm and plays through the Web Audio API** — see the "Web audio" section
  below and [`docs/web-audio.md`](docs/web-audio.md). The `audio` cargo feature means
  *cpal*, which wasm never has, so it is irrelevant there.

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
- ~~`get_mbc` returns MBC3 for **any** cartridge when built without a filepath~~ —
  **fixed, and this note was stale for a long time after it was.** `get_mbc`
  (`src/mbc/mod.rs`) takes the save path *only* to decide whether cartridge RAM is
  battery-backed; the mapper is header byte `$0147` on every path. Proved end to end by
  `mbc::test::the_mapper_is_chosen_from_the_header_with_or_without_a_save_path` (the core)
  and `lib::ffi_tests::the_ffi_entry_point_takes_the_mapper_from_the_cartridge_header`
  (the real `extern "C" load()` symbol, which is what an FFI or web embedder actually
  gets). Do not re-add "always pass `Some(path)` or you get an MBC3" to any comment.
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
  PPU — and that is now settled, not predicted: both PASS on the `identical` engine and
  stay red here.** They sample OAM/VRAM **accessibility** at 1 M-cycle resolution over
  three scanlines, so they need (a) mode-based VRAM/OAM access blocking and (b) the PPU
  stepped per bus access. The per-dot engine has both, so it gets the blocking
  (`Gpu::cpu_can_read_vram` and its three siblings). **Do not add (a) to the default
  engine**: with mode 3 fixed at 172 dots where hardware varies it 172-289, blocking would
  silently drop writes a game placed legally in HBlank — strictly worse than not blocking.
  That is why all four predicates return "reachable" unconditionally for `RenderMode::Fast`.
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
- **`oam_bug@cgb` MUST STAY RED, and it is corroborated in the DETAIL now, not just
  quoted.** The bug is DMG/MGB/SGB silicon only — Pan Docs says CGB and Advance are
  unaffected *even running monochrome software* — so a real CGB fails this ROM too.
  `Gpu::oam_bug` returns early unless `gbmode == GbMode::Classic`, which deliberately
  excludes `ColorAsClassic` (that is CGB hardware in compat mode). The baseline header says
  all this so nobody "fixes" the row. **Checked against an independent implementation on
  2026-08-15** (`tools/sameboy-oracle/`, one command): SameBoy on a DMG prints
  `01:ok 02:ok … 08:ok  Passed`; on CGB-E it prints
  `oam_bug  01:ok 02:02 03:ok 04:03 05:02 06:ok 07:01 08:02 … Failed #2` — **character for
  character** what `testharness/baseline.txt` records for `blargg/oam_bug@cgb`. And SameBoy
  gates the defect exactly as we do (`GB_trigger_oam_bug` / `GB_trigger_oam_bug_read`,
  `Core/memory.c`, both opening `if (GB_is_cgb(gb)) return;`). Agreeing with a foreign
  emulator sub-test by sub-test is a far stronger claim than "we fail it too" — **do not
  re-litigate this row.**
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
- **The `retrio/gb-test-roms` submodule is NOT initialised in a fresh clone or worktree, and
  `testharness/testroms.rs` PANICS without it** (`cannot read ROM tests/cpu_instrs/…`) — so a
  `cargo test` run silently proves nothing about the in-repo accuracy harness. Run
  `git submodule update --init --depth 1 tests` **before** trusting any before/after comparison.
  Same class as the `dmg_sound` ROMs `audiocapture` wants; the note at the top of this file
  saying it "is now initialised and wired into CI" is about CI, not about your working copy.

## The c-sp aggregation — thirteen suites nothing here had ever run (`examples/csp_suite.rs`)

**Full record: [`docs/measured/scoreboard.md`](docs/measured/scoreboard.md#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time)
(scores + a reason per red group), [`docs/testing.md`](docs/testing.md#the-c-sp-aggregation--thirteen-more-suites)
(mechanics), `docs/conformance-notes.md` (2026-08-15).**

- **[c-sp/game-boy-test-roms](https://github.com/c-sp/game-boy-test-roms) bundles eighteen
  suites; five were gated here and THIRTEEN had never been run at all** — AGE, Bully,
  cgb-acid2, cgb-acid-hell, Gambatte, GBMicrotest, MBC3 Tester, the wilbertpol Mooneye fork,
  little-things-gb, rtc3test, Scribbltests, Strikethrough, TurtleTests. 4,510 ROMs, **6,045
  rows** once each is scored on the consoles its own file names name. Gated in **both** picture
  modes by `make test-external` / `make test-external-accurate` against
  `testharness/csp_<suite>[_accurate]_baseline.txt`.
- **THE RELEASE IS PINNED WHOLE, and that is a deliberate departure from the per-suite pins
  beside it.** One immutable artefact, one sha256; it is the corpus other emulators' published
  c-sp scores are measured on; and — the reason that decided it — **every suite in it ships the
  author's own `game-boy-test-roms-howto.md`** stating that author's exit condition and pass
  criterion. Those documents are the specification `csp_suite.rs` implements. **Do not
  re-point the five already-pinned suites at this release**: their baselines were measured
  against those exact builds.
- **Five verdict conventions, none of them ours:** `LD B,B` + Fibonacci registers (AGE), the
  undefined `$ED` + Fibonacci registers (wilbertpol — that is how Mooneye worked in 2016), a
  verdict byte at **`$FF82`** (GBMicrotest — read *only* that byte, several tests leave
  `$FF80 == $FF81` on failure), a hex digit drawn in the top-left corner compared against
  Gambatte's own font (transcribed into `OUT_TILES` from its `testrunner.cpp`), and the
  author's reference screenshot for everything else.
- **COLOUR IS THE PART THAT IS EASY TO GET WRONG, and it is decided from evidence.** A frame we
  drew in `GbMode::Color` is real colour; anything else is four greys, and then the reference is
  either the DMG palette or the Color boot ROM's eight compatibility colours — told apart from
  the reference's own palette. **A shade-rank compare cannot do the compatibility case**: eight
  reference luminances against four of ours re-ranks every pixel. A true colour reference is
  taken back to 15-bit RGB (`(X << 3) | (X >> 2)` is exactly invertible) and pushed through
  *our* correction, so our framebuffer is never re-ranked. Gambatte's own screenshots are
  already in our space (it uses the same correction) and take its `& 0xF8F8F8` mask.
  Sharp edge: **the shipped `mbc3-tester-cgb.png` uses `#7BFF4A` where every howto says
  `#7BFF31`** — both are accepted as compat shade 2.
- **A `SKIP` row counts in NEITHER column and says why** (96 of the 6,045: 95 Gambatte ROMs
  upstream itself disables via an `_xout0` name that its own `find("_out")` misses, or state
  dumpers; plus `tellinglys`, which needs a human pressing every button). `count()` in
  `docs/tools/status_numbers.py` drops them from both columns.
- **Two runner traps that produced wrong verdicts before they were caught:** a reference
  belongs to the ROM with the **longest** matching stem (suites ship `foo.gb` beside
  `foo-nocgb.gb`, and shortest-match scores the wrong picture as a wrong emulator); and a
  runner must open **only** the reference names upstream opens (Gambatte ships bare
  `<stem>.png` and `<stem>_gba.png` files its `testrunner.cpp` never looks at, and picking them
  up also produces two rows claiming the same console).
- **The Gambatte baseline is 5,320 rows / ~400 KB, twice over.** Deliberate, and the same trade
  the Mealybug baselines make with their pixel counts: at that size a PASS/FAIL total is a flat
  wall and what a PPU campaign needs is to see *which* rows moved. Expect a large readable diff
  from any PPU change.
- **Teeth proven:** `DRAW_DOTS` 172 → 176 (one line) turns the Gambatte gate red on **288**
  rows, GBMicrotest on 38 and wilbertpol on 14; restoring `MBC3`'s `& 0x7F` turns
  `mbc3-tester` red on both rows. Same rebuild caveat as the other scouts — `csp_suite` is a
  `cargo build` target, so rebuild after breaking source.
- **The fix it bought: MBC30 (`src/mbc/mbc3.rs::bank_mask`).** MBC3's ROM-bank register is
  seven bits (Pan Docs, MBC3); **MBC30's is eight** (Pan Docs, MBC30 — "4 MiB ROM, 64 KiB RAM,
  timer … the only title to be shipped with the MBC30 mapper was Pocket Monsters: Crystal
  Version in Japan"), and **no cartridge-type byte tells them apart** — the tester declares type
  `$11` with no RAM at all. The ROM's own size does: 256 banks cannot be addressed by seven
  bits. `mbc3-tester` FAIL → PASS on both consoles; nothing at 128 banks or fewer changes, so
  no other baseline moved. No `savestate::FORMAT_VERSION` bump (`rombanks` is derived from the
  cartridge).
- **`boot_ppu_dot`'s "no ROM measures this on a plain DMG" is FALSE and the source now says
  so.** GBMicrotest's `poweron_ly_*`/`poweron_stat_*` are a run of `nop`s and one
  `LDH A,[$FF44]`, so every one of them pins it: hardware reads `LY = 0` at 119 M-cycles and
  `1` at 120 where we roll over at 114. **The constant was still not changed** — sweeping it
  gives 237/513 at dot 69900, 251 at line 0 (today), 262 at 70168, 250 at 70184: a jagged
  profile with no clean optimum, i.e. a constant absorbing an error somewhere else. Come back
  with a model, not a sweep.
- **`identical` does NOT dominate everywhere any more, and the scoreboard's old claim is
  corrected.** It loses `cgb-acid2` outright (pixel-exact on `standard`, 158 px wrong on
  `identical`) and part of Gambatte's `sprites` (245 → 241) and `window` (259 → 227) groups,
  while winning hugely overall (Gambatte 2,682 → 3,059, GBMicrotest 251 → 338). **The accuracy
  precondition for making it the default is therefore not met** — that is a PPU finding, not a
  cost decision.
- **Where the remaining reds are, grouped:** mid-scanline PPU timing is most of them and is the
  same wall Mealybug describes (Gambatte `scy` 0/67 and `bgtilemap` 0/40 on the default engine
  against 54/67 and 36/40 on the per-dot one); OAM/VRAM access blocking, which is deliberately
  absent from the default engine; console revisions we do not model; `firstwhite` printing an
  error on a DMG and not on a Color console (unexplained); and **`bully` drawing an entirely
  black screen**, which is a failure to *start* and is the first thing to look at.

### A SUITE IS AN INPUT, NOT A SCOREBOARD — and `docs/pitfalls/codes.md` is the third output

The captain's framing, and it changes what "triage" means here: every failure ends as **(1)** a
fix, **(2)** a check in the GBSelfTest cartridge so it is caught forever, and/or **(3)** a
documented behaviour carrying a **stable code** the cartridge prints, so a failure on anybody's
emulator points a stranger straight at our page. Nothing else in the ecosystem does the third.

- **[`docs/pitfalls/codes.md`](docs/pitfalls/codes.md) is the registry and it is a CONTRACT.**
  `TGB-<AREA>-<NNN>`, one code per behaviour, each with an anchor on that page naming the
  hardware, the public suites that already measure it, and what a wrong emulator does.
  **A code is permanent once published** — its whole value is that a five-year-old build log
  still resolves — so adding is free and renumbering is forbidden. The registry is ours; which
  codes the cartridge *emits* is the cartridge crew's (`gameboy-testcart-m5`).
- **Every row carries "already covered?" and "diagnosable from inside a cartridge?", and both
  are there to stop wasted work.** A behaviour Gambatte already measures across 811 rows does
  not want a fourteenth reimplementation — GBSelfTest should aim at **gaps** and at checks that
  *discriminate*. And three of the named behaviours are **not decidable from inside a
  cartridge at all** (mid-mode-3 register writes, late OAM writes, the host-side presentation
  defects): the evidence is the picture or the wire, so they belong to Mealybug and to the
  render conformance record, not to a self-test.
- **The ranked handoff, top of the list first:** `TGB-MBC-002` (cartridge input must never
  abort the host — **nothing public covers it and nothing ever will**, no suite ships a ROM
  designed to crash its runner, and this class has cost this project five real aborts);
  `TGB-MBC-001` (a mapper's bank register is as wide as the cartridge needs — one 4 MiB
  screenshot-judged ROM in the world tests it); `TGB-PPU-004` (OAM/VRAM access blocking —
  trivially self-checking and it discriminates rough from careful); `TGB-PPU-008` (the mode-0
  interrupt versus `SCX` — eight values, one interrupt each, and we fail all 24 rows);
  `TGB-PPU-007` (the STAT write glitch, covered only by ROMs the modern Mooneye suite dropped,
  so it is quietly rotting out of the ecosystem); `TGB-SER-001` (the serial interrupt's phase
  against `DIV` — one suite measures it at all).
- **`TGB-DMA-001` is the single largest named cause in the whole aggregation** — 562 failing
  rows: while OAM DMA runs the CPU sees the DMA's bus rather than the memory it addressed. Our
  OAM blocking and 160-cycle timing are right (every Mooneye `oam_dma_*` passes); the bus
  conflict is not modelled at all. Gambatte covers it exhaustively, so this is a *fix* item,
  not a cartridge item.
- **`TGB-APU-001` was verified not to be a runner artefact before it was written down.** 71
  Gambatte rows where we emit a flat 0.0 for the whole judged frame and hardware does not.
  Re-running the audio player at the Game Boy's own 2 MiHz rate (`Sound::set_player` honours
  whatever `samples_rate()` reports) changes **not one verdict**, nor does widening the
  window from one frame to a whole second, and the APU demonstrably produces non-zero output on
  37 other rows in the same group. **Do the equivalent check before blaming an emulator for an
  audio verdict** — a band-limited resample really can turn a high square wave into a flat line,
  it just was not what happened here.

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

## Console revisions — nine machines, not four (`src/mode.rs`, `Registers::boot`)

**Mooneye `acceptance` 60/75 → 67/75, `misc` (CGB) 4/8 → 8/8**, measured before and after
with `make test-external`; everything else byte-identical, including Mealybug's per-row
pixel counts, on **both** PPU engines. Full per-fix record with citations:
[`docs/conformance-notes.md`](docs/conformance-notes.md) (newest entry).

- **A boot ROM is part of the silicon, so a revision is a machine.** Mooneye ships a
  `boot_regs`/`boot_div`/`boot_hwio` ROM per console and **no two consoles hand the
  cartridge the same state** — passing one is failing the others. `Model` now carries
  `Dmg0`, `Mgb`, `Cgb0` and `Agb` beside `Dmg`/`Cgb`/`Sgb`/`Sgb2`; ten of the eleven rows
  this work turned green were one cause, "we only modelled four of the nine".
- **The revisions are boot state ONLY and are deliberately not in `GbMode`.** From
  `PC=$0100` a DMG0 and a DMG are identical silicon, so `GbMode` still has its five values,
  **`savestate::FORMAT_VERSION` did not move**, and `MemoryManagementUnit::console` is not
  serialised — same call as `Sound::cgb_revision`: a property of the machine a state is
  loaded *into*. `Model::console(cgb_flag)` resolves `Auto` away; `Model::mode(cgb_flag)`
  turns a concrete console into a `GbMode`.
- **`Model::Auto` must never infer a revision** (`auto_never_infers_a_revision`). A
  pre-release DMG, a Pocket or an AGB is something you ask for by name — `GB_MODEL=dmg0|
  mgb|cgb0|agb`; inferring one would silently change every game's boot registers.
- **`A = $FF` is how a cartridge detects an MGB and `B` bit 0 is how it detects an AGB**, so
  these are not trivia. An AGB's boot ROM ends with an `inc b`, which is both why `B` holds
  `$01` and why `F` reads `$00` instead of the CGB's `Z` — one instruction explains both.
- **DMG 0's boot ROM is a different program, not a variation.** It has no header-checksum
  lock-up, so it never runs the code that leaves the registers tidy (`F=$00 B=$FF DE=$00C1
  HL=$8403`), hands over with the system counter nearly `$9400` lower, and — because it
  skips `CheckLogo` — leaves the **PPU nine scanlines earlier**, still in VBlank.
- **The PPU's position at hand-off is a per-console constant** (`Gpu::set_boot_position`,
  `mmu::boot_ppu_dot`). Only DMG 0 gets one: `boot_hwio-S` and `misc/boot_hwio-C` **skip**
  `$FF40`/`$FF41`/`$FF44` in their own masks, so no ROM measures it on those machines and
  none is invented. `DMG0_BOOT_PPU_DOT` is calibrated **with its window stated** — line 145
  dots 96..260 all pass, so it is the centre of a 168-dot band, and neither edge is hardware
  (one is our fixed 172-dot mode 3, the other is the ROM's `LY` read rolling over).
- **An SGB's boot ROM does NOT take a fixed time, and `boot_div2-S` exists to catch you
  hard-coding one.** It clocks the 76-byte cartridge header to the SNES as six 16-byte
  packets (id + checksum + data, eight zero bytes of padding = 96 bytes, 768 bits) through
  an **unbalanced** bit loop: a set bit takes a taken `jr`, a clear bit falls through *and*
  runs a second `ld a`, so **a zero bit costs one M-cycle more**. The disassembly says so
  itself — "Both code paths aren't equal CPU time … NINTENDOOOOO"
  ([ISSOtm/gb-bootroms](https://codeberg.org/ISSOtm/gb-bootroms) `src/sgb.asm`).
  `mmu::sgb_boot_system_counter` is that model; the two ROMs independently give the same
  all-ones base `$D174`, which is what says the shape is right rather than fitted.
- **`P1 = $CF` is a `GbMode::Classic` fact, not a `GbMode::is_dmg()` one.** An SGB's last
  use of the port is clocking packets, ending on `$30`, so it leaves the lines **released**
  (`$FF`) like a CGB. Its last audio write is `NR14 = $07`, not `$87` — channel 1 is set up
  and never triggered, so **`NR52` reads `$F0` and an SGB has no start-up ding**.
- **The scouts must boot the machine the filename names, or the row is a runner bug, not an
  emulator one** (`examples/mooneye_scout.rs::model_for`). Two conventions: spelled-out
  (`-dmg0`, `-mgb`, `-sgb`, `-sgb2`, `-cgb0`) and Gekkio's single compatibility letters
  (`G`=DMG, `M`=MGB, `S`=SGB, `C`=CGB, `A`=AGB). **A suffix naming *several* machines
  (`-GS`, `-cgbABCDE`) must be left alone** — forcing one of them boots the wrong console
  for the rest.
- **Do not quote the community's 66-row denominator as if it were the whole suite.** It
  drops the nine named-machine ROMs precisely because most emulators cannot run them; our
  baselines score all 75 and all nine are green.

### The CGB strobes STAT one M-cycle before VBlank — and it must NOT reorder the two

- `misc/ppu/vblank_stat_intr-C` and `acceptance/ppu/vblank_stat_intr-GS` are the same
  program with one `nop` moved: on Color hardware the line-144 OAM strobe is one M-cycle
  **before** the VBlank flag, on a DMG they are simultaneous (`VBLANK_STROBE_LEAD`,
  `Gpu::oam_strobe`).
- **Implementing that alone cost two hardware-photographed Mealybug rows, and that was the
  real finding.** `mealybug/m2_win_en_toggle` enables both interrupts, *sets* `LCDC.5` in
  one handler and *toggles* it in the other, so the dispatch order inverts the window on
  every scanline — 18,060 of 23,040 pixels. Its `CPU CGB C`, `CPU CGB D` and `DMG-blob`
  reference photographs are **identical in shade rank** (checked, 0 pixels differ), so real
  Color hardware does *not* reorder them.
- **What reconciles the two: interrupt dispatch decides its vector in the middle of
  itself.** The vector is taken from `IE & IF` at M4, so VBlank's flag going up one M-cycle
  into STAT's dispatch **redirects it** to the higher-priority vector. The gap is still
  measurable when only one of the two is enabled, which is exactly how `vblank_stat_intr`
  enables them.
- **The defect that hid it was the batched PPU standing still for the whole dispatch.** The
  per-dot engine already advanced per bus access and got this right; the fast one applied
  all 20 T afterwards, so no PPU interrupt could ever redirect anything.
  `MemoryManagementUnit::set_ppu_bus_ticking` now runs the batched PPU with the bus for the
  five M-cycles of a dispatch (and *only* those — the dots are counted into
  `gpu_bus_ticked`, so total PPU time is unchanged). Both engines agree, Mealybug is
  byte-identical, and `Gameboy::frame` cost is unmoved (0.219 ms/frame fast).
- **Two verdict methods disagreeing is the finding** — the same rule as the boot-tilemap
  entry. A ROM's own report said the change was right and a hardware photograph said it was
  wrong; taking the flattering one would have shipped a regression as a win.

## Console model — DMG vs CGB (`src/mode.rs`, `Cpu::new_with_model`)

- **There are three machines, not two.** `GbMode::Classic` (a DMG), `GbMode::Color` (a CGB
  running a CGB cartridge) and `GbMode::ColorAsClassic` (**CGB hardware in DMG-compat mode**).
  The last two are what this emulator has always booted: `Cpu::new` called
  `MemoryManagementUnit::new_cgb` unconditionally, so a DMG-only cartridge got compat mode.
  `MemoryManagementUnit::new` (Classic) existed but **nothing in the product ever called it**.
- **`Model`** is the user-facing choice, resolved to a `GbMode` by
  `Model::resolve(cgb_flag)`. `Cpu::new` = `Model::Auto` = "CGB flag bit 7 set → Color, else
  DMG", i.e. what a person with both consoles would do. `Cpu::new_with_model` forces one.
  (It has since grown the console *revisions* — `Dmg0`, `Mgb`, `Cgb0`, `Agb` — see "Console
  revisions" above; `Model::NAMES` is the authoritative list.)
  `$GB_MODEL` overrides `Auto` only (an explicit caller choice always wins), so
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
  (`VolumeState.from_config`, `${XDG_STATE_HOME}/terminalgb/volume`). Without that fingerprint one
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
  **The NRx2 "zombie mode" cluster is CLOSED — all six rows** (`channel_{1,2}_nrx2_glitch`,
  `_nrx2_speed_change`, `_restart_nrx2_glitch`) pass on their own `RESULT_CODE`. It took
  two commits and neither alone was enough: `e2006f1` got the CGB-E formula over the
  register bits (the short half), this envelope split got the long half, whose answers
  depend on the counter's actual value. `e2006f1`'s own message honestly scoped the
  remainder as "not fixed" and was never amended, so briefs kept naming the cluster as
  the last open APU gap long after it was green — **check the baseline, not a commit
  message, before starting on a named gap.**
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
  `channel_1_freq_change_timing-{A,cgb0BC,cgbDE}` variants is winnable at all — and it is
  **`-cgbDE`, which passes now**; see "the frequency reload is latched late" below.
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
- **`CgbRevision` deliberately lumps C/D/E.** `-cgbDE` passes on the machine we already
  are; `-cgb0BC` needs C split from D/E **and** two more behaviours (see the AGB section
  below), so splitting it alone still buys nothing. `from_tag` returning `None` falls back
  to the default, so adding a tag stays additive. **`-A` is not a CGB revision at all** —
  it is a Game Boy Advance, and it PASSES now; see "The AGB is a console the APU knows
  about" below. An older version of this entry called it "a console we do not emulate, so
  passing it would be an accident": we do emulate one, for its boot state, and the accident
  was only that nothing told the APU.
- **`cgb_revision` is machine configuration, NOT cartridge state, so it is not
  serialised** — no `savestate::FORMAT_VERSION` bump. A save state restores into whatever
  machine is running, which is the behaviour you want.
- **`channel_4_freq_change` is red on a MEASURED reason, and the one that used to be
  written here was stale.** This entry quoted SameSuite's `apu/README.md` (*"SameBoy …
  passes all tests except `channel_4_freq_change` and `channel_1_sweep_restart_2`"*) as
  proof that no reference implementation existed. **Measured 2026-08-15 with
  `tools/sameboy-oracle/`: current SameBoy on CGB-E PASSES it**, and passes
  `channel_1_sweep_restart` too. That README predates its own emulator. The structure is
  the free-running 14-bit counter this file already predicted (tick every `(NR43 & 7) × 8`
  T, 4 for code 0; LFSR on the 0→1 edge of counter bit `NR43 >> 4`) — and our single
  `period` countdown is *algebraically identical* to it for a fixed NR43, which is why ten
  CH4 rows are green and why only an NR43 write can tell them apart. What blocks the row:
  searched exhaustively (both reload policies × every trigger countdown `1..=39` × every
  read offset `−8..=8`) the clean counter model reaches **61 of 64 cells** and the
  parameters that get there contradict what `channel_4_delay` pins. The rest is SameBoy's
  NR43-write glitch table, annotated in its own source *"non-deterministic"* / *"no clue
  why"*.
- **`apu/README.md` inside SameSuite is the first thing to read before grinding a row —
  and its claims about OTHER EMULATORS are stale, so check them.** Its per-revision table
  and the `.asm` `CorrectResults` tables are where every expectation in this section came
  from, and those are good; its "SameBoy fails X" line is years out of date and cost this
  repository two permanently-red rows that were never re-checked. Clone the source
  (`github.com/LIJI32/SameSuite`) for the tables, and run
  **[`tools/sameboy-oracle/`](tools/sameboy-oracle/README.md)** for anything about another
  implementation. It builds SameBoy at a pinned commit and prints the ROM's own verdict on
  any of nine consoles — `dmg mgb sgb cgb0 cgbB cgbC cgbD cgbE agb` — for SameSuite
  (`$CFFE`) and for Blargg (cartridge RAM). Not vendored, not in CI, same rules as
  `tools/bgb-interop/`. **Never quote a README about another project's results again.**

## The AGB is a console the APU knows about (SameSuite 72/76 → 73/76)

**`channel_1_freq_change_timing-A` FAIL → PASS**, APU half 65/69 → 66/69. Everything else
byte-identical: Mooneye 103/103 + 8/8 (`standard`) and 103/103 (`identical`), Mealybug 3/79
and 17/79 **per-row**, `cpp` 4/4, `testroms` 46/47 with the same verdict text,
`audiocapture` including every hash, `savestate`, `battery` 8/8, `ppuengines`, 665 lib
tests, `make embed` / `check-wasm` / `lint` / `docs-check` / `test-cross`. Full record:
[`docs/conformance-notes.md`](docs/conformance-notes.md) (newest entry) and
[`docs/measured/apu.md`](docs/measured/apu.md#the-frequency-reload-latch-is-narrower-on-an-agb).

- **A GAME BOY ADVANCE LATCHES THE SQUARE FREQUENCY RELOAD TWO T-CYCLES EARLIER THAN A CPU
  CGB D/E.** `SQUARE_RELOAD_LATCH` = 6, `SQUARE_RELOAD_LATCH_AGB` = 4. Sweeping the latch
  across the whole suite gives two adjacent 2 T bands that do not overlap — `3..=4` passes
  `-A` only, `5..=6` passes `-cgbDE` only, `1..=2` and `7` pass neither — and **nothing
  else in the 76 ROMs moves anywhere in `1..=6`**. So the console selection is the *whole*
  of the difference and no one machine can pass both, exactly like the suite's
  `-cgb0`/`-cgbB` pair.
- **The behaviour behind the two numbers**: the correction the reload applies to the duty
  position when a frequency write drops NRx4's high bits from 7 is unconditional on CPU CGB
  D/E and depends on the 2 MHz clock phase on an AGB and on CPU CGB C and older (SameBoy
  `Core/apu.c`, the `GB_IO_NR14` case — *"On an AGB, as well as on CGB C and earlier, it
  behaves slightly different on double speed"*). Read there, reimplemented here as the
  width of the window, which is what the ROM measures.
- **`Model::Agb` already existed for boot state; nothing told the APU.** The flag flows
  `MemoryManagementUnit::console` → `enable_audio` → `Sound::set_agb`, so **`GB_MODEL=agb`
  is the only user-facing knob** and there is deliberately no second one. `Model::Auto`
  still never infers an AGB. `Sound::agb` is machine configuration like `cgb_revision` and
  is **not serialised** — no `savestate::FORMAT_VERSION` bump.
- **`examples/samesuite.rs::console_for` boots the console the ROM's filename names**, the
  same rule `revision_for` already applies to `-cgb0`/`-cgbB` and `mooneye_scout::model_for`
  applies to Gekkio's single letters (where `A` has meant AGB all along). Only the `-A`
  suffix maps; everything else stays `Model::Auto`.
- Pinned device-free by `sound::tests::an_agb_latches_its_frequency_reload_two_cycles_earlier`
  (teeth proven by widening the AGB constant back to 6). Nothing in `cargo test` runs
  SameSuite, so that test is the gate.
- **The general rule this belongs to: a suite that ships several reference tables for one
  program is telling you there is more than one machine.** Score each on the machine it
  names; a runner that ignores the filename can never win more than one of them.

## `channel_1_sweep_restart` is NOT a sweep bug and NOT `SQUARE_RESTART_SPEEDUP`

143 of 144 cells match and the row is still red — but the cause recorded here for months
was wrong, and acting on it would have cost four green rows for nothing.

- **Traced against SameBoy through the failing subtest** (T-cycles, each from its own NR14
  write): a fresh trigger at freq `$7FF` steps at 12 there and **10** here; the sweep writes
  `$7F0` at 16400 with duty index 2 there and **16404 with index 3** here; the re-trigger
  steps at 68/index 4 there and **66/index 5** here.
- **The re-trigger is not the problem.** Duty 2's positions 3 and 4 both output 0, which is
  why SameBoy reads `$00` at 68 T *even though the step happened there*. We arrive at the
  re-trigger **one duty position further on**, having taken one extra 4 T step in the ~4100
  the channel makes at period 4 T before the sweep event.
- **The cause is that our DIV-APU event lands 2 T off the 1 MHz grid the duty steps are
  on.** On hardware it cannot: the DIV-APU edge is a multiple of 8192 T from the system
  counter's zero and a period-4 channel steps on every 1 MHz boundary, so the two come off
  one counter chain. **Neither half moves, alone or together** — `SQUARE_TRIGGER_DELAY` 6/7/8
  × DIV-APU event 0/−4/−6 T gives 73/59/59, 72/58/58, 47/39/39 out of 76. Rounding the
  trigger delay up to the grid, and `SQUARE_TRIGGER_ALIGN = 2`, are both `DELAY = 8` by
  another name at single speed (47/76).
- **It is a structural 2 T offset between where this APU places a bus access and where the
  1 MHz grid is**, invisible to all 75 other rows because every one of them measures a
  *relative* interval the offset cancels out of. Fixing it re-derives the APU's access phase
  and re-calibrates every constant in `docs/measured/apu.md`'s table. **Do not touch
  `SQUARE_RESTART_SPEEDUP` for this row.**

## The square frequency reload is latched LATE (SameSuite 71/76 → 72/76)

**`channel_1_freq_change_timing-cgbDE` FAIL → PASS**, APU half 64/69 → 65/69. Measured
with `make test-external` before and after; everything else byte-identical (Mooneye
103/103 + 8/8, Mealybug 79/79, `cpp` 4/4, `testroms` 46/47, `audiocapture` **including
every hash**, `savestate`, `battery`, audio-perf 0.253 ms/frame, 469 lib tests, and the
accurate-PPU gate). Full record: [`docs/conformance-notes.md`](docs/conformance-notes.md)
(newest entry).

- **A frequency write does not merely "take effect at the next expiry".** The timer
  samples its reload value a few T-cycles *after* the expiry, so a write inside that
  window supplies the period the timer has **just** reloaded with, and the next sample
  lands one whole *new* period from that expiry instead of the old countdown finishing.
  `SquareChannel::write_freq`, `SQUARE_RELOAD_LATCH`.
- **The constant is measured and the band is narrow: `5..=6` T, uniform at BOTH speeds.**
  The ROM sweeps the write across the expiry one `nop` at a time, single *and* double
  speed, reading PCM12 immediately and again once the channel has frozen — so it pins the
  window from both ends at both clock rates. `≤4` moves the verdict onto `-A` (the AGB
  table), `≥7` fails all three. **Do not "tidy" it to a round 4**, and do not make it
  speed-dependent — that was tried and only a uniform value fits.
- **`period - delay` IS NOT "time since the last expiry", and inferring it costs two
  rows.** A freshly triggered channel walks that same range with no expiry behind it (a
  trigger loads the timer itself), and `channel_{1,2}_freq_change` write NRx3 exactly
  there — the first attempt derived the window arithmetically and turned both FAIL.
  Hence `SquareChannel::since_expiry`, tracked in `run_span` and parked at `u32::MAX` by
  a trigger. It is live machine state: `savestate::FORMAT_VERSION` is **15**.
- **Nothing outside SameSuite sees this.** `dmg_sound` stays 12/12 with every captured
  audio hash unmoved on both sides — a whole class of sub-sample APU timing is invisible
  to a suite judged by ear or by checksum. That gap is the finding, not an aside.
- **Cost: ~1.3 µs/frame** on the APU's own figure (`apu_bench`, min of 5: 26.7 → 28.0
  µs/frame) — 0.008 % of a 16.743 ms frame; one comparison per NRx3/NRx4 write plus one
  `Option` store per channel span.
- **Two red rows here are deliberate and evidenced** — `-A` is an AGB, `-cgb0BC` is the
  undocumented PCM12 read-back glitch of revisions C and older. See the `CgbRevision`
  entry above, which this work corrected.

## Terminal frontend (TUI)

- The terminal frontend is a proper `ratatui` TUI split across three files in `src/screen/`:
  `config.rs` (config + recent-list, hand-rolled `key = value` parser — **no config crate**),
  `romlib.rs` (cartridge-header title/MBC reading, directory scan, fuzzy filter — pure,
  file-system-only, heavily unit-tested), and `tui.rs` (the App: picker screen + game screen,
  one single-threaded event loop, help/quit overlays, centred display).
- **Entry point is `terminalgb::run_terminal(Option<PathBuf>)`** — `Some(path)` launches straight
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
- **A WALL-CLOCK LOCKOUT MEASURES YOUR OWN LATENCY, NOT THE TERMINAL'S — AND ON A LOADED BOX
  THAT IS THE SAME BUG AS HAVING NO LOCKOUT.** The event loop takes **one event per
  iteration**, so the gap `CONTROL_REPEAT_LOCKOUT` compares is between *our handling* of two
  keys; one iteration is an emulated frame plus a draw, and under load it can outlast 200 ms,
  at which point a held key strobes again. `App::input_queued_behind` closes it: the loop
  samples `event::poll(ZERO)` after reading each event, and a key that was **already queued
  behind its predecessor** cannot be a fresh human press however long we then took to reach
  it. Strict addition — the timed rule still decides every key that arrives on an idle queue.
  Sampled for *every* event kind, not just keys: set by a key and then consumed by a resize, a
  stale `true` would swallow a real press. Pinned by
  `a_stalled_loop_does_not_unsuppress_auto_repeat` (teeth proven).
- **AND THE OTHER HALF WAS THE HARNESS: `sleep()` IS NOT A CLOCK ON A BUSY MACHINE.**
  `testharness/basicterm.rs` used to pace its burst with twelve writes 30 ms apart, so under
  load *it* delivered presses far enough apart to genuinely be presses — and the emulator was
  then right to act on them. Reported: 2 failures in 3 runs at load average 8-15, green when
  quiet. It writes all twelve bytes in **one** `write_all` now; a burst on the wire is what
  auto-repeat is, and how fast the harness can pace one was never part of the claim. Verified
  3/3 at load average 62-72. **General rule for any pty/subprocess harness here: never encode
  the property under test in your own `sleep` spacing** — put it in one write and assert on
  the response.
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
  `terminalgb::run_terminal` as a child in its own session (`setsid` + `TIOCSCTTY`, so even the
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
  `$TERMINALGB_CELL_SCALE` (or the pre-rename `$GAMEBOY_CELL_SCALE`) overrides for testing. Proven: at `142×46` cells, `disp_px` 784×704 →
  1568×1408 with `cell_scale=2`, i.e. it now fills the physical window. **Screenshots are blocked
  (GNOME/Wayland denies the D-Bus screenshot and `import -window root` is empty), so this was
  verified by the logged numbers, not by eye** — a human on a HiDPI display must confirm the
  picture visibly fills. `GB_DEBUG_LAYOUT=<file>` dumps term cells, queried/effective font,
  area, fit rect, scale and `disp_px` (gated, zero-cost when unset).
- **Note for HiDPI users:** the correction is **opt-in** (default `1.0` never regresses low-DPI /
  XWayland). A native-Wayland HiDPI user must add `cell_scale = auto` (or an explicit `2.0`) to
  their `config.toml` for the picture to fill.
- Config lives at `${XDG_CONFIG_HOME:-~/.config}/terminalgb/config.toml`; recents at
  `${XDG_STATE_HOME:-~/.local/state}/terminalgb/recent` (with a pre-rename `gameboy/`
  fallback — see "The rename to TerminalGB" below). Missing config = defaults, never an
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
  - *A missing save file that `std` cannot call missing.* Symptom: a cartridge copied to a
    memory stick with no `.sav` beside it yet refuses to start — `could not read the
    existing save file`, for a file that never existed. Caught by: nothing, and nothing
    here could — every harness runs on a filesystem whose `std` classifies a missing file
    correctly. Found by running the real PSP build (its sysroot overlay reports
    `ErrorKind::Uncategorized` from both `fs::read` **and** `fs::metadata`, so
    `Path::exists` is no help either). Fixed by asking the **directory listing**, which
    does work there, and failing towards "present" so *"a save that cannot be read is never
    overwritten"* is untouched; `battery::listed_in_parent`, pinned by
    `a_missing_save_is_absent_and_a_present_one_is_not`.
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
  `terminalgb/states/<sanitised-title>.slot<N>.gbss` (pre-rename `gameboy/` fallback).
- **Save-state capture must track new machine state.** After the accuracy crew's fixes it now
  also covers `Cpu.halt_bug` (HALT-bug latch — can be live at a frame boundary) and the MMU's
  `bus_tick_enabled`/`bus_ticked` (per-access timer stepping; reset between instructions but
  captured for completeness). Any future CPU/MMU/PPU field that survives across a `do_cycle`
  boundary MUST be added to the `save_state`/`load_state` pair or `savestate` round-trip desyncs.
- **Terminal pacing:** the speed ladder gates `frame()` on the true 16.743 ms DMG frame time,
  so `1x` is real-time (fixing the old 5 ms/~200 fps terminal bug the audio crew flagged);
  `2x`/`4x` shorten the interval and `MAX` runs an uncapped burst.

## `docs/measured/` — the reader-facing distillation, and the Pan Docs licence correction

- **PAN DOCS IS CC0 1.0 UNIVERSAL — PUBLIC DOMAIN. IT IS NOT CC BY-SA.** Verified, because the
  wrong belief was in circulation and the two licences imply opposite obligations:
  `gbdev/pandocs`'s `LICENSE` is CC0 1.0, added in **one commit in January 2020 and never
  changed since**, and its README says *"Pan Docs is released into the public domain."* So
  there is **no copyleft hazard for this MPL-2.0 repo** and no legal barrier to quoting it.
  A crew acting on "CC BY-SA" would refuse things that are perfectly fine.
- **The reason we still do not reproduce Pan Docs is editorial, not legal** — a second copy of
  a maintained reference goes stale and becomes a liability. Cite and link generously; do not
  fork it. That distinction is stated on the section's entry page so it cannot drift back.
- **[`docs/measured/`](docs/measured/README.md) is "the measured Game Boy"** — eleven pages
  (`README`, `scoreboard`, `timing`, `ppu`, `apu`, `boot`, `cartridges`, `serial`, `sgb`,
  `host`, `symptoms`) built on the one thing gbdev does not have: *our measurements*. Shape of
  every page: **what the reference says (cited, linked, never restated) → what we measured →
  the test that pins it → what a wrong emulator does.** The three newest cover the subjects
  whose corpus entries had no home: **`serial.md`** (the link port — the half no ROM-driven
  suite can reach), **`sgb.md`** (the ICD2 command channel, and scope as the interesting
  question), **`host.md`** (defects that leave the framebuffer perfect — the pacing clock, the
  build configuration, the audio device, the terminal).
- **A SCORE WRITTEN INTO A SENTENCE IS GATED NOW — wrap it, or it will go stale and CI will not
  notice.** `docs/tools/status_numbers.py` used to verify only the *generated* table on
  `docs/status.md`, so hand-written prose was invisible to it; `docs/measured/scoreboard.md`
  carried "Mealybug 3 vs **12**/79" for as long as it took a human to spot, five rows after
  `identical` reached 17. Every current suite score in `docs/measured/` is now wrapped
  `<!--score:mealybug_accurate-->17/79<!--/score-->`, rewritten by `make status` and verified
  by `make docs-check` (59 of them today). Suffixes: `:pass`, `:total`, `:px`, or the default
  `pass / total`. Only the *digit runs* inside the marker are substituted, so emphasis and
  spacing survive a re-bless. Add a new page to `INLINE_PAGES` to opt it in.
  - **A regex hunting prose for `\d+/\d+` was tried and rejected** — `docs/measured` is full of
    legitimate non-score fractions (a constant sweep printing `84/77/47/12/53`, "3 of 8
    sub-tests", "`oam_bug` 4/5/6/8"), so it cried wolf. Naming the suite cannot be wrong.
  - **NEVER put a marker inside a heading** — a heading is a link target, and burying an HTML
    comment in one produces an anchor nobody can guess. Two links broke exactly that way;
    `headings_with_scores` now fails the build instead. Put the score on the line below.
  - **Deliberately ungated, and the page says so at the top:** the Shootout figure (not
    baseline-derivable), other emulators' published scores, historical attribution figures
    (`13/66 → 52/66`) and measured counterfactuals ("moving that constant costs 56/76"). Those
    must *not* track the baselines.
- **`make docs-check` still does not verify in-page anchors, so check them by hand after
  editing headings** — the ~20-line slug checker used for this work is in the commit that added
  these pages; the rule is below.
- **Which file gets a new entry: `docs/conformance-notes.md`, always.** That is the raw
  corpus, chronological, written at the moment of the fix. `docs/measured/` is the same
  material organised **by subject for a stranger**; distil into it when a subject accumulates
  enough entries. Do not fork the corpus into two chronologies.
- **`make docs-check` does NOT check in-page `#anchor` links** — only file/dir targets and
  line ranges. Broken anchors are silent, and two pre-existing ones in `docs/testing.md` were
  found this way. GitHub's slug rule is: lowercase → drop everything that is not
  word/space/hyphen → replace **each** space with a hyphen, **without collapsing runs**. So
  `— ` and ` / ` both leave a **double** hyphen (`Mealybug Tearoom — 3 of 79` →
  `mealybug-tearoom--3-of-79`). A checker that collapses whitespace reports false positives;
  one that ignores anchors misses real breakage.
- **`codemap_links.py --check` rejects any `github.com/<owner>/<repo>/blob/…` link that is not
  ours**, so you cannot cite a file in someone else's repo that way. Link the repo and name
  the file, or use a non-`blob` URL form.
- **WHICH OTHER EMULATORS MAY BE READ, AND WHAT MAY BE DONE WITH WHAT YOU FIND — verified, not
  recalled, and tabulated on `docs/measured/README.md`.** **mGBA is MPL-2.0 — the *same*
  licence as this repository** (checked against `mgba-emu/mgba`'s root `LICENSE`, which is the
  Mozilla Public License 2.0 verbatim, and its README: *"distributed under the Mozilla Public
  License version 2.0"*; its bundled third-party components — inih BSD-3, LZMA SDK /
  MurmurHash3 / getopt-for-MSVC / SQLite3 public domain — are separately licensed). That is
  **materially stronger than docboy (MIT) and SameBoy (Expat, except `iOS/` and `HexFiend/`)**:
  MPL-2.0 → MPL-2.0 means its code is *reusable* with file-level attribution and the notice
  retained, not merely readable for behaviour. Even so, **prefer reimplementing in this
  codebase's idiom per `CONTRIBUTING.md`, and cite repo + file + symbol for anything taken.**
  - **A borrowed structure that does not fit this codebase's shape can be worse than an
    approximation of it** — docboy's own fetcher-during-an-object-hit model costs 64,784 extra
    Mealybug pixels here against a one-dot approximation's 151, because our neighbouring code
    charges Pan Docs' *formula* where theirs counts real time. Read the behaviour; measure the
    port.
- **`src/sound.rs` is in `SRC_EXCLUDE`, so its references stay file-level.** Do not write
  `src/sound.rs:1243` in link text there — the tool will not anchor it and the tool's own
  comment calls the leftover `:line` a *stale hint*. Name the symbol instead.
- **Two stale claims found and fixed while writing this section** (both contradicted by later
  measurement in the *same* repo, which is why the standing "flag stale notes" rule earns its
  place): `docs/testing.md` called a cycle-stepped PPU *"a structural commitment the project
  has not made"* thirty lines above its own description of the gated pixel-FIFO engine; and
  `docs/components/ppu.md`'s "Honest approximations" described fixed mode 3, coarse line
  stepping and whole-scanline rendering unqualified, while §4.0 of the same page documents the
  per-dot engine that does all three properly. Both now say *"this is the default engine"* and
  link to `docs/measured/ppu.md`.
- **Verify a doc by rendering it through GitHub's own API and looking at it** (recipe in the
  README-verification section below). Two things that recipe's caveats do not spell out:
  the preview needs an explicit **`<meta charset="utf-8">`** or every em dash renders as
  `â€"` and looks like a corrupt source file — `file docs/**/*.md` confirms the source is
  fine; and delete the preview directory before committing.

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

## Half-blocks are OURS now — `ratatui-image` smooths them (`src/screen/halfblocks.rs`)

**Full story, the before/after screenshots in the captain's own terminal, the per-terminal
matrix and the guard: [`docs/display.md`](docs/display.md) §1 "The half-block path
smoothed".** Reported as *"running gameboy on our normal terminal is blurry again — so only
wezterm is looking good, we have corrected that before"*.

- **`ratatui_image::protocol::halfblocks::encode` HARD-CODES `FilterType::Triangle`** in its
  `resize_exact(w, h * 2, …)` onto the cell grid, so every cell was a weighted average of the
  Game Boy pixels under it. **Its sixel encoder does not, and `Resize::Scale`'s own default
  filter is `Nearest`** — half-blocks is the single path in that crate with a filter baked in,
  which is exactly why WezTerm (kitty/iTerm2) looked right and Ptyxis (VTE → half-blocks) did
  not. *Re-check this if the crate is ever bumped: it is upstream behaviour we route around,
  not a bug we fixed there.*
- **This was NOT the desktop `gl::LINEAR` regression coming back**, and the first instinct —
  "the old fix regressed" — is wrong. Same *class* (something downstream interpolating), a
  different layer, three frontends now crisp by three different mechanisms: desktop
  `gl::NEAREST`, web `image-rendering: pixelated`, terminal `screen::halfblocks`.
- **A COLOUR COUNT IS THE BLUR DETECTOR, NOT A PIXEL DIFF.** The Game Boy emits exactly four
  shades and every palette maps them to four colours, so "a colour on screen that is not one of
  the four" is a **categorical** error with no threshold to tune, and it holds at every window
  size, palette and scaling ratio. Measured on dmg-acid2 at 80x24: **271 distinct colours in a
  four-shade picture**. Guarded at two layers —
  `screen::halfblocks::tests::no_colour_appears_that_was_not_in_the_framebuffer` (unit, a
  one-pixel checkerboard, five sizes) and
  `screen::tui::tests::the_halfblock_picture_uses_only_real_game_boy_shades` (end to end
  through the real `App::render`, four sizes). The second **is** the reproduction: against the
  unfixed code it reports "268 colour(s) no Game Boy shade maps to".
- **Sample at pixel CENTRES** (`(2i+1)·s / 2t`), the same rule and the same reason as
  `video::fit_maps`: edge sampling never reaches the last source column or row. A column whose
  centre lands exactly on a source-pixel boundary is a tie and cannot be symmetric — the
  symmetry test skips those explicitly rather than picking sizes that hide it.
- **A uniform cell is written as a space, not `▀`.** Visually identical, but the terminal
  rasterises no glyph, so a font with a poor `▀` cannot soften the large flat areas a Game Boy
  picture is mostly made of.
- **It is CHEAPER than what it replaces**: whole-frame `frame_ms` **0.460 → 0.349** over ~1,070
  frames at 146x54 cells (telemetry, WezTerm forced to half-blocks). Ours pays its full
  sampling cost on *every* frame — `ratatui`'s per-cell diff is what suppresses an unchanged
  screen — and still wins, because the old path did **two** `image`-crate resizes.
  `encode_ms` alone (0.013 → 0.073) is misleading: the library's per-frame cell copy is billed
  to the draw, not to `encode_ms`. **Compare `frame_ms`.**
- **`render_path = legacy` still takes the library's path for half-blocks**, deliberately, so
  one binary can A/B the two in the same terminal — the `GB_TERM_IMAGE=rgb` precedent.
- **Crisp is not full resolution, and the product says so.** Half-blocks resolution *is* the
  character grid (~87x78 at 80x24 against 160x144). `HALFBLOCKS_NOTICE` rides the
  `config_warnings` channel (the `BASIC_INPUT_NOTICE` pattern), and the Status `Proto` row now
  carries the picture's **real resolution** instead of the transport word `full`, which meant
  nothing there — half-blocks has no wire transport, it goes through `ratatui`'s buffer.
  **Push that notice from `App::with_warnings`, never `App::build`**: every render test builds
  a half-block `App`, so a warning added in `build` puts a toast over the picture in all of
  them. Same reason the volume and the link cable live there.
- **The `Proto` row is one frame behind** (`disp_cells` is set by `render_display`, and the
  sidebar is drawn first), exactly like `Sent`. A test must draw twice.
- **Reproducing in the captain's own terminal, headlessly**: `gtk4-broadwayd :7` then
  `GDK_BACKEND=broadway BROADWAY_DISPLAY=:7 ptyxis -s -- <cmd>`, screenshot
  `http://localhost:8087`. Two traps that cost real time: **one client at a time** — a second
  connect (or a `navigate` to a different path) leaves the page frozen on a stale surface
  showing `FPS 0`, which reads exactly like a hung emulator, so restart `broadwayd` and connect
  **once**; and `pkill -f "ptyxis -s"` matches your own command line (the standing `pgrep -f`
  trap). Check `ps -o wchan` before believing a frozen picture — `ep_poll` at 4 % CPU is a
  healthy emulator with a dead viewer.
- **Force the protocol to isolate the variable.** `protocol = halfblocks` in WezTerm gives the
  same code path in a terminal that *can* be screenshotted with `import`, with font, cell size
  and window identical — which is what proves the softening is ours and not the terminal's.
  `Scale` in the status panel fluctuates between runs when the capability probe does (measured
  3.4x once, 9.6x on three re-runs of the same binary); re-run before reading anything into it.

### EVERY picture needs the half-block fallback, not just the game picture

- **`new_term_image()` returns `None` for anything that is not Kitty or iTerm2, and
  `place_panel_image` returns immediately on a missing image — so any panel picture built on
  that pair is INVISIBLE in a VTE-family terminal and says nothing about it.** The music-mode
  cover was exactly that for its whole life (reported as a regression; it never worked), while
  the panel beside it still printed `▦ cover 1/4 · … · c to change`. The game picture had had
  `screen::halfblocks` since the blurriness work; the cover simply never grew one, and neither
  did anything else that copies the `place_panel_image` shape. **When you add a picture to a
  panel, add its `m.image.is_none()` arm in the same commit** — `halfblocks::render` is a free
  function over `(rgba, w, h, rect, buf)` and needs no state.
- **A half-block picture must NOT take the `erase_cover`-under-a-popup exit.** That exit exists
  because a *placed* still is deduplicated and its cells marked `skip`, which eats a popup drawn
  across it. Half-block cells are ordinary buffer cells with `skip` clear and the overlay is
  rendered after the screen, so ratatui's `Clear` handles it. Gate that early return on
  "there is a placed image", not on the overlay alone.
- **Sixel gets half-blocks for a panel picture, deliberately.** It has no cell-addressed
  placement, so the alternative is `ratatui-image`'s resize protocol, which puts the whole
  payload in one cell symbol — render-conformance defect #2, the whole screen re-emitted. For a
  *still* sleeve a crisp lower-resolution picture that always works is the better trade; say so
  rather than leaving the inconsistency unexplained.
- **Cost, measured (release, best of 5,000 draws): 7.8 µs at 40×25 cells, 14.0 at 56×34, 27.9 at
  80×48** — 0.05-0.17 % of a 16.743 ms frame, paid only on a frame that screen redraws. The
  game's frame path (`render_display`, `step_frame`) is not on this route at all.
- **The colour count is the end-to-end proof, and it is categorical.** Measured on the real
  frontend forced to `protocol = halfblocks`, cropping the cover rectangle out of an `import`
  screenshot: **before 1 distinct colour** (the terminal's own background — the area is empty),
  **after exactly 4**, and they are `000000/606060/c0c0c0/ffffff`, our DMG shades to the byte.
  `magick <crop>.png -format '%k' info:` is the whole measurement. Same rule as
  `no_colour_appears_that_was_not_in_the_framebuffer`, applied through a real terminal.

### A key bound on one screen and not the other is a silent dead key

- **`M` was bound only in the picker's handler**, so pressing it in a running game did nothing
  whatever — the defect this frontend's own cable-key comment names ("a key that does nothing is
  worse than no key at all"). It is bound in game now and **`enter_music` puts the cartridge
  down first** (`unplug_net_link` + `session = None`, exactly as `L` does): `autoplay_when_ready`
  refuses to drop the needle while `session` is occupied, so without that teardown the shelf
  appears and is permanently silent.
- **`L` was labelled `load ROM`** — where it leads, not what it does — which hid that it is the
  route back to the picker. It is `L picker` beside `M music` now, adjacent in `CONTROLS` so the
  panel packs them onto one row. Cost: the Game group goes 10 → 11 `Half` entries, i.e. **one
  more panel row**, so at ~34 rows `q quit` is clipped where it used to fit (it is still on the
  top bar). At 44 rows nothing is lost and `the_controls_panel_is_rendered_from_the_registry`
  pins the whole list.
- **The `?` overlay does not scroll and is already longer than its popup, so fold a new key onto
  an existing line.** `R reset  L/M picker / music` costs no line; a new line silently deletes
  one at the bottom and reddens `the_help_says_the_feed_is_game_only_and_where_to_go_instead`.
- **A digit range that `return`s unconditionally swallows the whole range.** The save-slot arm
  matched `'1'..='9'` with only three slots, so `4`-`9` did nothing and said nothing; it also
  carried no `is_game_button` guard, unlike every neighbour beside it, so a digit bound as
  `key_a`/`key_b`/`key_start`/`key_select` never reached the game either. Both fixed. **When you
  add a control key to that match, copy the `if !self.is_game_button(&key.code)` guard** — the
  four pad keys are configurable and any character can be one.

### Driving the real frontend for before/after screenshots — two traps that cost a run each

- **`WEZTERM_UNIX_SOCKET` cannot be chosen: the socket is named after the GUI pid.** Exporting a
  made-up path makes `wezterm cli send-text` fail silently, every key is dropped, and the
  screenshots then "prove" that a key does nothing — which is indistinguishable from the bug you
  are testing. Launch first, then `ls -t /run/user/$(id -u)/wezterm/gui-sock-* | head -1`, and
  **always send a control key with a visible effect first** (`[` cycles the palette: greyscale →
  `#E0F8D0/#88C070/#081820`) to prove delivery before believing any negative result.
- **`--config enable_kitty_keyboard=false` is the right setting for this**, not a compromise: it
  is VTE's real behaviour, so it reproduces the captain's input path (`Input basic · autorepeat`
  in the Status panel) *and* it is the mode `send-text` drives reliably.
- A toast lasts ~2.5 s, so sleep well under that between the key and `import` or the row is
  empty and reads as "nothing happened".

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

## The rename to TerminalGB — what moved, and the three things that deliberately did not

The project is **TerminalGB**. The name was already in the product (the terminal window title
and the virtual camera's idle card) before the codebase used it; the rename made them agree.

- **What is now `terminalgb`:** the crate and its lib (`use terminalgb::…`), the plugin crates
  (`terminalgb-plugin-*`, and the directory `crates/terminalgb-plugin-api/`), the MCP binary
  (`terminalgb-mcp`) and the `SERVER_NAME` it reports to a client, the FFI header
  (`terminalgb.h`) and static lib (`libterminalgb.a`), the wasm module (`terminalgb.wasm`),
  and the user-visible strings (desktop `with_title`, picker header, quit prompt, the
  `terminalgb:` prefix on warnings and `report_fault` messages).
- **`Gameboy` and the `gameboy` module are NOT renamed, on purpose.** The *product* is
  TerminalGB; the *machine it emulates* is a Game Boy. `src/gameboy.rs`, `crate::gameboy`,
  the `Gameboy` type and the `GAMEBOY` FFI static all keep their names, so
  `use terminalgb::gameboy::Gameboy` is the correct spelling and is not a leftover.
- **The GitHub repo is still `Alchemy86/gameboy`** — not the crew's call, and renaming it
  would break every clone, remote, badge, permalink and the ~737 generated `path:line` links
  `docs/tools/codemap_links.py` emits (`CODEMAP_REPO` defaults to that slug). `cd gameboy`
  after a clone is therefore correct in the README and the MCP docs.
- **THE USER'S DIRECTORIES ARE NOT MOVED, AND MUST NOT BE.** `screen::config::app_dir_in`
  (and `prefer_existing` beside it) prefer `terminalgb` and fall back to the pre-rename
  `gameboy` name **when the old one is what exists on disk** — for the config file, the state
  directory (recents, remembered volume, save states), screenshots and recordings. The config
  is resolved at *file* level, not directory level, so a `terminalgb/` directory some other
  feature created cannot hide a real `gameboy/config.toml` beside it. A migration that
  half-succeeds — or that runs while a second copy of the emulator is writing — is worse than
  a fallback that always works. Four tests pin it, teeth proven by making `prefer_existing`
  return `current` unconditionally (two go red). Anything new that wants a per-user directory
  goes through `app_dir_in`; do not spell `"terminalgb"` at a call site.
- **Untouched by design:** `<rom>.sav` / `.gbsave` / `.gbss` filenames and locations (a rename
  that orphans somebody's saves is a bad rename — verified by booting the captain's real
  Pokémon Blue battery save through `terminalgb-mcp` and reading MEWTWO lv70 back out); the
  `palette = gameboy` config alias; `ot_name: "GAMEBOY"` in `plugins/gen1-link/src/spec.rs`
  (that is a Pokémon's original-trainer name on the wire, i.e. game data); the gg8-wiki
  citations ("Gameboy sound hardware"); `web/`'s CSS classes and wasm-pack `outDir` (they name
  the drawn console shell and an unverifiable npm build); and the `/tmp/gameboy-*` scratch
  paths, including `/tmp/gameboy-gen1-trade/cableclub.sav` — that fixture cannot currently be
  regenerated, so renaming its path would orphan it.
- `$TERMINALGB_CELL_SCALE` is the new spelling of `$GAMEBOY_CELL_SCALE`; the old name is still
  read as a fallback in the same expression. `GB_*` env vars were never renamed — the console
  really is a GB.
- **The launcher `~/.local/bin/gameboy` lives outside this repo** and is the command the
  captain types. Nothing here renames it and nothing here should; if a `terminalgb` command is
  ever added, `gameboy` must keep working.
- Sweeping a rename like this: protect the not-ours tokens *first* (`Alchemy86/gameboy`,
  `raphamorim/gameboy`, `crate::gameboy`, `CableClubLeftGameboy`, the XDG paths), then apply
  the token rules, then **read every survivor**. Two traps this one hit: a `gameboy.h` rule
  chewed `gameboy.height`, and a `terminalgb::` rule turned `terminalgb::gameboy::` into
  `terminalgb::gameboy::`. Both compiled-or-not immediately; a prose-only file would not
  have told you.
- **A bare `grep terminalgb::` over this tree is NOT a stale-rename check, and will read as a
  regression when it is not.** It matches inside `terminalgb::gameboy::`, which is the
  *correct* post-rename spelling, and inside the built `docs/*.module.wasm` binaries, which
  are deliberately not swept. Anchor the pattern (`grep -v 'crate::terminalgb::\|terminalgb::gameboy::'`
  and exclude the wasm) or the answer is ~20 false positives every time. Two crews reached
  that grep independently within an hour of the rename, which is why it is written down.
- **Check a cross-branch worry by trial-merging, not by comparing line numbers.** Treehouse
  worktrees share one object store, so `git merge-tree --write-tree <ours> <theirs>` resolves
  any other branch by SHA with no fetch, no checkout and no risk to either branch: exit 0 and
  no conflict markers is the answer, and `git show <tree>:<path>` then says what the merged
  *content* is. Line distance is silent about content; this is not. Used here to prove the
  rename survives another crew's edits to the same three files, from both directions.

## Music mode — cartridges as albums (`src/music/`, `src/screen/music_view.rs`)

**Full write-up: [`docs/music-mode.md`](docs/music-mode.md).** Behind the `music` cargo
feature (in `default`); `terminal music` or `M` from the picker. Read that page before
touching the cover picker — every constant in it is measured against real cartridges.

- **Track selection has no general solution and the UI must SAY which one it got.** A Game Boy
  cartridge exposes no "play track 5" interface — no sound-driver entry point, no track table.
  Booting it and listening always works and gets the **title theme**; a game's own **sound
  test** or its **sound driver** would get whole soundtracks and need per-game knowledge, which
  belongs in a plugin. `library::TrackSource` exists as a *type* rather than a comment so the
  claim is checkable, and `the_shelf_never_claims_a_soundtrack_it_did_not_get` /
  `the_panel_never_promises_more_than_a_title_theme` render the strings and grep them. **Do not
  soften those words without moving the tests.**
- **Cover art: score SCENES, not frames.** Scoring each sampled frame on its own picks whatever
  looks busiest, and measured on real cartridges that is *a fade for Pokémon Blue and the sea
  behind Pokémon Gold's intro*. The property that identifies a title screen is that it **stays
  there**, so `cover::Picker` groups samples into scenes and scores `duration × detail`. Three
  parts are load-bearing: **a cut is measured against the frame that OPENED the scene, never
  against the previous sample** (a slow pan moves a fraction of a per cent per sample, so
  neighbour comparison swallowed a whole 60-second intro into one "scene"); **detail is the
  count of distinct 8×8 blocks**, not edge density, because a dithered sky is a couple of tiles
  repeated and edge density rates it as the most detailed thing on screen; and both terms are
  needed, since duration alone is a black screen.
- **The probe window is 75 s and that is measured, not chosen.** Blue reaches its title screen
  at ~25 s, Gold/Silver at ~55–65 s after a long intro. At 26 s Gold offered the sea. ~2–4 s of
  CPU per cartridge, on a **worker thread** — the shelf is browsable from the first frame with
  every album's header title, and albums fill in.
- **A cover candidate is a MOMENT, not a picture: it carries a save state of the machine at its
  scene's first frame.** Playing and exporting both resume from it, which is the only way the
  picture and the sound agree on a game with a minute of intro. Cycling covers with `c`
  re-drops the needle for the same reason. Consequence: an album does not start until its probe
  lands (~2 s, header says `listening…`). Starting at reset and seeking later was tried and is
  worse — the first seconds are a silent logo, so it buys no music and then jumps.
- **The sleeve is a STILL, deliberately** — it was written live first. A title screen is not
  where a game *stays*: Pokémon Blue rolls into an attract demo after ~20 s, so a live sleeve
  quietly becomes gameplay footage. `g` is how you get the live picture.
- **MUSIC MODE MUST NEVER OPEN A SAVE FILE.** Every machine it builds passes `filepath = None`,
  so the MBC has nowhere to write; the mapper still comes from header `$0147`. It boots every
  cartridge on the shelf, and overwriting a real save with the pristine RAM of a
  thirty-second listen is the bug `src/mbc/battery.rs` exists because of. Pinned on **both**
  paths (`probing_a_battery_cartridge_writes_nothing_beside_it`,
  `playing_an_album_never_writes_a_save_beside_the_rom`), both proven to fail when the path is
  put back. **The fixture must WRITE to cartridge RAM** (`testrom::battery_rom`) — one that
  merely declares RAM passes either way, because the battery layer writes nothing for a session
  that never touched it. That trap cost a first version of the test its teeth.
- **No plugins are attached.** A trainer panel or link partner appearing because you opened a
  music player would be a surprise.
- **The WAV encoder is `terminalgb_chiptune::wav`, shared with the chiptune renderer and
  `testharness/audiocapture.rs`.** There is one in the workspace, agreed before either crew
  wrote a line. Do not add a second.
- **Export re-boots the cartridge rather than tapping the live stream** (deterministic, starts
  at the beginning of the theme, does not perturb playback) and runs **on its own thread** — a
  minute of audio is ~1 s of CPU and the audio queue's cushion is ~41 ms.
- **`AudioVerdict` has four states because "silent" and "the sound chip never ran" look
  identical from outside and mean opposite things.** `Sound::do_cycle` returns immediately with
  no `AudioPlayer` installed, so a capture harness without one has *no APU at all*. The
  recorder counts its own calls; the panel says a missing APU is a fault in the emulator.
- **Fixtures are hand-assembled (`src/music/testrom.rs`), so nothing here needs a game.** The
  same licence boundary as everywhere else, applied to pixels and samples: art and audio are
  rendered from the user's ROM at run time and **never written into this repository**.
  `examples/music_probe.rs` refuses a `--covers`/`--wav` destination inside the working tree.
- **Evidence tool: `cargo run --release --example music_probe -- <dir> [--covers D] [--wav D]
  [--timeline]`.** `--demo` uses the built-in fixtures and needs no ROM at all. Measured on the
  three cartridges here: **3/3 usable covers, 3/3 audible title themes**.
- **A still picture's cells are `skip`ped, so a popup drawn across them is
  clipped.** The help overlay rendered with the right half of every line missing,
  cut at the cover's edge; `render_cover` now erases the cover while a popup is up
  and *invalidates* it so it re-transmits when the popup closes (a still cover
  would otherwise deduplicate to "unchanged" and never come back). The game
  picture never hit this because a running game changes every frame. **Found and
  verified by screenshot, and no unit test can catch it**: `ratatui`'s `Clear`
  calls `Cell::reset`, which clears `skip` over the popup's own rectangle, so a
  `TestBackend` always flushes the popup and the test stays green with the fix
  removed. Say that rather than claiming teeth.
- **A cover letterboxed with black bands is NOT a music-mode bug.** The terminal letterboxes a
  placed image when the cell rectangle's aspect does not match 160:144, and the *game* picture
  has identical bands on the same display. `music_view::fit_cover` deliberately uses the same
  arithmetic as `fit_metrics` so the two cannot diverge; fixing the bands belongs with the
  render path.

## Repo furniture & attribution (fork of raphamorim/gameboy)

- **This is a fork of [raphamorim/gameboy](https://github.com/raphamorim/gameboy)** (itself
  modelled on [rboy](https://github.com/mvdnes/rboy)), now at **Alchemy86/gameboy**, MPL-2.0.
  Attribution is a **standing requirement, not decoration**: keep the original author credited
  in `Cargo.toml` `authors` (alongside "TerminalGB contributors") and in the README's
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

## Build profiles & throughput — the shipping build optimised for SIZE for four years

**Full page, both units, every negative result: [`docs/throughput.md`](docs/throughput.md).**
Instrument [`examples/core_bench.rs`](examples/core_bench.rs), scripts [`tools/perf/`](tools/perf/).
Nothing here changed an emulated byte: all 18 external baselines, the 15 accurate-engine
baselines, `testroms`, both picture engines and all four architectures are byte-identical
across every configuration measured.

- **`[profile.release]` said `opt-level = "s"` from 2022 to 2026 and nobody here ever chose it.**
  Upstream `raphamorim/gameboy` set `z`, then `s`, before this fork or any of the constrained
  targets existed. It is `3` + **fat LTO** now, worth **+24 %** on a real cartridge. **The one
  target that had ever measured the question chose against "s"** — `tools/psp-build.sh` overrides
  it to 3 with a note that "s" costs the PSP 1 ms a frame — and that is most of the argument.
- **`opt-level` is nearly the whole term, so do not reach for an exotic knob first.** `"s"` *with*
  fat LTO *and* `-Ctarget-cpu=native` is +4.5 %, against +24 % for changing the one setting.
- **THREE EXPERIMENTS CAME BACK NEGATIVE — do not repeat them.** (1) **`-Ctarget-cpu=native` buys
  nothing measurable** (0.0895 vs 0.0912, inside the spread), so there is deliberately **no
  `.cargo/config.toml`** — which is a good outcome, since a flag file would have silently made
  released artefacts non-portable. (2) **PGO made it 4.3 % SLOWER** against its fair comparator
  (o3/thin — `-Cprofile-use` and fat LTO **do not combine on the pinned rustc 1.83**:
  `module flag identifiers must be unique … !"CG Profile"`). `tools/perf/pgo.sh` is kept so the
  experiment is one command, and is wired into no build target. (3) **`[profile.dev.package."*"]
  opt-level = 2` does nothing here** — 3.10 s vs 3.07 s on `cargo test --lib` for +1.2 s cold —
  because the debug build's cost is *our own* code, a local crate that stays at `opt-level = 0`
  either way, and no dependency is on the emulation path.
- **`codegen-units = 16` is not measurably slower than 1 and halves the cold build (30.9 s vs
  66.7 s). It stays at 1 anyway, on caution not a number**: after fat LTO the merged module is
  partitioned by an LLVM heuristic, so "the hot loop stayed in one partition" is a property of
  today's source. The measurement is done if anyone wants those two minutes back.
- **`[profile.quick]` is where compile time is bought: a one-line change is 1.7 s against
  `--release`'s 32.9 — nineteen times faster** — and it still runs fast enough to drive a harness.
  It is **not** `release` with the knobs loosened and has its own name on purpose, so a number
  measured under it cannot be quoted as one this project publishes. **It unwinds**, which is the
  second reason to use it: `[profile.release]` aborts, `cargo test` forces test targets to unwind,
  so `cargo test --release` builds this crate's rlib twice and intermittently links the wrong one
  — the whole family of failures in `docs/pitfalls/builds.md`.
- **`[profile.wasm-release]` exists for the one size-sensitive target, and its settings are NOT the
  ones you would guess.** Measured on the artefact `make build-wasm` actually produces:
  `opt-level` moves **nothing at all** between `3` and `"s"` (byte-identical), **LTO is the whole
  term**, and **thin LTO more than doubles the module** — an earlier draft set `lto = "thin"` and
  so produced a *bigger* artefact than the profile it was meant to shrink. It is `opt-level = "z"`
  inheriting fat LTO, 4.4 % under `[profile.release]`. `web/`'s real module goes through
  `wasm-pack`, which is not installed here and was **not** measured; do not assume the proportions
  carry over.

### The PPU renders frames nobody reads — `Gameboy::set_render_pixels`

- **Measured, and it is bigger than every build flag put together: pixels are 50.9 % of an
  overworld frame**, with instruction counts identical either way (2529 vs 2529 — an exact A/B).
  `set_render_pixels(false)` is **1.55x** on the overworld and **1.25x** in a battle, where the CPU
  has more to do so pixels are a smaller share of a larger frame.
- **Skippable, NEVER unavailable.** Pixels are on for every frontend, harness and default build —
  the picture is the product and the screen-only student needs it. This only lets a caller that has
  *declared* it will not read the framebuffer stop paying.
- **The cartridge cannot tell, and that is proved rather than argued.** The framebuffer is emulator
  *output*, reachable from no address. The one non-output thing rendering advances is the window's
  internal line counter (`Gpu::advance_window_line`, called from the skip path and from `draw_bg`,
  which is why it is a function — **the two call sites must not drift**). End to end on a real
  cartridge the **WRAM hash is identical** with pixels on and off, on both workloads.
- **Turn pixels back on at a frame boundary and the next frame is EXACT** — byte-identical to a
  machine that never stopped drawing. That is what action repetition rests on: skip the
  intermediate frames, ask for the one the policy will look at. Toggling *mid*-frame leaves that
  frame half-drawn, so toggle between `frame()` calls, never inside one.
- **The per-dot engine ignores it, deliberately**: under `identical` the fetcher *is* the timing.
  `Gpu::pixels_effective` reports what is actually happening, so a caller cannot believe it saved
  something it is still paying for.
- ~~**The APU needs no equivalent and never will**~~ — **WRONG, and it cost a 14 % saving until it
  was checked.** `Sound::do_cycle` does return immediately with no player installed, but every
  scenario, longplay and swarm run **installs a `SilentPlayer` on purpose**, because a machine with
  no sound chip is one a cartridge can tell apart (Pokémon Blue's Pokémon Center healing sequence
  never finishes without one). So a collector has a full APU and pays for a band-limited resample
  it throws away. `Gameboy::set_render_audio(false)` is the sibling — see the swarm section below.
- **`terminalgb-mcp --no-pixels`** wires it up, and **removes `screenshot` from `tools/list` and
  refuses it if called anyway**. Not politeness: the framebuffer holds whatever was last drawn, so
  answering would return a stale frame *as if it were current*. Same rule as `write_memory`.

### The SWARM is not the emulator — measure the fan-out, not one core (`docs/throughput.md` §8-§10)

**Full record with every table: [`docs/throughput.md`](docs/throughput.md) §8-§10.** Instruments:
`pewter_chain corpus`'s own last line (emulated frames/s aggregate + the emulate/loop split),
[`tools/perf/swarm.sh`](tools/perf/swarm.sh), `core_bench --audio --no-pixels --no-audio-out
--reset-every N`.

**Headline, interleaved A/B against the parent commit, Pokémon Blue link 01, load stated because
another crew held twelve of the twenty cores for most of the session:** twenty workers, 200
attempts, 987,736 emulated frames — **72,900 → 103,900 frames/s on the typical round (1.43x)**,
100,311 → 120,409 on the best. One worker pinned to one core: **CPU time 17.69 → 12.42 s (1.42x,
contention-immune)** and wall 19.35 → 12.45 s; on a quiet box earlier the same pair was 23.0 →
9.7 s (2.37x), because the filesystem half was larger then. Goal counts and frame counts identical
in every run.

- **THE UNIT IS EMULATED FRAMES PER SECOND OF WALL CLOCK ACROSS THE FAN-OUT, not episodes per
  second.** An episode is however long it is, so episodes/s is not comparable between links or
  against anybody else's harness. `corpus` prints both now, plus how much of the attempt clock was
  inside `Gameboy::frame` (`RunResult::emulate_ms`, two clock reads a frame = 0.04 %) — because
  "which half of the loop does the next hour belong to" is otherwise a guess.
- **THE PER-FRAME HARNESS WAS NEVER THE PROBLEM AND EVERYONE ASSUMES IT IS.** Two instruments
  agree: `emulate_ms` says 7.5 s of a 7.5 s run loop is inside the emulator, and an `eu-stack`
  profile puts **every agent-plugin frame together at 0.6 %**. The plugin dispatch, the six-frame
  sample, the referee and the guidance are noise. What was expensive was work nobody consumed.
- **A COLLECTOR MUST NOT WRITE A BATTERY SAVE — 1.87x on its own, and the A/B needs no code
  change.** `pewter_chain::play` staged the cartridge to `<out>/cart.gb` every attempt and built
  the machine with that path, so a 3,000-attempt run wrote 3 GB of identical copies and then
  `fsync`ed a `.sav` (file **and** directory, `Battery::write_atomic`) beside each, from twenty
  workers. **No attempt has ever read a `.sav`** — state arrives and leaves as a save state.
  Measured by pointing `--out` at `/dev/shm`, where `fsync` is a no-op: **23.0 s wall on disk
  against 12.4 s on tmpfs for identical CPU time (12.25 vs 12.33 s)** — the filesystem was 46 % of
  a single-worker run. Trajectory and final-state digests byte-identical either way.
  *(It also removed a coupling: attempt k+1 inherited attempt k's `.sav`.)*
- **`Gameboy::set_render_audio(false)` — the sibling of the pixel skip, and the construction is
  DELIBERATELY NARROW.** Skipped: `blip_buf::add_delta`, `end_frame`, the mixdown, the hand-off.
  **Kept, because the guest can see them**: the frame sequencer, length counters, sweep and
  envelope units, duty and wave positions, the LFSR, `since_fetch`, the `NR52` enable flip-flops,
  and `last_amp`. Every *serialised* field therefore holds the value it would have held, so the
  claim is the strongest available: **a save state taken with samples off is byte-identical to one
  taken with them on**, which is what
  `gameboy::tests::skipping_samples_changes_nothing_the_cartridge_can_see` asserts. `blip` is not
  serialised, which is what makes it possible. Teeth both ways: make the setter a no-op, or make
  `Sound::do_output` return early instead of calling `Sound::run` — the obvious next
  "optimisation", and the wrong one, since that is where the channel state machines advance.
- **Measured on the machine a collector runs (an APU INSTALLED), one worker, Pokémon Blue:**
  12,012 fps with both on, 18,219 with pixels off (**+51.7 %**), **20,695 with both off (1.72x)** —
  and the **WRAM hash is identical across all three**, which is the end-to-end evidence rather
  than a claim.
- **DECLARE IT BEFORE THE PHASE OFFSET, NOT ONLY IN THE RUNNER.** A `corpus` attempt burns
  `phase_for(seed, spread)` frames before the agent is attached — averaging 1,500 against an
  attempt's 3,500, i.e. **a third of everything the program emulates** — and those frames happen
  before `ScenarioRun` exists. Leaving the declaration to `ScenarioRun::collecting()` left that
  third drawing pixels for nobody.
- **`ScenarioRun::collecting()` is opt-in and `filming()` wins over it**, resolved together at the
  top of `play()` so the two commute. **`examples/scenario.rs` and `examples/longplay.rs` are
  deliberately NOT collectors**: both are paced by a live model, so the emulator is not their
  bottleneck, and both write a failure save state a human then looks at.
- **A COLLECTOR'S SAVE STATE CARRIES A STALE PICTURE, and that moves the corpus digests.** A save
  state holds the framebuffer. Measured on link 02 with the same entry pool under both builds:
  every decision list, every frame count and every ending **identical**; `final_state` and
  `start_state` hashes **different on all 40 attempts**. The runs played the same game button for
  button; only the picture bytes inside the state differ. Nothing commits those digests and
  `pewter_chain verify` is green — but say it out loud rather than letting someone find it.
- **SCALING IS SILICON-LIMITED, NOT CONTENTION-LIMITED, and the arithmetic proves it.** This box is
  8 P-cores + 12 E-cores, no SMT. 20 workers give 13.72x, which reads like a 31 % loss and is not
  one: the first eight workers deliver 18,502 steps/s each and workers 9-20 deliver 11,155 each —
  **60 %, the P/E ratio** — and `8 × 18,502 + 12 × 11,155 = 281,876` against a measured 281,879.
  Per-instance cost rises only 26 % from one worker to twenty. **There is no throughput to win in
  contention on this box; there is throughput to win in cores.**
- **About 3.3 MiB per agent, so cores bind two hundred times sooner than memory.** Peak RSS 7 MiB
  at one worker, 56 MiB at sixteen — ~4.5 MiB base plus 3.27 MiB an agent (a megabyte of it the
  cloned cartridge). ~9,000 agents would fit in 30 GiB. **That is why the cartridge is NOT shared**
  between agents: an `Arc<[u8]>` through all six mappers, the save-state codec and the FFI to save
  a megabyte of something nothing is short of.
- **A reset is not a cost: ~0.3-0.5 ms, four to seven frames' worth.** `core_bench --reset-every N`
  measures it in the unit an episode is measured in: +0.14 % at 3500, +0.72 % at 1000, +1.58 % at
  300, and *negative* at 100 and 30 (i.e. under the noise). Against a 3,500-frame episode that is
  0.01 %. **An episode would have to be under about fifty frames before it mattered.**
- **NEGATIVE, with the command: an allocator swap buys nothing.** Tested without adding a
  dependency at all — `LD_PRELOAD=/lib64/libtcmalloc_minimal.so.4` on the real corpus, which is the
  fair test to run *before* proposing a crate. Two interleaved rounds each: glibc 57,916 / 49,761
  frames/s, tcmalloc 52,481 / 56,934 — ranges overlap completely, and the profile predicted it
  (`_int_malloc` + `_int_free_chunk` = 0.3 % of samples).

#### NEVER MEASURE THROUGHPUT WITH `--features profile` ENABLED

`src/profile.rs`'s counters are a single `static mut` **by design** (its own docs say
single-threaded), so at twenty workers every retired instruction writes the same cache line.
Measured: **12,686 aggregate steps/s with the feature on against 246,376 with it off — a factor of
nineteen**, and it reads exactly like a catastrophic scaling collapse in the emulator. Build a
separate binary for `--breakdown` and never quote a throughput number from it.

#### Profiling without `perf`, and the two traps in doing it

There is no `perf` on this box, but **`eu-stack` is installed** and a poor-man's sampler is enough:
build with `cargo build --release --config 'profile.release.debug=1' --config
'profile.release.strip=false'` (no manifest edit), then loop `eu-stack -p <pid> -n 60`. Two traps
that produced wrong answers first: **attribute per THREAD, not per sample** (the parked main thread
otherwise reads as 50 % blocked in `syscall`), and **filter to the thread that is actually inside
the work** (`pewter_chain::play` appears in the worker's stack and nowhere else).

#### A swarm figure is wall-clock by definition, so state the load average and interleave

Measured while doing this work: the identical single-worker command read **10,646 frames/s on a
quiet box and 7,405 after fifteen minutes of twenty-thread runs** — 30 %, all in one direction.
`tools/perf/swarm.sh` therefore measures every point twice, ascending and descending, and prints
both; if the two disagree by more than a few per cent the box was drifting and the sweep is not a
measurement. **Another crew was running a twelve-worker certification on the same twenty cores for
most of this session** (check `readlink /proc/<pid>/cwd` before blaming your own change), which is
exactly the condition that produces it — so every comparison is an **interleaved A/B**, the only
shape that survives steady contention.

### Measuring on this box at all — three traps, each of which produced a wrong answer first

- **SHORT ROUNDS MEASURE THE FREQUENCY GOVERNOR, NOT THE BINARY.** 2,000-frame rounds (~0.3 s)
  reordered the seven variants **between runs, twice** — `o3-fat-cu16` read +24.0 % in one run and
  −2.8 % in the next. At 25,000 frames two independent runs agree to ~1 %. Interleaving and
  min-of-N do not rescue a sample that is mostly process start-up.
- **A wall clock on a shared box measures the box's afternoon.** The same binary read 0.0918
  ms/frame quiet and 0.1336 with eight cores busy elsewhere — a 44 % error, larger than every
  effect being measured. `core_bench` takes the per-instance figure from
  **`/proc/thread-self/schedstat`** (thread CPU nanoseconds, no `libc`, so it still builds on
  `--no-default-features`) and prints the wall figure beside it so the gap is visible. The
  *aggregate* figure stays wall-clock — that one is wall-clock by definition.
  **CPU time does not fix frequency scaling**, which is what the contention actually costs here;
  only a quiet box does. Always report the load average.
- **A fan-out benchmark must start its clock behind a barrier.** Without one the aggregate divides
  measured frames by a wall clock that also contains the warmup and understates itself by 13 % at
  the defaults.

### `RenderMode::IDENTICAL_COST` moved 6.3 → 6.8 → 8.8, and every time it is `standard` that moved

The profile change was worth more to the whole-scanline renderer (a branchy loop an optimiser is
good at) than to the per-dot pipeline (which inlines less); the word-at-a-time monochrome tile blit
below is worth more to it again, and `identical`'s pixels come out of the fetcher so it never sees
either. Twelve interleaved samples give 7.03–10.00 (median 8.5) against 5.57–6.38 for the *same
harness on the parent commit in the same session*, whose published figure was 6.8 — so both ends
are depressed by the same contention and the constant is that median scaled onto the old one.
**The figure is hard-coded in `docs/tools/status_numbers.py`** as well as in `src/gpu/mod.rs` and
five prose pages — `make status` will silently put the old one back if you only edit the constant.

### Round three: the emulator itself (`docs/throughput.md` §12-§13)

Rounds one and two removed work nobody consumed. This one profiled what was left and found it
**flat** — no item above 25 % — which is itself the result to carry forward. Measured pinned to one
P-core, interleaved against the parent commit, best of twelve 15,000-frame rounds, load 19-27:
**collector 0.0722 → 0.0608 ms/frame (+18.8 %), player 0.1208 → 0.0974 (+24.0 %), the renderer
alone 0.0486 → 0.0366 (+32.8 %)**; a second run gives +22.2/+25.1/+30.0. In the swarm's own unit
(link 01, 20 workers, four rounds alternating which binary runs first) the best round is
43,333 → 47,577 frames/s (+9.8 %, median +8.7 %) and the contention-immune single-worker CPU
figure is 12.95 s → 12.06 s (+7.4 %).

- **`Timer::do_cycle`'s loop was three quarters of the timer, and it is closed form now
  (`Timer::batch_mcycles`).** A halted CPU is batched up to 64 M-cycles and Pokémon Blue is halted
  four fifths of its frame, so ~13,300 of ~17,500 `do_mcycle` calls a frame were loop iterations
  adding four to a counter with nothing observing them. `TIMA` counts falling edges of one counter
  bit whose period is ≥ 16 T against a 4 T step, so an edge is exactly a crossing of a multiple of
  the period and the count telescopes to a difference of two divisions; the period divides 65536,
  so the counter's own wrap is one of them and 32-bit arithmetic gets it right. The serial shift
  clock is the same argument at period 512 T taken **one M-cycle ahead**
  (`Timer::observed_counter` — from `div` instead and `boot_sclk_align` fails by one instruction).
  **It declines rather than approximating**: a reload in flight, a `TIMA` overflow anywhere in the
  span, or a counter off the 4 T grid all fall back to the loop, which stays the definition.
  Worth **+17.1 %** of an overworld frame and **+11.1 %** of a cold boot — the difference is how
  much the game halts. Pinned by `the_batch_matches_the_loop_everywhere_it_is_taken`, 133,120
  starting states, with an assertion that the batch is actually *taken* on 100,000 of them so it
  cannot degenerate into checking the loop against itself.
- **`Gpu::blit_dmg_tile` draws a whole monochrome background tile with word arithmetic** — a
  256-entry `BIT_SPREAD` table for the colour indices, a mask-select for the palette, and a
  two-pixels-at-a-time read-modify-write of the framebuffer. **Alpha is preserved, not chosen**:
  `clear_screen` writes 255 into it and `make test-cross` pins a hash of the whole buffer.
  A Color tile, a flip, or the partial tile at either end of a run falls through to the pixel loop,
  unchanged. Checked against that loop over **all 65,536 tile-data pairs**.
- **THREE EARLIER RESTRUCTURINGS OF THE PIXEL LOOP ALL MEASURED WORSE — do not repeat them.**
  Slicing the outputs to the run once (which removes four bounds checks a pixel), a per-tile outer
  loop with `split_at_mut` and a const-generic console, and a flat `chunks_exact_mut(4)` zip all
  came in at 0.0470-0.0476 ms/frame against 0.0445 unchanged. LLVM had already hoisted what they
  were aimed at. **The word blit works because it removes stores, not branches.**
- **Devirtualising the mapper is a negative result, in both shapes.** `Box<dyn
  MemoryBankController>` costs an indirect call on every opcode fetch; a closed `enum Cartridge`
  delegating to the same trait measured **4 % slower** and `Box<Cartridge>` (which devirtualises
  *and* keeps the MMU struct small) worse still, both interleaved and both losing twice. The
  predictor sees one answer for a whole session either way, and inlining a mapper into `rb` grows
  the emulator's hottest function.
- **The APU's per-M-cycle call is not the APU's cost — 1.5 %, sized before attempting it.**
  Moving `Sound::do_cycle(4)` out of `bus_tick` to the end of the instruction (the upper bound on
  any lazy-accumulation scheme) gives 0.0409 → 0.0403. The `no apu` row's **21.6 %** is almost all
  `Sound::run`: the channel synthesis loops, which keep advancing duty positions, wave positions
  and the LFSR with samples switched off because those *are* guest-visible. **That is the largest
  named target left in a collector's frame**, and it is the most calibrated code in the repository.
- **THE SCALING CURVE COULD NOT BE RE-MEASURED THIS SESSION, AND THE SWEEP SAYS SO ITSELF.**
  With twelve to sixteen of the twenty cores belonging to another crew all day, `tools/perf/
  swarm.sh` reports 1 → 20 workers as only **5.94x** and **saturates at twelve** (5 % more from
  twelve to twenty, where §9's quiet-box sweep gained 46 %) — that is what "about five cores are
  free" looks like, not a scaling regression. Its ascending and descending passes agree to 2 %
  from four workers up and disagree by **25 % at one worker**, which is exactly the condition the
  two-direction sweep exists to expose. What *is* measurable under contention is the interleaved
  before/after pair at each width, and it answers the question this round raises: the per-instance
  figure is flat (0.1006 → 0.0846 at one worker, 0.1031 → 0.0890 at twenty), so **the gain survives
  fan-out and the change adds no sharing**.
- **`core_bench --breakdown` takes the run's own configuration as its baseline now** — so
  `--breakdown --audio --no-pixels --no-audio-out` breaks down the machine a *collector* runs
  rather than a bare `Gameboy` — and gained `no timer batch`, `no apu` and `no oam dma` rows plus a
  line of idle-batch counters. **Those counters answer a question worth knowing: every idle batch
  is bounded by the PPU's next mode edge, never by the timer and never by `IDLE_BATCH_MAX`**, so
  raising the cap buys nothing.
- **Nothing the cartridge can see moved, and here it is provable in a way round two's was not.**
  A 40-attempt `pewter_chain corpus` run under both builds is identical in **every** recorded field
  except `wall_ms` — decisions, frame counts, endings, and both the `start_state` and `final_state`
  digests. Round two's pixel skip moved those digests because it was a declaration; these two are
  equivalences.

#### PIN A SINGLE-WORKER COMPARISON TO ONE CORE, OR IT MEASURES CORE PLACEMENT

This box is 8 P-cores at 5.3 GHz and 12 E-cores at 4.6 GHz, and a fresh process lands on whichever
the scheduler picks — so the per-instance figure moves by 15 % or more between invocations of the
**identical binary**, *even though it is CPU time*. Unpinned, the boxed-enum mapper experiment above
reported **35 % faster**; pinned with `taskset -c 4`, in the same session, it reported 4 % slower and
reproduced that twice. `/sys/devices/system/cpu/cpu*/cpufreq/cpuinfo_max_freq` identifies the cores
(0-7 are 5.3 GHz here, 12-13 are 6.5 GHz preferred cores, the rest 4.6). Everything in
`docs/throughput.md` §12 is pinned.

#### A between-binary difference under 2 % is code layout, not your change

With fat LTO, a change in one function moves everything after it. Measured repeatedly here: two
binaries that differ only in `draw_map_run` reported **pixels-off** figures 2 % apart, on a path
that never calls it. So an isolated component's cost has to be measured as a *difference within
each binary* (`baseline − --no-pixels` is exact, because `--no-pixels` has identical instruction
counts) and a sub-2 % whole-frame difference between two binaries is not evidence of anything.

#### `pkill -f` AND `pgrep -f` MATCH YOUR OWN SHELL — this cost two runs in one session

Already recorded further up for `pgrep`; it bit twice more here, and `pkill -f 'no-pixels'` and
`pkill -f quiet_runs.sh` each killed the command that ran them, losing the run in progress. Kill by
the PID you captured at launch.

#### `docs/components/ppu.md`'s line references were ALREADY POINTING AT THE WRONG CODE

Not caused by this round, and worth knowing because `make docs-check` is green either way. Checked
on the parent commit: `mod.rs:570`, documented as `renderscan`, was a palette comment;
`mod.rs:702`, documented as `draw_sprites`, was inside `load_state`. The whole of §4.1 had drifted
by roughly a thousand lines, because `make docs-links` rewrites a reference's *URL* from its
`path:line` text and never re-anchors the text by content — which is exactly the failure this file
already warns about, observed again. §4.1's references are re-anchored here (verified line by
line); **the rest of that page has not been checked and should be assumed stale.** The general
rule stands and is worth repeating: after editing source that moves line numbers, `make docs-links`
followed by `make docs-check` proves only that the numbers are *in range*.

#### The dual-rlib artefact fires between test targets — build them in one invocation

`cargo test --release --test a` followed by `--test b` reliably produces the panic-strategy wall
`docs/pitfalls/builds.md` catalogues, because the first rebuilds the abort rlib. `touch src/lib.rs`
clears it, but the reliable shape is **one invocation listing every test target**
(`cargo test --release --test savestate --test audiocapture --test ppuengines …`), which builds the
rlib once.

### `make no-plugins` was RED on main before any of this, and the cause is a documented trap

`fit_bar` in `src/screen/tui.rs` was used unconditionally and defined under
`#[cfg(feature = "plugin-trainer")]` — because a function was inserted between an orphaned doc
comment and its attribute, so the `#[cfg]` landed on the wrong item. Exactly the hazard this file
already records for `load_session`'s `#[cfg_attr]`. Fixed by deleting the orphan. **When you insert
an item in `tui.rs`, check what attribute is immediately above it.**

## Portability: the core RUNS on other architectures, and on a PSP (`docs/portability.md`)

**Full measured report, with the honest gaps: [`docs/portability.md`](docs/portability.md).**
Read it before quoting any cross-target claim; the README's old "type-check passes, not
linked-and-run" sentence is superseded and was rewritten.

- **`make test-cross` builds and RUNS the embedding core on aarch64, armv7 and i686** and
  requires a byte-identical frame hash *and* save-state length against the host. In CI
  (`embed` job). The linker is rustup's own **`rust-lld`** and the targets are all **musl**
  (rustup ships their `libc.a`), so **no C cross-toolchain is installed for any of it**;
  `qemu-<arch>-static` is the only extra and a missing one SKIPS loudly. Teeth proven with a
  32-bit-only bug the host build accepts.
- **`state bytes` is 142090 on 32-bit and 64-bit alike** — that is `savestate::Writer::usize`
  storing a `u32`, observable. A save state written on a phone loads on a desktop.
- **A TYPE CHECK CANNOT FIND A MISSING ATOMIC.** `sound::host_status` counted errors in an
  `AtomicU64`; **MIPS32 has no 64-bit atomics and `std::sync::atomic::AtomicU64` does not
  exist there**, so the *whole emulator* failed to compile for `mipsel-sony-psp` on that one
  line. armv7 and i686 are 32-bit and both build it fine (`ldrexd`, `cmpxchg8b`), so nothing
  short of building for the real target catches it. It is `AtomicU32` now, widened on read so
  the public `Snapshot` is unchanged. **Do not put a 64-bit atomic in the core.**
- **`make test-cross` SKIPs a target when its `qemu-<arch>-static` is missing, and a skip is
  not a pass.** This box has no `qemu-user-static` package, but the rootless route in
  `tools/run-cross.sh`'s own header works and takes a minute:
  `dnf download qemu-user-static-aarch64 qemu-user-static-arm`, `rpm2cpio *.rpm | cpio -idmu`,
  then `GB_QEMU_DIR=<dir>/usr/bin make test-cross`. With that, all four architectures really do
  run and really do produce the identical frame hash.
- **`riscv64gc-unknown-linux-musl` is deliberately out of `run-cross.sh`**: this package
  declares `crate-type = ["cdylib", ...]` and a musl target that *supports* `cdylib` then
  fails to link one against a shared libc rustup does not ship. The other three escape it
  only because rustc drops `cdylib` for them. A packaging quirk, not a RISC-V finding.

### The PSP — it plays Pokémon, and what that cost

- **`./tools/psp-build.sh` → `target/psp/EBOOT.PBP`; the frontend is `platform/psp/`** (~350
  lines, public embedding API only). **NOT a workspace member and must never become one** —
  nightly, `-Z build-std` and a third-party std overlay may not reach `make embed`,
  `cargo test` or `cargo install`. `make embed` still reports one third-party dependency.
- **Measured under PPSSPP 1.20.4 at 333 MHz, `standard` picture mode: Pokémon Blue runs the
  overworld at the DMG's own 59.7275 fps, work 9.4-9.9 ms of a 16.743 ms budget, 0 of 120
  frames over budget in every window** (it was 42.0 fps / 23.8 ms before this work). **A
  battle text box is 50.0 fps / 19.6 ms and that is the wall** — see the next section.
  **Nobody has run it on real hardware** and PPSSPP does not model the PSP's caches, so
  hardware is more likely slower than faster.
- **PPSSPP's `sceKernelGetSystemTimeWide` is CYCLE-derived, not wall clock** — 222 → 333 MHz
  changed the cost by exactly 1.5000x. That is how you know the host is not the limiter and
  the figure is a cycle count over a clock.
- **`opt-level = 3` is MEASURED for this target and the repo's `"s"` is wrong here** (23.4 vs
  22.4 ms). Fat LTO + one codegen unit is worth ~14 %. `panic = "abort"` is inside the noise.
  `scePowerSetClockFrequency(333, 333, 166)` is worth exactly 1.5x and is one call.
- **The frame hash is identical on every frame AND identical to x86-64, aarch64, armv7 and
  i686.** That is the proof the port is exact rather than close: a Game Boy emulator has a
  hundred ways to be nearly right and every one of them moves the frame.
- **`mipsel-sony-psp` is Tier 3 and `no_std` ONLY upstream.** Stock nightly fails *inside
  `std`* (14 errors), never reaching our code. `std` comes from a sysroot overlay
  (AndrewAltimit/rust-psp, MIT) rebuilt with `-Z build-std`, and **it rots**: it does not
  compile on nightly-2026-08-14 and does on `nightly-2026-04-14`, which its own CI pins.
  Both the nightly and the overlay commit are pinned in `tools/psp-build.sh`. **The PSP is a
  toolchain you maintain, not a target you add.**
- **`#![feature(restricted_std)]` is needed only in the BINARY crate**, never in the library
  — the emulator compiles unmodified.
- **PPSSPP's default keyboard map is a trap that reads as a broken build**: keyboard `X` is
  **Circle** and Cross is on **`Z`** (Square `A`, Triangle `S`, Start Space, Select Enter,
  d-pad arrows). Name the button glyph in any prompt, and accept several buttons in a menu.
- **`sceCtrlSetSamplingCycle(0)` + `sceCtrlSetSamplingMode(Digital)` before reading the pad**,
  or the pad read returns buffer garbage — seen as a ROM picker scrolling by itself.
- **PEEK THE PAD IN A GAME LOOP, NEVER READ IT.** `sceCtrlReadBufferPositive` *blocks* until
  the pad is sampled again, which with sampling cycle 0 is once per VBlank — so a loop that
  finishes its frame early spends the rest of the display's 16.7 ms inside it, and charges
  that wait to whatever it is timing. Invisible while a frame cost 23.9 ms (the sample was
  always already there); the moment the core got fast it reported **15.2 ms for 8.3 ms of
  work** and locked the machine to the panel's 59.94 Hz instead of the DMG's 59.7275.
  `sceCtrlPeekBufferPositive` is the call a self-paced loop wants.
- **XTEST does not reach the PPSSPP flatpak here**; kernel-level injection does
  (`ydotoold` on a SHORT socket path — unix paths cap ~108 chars — with `/dev/uinput` group
  `input`). **Focus the target window first**: a kernel key goes to whatever has focus,
  including the captain's terminal.
- **Do not measure a picture's extent by trimming black.** Two hours went into a phantom
  "the quad renders 30 % narrow" because the Pokémon Center's right-hand columns are dark
  and `magick -trim` cut them. Measure on a frame that is light at the edges (a battle
  screen) before concluding anything about geometry.
- **The PSP's fps came from a save state, not from mashing buttons.** `<rom>.gbss` beside
  the cartridge is auto-loaded at boot. The two that this campaign's numbers are quoted on:
  `shiny_hunt --rom <blue.gb> --make-state route1.gbss` (a new game walked to Route 1 grass
  — the overworld) and `gen1_battle --rom <blue.gb> --preset charizard-vs-pidgey
  --save-state battle.gbss --save-at 900` ("Wild PIDGEY appeared!"). Neither needs a save
  file, which matters because **there is no real Pokémon Blue battery save on this box** —
  every `.sav` under `~/roms` is essentially blank (~700 non-zero bytes of 32,768), so a
  script that expects CONTINUE starts a new game and lands on the title screen instead.
  Check `sum(1 for b in sav if b)` before believing a save.
- **A save state does not survive a `savestate::FORMAT_VERSION` bump, and the PSP frontend
  fails soft** — it prints `state ignored` and boots the cartridge cold, which looks like a
  working benchmark measuring the title screen. If a workload's `instr/f` suddenly collapses
  and `halt/f` shoots up, the state stopped loading. Regenerate it.

### Making the PSP fast: the instrument, and what it found

**Full write-up with every number: [`docs/portability.md`](docs/portability.md) §3.7-3.8.**
Emulation went 22.4 → 8.3 ms/frame on the overworld, every frame byte-identical.

- **BUILD THE INSTRUMENT BEFORE OPTIMISING ANYTHING.** `tools/psp-build.sh --profile`
  replaces the interactive loop with a fixed-length benchmark (`platform/psp/src/main.rs::
  bench`) that times the same frames under several configurations and prints the split.
  `crate::profile` (cargo feature `profile`, off by default, **no dependency**, in no
  shipping build) supplies free per-frame counters and switches that remove one component's
  work; a component's cost is `t(normal) - t(without it)`.
- **Per-call timing is NOT available on a PSP.** `sceKernelGetSystemTimeWide` costs
  **0.785 µs a call** (measured: 100k reads in 78.5 ms) and quantises to 1 µs — smaller than
  the thing being measured, ~17,000 times a frame. The Allegrex **CP0 counter (`mfc0 $9`) does
  not advance usefully under PPSSPP** — two consecutive reads return the same value; do not
  spend time on it again. Whole *frames* are ~10,000 µs, where both problems vanish, so every
  row is a whole-frame time.
- **Print the instruction count in every row. An A/B whose instruction counts differ is not
  an A/B** — that is what says a switch which *is* guest-visible (`no_bus_tick`) still
  measured a comparable run, and it is what showed a `no_pixels` row is exact.
- **THE `--profile` BENCH IS LOAD-INDEPENDENT AND THE INTERACTIVE LINE IS NOT.** Two bench
  runs on the same build and state gave identical means to the hundredth of a millisecond
  with the box's load average moving between them: it is a cycle count. The in-game
  statistics line measures a loop making PSP syscalls whose emulated cost PPSSPP does not
  derive purely from cycles, and those windows wander by milliseconds under load. **Quote
  the bench for component costs, the interactive line for sustained fps, and the load average
  either way.**
- **THE FINDING: Pokémon Blue's CPU is halted for 82 % of its `exec` calls** (13,260 of
  17,556 M-cycles a frame in the overworld), and every idle M-cycle paid the whole
  per-instruction pipeline. `MemoryManagementUnit::idle_mcycles` batches them to the next
  point a peripheral could raise an interrupt — the PPU's next mode edge, the timer's next
  `TIMA` overflow — and stops **on** that edge, so the CPU looks at `IE & IF` on the same
  absolute M-cycle. 13,260 idle M-cycles now arrive in **349 batches**, worth **10.61 ms a
  frame**, and the bench's `no idle batch` row reports the *same frame hash*.
- **`Serial::transfer_in_flight`, not `SC` bit 7.** A Game Boy listening for a partner leaves
  `SC = $80` set indefinitely (Blue does it in its overworld loop) and that console is idle:
  with no internal clock `bits_left` is zero and nothing can happen until an external clock
  arrives, which every cable driver delivers between `do_cycle` calls. Testing the control
  bit refused **2,836,544 of 2,836,544** batches — the optimisation looked completely inert
  and was one predicate away from working. *Count your refusals before believing a switch is
  broken.*
- **The whole-scanline renderer fetched once per PIXEL.** `Gpu::draw_map_run` fetches once
  per *tile* (four VRAM reads per eight pixels instead of thirty-two), splits the line into
  at most two runs at the window's left edge instead of testing per pixel, addresses the
  framebuffer through a row slice (one bounds check instead of three per pixel), reads OAM
  from `voam` instead of through `Gpu::rb`'s address dispatch (11,520 dispatches a frame),
  and skips the white pre-clear whenever `draw_bg` will cover every pixel anyway. **Worth
  3.5x on x86-64 too** — 0.234 → 0.067 ms/frame — which is why `RenderMode::IDENTICAL_COST`
  moved 2.1 → 6.3 (re-measure it; both halves of a ratio can move). **`identical` did not get slower; `standard` got faster.** Say that
  wherever the figure is quoted.
- **`opt-level = 3`, fat LTO, one codegen unit and 333 MHz are all still measured and still
  correct** — they were spent before this work and none of it changes them.

- **The cheapest console left is the PS VITA, not the PSP** — `armv7-sony-vita-newlibeabihf`
  is Tier 3 **with full std upstream**, so there is no overlay to maintain and no nightly to
  pin. 3DS next (`ctru-rs`, std is `?`); Switch is `no_std` like the PSP.
- **Big-endian has never been run** (GameCube/Wii/Wii U are BE PowerPC). Audited only: the
  savestate codec is explicitly little-endian, there is no `transmute` or reinterpreting cast
  in the core, and `src/upscale/`'s `*_ne_bytes` use is a symmetric pack/unpack that never
  crosses a boundary. That is a prediction, not a measurement.

- Add durable project-specific notes here as they are discovered through real work.

## Web audio, and the APU that was missing on wasm (`src/sound.rs` `mod web_audio`, `src/screen/{webaudio,pace,web}.rs`)

**Full page: [`docs/web-audio.md`](docs/web-audio.md); conformance record in
`docs/conformance-notes.md` (2026-08-15).** Read the page before changing any constant here
— every one of them was measured in a real browser and the first value tried was wrong.

- **THE WEB BUILD HAD NO SOUND CHIP, NOT A QUIET ONE.** `MemoryManagementUnit::sound` and the
  whole `sound` module were `#[cfg(not(target_arch = "wasm32"))]`, so `$FF10-$FF3F` read back
  `$FF`, `$FF76`/`$FF77` read `$00` and every write was dropped. A cartridge can tell: `NR52`
  never reports itself powered on, so a sound-driver init loop spins for ever and the game
  looks *hung*, not silent. Same argument `sound::SilentPlayer` already made for native
  builds, never applied to wasm.
- **The cause was a stale dependency pin, and it is now a one-line trap to reintroduce.**
  `blip_buf` **0.1.4** binds the C library (`blip_buf-sys` + `libc` + a `cc` build script) and
  cannot cross-compile to `wasm32-unknown-unknown`; **0.1.5 is a pure-Rust rewrite with no
  dependencies and no build script**, and the lockfile had resolved to it long before. The
  requirement is `"0.1.5"` in the **shared** `[dependencies]` block with the reason written at
  the declaration — do not move it back under `cfg(not(wasm32))` and do not relax the version.
  `make embed` still shows exactly `blip_buf` + first-party `terminalgb-plugin-api`.
- **`requestAnimationFrame` IS NOT A CLOCK.** The old loop stepped one emulated frame per
  callback, i.e. at the *display's* refresh: +0.46 % on 60 Hz and **double speed on 120 Hz**.
  `screen::pace::FramePacer` is the deadline accumulator (same rule as the TUI: advance by
  exact periods, never reset to "now"), capped at `MAX_CATCHUP = 8` frames so a backgrounded
  tab is resynchronised rather than replayed. **`std::time::Instant` panics on
  `wasm32-unknown-unknown`** — the clock is passed in, which is also what lets the loop pace
  on `AudioContext.currentTime` and share ONE clock with the audio consumer (zero structural
  drift).
- **No JS object may go near the APU.** `AudioPlayer: Send` (the FFI build parks a `Gameboy`
  in a `static Mutex`) and `web_sys::AudioContext` is not `Send`. So `WebAudioPlayer` holds
  only a `SampleQueue`, `Scheduler` holds only arithmetic (both in `src/sound.rs`, both
  compiled under `cfg(test)` on the host so they have real tests), and the ~20 lines that
  touch `AudioContext` live in `src/screen/webaudio.rs`.
- **There is no usable callback thread in a browser.** `AudioWorklet` + `SharedArrayBuffer`
  needs COOP/COEP headers a static GitHub Pages deployment cannot set; `ScriptProcessorNode`
  is deprecated *and* runs on the main thread. So blocks are **scheduled**:
  `AudioBufferSourceNode.start(when)` with a cursor kept a fixed distance ahead of
  `currentTime`. Two rules, both tested: **never schedule in the past** (a late `start()`
  plays *immediately* and overlaps — a click, not the gap it really is) and **never pad a
  block with substituted samples** (the cursor is the output clock; padding slides it for
  ever). `SampleQueue::drain_block` exists for the second — all of `n` or none of it.
- **Three constants, all measured, all wrong the first time:**
  - `TARGET_LEAD = 4096` (93 ms @44.1k). At 2048 — the desktop cushion's size — Chrome
    measured **~2 underruns per second** at a perfect 59.6 fps. The desktop analogy fails
    because there a cpal callback thread keeps pulling while the frame loop stalls; in a
    browser one rAF callback produces *and* schedules, so the cushion must cover the whole
    stall. The ceiling is `pace::MAX_CATCHUP` (134 ms) — beyond that the audio is never
    produced and no lead can hide it.
  - **The queue is `TARGET_LEAD` / `4 × TARGET_LEAD`, NOT `sample_queue::start_fill()` /
    `high_water()`.** Those size a queue a device callback drains; here the scheduler owns
    the latency and the queue is only the transfer buffer, so clipping at the desktop's 2816
    destroys the backlog the scheduler needs after a stall (**~2,560 samples/s discarded on a
    healthy run**) — and with the lead above the ceiling the two *deadlock* and nothing ever
    plays. `the_queue_can_hold_a_whole_scheduling_lead` pins it.
  - **Prime once, NEVER re-prime after an underrun.** A cushion is rebuilt out of a backlog,
    and after a stall the backlog is exactly what the pacer's catch-up supplies. Waiting for
    a full lead instead adds silence, lets the queue overflow, and turns one stall into a
    self-sustaining ~2 underruns + ~2,560 dropped samples per second.
- **Measured, in Chrome, tapping the destination so the numbers are what was *played*:** 311 s,
  **0 underruns, 0 drops**, 59.6 fps, lead stable 85-96 ms, one 96 ms priming silence at start
  and none after, pitch 439.7 Hz against the ROM's programmed 439.84 (the estimator is diluted
  by that startup silence — 439.84 × (311−0.096)/311 = 439.70, exact). dmg-acid2 in the same
  browser returns the four DMG shades with histogram `3749/188/6254/12849`, **identical to the
  hardware reference PNG**.
- **NOTHING IN CI CAN SEE ANY OF THIS AND THAT IS A STANDING GAP.** Every harness, scout and
  unit test runs on the host, where the APU was always present; `make check-wasm` type-checks
  and a type-check cannot see a missing peripheral. There is no `wasm32` test runner here.
  Reproduce by hand: build the lib for wasm, run `wasm-bindgen --target web`, serve a page that
  imports `{ render, status }`. `status()` is exported for exactly this and the shipped page
  shows it above the picture — a browser silently refuses to start audio until a user gesture
  ([autoplay policy](https://developer.chrome.com/blog/autoplay/#webaudio)), and "silent" must
  never look like "broken".
- **A matching `wasm-bindgen` CLI can be fetched prebuilt** from the rustwasm releases
  (`wasm-bindgen-<version>-x86_64-unknown-linux-musl.tar.gz`); the version must equal the
  `wasm-bindgen` crate version in `Cargo.lock` exactly. No `wasm-pack` needed to test.
- **Save-state format did NOT change**, and that is the point of the skippable APU block: a
  state from a machine with a sound chip now loads into one without and vice versa, with no
  `FORMAT_VERSION` bump.
- **The mapper roadmap item was already fixed and only the README was stale.** `mbc::get_mbc`
  takes the save path *only* to decide whether to write a `.sav`; the mapper is header byte
  `$0147`. What was added is the end-to-end proof —
  `lib::ffi_tests::the_ffi_entry_point_takes_the_mapper_from_the_cartridge_header` drives the
  real `extern "C" load()` symbol with an MBC1 cartridge that bank-switches, because "the core
  is right" and "the entry point is right" are different claims. The web path uses the
  identical `Gameboy::new(rom, None)`.

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

### The brand assets are generated, hand-authored, and font-free (`docs/brand/`)

- **THE final mark is `docs/brand/terminalgb-logo.svg` + `terminalgb-icon.svg` — captain's
  choice, no options remain open.** Composition matched to the reference he picked: heavy
  wordmark, a solid DMG-green **block cursor replacing the I** of TERMINAL, a green **full
  stop** closing the word, grey wide-tracked tagline. The three exploration options are
  retired into `docs/brand/exploration/` and must not be shipped anywhere; any other repo
  (the portal included) should copy the two root files, per
  [`docs/brand/README.md`](docs/brand/README.md).
- **The cursor is SOLID on purpose — do not "improve" it back to inverse video.** A
  knocked-out I inside the green cell was tried at several cell ratios and always read as a
  zero or an empty box (the original option A's recorded risk, confirmed by rendering);
  a solid block is also what a terminal actually shows, the glyph hidden beneath.
- Everything is emitted by `docs/brand/generate.py` — **edit the generator, never the SVGs
  by hand**, and re-render the `preview/` PNGs with `magick` in the same commit. The
  letterforms are original stroked skeleton paths drawn in that script: **no font is
  embedded, subset or traced**, so there is no font licence to audit, and nothing borrows a
  Nintendo motif — the block cursor and a full stop are the whole vocabulary. Every SVG
  paints its own dark panel + faint border, so the theme-in-`<img>` trap recorded above
  cannot bite; check any new lockup on white, black **and** at 16 px before committing — a
  mark that fails as a favicon has failed.

### The README's first code block is a PROMISE, and nothing in CI checks it

`README.md` opens with a **`## Run it`** block — `git clone …` then
`cargo run --release --example terminal -- third_party/dmg-acid2/dmg-acid2.gb` — placed
immediately after the tagline, above every screenshot, because the whole of it had previously
sat at line 68 of a 2,003-line file behind five images and two prose sections. It is the first
thing a stranger reads and **it is the one claim on the page no gate can verify**:
`docs-check` checks links and scores, not that a command works.

- **Re-verify it from a GENUINELY CLEAN CLONE whenever the ROM path, the default feature set or
  the toolchain pin moves** — `git clone <this repo> /tmp/x && cd /tmp/x && cargo build --release
  --example terminal`, with the ALSA shim variables (`PKG_CONFIG_PATH`/`LIBRARY_PATH`/
  `LD_LIBRARY_PATH`) **unset**, or you are measuring this box's shims and not a stranger's
  machine. Verified 2026-08-16: 34 s build, no system libraries, `tests/` submodule empty and
  irrelevant, `rom: "DMG-ACID2"`, `speed_pct` 99.94.
- **A TUI cannot be smoke-tested by piping it to a file** — it refuses only when *neither*
  stdin nor stdout is a tty, and otherwise draws nothing useful into a pipe. **There is no
  `script(1)` on this box**; a ~25-line `pty.fork` harness plus `GB_TELEMETRY=<path>` is the
  way, and the telemetry line is the evidence (`rom`, `emulated_fps`, `speed_pct`) rather than
  bytes on the wire. Point `XDG_CONFIG_HOME`/`XDG_STATE_HOME` at a scratch dir so the check
  cannot rewrite the captain's recents or volume.
- **Three headings now contain the word "run"** — `## Run it` (the two-line answer, top),
  `## Run it in one click` (Codespaces, the MCP server, the headless agent) and
  `## Quick start` (`cargo install`, the desktop window, sound). That is deliberate: the top
  block is a **doorway** that links down to both and duplicates neither. If you add a fourth,
  collapse one.
- The Codespaces badge is repeated in the top block, and the argument is that it is the only
  genuinely **zero-install** path: a reader without a Rust toolchain is otherwise told to
  install one before seeing anything. The detail stays in its own section below.

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

### The captain edits `README.md` directly on GitHub — expect `origin/main` to diverge

Two commits (`7855b0f`, `c746217`) were made in the GitHub web editor while 74 commits of local
work were unpushed, so both sides had touched the same file. Things worth knowing next time:

- **Reconcile with a MERGE, not a rebase.** At any given moment ~9 crew branches in the
  treehouse pool are based on local `main`'s commits; rebasing `main` rewrites every one of
  their bases. A merge also keeps the branch a fast-forward from local `main`, which is what
  firstmate needs to land it, and needs no force-push in either direction.
- **A web-editor image is a hot-link, not a repo file.** GitHub's editor uploads to
  `https://github.com/user-attachments/assets/<uuid>` and inserts
  `<img width= height= alt="image" src=…>`. Nothing lands in the tree, so `docs-check` and a
  broken-image sweep of relative paths will both pass while saying nothing about it — fetch the
  URL and *look* at it. `curl` it and read the PNG `tEXt` chunks: these carry
  `Software: gnome-screenshot` and a creation time, which is how the provenance was settled.
- **The provenance question is not the same one as for `resources/`.** These are the captain's
  own captures of *our* frontend, so the [licence
  boundary](#licence-boundary-format-knowledge-is-ours-game-assets-are-not) rule applies as
  written: our interface doing something, with the game incidental, is fine — the same test that
  lets `docs/link-trade/` commit its screenshots. That is a different finding from the inherited
  third-party figures above, and the two must not be conflated.
- **A commercial-game screenshot at the top invalidates the README's own caption.** The caption
  under the acid2 hero claimed *"using only test ROMs we are free to show"*; it now excepts the
  trainer picture explicitly. If a picture is added, re-read what the surrounding prose promises.
- **A superseded upload can leak a path.** The first attempt (`a19c215e…`, replaced an hour
  later) is a mis-cropped capture whose status bar reads `/home/aaron/Videos/gameboy/…`. It is
  not referenced any more, but it is still live on GitHub's CDN — worth remembering next to the
  personal-identifier scrub.

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
- **The focused Pokémon is a SECOND terminal image and the WHOLE PARTY is a THIRD.** Ids
  `0x00676202` and `0x00676203` against the game's `0x00676201`; each has its own cell
  rectangle and its cells marked `skip`. All three rules from `docs/terminal-render.md` apply
  to both. They are drawn **after** `render_display`, because `render_display` clears
  `pending_wire` for the frame. (`0x00676203` used to be the *opposing trainer's* roster
  strip; the redesign spent it on the player's own six and the roster went back to the text
  form it already fell back to on most window sizes.)
- **THE PICTURES COST COLUMNS, NEVER ROWS, and that is the whole reason six of them are
  affordable.** A sprite lives in a **gutter** down the left of the block it belongs to and
  spans exactly the rows the text beside it already needed, so the panel with six party
  pictures is exactly as tall as the panel with none. The one it replaces took eleven rows of
  a twenty-four-row panel to itself. `pictures_cost_columns_and_never_rows` pins it.
  Vertically the only sacrifice is text: the party's *second* line goes first (and the gutter
  with it — a one-row sprite is a smudge), and a slot is never dropped.
- **The party column is ONE composed image of six cells**, keyed on species + fainted +
  palette + the cell's pixel size, so it is rebuilt on a switch or a faint and never per
  frame. Measured: **37.4 µs against 19.2 µs** for the single large sprite it replaces — 1.95x
  for 2.45x the pixels, on a path that is not the frame path
  (`party_column_costs_no_more_than_the_sprite_it_replaces` prints both).
- **A picture is composed at the size the terminal will DRAW it** (`gutter_px`, `blit_cell`),
  nearest-neighbour, sampled at pixel centres — the `screen::halfblocks` / `video::fit_maps`
  rule. Hand the terminal a 56x56 picture for a 32x32 hole and *it* resamples with whatever
  filter it likes, which is exactly the softness `08902d5` removed one layer further out.
  Verified categorically on a real screenshot: the party gutter contains **five** colours, the
  four Game Boy shades plus the terminal's own background.
- **ONE focus drives the picture and the numbers** (`trainer_view::Focus`, `focus_ring`,
  `focus_at`). The sprite used to switch to the opponent on its own while the stat block kept
  rendering the player's party, so during a battle the panel showed one Pokémon's face over
  another's statistics — and the opponent's own stats were unreachable, which is most of what a
  tracker is for. `Tab` walks a ring that **starts at the opponent in a battle**;
  `App::trainer_sel` indexes the ring, not the party, and snaps to 0 on a battle transition
  (`App::trainer_fighting`) because the ring changes length underneath a stale index.
- **A PANEL PICTURE MUST HAVE A HALF-BLOCK ARM OR IT DOES NOT EXIST IN THE CAPTAIN'S
  TERMINAL.** `new_term_image()` is `None` for anything but Kitty and iTerm2 and
  `place_panel_image` returns on a missing image, so the trainer sprite was **invisible in
  every VTE-family terminal** — reserving eleven blank rows and saying nothing — exactly as
  the music cover was until `f99a033`. Two features have shipped with it; a third must not.
  `screen::halfblocks::render` is a free function over `(rgba, w, h, rect, buf)` and needs no
  state.
- **A CELL IS A UNIT OF RESOLUTION *PER PROTOCOL*, and sizing a picture in cells without
  asking which is how six smudges get shipped.** An image cell is 8x16 device pixels; a
  half-block cell is **1x2 samples**. `trainer_view::Pictures` owns the difference: half-blocks
  get the focused picture at **six** rows rather than three (twelve samples — legible,
  screenshotted) and **no party pictures**, because a party slot is two rows and a Pokémon at
  four samples was rendered, looked at, and is noise. The panel says
  `party pictures need an image protocol` rather than drawing them anyway.
- **`screen::halfblocks` HONOURS ALPHA NOW, and had to.** It ignored it, which is harmless for
  its two previous callers (the game picture and the music sleeve, both opaque) and puts an
  **opaque black slab** behind every Pokémon here, whose background is flood-filled
  transparent. A fully transparent sample is `Color::Reset`; `▄` (`LOWER_HALF`) joined `▀`
  because a transparent *top* half cannot be expressed with `▀` alone. A **partly**
  transparent sample is drawn at full strength — the terminal's background is not knowable, so
  blending towards it is a guess, and a fainted party member's state is carried by the hollow
  marker, the empty bar and `FNT`. Note the existing colour guard passes either way: black
  **is** in the framebuffer, as shade 0 with alpha 0.
- **The palette and the layout primitives live in `src/screen/panel.rs`, once**, so the Gen 1
  battle panel that `crates/terminalgb-gen1-battle` is waiting for reads the same decisions.
  Five families and nothing outside them (structure, type, HP, status, stat stage), and
  **every one is redundant with something a reader can also read** — a raised stat is green
  *and* carries a `+`. Types use the 256-colour cube with a named-ANSI fallback whose
  collisions are **declared** and chosen so no pair that occurs on one Pokémon shares a hue;
  `no_dual_type_shares_a_colour_at_either_depth` enumerates them. **Ground and Rock are
  further apart here than in the games** on purpose — their canonical colours are two shades
  of one khaki and Rock/Ground is a real dual type.
- **`rule` is LEFT-anchored** (`─ PARTY ──── 6/6`), not centred: `[ TRAINER ]` and
  `[ BADGES ]` were two rows restating the border's own title and the eight markers, and a
  centred label's column depends on how long it is, so the page could not be scanned down its
  edge. `[ MOVES ]` and `[ HIDDEN ]` became **column headers**, which earn the row by being a
  legend as well as a label.
- **A tall picture must not half-indent the table under it.** The half-block picture reaches
  past the identity rows, and a moves header indented over rows that are not is a legend that
  lies about its own columns — drawn exactly once, in a screenshot. `indent_beside` **pads**
  the beside-the-picture block to the picture's height; a blank row beside a picture is not
  waste, it *is* the picture. `row_width` gives the rows below it their full width back.
- **The row estimate must use the SAME composition that will be drawn.** `trainer_panel_rows`
  called `body_lines` (no pictures), which is one row short in the terminal that gains the
  "no party pictures" line — so the composer was handed one row too few and collapsed the
  party to one line each. Only visible in a screenshot of that terminal.
- **No two stats may share a label at any width.** The narrow `stat_line` tier used one letter
  each and printed `S55 S29` — Speed and Special, indistinguishable. Two letters cost two
  columns. `the_four_stats_never_share_a_label_at_any_width`.
- **`MoveSlot` carries power, type and accuracy** now, from the same `Moves` entry PP already
  came from (`animation, effect, power, type, accuracy, PP`; `macros/data.asm`). **Accuracy is
  a fraction of 255**, because `MoveHitTest` compares it against a random byte — `$FF` is
  100 %, `$CC` is 80 %. **Zero power is a status move, not a missing value**: the table prints
  a dash, never a `0` (which invites comparison with a 40) and never `?` (which claims we
  could not read it).
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
- **"The panel should scale with the game" — text cannot, so two things do.** The panel takes
  **two fifths of the width** rather than a fixed `PANEL_W`, and `party_cols` grows the
  name/bar/type columns above the design width instead of stranding the space. A fixed
  40-column panel beside a picture that grows with the window reads as "a footnote pinned next
  to a poster", which is exactly how it was reported. **`sprite_rows` used to be the third and
  is gone** — the sprite band grew with the panel, which is what made the picture "TOO HUGE";
  the pictures are icons in a fixed-width gutter now, so a wider window buys more *text* and a
  taller one buys more *party*. `party_cols`'s tiers key on the width **left of the gutter**,
  not the panel's, or the design width falls into the tier below.
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

- **Layout.** The contract is [`crates/terminalgb-plugin-api/`](crates/terminalgb-plugin-api/)
  (no dependencies); each plugin is its own crate under [`plugins/`](plugins/) —
  `trainer`, `gen1-link`, `battle-control`, `agent`, plus `example-tracer`, the one a
  stranger copies. The emulator's side of the contract is `src/plugin.rs`. Stranger-facing
  guide: [`docs/writing-a-plugin.md`](docs/writing-a-plugin.md); design rationale stays in
  [`docs/plugins.md`](docs/plugins.md).
- **The API crate is NOT tidiness — it is the only acyclic arrangement.** The TUI *draws
  panels* for the plugins it ships with, so `terminalgb` depends on the plugin crates. If the
  `Plugin` trait lived in `terminalgb`, plugins would depend back on it and Cargo would refuse
  the cycle. `MachineAccess` is the seam that made the split possible: the API crate declares
  what a machine must provide, `src/plugin.rs` implements it for `MemoryManagementUnit`, and
  `Machine` (the capability-checked handle) is defined over the trait, not over the MMU.
  **Never move the trait back into `terminalgb`, and never let a plugin crate depend on
  `terminalgb` outside `[dev-dependencies]`.**
- **A dev-dependency back-edge IS allowed and we use it.** `battle-control`'s tests need a real
  MBC, so it dev-depends on `terminalgb` with the `testing` feature and uses
  `terminalgb::testing::TestMachine` (`src/testing.rs`) — a real machine exposed only as
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
  and excludes first-party `terminalgb-plugin-api`. That is not a loosening: the crate has no
  dependencies of its own and is the code that used to be `src/plugin/mod.rs` (now `crates/terminalgb-plugin-api/`), so an embedder
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
  bugs below were found). `wezterm-gui --config enable_wayland=false start --cwd <abs repo>
  -- <abs cmd>` on XWayland (**absolute paths only** — a spawned terminal starts in `$HOME`;
  see the screenshot recipe near the end of this file), then
  `wezterm cli send-text --no-paste --pane-id 0` against **that instance's**
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
- **`$6F` — the cancel nybble — MUST be echoed from EVERY phase, and it is the only way off the
  trade *screen*.** (**Correction, measured 2026-08-15:** this entry used to end that sentence
  "the only way out of the Cable Club", and add that because the Trade Center has no warps,
  backing out of the trade screen *is* leaving the room. **Both halves are wrong and they cost
  a captain-reported bug hunt.** `ReturnToCableClubRoom` reloads the *same* map: the player is
  back in a room that has **no warp events at all** — `data/maps/objects/TradeCenter.asm`,
  `wNumberOfWarps` reads 0 at run time — so leaving the screen is emphatically *not* leaving the
  room. The cartridge's exit is **START → RESET**. See
  [`docs/gen1/leaving-the-cable-club.md`](docs/gen1/leaving-the-cable-club.md), which measures
  all of it.) `.cancelMenuItem_APressed`
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

### Leaving the Cable Club — the report that was NOT our bug (`docs/gen1/leaving-the-cable-club.md`)

Reported live: *"there is simply no leaving the damn room… Blue is trapped in the void trade
room. Gold has a door to walk out."* Measured end to end with
`cargo run --release --features image,plugin-gen1-link,plugin-trainer --example gen1_cancel_exit
-- --rom <blue.gb> --save <cableclub.sav>` — two whole cores on a `LinkCable`, so nothing of ours
is impersonating anything. **Three separate claims, three different answers:**

1. **One player cancelling alone is stuck, and that is the cartridge.**
   `.cancelMenuItem_APressed` falls through only when the round comes back `$F`, and the
   disassembly's own comment on that compare is *"did the other person choose Cancel too?"*
   Observed: 60 A presses on CANCEL with `wLinkState` pinned at `$32`, `wSerialExchangeNybbleSendData`
   (**`$CC42`**, read out of the cartridge) latched at `$0F`,
   `wSerialSyncAndExchangeNybbleReceiveData` (**`$CC3D`**) reading `$FE`, and the cancel cursor
   cell `wTileMap(1,16)` = `$C4E1` showing `▷` (`$EC`) — the *observable* signature of "pressing A
   on CANCEL for ever". Gen 2 is symmetric: `LinkTradeOTPartymonMenuCheckCancel` sends
   `SERIAL_TIMECAPSULE | $F` = `$6F` and requires `$F` back, so a Gold partner is under exactly
   the same rule and there is nothing for us to relay or translate.
2. **Both cancelling does leave the trade screen** (`recv` = `$0F`, `wLinkState` → `$01`).
3. **…and leaves you in a room with no door.** `wNumberOfWarps` reads **0** in the Trade Center.
   `ReturnToCableClubRoom` reloads the same map. **The cartridge's exit is START → RESET**:
   `StartMenu_SaveReset` jumps to `Init` whenever `BIT_LINK_CONNECTED` is set, which is why the
   START menu there shows RESET where it normally shows SAVE. Measured safe — RESET → CONTINUE
   put the player back on map `$29` (the Pokémon Center it saved in) with the party intact, and
   **neither console faulted, so no battery was poisoned**.

- **Gen 2 having a door is not a second bug, and it is cited:** `TimeCapsule_MapEvents`
  (`pokegold/maps/TimeCapsule.asm`) declares `warp_event 4,7` and `warp_event 5,7` back to
  `POKECENTER_2F`, one behind each seat. The asymmetry is the two cartridges, not the cable.
- **Nothing of ours was in the path.** Only the trainer plugin auto-attaches (`attach_plugins`),
  and it is read-only; `gen1-link` and `gen1-lobby` need an explicit `plugins = …`.
- **Now surfaced in the product**, because a room with no exit and no explanation reads as
  "the emulator has trapped me": `plugins/trainer/src/cableclub.rs` watches `wLinkState`
  (`$D12B`) with the panel closed — one peek at ~10 Hz, a second only once it is non-zero — and
  the TUI toasts `CableClub::advice()` on the way in (`App::announce_cable_club`), naming both
  obstacles. It also takes the trainer plugin's status row, outranking the encounter verdict,
  because it is the one thing there that is *actionable*.
- **The lesson for this file:** the old entry above asserted the exact opposite of (3) and was
  never measured. A prose note that contradicts the disassembly sends the next crew hunting for
  a defect that does not exist — *check the claim against the cartridge before building on it*,
  the same rule the APU campaign wrote as "check the baseline, not a commit message".

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

## The Gen 1 battle ENGINE, and the cartridge as an oracle (`crates/terminalgb-gen1-battle/`)

**Full page: [`docs/gen1/battle-engine.md`](docs/gen1/battle-engine.md).** A model of the
battle *rules* (damage, crits, accuracy, status, PP, the Gen 1 bugs), a differential harness
that holds it to the retail cartridge, agents that play it, and an evaluation protocol.
`cargo test -p terminalgb-gen1-battle` — **136 tests, no ROM on the machine.**

- **`plugins/battle-control` STARTS battles; this REASONS about them, and they are
  deliberately separate crates.** A `Plugin` is handed a machine during a frame the emulator
  already decided to run; an engine that searches thousands of hypothetical battles has no
  machine and no frame. Same conclusion `docs/mcp.md` §7 and `docs/shiny-hunt.md` reached from
  their own directions. The engine crate is **dependency-free and does not depend on
  `terminalgb`** — the oracle and the CLI live there, so the edge would otherwise be a cycle
  (the `terminalgb-plugin-api` / `terminalgb-chiptune` arrangement, for the same reason).
  `make embed` is unmoved: `gen1-battle` is in `default`, out of `--no-default-features`, and
  first-party.
- **THE CARTRIDGE IS THE ORACLE, AND IT EARNED THAT FIVE TIMES IN ONE SITTING.** Every
  correction below came from a refusal or a disagreement on the retail Blue image, not from
  reasoning, and each is now a named constant or a modelled behaviour with a citation:
  - **`PokedexOrder` is 190 entries with 39 zeroes**, not 151 dense ones — internal index
    numbers are not dense and the holes are the MissingNo slots.
  - **`BaseStats` is 150 records**: its own line is `assert_table_length NUM_POKEMON - 1 ;
    discount Mew`, and Mew lives at `MewBaseStats` in another bank.
  - **`MonsterNames` is 190 too.** Reading 151 loses most species — internal index 1 is
    RHYDON, not BULBASAUR.
  - **A CRIT DOES NOT READ THE ENEMY'S STAT, IT RECOMPUTES IT.** `GetEnemyMonStat`
    (`engine/battle/core.asm`) outside a link battle calls `GetMonHeader` on
    `wEnemyMonSpecies` and recomputes from base stats, `wEnemyMonDVs` and `wEnemyMonLevel`
    with **zero stat experience**. Identical for a plain wild Pokémon, which is why it hides;
    different for a trainer's Pokémon with stat experience. `BattleMon::crit_stats` is that
    asymmetry — the player's crit reads the party struct, the enemy's is reconstructed.
  - **`damage * r / 255` TRUNCATES, so it cannot be inverted by dividing back.** The harness's
    damage check must be a *forward* search over `r ∈ 217..=255`. Dividing back produced a
    false accusation on the first small hit: a predicted 6 observed as 5 inverts to
    `5*255/6 = 212` (outside the band) while `6*217/255 = 5` matches exactly. On small figures
    one output covers a dozen rolls, so the naive inverse accuses the model precisely where it
    is least likely to be wrong. **General rule: when a harness and a model disagree, suspect
    the harness first** — it is younger and nobody has run a hundred battles through it.
- **Two whole classes of check are REFUSED rather than faked, and both are battle-control's
  doing.** The plugin patches `wEnemyMon` *after* the engine filled `wEnemyMonUnmodified*`
  from the Pokémon it generated, so from the first stat change the cartridge is calculating
  with numbers no observer can see. Enemy-side stat recomputation is therefore not checkable
  at all in a plugin-built battle (it produced four false "disagreements" of the shape "the
  cartridge behaved as if the unmodified stat were ~8 % lower"), and any damage after a stage
  has moved is undetermined. **That is a narrowing of what is determinable, never a widening
  of a tolerance** — the damage band stays exactly `217..=255`.
- **`Undetermined` is never counted as agreement.** A harness that scored "could not tell" as
  a pass would have no teeth. Current measurement on retail Blue: **32 checked (2 exact, 30 in
  band), 51 undetermined, zero disagreements, mean implied roll 236.2 against a band centre of
  236.** That mean is the number that matters most — every hit passing individually is still
  consistent with a model wrong by a small constant factor, and only the mean exposes it.
- **Bugs are behaviour and are reproduced with tests that fail if someone tidies them away**:
  the Focus Energy *inversion* (it quarters the crit rate), crits reading BASE Speed, crits
  discarding stat stages on **both** sides (so a crit through your own Swords Dance deals
  *less*), the 1/256 miss, a resisted move counting as a **miss**, badge boosts compounding, a
  stat change wiping the burn penalty, Hyper Beam skipping its recharge on a KO, full
  paralysis being 63/256, the lost wake-up turn, Counter's negative priority.
- **Dice CONSUMPTION is part of the model.** A turn that reaches the right answer while
  drawing a different number of random bytes diverges on the *next* turn. `TurnLog::rolls`
  counts them and the tests assert them: a zero-power move draws no crit die, a decided speed
  race draws no tie-break die, and `RandomizeDamage` is a rejection loop that can eat several.
- **What is not modelled SAYS SO, per battle.** `effects::support` classifies all 86 move
  effects; an episode that touched an unmodelled one carries it, and the report refuses to
  present an unqualified win rate (`QUALIFIED: only 94.2% of episodes used exclusively
  modelled effects`). A win rate measured through a silently-dropped Disable is not a win rate.
- **The evaluation protocol's seed comes from the EPISODE INDEX, never from the agent**, so
  two agents run on identical dice and nobody can be re-rolled until they win. Both sides of
  every matchup are played; illegal choices are counted and never silently improved; a run
  under `Provenance::Foreknowledge` prints "NOT A COMPETITIVE SCORE"; Gen 1 bugs that appeared
  in a *winning* line are named in the report, because an agent that wins by the 1/256 miss is
  an interesting result and hiding it would make the number a lie.
- **MEASURED, 720 episodes (`--seed 2654435769 --episodes 40 --level 30`): random 49.7 %,
  greedy 72.6 %, expectimax d3/s3 72.2 % vs random and 46.5 % vs greedy.** **Expectimax does
  NOT beat one-ply greedy**, which is the opposite of what was expected and is written at the
  top of `ai/expectimax.rs` rather than buried. An earlier 216-episode run read 52.3 % and was
  noise — which is why every report prints a Wilson interval. Random-vs-random landing on
  49.7 % is the protocol's own sanity check.
- **A one-ply agent may be naive; it must not be suicidal.** Greedy first measured *below*
  random (33 % vs 42 %) because the team builder handed every Pokémon **Explosion** and greedy
  picked the biggest damage number on the board. `ai::greedy::self_damage` prices what a move
  costs its user, and the default roster excludes `EXPLODE` and `OHKO` because a roster with
  either measures a different game. 33 % → 72.6 %.
- **An external agent is a SUBPROCESS** (`ai::ExternalAgent`, JSON lines on stdin/stdout) —
  no dependency, language-agnostic, and the network stays out of the engine, the same three
  reasons `plugins/agent`'s `CommandDriver` chose one. It is never given the dice, the seed or
  the opposing party's hidden members; stat stages are reported as a **player** reads them
  (−6..+6), not as the cartridge stores them (1..13). A child that crashes or answers illegally
  does **not** get its turn replayed — the substitution is counted.
- **A Gen 1 battle sits at its own menu until a button arrives.** The oracle's first version
  ran 3000 frames and observed nothing while the plugin cheerfully logged "battle started".
  Press every `MENU_GAP` (24) frames — `HandleMenuInput` delays several frames after accepting
  one and does not see a button that arrives inside that delay.
- **Do not observe for the first ~90 frames of a plugin-built battle.** `BattlePlugin`
  re-applies its opposition override until it reads back intact, up to `PATCH_WINDOW_FRAMES`
  (40); a frame taken inside that window holds a half-written struct.
- **Nothing here is in CI except the unit tests.** The oracle and the evaluation need the
  player's own cartridge, so they are run by hand — the same rule `tools/sameboy-oracle/` and
  `tools/bgb-interop/` follow. **No LLM has been measured through the seam**; it exists, is
  tested against a real child process, and has never had a model on the far end.

## Checking somebody else's Gen 1 claims — and the corner of our own model it exposed

**Full page: [`docs/gen1/paper-claims.md`](docs/gen1/paper-claims.md)** (the table, the
provenance, the sample sizes); conformance record in `docs/conformance-notes.md`
(2026-08-16). Instrument: `examples/gen1_paper_claims.rs` + its `.preset`; gate:
`testharness/paperclaims.rs` (`POKE_ROM=… cargo test --release --features
plugin-battle-control,plugin-trainer --test paperclaims`, skips exit 0 without a cartridge,
**not in CI**). Both share `examples/support/paperclaims.rs` so the instrument and the gate
cannot drift.

- **THE ORACLE NEVER SWITCHES, AND THAT HID A REAL BUG FOR THE WHOLE LIFE OF THE ENGINE.**
  `gen1_battle_sim` compares stats and damage inside one battle, so nothing checked which
  volatiles survive a Pokémon leaving the field — and `Volatile::on_switch_out` **preserved
  Reflect and Light Screen**, with a confident comment calling that a Gen 1 behaviour and a
  unit test underneath pinning it. `SendOutMon`, `EnemySendOut` and `EnemySendOutFirstMon`
  each zero five bytes from `wPlayerStatsToDouble` / `wEnemyStatsToDouble` — `$D060`-`$D064`
  and `$D065`-`$D069`, **all three `BattleStatus` bytes** — so the screens go with everything
  else. Measured on the cartridge through a real menu-driven switch. **The evaluation numbers
  are unaffected and that was checked, not assumed**: `examples/gen1_battle_ai.rs::build`
  only ever picks moves with `power > 0`, so a 0-power screen can never be in an evaluated
  team.
- **A comment that asserts a behaviour is a claim, and a claim with no test is a guess.**
  Same shape as `src/link.rs`'s torn-byte note: prose, code and a green test all agreeing
  with each other and disagreeing with the cartridge. When you check somebody else's claims,
  check them against your own code too — the row where they were right was worth more than
  the rows where they were not.
- **KEY AN EPISODE ON THE FLAG, NOT THE COUNTER.** `MoveHitTest`'s `.moveMissed` does
  `res USING_TRAPPING_MOVE` and leaves `wPlayerNumAttacksLeft` standing, and `TrappingEffect`
  runs *before* the hit test — so a missed Wrap sets both and clears only the flag, and the
  next Wrap's fresh roll reads as a decrement of the old one. The first version of this
  harness recorded a counter going `4 -> 2` and would have published a duration the game has
  never rolled. 73 of 371 trapping locks and 39 of 161 thrash locks end early like this
  (thrash's are self-confusion damage, which clears the whole of `wPlayerBattleStatus1`).
- **The measured rolls, so nobody re-derives them:** confusion **2-5** (97 episodes), Disable
  **1-8** with the slot uniform over 1-4 (136), thrash **2-3** (161), Bide **2-3** (281),
  partial trapping **1-4** in the source's 3/8-3/8-1/8-1/8 shape (371). Every countdown steps
  down by one and the effect ends when it reaches zero, so "turns remaining after this one"
  is `roll - 1` for confusion and Disable and `roll` for the three that share
  `wPlayerNumAttacksLeft`. Badly-poisoned residual is `max(1, floor(maxHP/16)) * N` —
  200/200 unclamped ticks — and the counter has **no bound in the code** while a plain battle
  cannot push it past 6 (measured max 5; **10** against a target that heals every turn).
- **The critical-hit rate is measured against an ABSOLUTE prediction, not a ratio.** For a
  level 5 Electrode (base Speed 140, the highest in the game, which is what makes a few
  hundred attacks enough) `CriticalHitTest` gives 70/256 without Focus Energy and 17/256
  with; measured **0.2604** (250/960) and **0.0612** (36/588), both inside their 95 %
  intervals. A ratio alone would pass a model with both halves wrong by the same factor.
- **The crit measurement makes the OPPOSITION the attacker, deliberately.** A harness that
  advances a battle by pressing A always takes move slot 1, so a player-side Focus Energy
  could never be followed by an attack; a wild Pokémon picks one of its four slots uniformly
  (`SelectEnemyMove`, `.chooseRandomMove`), so Focus Energy in slot 1 lands early and stays
  up while the other three keep attacking. **One battle is worth ~40 attacks and no more** —
  the player's Splash has the game's maximum 40 PP and the move menu then refuses it and the
  battle stands still (20 attacks in 12,000 frames, 40 in 60,000, still 40 in 120,000), so
  the sample comes from many battles.
- **The Gen 1 battle menu is `FIGHT PKMN` over `ITEM RUN`** — PKMN is to the **right** of
  FIGHT. Pressing Down opens the bag, whose `CANCEL` entry looks enough like a party list to
  fool a needle search, which is exactly what the first version of the switch route did.
- **`terminalgb::KeypadKey`, never `terminalgb_plugin_api::KeypadKey`, in a `harness = false`
  test.** The second spelling can resolve to a different rlib and the error reads like a
  broken `Gameboy::keydown` signature. Same family as the `BattlePlugin: Plugin` failure that
  `touch src/lib.rs` clears (`docs/pitfalls/builds.md`).

## The Gen 1 memory atlas — AtlasGB owns the data, this repo produces the evidence

**The atlas moved out.** It lives at **<https://github.com/Alchemy86/AtlasGB>** (MIT, its own
CI, its own release cadence); a pinned snapshot is vendored here at
[`third_party/atlasgb/pokemon-rb/`](third_party/atlasgb/README.md) and
[`docs/gen1/atlas/README.md`](docs/gen1/atlas/README.md) is now a **pointer page**, not a copy —
twenty-two generated pages and a 238 KB TSV used to be duplicated in this tree, and a mirror
nothing regenerates is wrong within a month. The harness is still
[`testharness/gen1atlas.rs`](testharness/gen1atlas.rs). `docs/gen1/memory-map.md` is still the
right page for *why* — it is task-shaped and explains mechanisms. The atlas is the *where*:
2,898 entries, **every byte of WRAM and HRAM accounted for**.

- **THE POINT IS NOT THE MAP, IT IS THAT THE MAP IS CHECKED.** Every published Gen 1 memory
  map is a transcription — somebody read an address once and everybody since copied it. This
  repository has a cycle-accurate emulator, so an entry can carry *evidence*: **R** the
  cartridge names the address as an instruction operand, **L** the byte changed during a
  fixed script on a real cartridge, **I** a named invariant proved it means what it says.
- **THE SPLIT IS THE WHOLE POINT AND IT IS NOT ARBITRARY: upstream owns everything provable
  from the file alone, we own everything that needs a machine.** AtlasGB's CI proves it
  parses, that addresses lie inside their regions, that the walk is complete, that every link
  resolves. It cannot prove a single tier, because proving an address against a running
  cartridge needs a cartridge and an emulator and it has neither. So the flow is one way: the
  snapshot comes in, the evidence goes back out as a report.
- **QUOTE THE TIERS THE WAY UPSTREAM PUBLISHES THEM — over STORAGE entries.** On the retail
  cartridge: **1,594 R, 2,694 L, 36 I, and 7 storage entries with no evidence at all**, which
  the pages say out loud rather than hiding. Counting all 2,898 entries instead gives `inv`
  **39**, because three ROM-table labels (`MonsterNames`, `BaseStats`, `PokedexOrder`) earn
  `inv` by having their *contents* read out of the cartridge — "observed live" means something
  quite different for a table in a ROM than for a byte of RAM. The harness now prints both
  lines so 36 cannot silently become an unexplained 39; the older note here said 39 with no
  qualifier and disagreed with the published atlas.
- **THE `verify` COLUMN IS THE ONE BASELINE IN THIS REPOSITORY THAT IS NOT OURS TO REWRITE.**
  There is no `UPDATE_ATLAS_VERIFY` any more and there must not be one: upstream makes that
  column unwritable by hand on purpose and checks a SHA-256 digest of it in its own CI, so a
  hand-edited tier is a claim with nothing behind it — the exact failure the project is a
  reaction to. A disagreement is still red in either direction; it is landed by **publishing**:
  `ATLAS_EVIDENCE_OUT=report.json POKE_ROM=… cargo test --release --test gen1atlas`, then
  `tools/apply-evidence.py report.json && make docs data` in a clone of AtlasGB, then re-pin.
  `make atlas-evidence` is the target. The report must carry **every symbol including the ones
  that earned nothing** — a partial report reads as a downgrade and upstream refuses it — and
  is written **only** when the env var names a path, because a file that appears on every test
  run is a file somebody commits by accident. It also carries a top-level `"atlas"` key —
  optional upstream (it defaults to `--atlas`) but the better habit now that there is more than
  one atlas, because it makes the report self-describing rather than dependent on how it is
  landed.
- **The migration was proved faithful, not asserted: landing our report upstream moved 0
  entries and reproduced the published `verify_digest` `sha256:59c413f2ff645ff3…` exactly.**
  Do that check after any change to the tier machinery — it is the one measurement that says
  the evidence this repository produces is the evidence the published atlas carries.
- **ATLASGB IS AN ATLAS *PROJECT*, NOT ONE ATLAS, AND BOTH SIDES ARE NAMESPACED BY ATLAS ID.**
  Pokémon Red/Blue is `pokemon-rb`, its first; upstream keeps the data at
  `atlases/pokemon-rb/data/atlas.tsv`, project-wide pages at `docs/` (`consuming.md`,
  `verification.md`, `schema.md`) and this atlas's chapters at `atlases/pokemon-rb/docs/`
  with their filenames and their `#s-<symbol>` / `#a-<ADDR>` anchors unchanged. Our vendored
  copy and `tools/fetch-atlas.sh --atlas <id>` (default `pokemon-rb`) mirror that, so a
  second atlas is a flag rather than a restructuring. **`--atlas`, never `--game`** — one
  atlas covers two games, which is the whole reason the unit is an atlas.
- **VENDORED, NOT A SUBMODULE AND NOT A BUILD-TIME FETCH — and the lock is the load-bearing
  half.** A submodule breaks shallow CI checkouts and source tarballs and makes a one-file
  dependency cost a clone; a build-time fetch would put the network on the path of
  `cargo test`, which is fatal because Tier A and the anti-drift gate are supposed to run on
  every plain test run. A vendored file is exactly the file somebody eventually hand-edits, so
  `tools/fetch-atlas.sh --verify` recomputes the sha256 offline **and the harness makes the
  same comparison in-process** — teeth proven by editing one `verify` cell and watching Tier A
  go red naming both digests. Same shape as `third_party/dmg-acid2/` and the GBSelfTest
  cartridge; one pattern for every artefact consumed from outside this tree.
- **SHA-256 AND SHA-1 ARE HAND-ROLLED IN THE HARNESS AND MUST STAY THAT WAY.** The dependency
  count is a contract `make embed` measures, and shelling out to `sha256sum` would put a
  program that need not exist on the path of `cargo test` on every platform this core is
  cross-built for. Same call as the FNV hash, the PNG codec and the inflater. Both were
  cross-checked against the system tools: the sha1 the report emits is byte-for-byte
  `sha1sum`'s and matches the one already recorded upstream.
- **`tools/fetch-atlas.sh` IS UPSTREAM'S OWN SCRIPT, COPIED — do not reinvent it.**
  AtlasGB's `docs/consuming.md` asks consumers to copy it and the lock format is shared (down
  to the `atlas=` line), so its copy and ours read each other. Three adaptations, all marked
  in the file: the destination resolves from the script's own location; `ATLASGB_LOCAL=<checkout>`
  can stand in for a published ref (AtlasGB has cut no release tag yet, and it is how a data
  change is tried here before it is published); and `--file` names the path explicitly, so a
  ref from *before* the namespacing is still pinnable.
- **THE PIN IS `1f8e7fe`, THE NAMESPACED LAYOUT, AND THE MOVE COST NOTHING — which is worth
  knowing before anyone treats a re-pin as risky.** It was first pinned at the pre-namespace
  `data/atlas.tsv` while upstream was mid-restructure and re-pinned once that landed, and the
  vendored bytes are **identical across the two**: same `dc07eeec…`, git recorded a pure
  rename. So a layout change upstream is provenance, not a correction, and the lock's checksum
  survives it. Re-pin in a commit of its own — `tools/fetch-atlas.sh --ref <ref>` then
  `make test-gen1-atlas` — and ask the captain for a `v1.0.0` tag while you are there:
  `consuming.md` recommends pinning a tag over a bare commit and there is not one yet.
- **DO NOT PUT A COMMENT BANNER ABOVE THE VENDORED TSV.** The file this repository used to
  own opened with six `#` lines; AtlasGB's does not, on purpose — GitHub renders a TSV whose
  first line is the header row as a *table*, and a preamble turns it back into plain text.
  `tools/fetch-atlas.sh` sanity-checks for that header row and refuses anything else, which is
  also what catches a 404 page being saved as an atlas.
- **UPSTREAM'S JSON HAS ONE BREAKING RENAME AND WE DO NOT READ IT — but check before you do.**
  In the namespaced layout the exports' `meta` block renames `name: "AtlasGB"` to
  `project: "AtlasGB"` and gains `atlas`/`title`/`platform`/`engine`/`pages`; the schema moves
  to `schema/atlas.schema.json` and no longer pins `source_of_truth` to a const path; and
  `atlases/<id>/meta.json` is the new machine-readable "which cartridge is this". Nothing here
  parses `atlas.json` — this harness reads the TSV — so none of it reaches us today. It would
  the moment anybody reached for the JSON.
- **`make atlas-upstream` is ADVISORY and is in no gate.** A build must not break because
  somebody else published; `--check-upstream` exits 0 whether upstream has moved or not, and
  with no network at all. Re-pinning moves what the harness scores, so it belongs in a commit
  of its own.
- **The anti-drift gate is the thing to keep: every Gen 1 address hard-coded in a plugin must
  appear in the atlas.** `wCurMap` was written out in **nine** separate files before this
  existed and nothing could tell you whether they agreed. It is a *text scan* for
  `const NAME: u16 = 0x____;` across a named list of files — deliberately, because making
  every consumer import a shared table is a large mechanical change across crates other crews
  are editing, and the drift that matters is "somebody wrote down an address nothing has heard
  of". **107 constants across 9 files gated today**, and adding a file to `PLUGIN_SOURCES` is
  how a new consumer joins. It is not plugins-only: `src/gen1battle.rs` and
  `src/longplay/mod.rs` carry Gen 1 addresses of their own and both agree with the atlas.
  *(Upstream's `consuming.md` quotes 98 across 7 — that was true when it was written; this
  gate has only ever grown, and it must not shrink to match a number in a document.)*
- **`battle_struct` IS 29 BYTES.** `plugins/battle-control/src/wram.rs` said 29 and
  `plugins/trainer/src/gen1.rs` said 28; the map settles it and the harness proves it by
  requiring `wTrainerClass` to sit immediately after `wBattleMonPP + 4`. Do not copy the 28.
- Tier A needs no cartridge **and no network** and runs in CI under a plain
  `cargo test --release`; tiers B and C skip with **exit 0** without `POKE_ROM`, like
  `colosseum.rs`.
- **The live sweep runs WITHOUT a save, on purpose.** Its result must depend only on the
  cartridge and the fixed script or it cannot be a baseline. The invariants run on a
  *separately staged* copy carrying the player's real save — one staged path for both was a
  real bug here, where whichever ran last silently decided the other. Neither ever writes
  beside the user's ROM. The report records which of the two it was, because a save changes
  which invariants can run and that is provenance, not a detail.
- **AN INVARIANT CAN PASS FOR THE WRONG REASON, AND SAYING SO IS THE JOB.** `owned ⊆ seen` is
  symmetric when the two sets are equal, so on a save where everything seen was caught it
  cannot tell `wPokedexOwned` from `wPokedexSeen` — proved by swapping the symbols and
  watching it stay green. The check now reports that it is vacuous, and the asymmetric
  replacement ("the party is in the Pokédex") cross-references three independently located
  things: the party's *internal* species index, `PokedexOrder` **read out of the cartridge**,
  and `wPokedexOwned`. Shifting `PokedexOrder` by one byte reddens it and the content check
  beside it, independently.
- **Teeth proved, not claimed:** moving `wPartyCount` one byte reddens the ordering and alias
  checks; an invented plugin constant reddens the drift gate by name; a one-byte
  `PokedexOrder` shift reddens two checks; a one-cell edit to the vendored TSV reddens the lock
  check. The RHYDON signature check caught a typo of *mine* (`D` is `$83`, not `$84`).
- **`gap` and `free` rows are why the coverage adds up.** The Stack section is `$DF00-$DFFF`
  and its only label `wStack` sits at `$DFFF` because the pointer grows **downwards** — so
  255 bytes are "reached by no label", which is the honest description and not a hole. `free`
  rows are runs the linker reported EMPTY.
- **THE EXTRACTOR AND THE RENDERER ARE UPSTREAM'S NOW — `tools/gen1-atlas/` is deleted, and
  `make gen1-atlas` and `docs-check`'s `render.py --check` are gone with it.** Refreshing the
  data from a built `pret/pokered` checkout, re-rendering the twenty-two pages, and the
  `<!-- atlas:… -->` marker discipline all live in AtlasGB (`make extract POKERED=…`,
  `make docs data`). Do not re-add a second copy here; **the pages that were deleted were
  generated files, and regenerating them in this tree would recreate the drift the split
  exists to remove.**
- **Licence line, unchanged and load-bearing:** addresses, symbol names and structure layouts
  are **facts** and are cited; pokered's prose and comments are **not ours to copy**. Nothing
  from pokered is vendored in either repository, nothing is fetched at run time, and no ROM or
  game data is in either tree. AtlasGB is MIT; its `docs/licensing.md` is the honest longer
  answer, and it is a better one than "MIT" alone.
- **Yellow is deliberately out** — its work RAM shifted, so an address from this map is *wrong*
  there rather than approximately right, and the failure mode is a write landing in the middle
  of somebody else's data that still passes its own checksum.
- **The highest-value contribution left is a SECOND LIVE SCRIPT, and it can only be written
  here.** The current one plays the opening and then walks and opens menus, so it never reaches
  a battle or a PC — which is why several hundred entries are honestly unmarked. A script that
  starts from a save, fights something and opens the storage system would move them from "no
  evidence" to observed. The machinery exists; only the script is missing.

## The TEST RIG — one command, two players, the same fight (`pewter_chain ab`)

**Reader-facing page: [`docs/agent/test-rig.md`](docs/agent/test-rig.md); the
measured first result: [`docs/agent/test-rig-results.md`](docs/agent/test-rig-results.md).**
Reusable half `examples/support/abrig.rs` (pure, unit-tested, no cartridge);
front end `cmd_ab`/`cmd_abfilm` in `examples/pewter_chain.rs`; host-side battle
observation `scenario::run::{Battles, BattleSummary}`.

- **THE POINT IS THE INSTRUMENT, NOT THE FIRST ANSWER.** Every number this
  project has published — the four-emulator conformance score, 81-84 % damage
  agreement, the null ablations, fourteen certified links — came from code
  written once and left behind, so **no two claims are strictly comparable,
  nothing is re-runnable by somebody who was not there, and every new question
  cost a new harness**. This is the harness once, with the question as a
  `gbtestrig/1` plan file. A committed plan is a test somebody else can re-run.
- **AN ARM IS A DRIVER AND NOTHING ELSE.** `play_with` is `play` with the
  driver lifted out, so the entry state, phase offset, scenario, referee,
  sampling clock and battle bookkeeping are **literally the same code** for
  every player. A comparison whose two halves ran through different harnesses is
  not a comparison, and one code path is the only cheap guarantee.
- **THE ARMS ARE PAIRED, SO THE TEST IS McNEMAR'S, NOT TWO PROPORTIONS.** Which
  entry state, which driver seed and which phase offset are all functions of the
  seed alone, so two arms at seed N play the same fight. **A gap between arms
  that did not play the same seeds is REFUSED, never approximated by an unpaired
  comparison** — the pairing is the whole design and silently dropping it
  produces a number that looks the same and means something else.
- **`sqrt(b+c)/n`, NOT the maximum-likelihood `(b + c − (b−c)²/n)/n²`.** The
  corrected form degenerates to **zero** when every discordant pair falls the
  same way, so two attempts print a hundred-point gap with a zero-width
  interval — certainty manufactured from an artefact. The conventional McNemar
  error is slightly conservative and cannot do that. Consequence to expect: a
  gap interval may exceed 100 points at the boundary.
- **Wilson, never the normal approximation.** At a rate near 1.0 the normal one
  puts the upper bound above 100 %, and at a rate of 0 it returns `[0,0]` —
  certainty from no evidence. Both boundaries are pinned by tests, one of them
  against this project's own published 2999/3000.
- **SUBJECTS ARE NEVER AVERAGED, and when there are two the rig tests whether
  the GAPS differ.** Pooling two fights with different ceilings is a number
  about the mixture and about neither fight. Same gap on both ⇒ the distillation
  is subject-independent and the fights merely have different ceilings; the gap
  itself moving ⇒ something subject-specific in the player. Two subjects are
  independent samples so the difference of their paired gaps has variance equal
  to the sum, and **the report says plainly when the sample cannot separate
  them** rather than picking the flattering reading.
- **A PLAN WITHOUT A `split` IS REFUSED.** `seen` (trained on this fight —
  measures distillation quality, NOT generalisation), `unseen` (trained
  elsewhere — measures whether it learned to battle or memorised a fight),
  `n/a` (no trained player). Printed in capitals at the top of every report,
  because a `seen` number quoted as generalisation is the worst thing this rig
  could produce. **Nothing in the published literature runs the `unseen`
  control**: the competition benchmark is a fixed route, so a memoriser and a
  battler score identically there and the question never arises.

### THE MEASURED EVENT FOR MOVE LEARNING IS A PROMPT, AND THE CORPUS CONTRACT IS TWO FILES

**The rig's subject is level-up move learning, not battles.** A level-up prompt
is a *closed* problem — `docs/gen1/learning-moves.md` enumerates 840 distinct
decisions across 151 species — so it has one right answer per case, which is
exactly what a distillation can be scored on. The unit is a **prompt**: what was
held, what was offered, what each player did, and whether they agree.

- **A producer owes the rig two files per prompt and nothing else**: a
  `<pool>/NNNNN.gbss` save state, and beside it a `gbtestprompt/1`
  `<pool>/NNNNN.expect.json` naming `correct_moves`. The state→action training
  corpus never touches the rig. That division is deliberate — a rig that also
  knew about corpus formats would need changing every time one did.
  **`gen1_learn_drill drill --prompts <dir>` writes it**, on the first frame the
  `Delete an older move…?` box is *answerable* — before the cursor exists the
  question is still typing itself out, and after the first press the answer has
  been given, so that is the only moment a foreign player can be handed the same
  question. Only a **verified** decision earns a place: a prompt the drill could
  not itself answer correctly has a "right answer" that is a claim rather than a
  measurement. Numbering continues from what is already in the directory,
  because both lines go into one pool and a run that restarted at 00000 would
  silently overwrite the other species.
- **A SUBJECT IS USUALLY A LINE, SO `lead_species` TAKES A LIST**
  (`[178, 180]`). The Charmander line meets its prompts as a CHARMELEON and a
  CHARIZARD; filing those under two subjects of two prompts each answers a
  question nobody asked. One number still parses.
- **`"runs": "pool"` — one attempt per entry state the subject holds.** An entry
  is `seed % pool.len()`, so a fixed count that does not divide the pool runs
  some prompts twice and others not at all, and the interval beside the rate is
  over an *n* that is not the number of questions asked. Meaningless for a
  *battle* pool, where entries are interchangeable worlds and the run count is
  the sample size.
- **A DECLINE IS SCORED BY A PLAYER THAT DOES NOTHING, so it gets its own
  column.** A decline's correct move set is the one the prompt arrived with.
  Measured on the first run: the `mash-a` floor scored **3 of 9 — every one of
  them a decline — and 0 of 6 replacements**, while never reaching the goal on
  any attempt. Pooled it reads as a third of the task. `Cell::prompts_split`,
  `declines_are_counted_on_their_own`.
- **What a player chose is read off the party it left behind**
  (`wPartyMon1Moves`, via the `lead_move1..4` named watches), so nothing is
  inferred from a recording and every player is scored identically. One machine
  per attempt, no emulated frames.
- **SLOT ORDER IS PART OF THE ANSWER.** Gen 1 replaces a forgotten move in
  place, so a set comparison would score a wrong choice as right whenever the
  multiset matched. Pinned by
  `a_prompt_is_answered_in_slot_order_not_as_a_set`.
- **AGREEING WITH THE TEACHER AND BEING RIGHT ARE DIFFERENT QUESTIONS, and the
  report carries both.** Two players can each be wrong *and wrong the same way* —
  a student that faithfully learnt its teacher's mistake, which is a success for
  the distillation and a failure for the criterion, and is invisible in a
  correctness column alone. `Cell::agreement`,
  `agreeing_with_the_teacher_is_not_the_same_as_being_right`.
- **An entry with no expectation beside it is in NEITHER column.** A pool that
  is half annotated measures its annotated half rather than marking the rest
  wrong.
- **A species difference cannot come from the decision itself.** Of the 840
  decisions there are 581 distinct (four held, offered) pairs and the number
  whose answer differs by species is **zero**, so a student that does better on
  one starter is not doing so because of the move-learning decision. Say that
  rather than letting a reader infer a skill the task does not contain.
- Per-attempt rows go to `<out>/<subject>-<arm>/attempts.ndjson` with the four
  moves each attempt left. **A summary cannot be debugged**: when a prompt
  scores wrong the only useful question is which four moves the player chose.

### IT SAYS HOW FAR IT HAS GOT, BY DEFAULT — and where the wall clock went

- **Progress to STDOUT every 5 s, plus the same tick in `<out>/progress.ndjson`.**
  On by default with `--quiet` to silence it, never a flag to enable it: a run
  that prints nothing until it finishes cannot be told from a wedged one, and an
  hour went on checking process tables for exactly that reason. It was on
  *stderr*, which is the AXI default and is **wrong for this tool** — an agent
  drives it through a shell and does not read stderr, the same correction
  `9d02819` made to `examples/scenario.rs` and `gen1_tables.py`. It is safe
  beside the report because it is TOON with a name of its own
  (`progress[1]{done,total,pct,cell,…}`), so nothing can mistake a tick for a
  row of the result and an agent that wants only the result drops every block
  with that name. A one-row *table* rather than the object form, because a tick
  is a sample and a stream of samples is a collection: every tick carries its
  own header and parses alone. The file is written whether or not the line is.
- **`WHERE THE WALL CLOCK WENT` splits an arm's attempts into emulating, setup
  and the rest**, because a rig like this is worth having only if it is fast and
  somebody has to know which part to attack. `setup` is the per-attempt tax an
  *episode* pays — machine, phase offset, start state — and is the number that
  decides whether short episodes are affordable. Measured on a short link:
  **89.6 % emulating / 10.2 % setup for an expert attempt, 97.1 % / 2.7 %
  overall.**
- **THE REMAINDER IS NOT THE PLAYER'S THINK TIME AND READING IT AS SUCH IS THE
  TRAP.** The emulator never blocks on a driver, so a subprocess arm's
  spawn-and-decide happens while the machine keeps running and is charged to
  `emulating`. **A slow player shows up as more FRAMES per attempt**, which is
  why the frame count sits in the table beside the shares.

### THE RIG ALREADY CLAIMED THE THROUGHPUT WORK — and two wastes beside it did not

Checked against `pewter_chain ab` specifically, because a rig whose value is the
speed of the loop must not leave this repository's throughput work on the floor.
`corpus` is the fair comparator: the same emulator doing the same work on the
same entry states. Every figure below is one fixed job — **60 attempts of pewter
link 04, from the same 600-state pool, at the same seeds and phase spread
3001** — on a 20-core box shared with other crews, with the load average stated,
because a wall clock on a shared box measures the box's afternoon.

- **Fan-out: present.** 41.87 s at `--workers 1` against 3.65 s at `--workers
  16`, load 1.67/1.52 — **11.47x**, of the wall clock of the identical run.
- **Collector mode: present, and declared before the phase offset.** 42.07 s
  with `collecting()` + `set_render_pixels(false)` + `set_render_audio(false)`
  against 51.88 s with all three removed, one worker, load 3.92 — **1.23x**. The
  order matters: a phase offset averages half its spread, so about a third of
  everything the program emulates happens before `ScenarioRun` exists.
- **The battery: never touched.** `Gameboy::new(rom, None)`, so 0 `.sav` files
  and 24 KB of output for the whole run.
- **THE ENTRY POOL WAS CLONED ONCE PER SUBJECT AND AGAIN PER WORKER.** 600 save
  states at 142 KB is 85 MB, sixteen workers is 1.4 GB of memcpy for data
  nothing mutates, and a three-subject three-arm plan does it nine times. One
  `Arc` and a per-subject list of indices instead. Interleaved before/after at
  load 1.98 and 4.22: **wall 4.28 -> 3.85 s and 4.27 -> 3.85 s (1.11x), peak RSS
  1,911,508 -> 160,560 KB (11.9x)**. The same correction is on `corpus`, whose
  trajectory and final-state hashes are byte-identical either side of it.
- **The lead species was read serially for every state in the pool**, one whole
  `Gameboy` each, including for plans where no subject filters by lead and the
  answer is never consulted. Skipped in that case, fanned across the workers
  when needed.
- **`--workers` was ignored whenever the value typed equalled `cores - 4`.**
  Comparing a value against the default cannot tell "not given" from "given the
  default", so a plan's own `workers` survived an explicit override. There is a
  `workers_given` flag now. **General: never infer "was this argument given"
  from its value.**
- **Parity, both sides in seconds.** Five interleaved rounds at 16 workers, load
  rising 3.2 -> 17.4 as another crew fanned out: `ab` 3.86 / 8.07 / 7.67 / 7.29
  / 8.28 s against `corpus` 7.72 / 8.34 / 8.23 / 9.00 / 9.37 — `ab` faster in
  five of five. Pinned to one P-core at one worker over 20 attempts, which is
  contention-immune because it is CPU seconds: `ab` 13.78 / 13.56 / 13.43
  against `corpus` 13.73 / 13.48 / 13.39. So **`ab` is 0.3 % behind at one
  worker** — it reads the lead's four move slots out of every finished state,
  which is the rig's own product — **and ahead at fan-out**, because it writes
  no per-attempt recording.

### THE FIRST MOVE-LEARNING RESULT, AND WHAT THE HELD-OUT CONTROL SAID

**Full page: [`docs/agent/move-learning-result.md`](docs/agent/move-learning-result.md).**
Nine prompts (Charmander 4, Bulbasaur 5, three of them declines), a teacher that
replays the drill's own answers, a student fitted from the drill's corpus, and a
press-A floor. **Teacher 9/9, student 8/9, floor 3/9 — and all three of the
floor's are declines it got by inaction.**

- **The gap teacher − student is +25.0 points on one line and +0.0 on the other,
  and the two CANNOT be separated at nine prompts** (difference +25.0, se 25.0,
  p = 0.317). Say so. And the task forbids the flattering reading anyway: of the
  840 decisions, the number whose right answer differs by species is **zero**.
- **THE HELD-OUT CONTROL AT NINE PROMPTS WAS 0 of 9 BOTH DIRECTIONS — AND IT WAS
  MEASURING THE CORPUS, NOT THE METHOD.** The reason was in its own overlap
  figures: across the two lines the corpora share **one held move name of eleven
  and none of the nine offered moves**, so the unseen student had never seen a
  move it was asked about. **Scaled to 140 species lines it transfers: 73 of 97
  held-out prompts, 42 of 60 replacements, against a floor of 0 of 60.** The
  policy is the same three counters; only the corpus grew. **The general lesson:
  a negative held-out result at a tiny sample means "the corpus is too small to
  say", not "the method does not work" — and the way to tell them apart is to
  measure the overlap between train and test before drawing the conclusion.**
- **The student may not read the drill's `label` block** — it is ground truth,
  not something a player sees. `policy.py` raises rather than quietly scoring a
  cheat.
- **The teacher is a REPLAY, not a second implementation of the criterion.** The
  criterion lives in `tools/gen1-tables/learn_table.py` and only there; two
  implementations of one rule disagree the moment either is improved. So the
  teacher's 9/9 is a statement about the pool and the harness, not about the
  criterion.
- **The prompt's goal condition is `watch:in_battle=0`, and it was earned rather
  than chosen.** A move-learn prompt sets no event flag. What the cartridge does
  record is that the prompt happens *inside* the battle sequence: measured on
  all nine pool states, `wIsInBattle` ($D057) reads `$01` on the frame the
  question is answerable and `$00` once it has been answered and the game has
  left the battle. It is 1 at the start of every attempt, so it cannot fire
  before the player acts.
- **THE ANSWER DOES NOT LAND UNTIL THE TEXT AFTER IT HAS BEEN READ.** The game
  prints `LEARNER forgot LEER!` and `LEARNER learned RAGE!` and only *then*
  writes the party. An adapter that waits on those boxes leaves the move set
  exactly as it found it — measured: every arm scored the pool's own `held`
  values and the prompt looked unanswered when it had in fact been answered
  correctly six decisions earlier. `advance_text` is safe to give any arm: it
  refuses to answer a question.
- **THE SCALE-OUT IS `tools/learn-student/drill_all.py`: 140 lines, 46.4 s of
  wall clock on 14 workers (616 worker-seconds), 370 verified prompts.** Numbers
  to quote: 151 species, 140 lines with a decision, 10 whose walk offers none,
  **1 that cannot be planned — MEW**, because `BaseStats` holds 150 records
  (`assert_table_length NUM_POKEMON - 1 ; discount Mew`) and MEW's live in
  another bank. 432 decisions planned, 370 drilled, **236 of the published 581
  distinct (held, offered) pairs**, 268 of the 840, 98 species, 1,360 corpus
  rows. The 581/840 are over every *catchable* starting state; `--drill-plan`
  walks one line per species from level 1, which is why it is a subset.
- **58 DECISIONS WERE LOST AND EVERY CAUSE IS THE DRILL, NOT THE RIG.** 14 lines
  where **the level-up never happened** — the drill wins its battle by pressing
  A and A takes **move slot 1**, so a line whose slot 1 is SPLASH/HARDEN/SING
  cannot kill the level-2 MAGIKARP and one whose slot 1 is TELEPORT *flees*;
  9 where the move to forget was not in the list; 5 where the NO at the delete
  box did not take; 1 where the lead fainted. Choosing a damaging slot would fix
  about half of the first group; the rest hold no damaging move at all.
- **THE DRILL STAGES THE CARTRIDGE PER PROCESS NOW** (`out/stage-<pid>/`). It
  used to be a fixed `target/gen1-learn-drill/`, and the drill *deletes* the
  `.sav` there on the way in so every line starts from a new game — so two
  drills sharing it each delete the other's save file. Harmless only because the
  drill never reads one. **`--prompts` into a shared directory is still not
  parallel-safe** (the numbering reads the directory), so `drill_all.py` gives
  each process its own and merges.
- **A TEACHER THAT REPLAYS BY SCREEN MUST KEY ON THE EXACT TEXT, AND ON THE
  PARTY'S LEVEL.** The `Delete an older move…?` box shows the offered move and
  the party's HP and **not what is held**, so digit-stripping the key made two
  prompts on one line that offer the same move collide: 18 ambiguous keys of 619
  against 4 of 895 exact, and the teacher scored 88 of 97 instead of 97 of 97.
  The remaining four are **character-for-character identical screens with
  different right answers** — the ceiling on any screen-only player, teacher
  included, and a fact about the cartridge rather than the tool.
- **MATCH A PROMPT PHRASE AS IT LOOKS WHILE IT IS STILL BEING TYPED.** Gen 1
  prints a box a character at a time, so waiting for `which move should be
  forgotten?` and being handed `Which move` is not a near miss — it fell through
  to "this is not the question", pressed A to read on, and forgot whichever move
  the cursor was sitting on. Match `which move`, `make room`, `abandon learn`.
  And **a menu with something highlighted is a question whatever its words**, so
  never `advance_text` on one: A on a list is a choice.

### THE PASS RATE IS BLIND TO THE FIGHT UNDER IT — found on a BATTLE run, which is not the deliverable

- Link 05's goal is that the rival battle **happened**: `HandlePlayerBlackOut`
  special-cases it (`cp OPP_RIVAL1` … `cp OAKS_LAB / ret z`) and
  `OaksLabRivalEndBattleScript` sets the flag whoever won. So the expert passes
  **300/300 with every starter** while *losing* that battle at very different
  rates. A rig reporting only the goal would have said the three starters were
  identical. Hence `abrig::Outcome`: goal **and** battle-won, each with its own
  rate, interval and paired gap. **`Outcome::BattleWon` is `None` for an attempt
  that never fought** — "did not fight" is not "lost" — and those attempts are
  dropped from the pairing rather than counted as failures.
- The battle column **independently reproduces a figure this project already
  had** (900 certified exits: Squirtle 14.7 %, Charmander 23.0 %, Bulbasaur
  54.0 % lost, because Bulbasaur draws Charmander, faster at every level).
  Two independent measurements agreeing is the check; disagreeing would have
  been the finding.

### `RunResult::battles` is HOST-side, and that is what makes any arm scoreable

- **The battle numbers this project published came from `ChainDriver`'s own
  bookkeeping, which only the scripted expert can produce** — so a comparison
  against anything else had no battle column at all. `scenario::run::Battles`
  already observed win/fled/lost host-side for the referee; it now also carries
  wild/trainer, frames in battle and the lead's HP at each ending, and is
  exposed as `RunResult::battles`. Read on the run's own sampling clock through
  named watches, so every player produces it by the same work.
- Driver-side bookkeeping keeps its place — it can see *why* a move was chosen,
  which nothing outside the driver can — but **only host-side numbers are
  comparable between players**.
- **`wIsInBattle` is a RAW BYTE and `$FF` means blacked out, not fighting.**
  Third time this repository has been bitten by a catch-all over that byte (the
  first was `plugins/trainer/src/encounter.rs`, which drew a whole stale battle
  panel in the overworld). Pinned by `a_blackout_sentinel_is_not_a_battle`.
  **This byte also feeds the REFEREE** (`Observed::battles_won`/`battles_fled`,
  and so the `battle flee|fight` rules), so the fix was checked to be
  verdict-neutral rather than assumed to be: `$FF` is written inside
  `.allPokemonFainted`, which runs with the party already at zero HP, so a
  sample landing on it ends the battle as `lost` under both the old `!= 0` test
  and the new one — the old test merely deferred the same verdict by a sample.
  The empirical check is the host-versus-driver agreement below, which is exact
  on a link whose battles are trainer battles.
- New named watch `lead_species` (`tr::PARTY_MONS`, the **internal** index).
  Verified against the cartridge rather than recalled: 176 CHARMANDER, 153
  BULBASAUR, 177 SQUIRTLE, and `RomTables::species_name` is the only thing that
  turns one into a name — no species table is written down anywhere here.

### A PLAYER THAT LOOKS AT THE SCREEN MUST BE GIVEN ONE

- **The snapshot has always been able to carry the frame** (`Config::frame_capture`
  → `frame_png`), and `pewter_chain` never set it *and* switched pixels off via
  `ScenarioRun::collecting`. A machine that is not drawing hands a screen-only
  policy whatever was last in the buffer, which is not a failure any rate would
  explain. The pixel skip is now conditional on nobody wanting the picture and
  **the arm that wants it pays for it**, per worker so two threads cannot
  overwrite each other's frame. Verified end to end: a subprocess arm receives a
  real 160×144, 4-shade, 130-distinct-byte PNG of the live frame.
- **That is the piece TerminalGB lacked for the student**, and it is why the
  `command` arm is the whole integration surface — the student must not be
  copied into this repository.

### WHERE THE RIG BELONGS — AgentGB owns the student, and the two harnesses differ

- **The trained network lives in AgentGB** (`/home/aaron/Github/firstmate/projects/agentgb`,
  `agentgb evaluate`, `src/agentgb/convnet.py`), which already depends on
  TerminalGB as its environment — it drives `terminalgb-mcp` over JSON-RPC
  (`src/agentgb/mcp.py`, a deliberate *copy* of `tools/agent-docs/gb_env.py`'s
  shape so AgentGB needs no TerminalGB checkout) — and already has the shape of
  this comparison in `src/agentgb/baselines.py`: teacher, random, open-loop and
  student on the same starts.
- **The difficulty is that they are two harnesses.** AgentGB's `LinkEnv` presses
  buttons over MCP with its own settle frames, press timing and goal judgement;
  `ScenarioRun` has its own sampling clock, referee and budget. So one of them
  must be *the* harness. Two workable arrangements: **(1) the student comes here
  as a `command` arm** — landed, and needs nothing further from either side; or
  **(2) the teacher goes there**, which needs the piece TerminalGB still owes:
  AgentGB's own teacher is a *navigator* (`src/agentgb/expert.py`, shortest path
  over a walked graph) and its `battle.py` only *detects* battles, so it has no
  move-choosing policy at all. **TerminalGB owns the battle brain
  (`plugins/agent/src/battle.rs`) and it is reachable only from a Rust plugin
  inside the emulator.** Making it callable from outside — "what would the expert
  do in this battle?" — is that owed piece, and it is also what AgentGB needs to
  label battle states for a DAgger round.

### The side-by-side cut (`pewter_chain abfilm`)

- **The swarm compositor is deliberately NOT reused.** `examples/swarm_view.rs`
  draws agents on the world atlas from position tracks, which is right for six
  hundred agents crossing Kanto and wrong for a battle: a battle has no map
  position, so every agent is a motionless dot. A fight needs the framebuffer.
- Each arm plays **one** attempt at the same seed from the same entry state with
  pixels on, streaming to a raw RGBA file through the existing
  `ScenarioRun::filming` sink at `Pace::Emulated` (so the cut is a function of
  the run, not of how busy the box was); the panes are composed left to right at
  integer zoom with `video::card::Canvas` for the captions. It reads instantly:
  in the first cut the teacher had finished, `mash-a` was standing in the lab
  and `random` had stumbled into the battle — a judgement no table supports.
- **The stride is derived from the emulated time to cover and ROUNDED UP**, and
  the ratio is printed. The arms differ in length by an order of magnitude here
  (6,558 frames against 79,669), so a fixed stride gives a five-minute film of
  one arm flailing, and flooring loses the ending.
- **A short arm is held on its last frame, never blanked** — they do not finish
  together, and an emptying panel reads as a crash rather than as "finished
  first", which is the opposite of what happened.
- **Captions shorten through complete forms and are never clipped** (`fit_caption`,
  the `fit_bar` rule) — the first cut printed `250 DECISIO`, and a truncated
  measurement is worse than none because a reader cannot tell it was cut.

### THE HOST-SIDE SUMMARY REPRODUCES THE DRIVER-SIDE TALLY EXACTLY — check it, do not assume it

On the same 60 seeds of link 05, `ab` reports **60 battles, 42 won, 18 lost,
28.9 % HP left** and `corpus`'s `ChainDriver` tally reports *"battles 60: won 42
(70.0 %), lost 18 (30.0 %) … 28.9 % of max"* — **identical on every field**.
That agreement is the whole warrant for the new observer: it measures the same
thing the project already trusts, while being available to a player that is not
the expert. **Re-run that comparison after touching `Battles`** — it is one
`ab` and one `corpus` at the same `--runs`, `--seed0` and `--idle-spread`, and
it is the cheapest possible check that the two have not drifted.

### THE PUBLISHED CHARMANDER LOSS RATE NO LONGER DESCRIBES THE EXPERT

`docs/agent/pewter-chain.md` records link 05's loss rates over 900 certified
exits as SQUIRTLE 14.7 %, CHARMANDER 23.0 %, BULBASAUR 54.0 %. Measured through
the rig on 300 attempts each: **14.0 %, 5.7 %, 48.3 %** — two agree closely and
**Charmander does not, by seventeen points**.

- **The cause is measured, not guessed: the expert fights differently now.** A
  link-05 corpus today spends **49 stat-lowering turns across 60 battles**, i.e.
  roughly one turn in eight on GROWL. That rule landed with the shared battle
  policy *after* the certification the published table came from, along with the
  crit-aware faint rule and the accuracy-stage scaling. Lowering the opponent's
  Attack helps a glass cannon most, and Charmander is the glass cannon — 39 HP
  and Defense 43 against Squirtle's Attack 48, with Speed 65 against 43 buying
  the extra turns to spend it.
- **Settling it is a RE-CERTIFICATION of link 05 and belongs to the chain crew**,
  under that page's own standing rule that changing shared driver code
  invalidates every link already certified. What the rig establishes is that the
  published figure is stale and that the two rates which still agree are evidence
  the disagreement is real rather than a harness difference.
- **The general lesson, and this project keeps re-learning it:** a measured
  number is true of the code that produced it. Two independent measurements
  agreeing is the check; when they disagree, that *is* the finding, and taking
  the flattering one would have shipped a stale figure as a current one.

### Running it, and two things that cost time

- Entry states come from `corpus --exits N` chained through the earlier links:
  **700 exits through links 01-04 in 2 m 27 s** at ten workers, 700/700 each,
  splitting 234/233/233 by starter because link 04's ball is `6 + seed % 3`.
- **Reading a lead species out of a state costs one machine and no emulated
  frames** — the state carries the party. That is the whole of "a different
  starter is a flag rather than a rewrite".
- **A subprocess arm is the expensive one by a wide margin**: one process per
  decision times a 250-call budget times the attempts. On this box the two
  screen-blind arms are most of the wall clock while the expert finishes in ~66
  decisions. Budget for it, and do not read a slow cell as a bug.
- **USE `pgrep -a`, NOT `ps | grep`, TO ASK WHETHER A BACKGROUND RUN IS ALIVE
  HERE.** `ps -eo pid,args | grep pewter` returned *nothing* for a process that
  `pgrep -a` listed seconds later, twice, which read as "the run died" — and
  acting on it cost a healthy 25-minute run that was killed and restarted for no
  reason. The standing `pkill -f`/`pgrep -f` warning is about matching your own
  shell; this is the opposite failure and it is worse, because a false negative
  makes you destroy something.
- **`VAR=value cmd "$VAR"` PASSES THE OLD VALUE.** A prefix assignment applies to
  the command's environment, not to the expansion of its own arguments, so
  `ROM=... prog --rom "$ROM"` launched with an **empty** ROM path and died
  instantly with `cannot read : No such file or directory`. `export` on its own
  line, then launch.
- **`--out` defaults under `target/`, and the film's raw streams are large**
  (160×144×4 per frame, ~4.6 MB per thousand frames per arm). Nothing here is
  committed; the plan file and the report are.

## Catching — the gate, the discriminator, and the ball (`plugins/agent/src/catch.rs`)

**Full page with every measurement and the commands: [`docs/gen1/catching.md`](docs/gen1/catching.md).**
Instrument: `examples/gen1_catch_drill.rs` + its `.preset`. Modules:
`plugins/agent/src/catch.rs` (the policy), `plugins/agent/src/battlekind.rs` (wild-vs-trainer
from the screen), `plugins/trainer/src/capture.rs` (the odds, extended with `ball_for_item`).
Query: `tools/gen1-tables/gen1_tables.py --plan|--gyms|--marts`. **Not in CI** — it needs a
commercial cartridge, the `tools/sameboy-oracle/` rule; the 18 unit tests across `catch.rs` and
`battlekind.rs` need none and run in a plain `cargo test`.

- **CATCHING IS A CAPABILITY THAT STARTS AT ZERO, AND THE PEWTER CHAIN NEVER OPENS IT.**
  Measured on the chain's own carried-forward states: **the bag is empty at every one of links
  01 to 10**, and `EVENT_GOT_POKEBALLS_FROM_OAK` (`$D74B` bit 4, the bit below link 09's own
  `EVENT_GOT_POKEDEX`) is still clear at 09 and 10. So the honest answer to "which link opens
  it" is **none of them**. Two doors, both read out of the cartridge and both one interaction
  away: Oak's scripted gift of five (`07:52D9` — `ld hl,$D74B / bit 4,[hl] / set 4,[hl] /
  jr nz / ld bc,POKE_BALL:5 / call GiveItem`, text at `0x9506D`), and a **purchase** — Viridian
  is the first mart list (`POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL`, marts at `0x02442`),
  `ItemPrices` (`0x4608`) says ¥200, and the chain already walks into that shop at link 07 with
  ¥3175. Opening it belongs with whoever owns `examples/pewter_chain.rs`.
- **`Verdict::Unavailable` is NOT `Verdict::No`, and that distinction is the captain's
  constraint made checkable.** A policy with no ball has weighed nothing. Measured: 40
  encounters with an empty bag give **40 `unavailable`, 0 decisions**; the same 40 with balls
  give 40 decisions. It also means a student trained on these runs never sees a catch attempt
  before the capability exists, so it cannot learn to try one.
- **A FINDING FOR THE CHAIN CREW: link 09's exit state is captured INSIDE the cutscene.** Its
  goal is the flag and `OaksLabOakGivesPokedexScript` sets the flag before handing control back,
  so the state has **`wJoyIgnore = $FF`** and discards every button. A probe that starts pressing
  there measures nothing and reports its negative result with total confidence — which is what
  the first version of the gift probe did. Link 10's solver survives it by running frames until
  control returns. **Wait for `wJoyIgnore` ($CD6B) to clear before driving any state.**
- **THE SCREEN ONLY SAYS WILD-OR-TRAINER ONCE, AND THE MEASUREMENT IS THE POINT.** Traced frame
  by frame over a matched pair (one save, one challenger, only `wild =` vs `trainer =`
  different): **no cell differs on all 1200 paired frames**; the fewest that differ on any is
  **2**, and they are the level digits. The `Wild <NAME> appeared!` line is on screen for **9
  frames** and `<NAME> wants to fight!` for **11**. So at the battle menu — the moment a policy
  decides — the two are the same picture. `battlekind::Watcher` is therefore a **latch**: read
  the first line, remember it, `reset()` when the battle ends. Scored at the menu over 50
  battles of each kind: **latched 0/100 wrong, stateless 100/100 unknown**.
- **THE SAMPLING RATE IS PART OF THE DISCRIMINATOR.** Measured as the fraction of sampling
  *phases* that never land in the window: every interval **up to 8 frames catches it from every
  phase**; at 12 frames 3 of 12 phases miss the wild line, at 24 frames 15 of 24 do. The agent
  plugin samples every 6 frames, so it is inside — but only just, and anything that slows to a
  12-frame cadence starts losing battles silently.
- **A BALL THROWN IN A TRAINER BATTLE IS REFUSED AND KEPT — a free safety net.** `ItemUseBall`
  branches to `ThrowBallAtTrainerMon`, prints `TrainerBlockedText` ("The trainer blocked the
  BALL! / Don't be a thief!", `0xA6B0B`) and returns without touching the bag. Measured: **64
  throws in trainer battles, 64 blocked, 64 balls kept, 0 spent.** So a wrong discriminator
  costs a *turn*, never a ball — which is why declining an *uncertain* screen is cheap and is
  the default.
- **The closed form in `capture.rs` agrees with the cartridge: 251 throws, 61.0 % caught against
  59.8 % predicted, z = +0.36** (1 s.e. = 3.09 %), swept across HP, status and three balls.
  Report the per-bucket curve beside the aggregate — an aggregate can be right on average while
  being wrong everywhere — but quote the aggregate, because the buckets are small-n by design.
- **WHICH BALL IS A DERIVATION, NOT A TUNED MARGIN.** A dearer ball's advantage is bounded: the
  resampling loop is worth at most 1.27x (Great) and 1.70x (Ultra), and the Great Ball's smaller
  HP divisor at most a further 1.5x — against price ratios of **3x and 6x**. So **a Poké Ball
  always wins on chance per ¥**, swept over 735 states by
  `a_poke_ball_is_always_the_better_value`. The dearer balls are reached for only when even the
  cheap one is under a third, i.e. when a *turn* rather than ¥400 is the scarce thing. A first
  version used a "within 90 % of the best chance" margin and measured as spending a Great Ball
  on nineteen of twenty Pikachu; three times the price for 27 % more chance is a bad trade
  nobody had written down as one.
- **THE TARGET LIST IS DERIVED FROM THE FIGHTS AHEAD AND CHECKS THE MOVES, NOT ONLY THE TYPE.**
  `gen1_tables.py --plan brock,misty` reads the leaders' own parties, their types, what beats
  them, the candidates' **starting moves** (`BaseStats` offset 15 holds `db MOVE1..MOVE4`) and
  where they live. It reaches **PIKACHU, Viridian Forest, levels 3-5, 2/10 grass slots, catch
  rate 190, arrives with THUNDERSHOCK** without being told. The move check is what separates it
  from a type query: a level-6 SANDSHREW is GROUND and will beat Brock eventually, but it
  arrives with SCRATCH.
- **NO STARTER ARRIVES WITH A MOVE THAT BEATS ANYTHING ON THAT LIST** (Bulbasaur TACKLE/GROWL,
  Charmander SCRATCH/GROWL, Squirtle TACKLE/TAIL WHIP), so the target list is **the same for all
  three starters** through Brock and Misty and the per-starter difference is urgency, not
  contents. `--plan` prints "with the moves it has" and "by its own type, once it learns one"
  **separately**, because conflating them says a level-5 Bulbasaur beats Brock — it does, with
  VINE WHIP, at level 13.
- **`TrainerDataPointers` MUST BE ANCHORED ON REAL PARTY DATA AND ITS ROSTERS BOUNDED BY THE NEXT
  CLASS'S POINTER.** A scan for "a long run of in-bank little-endian words" finds a *different*
  table in this cartridge and answers with LT.SURGE fielding a level-1 RHYHORN; and asking for a
  roster a class does not have walks into the next class's data — the first version reported
  BROCK #2 as MISTY #1. Anchor on `BugCatcherData`'s three forest rosters (level 6
  WEEDLE/CATERPIE, 7 WEEDLE/KAKUNA/WEEDLE, 9 WEEDLE), which is the table's **second** entry, and
  the answer is `0x39D3B` — Brock is GEODUDE Lv12 + ONIX Lv14, Misty STARYU Lv18 + STARMIE Lv21.
- **`TrainerNames` HAS TWO CANDIDATE RUNS IN THIS CARTRIDGE** and the shorter one is missing
  SAILOR, HIKER, BIKER and every gym leader. Carry SAILOR in the signature; only the real table
  (`0x399FF`, 47 classes) has it.
- **Rotation (`Action::Switch`) is deliberately NOT built.** The chain carries one Pokémon and
  the bag is empty, so a switch rule could never fire and could not be tested. It waits for a
  bench, which waits for the gate above.
- **Two things the "tens of thousands of synthesised battles" task will want from here**: the
  preset's **matched pairs** (a discriminator scored on wild battles from one save and trainer
  battles from another is measuring the saves), and `Moves`/`starting_moves` (a synthesised
  opponent is only interesting if you know what it can do at the level it is generated at).

## Learning moves is a CLOSED TABLE, and so is evolving (`tools/gen1-tables/learn_table.py`)

**Full pages: [`docs/gen1/learning-moves.md`](docs/gen1/learning-moves.md) (the enumeration, the
rule, the prompts) and [`docs/gen1/battling.md`](docs/gen1/battling.md) (the map of the subject,
and what a battle screen shows).** Instruments: `learn_table.py` and
`examples/gen1_learn_drill.rs`. **Not in CI** — both need the player's own cartridge, the
`tools/sameboy-oracle/` rule. The drill starts a NEW GAME and needs no save.

- **THE WHOLE ARGUMENT IS THAT THE QUESTION IS FINITE. Measure it, never estimate it.** 151
  species, every level-up learnset fixed data: **728 level-up learn entries**, learns per species
  `0:12 1:3 2:4 3:7 4:11 5:51 6:49 7:9 8:5` (at most 8), **72 evolutions = 52 level + 16 stone +
  4 trade**. Two crews parsed it independently and agreed to the digit; Gen 1 having exactly 4
  trade and 16 stone evolutions is the check that falls out for free.
- **`EvosMovesPointerTable` IS AT `0x03B05C` (bank `$0E`) AND HAS 190 SLOTS INDEXED BY INTERNAL
  INDEX, NOT 151.** Reading the first 151 gives a mix of real species and MissingNo and
  undercounts; `PokedexOrder` (`0x41024`) is what filters it. Located by property — the only run
  of 190 in-bank pointers whose BULBASAUR entry dereferences to `01 10 09` — never by a
  remembered address.
- **728 IS THE CEILING ON *EVENTS*, NOT ON DECISIONS: the first four slots fill for free.** The
  real number needs the starting move set, and Gen 1 is unusually kind there — **a caught
  Pokémon's four moves are a function of its species and its level and of nothing else**
  (`WriteMonMoves`). Verified against the cartridge on three species (TENTACOOL@40, NIDORINO@40,
  PIDGEOTTO@30, all exact). So the starting states are enumerable: 151 x 100 levels x every
  stone/trade branch = 17,100 trajectories walked to level 100 → **840 DISTINCT DECISIONS**
  (species, the four held, the offer), 236 on the from-level-1 walk of the 79 base forms.
- **THE RULE IS OURS AND IT IS STATED SO IT CAN BE ARGUED WITH** (`learn_table.py`, top): rule 0
  protect the moves the table cannot price, rule 1 never forget the last damaging move, rule 2
  damage outranks status, rule 3 order by `power x accuracy/255 x STAB`, rule 4 status moves are
  NOT ranked against each other. Answers over the 840: refuse 209, status-victim 573,
  damage-victim 58, **coverage-loss 11 (1.3 %)**, status-tie 551, unpriced-present 85, no-answer 0.
- **RULE 0 FELL OUT OF THE CARTRIDGE AND IS THE ONE THAT MATTERS.** Grouping every move whose
  power byte is 0 or 1 by its effect byte gives **effect 38 = OHKO (GUILLOTINE/HORN DRILL/
  FISSURE), 40 = SUPER FANG, 41 = fixed damage (SONICBOOM/SEISMIC TOSS/DRAGON RAGE/NIGHT SHADE/
  PSYWAVE)** — seven moves whose power byte is a placeholder. Without the clause the rule has
  PRIMEAPE forgetting SEISMIC TOSS for a POUND. **Never price a Gen 1 move on `power` without
  checking its effect first.**
- **THE PARTY-CONTEXT QUESTION IS MEASURED: 11 of 840 = 1.3 %.** Those are the decisions where
  the rule drops a damaging move that was the holder's last of its type, so a second Pokémon
  covering it, or the route ahead, could reasonably change the answer. On the other 98.7 % the
  party cannot.
- **AND THE STUDENT NEEDS NOTHING THE PROMPT DOES NOT SHOW IT.** 840 decisions collapse to **581
  distinct (four held, offered) pairs, and the number whose answer differs by species is ZERO.**
  The move menu shows four **names** and no power, type, accuracy or PP; the offered move's name
  is in the box two screens earlier; the species is never on screen (the box uses the nickname).
  So the perception problem is recognition, not reasoning.
- **NEVER CANCEL AN EVOLUTION, and the cost of being wrong is measured.** Across all 52 level
  evolutions, **0 lose a move** the evolved form never learns and **44 delay one**, longest 11
  levels (SOLARBEAM 54→65, IVYSAUR→VENUSAUR). And a cancelled evolution is **re-offered at the
  very next level-up** — measured: B pulsed at 16 prints `Huh? LEARNER stopped evolving!` and it
  stays a BULBASAUR; one more level and it becomes an IVYSAUR at 17. Whole cost: one level of the
  evolved form's stats.
- **B HAS TO BE *PULSED* THROUGH THE EVOLUTION ANIMATION — not tapped, not held.** Gen 1 reads
  some buttons through a routine that only sees a transition. Two versions of the drill pressed B
  and reported "cancelling does not work"; the Pokémon evolved both times.
- **A BATTLE THAT CROSSES TWO LEVELS LOSES THE MOVE IN BETWEEN.** One level-up message per battle
  and one learnset check, on the level it *ended* on. A level 6 BULBASAUR fed one CHANSEY lands
  on level 8 and level 7's LEECH SEED is never offered — which reads exactly like the prompt not
  firing, and is what made the first drill report nothing.
- **The two modal interruptions are NOT confusable, and every one is easy to sample.** Evolution:
  `What? X is evolving!` 599 frames, `X evolved into Y!` 245. Move-learn: six boxes of 23-158
  frames. No phrase is shared, and **every sampling interval up to 24 frames catches every prompt
  from every phase** — far more forgiving than catching's 9-frame wild/trainer line.
  **When both land on the same level (4 places in the cartridge), the EVOLUTION runs first and
  the evolved form's move for that level is then offered** — measured on ABRA→KADABRA/CONFUSION.
- **The prompt never arises before Pewter.** First four-move prompt raising from level 1:
  BULBASAUR 22, CHARMANDER 24, SQUIRTLE 24, CATERPIE 16 (the earliest on the route). The chain's
  training leg targets `lead_level>=10`.
- **`BattlePlugin` RESTORES THE PARTY WHEN THE BATTLE ENDS, so experience cannot accumulate
  across battles from a `player =` preset.** The drill snapshots the party mid-battle and writes
  it back — and **snapshots on "there is a party", not "we are in a battle"**, because the
  evolution and the move-learn happen after `wIsInBattle` has gone back to zero and a
  battle-gated snapshot puts the pre-evolution form back. It also tops PP up on the same pass: a
  drill that fights a dozen battles on one HYPER BEAM runs out of its 5 PP and parks the battle
  menu for ever. Later rounds arm an **opponent-only** preset (no `player =` line ⇒ nothing to
  restore).
- **`text::decode_screen` returns ONE FLAT 360-CHARACTER STRING**, so a caller that wants rows
  has to cut the tiles up 20 at a time; and a Gen 1 sentence wraps across rows padded with
  spaces, so a phrase is only contiguous after collapsing whitespace. Both cost this drill a run.

### The drill proper — play the level-up out, and record it (`gen1_learn_drill drill`)

**Full page: [`docs/gen1/level-up-drill.md`](docs/gen1/level-up-drill.md).** The rule above,
carried out on the cartridge one level-up at a time, with the corpus a student trains from as its
output. `learn_table.py --drill-plan <species>` emits the plan;
`gen1_learn_drill drill --plan <p.json>` plays it. **Not in CI** (it needs the player's own
cartridge), ~6.4 s a line, starts a NEW GAME so it needs no save.

- **Measured on retail Blue, both lines 7 of 7 events verified against the cartridge:**
  **CHARMANDER → CHARMELEON → CHARIZARD** 7 events (2 free, 1 already-held, **4 decisions**, 1 of
  them a decline), 16 corpus rows; **BULBASAUR → IVYSAUR → VENUSAUR** 7 events (2 free, **5
  decisions**, 2 declines), 17 rows. The three forms of each line hold **18** and **21** learnset
  entries between them — the walked count and the table count are different questions and the
  page states both.
- **THE DRILL DECIDES NOTHING.** The criterion is `learn_table.py`'s `decide()` and the plan
  arrives with the answer already in it. Same split as the battle policy, same reason: two
  implementations of one rule disagree the moment either is improved, silently.
- **TRIGGER A LEVEL-UP BY WRITING THE EXPERIENCE, NOT BY PICKING AN OPPONENT.**
  `wPartyMon1Exp` (`$D179`, three bytes big-endian, offset 14 into `party_struct`) set to
  `romdata::experience_for_level(growth, level) - 1` — *the same function the preset used* — then
  one **level 2 MAGIKARP** (5 experience, SPLASH only). That makes the arithmetic identical for
  an event at level 9 and one at level 65, instead of the per-event CHANSEY tuning the `trace`
  scenarios need to dodge the two-levels-in-one-battle trap.
- **A TAP SHORTER THAN `MENU_GAP` IS SWALLOWED, AND ON A MOVE LIST THAT FORGETS THE WRONG MOVE
  WITH NO COMPLAINT.** Measured: three 12-frame taps moved the cursor **two** slots, so RAGE
  replaced EMBER where the rule chose LEER and the screen looked perfect throughout. **Read the
  cursor back (`Screen::selected()`) and press Down only while it is not on the target** — never
  count presses. The party read-back is what caught it; nothing else could have.
- **A YES/NO BOX SCROLLS ITS OWN QUESTION OFF THE SCREEN.** `Delete an older move to make room
  for FIRE SPIN?` is three lines and Gen 1 holds two, so when the box is finally up the only
  words left are `move to make room`. Matching the opening words made the box invisible, the
  drill fell through to "press A", **and A there means YES** — a decline silently became a
  replacement. General rule: **a phrase that identifies a screen must still be on it when the
  screen asks you something.**
- **THE PARTY-SNAPSHOT RULE ABOVE IS NOT ENOUGH FOR A MULTI-EVENT RUN.** "Snapshot on there is a
  party" captures the party `BattlePlugin` **restored** — measured, every event after the first
  read back the previous event's state. Keep the **best-scoring** snapshot instead, scored on the
  three things the verdict checks (level, species, the four moves), sampled **every frame**
  rather than once per settle.
- **AN EVOLUTION CANNOT BE VERIFIED THROUGH AN INJECTED PARTY.** `step_fighting` restores the
  moment `wIsInBattle` reads 0, and that lands **between the evolution's animation and its
  commit**: on CHARMELEON at 36 the screen prints `LEARNER evolved into CHARIZARD!`,
  `wPartyMon1Species` reads 178 on every frame to the restore, and with the drill's write-back
  disabled the party ends **empty**. Reported as `ok (form unchecked)` and counted, never papered
  over. It costs nothing here: **no decision on either line falls on an evolution level.**
- **The corpus is the emulator's own agent NDJSON** — observation in `request`, the teacher's plan
  in `reply` — so `tools/agent-docs/log_to_dataset.py` flattens it unmodified and a `command` arm
  of the A/B rig speaks the same protocol. **No new format, and no second comparison harness.**
  The `label` object beside the observation is ground truth and is *not* what a player sees; a
  pipeline wanting a strictly observable input drops it.
- **`GB_LEARN_DRILL_DUMP=1` prints the decoded screen, the cursor, what it points at and the live
  party at every settle.** All four findings above came out of it; reach for it first when a
  prompt is not being answered the way you expect. `--level N` runs one event alone.
- **NOT DONE, and named on the page:** Magikarp (parked by the captain), TMs, refusing an
  evolution, and **no model has been through this corpus**.

### Four more cartridge tables, and the route claim they settled

- **`TechnicalMachines`, the TM/HM compatibility bitfield (`BaseStats` offset 20, 7 bytes),
  `ItemNames` and a map's item balls** are all in `gen1_tables.py` now: `--tms`, `--can-learn
  <species>`, `--items-on <maps>`. Map items come through `MapHeaderPointers` (`$01AE`) +
  `MapHeaderBanks` (`0x00C23D`), the same walk `tools/pewter-chain/route_audit.py` established.
- **A ROUTE PLAN CHECKED AGAINST THE CARTRIDGE, AND ONE QUARTER OF IT WAS FALSE.** *"Magikarp
  near Mt Moon, Gyarados at 20, BubbleBeam TM there."* Magikarp→Gyarados at level 20: **true**
  (`01 14 16`). Gyarados can learn TM11: **true** (23 machines). A BubbleBeam TM in Mt Moon:
  **FALSE** — TM11 lies on **no map** and is in **no mart**; Mt Moon 1F (`$3B`) holds **TM12
  WATER GUN** (plus a RARE CANDY and a MOON STONE) and B2F (`$3D`) holds TM01 MEGA PUNCH. TM11's
  own text sits beside the CASCADEBADGE line, i.e. it is Misty's gift, after Mt Moon.
- **MAGIKARP IS IN NO WILD ENCOUNTER SLOT ANYWHERE** — Gen 1 keeps fishing in a separate table.
  The Old Rod's routine is `ld bc, $0585` at `0x00E252` (bank 3): level 5, MAGIKARP,
  unconditionally. And the cartridge sells one for ¥500 (text at `0x08A977`). **Which map sells
  it, and where the Old Rod comes from, were NOT established.**
- **And the plan's real cost is a number nobody had computed: MAGIKARP is on the SLOW curve, so
  level 5→20 is 9,844 experience** — about 128 level-10 Zubat — **and it can earn none of it by
  fighting**: SPLASH (power 0) until TACKLE at 15, and **it can be taught none of the 55
  machines**. All of it has to come from switching it into battles something else wins.

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
  reference image. It is **enforced now, not just written down**:
  `test_no_check_expects_a_reference_frame_produced_here` refuses any check that pins an image
  unless that image ships with its own ROM under a redistributable licence. A future check that
  quietly pinned one of our frames would go red rather than merely be regrettable.
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
  tester covers the screen capability in a short adapter and scores **4/5 unmodified**. It has
  **no serial output** — no flag, and the only serial hook in `emulator.c` is inside an `EM_ASM`,
  so it reaches the emscripten build and nothing else.
- **The serial half IS proven foreign now, by [Peanut-GB](https://github.com/deltabeard/Peanut-GB)**
  (single-header, MIT, Mahyar Koshkouei) — `gb_init_serial()` is in its public API.
  `tools/conformance/adapters/peanut-gb.c` is ~100 lines against its header, nothing patched:
  `git clone --depth 1 https://github.com/deltabeard/Peanut-GB /tmp/peanut && cc -O2 -I/tmp/peanut
  -o /tmp/peanut-gbconform tools/conformance/adapters/peanut-gb.c`. Scores **13/18** (11 of 12
  serial, dmg-acid2, and all four `screen-text` checks reporting the ROM's own `Failed`). It is a
  DMG-only emulator, so a CGB-pinned check measures a DMG running a CGB cartridge — a true
  statement about it. **Do not go back to saying the serial half is unproven.**
- **Neither foreign run is in CI** (each fetches and builds somebody else's repo — the same call
  `tools/bgb-interop/` makes). Re-run both by hand when the runner or an adapter changes.

### `screen-text` — reading a ROM's own printed verdict off the frame

Four Blargg ROMs never touch the link port. They are scored now, and the guiding principle above
is intact: `tools/conformance/screentext.py` decodes the **text**, so the verdict is still the
ROM's own `Passed`/`Failed #N`.

- **The character table comes from the ROM UNDER TEST, not from us and not from a shipped font.**
  Blargg's print routine loads a font whose 8x8 glyphs sit in the cartridge in ASCII order, so
  every glyph on screen is eight bytes already in the ROM we were handed. Cut the frame into 8x8
  ink masks, `rom.find` each one, and `offset // 8` is the character code up to one constant. The
  constant is pinned by keeping the alignment whose decoded **words appear verbatim in the ROM**
  (each ROM carries its own title plus `Passed`/`Failed` as plain ASCII). **Measured: all 19
  non-blank glyphs of `halt_bug` are found, uniquely, and every one lands on the same `offset % 8`
  class** — that shared alignment is the check that the block really is the font.
- **A wrong alignment decodes to noise, and noise is not in the ROM** — which is what makes this
  self-validating. When nothing scores, the check reports **error**, never a guessed verdict.
- **NO CONTRACT CHANGE, AND THAT WAS THE DESIGN GOAL.** `screen-text` is a new *method* riding the
  existing *screen capability* (`CAPABILITY_FOR_METHOD` in `gbconform.py` is where the two are
  kept apart). Every adapter with a screenshot flag gained four checks with no edit at all —
  binjgb went 1/1 → 4/5.
- **Two real frames needed two wrinkles, both handled and both unit-tested:** `oam_bug` prints a
  failing sub-test's code in **inverse video** (a cell that is the glyph's exact complement), and
  a console scrolled by a non-multiple of eight puts the cell grid off the frame's lattice (the
  sub-cell origin is searched 0..7 in each axis).
- **It reads a FOREIGN emulator's frames whose shade ramp is inverted relative to ours** — the
  decoder never looks at a colour, only at which cells carry ink. Proven on Peanut-GB, whose
  shade 0 is white.
- **The tests build a synthetic cartridge** (font + message + rendered frame) rather than needing
  a real ROM, because Blargg's are fetched at run time and never vendored. Teeth proven: removing
  the inverse-video retry reddens one case, taking the first alignment instead of the best-scoring
  one reddens five.

### `GBCONFORM_MODEL` is load-bearing, and BOTH adapters ignored it until a check needed it

- **`oam_bug` is pinned to `dmg` and `interrupt_time` to `cgb` in `checks.json`** (a per-check
  `model` overrides `--model`), because the answer differs by machine: the OAM-corruption defect
  is DMG/MGB/SGB silicon only, and `interrupt_time`'s cartridge header says `$0143 = $C0`.
- **`examples/conformance_adapter.rs` documented `GBCONFORM_MODEL` and ignored it**, calling
  `Cpu::new` (i.e. `Model::Auto`) — so `oam_bug` ran on a Color console and failed. Nothing caught
  it because no check had ever used the variable. `tools/conformance/adapters/binjgb.sh` had the
  mirror-image bug: a hard-coded `--force-dmg`, which cost it `interrupt_time`. **An adapter that
  hard-codes a console returns a wrong answer, not a missing one** — check that before believing a
  single-check failure.
- **`examples/conformance_adapter.rs` also built a machine with NO APU**, so `interrupt_time`
  (which spins on `NR52`) never left its title screen — the exact trap `testharness/testroms.rs`
  already records as "never build a test machine without an APU". `sound::SilentPlayer` is the fix.
  Both defects were invisible for as long as the suite avoided the ROMs that expose them.
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

## The link port over TCP — BGB's protocol (`src/bgblink.rs`, `src/link/transport.rs`)

**Full guide: [`docs/bgb-link.md`](docs/bgb-link.md); conformance record in
`docs/conformance-notes.md` (newest entry).** Behind the `bgblink` cargo feature — in
`default`, out of `--no-default-features`, out of wasm — and it adds **no third-party
crate**: the whole implementation is `std::net`. `make embed` still reports one
third-party dependency.

- **This is the third way to be the far end of the link port**, and the only one that
  reaches hardware you do not own. `plugins/gen1-link` impersonates a console;
  `LinkCable` relays between two local cores; this relays over a socket. Through
  [`Lorenzooone/PokemonGB_Online_Trades`](https://github.com/Lorenzooone/PokemonGB_Online_Trades)
  (**MIT**, used unchanged) the far end can be a **real Game Boy on somebody else's USB
  adapter**, because that tool's `emulator_trading.py` and `usb_trading.py` put the *same*
  trading logic behind two different byte channels. It costs no hardware purchase at all.
- **`LinkTransport` (`src/link/transport.rs`) is the seam; `TransportLink` is the driver.**
  A future USB-adapter transport implements the same trait. The trait lives in `link.rs`'s
  submodule and not in a frontend, because it is pure logic with no host dependency — the
  socket half is what is feature-gated.
- **THE CONSOLE IS STALLED, AND THAT IS THE WHOLE DESIGN.** A master's transfer finishes on
  its own shift clock in ~1 ms of emulated time whether or not anything answers, so a
  driver that lets the console run hands it `$FF` every time and no link ever carries a
  byte. `TransportLink::run_frame` stops paying cycles to the CPU until the answer arrives.
  That is the honest implementation of BGB's timestamps ("so each side can, at the right
  times, wait for the remote side") — and it is why the timestamp we report is derived from
  **emulated** T-cycles (2 T = 1 tick of its 2 MiHz clock), never the wall clock.
  Expect **one stall per transferred byte on whichever side drives the clock**; measured
  1526 stalls on the master and 0 on the slave in a real trade. That is not a fault.
- **Timestamps are carried and deliberately not enforced.** The shipping Python tool
  "cheats" the same way (its own comment: *"Cheat, and say we are exactly in sync with the
  client"*), paces at **10 ms per byte** from a Python thread, does not set `TCP_NODELAY`,
  and completes real trades. These games are turn-based; the request/response structure
  *is* the synchronisation. Do not over-engineer this.
- **An INCOMING byte is HELD, not refused.** Our console may not have armed `SC = $80` yet.
  Refusing immediately drops the byte — the first interop run lost the first two bytes of a
  twelve-byte payload exactly that way. `SLAVE_ARM_WINDOW` is two frames; refusals are
  counted as `missed`. **`missed` climbing while `transfers` stays flat is the signature of
  both ends settling into the same role**, and it is invisible without the counter. It is
  in the TUI's `Net` status row for that reason.
- **NEVER answer a status packet with a status packet.** The reference Python server does
  (its own comment admits the docs say not to), so an implementation that does the same
  ping-pongs for ever. Send one **in response to version** and nowhere else.
- **An unplugged cable is not a broken one, and only the second may fault the console.**
  `TransportError::Setup` (nothing listening, refused, version mismatch) costs the cable
  and nothing else; `TransportError::Broken` (live and then gone) faults the console, which
  poisons the battery — same reasoning as `LinkCable`'s fault propagation. `wantdisconnect`
  (packet 109) is the **only** thing on the wire that tells "the player quit" from "the
  emulator died"; we send one from the frontend's shutdown path, **never from `Drop`**.
- **Wiring: `link_net = [listen:|connect:]host:port`** in `config.toml` (or `GB_LINK_NET`),
  following the `link = …` precedent. Without a prefix the default is **connect**,
  because BGB's "Listen" and the Python bridge both bind first.
  **A Game Boy has one link port and THREE things now want it** — the auto-discovered
  local peer (`link = auto`, the default), a second console in this process
  (`link = <rom>`), and the socket. **`link_net` wins**, because it is the only one of
  the three a user types on purpose; whichever it displaces is reported in the warnings,
  not silently dropped (`only_one_thing_can_have_the_link_port`).
  A `listen:` link **binds and returns**; the console runs with an unplugged cable until
  somebody connects. Blocking in `accept()` would be the video sink's startup hang again.
- **Verified: a real Gen 1 trade over TCP.** Two `examples/bgb_link` processes, real
  sockets, Pokémon Blue on both: **1786 transfers, 0 unanswered, 0 missed, 0 timeouts, 22
  s** (1,550-1,800 depending on how the bots interleave), and each console ends holding a
  Pokémon whose OT is the *other* console's player. `--expect-ot` makes that the exit
  status. Run it with `--listen`/`--connect`, `--party red|blue`, `--trade`.
- **A TRAIT METHOD THAT IS ONLY AN INHERENT METHOD IS A SILENT NO-OP, AND THAT COST A
  REAL BUG HERE.** `BgbLink::disconnect` existed but was never added to
  `impl LinkTransport for BgbLink`; `TransportLink` calls it through `dyn LinkTransport`,
  got the trait's empty default, and **every clean shutdown was reported by the peer as a
  broken link — so the peer stopped writing its save**. The unit test passed the whole
  time because it called the inherent method on the concrete type. The test goes through
  `Box<dyn LinkTransport>` now (teeth proven). Two-process end-to-end found it; nothing
  else could.
- **`examples/bgb_link --settle N` (default 600) is not padding.** The two consoles do not
  finish together — Gen 1 runs another full three-block exchange after "Trade completed!"
  — so whichever finishes first must keep servicing the cable. Without it a run that had
  traded correctly reported a party entry read *mid-write* (`LIMMER`, OT `ED`, level 0).
- **Closing a socket with unread data in its receive queue sends RST, not FIN**, and an
  RST discards the goodbye you just wrote. `BgbLink::graceful_close` writes, drains,
  half-closes and then waits up to `LINGER` (250 ms) for the peer's FIN. Measured: without
  it the peer read `Connection reset by peer` and faulted.
- **Parse the buffer BEFORE acting on the hang-up that came with it.** A polite peer writes
  its `wantdisconnect` and closes, so both land in one `read(2)`; `close_as` keeps the
  *first* reason it is given, so recording the EOF as it is seen overwrote the goodbye and
  turned every clean shutdown into a broken link. `went_live` is latched where the phase
  becomes `Live`, not at the end of a poll, for the same class of reason.
- **Verified against a SECOND IMPLEMENTATION, which is the claim that matters.**
  `tools/bgb-interop/interop.py` imports `PokemonGB_Online_Trades`'
  `utilities/bgb_link_cable_server.py` **unchanged, as a library** (pinned commit in
  `tools/bgb-interop/fetch.sh`, fetched at run time, never vendored) and drives it as
  master. Both verdicts are independent because the emulated cartridge answers with the
  **complement** of what arrived — a plain echo cannot tell a working link from a peer
  reading its own transmission back. **Teeth proven**: making `answer_incoming` send `$00`
  turns it red. **Not in CI** (fetches somebody else's repo, needs Python); run it by hand
  when `src/bgblink.rs` changes.
- **The counterparty sends NO DATA until it has seen one sync3 `b2 = 1` from us** (its
  `can_go` flag) — i.e. until it has caught us *not* armed once. Every real cartridge
  satisfies that by booting before it touches the link port; a test ROM that arms on its
  first instruction never would, and the link comes up and then sits there. Hence the
  deliberate boot delay in `examples/bgb_link`'s `echo_rom()`. **An interoperability
  harness has to look like a cartridge, not like a protocol.**
- **Not tested, and say so:** BGB itself (a Windows program nobody here has run) and a real
  Game Boy (no adapter on this machine). The hardware path is established *by
  construction* — `emulator_trading.py` and `usb_trading.py` bind the same `RBYTrading`
  logic to different `sendByte`/`receiveByte` pairs — and that is an inference, not a
  measurement. Do not let it be quoted as one.
- **Dead ends, recorded so nobody repeats them:** (a) sending the goodbye and dropping the
  socket — see the RST note above; (b) recording the hang-up before parsing the buffer that
  came with it; (c) the inherent-versus-trait `disconnect`; (d) starting the interop payload
  as soon as the counterparty is ready, which loses the first bytes while the console is
  still booting — the fix is a **sentinel probe, not a `sleep`** (probe `0x00` until the
  echo ROM's pre-arm `$A5` comes back, which is also why `0x5A` is absent from the payload:
  the ROM answers with the complement, so it can only offer `$A5` again after receiving
  `$5A`).
- **`MASTER_TIMEOUT` is 2 s and that is deliberately generous** — the reference server paces
  at 10 ms per byte from a Python thread with Nagle on. A timeout tight enough to feel
  "responsive" would break the one counterparty this exists to talk to. `SLAVE_ARM_WINDOW`
  is two frames for the mirror-image reason. Neither is a round number chosen for looks;
  both are bounded by a measured counterparty.
- **`examples/support/gen1_cable_club.rs` is now shared** by `link_two.rs` (both its
  in-process and its auto-discovered-peer modes) and `bgb_link.rs` — the Cable Club bot,
  the save staging, `patch_party`, the tilemap screen reader and `write_one`. Three copies
  of that bot would eventually be three different bots and the trade proofs would stop
  being comparable. `#[path]`-included, same pattern as `support/suite_baseline.rs`.
- **A trade verdict must check the whole Pokémon, not just an OT name** — the peer-mode
  finding, adopted here. A lost byte shows up as `$FF` inside a name, `$FF` is not a
  character, and a decoder that skips unknown bytes turns a corrupt `GLIMMER` into a
  plausible `MMER`. `bgb_link --expect-ot` compares nickname, species and level too.

## Two consoles on one cable (`src/link.rs`, `src/mmu/serial.rs`, `examples/link_two.rs`)

- **Full write-up, with the Gen 1 ↔ Gen 2 Time Capsule rules:
  [`docs/link-cable.md`](docs/link-cable.md).** This is the *other* way to be the far end of
  the link port: `plugins/gen1-link` **impersonates** a console and can only know protocols
  somebody wrote down; `LinkCable` **relays bytes between two real cores** and needs to know
  none. Both are wanted — impersonation is the answer with one cartridge.
- **The slave half of the port did not exist.** `Serial::wb` acted on `SC = $81` and ignored
  `SC = $80` entirely. That is *correct* for one console alone (nothing is ever going to clock
  it) and is exactly the half a second console needs. Adding it was the substantive work; the
  cable on top is ~250 lines.
- **TWO IDENTICAL CORES IN LOCKSTEP NEVER DRIFT, AND THAT BREAKS THE HANDSHAKE.** Real consoles
  have independent crystals and real players press A on different frames, so their link
  handshakes slide past each other until they interleave. Started together, two cores stepped
  by one loop stay in phase **for ever**: both drive the clock at the same instant, each finds
  the other deaf, and Gen 1's `.establishConnectionLoop` times out at 90 iterations. Measured
  Blue-to-Blue: skew 0 **never** connects, every skew from 1 to 34 frames connects within ~700
  frames. `examples/link_two.rs --skew N`; the TUI applies `LINK_START_SKEW = 3` at startup.
  **This is the price of determinism, not a cable bug** — do not go looking for the bug.
- **A torn byte must KEEP its data.** When the far console arms `SC = $80` and rewrites `SC`
  before the eighth edge, the master keeps what was latched; only the slave half is abandoned.
  The first version voided it and faulted both machines as a desync — and then faulted them 898
  frames into a real trade, because Gen 1's handshake offers `SB = $02` behind `SC = $80` and
  replaces it with `SB = $01` / `SC = $81` about **100 T-cycles** later, every frame. Reading
  `$02` out of that window is the only way two consoles agree who drives the clock, so voiding
  a torn byte means never connecting two Game Boys at all.
  - **Now measured rather than asserted, because the source had drifted against itself.** With
    a slave offering `$22` and disarming mid-transfer, the torn transfer lands **`SB = $22`**;
    only the *next* transfer, with nobody listening any more, shifts in the idle-high `$FF`.
    `src/link.rs`'s module docs had grown a sentence claiming a torn byte gives the master
    `$FF`, and the test was **named** after that (`…_leaves_the_master_an_idle_line`) while
    asserting `$C000` after the ROM's loop had already run a *later*, unanswered transfer — so
    the name and the prose agreed with each other and disagreed with the code and with
    `docs/components/serial.md` item 10. Renamed to
    `a_slave_that_disarms_mid_transfer_keeps_the_byte_it_had_offered`, and it asserts both
    halves now. **A test that asserts the right value for the wrong reason is a stale note with
    a green tick on it.**
- **The fail-safe is fault propagation, not byte accounting.** One console faulting faults the
  other (`Gameboy::report_fault`, so both batteries are poisoned): half a trade leaves the
  *survivor* holding the dangerous save. Byte-level desync is the games' job and Gen 1 is good
  at it (`Serial_ExchangeBytes` re-sends until it sees the `$FD` preamble).
- **Which Cable Club seat is not a choice.** `CableClubLeftGameboy` returns immediately unless
  this console took the *internal* clock and wants the player facing **right**;
  `CableClubRightGameboy` mirrors it for the external clock (`engine/pokemon/bills_pc.asm`).
  Read `hSerialConnectionStatus` (`$FFAA`, asserted by name in `engine/link/cable_club_npc.asm`)
  and face accordingly — pressing A at the wrong console is a silent no-op that looks exactly
  like a broken link. Note the values: `USING_EXTERNAL_CLOCK` is **`$01`**, internal is `$02`.
- **TUI: `link = <rom>` boots a second console, `W` swaps which one you play** (`L` was already
  "back to the picker"). Focus is a literal `mem::swap` of the two `Gameboy`s so the renderer,
  palette, upscaler, screenshots and plugins keep working on `Session::gb` — but the cable
  records the master positionally, so `LinkCable::swap_ends()` must be called with it.
  **`Session` is built on the stack by every render test: anything `Gameboy`-sized added to it
  must be `Box`ed** or `cargo test --lib` dies with a stack overflow.
- **`gen1_trade --make-save` still does not work on this box** (it loops in Oak's speech at a
  "BLUE is playing the SN[ES]" screen). It does not need to: **`~/roms/*.sav` already *is* a
  Cable Club fixture** — player BLUE standing at (11,3) in the Viridian Pokémon Center, party of
  two, no badges. Copy it; never open it for writing.
- **Verified end to end, Blue to Blue:** 1552 transfers, 0 unanswered, 0 torn, each console
  holding the other's Pokémon with DVs intact, and both `.sav` files passing the game's own
  `CalcCheckSum`.
- **Gen 1 ↔ Gen 2 is DONE — the Time Capsule trade completed both ways** (see the Gen 2
  section below and `docs/link-cable.md` §8). The cable itself needed *nothing* added — in Time
  Capsule mode Gen 2 speaks Gen 1's protocol byte for byte (`Link_PrepPartyData_Gen1`,
  `ConvertMon_2to1`). The in-game state (Bill met in Ecruteak, a day elapsed, a party with no
  species `>= #152`, no move `> STRUGGLE`, no mail — all three Johto starters excluded) came
  from the captain's real Gold save; its gates were already open, and only the six-Johto party
  had to be patched on the staged copy.

## The cable is live by default, and Gen 2 (`src/link/peer.rs`, `examples/gen2_timecapsule.rs`)

- **Full write-up: [`docs/link-cable.md`](docs/link-cable.md)** — including §9, the captain's
  remote-play vision, marked *not built*.
- **`link = auto` is the DEFAULT.** Two instances on one machine discover each other over a
  unix socket in `$XDG_RUNTIME_DIR/terminalgb/link` (`$GB_LINK_DIR` overrides) and pair. That
  is a correctness requirement, not a convenience: Gen 1's Cable Club attendant checks the
  link *before* she lets you through, so a cable that only wakes on request is the cable she
  turns away. `link = off`, or `W` in game, opts out.
- **A CROSS-PROCESS MASTER TRANSFER MUST BE HELD** (`Serial::link_hold`). Both emulators pace
  to 59.7275 Hz and each emulates a whole frame in a fraction of a millisecond then sleeps —
  two bursts need not overlap at all, so a 4096-cycle transfer that completed on schedule
  shifts in `$FF` every time and no link ever works. The game cannot tell: it is already
  spinning on `SC` bit 7. **Never "fix" this by making the transfer complete on time.**
- **Three bugs cost 409 bytes on the first cross-process trade, and it LOOKED FINE**: the
  received Pokémon read `MMER` (OT `D`) instead of `GLIMMER` (OT `RED`), because a lost byte
  arrives as `$FF`, `$FF` is not a character, and a decoder that skips unknown bytes turns
  corruption into something plausible. **Check the bytes, not that something arrived.** The
  three: both consoles mastering deadlocked (each holding for the other — Gen 1 masters on
  both every frame until roles settle, so answer "nobody listening" at once); a timed-out
  offer answered later, spending one of the *far* console's bytes and sliding its whole
  stream (`Kind::Abort` retracts); and a timeout counted as benign — it is `torn` now and it
  **faults**, because the connection is alive so the two are still exchanging and have simply
  stopped agreeing.
- **Pairing is two at a time in ARRIVAL ORDER, not by cartridge.** Blue, Blue, Gold → the two
  Blues share the cable. A paired instance **stops advertising** (that is what makes three
  terminate) and **drains its backlog with `Busy` first**, because a connection that dies
  before saying hello is indistinguishable from a crashed partner — and faulting there would
  poison the save of a console that was never linked. **Nothing faults over a peer it never
  handshook with.**
- **`Session` is built on the stack by every render test**: anything `Gameboy`-sized added to
  it must be `Box`ed or `cargo test --lib` dies with a stack overflow. Discovery happens in
  `App::with_warnings`, **never** `App::build` — `build` is what the render tests construct
  in parallel and two of them would find each other, the same reason the saved volume is
  applied there.
- **Reading a ratatui byte stream tells you the FIRST paint, not the state.** Only changed
  cells are rewritten, so a later state lives elsewhere in the stream. A two-instance run
  read as `Link waiting` and had in fact paired; measure the state, not the first paint.

### Gen 2 / Pokémon Gold

**Everything Gen 2 now lives in [`docs/gen2/`](docs/gen2/README.md) — one source of truth, do
not restate it here.** [README](docs/gen2/README.md) (the grades, and a blunt list of what is
*not* established), [building the disassembly](docs/gen2/building-the-disassembly.md),
[memory map](docs/gen2/memory-map.md), [the save file](docs/gen2/the-save-file.md),
[reading the screen](docs/gen2/reading-the-screen.md),
[the Time Capsule](docs/gen2/time-capsule.md), [sharp edges](docs/gen2/sharp-edges.md). The
cable-side record of the run stays in `docs/link-cable.md` §8.

The handful of things a crew must know *before* opening any of that:

- **Gen 2 knowledge here is narrow and deep, not a map.** It all came from one project — the
  Time Capsule — so battles, boxes, the bag, the map system and the RNG are **not established
  at all**, and the area says so on every page. **A plausible-looking Gen 2 address is worse
  than no address**: the failure mode is a save that passes its own checksum with a write
  sitting in the middle of somebody else's data.
- **Build `pokegold` at rgbds 1.0.3 and check it against `roms.sha1` before deriving any Gen 2
  address** (SHA-1 `d8b8a360…`; 0.6.1 is too old, the repo demands ≥1.0.0). Constants come from
  `rgbasm -Q8 -P includes.asm -D _GOLD` + `PRINTLN "{d:SYMBOL}"`, never a hand count.
- **No Gen 2 address in this repo is signature-verified against the cartridge at run time** —
  the grade Gen 1's `plugins/battle-control/src/wram.rs` supplies and Gen 2 lacks entirely, so a
  Gen 2 tool handed Silver, Crystal or a hack writes wherever the transcribed number points.
  Highest-value gap; do not ship a Gen 2 tool without closing it.
- **THE SHINY RULE IS CONFIRMED AGAINST GOLD ITSELF** (`CheckShininess`, `engine/gfx/color.asm`:
  `SHINY_ATK_MASK %0010`, def/spd/spc all `10`) — exactly what `plugins/trainer`'s
  `derived::shiny_if_traded` implements, and reading it out of the cartridge covers all 65,536
  DV pairs where a trade samples one. **Do not re-litigate this.**
- **CLOSED: the Gen1↔Gen2 trade completed end to end** (`gen2_timecapsule --capsule-trade`, one
  process, both cores on `LinkCable`): DVs byte-identical across the wire, drawn by Gold in the
  ROM's own shiny palette and quick-saved by Gold itself. 2,580 transfers, 0 torn.
- **Two facts are INVERTED between the generations and will bite a Gen 1 habit**: dismiss
  dialogs with **A, never B** (B backs out to the title screen), and `hSerialConnectionStatus`
  is **`$FFCD`** on Gen 2 against Gen 1's `$FFAA`.
- **One correction the new pages carry that the old note did not**: "a Gen 2 save offset is not
  one base plus an offset" is a claim about the *structure* (three separate copies), not about
  today's arithmetic — on Gold the three runs are contiguous on both sides, so the shortcut
  happens to give the same answer, which also means a bug in the per-run form would not have
  been caught here. Keep the per-run mapper; do not claim it has been proved necessary.

### Two hand-launched emulators traded across the generations (2026-08-15) — and what that measured

**The milestone: the captain started Gold in one terminal and Blue in another, by hand, no
lobby, no flags, nothing scripted; `link = auto` paired them and a Time Capsule trade
completed.** Everything before this was one process driving both cores. Full forensics —
field-by-field table, file offsets, md5s — [`docs/link-cable.md`](docs/link-cable.md) §8
"Two independently launched emulators"; conformance record in `docs/conformance-notes.md`.

- **`testharness/twoprocess.rs` is the first thing here that launches two processes and links
  them** (`GB_TWOPROCESS_RUN=1 cargo test --release --features
  image,plugin-gen1-link,plugin-trainer --test twoprocess`; **opt-in, checked before any fixture
  search** — without `GB_TWOPROCESS_RUN=1` it prints why and skips exit 0, unconditionally, and
  a plain `cargo test` never runs it at all. See "gate the two-process harness behind an explicit
  env var" below for why that had to be added on top of the fixture skip. Once opted in it still
  skips exit 0 without `POKE_ROM` + `GB_CABLECLUB_SAVE`, like `colosseum.rs`, and like it does
  **not** fall back to a player's save directory). It re-execs itself as both
  children — the `basicterm.rs` pattern — so there is no cargo-inside-cargo. Judge three ways:
  each child checks its arrival field by field, the parent checks `transfers > 0 / torn = 0 /
  no fault`, **and the parent reads both `.sav` files back off disk**. That third check is the
  claim only two processes can make and it is the one a person means by "the trade worked".
- **IT IS NOT IN CI ON PURPOSE, AND ITS FAILURE RATE IS A FUNCTION OF MACHINE LOAD, NOT A
  PERCENTAGE.** `GB_TWOPROCESS_REPEAT=N`, 20 cores, same binary and fixtures throughout:
  **122/130 on an idle box**, then **19/20 at load ~5, 7/10 at ~10, 3/10 at ~31, 1/10 at ~49**.
  Two failure modes — stalls (both consoles pair, exchange 17-22k bytes, never finish) and
  **silent corruptions where an arrival that had lost a byte off the front was written to the
  battery file with `torn = 0` and `fault = none`** (`OCKY` OT `LUE` Lv0, `MMER` with no OT,
  `LIMMER` OT `ED`). A CI runner is a loaded machine, which is the bad end of that table.
  **Run it by hand on an idle box and quote the load average**, before and after anything
  touching `src/link/peer.rs`. **The obvious cause was tested and REJECTED**: `OFFER_TICKS` is
  two frames of *emulated* time (~0.7 ms of wall clock on an unpaced console — the sister of
  §10's "a deadline for another process belongs to the wall clock"), and ×100 it changes
  nothing (15/20 patched vs 17/20 baseline, interleaved under the same load). Cause open; build
  a wire log on the peer transport before guessing again.
- **`LinkPeer::discover` scans the rendezvous directory ONCE and never rescans**, so two
  instances started in the same instant both advertise and neither ever pairs (measured: 24,000
  frames, `transfers = 0`). A person cannot hit it; anything spawning two processes hits it
  every time. The harness waits for the first socket to appear. Fix shape, left to the link
  crew: rescan while `Waiting`, with a deterministic tie-break (lower pid initiates).
- **`examples/link_two --peer a|b` stages BOTH sides in each process**, so two invocations race
  on each other's `.sav`. Fine for a human running them seconds apart; the harness stages in the
  parent instead. Don't copy that pattern.
- **`GB_LINK_DIR` is mandatory in any automated link test** — `link = auto` is the default, so
  without a private rendezvous directory the test pairs with the developer's own emulator and
  drives *their* console into the Cable Club.
- **A stray temp file used to be enough to make this run inside a PLAIN `cargo test`, and that
  was a second bug riding on the first.** `find_save()`'s fallback is
  `$TMPDIR/gameboy-gen1-trade/cableclub.sav` — the exact fixture `colosseum.rs` also looks for,
  and `required-features = ["image", "plugin-gen1-link", "plugin-trainer"]` are all satisfied by
  `default`. So once `gen1_trade --make-save` had ever produced that file on a box, any later
  full-suite `cargo test` run would find it, find a ROM in `~/roms`, and actually launch two
  processes into the reliability table above — on whatever load the machine happened to be
  under, for a bug nobody asked to reproduce. **`GB_TWOPROCESS_RUN=1` is now required and is
  checked before either fixture search runs**, so the trigger is what a person typed on
  purpose, never test order or a leftover file. The verdict when it does run — field-by-field
  arrival, `torn = 0`, no fault, both battery files checksum-verified off disk — is unchanged;
  gating *whether* it runs must never become an excuse to loosen *what it demands*.
- **GOLD SAVES TWICE AROUND A TIME CAPSULE TRADE.** `TryQuickSave` at the receptionist on the
  way in, and `LinkTrade`'s `cp LINK_TIMECAPSULE / jr z, .save` → `SaveAfterLinkTrade`
  (`SavePokemonData` + `SaveChecksum` + both backups) *before* "Trade completed!" is printed.
  Gen 1 saves once, after ("this allows reset into Pokecenter"). **So a Gold `.sav` holding the
  pre-trade party after a completed trade means the second write did not land — not that Gen 2
  declines to save.** That is what happened here and **why is not established**; the artefact
  that would settle it is the emulator's own `Fault: SAVE NOT WRITTEN` row, which nobody read
  before closing the window. *Next time two emulators are linked by hand, read the Status panel
  of the console that looks wrong before closing it.*
- **Never diff a `.sav` against an "original" without checking it is the immediate ancestor.**
  `~/roms/goldsave/Pokémon - Gold Version.sav` is the *cartridge's* 2024 save; between it and
  the trade the party was replaced with Time-Capsule-legal Pokémon and the Johto team boxed, so
  diffing those two measures the preparation, not the trade. The correct pre-trade snapshot was
  Gold's own pre-link quick-save — identifiable because `wMapGroup`/`wMapNumber` = 20/1 at
  (13, 4), the tile below the receptionist.
- **A Gen 1 party slot replaced by a trade is a 4-run signature in the save**: the species-list
  byte, the 44-byte struct, the OT name, the nickname — plus the dex owned/seen bit for the
  arrival and the checksum. Everything below file offset `$2000` is bank 0 "Sprite Buffers"
  scratch the Cable Club legitimately rewrites (same exclusion `colosseum.rs` makes).
- **Verify a species conversion against the cartridge, not against memory.** `PokedexOrder`
  (`pokered/data/pokemon/dex_order.asm`) is findable by its first eight entries
  (112,115,32,35,21,100,34,80) — exactly one hit in Blue, at `$41024` — and gives internal `$99`
  → dex 1, internal `$83` → dex 150.
- **`$E1` is `<PK>` in BOTH charmaps** (`pokered` and `pokegold` `constants/charmap.asm` agree),
  which is why an OT name containing it renders identically on the receiving console.

## The trade-portal client (`src/portal/`, `portal` cargo feature)

**Full write-up, the HTTP contract as this client speaks it, and the honest
measured/unverified split: [`docs/portal.md`](docs/portal.md).** The portal itself is a
separate repo built by another crew; the only join is the HTTP contract, so
`testharness/portal.rs`'s stub **is** that contract from this side (16 checks, real
`TcpListener`, real curl transport). Field names are **unverified against the real portal**
until it is live — treat a mismatch there as a coordination item, never something to patch
silently on one side.

- **The transport is the system `curl` binary, not a crate** (`portal::http`), because the
  dependency count is a contract (`make embed`) and TLS is the one thing never to hand-roll.
  Every request goes through `curl --config -`: **credentials ride stdin, never argv**
  (argv is world-readable in `ps`; the device code is a credential too). curl's config
  quoting understands only `\\` and `\"` — `config_quote` emits exactly those and drops
  control characters. Timeouts are hard on every request (5 s connect / 15 s total),
  measured by harness check [7] (an unreachable portal fails in bounded time).
- **A portal error is a REFUSAL to arm gen1-link, never a fallback** (`attach_portal_gen1`,
  `src/screen/tui.rs`). Signed out or unreachable ⇒ *no partner at all*, not the trade
  file, not the default Mew: a matched trade may exist that this process cannot see, and a
  player who trades their Pokémon to the built-in partner while a friend waits has lost it
  silently — the exact failure the feature exists to prevent. The **only** route to the
  trade-file fallback is the portal answering, signed in, "nothing pending". The portal
  outranks `gen1_trade` for the same reason: a pending trade was explicitly accepted.
- **The stored token is fingerprinted to the portal URL that issued it**
  (`auth::parse_store`, the volume file's `from_config` rule applied to a credential):
  change `portal =` and the token is refused, not sent to the new server. Stored at
  `state_dir()/portal-token`, `0600` from creation (`OpenOptionsExt::mode`, no
  default-permission window), tmp→rename like the battery. Sign-in is RFC 8628; any 4xx
  carrying `error` is accepted as flow state (the report sketched 428, the RFC says 400 —
  do not pin one).
- **In-TUI login (`@`) is a thread the loop polls** (`portal::login::LoginFlow`,
  `App::portal_tick`) — same never-block-the-frame rule as the audio hand-off. The browser
  open is fail-soft (`portal::browser`, `$BROWSER` first, then xdg-open/open/start) and the
  URL + code are always shown as well; `GB_PORTAL_NO_BROWSER=1` for headless. Auth state is
  a Status-panel `Portal` row — the plugin-row rule: a state nobody can see is one nobody
  can debug. Completion (`POST /trades/<id>/done`) is watched on the partner's own
  `trades_completed()` counter and retried on a bounded schedule (30 s × 10, then a loud
  give-up naming the website); the received Pokémon is already safe in the save either way.
- **THE TWO-RLIB TRAP BIT THIS WORK — a `harness = false` test must not link a plugin
  crate the lib also links.** `testharness/portal.rs` calling
  `terminalgb_plugin_gen1_link::spec::PokemonSource::describe(&src)` failed E0277 ("trait
  not implemented" for a type that plainly implements it): with `[profile.release]
  panic = 'abort'` on the lib targets, the test resolved the plugin crate to a *second*
  (unwind-built) rlib, so trait and impl came from different crate instances. It compiled
  on the first run and failed on the next two — the same intermittent class the chiptune
  section records. Fix: the harness talks only to `terminalgb` re-exports
  (`PortalSource::describe_line` exists for exactly this); the real
  partner-from-portal-source construction is proven in `examples/portal_trade.rs`, where
  examples and lib share one build graph. Verified there against a staged copy of Pokémon
  Blue: trade #71 resolved to "BULBA the BULBASAUR (Lv14)" through the cartridge's own
  tables and the done-POST landed authenticated.
- **The wasm break the scout report flagged really was TWO breaks, the second hidden
  behind the first** — measured independently here before `5092a34` landed the same
  diagnosis on `main` (whose fix is the better one and is what shipped: `audio_enabled`
  answers `false` on wasm instead of vanishing from the API, and `make check-wasm` now
  gates it). The report's own caveat ("I cannot tell whether a second error hides behind
  the first — treat 'one line from working' as unverified") was exactly right: applying
  the one-line `audio_enabled` cfg let the lib compile and exposed the next failure, the
  default-feature `terminalgb-chiptune` `[[bin]]` importing a module that is compiled out
  on wasm. Two crews hit both within the hour; re-measure before fixing a red build
  someone else may already have fixed.
- **Config key `portal` validates** (must start `http://`/`https://`, trailing slash
  trimmed so path joins cannot `//api`; a non-URL warns and is dropped — the `protocol`
  key's rule). It is a URL, deliberately not in the tilde-expansion test.

### The chain is PROVEN end-to-end against the real portal, and the wire shapes moved (2026-08-14)

- **The whole chain ran locally**: portal repo (`terminalgb-portal`, docker Postgres on
  :5433, real Clerk dev keys in its `.env.local`, `npm run dev`), two Clerk test users,
  `terminal login` device-code sign-in for both (CLI *and* the TUI `@` key), friends by
  code, offer/accept on the website, and **both sides' in-game trades completed and
  field-verified** via `portal_trade --play` (below). The portal's trade log ended
  `completed` with both attestations.
- **The two repos had shipped INCOMPATIBLE wire shapes, and the emulator moved.** The
  portal implements its own `docs/api-contract.md` exactly (camelCase `otId`/`otName`,
  `dvs` as an object, trades relative to the viewer: `youReceive`/`youGive`/`partner.
  displayName`, accept body `{"mon":…}`, replies wrapped `{"offer":…}`/`{"trade":…}`);
  this client had shipped snake_case, a DV array, flat `receive`/`friend`/`give` and
  `{"give":…}`. Symptom against the live portal: `portal protocol: trade 10: no
  'receive' Pokémon` — the partner is refused, correctly, per the no-silent-fallback
  rule. The client now **sends** the contract's spellings (both OT spellings for
  compatibility) and **accepts** both; `testharness/portal.rs` pins both shapes plus the
  new rule that completed/`youDone` trades are never re-armed (`trades/mine` includes
  them; arming one would replay a finished trade — check [5b]/[5c]).
- **The live portal's token reply carries no `user`**, so "signed in as …" comes from
  `auth::whoami` (`GET /api/me` right after the token lands, fail-soft). A token stored
  before that fix has no `user =` line and shows "(name unknown)" until re-login.
- **`portal_trade --play --save <cableclub.sav>` is the one-command end-to-end proof**:
  stages ROM+save, arms the partner from the portal, drives the shared Cable Club bot
  (`examples/support/gen1_cable_club.rs`) through the whole trade, verifies the arrived
  party mon field by field, and only then POSTs `/done`. ~5,100 frames headless. The
  `~/roms` Blue `.sav` remains the Cable Club fixture (copy it, never write beside it).
- **Clerk's sign-UP is behind a Turnstile CAPTCHA that automation cannot click**; create
  test users through Clerk's Backend API (`POST https://api.clerk.com/v1/users` with the
  secret key) and sign IN instead — password + the fixed `424242` OTP for
  `+clerk_test@example.com` addresses. The portal's own `DEV_AUTH=1` bypass and real
  Clerk keys are mutually exclusive per server run (`devMode()` short-circuits Clerk).

### Offering FROM THE SAVE (`O`, `src/portal/offer.rs`, `plugins/trainer/src/collection.rs`)

**An offer never comes from a form — it is picked from the loaded save's party and PC
boxes**, because a typed offer is unverifiable and the emulator is the one component that
can read the actual save (captain's rule; full write-up + the portal-side change spec in
`docs/portal.md` §"Offering from the emulator"). Verified end-to-end against a staged copy
of the captain's real Blue save: party (SPARK Lv12 / MEWTWO Lv70) **and** a checksummed
SRAM box entry both read back (`cargo run --release --example offer_list -- <rom>
[--save <sav>]`, the evidence tool).

- **The box layout is symbol-verified, not walked.** `/tmp/pokered` + rgbds 1.0.3
  (`/tmp/rgbds103`) build `pokered.gbc`/`pokeblue.gbc` **byte-identical to retail**
  (`sha1sum -c roms.sha1` OK), and every constant in `gen1::boxes`/`gen1::boxmon` is the
  `.sym` value, identical in both games: `wCurrentBoxNum $D5A0`, `wBoxDataStart $DA80`,
  block `$462`, `sBox1..6` bank 2 / `sBox7..12` bank 3 from `$A000`, per-box checksums at
  `$BA4D`. Re-derive by rebuilding, never by hand.
- **Three `engine/menus/save.asm` semantics are load-bearing** (module doc of
  `collection.rs`): until the first-ever box change (`wCurrentBoxNum` bit 7,
  `BIT_HAS_CHANGED_BOXES`) SRAM banks 2/3 are **uninitialised garbage** — do not read
  them; the current box's SRAM slot is deliberately emptied (WRAM is authoritative for
  it); every SRAM box carries `CalcCheckSum` (complement of byte sum) in
  `sBank{2,3}IndividualBoxChecksums`, and a box failing it is refused with a note, never
  decoded.
- **SRAM is unreachable over the bus almost always** — games leave cartridge RAM
  *disabled*, so `Machine::peek($A000)` reads open bus. The seam is
  `Machine::peek_sram_bank` → `MemoryBankController::readram_bank` (default `None` =
  "this cartridge cannot answer" — absent storage must be distinguishable from a stored
  `$FF`). Touches no MBC register, so it cannot perturb the game.
- **The two-phase read exists for the borrow checker**: `collection::read_raw(&mut
  Machine)` then `collection::decode(&raw, plugin.tables())` — a frontend cannot hold
  `with_machine`'s `&mut gb` and the trainer plugin's tables at once.
- **Tradeability verdicts are the wire's rules, not taste** (`collection::verdict`):
  Gen 1's own Trade Center refuses nothing per-Pokémon (`TradeCenter_SelectMon` goes
  straight to the exchange; no items/mail/eggs exist), so what is refused is: `$FE` (the
  digit `8`) in a name — the link protocol's no-data byte; bytes outside the printable
  charmap; glitch species (dex 0) and moves above `STRUGGLE` (`$A5`); levels outside
  1-100; and a slot whose two independently-written species copies disagree (the
  enemy-team rule applied to storage).
- **The shiny flag rides the offer wire and is derived at send time**
  (`client::mon_to_json` writes `shinyIfTradedToGen2` from the DVs via the trainer
  panel's ONE rule — never accepted as an input, so it cannot disagree with the `dvs`
  beside it). Wording rule everywhere it appears, picker legend included: *"would be
  shiny if traded to Gen 2"*, never "is shiny" — Gen 1 has none. `shiny` is a filter
  word in the picker.
- **THE SAVE FILENAME IS `<stem>.sav` — the extension is REPLACED, not appended.**
  `game.gb` saves to `game.sav` (`Battery`/`cable_club::stage` both use
  `with_extension("sav")`). Staging a save as `game.gb.sav` silently boots a fresh game,
  whose menu has no CONTINUE, and a screen-driven bot then loops in Oak's speech forever
  — this cost an hour here. Use `cable_club::stage()`; do not hand-roll staging.
- The offer UI is `Overlay::Offer` in the TUI (`O` in game, `Esc`/type/`Enter`×2), state
  machine in `src/portal/offer.rs` (terminal-free, unit-tested); the POST runs on its own
  thread polled by `portal_tick` — the never-block-a-frame rule. `testharness/portal.rs`
  checks [6b] pin the offer POST (authenticated, save-read fields, shiny flag); the
  harness reaches trainer types **only through `terminalgb::portal::offer` re-exports**
  (the two-rlib trap again — never add a plugin crate to a `harness = false` target).

### The offer CARD, live offers, and the Gen 1 identity heuristic (`src/portal/card.rs`)

Full contract + proposal to the portal crew: `docs/portal.md` ("The offer card", "Offers
track the live game", "Offers group by game and player"). What is durable:

- **The offer POST carries `card` + `game` + `identity` beside `mon`, and `mon` itself is
  untouched** — every trade route keeps its exact shape; the extras ride at the offer level
  only and are all optional on the wire. The sprite is the offered species' front picture
  **decompressed from the offering player's own cartridge at offer time** (the trainer
  panel's decoder + edge-flood-fill transparency), palette PNG with `tRNS`, base64 —
  measured 680 B for Pikachu, 1,075 B for Mewtwo against the captain's real save. **The
  sprite rule is unchanged**: no artwork is shipped, vendored or served from this repo;
  a picture that does not decode sends *no* sprite, never a placeholder. `moves` were
  already travelling inside `mon` before any of this — the portal just wasn't showing them.
- **The Gen 1 identity key is OT id + OT name + DVs, and NOTHING else — the nickname is a
  trap.** Verified against `pret/pokered` before relying on it (a brief claimed "Gen 1 has
  no Name Rater" — it does: `scripts/NameRatersHouse.asm`, Lavender Town, renames any
  own-OT Pokémon, which is exactly the Pokémon a player can offer; and `RenameEvolvedMon`
  rewrites an unnicknamed Pokémon's name on evolution). DVs and OT are written once at
  capture (`_AddPartyMon`) and never rewritten. Species changes on evolution — the exact
  case live tracking exists for. **Fail-safe direction: ambiguous identity (same-OT
  identical-DV twins) is withdraw/untracked, never guess** — a trade is irreversible.
- **The live watch's suspend-vs-withdraw split is load-bearing** (`portal::offer::plan`,
  pure + unit-tested): a *different save loaded* (game title or player mismatch) means
  **do nothing** — withdrawing there would nuke Blue's offers the moment Gold boots; only
  the same save answering "gone / refused / ambiguous" withdraws. Only offers posted this
  session are tracked (ids come from POST replies); resuming needs the proposed
  `GET /api/offers/mine`.
- The update/cancel routes (`POST /api/offers/<id>/update|cancel`) are **proposed** —
  unverified against the real portal; the watch gives up loudly per offer after 3 failures,
  so a portal without them degrades to stale-offer warnings, not a hammer loop. — "Claude plays Pokémon" (`plugins/agent/src/`, `src/screen/agent_view.rs`)

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

### Live-run findings, third session (the captain watching in a wezterm window)

- **A fact the model may not consult is not a rule — the naming keyboard proved it.** The
  "press START, A types letters for ever" fact had been in the prompt AND in two docs since
  the first recorded run, and a live run typed a name of A's anyway, in front of the captain.
  Cause: the presses came from **the host's own `advance_text` loop** — the naming keyboard
  reads as "text box wants A", and no model call was in flight at all. Enforced host-side
  now: `Screen::naming_keyboard()` (`plugins/agent/src/observe.rs`), an early-out in
  `advance_text` that names START, and a situational snapshot note; pinned by two
  `testharness/agent.rs` cases (teeth proven by reverting the guard: 12/13). **When a
  documented fact is walked into live, promote it to an enforced rule where the buttons are
  pressed** — the same promotion the standing-on-an-exit note earned.
- **A `game.sprites` observation was built and WITHDRAWN — ship nothing gated on the player's
  sprite slot.** The sprite-table layout is right (`wSpriteStateData1`/`2`, map Y/X at
  `+4`/`+5` in `wXCoord`'s tile grid offset by 4, `SPRITE_POKE_BALL = $3D`), and it is the
  generic answer to the balls-on-the-table blindness — but the consistency gate "sprite 0's
  map coordinates == `wYCoord+4`/`wXCoord+4`" measured **never true** in twenty live
  decisions across four maps, so the feature reported nothing and was pulled rather than
  committed inert. The engine does not maintain the player slot's map coordinates in
  ordinary play (only map scripts write `StateData2MapY`). A future gate must be
  per-NPC-slot bounds against the map size, **verified against a live game first** — the
  harness fixture passing is not that proof; it encodes whatever the author believed.
- **The TUI's agent path had two budget gaps, both now closed:** `build_agent` left
  `call_timeout` at `Budget::default()`'s 60 s while the shipped CLI adapter legitimately
  retries up to 2 x 60 s (its header assumes "150 s by default") — every slow call was
  killed mid-retry, seen live as the game sitting on a menu with "the agent took too long to
  answer" — now 150 s; and the wall clock (the one host ceiling that is not self-reported)
  was not settable at all — now `GB_AGENT_WALL` seconds, typo warns and is ignored.
- **What a viewer sees is think-time, and it clusters at map changes.** Every map change
  interrupts the plan and forces a decision; via the `claude` CLI a decision is a fresh
  session — measured this session at 8-25 s (haiku) and ~4-8 s (sonnet), plus occasional
  60 s+ stalls under rate limiting. The captain reads that as "it sits on the steps after
  every doorway". The CLI adapter is the convenient path, not the watchable one; the API
  adapter (`ANTHROPIC_API_KEY`, not present on this box) is the fix, not more prompt.
- **The screen text was not the problem *on the screens this session saw*.** `screen.text`
  (VRAM-decoded) is in every snapshot — the runs read "YOUR NAME?" and "but you can have
  one! Choose!" fine. When a watching human says "it cannot read the screen", check what is
  NOT text first: the balls are sprites, NPCs are sprites, terrain is collision data.
  **Narrowed by measurement afterwards** (next section): on screens this session never
  reached — the battle menu, the intro, any two-column menu — the decode *was* wrong, and
  silently. "It read the overworld fine" is not "the decode is right".
- **Watching a TUI agent run headlessly:** `GB_AGENT_LOG=<path>` NDJSON plus `tail -F` (the
  file is recreated per launch — `-f` follows a deleted inode and goes silent forever).
  Milestones and failures both appear as `decision` events; `error:"timeout"` with an empty
  reply is the host killing a call.

### The screen text channel — a decode that lies is worse than one that is missing

**Full write-up + the fourteen-screen evidence table: [`docs/claude-plays.md`](docs/claude-plays.md) §12.**
Evidence tool: `cargo run --release --features plugin-agent --example screen_text -- --rom
<blue.gb> [--save <sav>] (--tour | --menu-tour | --stability | --state <gbss>)`.

- **The channel already existed and already worked — the defects were in what it omitted,
  invented and never admitted.** `screen.text` has been in every snapshot since the plugin's
  first commit and the live-run list says it read menus and dialogue correctly. Check that
  before rebuilding a pipe: the finding was that four of §11's six blind spots were *decode*
  gaps, not plumbing.
- **A TILE ID IS A VRAM INDEX, NOT A CHARACTER — and Gen 1 reloads different pictures into
  the same slots.** `pret/pokered`'s `constants/charmap.asm` lists `$6E`-`$75` **twice**:
  once in the main font (quotes, unused hiragana) and again under *"Actual characters (from
  `gfx/font/font_battle_extra.png`)"*, plus overlays for the status screen (`<BOLD_P>` at
  `$72`) and the Pokédex (`$60`/`$61`). Decoding `$70`-`$75` as quotes put `”’` on a line of
  its own and `153/153”` on the HP row of **every** battle screen. They are left blank.
  `$6E` is the opposite and *is* decoded (`Lv`): its main-font meaning is a hiragana no
  English screen ever places, so only one picture can be on screen. `$F0` is `¥` normally
  and the `ED` key on the naming screen — the one ambiguity resolvable, because that screen
  announces itself, and `Screen::from_tiles` renders twice there for exactly that.
- **The battle menu's `PKMN` was invisible** — `<PK><MN>` (`$E1 $E2`) both decoded to
  spaces, so a model was shown `FIGHT / ITEM RUN` and could not know switching exists.
  Contractions (`'s`, `'t`) and `<LV>` were the same omission.
- **`Screen::selected` needs a column rule, and both halves are load-bearing.** A menu item
  may contain a blank (`NEW GAME`); a two-column menu may separate its columns with only one
  (`▶FIGHT PKMN`, measured — `FIGHT` ends at column 14, `PKMN` starts at 16). The rule is
  that a menu's columns line up across **its own rows**, i.e. rows that begin an item in the
  same column as this one. Using *every* row instead truncated `NEW NAME` to `NEW`, because
  the question printed under the list begins a word where `NAME` does. On the naming
  keyboard the same rule yields the single letter under the cursor for free.
- **Withheld beats confidently wrong.** `Screen::trust` is `graphics-not-text` when artwork
  laid out as sequential font-range tiles decodes to alphabet salad (the Game Freak intro
  really does read `ABCDEFGHIJKLMNOP`), `still-redrawing` when the tilemap was still moving,
  `screen-off` with the LCD off. In those cases `screen.text` is sent **empty** and a note
  says why. `screen.kind` (`no-text`/`text`/`dialogue`/`menu`/`yes-no`/`naming-keyboard`) is
  sent regardless, because it comes from the cursor and arrow glyphs, not from words — and
  `no-text` is what makes an empty overworld screen a *fact* rather than a failed read.
- **Bound a "wait for it to settle" gate with a measurement.** `--stability` on a real
  cartridge: main menu / overworld / START menu identical for all 240 frames; a dialogue box
  waiting on the blinking `▼` changes 6 times in 240 frames with a **longest identical run of
  42 frames**, against a 4-frame gate. Without that number the gate is a guess that could
  blind the agent on every dialogue box. The wait is capped at 120 extra frames anyway.
- **Sprites are not in the screen text and never can be** — people, Poké Balls and objects
  are OAM, drawn *over* the tilemap. That is §11's blind spot 1, still open, and the prompt
  now says so explicitly so an empty screen does not read as an empty room.
- **Conformance record** (per the standing convention; this is a *consumer of emulated
  state* defect, like the `wIsInBattle == $FF` one, so point 3 cites the disassembly rather
  than Pan Docs): *symptom* — an agent is shown menu options that do not exist and shown
  quote marks where a health bar is, with the same confidence as good data. *Caught by* —
  **no test ROM, and none can**: the emulator was entirely correct; every one of these is a
  reader bug, which is the same blind spot as the render defects. Now
  `observe::tests::{the_battle_menus_pkmn_option_is_visible,
  the_ambiguous_battle_font_slots_stay_blank, the_naming_screens_end_key_is_not_a_yen_sign,
  text_below_a_menu_does_not_cut_an_item_in_half}` and two end-to-end cases in
  `testharness/agent.rs`, all teeth-proven by reverting the fix. *Contract violated* —
  `constants/charmap.asm`, which states the overlays in its own section headings.
  *Conformance candidate*: **a decode that cannot be sure must say so, not guess** — the same
  principle as "a missing optional report must degrade only the thing it reports".

## Embeddability — the core stands alone (`--no-default-features`)

The captain's requirement: someone must be able to pull this crate into their own UI, tool
or hardware project and get the emulator, with nothing about this machine or this owner in
it. That is now a real, CI-enforced build; it did not compile before.

- **`--no-default-features` is THE EMBEDDING BUILD, and the dependency count is the contract:
  `terminalgb -> blip_buf`, one crate, against 193 on the default build.** `make embed` and the
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
- **GitHub CI cannot protect the wasm build — `make embed` now does (`make check-wasm`).**
  CI's wasm jobs are real, but CI runs on *push* and this project merges into local `main`
  first, so a wasm break on unpushed commits is invisible for days (`c240626` broke
  `Gameboy::audio_enabled` — an unconditional read of the `#[cfg(not(wasm32))]` `sound`
  field — and `1b3e3e7`'s `terminalgb-chiptune` bin imported a module wasm compiles out;
  GitHub CI never saw either, being already red on an *earlier* unpushed-fix breakage, which
  also skips the embed job's wasm cross-check step). Two standing rules from the fix: a
  `pub` accessor over a target-gated field needs a wasm arm (return the honest answer, e.g.
  `audio_enabled() == false`), and **Cargo cannot target-gate a `[[bin]]`** — a bin whose
  feature is in `default` must carry a `#[cfg(target_arch = "wasm32")] fn main() {}` stub.
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

## Two picture modes — `standard` and `identical` (`src/gpu/fifo.rs`, `docs/picture-modes.md`)

- **Reader-facing page: [`docs/picture-modes.md`](docs/picture-modes.md).** Per-test numbers and
  the calibration argument: [`docs/mealybug.md`](docs/mealybug.md) §7. Component-level
  orientation: [`docs/components/ppu.md`](docs/components/ppu.md) §4.0.
- **THE MODES ARE `standard` AND `identical`; THE RUST VARIANTS ARE STILL `RenderMode::{Fast,
  Accurate}`.** That split is deliberate: `Fast`/`Accurate` describe what the two engines *are*
  and are referenced from a large amount of code, docs and history, while `standard`/`identical`
  are what a person types and sees (the captain's own words). `RenderMode::name()` returns the
  user-facing name, `alias()` the pre-rename one. **Both spellings parse everywhere** —
  `parse()` accepts `standard|fast|scanline` and `identical|accurate|fifo` — so every existing
  `config.toml`, `$GB_PPU` invocation and script keeps working, and the `make` targets
  deliberately still say `GB_PPU=accurate` to keep the alias exercised in CI.
- **It is a real, findable mode, not an internal switch, and five surfaces say so**: the TUI's
  **`P` key** (shifted, like `T`/`B`/`A` — `p` is pause and the unshifted letters are
  configurable game buttons), the Status panel's `PPU` row, the Controls list, the `?` overlay,
  and the `ppu` config key whose warning offers every spelling back. Also `GB_PPU`, and
  `Gpu::set_render_mode` for an embedder. Switching is safe mid-frame — the two engines share
  every register and the framebuffer, and the accurate one rebuilds its pipeline at the head of
  the next scanline.
- **THE COST IS QUOTED FROM A CONSTANT THAT IS CHECKED AGAINST A MEASUREMENT.**
  `RenderMode::IDENTICAL_COST` = **2.1**, printed on the Status row (`identical ~2.1x`), in the
  `P` toast and in the `?` overlay — and `testharness/ppuengines.rs` times both engines over 400
  frames of dmg-acid2 on every `cargo test --release` and **fails the build if the constant
  disagrees with what it measured** (teeth proven: setting it to 4.5 turns the harness red).
  This repository has been bitten by a stale performance number before ("audio costs ~8 ms a
  frame" outlived its fix by months); a figure shown to users has to be a measurement.
  Measured here four runs running: 0.221 → 0.461 ms/frame, ratio **2.07-2.09x**. Older notes
  saying 2.13x or 2.17x are other ROMs/boxes, not wrong — hence the `~`.
- **`standard` is still the default and every unsuffixed baseline is measured on it — but the
  accuracy argument for that is GONE.** `identical` used to be behind on two rows; both are
  closed (see "The line-0 STAT phase" below) and it now **loses no row to the default on any
  gate**: in-repo harness 46/47 both with *identical verdicts row for row*, Mooneye
  `acceptance`+`emulator-only` 95 vs **97**/103, `misc` 8/8 both, SameSuite 72/76 both, `cpp`
  4/4 both, Mealybug 3 vs **12**/79. What stands between it and the default is **cost alone**,
  plus the fact that flipping it swaps
  `testharness/{baseline,mealybug_baseline,mooneye_baseline,mooneye_cgb_baseline}.txt`
  with the `*_accurate_baseline.txt` files in one commit. That is a captain-facing product
  decision, not a crew one. **Never re-bless `testharness/baseline.txt` to hide a regression.**
- **The mode is deliberately NOT in the save state** — it is a property of this session's host,
  not of the emulated machine, so loading someone else's state must not switch their renderer.
  `Gameboy::set_render_mode` takes `terminalgb::ppu::RenderMode`; the `gpu` module itself stays
  private, and the crate root's `terminalgb::RenderMode` is a *different* enum (which frontend).
- **`docs/tools/status_numbers.py` reports BOTH modes** and both pages are gated by
  `make docs-check`: the Markdown table has a column each (`same` where a suite has only one
  baseline because the mode does not reach what it measures), and the HTML page carries a
  generated `<!-- picture-modes:begin -->` block. That block is a plain `<table>` on purpose —
  `ROW_RE` rewrites the `.name`/`.score`/`.bar` rows by name, and a second row called "Mooneye
  GB — acceptance" would be an obvious trap.
- **Adding a line to the `?` overlay can silently delete one further down.** The overlay does
  not scroll and is already longer than its popup; a six-line paragraph added here pushed the
  "GAME PICTURE ONLY" line off the bottom and turned
  `the_help_says_the_feed_is_game_only_and_where_to_go_instead` red. Keep additions to two lines
  and re-run that test.
### The line-0 STAT phase — the OAM strobe must ask `LY`, not `mode` (accurate 6/79 → 12/79)

**Full write-up: [`docs/mealybug.md`](docs/mealybug.md) §9.** Mealybug on the accurate engine
**6 → 12 of 79** rows pixel-exact (693,023 → 646,838 px), Mooneye `acceptance` **68 → 69/75**,
`ppu/intr_1_2_timing-GS` FAIL → PASS. The default engine is byte-identical throughout — none
of its five baselines moved.

- **`Gpu::oam_strobe` derived "the OAM scan is running" from `mode == 2`, and `mode` lags `LY`
  by one dot** (the dot loop moves it *after* `next_line`). Since `bus_tick` folds `IF` once
  per M-cycle, one dot late is a whole **M-cycle** late to the CPU. It asks `LY` and the dot
  counter now: `line < VBLANK_LINE && modeclock < OAM_DOTS && !skip_first_oam`, accurate engine
  only.
- **Lines 1-143 were raising TWO STAT interrupts each, and that hid the bug for months.** The
  strobe dropped at the line boundary and rose again a dot later. Invisible to any program that
  services the interrupt at once, because the second edge lands *inside* the first one's
  dispatch, which clears `IF.1` at M4 after it — so the pair collapses into one. **A defect
  that is masked by the very thing that consumes it will not show up in a verdict; count the
  edges.** `gpu::tests::a_visible_line_strobes_the_oam_source_exactly_once` counts them, teeth
  proven by reverting the arm.
- **Line 0 had nothing to collapse into**, because by design it gets no early window (lines
  1-143 rise 4 dots early on the previous line's tail; line 144 keeps dot 0). So line 0 carried
  the whole error: its interrupt arrived **8 dots** after the equivalent point on lines 1-143
  where hardware wants **4**. `m3_bgp_change`'s own source pads its handler by exactly one
  taken `jr` on every line *except* line 0 to cancel that 4 — so the residual was line 0, and
  only line 0, four pixels right on every ROM that syncs off the mode-2 source. Measured on
  both sides: the handler read `LY` at dot **52** on line 0 against **44** on lines 1-2.
- **The obvious fix is wrong and was measured wrong.** Giving line 0 the early window on line
  153's tail puts it at the *same* phase as lines 1-143, where hardware wants it one M-cycle
  later: Mealybug **6 → 3** and `intr_1_2_timing-GS` still red. The placements available at
  M-cycle granularity are +0, +4, +8 relative to lines 1-143; hardware is +4.
- **A PPU-bound register write now commits partway through its own M-cycle**
  (`mmu::ppu_write_commit`, `MemoryManagementUnit::gpu_defer`), because `bus_tick` advances the
  per-dot PPU a whole M-cycle *before* every access. Model read from
  [docboy](https://github.com/Docheinstein/docboy) and reimplemented per `CONTRIBUTING.md`;
  every arm cites the Mealybug row that measures it. Includes the DMG's one-dot `old | new`
  palette glitch. Gated on `RenderMode::Accurate`, so the default path is untouched.
- **`oam_bug` #7 was ALREADY passing** at the parent commit — `docs/mealybug.md` §7.4 named it
  as blocking and was never amended after §8's `ACCURATE_OAM_BUG_PHASE = 0` work fixed it. The
  APU campaign's standing lesson, again: **check the baseline, not a prose note, before
  starting on a named gap.** (`oam_bug@cgb` must stay red — Color silicon has no OAM defect.)
- **NOT EVERY LINE OF A SALVAGED/INHERITED PATCH EARNS ITS PLACE — A/B EACH ONE.** The
  mid-flight work this built on also moved `obj_fetch_dot`'s VRAM reads from dots 3/5 to 4/6
  with no justification attached. Reverting it is **21,996 pixels better**
  (`m3_lcdc_obj_size_change_scx@DMG` 22,190 → **298**, `m3_lcdc_obj_size_change` better on all
  three columns) with no row lost, so it was dropped. An unexplained constant in a patch is a
  measurement waiting to be taken.
- **Any row near 22,500 is a RANK ARTEFACT, not a distance.** One extra distinct shade in our
  frame re-ranks the whole picture and reports ~23,040. `m3_lcdc_obj_size_change_scx@DMG` is
  the worked example both ways: 22,070 on the parent, **298** now — the picture did not improve
  by 21,772 pixels, it stopped being mis-ranked. Its CGB columns are still in that zone. Net:
  5 rows numerically worse, 41 better, none lost pixel-exactness.
- **Closest remaining rows** (§9.6): `m3_lcdc_bg_en_change@DMG` 4 px (one pixel at `x=0`, DMG
  only), `m3_scx_high_5_bits` 12 px on all three columns (**+3 rows** if closed — the biggest
  single prize left), `m3_window_timing@CGB-C` 138 and `m3_bgp_change@CGB-C` 864 (both already
  exact on DMG and CGB-D), `m3_lcdc_obj_size_change_scx@DMG` 298.
- **THE COMMIT PHASES ARE AT THEIR OPTIMUM — DO NOT RE-SWEEP THEM.** Both remaining
  near-misses were swept against `ppu_write_commit` afterwards and `early: 3` wins by a wide
  margin on each (DMG `SCX` 0/1/2/3/4 → 84/77/47/**12**/53 px; DMG `LCDC` 1/2/3/4 → suite total
  671,348/669,696/**646,838**/712,414). `SCX` and `SCY` were split into separate arms to see
  whether they wanted different phases — they do not, and the split was reverted.
  `m3_scx_high_5_bits` reads 12 on **all three** columns while only DMG has an early commit at
  all, which says the same thing from the other side: **what is left is inside the fetcher, not
  in when the write becomes visible.** That is the per-tile calibration `docs/mealybug.md` §8.9
  scopes in weeks.

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
- ~~*"A wrong variable mode-3 length is worse than an honest constant one"*~~ — **withdrawn,
  and both of its examples were misdiagnosed.** `m3_bgp_change`'s near-blank frame was the
  mode-0 deadlock below, not the mode-3 length (it is 2,244 pixels now, and the fast engine's
  5,296 is simply "no mid-line bands at all"); `m3_lcdc_obj_size_change_scx`'s 22,032 was the
  rank-count artefact below, from **four** wrong pixels. The surviving half of that note is
  still true and still worth obeying: **never quote the aggregate alone**, because a handful of
  pixels can move it by tens of thousands.
- **Cost, measured, since the whole two-renderer decision rests on it:** dmg-acid2 0.263
  ms/frame fast vs 0.559 accurate — **2.13x, 3.3 % of a 16.7427 ms frame, 30x real time
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

### Second pass on the accurate engine (`docs/mealybug.md` §8) — 3/79 → 6/79

**Measured on the pinned ROMs: Mealybug rows 3 → 6, pixel distance 1,268,382 → 693,023, and
Mooneye `acceptance` +1 (`intr_2_mode0_timing`).** The default renderer is byte-identical to
its five baselines throughout — everything here is `src/gpu/fifo.rs` plus one STAT signal.
**Re-measure before quoting an absolute Mooneye figure**: this work's delta was taken against
the base it started from, and the nine-machine model work has since moved the totals — at
`e58b651` the accurate engine is **68/75 acceptance, 28/28 emulator-only, 8/8 misc** and the
default is 67/75. Mealybug is unaffected by that work (re-measured, identical).

- **THE BIGGEST ONE WAS NOT AN ACCURACY BUG AT ALL: the accurate engine never left mode 0
  after the LCD came on.** `skip_first_oam` is cleared only by entering mode 3 at dot 80 of
  the restart line, and `do_cycle_accurate`'s "mode 0 has no per-dot behaviour, skip to end of
  line" shortcut swallowed that dot — so the latch survived to the next line, and the next:
  `LY` ticking over with **no mode 2, no mode 3 and no STAT interrupt** until line 144. Every
  Mealybug ROM arms itself on the mode-2 STAT source and then falls through 1200 `nop`s, so
  they all ran off the end of the slide and crashed before drawing anything. Most of "3/79"
  was measuring that. **Lesson to carry: before believing a suite score, check the ROMs are
  running** — `m3_bgp_change` hit its ten-frame breakpoint in 80,668 cycles (should be
  ~767,000) and made exactly one `BGP` write, both visible in a five-minute trace.
- **A fast-forward past a mode is only safe if nothing inside it changes state.** That is the
  general form of the bug; the same shortcut is fine for real HBlank and for VBlank.
- **The mode-2 STAT strobe rises one M-cycle BEFORE its scanline**, on lines 1-143 only. Line
  144 keeps dot 0 (`vblank_stat_intr-GS`) and **line 0 has no early window** — Mealybug's
  `m3_bgp_change` says so in its own source ("line 0 timing is different by 4 cycles") and
  `intr_1_2_timing-GS` agrees. **Only the per-dot engine emits it**: the whole-scanline
  renderer changes `mode` at instruction granularity, so a 4-dot signal there is mixing
  resolutions, and it measures as such (Mooneye unmoved, Mealybug loses `m2_win_en_toggle`).
- **Technique worth stealing: a test that measures the distance between two events delivered
  through the same path has already cancelled that path's latency.** `intr_2_0_timing` times
  the mode-2 interrupt to the mode-0 interrupt, so dispatch latency cancels and the 4-cycle
  error had to be in the PPU signal. That is what stopped this being "fudge the CPU".
- **The OBJ penalty is Pan Docs' algorithm, charged explicitly** (`Gpu::start_obj_fetch`): six
  dots per object plus `max(0, 5 - ((X + SCX) % 8))` for the **first** object on each
  background tile. Deriving it from the fetcher's phase reproduced only the maximum.
  `acceptance/ppu/intr_2_mode0_timing_sprites` is the ground truth — 39 arrangements, and its
  unit is one M-cycle **truncated**, so 65 dots reads as 16. Pinned device-free by
  `the_obj_penalty_matches_mooneyes_table`.
- **An object at `X < 8` draws nothing and still costs its six dots.** `due_sprite` used to
  require `x > lx`, which made every object at `X = 0` free — the first ten rows of that
  Mooneye table exist to catch exactly this.
- **The window's takeover is decided at the OUTPUT**, so it cannot happen before the FIFO has
  a pixel. Checking it during the head-of-line fetch (where `lx` is already 0) let `WX <= 7`
  take over a fetcher with nothing to throw away, and cost nothing; Pan Docs says the window
  always makes mode 3 6 dots longer. `the_window_costs_six_dots_wherever_it_starts`.
- **`savestate::FORMAT_VERSION` is 15** (`Pipeline::obj_wait` + `obj_tile_paid`).
- **THE PIXEL COUNT IS A RANK DISTANCE AND IT IS WILDLY NON-LINEAR.** Ranks are per frame, so
  **one extra distinct shade re-ranks everything and reports ~23,040**.
  `m3_lcdc_obj_size_change_scx` is four wrong pixels and scores 22,070. Read a row near 22,500
  as "wrong shade count", not "nothing works", and never convert the total into a percentage.
  The metric is deliberately unchanged (same shade-rank rule as `tools/conformance/image.py`).
- **Eight rows cannot be compared at all.** The CGB references for `m3_wx_4/5/6_change` and
  `m3_lcdc_win_en_change_multiple_wx` carry **125 distinct luminances** — a Color console
  colourising a DMG cartridge from the boot ROM's title-hash palette — against four or fewer
  everywhere else. That is 26 % of the remaining pixel distance and no PPU work moves it.
- **Blocker C is now measured and it is EIGHT BYTES.** The boot logo tiles `$01`-`$18` buy
  **nothing** (every ROM that would show them either loads the suite's own font or selects the
  `$8800` block); the whole payoff is tile `$19`, the `®` glyph. Writing the boot ROM's eight
  bytes to `$8190` takes the suite **693,023 → 483,005 pixels (−30 %)** and three rows from
  ~22,750 to 66-324. **It is a licence/policy call and has NOT been taken** — see
  `docs/mealybug.md` §8.6 for the case each way. Do not re-run the experiment without reading
  that section; the number is already measured.
- **The last uniform error left is 2-3 pixels at the head of the line.** After the strobe and
  window fixes, `m3_bgp_change` and `m3_window_timing` agree with hardware in shape and sit a
  constant **3 pixels late**; `m3_lcdc_bg_en_change` says **2**, and only on the first
  transition of each pair. Not the interrupt path (that is M-cycle quantised and 3 is not a
  multiple of 4) — it is inside the twelve dots between mode 3's start and the first pixel.

### The fetcher at an object hit, and the CGB stepping the PPU shipped in (accurate 12/79 → 17/79)

**Mealybug `identical` 12 → 17 of 79 rows, 646,838 → 630,266 px, five rows FAIL→PASS and none
lost.** Every `standard` baseline byte-identical; Mooneye 95/103 standard and 97/103 identical,
misc 8/8, SameSuite 72/76, `cpp` 4/4, `testroms` 46/47 all unmoved. Full record:
[`docs/mealybug.md`](docs/mealybug.md) §10, `docs/conformance-notes.md` (newest entry).

- **READ THE LEADERS' REPOS FIRST — this pass cost hours, not weeks, because of it.** docboy
  (**MIT**) and SameBoy (**Expat**, except `iOS/` and `HexFiend/`) are both MPL-2.0-compatible,
  and both already answered the two open questions. **Check the `LICENSE` yourself and record
  it**; learn the behaviour, cite repo + file + symbol, and reimplement in our idiom —
  `CONTRIBUTING.md`'s rule, which `ppu_write_commit` already followed. Mealybug's own
  `src/ppu/*.asm` (MIT) says what each ROM measures and is worth reading before any row.
- **The dot an object is detected is still a FETCHER dot.** docboy starts the object fetch from
  inside the background fetcher's own tick (`ppu.cpp`, `handle_pixel_slice_fetcher_push`: *"the
  first obj prefetcher tick overlaps this tick"*). Ours froze the fetcher on the spot, so every
  tile-number read after an object was one dot late — the whole of `m3_scx_high_5_bits` (12 px
  on all three columns → 0). The charged Pan Docs penalty is unchanged, so mode 3's length and
  `intr_2_mode0_timing_sprites` do not move.
- **The FULL structural model (fetcher runs for the whole penalty) is docboy's shape and is
  WORSE here — do not "finish the job" without doing the calibration.** Same +3 rows, but
  +64,784 px elsewhere against +151 for the single trigger tick. Our `obj_wait` is Pan Docs'
  *formula*, not the fetcher's real remaining time, so running the fetcher through it
  double-counts. Making the wait structural is the per-tile calibration `docs/mealybug.md` §8.9
  budgets in weeks.
- **THE COLOR CONSOLE'S PPU SHIPPED IN REVISIONS TOO.** `mode::PpuRevision` (`CgbC`/`CgbD`,
  default `CgbD`): CPU CGB D and later **latch `SCY + LY` at the tile-number fetch** and reuse it
  for both bitplanes, so the DMG's bitplane desync does not exist there (SameBoy
  `Core/display.c` `GB_FETCHER_GET_TILE_T1`, *"cached on the CGB-D and newer, so it cannot be
  used to mix tiles together"*; docboy `bg_prefetcher_get_tile_0` says the same and models D+
  only). Plus one measured here and in neither of them: **a palette write is seen one dot later
  on a CPU CGB C.**
- **It is deliberately NOT `sound::CgbRevision`.** The APU's boundary is 0/B against C-and-later;
  the PPU's is C-and-earlier against D-and-later — one enum would name four steppings to express
  two facts — and `src/sound.rs` is not compiled on `wasm32` while the PPU always is. Both read
  `$GB_CGB_REVISION` and accept the same tags, so the user still has one knob. **Not serialised**
  (machine configuration, same call `Sound::cgb_revision` makes); `savestate::FORMAT_VERSION` is
  **18** for the latched `SCY + LY`, which *is* live pipeline state.
- **Before this we were a machine that never shipped** — CGB-D palette timing with CGB-C `SCY`
  behaviour. Default is `CgbD` because the APU already targets CGB-E, SameBoy's default is
  `GB_MODEL_CGB_E` and docboy models D+.
- **`examples/mealybug` runs THREE machines now, one per reference set.** The two Color reference
  sets are different CPU steppings: seven ROMs disagree between them by **8,265 px**
  (`m3_scy_change` alone by 6,217), so scoring both columns against one Color machine can never
  pass both. Same lesson `examples/samesuite.rs::revision_for` records for SameSuite's
  contradictory `-cgb0`/`-cgbB` pair. **When a suite ships more than one reference for a ROM it
  is telling you there is more than one machine.**
- **Diagnostic habit that closed two named rows in minutes: compare the suite's own reference
  sets against each other.** `m3_window_timing@CGB-C` was 138 px and `m3_bgp_change@CGB-C` was
  864 — and the C-vs-D *references* differ by exactly 138 and 864. If the gap between two
  references equals your error, the emulator is right and the runner is wrong.
- **Read the failing FRAME, not the pixel count.** `m3_scx_high_5_bits` fills the background with
  `'A'`, `'B'`, `'C'`… one letter per tile column, so decoding the frame as ASCII says which
  `SCX` each tile's fetch used — we drew `'S'` (column 18) where hardware drew `'C'` (column 2).
  The count alone could not have said that. Same trick as `GB_SCOUT_SCREEN=1` and the Gen 1
  tilemap readers.
- **`m3_lcdc_bg_en_change@DMG` (4 px) is still open and docboy's `LX == 8` exception is NOT the
  answer** — *"On DMG … 1 T-cycle delay between the real LCDC and the LCDC the PPU sees, both
  for LCDC.OBJ_ENABLE and LCDC.BG_WIN_ENABLE. LX == 8 seems to be an exception"*
  (`pixel_transfer_lx8`). We already model that delay write-side as `LCDC_DMG_STAGE_BITS`;
  moving it read-side with the exception was implemented and measured — **suite total identical
  to the digit, 646,989 either way** — and reverted. Do not re-implement it.
- **The eight rows that got worse are all the object-trigger dot**, and they are all
  mid-line-write rows that also place objects (`m3_lcdc_win_map_change@DMG` 644 → 1,028 is the
  largest). ~800 px spread over eight rows against three exact rows and a 15,000-px correction.

### VRAM/OAM access blocking, and the two mode edges that are not one edge (Mooneye accurate 97/103 → 98/103)

**`acceptance/ppu/lcdon_timing-GS` and `lcdon_write_timing-GS` are GREEN on `identical`** — the
two rows this file spent a long time recording as needing the rewrite. `intr_2_mode0_timing`
went the other way and it is the same one M-cycle as the five below, not new code. Every
`standard` baseline and every Mealybug baseline **byte-identical**; SameSuite 72/76, `cpp` 4/4,
`testroms` 46/47, 656 lib tests, savestate/battery/audiocapture/bootrom/ppuengines all green.
Full record with citations: [`docs/conformance-notes.md`](docs/conformance-notes.md) (newest).

- **MODE-BASED VRAM/OAM BLOCKING IS NOW ON — FOR THE PER-DOT ENGINE ONLY, AND THAT GATE IS THE
  WHOLE POINT.** The standing warning ("do not add blocking alone; a fixed 172-dot mode 3 would
  drop writes a game placed legally in HBlank") is still exactly right **for the batched
  renderer**, and all four predicates (`Gpu::cpu_can_read_vram` / `cpu_can_write_vram` /
  `cpu_can_read_oam` / `cpu_can_write_oam`) return "reachable" unconditionally for
  `RenderMode::Fast`. The per-dot engine has both halves the rule needs — a mode-3 length its
  fetcher produces, and `bus_tick` standing it on the exact dot of the access — so for it the
  warning does not apply. mGBA (**MPL-2.0, the same licence as this repo**) spells the base rule
  `if (gb->video.mode != 3)` / `if (gb->video.mode < 2)` in `GBLoad8`/`GBStore8`,
  `src/gb/memory.c`.
- **Gate blocking on `bus_tick_enabled`, the existing "a CPU instruction is running" test.**
  `debug_peek` routes through `rb` with the tick suppressed; block without that gate and every
  plugin, the MCP server and the trainer panel read `$FF` out of VRAM at random.
- **A READ IS BLOCKED ONE DOT BEFORE A WRITE IS.** `lcdon_timing-GS` and `lcdon_write_timing-GS`
  are the same nineteen offsets read and written, and their tables disagree at exactly the dots
  the bus changes hands: at dot 80 of a normal line a VRAM *read* returns `$FF` while a VRAM
  *write* still lands, and the same for OAM at dot 0 and dot 80. Only the acquiring edge is
  asymmetric — the release at the end of mode 3 is shared. That is why there are four
  predicates and not two. **Collapse any pair and the row goes red.**
- **The mode-3 → mode-0 transition belongs at the TOP of the first mode-0 dot**, the same edge
  convention mode 2 → 3 already used; ours was at the bottom of the last mode-3 dot, and since
  the caller folds `IF` and reads registers once per M-cycle, one dot is a whole M-cycle to the
  CPU. `lcdon_timing-GS` reads `STAT` at dot 252 and requires mode **3**.
- **An interrupt source and the register that reports it are not obliged to be the same edge.**
  Mooneye ships the same interval twice — `intr_2_0_timing` to the mode-0 *interrupt* (dispatch
  latency cancels) and `intr_2_mode0_timing` to the first `STAT` read that *reports* mode 0 —
  and the answers differ by one M-cycle. `Gpu::hblank_strobe` is that split; SameBoy models the
  same shape with a `mode_for_interrupt` field distinct from the `STAT` mode bits
  (`Core/display.c`).
- **The LY==LYC comparator blacks out for the dot `LY` changes** (`Gpu::lyc_signal`). It is a
  *suppression*, not a hold: at the next line's dot 0 with a stale `LYC` the flag must read
  clear too. mGBA's `_endMode0` carries the comment *"LYC stat is delayed 1 T-cycle"*; SameBoy
  keeps a separate `ly_for_comparison` set to `-1` over the same window.
- **`fifo::mode3_length`'s helper measures the FETCHER's length, not the window `STAT` reports.**
  They now differ by one dot on purpose; the helper subtracts it. Do not "fix" the 172 to 173.
- **THE MODE-2 STROBE IS ASSERTED INSIDE AN M-CYCLE, AND THAT IS WHY A HALTED CPU IS ONE
  M-CYCLE SLOWER TO SEE IT.** `OAM_STROBE_LEAD` used to be recorded here as "the one contested
  constant left, a knob not a model", because at `4` Mooneye `acceptance` was 70/75 with
  Mealybug 17/79 and at `0` it was 75/75 with Mealybug 6/79 — eleven rows lost to a *uniform
  four-pixel shift*. **Both are hardware ground truth and the constant was never the
  disagreement.** Mooneye's five `acceptance/ppu/intr_2_*` take that interrupt out of **`HALT`**;
  Mealybug's ROMs take it out of a **`nop` slide** (its `lcdc_handler` runs off a 1200-`nop`
  slide, and its own comment pads every line but line 0 by one taken `jr`). Every other
  interrupt in the machine — VBlank, the mode-0 and mode-1 STAT sources, the timer, the serial
  port — flips on an M-cycle boundary, so a CPU sampling once per M-cycle sees it at the same
  instant whatever it is doing. The OAM strobe does not, and the two CPUs are one M-cycle apart
  on it. `Gpu::take_stat_irq_mid_mcycle` + `Cpu::mid_mcycle_request_reaches_halt_late` is that
  M-cycle. **Result: `acceptance` 75/75 AND Mealybug 17/79, with the lead where it always was,
  every Mealybug per-row pixel count byte-identical and every `standard` baseline untouched.**
- **The two references that name the mechanism, and it is worth reading both:** **docboy** (MIT,
  `src/docboy/docboy/cpu/cpu.cpp`) tabulates interrupt latency per source, per halt state **and
  per T-cycle of the M-cycle** — its DMG STAT row is `{1,1,1,2}` running against `{1,2,2,2}`
  halted, identical on the cycle's first T and one M-cycle longer part-way through; **SameBoy**
  (Expat, `Core/sm83_cpu.c` `GB_cpu_run`) reaches it from the other side, advancing the clock
  around its halt-path `IE & IF` sample and not around the run-path one. Neither was copied;
  both are cited at `Cpu::mid_mcycle_request_reaches_halt_late`.
- **A BLANKET "+1 M-cycle on the HALT wake" IS WRONG AND WAS MEASURED WRONG.** It fixes all five
  `intr_2_*` and breaks `di_timing-GS`, `halt_ime1_timing2-GS`, `intr_1_2_timing-GS` and
  `hblank_ly_scx_timing-GS` — and the reason is exact, not bad luck: those four wake from `HALT`
  on **boundary-aligned** sources (VBlank, the mode-1 STAT source, the mode-0 STAT strobe), and
  two of them measure *halt-dispatch minus run-dispatch* off the same VBlank flag, which pins
  that difference to zero outright. The delay belongs to the *request*, not to `HALT`.
- **Two other places the M-cycle was ruled out of, each measured, so nobody repeats them:** not
  `ppu_write_commit` (it spans one M-cycle and has one dot of slack at `early: 3`); not the
  pixel phase (the fetcher pushes pixel 0 on dot 92 and pixel 159 on dot 251, so mode 3's 172
  dots are full — and sweeping the LCD-on PPU phase 0..7 moves Mooneye while leaving Mealybug
  untouched at every value, because **both suites anchor on the strobe and are invariant to a
  global PPU-vs-CPU phase shift**). That invariance is the fact that localises the error: it
  cannot be in the phase, so it is in how the two *CPU states* consume the same request.
- **Any `OAM_STROBE_LEAD` in 1..=4 is the same model on this engine** — the whole content of the
  constant is "inside an M-cycle rather than on its boundary", so SameBoy's *"1 T-cycle"* and
  docboy's dot 452/453 and our `4` agree. Only `0` is a different machine. Do not "correct" it
  to 1 thinking it changes anything, and do not set it to 0.
- **THE DIAGNOSTIC THAT CRACKED ALL OF THIS: trace the emulator against the ROM's own expected
  table, not its pass/fail line.** `lcdon_timing-GS` ships a 24-entry `cycle_counts` table beside
  four 24-entry expectation tables. Printing `(line, modeclock, mode)` at every `STAT` read and
  laying it beside those tables turned "one row is red" into "every entry agrees except
  `modeclock == 252`" in one run. Same family as `GB_SCOUT_SCREEN=1`; do this before changing a
  constant.
- **A knob that trades tests one-for-one is not modelling anything.** The lead bought
  `intr_2_mode0_timing` + `intr_2_oam_ok_timing` and cost `intr_2_0_timing` +
  `intr_2_mode3_timing`, exactly, in both directions — which is the signature of a constant
  absorbing an error somewhere else, and is what sent the search to the mode-0 end.

### The head of a scanline: the discard, and where the window really starts (Mealybug identical 17/79 → 26/79)

**Full record, including the corrected denominator, `docs/mealybug.md` §11.** Suite total on
the `identical` engine 630,266 → **498,206 px**; `standard` byte-identical throughout, Mooneye
95/103 + 8/8 (**103/103** on `identical`), SameSuite 72/76, `cpp` 4/4, in-repo harness 46/47 on
both engines.

- **42 OF THE 53 FAILING ROWS CANNOT BE WON BY PPU WORK, AND ANYONE PLANNING AGAINST THIS SUITE
  SHOULD PLAN AGAINST ELEVEN.** 27 rows need the boot ROM's `®` glyph (a captain decision, not a
  crew one — §8.6), 8 compare our greyscale against a colour reference (§8.7), and **7 are a
  suite ROM/reference mismatch** — the `*2` ROMs ship a DMG build (`$0143 = $00`) and are scored
  against a CGB-mode reference. That last one is new and it is *proved*: our frame is
  structurally identical to the reference and differs only in colour (its black 4,792 and white
  15,379 match ours to the pixel; its 2,729 green + 140 pink are our 2,869 grey), and patching
  the flag in a scratch copy drops the row 18,108 → 2,869. **Do not close them by forcing the
  flag in the runner** — that is the runner measuring a machine the cartridge does not ask for.
  **`grep -l '$19'` finds ten ROMs for block A and only nine are blocked**: `m3_wx_4_change_sprites`
  references the glyph and then fills every tile with `$FF`, so it never reaches the screen.
  Referencing a tile is a first pass, not a verdict.
- **THE `expected/` IMAGES ARE THE AUTHOR'S OWN EMULATOR'S SCREENSHOTS, NOT PHOTOGRAPHS.** His
  README says so plainly ("screenshots from my Game Boy emulator (which I believe to be
  correct)", with blurry hardware photos in a separate `photos/`). Still somebody else's
  judgement — which is the property that makes the baselines trustworthy — but this repo said
  "photographed off real hardware" in four places and now does not.
- **`SCX & 7` is read on every discarded dot, not latched when mode 3 began.** Five M-cycles of
  interrupt dispatch plus a jump put every Mealybug handler's `SCX` write around dot 8 while the
  first pixel is not shifted until dot 12, so a latch uses the *previous* line's value and the
  whole stair pattern comes out one scanline stale. The counter wraps at 8 exactly as SameBoy's
  `(position_in_line & 7) == (SCX & 7)` does, so lowering `SCX` mid-discard costs a further lap.
- **A `WX = 0` window costs one extra dot when the line has fractional scrolling** — the ROM's
  own header says it, SameBoy charges it right after `fifo_clear`. Here the takeover simply
  consumes its dot, so it needs no state.
- **THE WINDOW STARTS ON THE PIXEL `WX - 7` AND ON NO OTHER — an equality, never a threshold.**
  Indistinguishable for the first activation and wrong for every later one: after `LCDC.5` has
  gone low and the background resumed, `lx` is already past the old `WX - 7` and a threshold
  restarts the window on the next dot however `WX` was left. Mealybug's own WIN_EN notes state
  the rule; SameBoy spells it `WX == (uint8_t)(position_in_line + 7)`.
- **AND ITS TILE GRID IS ANCHORED AT `WX - 7` EVEN WHEN THAT IS OFF THE LEFT EDGE**, so for
  `WX <= 6` the takeover happens **inside the head-of-line prologue** (`Gpu::prologue_pos` — the
  eight output positions left of pixel 0) and the first tile's off-screen pixels are shifted out
  and dropped a dot each. Both halves or neither: dropping them for free brings the next push
  forward and shortens the line, which was measured and costs `m3_window_timing` and
  `m3_window_timing_wx_0` outright. `prologue_pos`'s dummy-fetch dots must stay **negative** or
  `WX = 7` matches `lx == 0` on dot 0 and truncates the prologue. Worth `m3_wx_4_change@DMG`
  **12,409 → 229 px**.
- **THE WINDOW PIXEL-INSERTION GLITCH: once `WY == LY` has latched for the frame, a background
  push landing on the pixel the window starts at supplies one transparent pixel instead of the
  tile and postpones the real push a dot.** It does not need the window to be drawing and the
  window's own first tile is exempt (docboy `handle_pixel_slice_fetcher_push`; SameBoy
  `insert_bg_pixel`). A push only ever lands on the eight-pixel grid, so it is reachable on one
  line in eight — which is why `m3_wx_4_change@DMG` was 229 pixels and not 23,040.
- **AND THE WINDOW'S OFF-SCREEN PIXELS MUST NOT SHIFT THE SPRITE FIFO.** An object's `X` is a
  *screen* coordinate, so consuming its leading pixels against the `7 - WX` pixels of the
  window's first tile that fall off the left edge costs an object at the left edge exactly that
  many of its own. Worth three rows on `m3_wx_4_change_sprites`. The `SCX & 7` discards
  deliberately keep shifting it — nothing measures them and `m3_bgp_change_sprites` is the row
  that would notice.
- **An object's row index is MASKED, not clamped, and Y-flip is an XOR against the same mask.**
  `LCDC.2` is read at the bitplane fetch, so the scan can accept an object as sixteen tall and
  the fetch find it eight — a row of 8-15 with three bits to hold it. Hardware has no comparator
  there. Worth two rows, and it is the worked example of the rank metric: **sixteen wrong pixels
  scored 22,154**, because one extra distinct shade re-ranks the whole frame. Read a row near
  22,500 as "wrong shade count", never as "nothing works".
- **The measurement trap that costs an hour: `target/mealybug/*.png` holds whichever engine ran
  last.** Scoring `identical`'s frames against `standard`'s numbers invents both regressions and
  gains. `tools/mealybug/characterise.py` re-renders before it reads for exactly this reason
  (`MEALYBUG_NO_RUN=1` opts out); `tools/mealybug/rowview.py` prints one row's frame beside its
  reference as shade digits, and `gpu::fifo::tracing::mid_line_trace` (ignored by default) is
  the dot-by-dot trace of one ROM. Every finding above came out of those three.
- **Sweeps already done — do not repeat them.** `ppu_write_commit`'s scroll phases are at their
  optimum (DMG `early` 0/1/2/3 → `m3_scy_change` 6313/5128/965/**257**; Color end-of-cycle/1/2/3
  → **560**/4699/5418/5453), which also settles the author's "a CGB `SCY` write lands 2 T-cycles
  later" note at our M-cycle granularity. The object fetch's two plane-read dots were swept over
  six pairs; `3/6` buys a third row and **was not taken**, because low-on-the-first-dot and
  high-on-the-second is an asymmetry no source justifies — that is the per-test fudge constant
  to avoid. The measured table is in `docs/mealybug.md` §11 and `data/gameboy-mealybug-w8/`.
- **Cost: +2.6 % on the `identical` engine's own figure** (0.4824 → 0.4948 ms/frame, interleaved
  A/B, four rounds), i.e. **0.07 % of a 16.743 ms frame**, and the `standard` engine is
  unmoved (0.235). `RenderMode::IDENTICAL_COST` is still 2.1 and `testharness/ppuengines.rs`
  still agrees.
- `savestate::FORMAT_VERSION` 19 → **22**: v20 because `Pipeline::discard` changed meaning
  (a count, not a countdown — same width, same place, so nothing about the layout moved), v21
  for `Pipeline::win_skip`, v22 for `Pipeline::win_fresh`.

## Optional user-supplied boot ROM (`src/boot.rs`) — the answer to Blocker C

**Captain's decision, 2026-08-14: do NOT embed the boot ROM's `®` glyph; support an optional
boot ROM the user supplies instead.** The reasoning is worth keeping because it draws a line
this repo will need again: a register value or a counter phase is a *fact about hardware* and
not copyrightable expression, which is why `set_initial` transcribes the whole `$FF00-$FF7F`
map by hand; **a glyph bitmap is expressive content** and is not ours at any size. The repo is
public, MPL-2.0, and has a standing requirement to be freely distributable.

- **Where it looks:** `${XDG_CONFIG_HOME:-~/.config}/terminalgb/{dmg_boot,cgb_boot}.bin` (256
  and 2304 bytes), with the pre-rename `gameboy/` fallback, and `$GB_BOOT_ROM=<path>` on top.
- **Three rules, not negotiable: never bundle one, never fetch one, fail soft and silently.**
  No file, unreadable, wrong length or wrong console → exactly the HLE machine a fresh clone
  gets. `boot::explain` returns the reason *only* when the user evidently meant to supply one
  (an explicit `$GB_BOOT_ROM`, or a file at the config path that does not work), so a machine
  with no boot ROM says nothing on every launch.
- **A boot ROM belongs to ONE console.** A DMG image on a Color machine is declined, not
  approximated — it would map over the wrong addresses and hand over the wrong registers. The
  SGB's own boot ROM is not modelled, so an SGB always takes the HLE path.
- **THE GATED SUITES MUST NEVER PICK ONE UP AUTOMATICALLY.** Only `screen::tui::load_session`
  auto-loads; `examples/mealybug` takes `--boot-rom <path>` explicitly and **refuses to be
  combined with `--baseline`**, and no other harness or scout looks at all. A score that
  depended on what happens to be in somebody's `~/.config` would not be a gate. If you add a
  boot ROM to another runner, add the same refusal.
- **Layering:** `BootRom` is pure bytes and is what the core takes
  (`Cpu::new_with_boot_rom`, `Gameboy::new_with_boot_rom`). `boot::find`/`load`/`explain` are
  the *host* half — they read `$GB_BOOT_ROM`, `$XDG_CONFIG_HOME` and `$HOME` — and **nothing in
  the core calls them**, which is what keeps the embedding claim above true. `make embed` still
  passes with `HOME`/`DISPLAY` unset.
- **The mechanism:** the image overlays `$0000-$00FF` (plus `$0200-$08FF` for a CGB image, with
  the cartridge header showing through the hole it reads its title from), the machine starts at
  **reset** (`Registers::at_reset`, every register zero, `PC = SP = $0000`) and `set_initial` is
  **not** run — the boot ROM produces the handoff itself. `$FF50` bit 0 unmaps it for good;
  the register reads `$FE` while mapped and `$FF` after, which is what `boot_hwio-dmgABCmgb`
  has always seen. Writes in `$0000-$7FFF` still reach the mapper.
- **`savestate::FORMAT_VERSION` is 17** (`boot_active`). The *image* is never stored — it is
  the host's file, like the cartridge — so a state taken during the boot animation refuses to
  load into a machine that has none rather than resuming into the cartridge at `PC=$0000`.
- **Both Mealybug numbers, measured through the shipped path** (`GB_PPU=accurate`):
  **693,023 px HLE → 483,005 px with a boot ROM (−30 %)**, `m3_obp0_change` 22,750 → **108**,
  `m3_lcdc_obj_en_change` 22,894 → **256**, `m3_scx_low_3_bits` 22,500 → **324**. **No row
  flips** — the boot ROM removes a *content* error the emulator could never fix, and what
  remains in those rows is mid-scanline timing, which is ours.
- **There is no boot ROM on this machine and we may not fetch one**, so the right-hand column
  was produced with hand-written 256- and 2304-byte stand-ins in `/tmp` that reproduce the
  post-boot state and then `ldh [$50],a` themselves out — never committed, and cross-checked
  against an earlier experiment that injected the same state directly into the MMU (they agree
  to the pixel). If you need to re-measure, write the stand-in again; do not go looking for a
  dump in the repository, and do not download one.
- **`testharness/bootrom.rs`** (`cargo test --release --test bootrom`, in the default `cargo
  test`) drives the whole contract with a 256-byte image written in the harness itself: the
  overlay, execution from `$0000`, VRAM the HLE machine cannot reconstruct, the `$FF50` unmap,
  the handoff to `$0100`, both fail-soft paths and the save-state refusal. Teeth proven by
  making `read_rom` ignore the latch — five of the thirteen checks go red.

## MCP server (`src/mcp/`, `terminalgb-mcp` binary, `mcp` cargo feature)

- **Full contract, install steps for Claude Desktop and Claude Code, every flag, the measured
  numbers and the honest limitations: [`docs/mcp.md`](docs/mcp.md).** Behind the `mcp` cargo
  feature, adds **no third-party crate**: the JSON codec is the agent plugin's, PNG and
  base64 are `termimage`, and every crate it pulls in is in this repository.
- **It is a thin wrapper over the agent plugin, and must stay one.** `observe` calls
  `terminalgb_plugin_agent::observe::{Gen1Observer, Screen}`; `run_actions` drives
  `terminalgb_plugin_agent::action::Executor` unchanged; the JSON codec is that crate's too.
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
  REPO=$(git rev-parse --show-toplevel)          # absolute paths only — see below
  env -u WAYLAND_DISPLAY DISPLAY=:0 wezterm-gui --config enable_wayland=false \
      --config initial_cols=150 --config initial_rows=45 --config font_size=13 \
      --config enable_kitty_keyboard=true start --cwd "$REPO" \
      -- "$REPO/target/release/examples/terminal" "$REPO/target/repro/<rom>"
  WID=$(DISPLAY=:0 xprop -root _NET_CLIENT_LIST | grep -o '0x[0-9a-f]*' | tail -1)
  DISPLAY=:0 import -window $WID shot.png
  magick shot.png -resize 1600x -strip -colors 128 PNG8:docs/shots/<name>.png
  ```
- **A SPAWNED TERMINAL STARTS IN `$HOME`, NOT THE REPO — EVERY PATH ON A `wezterm-gui …
  start --` LINE MUST BE ABSOLUTE, AND `--cwd <absolute repo path>` IS NOT OPTIONAL.** A
  relative binary path fails with `Unable to spawn /home/<user>/./target/release/examples/
  terminal because it does not exist`; a relative ROM or save argument fails one step later
  and much less obviously, because the window opens and the emulator reports the missing
  file inside it. This is a **repo-wide trap that has caught several people**, which is why
  both screenshot recipes in this file and in `docs/rendering.md` now derive `$REPO` from
  `git rev-parse --show-toplevel` rather than writing `./target/…`. Same rule for the ROM:
  stage it into `target/` first (the MBC writes a `.sav` beside it) and pass the absolute
  staged path.
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
sweep-shadow fix `52d98f5`) → 71 (SGB) → 72 (the square frequency-reload latch), and
Mooneye 86 → 88 → 95 (the console revisions). **Measured on `main` plus both:** testroms
**46/47**, Mooneye **95/103** (acceptance 67/75, emulator-only 28/28), Mooneye CGB misc
**8/8**, SameSuite **72/76** (apu 65/69, dma 4/4, ppu 1/1, sgb 2/2), `cpp` **4/4**, Mealybug
**3/79**. The one-liner that produces all of them without a full run:

**Every number above is the DEFAULT `standard` picture mode.** `identical` is scored separately
and differently — measured on this branch, `GB_PPU=identical` gives Mooneye **97/103**
(acceptance **69/75**) against the default's 95/103 (67/75), and Mealybug **17/79** against
3/79 — so **add the mode to any figure you quote**, and read
`testharness/{mooneye,mealybug}_accurate_baseline.txt` (not the unsuffixed files) for it. The
baseline filenames still say `accurate`; the mode is `identical`. Both spellings parse.

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

## Chiptune renderer — the APU driven with no CPU (`crates/terminalgb-chiptune/`, `src/chiptune.rs`)

**Full guide: [`docs/chiptune.md`](docs/chiptune.md).** Instructions in (a score, a `.vgm`
log, or register writes with timings), a `.wav` out, through the *emulated* APU rather
than a synthesiser. `cargo run --release --bin terminalgb-chiptune -- <file> -o out.wav`;
`make test-chiptune` is the gate; `make chiptune-demo` renders the shipped phrase.

- **The APU can be driven directly, with no CPU, MMU or cartridge in the picture** —
  `Sound::new()`, `set_player`, `set_mode`, `catch_up`, `wb`, `do_cycle`, `rb` are the
  whole interface, and the frame sequencer is internal to `Sound` (it runs off `next_time`,
  not off the timer). That is the single fact the tool rests on. Writes go
  `catch_up()` **then** `wb()`, exactly as `MemoryManagementUnit` does on every APU access.
- **DO NOT advance the APU by large steps.** `Sound::do_cycle` calls `do_output()` **once**
  per call when `time >= output_period`, because it is written for a CPU that never
  advances it by more than one instruction. Step it by 1024 T-cycles and `blip_buf` panics
  with "buffer size was exceeded" / an out-of-bounds index — the overshot deltas land on
  top of a block that has not been read yet. `terminalgb_chiptune::STEP_CHUNK` is **64**
  (three times the longest instruction). Chunk size is otherwise *invisible*: channels are
  evaluated lazily over exact time ranges, so it changes not one sample.
- **`Sound::new()` reads `$GB_CGB_REVISION`** (`CgbRevision::from_env`). Anything claiming
  determinism must call `set_cgb_revision` explicitly; `RenderOptions` pins `CgbC`.
- **Layering, and why it is this way round:** `crates/terminalgb-chiptune` has **zero
  dependencies and does not depend on `terminalgb`** — the chip reaches it through an `Apu`
  trait the host implements (`src/chiptune.rs`, ~60 lines). That is not tidiness, it is the
  only acyclic arrangement: the CLI and the MCP tool live in `terminalgb`, so `terminalgb` must
  depend on the chiptune crate. Identical reasoning to `terminalgb-plugin-api`/`MachineAccess`.
  It is **not a plugin**: a `Plugin` is handed a machine during a frame the emulator decided
  to run, and there is no machine here.
- **`terminalgb_chiptune::wav` is now the workspace's ONE WAV encoder.** It is
  `testharness/audiocapture.rs`'s former private RIFF writer, moved byte-for-byte —
  `testharness/audio_baseline.txt` hashes the *interleaved PCM*, so any drift in `to_i16`
  or the 44-byte header moves a baseline that has nothing to do with music. A unit test
  pins the header field by field; the audio harness was run before and after the move and
  every hash was unchanged. `audiocapture` therefore carries
  `required-features = ["chiptune"]`. Agreed with the music-mode crew rather than
  discovered — do not add a second encoder.
- **Verdicts are properties of the audio, never a hash of our own output**
  (`testharness/chiptune.rs`, 27 checks, ~2 s, no ROM, no device): the envelope steps every
  3/64 s; an upward sweep that overflows past 2047 **switches the channel off**; the 7-bit
  LFSR repeats every 127 bits at 3.876 ms (measured autocorrelation 0.995) and the 15-bit
  one does not repeat at all; 50% duty is odd harmonics only, 25% and 75% share a magnitude
  spectrum. Each expected number comes from Pan Docs. Determinism is asserted *and* its
  converse (a changed note must change the samples).
- **Pitch quantisation is the headline answer for a composer, not a rounding error.** The
  period register is 11 bits, so C5 is **524.288 Hz** on a Game Boy against 523.25 in equal
  temperament, and `square_period`/`square_hz` report what the chip will really produce.
  **Channel 3 counts at half the square channels' rate** (`65536/(2048-x)`, not
  `131072/(2048-x)`) — using the square formula for the wave channel transposes it an
  octave, which is the classic Game Boy bug.
- **VGM was investigated and adopted, and it was the cheapest of the three front ends.**
  `0xB3 aa dd` writes `dd` to Game Boy register `aa`, and **register `0x00` is `$FF10`** —
  so a VGM *is* a `Track` and importing one involves no interpretation. Unknown commands are
  skipped by their documented operand lengths so the Game Boy part's timing stays exact.
  `.vgz` is detected by its gzip magic and reported with `gunzip` rather than mis-parsed
  (no DEFLATE decoder, no dependency). We export VGM too, so a phrase can be checked
  against a second implementation — a renderer only checkable against itself is not
  checkable.
- **`--no-default-features` must never inherit this.** The crate is an optional dependency;
  CI's "the core has exactly one third-party dependency" gate is measured on that build and
  is unmoved.
- **Nothing here writes `DIV`**, so the 512 Hz frame-sequencer phase is never reset. A score
  cannot ask for it and a `.vgm` cannot express it; `Sound::div_write` is the hook if it is
  ever wanted.
- **`cargo test` intermittently dies with a wall of "the crate X requires panic strategy
  `abort` which is incompatible with this crate's strategy of `unwind`" — RE-RUN IT.** Not
  caused by the chiptune work: it was reproduced on an untouched tree before any of it
  existed, and it has since hit `--test audiocapture` alone and a whole-suite run, passing on
  the retry each time with no change in between. The cause is the same one behind the
  "output filename collision" warnings every build prints: `[lib] crate-type = ["cdylib",
  "rlib", "staticlib"]` makes three targets share one output path, and with
  `[profile.release] panic = 'abort'` the `abort`-built rlib is occasionally picked up by a
  test target, which cargo builds `unwind`. Cargo's own warning says it "may become a hard
  error in the future". Do not go looking for it in your own change, and do not "fix" it by
  removing `panic = 'abort'` — that is load-bearing (see the battery section: a panic is
  process death *and* the loss of unwritten cartridge RAM). A real fix is giving the lib
  targets distinct names, which is a repo-wide change nobody has scheduled.
  **It is a stale artefact, not your source, and what clears it is forcing the LIB to
  rebuild.** Measured 2026-08-14, two different shapes: `--test battery` failed on three
  consecutive retries naming `terminalgb_plugin_battle_control`, and
  `cargo clean -p terminalgb-plugin-battle-control` let it pass first time; `--test
  basicterm` then failed the same way naming **every** crate in the graph, `cargo clean -p`
  did *not* help, and checking out another commit and back — which rebuilds the lib — did.
  Same test, same source, green either side of the rebuild. Do not read it as a
  regression.
  **It has a DETERMINISTIC trigger, and three more symptom shapes than this note recorded**
  (measured on `2e0da0e`, twice): `cargo build --release --all-targets` fails outright, with
  13 errors, while `--lib`, `--examples`, `--bins` and `--tests` each build clean separately
  and `cargo clippy --all-targets` is clean *and prints no collision at all* — because clippy
  uses the **dev** profile, which sets no `panic` key, so there is only one strategy and one
  rlib. Besides the panic-strategy wall, the same cause produces `` the trait bound `X: Y` is
  not satisfied `` with rustc's own *help* listing `X` as implementing `Y`; ``perhaps two
  different versions of crate `X` are being used?``; and `undefined reference to` a symbol
  that plainly exists. Full entry, with the verbatim strings for searching:
  [`docs/pitfalls/builds.md`](docs/pitfalls/builds.md).
  **A FIFTH symptom shape, and it is the most misleading one yet: a DOCTEST failing with
  `` unresolved import `terminalgb::<module>` `` plus rustc's own note ``found an item that
  was configured out``, naming a `#[cfg(feature = …)]` on a feature that IS enabled.** Seen on
  `src/link/transport.rs`'s `bgblink` doctest during a plain `cargo test --release`; it reads
  exactly like somebody broke the feature gating, and it is the wrong rlib again.
  **`cargo test --release --doc` on its own passes, on the same tree, in three seconds** — that
  is the cheap discriminator, and it is worth running before believing any feature-gate error
  from a whole-suite run. The whole-suite run then failed the *next* time with the ordinary
  panic-strategy wall, from the same tree: the two shapes alternate.
- **The renderer tracks the emulator's accuracy, and that was demonstrated by accident.**
  Rebasing across `7556dde fix(apu): a square's frequency reload is latched late` changed
  the demo render in exactly the places it should: **0.0 %** of the scale and arpeggio
  samples moved, 1.8 % of the falling sweep and **43.4 %** of the rising sweep — the sweep
  unit being the only thing that rewrites `NR13`/`NR14` mid-note. Expect any APU fix to move
  chiptune output, expect it to be *confined to what the fix is about*, and treat a diff that
  is not so confined as a finding. Nothing in `testharness/chiptune.rs` pins a hash of our
  own audio, which is why an accuracy fix does not turn it red.

## The GB Emulator Shootout entry (`docs/shootout.md`, `examples/shootout_scout.rs`)

- **We score 223/264 = 84.5 %, which is 5th of 19 on the published board** (measured on
  `main` at `1b3e3e7`) — behind docboy, Emulicious, SameBoy and Beaten Dying Moon;
  ahead of GSE (218), bgb (205), mGBA (150) and
  everything below. Re-derive it in seventeen seconds:
  `python3 tools/shootout/manifest.py <checkout> > m.tsv && cargo run --release --features
  image --example shootout_scout -- m.tsv`. An older report's **92/264** is stale by a wide
  margin — the DMG boot mode, the core-timing work, the APU campaign and the OAM-bug fix all
  land on rows nobody was scoring.
- **THE SHOOTOUT'S MOONEYE POPULATION IS 97 ROWS, NOT OUR 103** — `acceptance` 66 (not 75)
  and `misc` 2 (not 8). Its `testroms/mooneye.py` comments out **all fifteen** model-variant
  ROMs: nine `acceptance` (`boot_div-dmg0`, `-S`, `boot_div2-S`, `boot_hwio-dmg0`, `-S`,
  `boot_regs-dmg0`, `-mgb`, `-sgb`, `-sgb2`) and six `misc` (`bits/unused_hwio-C`,
  `boot_div-A`, `boot_div-cgb0`, `boot_hwio-C`, `boot_regs-A`, `ppu/vblank_stat_intr-C`).
  **That is exactly the population `0e099b0` fixed**, which is why eleven rows of real
  accuracy moved our own gates and moved the shootout number by **zero** — verified by
  re-scoring all 264 after the rebase and diffing: no row changed in either direction. Same
  trap the SameSuite notes record: *check which population a number belongs to before
  comparing two of them.*
- **THE SUITE IS 264 ROWS, NOT 311.** `testroms/mealybug.py` builds three lists and ends with
  `all = dmgs # + cgbcs + cgbds`, so 47 of the `Test(...)` objects it constructs are never
  run. A manifest generator that collects *constructions* instead of each module's `all` gives
  311 and quietly buries a DMG-only score under CGB rows nobody is graded on. Ours reproduces
  the published row set exactly, name for name — that equality is the check to keep.
- **They screenshot a Windows GUI window's client area; there is no serial capture anywhere in
  their project.** So the whole screen counts, boot-register dumps and palette included, and
  our `000000/606060/c0c0c0/ffffff` DMG palette passes only because `compareImage` allows a
  **50/255 greyscale** difference (measured against their dmg-acid2 reference: max diff **22**).
- **Two frontend defects that no test ROM can catch, both fixed here, both fatal to any
  automated runner:**
  1. **The desktop frontend paused when its window lost focus.** A runner started from a script
     may never be given focus at all (Windows refuses focus changes requested by a background
     process). Measured over ~11 s with two instances racing, counting frames through
     `--video-out`: **653 emulated frames with `--run-unfocused`, 29 without.** The pause is
     invisible from outside, so it reads as a catastrophic emulation failure.
  2. **A build with no sound device had no APU at all**, not a quiet one: `Sound::do_cycle`
     returns immediately while `player` is `None`, so `NR52` reads back powered off and
     PCM12/PCM34 report nothing. Worth ~85 shootout rows (`dmg_sound` 12, `cgb_sound` 12,
     `interrupt_time`, 57 SameSuite APU). `sound::SilentPlayer` is the fix and
     `render_desktop_with` installs it whenever no real player was attached. **Never ship a
     frontend that only emulates the APU when a sound card exists.**
- **New desktop flags, all ordinary user-facing flags — nothing is special-cased for their
  runner** (`screen::desktop::DesktopOptions`): `--model`, `--scale`, `--upscale`,
  `--upscale-factor`, `--speed`, `--run-unfocused`. Defaults reproduce the old behaviour
  exactly. `--scale N` sizes the client area in **device** pixels (a logical size would be
  multiplied by the display scale factor, and the runner measures `GetClientRect`) and makes
  the window non-resizable. `--scale` and `--upscale` are a pair: the first sizes the window,
  the second decides what is drawn into it, so `--scale 1 --upscale none --upscale-factor 1` is
  what pins a 160×144 picture whatever a user's `config.toml` says.
- **`--speed` shortens the emulated frame period, not the draw period.** Above 1× the extra
  frames are emulated in a burst inside one draw. Drawing at 15× would be 900 texture uploads
  and buffer swaps a second, and on the software OpenGL stack a CI runner has (`setupMesa`)
  *that* would be the limit, not the emulator.
- **`screen::desktop::WINDOW_TITLE` is a published constant**, pinned by a unit test, because
  the runner finds the window with `title.startswith(...)`. The board entry's display name is a
  separate string — renaming the project need not change the title, and vice versa. Both read
  `TerminalGB` today; **the repository slug stays `Alchemy86/gameboy`**, and that is what
  `downloadGithubRelease` takes.
- **Verified end to end on the real frontend, not only in a parser**: an X11 capture of the
  window is exactly 160×144, titled `TerminalGB`, and passes their dmg-acid2 comparison;
  `blargg/cpu_instrs` (≈31 s at 1×) finished in under 10 s at `--speed 15`. **Nobody has run
  the Windows binary on Windows** — the cross-target check is clean and the release workflow
  smoke-tests each build, but say that plainly rather than implying otherwise.
- **`.github/workflows/release.yml` exists because of this**: their `downloadGithubRelease`
  filters assets on `"win" in name`, so the asset must stay `terminalgb-windows-x64.zip`. A tag
  push publishes; a manual run produces a **draft**. It builds with `--features audio` on
  purpose (see the APU point above).
- **Re-scoring the board is one `workflow_dispatch`, not a new PR** — each emulator has its own
  `ci-<name>.yml`. That is the argument for submitting before the Mealybug campaign finishes
  rather than after: the gains land whenever we ask for them.

## Gen 1 shiny hunting — N emulators in parallel (`plugins/gen1-shinyhunt/`, `examples/shiny_hunt.rs`)

**Full write-up, measured rates and the cartridge traps: [`docs/shiny-hunt.md`](docs/shiny-hunt.md).**
The software answer to a hunter's four Nintendo DS consoles wired to one controller through an
input splitter — and it wins for a reason unrelated to parallelism: **we never look at a
sprite**. The DVs come out of `wEnemyMonDVs` the instant `LoadEnemyMonData` rolls them.

- **Gen 1 has no shiny Pokémon, and no string in this feature may imply otherwise.** What is
  hunted is the spread that *becomes* shiny in the Time Capsule (def/spd/spc all exactly 10,
  attack bit 1 set — 8 in 65,536, **1 in 8192**). **There is ONE implementation of that rule in
  the tree** (`terminalgb_plugin_trainer::derived::shiny_if_traded`), shared with the trainer
  panel's `★` and the encounter banner; `target::tests::the_shiny_target_is_the_trainer_panels_rule_and_nothing_else`
  enumerates all 65,536 spreads so a second copy cannot appear quietly. The hit *report* also
  picks its preamble from the actual verdict — a `--target spc=10` run stops on a Pokémon that
  is emphatically not shiny, and a report explaining the Gen 2 rule over it would be claiming
  something the run never checked.
- **The plugin declares `OBSERVE | READ_MEMORY` and nothing else** — not tidiness: a hunter that
  could write memory could fabricate its own hit, and then no result it ever reported would mean
  anything.
- **`wCurOpponent` reads back `0` once a battle has begun.** The latch first required
  `wEnemyMonSpecies == wCurOpponent` as an "independent witness" that the enemy structure was
  populated, and consequently **never latched anything at all** — every attempt was a stall and
  the feature looked completely broken while being one comparison away from working. The class of
  mistake: *an independent witness has to be observed to still hold at the moment you read it*,
  not assumed from where it was written. `--diagnose` exists because the four causes of "every
  attempt stalled" are indistinguishable from outside.
- **There is no half-written window on a wild encounter.** `InitWildBattle` is straight-line —
  `ld [wIsInBattle],a` then `call LoadEnemyMonData`, animation afterwards — so a once-per-frame
  sampler can never see the flag up and the DVs stale. Do not build machinery for that race.
- **The host's plugin frame counter is NOT machine state.** `Event::Frame { number }` keeps
  counting across `load_state` and `PluginHost::reset` does not rewind it. Putting it in a
  `Sighting` made **every** hit report "DID NOT REPRODUCE" while the replay was producing a
  byte-identical Pokémon. `Sighting::frames_into_attempt` is relative to the last `arm()` for
  exactly that reason. Anything else that wants a reproducible per-attempt value has the same
  problem.
- **A 24-frame hold is one *and a half* Gen 1 steps** and the game commits to a step once begun,
  so a navigation press moves the player two tiles as often as one; a greedy walker aiming at an
  exact coordinate oscillates 34 → 32 → 34 for ever. Close the last tile with a ~10-frame press.
  **A house's front door is a map-*edge* warp** (`RedsHouse1F` `warp_event 2,7` / `3,7`, row 7 of
  an 8-row map): arriving on the tile is not leaving through it, press Down again. **Mum has
  something to say downstairs** and a text box swallows every direction key — clear it with **B**,
  never A.
- **A base save state taken *during* a battle is the classic setup mistake** (the last step into
  grass is quite likely to be the step that rolls an encounter) and it makes every attempt a
  silent stall. `Instance::attempt` detects and names it.
- **Decorrelation is proved in BOTH directions, and it is NOT sufficient — this is the finding
  that cost the most.** Instances all start from the same bytes and emulation is deterministic, so
  identical input is one instance wearing N hats. The lever is *time* — `Random_` folds `rDIV` in
  on every call. `--probe` must show every instance different; `--probe --no-decorrelate` must
  show them **identical**, because a "divergence" that came from non-determinism would be a far
  worse bug. Both are pinned at unit speed with no ROM
  (`hunt::tests::instances_diverge_when_decorrelated_and_agree_when_they_are_not`).
  **But instances that differ from each other can still, between them, reach almost nothing.**
- **`Random_` builds BOTH DV bytes from ONE `rDIV` sample, so `lo - hi` IS that sample.**
  `LoadEnemyMonData` takes them from two consecutive `Random` calls microseconds apart:
  `hi = A + R`, `lo = hi + R` where `A` is `hRandomAdd` before the roll. The two bytes are not
  independent. Reload the same save state before every roll and `A` and `R` are both functions of
  elapsed frames, so every result lands on **one thin one-dimensional band**: measured **968 of
  the 65,536 spreads reachable, 2.7 distinct `lo` per `hi`**. A **97,212-encounter** hunt for a
  1-in-8192 target found nothing and never would have — while reporting 81 encounters/s, twelve
  diverging instances and a 1.7-minute ETA the whole time. **That is the failure shape to fear
  here: confident, well-instrumented and wrong.** (Corollary for anyone hand-checking: every
  shiny spread needs `lo = $AA` and `def = 10`, so `R` is always a multiple of 16 — a wild roll
  can only be shiny when its `rDIV` sample has a zero low nibble.)
- **A battle is how the RNG gets off the band, so an attempt is a RUN, not one encounter.**
  Gen 1's battle engine calls `Random` constantly, so the state after fleeing is somewhere else
  entirely. `run_length` (default 32) encounters per reload, fleeing between them, every one
  judged; purity is kept, so a hit still replays from `(base state, seed, attempt)` and the report
  names which encounter of the run it was. Measured: coverage **968 → ~6,150** of 65,536, i.e.
  **0.12 → 0.75 expected shiny spreads reachable**. Still short, and running *longer* inside an
  attempt does not push it further (4.8 and 26.2 encounters per run both give ~6,000), so the
  remaining narrowness looks structural to the wild-encounter path. **Say "roughly a coin-flip
  whether a wild shiny hunt is possible from a given base state at all", never "it will find
  one".**
- **`Coverage` is the product feature that falls out of this**, and it is the number that decides
  whether a hunt can ever win: `--probe` reports distinct spreads, an estimated reachable-set size
  (inverting `distinct = S(1-e^(-n/S))`) and a verdict. **`TOO NARROW` means reconfigure, not
  wait.** Never quote an ETA without it.
- **Two walk bugs that both looked exactly like "a bad encounter rate"**, and neither is visible
  without the run-health line (`mean run N of M`, plus stalls / flee failures / blackouts):
  a press is held 20-48 frames and a Gen 1 step is 16, so a long press walks **two or three
  tiles** while Route 1's grass is only four deep — uncapped, the walker paced 30 → 36 through a
  band of 32 → 35 and every run stalled after ~5 encounters (capped at the room left in the band:
  mean run **4.8 → 26.2** of 32); and the press floor must be long enough for **a turn AND a
  step**, because a press in a direction the player is not facing only turns the sprite and a
  narrow band flips direction constantly. Also: anchor the band once per attempt, or fleeing lets
  it *walk* out of the grass over a long run.
- **A party that faints ends the run** — `wIsInBattle` reads `$FF` and the game teleports the
  player to a Pokémon Center, where there is no grass. `--make-state` hands the scout a level 50
  Pokémon with Speed 250 so it can flee thirty battles in a row and survive them; blackouts are
  counted separately so a weak party is a *reported cause*, not a mysteriously short run.
- **Which instance wins a hunt is a race, so a whole hunt is NOT reproducible from its seed** —
  only each *attempt* is, which is the property a hit report needs. `Hunt::run` replays the
  winning attempt in a fresh machine before claiming anything, and `--replay <inst>:<att>` is
  printed in every report as the recipe. Do not restate this as "the hunt is deterministic".
- **The driver is not the plugin, and that is a finding for the plugin-API crew, not a
  workaround.** A plugin cannot press on a schedule, step a frame, load a state or own N
  machines; the driver therefore sits over `hunt::HuntMachine`, an eight-method seam the emulator
  implements for `Gameboy` in `src/gameboy.rs` behind the feature. The MCP server reached the
  same conclusion from the other side (`docs/mcp.md` §7). **Never make a plugin crate depend on
  `gameboy`** outside `[dev-dependencies]` — the trait is what keeps the graph acyclic, and it is
  also what makes the whole driver loop testable in CI against a fake machine with no ROM.
- **A hunt never touches the filesystem near the cartridge**: every instance is
  `Gameboy::new(bytes, None)`, and `src/mbc/battery.rs` only wires a battery when given a path.
  Keep it that way — N instances sharing one `.sav` would be a save-destroying bug at N=2.
- **Measured on this box (20 cores, contended): 8-18 encounters/s and 176-367x realtime on 12
  instances** (the spread is other work on the box; quote the range, not the flattering end) with the run loop (~1,250 emulated frames per encounter, most of it battle); the
  old reload-every-encounter loop did 81/s and 438x but see above for why its encounters were
  worth less. The useful instance count is bounded by cores;
  the default is `available_parallelism() - 1`. ETAs are computed from the *measured* rate and the
  target's exact odds — enumerated over all 65,536 DV pairs, never multiplied, because the HP DV
  is derived from the low bit of the other four and the terms are not independent.

### Watching a headless script run (`--show`, `src/screen/watch.rs`)

**The reusable technique, and it is the whole of the design: a watched run must change what
happens BETWEEN two frames and nothing INSIDE one.** `shiny_hunt --make-state --show` draws the
walk to Route 1 live; the state it produces is byte-identical to the headless one *by
construction*, because the walk's one primitive is a `Stage` trait with a single `frame` method
and every helper calls `stage.frame(gb)` where it used to call `gb.frame()`. Watch mode adds a
sleep, a redraw and three `peek`s around a frame. Any future "let me watch that script" feature
should take this shape rather than growing a second copy of the script.

- **The terminal frontend's *game session* is the wrong thing to reuse and `Start::Rom` must not
  be pressed into this service.** `load_session` builds its machine with `Some(path)` (so a
  `.sav` appears next to the cartridge), installs an audio player and attaches plugins. Two of
  those three change what the script produces: `Sound::do_cycle` returns immediately with no
  player, so a machine without one has **no APU at all**, not a quiet one, and a cartridge that
  reads `NR52` can tell. What *is* reusable is the frontend's **drawing** half, which is what
  `screen::watch::Viewer` is.
- **`Viewer` is handed a framebuffer (`&[u8]`), never the machine.** That is a type-level
  guarantee that watching cannot perturb emulation, and it is worth keeping when the module
  grows. It reuses `tui::build_picker` / `fit_metrics` / `probe_terminal` / `shm_supported`
  (`pub(crate)` **for this module and no other reason** — a merge that makes them private again
  will fail to compile, which is the intended guard), plus `FramePrep`, `Upscaler`, `TermImage`
  and `screen::halfblocks`. All three `docs/terminal-render.md` rules apply and are obeyed: cap
  the transmitted resolution, write the escape sequence after `ratatui` flushes, one image id
  for the whole run (`0x00676210` — clear of the frontend's four).
- **Gate the harness on the picture as well as the bytes.** `testharness/watchstate.rs`
  (`POKE_ROM=<blue.gb> cargo test --release --test watchstate`, skips exit 0 without a
  cartridge) runs the real example both ways — the watched one on a pty it owns, since `--show`
  refuses to start without a terminal — and compares the files. It *also* requires the watched
  run to have put picture bytes on the wire, because "identical" is otherwise satisfied by a
  `--show` that quietly drew nothing, and requires no `.sav` beside the staged ROM. Teeth
  proven: a stray `gb.debug_write` in the watched path reddens it at byte 54 with both files
  still 174,879 bytes; one extra emulated frame per displayed frame breaks the walk outright.
- **`--watch` was already taken** by the hunt's target selector, hence `--show` / `--show-speed`.
  The walk is ~3 minutes of emulated time, so the default pace is **2x** (a 90-second demo);
  `max` is what the harness uses.
- **A caption box must be sized from its own contents**, never a constant — the live detail line
  was clipped by a hard-coded 3-row footer on the first run. Same mistake, third time in this
  repository, after the frontend's status panel twice.
- **The walker is not the agent, and `docs/shiny-hunt.md` §2b now says so** because watching it
  reach Route 1 in seconds invites the opposite conclusion. It reads coordinates out of WRAM,
  steers to a number a human read off `pret/pokered`, and **writes memory twice** (a party
  Pokémon, and `EVENT_FOLLOWED_OAK_INTO_LAB`); the agent reads the *screen* as text and is
  refused `WRITE_MEMORY` by the host. A satnav with the postcode typed in versus looking out of
  the window — and the agent's slowness is the measurement.

## The swarm view — N agents on one map (`examples/swarm_view.rs`, `docs/swarm-view.md`)

Peter Whidden's opening shot from *Training AI to Play Pokémon with Reinforcement Learning*
([PWhiddy/PokemonRedExperiments](https://github.com/PWhiddy/PokemonRedExperiments)), reproduced
as a proof of concept: 24 emulators from one save state, every agent a dot at its own world
position, the camera pulling back. Assembled from what already existed — the shiny hunter's
`HuntConfig::seed_for` + `Schedule` for divergence, `hunt::HuntMachine` for the seam,
`wCurMap`/`wXCoord`/`wYCoord` from the atlas, `Gameboy::image()`, `video::card::Canvas` for text
— **the compositor is the only new code.** No emulation change; every baseline byte-identical
(in-repo 46/47, Mooneye 103/103 + 8/8, SameSuite 76, Mealybug 79, cpp 4/4, the whole c-sp
aggregation, savestate and battery).

- **`hunt::Schedule::step` is `pub` now and `Schedule::draw` was added**, both purely additive.
  A second walker that invented its own randomness would be measuring a different rig; the whole
  claim that these instances diverge for the *same* reason the hunter's do rests on both walkers
  drawing from one stream. Anything else needing a decision the type does not model should call
  `draw` rather than starting a private PRNG beside it.
- **The walk is deliberately NOT `Instance::attempt`.** That walker paces a ±1 tile band on
  purpose (it must not leave the grass); a swarm view wants agents that go somewhere. Only the
  *policy* differs — every timing decision still comes from `Schedule`.
- **SAMPLE ON EACH INSTANCE'S OWN FRAME COUNTER, NEVER ON WALL TIME OR A SHARED TICK.** That one
  choice is what makes the finished video independent of thread scheduling: instance *i*'s
  emulated frame 1200 always lands in video frame 50. Measured both ways — two separate processes
  produced 375 byte-identical PNGs (`sha256 494eb030…`), a different `--seed` produced
  `ba670d5e…`, so the check is not vacuous.
- **THE FINDING, and it is the reason the picture is worth having: Route 1's ledges are a
  one-way valve.** After ten emulated minutes per agent, **54.97 % of samples are in Pallet Town
  and 44.73 % on Route 1**, the northern half of Route 1 is empty in every frame, and **no agent
  ever reaches Viridian City** — not in that run and not in a 12-agent / 16-emulated-minute
  control (Route 1 70.06 %, Pallet 29.87 %). 5 % of the tiles they visit hold 37 % of their
  time. A random walk over one-way ledges is a ratchet. Do not read the swarm as learning agents:
  it is the null model.
- **A random walk is also a bad SURVEYOR, for the same reason** — 120,000 frames of undirected
  walking captured 100 % of Pallet Town and **44 %** of Route 1. `--capture-maps` therefore heads
  three times in four for the centroid of whatever is still un-photographed; that reaches Route 1
  100 % / Pallet 98 % in 25 s.
- **Stitching a map out of framebuffers: the anchor is screen pixel (64, 64) and the gate is
  `wWalkCounter == 0`.** The player's walk tile is screen tile (8, 8) (the World Trade Centre
  crew's measured anchor), so a frame belongs at world pixel `(x*16 - 64, y*16 - 64)` — but only
  between steps, because mid-step the engine is scrolling and the screen is off the tile grid.
  Skip the 16x24 box around the player or the finished map is a trail of Reds; write each pixel
  once so a later, worse view cannot overwrite a good one.
- **Map offsets are MEASURED from agents crossing the seam**, not tabulated: a connection
  preserves world position, so B's origin is `A's + (fx + step - tx, fy + step - ty)` tiles.
  **WATCH THE CROSSING EVERY FRAME, NOT EVERY SAMPLE, OR THE OFFSET IS SHORT BY HOWEVER FAR THE
  AGENT WALKED FIRST** — at one sample per 24 frames the agent got one to three tiles into the
  new map and Pallet Town landed **32 px under** Route 1, which looked plausible and was only
  caught because Pallet's own label was overpainted. Only observe on a frame where
  `wWalkCounter == 0`, out of battle, and `wCurMapWidth != 0`: during `LoadMapData` the
  coordinates and the header disagree. Fixed, the two maps abut to the pixel. **A warp looks
  identical from outside** — a door also changes `wCurMap` while a direction is held — so the
  inference is restricted to an allow-list of outdoor maps between which no warp exists.
  Anything else is reported unplaced and counted in the caption. The real world layout comes
  from PixelGB (`mapgb-render-t8`) later.
- **FEDORA'S FFMPEG HAS NO `libx264`** (patent-clean build): `-c:v libopenh264 -b:v 4M` is the
  substitute, and `-crf` is not accepted there. The tool prints that note itself, because
  `Unknown encoder 'libx264'` reads as a broken command rather than a missing codec. `libx265`,
  `libvpx` and AV1 encoders are present.
- **`video::card`'s 5x7 font has letters, digits, `-`, `.`, `:` and `/` and silently draws
  anything else as a blank.** A `+` in a caption disappears — seen live as `T 01:20` where
  `T+01:20` was written. Check any new glyph against `card::glyph` before shipping a caption.
- **The zoom is integer-scale nearest-neighbour and it is CHECKED, not promised.** Every colour
  in a scaled frame must already exist in the source — the same categorical guard
  `screen::halfblocks` uses, and the right one here because an invented shade between two agents'
  colours would be a marker that does not exist.
- **Nothing is committed**: the video and the captured map sheets are game art and live in
  `target/swarm/`. The two commands in `docs/swarm-view.md` regenerate them in about a minute.

## The status page (`docs/status.md`, `docs/status/index.html`) — scores are generated, not typed

- **`docs/status.md` is the front door** — suite standings, abilities, honest gaps, roadmap —
  linked from the very top of the README. Its styled twin `docs/status/index.html` is the
  captain-approved field-report page, ready for GitHub Pages (**Pages is not enabled** on the
  repo yet; enabling it with the `/docs` folder publishes the page at `/status/`).
- **Never hand-edit a suite score on either page.** Every baseline-derived figure is written by
  `docs/tools/status_numbers.py` from `testharness/*_baseline.txt` — `make status` regenerates
  both pages, and `make docs-check` (in CI) fails when either page disagrees with the
  baselines. **Re-blessing any baseline therefore requires `make status` in the same commit.**
  Teeth proven: flipping one SameSuite row FAIL→PASS turns the check red on both pages and
  `make status` moves the table, the HTML score *and* its bar width.
- **Two numbers are deliberately not generated**: the GB Emulator Shootout score (measured
  through their manifest + screenshot compare — `docs/shootout.md`) and the other emulators'
  Mealybug scores (published board figures). Update those by re-measuring, and say so.
- The Markdown table edits happen between `<!-- suite-scores:begin/end -->` markers; the HTML
  rows are matched by their `.name` text, so renaming a suite row in the HTML must be mirrored
  in `HTML_ROWS` in the script.

## Publishing readiness — the `terminalgb` binary, the package whitelist, the renamed repo

The publishing scout's blockers are fixed (2026-08-14); **nothing is published — that is the
captain's call**. What is now load-bearing:

- **`cargo install terminalgb` installs the emulator.** `src/bin/terminalgb.rs` is the
  terminal frontend's real source (`[[bin]] terminalgb`, `required-features = ["tui"]`);
  `examples/terminal/bin.rs` is a one-line `include!` of it so
  `cargo build --release --example terminal` — and the captain's `~/.local/bin/gameboy`
  launcher, which lives outside this repo and builds exactly that path — keeps working.
  **Edit the bin, never the example shim.** The bin carries the wasm stub `main()` (Cargo
  cannot target-gate a `[[bin]]` — the standing embeddability rule; `make embed` runs
  `check-wasm` and catches it).
- **The published crate is a WHITELIST** (`include = [...]` in the root `Cargo.toml`):
  source + testharness + the root files, nothing else. Verified by *reading*
  `cargo package --list -p terminalgb` — 141 files, 3.4 MiB: no ROM, no `tests/` submodule,
  no `resources/`, no `docs/`, no `AGENTS.md`, no local path. **Re-read the list whenever
  the manifest or the tree layout changes**; the test is the list's contents, not its size.
  Cargo also **excludes any subdirectory containing its own `Cargo.toml`** — the vestigial
  `examples/{terminal,desktop}/Cargo.toml` files were deleted for exactly that reason
  (they would have silently stripped the declared example paths from the package).
- **`src/cpu/z80` is deleted, do not restore it** — a dead, commented-out port of Imran
  Nazar's jsGB (his site states no licence; same provenance call as the removed
  `resources/jsgb-mmu-map.png`).
- **The repo slug is `Alchemy86/TerminalGB` everywhere now**: all 8 manifests
  (`repository`), README badges/clone lines, the conformance workflow/action, the shootout
  submission, and `docs/tools/codemap_links.py`'s `CODEMAP_REPO` default (docs links
  regenerated with `make docs-links`). GitHub redirects the old `Alchemy86/gameboy` slug.
  The root manifest also carries `rust-version`, `keywords`, `categories`, `homepage`,
  `documentation`, `readme` — crates.io resolves the README's ~111 relative links through
  `repository`, so that field staying correct is a discoverability feature, not decoration.
- **A NAMING COLLISION TO KEEP STRAIGHT: the release zip's `terminalgb(.exe)` is the
  DESKTOP frontend; `cargo install`'s `terminalgb` is the TUI.** The zip naming is
  load-bearing for the GB Emulator Shootout (`docs/shootout/terminalgb.py` launches
  `terminalgb.exe` and screenshots its window) and was left untouched on purpose. If the
  zips are ever renamed, the shootout adapter must move in the same commit.
- **The silent default build explains itself** — the `audio` feature stays off by default
  (ALSA-free fresh build, the standing choice), so: the Status `Audio` row reads
  `not built in (?)` on such a build, `NO_AUDIO_NOTICE` rides the `config_warnings`
  channel (stderr, picker footer, toast, `?` overlay — the `BASIC_INPUT_NOTICE` pattern),
  and the README's install line carries the caveat beside it.
- **The first-run picker is the "where do my ROMs go?" answer** — with no ROMs it says the
  emulator ships no game content, names `rom_dir` and the config path, and states the
  local-only position (ROMs read from disk, saves beside the ROM, nothing uploaded).
  Pinned by `an_empty_rom_dir_renders_first_run_guidance` against rendered output. The
  README section "Your ROMs stay yours — and stay local" is the prose version; keep the
  two telling the same story.
- **Still open for an actual publish** (see the scout report §3/§7, outside this repo):
  eight crates publish in dependency order, version bump + tag, GitHub topics/homepage/
  Pages are repo *settings* no commit can change.

## The World Trade Centre — a shared Gen 1 room (`plugins/gen1-lobby/`, `src/lobby.rs`)

**Full report, screenshots and the exact commands: [`docs/worldlobby.md`](docs/worldlobby.md).**
A 40x28 Cable Club room built at run time in unsaved WRAM, several independent emulator
processes in it at once, each walking under its own control and seeing the others. A real
trade and a real link battle both completed **inside** it. The way in is
**`gameboy --lobby`** (see below); `plugin-gen1-lobby` has been in `default` via
`standard-plugins` since the room landed, so nothing needs rebuilding with extra features.

- **RUN IT TWICE AND THE SECOND WINDOW JOINS THE FIRST — that is the product, and it did
  not work when it shipped.** Every `--lobby` run opened its own relay on `127.0.0.1:0`, an
  ephemeral port never surfaced anywhere, and joined *itself*: two windows were two rooms of
  one person. The rendezvous is now a fixed per-user port (`lobby::rendezvous_port` =
  `24000 + uid % 1000`, `$GB_LOBBY_PORT` overrides) and **the `bind` IS the decision**:
  success means we host, `AddrInUse` means somebody else's room is there (proved by reading
  its `you` greeting and hanging up), anything else means a private room and it says so out
  loud. A file holding a port number has a read-then-act window that two windows opened in
  the same second fall into — a port does not, exactly one process can hold it, and there is
  no corpse to sweep because a port dies with its process. Below the ephemeral range on
  purpose, so an outgoing connection cannot squat it.
- **BOTH PLAYERS ASKED TO BE `RED`, AND ONLY THE RELAY CAN SEE THAT.** Nobody types a name,
  so the default collides with itself, and the label over an avatar's head is the only thing
  telling you which walking figure is your friend. `lobby::unique_name` hands out the first
  free name from Gen 1's own cast under the room lock; `LobbyClient::join` reads the answer
  back before it returns and the frontend pushes it into the plugin with
  `LobbyPlugin::set_name`. Never silent: `joined … as BLUE (RED was taken)`.
- **ONE FIXED CABLE PAIR DIRECTORY FOR EVERY LAUNCH IS HOW FOUR CONSOLES CROSS-PAIR.** Every
  run used `target/worldlobby/cable-human`, so two `--lobby` runs put two player consoles and
  two driven partners in one directory that pairs in arrival order; the wrong two paired, one
  quit, and the survivor faulted and **poisoned its battery**. The room's consoles now share
  `$XDG_RUNTIME_DIR/terminalgb/lobby/cable` (`lobby::lobby_dir`) — narrow enough that an
  unrelated emulator cannot take the seat — and a **driven partner gets a directory keyed by
  the frontend's pid**, because it is a private two-console session.
- **The driven partner is OPT-IN now (`--lobby-partner`), and that is forced by the cable
  having two ends.** Booting one by default fills the seat the second person was walking
  towards, which is precisely what made two `--lobby` runs race. A partner is also only ever
  booted into a room we are *hosting*: joining one is proof somebody else is here. Say which
  of the two ways to fill the seat you want in the startup text — a solo `--lobby` can never
  reach the room, since the receptionist checks the wire.
- **`spawn_partner` used to create its log directory by accident.** `target/worldlobby` came
  into being as a side effect of `create_dir_all(pair_dir)`; the cable rendezvous moved to
  the runtime directory and the partner then failed with *"no partner (No such file or
  directory)"* — an error that names the log file nowhere and reads like a missing binary.
  Create the directories you write to, explicitly.
- **`wCurMap` IS NOT "the map is loaded", AND THE ROOM MUST NOT BE INSTALLED UNTIL IT IS.**
  The guard was `wCurMap == TRADE_CENTER && wLinkState == IN_CABLE_CLUB` — two bytes that
  are true for about **thirty frames** before `LoadMapHeader` has run. Install in that window
  and the room's block layout is drawn through the *previous* map's header: a screenful of
  bikes, tables, chairs and counters, and a console that then wedges and drags its cable
  partner down with it. Measured on the human console, the frame either side:
  `f2040 map0xEF link0x01 h4 w7` (the Pokémon Center's 7x4 — header not loaded) →
  `f2070 map0xEF link0x01 h4 w5` (the Trade Center's 5x4). **A "non-zero width and height"
  gate does NOT catch this** — the width was 7, not 0, restored from the save by `LoadSAV`.
  The gate that does is the plugin's own `screen::calibrate`, which reconstructs all 360
  cells of `wTileMap` from the live block map and demands an exact match; it already
  computed that proof and installed regardless of it. **Ask the engine, not a byte.**
- **AN EXACT MATCH AGAINST A BLANK SCREEN IS NOT EVIDENCE.** The first version of that gate
  still installed on **frame 1, before the boot logo**, scoring 360/360 — `wTileMap` and
  `wOverworldMap` are both untouched WRAM there, so "an exact reproduction" is two uniform
  buffers holding the same byte. `screen::MIN_DISTINCT_TILES` (4) is the fix, and it is the
  same rule as the test-ROM harness requiring its reference image to have >1 shade before it
  will believe a comparison against it. Any future "compare against the running engine"
  check needs the same guard.
- **A DEADLINE FOR SOMETHING ANOTHER PROCESS MUST DO BELONGS TO THE WALL CLOCK.**
  `LinkPeer`'s master-transfer hold was `HOLD_TIMEOUT_TICKS = 1_048_576` *emulated* T-cycles.
  That is a quarter of a second only if this console runs at 1x; on an unpaced one (a
  scripted player, fast-forward, a benchmark) it is **~5 ms of wall clock**, and the far end
  services its socket once per 16.7 ms frame — so every master transfer was declared torn and
  the save guard faulted both machines before the player left the Pokémon Center. It is
  `Duration::from_secs(2)` now, the same value and the same argument as `bgblink`'s
  `MASTER_TIMEOUT` (`docs/bgb-link.md` §3a; the counterparty it was verified against paces at
  10 ms/byte from a Python thread with Nagle on). Same mistake twice: `OFFER_TICKS` was 4096 T
  — **one seventeenth of a frame** — so a console that armed `SC = $80` on its very next frame
  missed every offer. It is two frames now, i.e. `transport::SLAVE_ARM_WINDOW`.
- **Do NOT "stall the console" here the way `TransportLink` does.** Right shape for a socket
  transport, wrong for this one: `LinkPeer`'s own tests drive two peers **in one thread**,
  alternately, so a peer that stops paying cycles to the CPU until the far end answers
  deadlocks against a far end that only runs once this call returns. Holding the shift
  register — which `LinkPeer` already does — gives the *game* the same thing without that
  hazard; only the deadline was wrong.
- **`gameboy --lobby` needs NO launcher change**, and that is measured, not assumed: the
  launcher's argument loop matches its own five flags and forwards anything else as the ROM
  path, so `--lobby` reaches the frontend intact and is matched before the path arm.
  `lobby`, `--lobby`, `--lobby=HOST:PORT` and `--lobby-alone` are all single tokens for that
  reason — the launcher forwards exactly one argument.
- **The seat is `lobby::take_seat`, and the demo's `--human` is the same call** with two
  fields different: `stage_self` (a capture run plays a copy out of `target/`; a launch plays
  the person's own save) and the partner binary's path. **The driven partner is
  `examples/worldlobby.rs`'s `play` subcommand spawned as a child process, never a second
  implementation of it** — a walker, a phase machine, a screenshot writer and the Cable Club
  bot do not belong in the shipped emulator, and two copies would drift apart. Missing binary
  costs the partner and nothing else.
- **The demonstration passed while the human seat was broken, and the reason is a coverage
  gap worth remembering:** `worldlobby play` attaches the plugin *after* its console has
  already arrived in the Cable Club; the terminal frontend attaches plugins at ROM load. Only
  a person walking in at their own speed goes through the window. **A driven console is not a
  test of a path a person takes.**
- **`worldlobby play`'s phase 0 must be PACED.** It ran unpaced with its patience counted in
  emulated frames — fine against another copy of itself, and six emulated minutes at ~50x is
  seven seconds of somebody's afternoon. The Cable Club attendant will not let *either*
  console through until both are at a desk, so the bot gave up before the player had left the
  Pokémon Center. That is §6.4's own rule ("both ends of a cable run at the same speed")
  applied to the half of the run that was exempt from it.
- **Diagnose two consoles from a TAGGED trace, not from screenshots.** Both processes append
  to one file and a pid does not say which console is which; two runs were spent squinting at
  pictures deciding. Tag with the plugin's own name (RED/BLUE) and it takes one run.
- **The room is a BLOCK MAP, not the screen buffer.** Writing `wOverworldMap` does not race
  the engine's scroll redraw; `RedrawRowOrColumn` then draws every row and column that
  scrolls in out of our blocks, so walking, collision, the walk cycle, the bump sound and
  the camera are all the cartridge's. The earlier per-frame `wTileMap` paint is the thing
  this replaces.
- **The cartridge's own Trade Center stays at block (0,0).** The consoles are hidden events
  at walk tiles (4,4)/(5,4), keyed to the map id in ROM, and cannot be moved. Keeping
  `maps/TradeCenter.blk` in the corner also means the first frame after install is already
  correct.
- **`wCurMap = $F0` (COLOSSEUM) starting a link battle is now MEASURED, not inferred** — the
  scout report listed it as explicitly unmeasured. `wLinkState` reaches `$03`/`$04`, 36,700
  transfers, HP visibly falling. One byte.
- **THE SCREEN ANCHOR IS MEASURED, NOT DERIVED.** The player's walk tile's top-left screen
  cell is (8, **8**); the cell its own collision test reads is (8,9), the *bottom* left.
  `screen::calibrate` reproduces all 360 cells from the cartridge's own block map for each
  candidate before the plugin paints anything: row 8 scores 360/360, row 9 scores 133/360.
  If neither matched it declines to paint at all.
- **`Gameboy::clear_input` does NOT release a key pressed with `keydown`** — it resets the
  scheduled-press *queue*. Handing over from one driver to another without releasing leaves
  a direction physically down for the rest of the run and pins the player against a wall,
  which reads exactly like a broken route. General fact; cost hours here.
- **A plugin's `Event::Frame` is dispatched from `Gameboy::end_frame`, i.e. AFTER the
  picture is drawn.** So anything written in `on_event` is seen on the *next* frame — and a
  write-then-undo pair inside one tick brackets the render exactly and is never visible.
  Draw first, hand back only what is no longer wanted. General fact.
- **A fixed key-hold is 1.5 Gen 1 steps**, and the game commits to a step once begun, so a
  walker aiming at an exact square overshoots and oscillates for ever. Release the key the
  instant the coordinate moves.
- **Both ends of a cable must run at the same speed AND start together.** One console
  reaching its seat first and running unpaced while its partner still walked produced
  **33,697 unanswered master transfers**, and `.establishConnectionLoop` gives up after 90
  iterations and sits on `PLEASE WAIT!` for ever. The relay reports how many players are at
  a console; both ends start on that.
- **DO NOT WRITE A SPRITE'S SCREEN PIXELS WHILE THE PLAYER IS MID-STEP — THE ENGINE OWNS
  THEM.** `wYCoord`/`wXCoord` stand still for the eight frames of a walk while the camera
  scrolls, so recomputing an avatar's pixels from them writes back the pre-step position and
  the avatar rides the camera, then snaps when the coordinates catch up. That is the
  captain's *"moving also moves the other player, then they auto correct"* exactly, and the
  cartridge already solves it: `AdvancePlayerSprite`'s `.spriteShiftLoop`
  (`home/overworld.asm`) shifts every sprite it maintains two pixels a frame against the
  player's motion, *"so that the player appears to move relative to them"*. The gate is one
  byte, `avatar::engine_owns_sprite_positions` (`wWalkCounter != 0` at **`$CFC5`** — walked
  six `db`s back from `wUpdateSpritesEnabled` and then **measured**, 7,7,6,6,…,1,1 with
  `wYCoord` incrementing on the frame it reads 0). The two agree byte for byte at every step
  boundary, which is what makes standing off free.
- **`.spriteShiftLoop` IS GATED ON `wNumSprites` (`$D4E1`), AND BOTH CABLE CLUB MAPS DECLARE
  ONE OBJECT** — so avatars in slots 2..9 were never shifted at all, whatever the plugin did.
  `avatar::hand_slots_to_the_engine` raises it. **Raising it is not free**: it also puts our
  slots inside `IsSpriteInFrontOfPlayer` — which is where their solidity comes from *and*
  where an A-press turns a slot number into a text ID via `wMapSpriteData` (`$D4E4`) — so the
  same call writes `TEXT_TRADECENTER_OPPONENT` (= 1 on both maps, each has a one-entry
  `def_text_pointers`) into every slot it claims. Without that, talking to the ninth person
  in the room walks off the end of the map's text-pointer table. Both addresses are walked
  from `wWarpEntries` `$D3AF` and land exactly on `wCurrentMapHeight2` `$D524`, which is the
  walk's own proof. Both are in the **saved** region and ride the same
  `wLinkState == LINK_STATE_IN_CABLE_CLUB` gate as the map header.
- **Measure this with `examples/lobby_drift.rs`, not by eye.** Two real consoles on a real
  cable in the real Trade Center, one synthetic peer, and the invariant *screen pixels plus
  camera scroll is constant*. Teeth proven by removing the gate: **14 px of drag against
  2 px**. On failure it reports where each bot stopped, because *"never reached the Trade
  Center"* on its own cannot tell "the bot could not navigate" from "the save never loaded"
  — a bot mashing A through a NEW GAME's opening is a bot pressing A, and that cost real
  time. **A save that gives the frontend a CONTINUE can still send this bot into a new
  game**: measured on the captain's own `~/roms` Blue save, which offers CONTINUE in a real
  window (screenshotted) and lands the bot in Red's bedroom with an empty party. Unexplained;
  do not conclude a save is broken from the bot alone — boot it and look.
- **Remote avatars are SOLID** (`CollisionCheckOnLand` calls `IsSpriteInFrontOfPlayer`
  before it looks at a tile), so a crowd converging on two consoles blocks itself — which is
  correct, and why a trade/battle marks you `away`. Note this was only ever true for the
  *first* avatar until `wNumSprites` was raised; the claim is now what the code does.
- **`IMAGEBASEOFFSET` stays 0** so `_UpdateSprites` skips the slot (no movement AI) while
  `PrepareOAMData` still draws it. The cost is that the engine's cull is skipped too, so
  `avatar::visible` reimplements `CheckSpriteAvailability`'s window. Cap at **9** remote
  avatars: `wShadowOAMEnd` *is* `wTileMap`, so the eleventh drawn character corrupts the
  screen — nine makes that impossible by construction.
- **The network is NOT in the plugin.** A `Plugin` is handed a machine during a frame the
  emulator decided to run; a socket is not that. `plugins/gen1-lobby` is pure memory work
  with no dependency but the plugin API; `src/lobby.rs` is the frontend's socket. Same
  layering reason as `MachineAccess`.
- **Save safety:** everything is below `wMainDataStart` except the map header and the two
  sprite-count bytes above, and those are gated on
  `wLinkState == LINK_STATE_IN_CABLE_CLUB`, where `LinkMenu` has set `BIT_LINK_CONNECTED` so
  `DrawStartMenu` prints RESET and `SaveMainData` cannot run. Every one of them is rewritten
  by `LoadMapHeader` on the next map load whatever we leave behind. Every ROM and save is
  staged into `target/` — including the human's, in the demonstration; the shipped
  `--lobby` deliberately plays the person's own cartridge.
- **`worldlobby play`'s `party_spec` fallback said `Nidoran M`**, which the cartridge's
  species table does not spell that way (the gendered names carry `♂`/`♀`), so any partner
  named something other than RED/BLUE/GREEN/YELLOW panicked in `patch_party` before drawing
  a frame. Its `bot.act` also hard-coded `verbose = false`, so *"never reached the Trade
  Center"* said nothing about where it stopped — both fixed, and both are the same lesson:
  **the failure path of a driven console is the one nobody exercises.**

## Authoring a NEW Gen 1 area and warping into it (`docs/gen1/authoring-an-area.md`)

**Reference page + the recipe: [`docs/gen1/authoring-an-area.md`](docs/gen1/authoring-an-area.md);
the experiment is [`examples/warproom_probe.rs`](examples/warproom_probe.rs)**
(`cargo run --release --features image --example warproom_probe -- --rom <blue.gb>
[--shots <dir>] [--no-latch]`, needs no save file — it starts a new game). The World Trade
Centre enlarges the room you are *standing in*; this is the other half — a **new
destination**, entered through the cartridge's own unedited door.

- **`BIT_NO_PREVIOUS_MAP` (bit 7 of `wCurMapTileset`, `constants/ram_constants.asm`) IS THE
  AUTHORING HOOK, and it is the same latch `docs/gen1/memory-map.md` §4 records as a trap.**
  `LoadMapHeader` clears it and `ret nz` before reading a single byte of
  `MapHeaderPointers`, so `LoadMapData` carries on to `LoadTileBlockMap`, which builds the
  room from **our** `wCurMapDataPtr`. It self-clears, so the next warp loads normally and
  nothing authored outlives the visit. **Measured both ways:** with the latch, 15 of 15
  header bytes survive and the live block map is ours; without it, 10 measurably change and
  the room is the cartridge's.
- **THE HEADER CANNOT BE INSTALLED BEFORE THE PLAYER STEPS THROUGH THE DOOR — a Gen 1 warp
  is decided from the SCREEN.** `CheckWarpsNoCollision` matches the coordinates and then
  asks `IsPlayerStandingOnDoorTileOrWarpTile`, which reads `lda_coord 8, 9` out of
  `wTileMap` and indexes `WarpTileIDPointers` **by `wCurMapTileset`** — the byte the latch
  lives in. And `wCurMapWidth` is the stride `RedrawRowOrColumn` repaints the departing room
  with. Change either early and the staircase stops being a staircase: measured, the player
  stood on it in the bedroom, unable to leave.
- **The window is `LoadMapData`'s own first instruction, `call DisableLCD`, which spins
  until VBlank.** So `wCurMap` has already changed a whole frame boundary before
  `LoadMapHeader` runs. Measured: stepping into the stairs, `wCurMap` read `$25` on frame 15
  while `wOverworldMap[0]` still held the bedroom's border block. A host stepping frames
  polls `wCurMap` and writes on the frame it changes. **This is the general pattern for any
  write that must land inside `LoadMapData`.**
- **A WARP NEEDS TWO THINGS: an entry in `wWarpEntries` *and* a tile the tileset itself
  calls a warp tile** (`WarpTileIDPointers`, `data/tilesets/warp_tile_ids.asm`). Without the
  second, the only route left is `ExtraWarpCheck`, which also needs a direction **held** at
  the instant of the check — so a warp entry on a plain floor block does not fire when the
  player simply steps onto it (measured). The cheap fix: lift the departing room's own
  staircase block out of its live `wOverworldMap` and place *that*.
- **The collision probe is the walk tile's BOTTOM-LEFT screen tile, i.e. block-definition
  indices 4, 6, 12, 14 — not 0, 2, 8, 10.** The two rules agree for uniform blocks and
  disagree for every mixed one; only bottom-left predicts the game (checked against nine
  blocks of the real `RedsHouse2` blockset, and Red's bedroom staircase is the worked
  example: `$0A` at index 10, `$1A` at index 14, and `$1A` is what the game reads). Same
  anchor the lobby crew's `screen::calibrate` found by brute force (row 8 scores 360/360).
- **`LoadTilesetTilePatternData` runs AFTER `LoadMapHeader` and the latch does not skip
  it** — 96 tiles (`$9000-$95FF`) copied from `wTilesetGfxPtr`:`wTilesetBank`
  unconditionally. Measured: a signature written into VRAM inside the install window is
  **clobbered**. Re-blit from the host after arrival; it costs no emulated cycles.
- **`wCurMap` must stay a real map id** — `SwitchToMapRomBank` and
  `MarkTownVisitedAndLoadToggleableObjects` index ROM tables by it *before* the latch is
  tested. Leaving it at the door's own destination means every ROM lookup is the intended
  one and only the header is ours.
- **The block-map scratch buffer must be outside `wOverworldMap` `$C6E8..$CBFC`** —
  `LoadTileBlockMap` fills all 1300 bytes with `wMapBackgroundTile` *before* copying, so a
  source inside it is destroyed mid-copy. `wTileMapBackup2` `$CD81` (360 bytes) is the
  largest clean gap, which also caps a room at 360 blocks — below the 1300-byte
  `(W+6)·(H+6)` footprint limit for anything squarer than ~19×19.
- **Ceilings, all sourced on the page:** 1300-byte block-map buffer (the cartridge's biggest
  map, `ROUTE_23`/`ROUTE_17` at 10×72, needs 1248); 127 blocks per axis because coordinates
  are `u8`; 32 warps / 16 signs / 16 declared objects but **15 usable** (slot 0 is the
  player); 256 blocks and **96 tiles** per tileset.
- **Gen 2 is scoped, not attempted, and NO GEN 2 ADDRESSES ARE PUBLISHED** — §7 of the page
  says what the work is (a map is `(group, number)`; find the analogues; establish whether a
  skip-the-header hook even exists, *first*, because it decides page-versus-project;
  re-derive the collision rule; re-measure every ceiling) and roughly what it costs.
- **Reusable pattern: run the host twice and print the diff.** `--no-latch` writes exactly
  the same bytes with the bit clear, and the "wrote / now / kept-or-clobbered" table is what
  turns the whole claim from a story into ten lines of evidence. Any host that writes
  emulated memory and hopes it sticks should do this.

## `docs/emulation/` — the conceptual layer, and the acid2 failure gallery

- **There are now THREE documentation layers about the machine and they must not merge.**
  [`docs/emulation/`](docs/emulation/README.md) is *what an emulator is* (concepts, no
  numbers of its own, written for a reader with no hardware background);
  [`docs/components/`](docs/components/README.md) is *how our implementation works*
  (`path:line`, quirks, future tests); [`docs/measured/`](docs/measured/README.md) is *what we
  measured and scored*. Four pages: `README` (what an emulator is, the loop, the three rates),
  `the-machine` (chip by chip), `accuracy` (the three cracks in the naive loop), `test-roms`
  (how we know it is right), `fpga` (emulation vs. a reconfigured circuit). Every section
  hands off rather than restating — **if a component has no deep page, say so, do not invent
  detail.**
- **THE ACID2 FAILURE GALLERY IS MEASURED, NOT COPIED — and the method is reusable for any
  picture-judged ROM.** `docs/emulation/faces/*.png` are renders of the vendored MIT
  dmg-acid2 ROM produced by breaking **one** behaviour in `src/gpu/mod.rs` at a time,
  rebuilding `examples/conformance_adapter`, rendering 600 frames and comparing with
  `tools/conformance/image.py`'s shade-rank compare. Measured distances, in the file names:
  `x-priority` **12**, `ten-per-line` **28** (the `!` of "HELLO WORLD!" disappears),
  `x-flip` **119**, `bg-priority` **144**, `tall-tile-bit` **256**, `wx-offset` **733**,
  `y-flip` **868** (the smile becomes a straight line), `tile-data-select` **1,044**. The
  119 for X-flip independently reproduces the figure the harness' own teeth-test records,
  which is what says the method is sound. Committing renders of an MIT test ROM is the same
  call `docs/upscale-samples/` already made — **never do this with a commercial cartridge.**
- **The lesson the gallery exists to teach: severity and pixel count are barely related.**
  12 pixels and 1,044 pixels are both total failures of a documented behaviour. Anyone
  reading a Mealybug or acid2 number as a percentage is reading it wrong.
- **`docs/tools/status_numbers.py`'s `INLINE_PAGES` now covers `docs/emulation/` and
  `docs/conformance.md`.** Adding a page to that list is what opts it in; a score written
  into a sentence anywhere else is unguarded. Teeth re-proven here: editing one wrapped
  Mealybug figure turns `make docs-check` red.
- **`docs/conformance.md`'s credibility paragraph had been stale for a long time** (it
  claimed 32/35 and a Shootout score of 92, on a page whose whole argument is "publish the
  denominator that hurts"). Its in-repo figures are wrapped in generated markers now; the
  Shootout figure is deliberately left ungated and must be re-measured, not edited.
- **`third_party/dmg-acid2/README.md` claimed the harness compares a framebuffer *hash*.**
  It has compared pixels against the author's reference image since the harness was fixed —
  fixed here. Worth remembering as a class: a stale sentence beside a vendored artefact is
  the least-read and longest-lived kind.
- **Two facts checked rather than recalled while writing these pages, both cited in them:**
  mGBA's SM83 core is an explicit per-cycle state machine (`enum SM83ExecutionState` in
  `include/mgba/internal/sm83/sm83.h`, `_SM83Step` in `src/sm83/sm83.c` — one bus operation
  per M-cycle), which is the instructive contrast with our fused decode-execute in
  `Cpu::operation`; and Analogue's own Pocket page says *"Completely engineered in two FPGAs.
  No emulation."*, which is the marketing claim the FPGA page examines. **The Pocket page does
  NOT name its FPGA models or its core authors** — a first draft asserted "Cyclone V, cores by
  Kevin Horton" and it could not be verified there, so it was cut.
- Measured on this machine while writing, for anyone who wants a fresh figure rather than a
  quoted one: dmg-acid2 runs **17,288 instructions per frame** at **4.06 T-cycles per
  instruction**, and `cargo test --release --test ppuengines` gave **0.2182 ms/frame
  standard vs 0.4614 identical, ratio 2.11x** (76.7x vs 36.3x real time).
- **`make docs-links` restamps the "documented as of <sha>" marker on EVERY page**, so
  running it after a docs change produces ~20 one-line diffs on pages you never touched.
  Revert the marker-only ones before committing to keep a review surface readable — nothing
  verifies marker freshness, only link validity.

## `docs/pitfalls/` — the trap index, for a stranger with a symptom

**There are now FOUR documentation layers about this project and they must not merge.**
[`docs/emulation/`](docs/emulation/README.md) is *what an emulator is*;
[`docs/components/`](docs/components/README.md) is *how our implementation works*;
[`docs/measured/`](docs/measured/README.md) is *what we measured and scored*; and
[`docs/pitfalls/`](docs/pitfalls/README.md) is **everything between a developer and the
machine** — the harness, the build, the terminal, the clock, the cartridge, the suite's own
denominator. `docs/measured/symptoms.md` indexes symptoms of the *machine*;
`docs/pitfalls/README.md` indexes symptoms of the *tooling*, and the two cross-link.

- **Six pages, one shape per entry**: Symptom (verbatim, quoted so it is searchable) → What it
  looks like (**including the wrong conclusion it invites** — that is the trap) → Root cause →
  Resolution → **Side effects and blast radius** → How we found it, citing the commit. The
  blast-radius section is the captain's explicit ask and is the part most write-ups omit; if
  an entry has none, it says so.
- **Write it for somebody who is not us.** The audience is a developer at 1am who landed here
  from a search on an error string, not a crew member with the repo's history in their head.
  That is why every entry quotes the literal text and why `README.md` opens with a symptom
  index rather than a table of contents.
- **`docs/pitfalls/{README,accuracy,tests,not-our-bug}.md` are in `INLINE_PAGES`**
  (`docs/tools/status_numbers.py`), so live scores in their prose are rewritten by
  `make status` and gated by `make docs-check`. **Historical and counterfactual figures on
  those pages are deliberately UNWRAPPED and must stay so** — the value a *rejected* constant
  scored, or a denominator another project uses, must not track our baselines. Teeth
  re-proven here by editing one wrapped figure and watching `docs-check` go red.
- **Two corrections that came out of writing it, both verified against a primary source:**
  - **The Mealybug reference images are NOT hardware photographs.** The suite's own README:
    *"the `expected` directory for screenshots from **my Game Boy emulator** (which I believe
    to be correct), and the `photos` directory contains blurry photos of the ROMs running on
    real devices."* So `expected/` — what every automated runner compares against — is Beaten
    Dying Moon's output, cross-checked by its author against `photos/`. Still a good oracle,
    but a weaker claim than the one `docs/mealybug.md` and the header of
    `testharness/mealybug_accurate_baseline.txt` make. **Those belong to the PPU crew and were
    left for them.**
  - **`state bytes` is 142091, not 142090** (measured live with `make test-cross` on
    `2e0da0e`; the save-state format grew a byte at `FORMAT_VERSION` 18 → 19).
    `docs/portability.md:49` and this file's portability section still say 142090. **Left for
    the crew that owns that page.**
- **A claim in the brief that did not survive checking, recorded because the correction is the
  better entry:** the eleven Mealybug rows that moved with `OAM_STROBE_LEAD` were *not*
  "correlating at 99.9 % under a +4 px offset" — no such correlation was ever computed here.
  The measured statement is *a uniform four-pixel horizontal shift, in both directions, every
  time*, and the symmetry is the tell. Repeat the weaker, true claim.
- **`cargo build --release --all-targets` fails deterministically on this repository** and is
  the sharpest reproduction of the multi-`crate-type` + `panic = 'abort'` trap; see the
  chiptune section above and `docs/pitfalls/builds.md`. Build target classes separately.

## The agent/MCP documentation surface (`docs/agent-reference.md`, `docs/agent-environment.md`)

Two new pages sit under the existing three (`docs/mcp.md` install, `docs/claude-plays.md`
design, `plugins/agent/README.md` overview). **Reference = what you receive and send;
environment = why this is a good place to run agents.** Both are linked from the README, the
plugin READMEs and each other; a stranger landing on `plugins/agent/` is now pointed at them.

- **EVERY PAYLOAD IN THE DOCS IS GENERATED, NOT TYPED.** `tools/agent-docs/capture.py` drives
  the real `terminalgb-mcp`, `claude_plays` and `screen_text` binaries and writes
  `docs/agent/examples/{mcp-transcript.md,mcp-transcript.jsonl,tools-list.json,
  agent-snapshots.md,agent-run.txt,screen-tour.txt}`. It needs **your own** Pokémon Red/Blue —
  none is distributed — and stages ROM+save into `target/` first. `make agent-docs-capture
  ROM=… SAV=…`. Screenshot base64 is truncated on the way out (a bare game frame as data is
  not ours to redistribute; a picture of *our UI* with the game incidental is, per the
  standing licence-boundary rule).
- **`testharness/agentdocs.rs` is the gate, and it has teeth (proved twice).** 105 checks:
  every `Limits`/`Budget` default and executor constant **on a line that also names it** (a
  whole-page number search is too weak — several constants share a value), every
  `Mode`/`ScreenKind`/`Trust`/`Halt` name string, all five `Halt` public sentences verbatim,
  every documented action verb and alias **driven through the real parser**, every tool in the
  captured `tools/list`, and — recomputed from the vendored cartridge —
  `agent-environment.md`'s frame hash and save-state size. It runs inside a plain
  `cargo test --release` (`plugin-agent` is a default feature); `make test-agent-docs` runs it
  alone. Known and documented limitation: a *second, stale* mention of a value it already
  found elsewhere is not caught.
- **Measured for those pages, so nobody re-derives them:** headless throughput through the
  JSON-RPC protocol **3,820 frames/s (64x)** single instance, **12,237 / 23,208 / 34,132
  frames/s at 4 / 8 / 16 instances** (20 contended cores — sub-linear past 8);
  `advance_text` through 4 boxes = **291 frames in one call** against 32 for a single
  `press A`, which is the whole frames-per-model-call argument; a deliberately naive adapter
  spent **235 calls on 23,772 frames**. `x86_64-unknown-linux-musl` builds a **974 KiB fully
  static** server that runs under `env -i`; the dynamic one is 880 KiB and links only
  libc/libm/libgcc_s. `cargo tree --no-default-features --features mcp` is **one** third-party
  crate.
- **`make test-cross` was run for real here and all three targets passed** — frame hash
  `0x01acf78fe505f31c`, state bytes **142091** (not 142090) on x86-64, aarch64, armv7 and i686.
  The rootless qemu route in `tools/run-cross.sh`'s header works and takes about a minute:
  `dnf download qemu-user-static-{aarch64,arm}` → `rpm2cpio | cpio -idmu` →
  `GB_QEMU_DIR=<dir>/usr/bin make test-cross`.
- **`observe`'s `frame` is a SESSION counter, not machine state.** `reset`/`load_rom` zero it;
  **`load_state` deliberately does not**. Anything proving a reset landed must compare memory
  or the structured state — `tools/agent-docs/gb_env.py`'s self-test reads `$C000..$C01F`, and
  says so out loud when the check proves nothing because that block never moved (dmg-acid2).
- **A real defect the capture found: the standing-on-an-exit snapshot note carried two runs of
  eighteen spaces**, from a multi-line string literal written without `\` continuations. Paid
  for in tokens on every decision and visible in any captured snapshot. Fixed in
  `plugins/agent/src/lib.rs`; write those notes the way their neighbours are written.
- **Three runnable artefacts under `tools/agent-docs/`, all stdlib/POSIX:**
  `hello_adapter.sh` (the minimum driver, 15 lines), `canned_adapter.py` (a working skeleton
  that branches on `screen.kind` — verified end to end, 62 calls, 0 failures, and it is what
  produced `docs/shots/agent-panel.png`), `gb_env.py` (a Gym-*shaped*, Gym-*free* client over
  the MCP stdio transport) and `log_to_dataset.py` (the decision log flattened into
  observation/action/outcome rows). **No reward function is shipped, deliberately** — a default
  one becomes everybody's benchmark by accident.
- **Verifying a doc through GitHub's `/markdown` API has a caveat the existing recipe omits:
  `mode=gfm` emits NO heading `id=` attributes**, so every in-page `#anchor` looks broken in
  the preview and none of them is. Check anchors with the slug rule instead (lowercase, drop
  everything not word/space/hyphen, one hyphen per space, **no collapsing runs**) — a ~15-line
  Python check over the pages' own headings is what was used here.

## One click: the Codespace, the image, and `terminalgb-run` (`.devcontainer/`, `Dockerfile`, `tools/oneclick/`)

**Reader-facing page: [`docs/quickstart.md`](docs/quickstart.md)** — the badge, the three
commands, the key, what is pinned, and every measured number. Gated by
`.github/workflows/container.yml` (both Docker stages + a smoke test that requires a decoded
160×144 PNG out of the MCP server, because a server that *starts* is not a server that
emulates).

- **THE DEVCONTAINER BUILDS THE REPOSITORY'S OWN `Dockerfile`, `--target dev`.** That is the
  whole arrangement, not tidiness: a Codespace and a local `docker build` are then the same
  environment rather than two definitions that drift. Three stages — `toolchain` (pinned Rust
  + the three apt packages), `dev` (adds the developer conveniences and a `vscode` user; what
  `.devcontainer` opens), `runtime` (the slim shipped image, no compiler). Adding a system
  package for the dev environment goes in `dev`, never in `runtime`.
- **The image's feature set is chosen for a container and is NOT `default`**: `tui,mcp,image`.
  No `desktop` (a container has no window) and no `audio` (no sound card, and it would pull
  ALSA in, which is exactly the fresh-clone trap the whole project avoids). Both the Makefile
  (`ONECLICK_FEATURES`) and `devcontainer.json`'s `onCreateCommand` spell the same set; if you
  change one, change both.
- **`terminalgb-run` must resolve the repository from ITS OWN PATH, never `git rev-parse` and
  never `$PWD`.** Both obvious alternatives fail in exactly the case that matters: a
  devcontainer bind-mounts the clone under a different uid, so git refuses it for *dubious
  ownership*, and a source tarball has no `.git` at all. Symptom, observed: `doctor` reported
  `NOT BUILT` / `MISSING` for everything while sitting beside a perfectly good
  `target/release`. `tools/oneclick/` is two levels below the root, and it confirms the guess
  with `Cargo.toml` + the vendored ROM before believing it (the same script is installed to
  `/usr/local/bin` in the image, where `../..` is `/usr`).
- **THE AGENT'S NDJSON LOG CARRIES AN ADAPTER'S STDERR VERBATIM — REPRODUCED, NOT ASSUMED.**
  `plugins/agent/src/lib.rs`'s `log_record` writes it deliberately (a failing model call has to
  be diagnosable), and that log is a *published artefact* here. A four-line adapter that echoes
  its environment to stderr puts `ANTHROPIC_API_KEY` in `agent.ndjson` **twice**, on a run that
  exits 0. `terminalgb-run agent` therefore greps its own run directory for the key's exact
  value afterwards, deletes what contains it, and exits non-zero; both directions are proven
  against a real cartridge and a real run. **The key goes to `grep` on stdin, never as an
  argument** — argv is world-readable in `ps`, which is the same reason the state snapshot goes
  to the adapter over stdin. This is a backstop on the artefact, not a sanitiser on the pipe:
  an adapter must not print the key.
- **Two POSIX-shell traps, both of which produced a silent exit with no diagnostic:**
  `shift` is a **special built-in**, so shifting an empty argument list exits a non-interactive
  shell *whatever `|| true` says* (`terminalgb-run mcp` with no ROM exited 2 saying nothing);
  and under `set -e` a top-level `[ … ] && cmd` whose test is false ends the script (running
  with no arguments at all exited silently instead of playing the bundled ROM). Use
  `if … then … fi`. The same shape bites a GitHub Actions `run:` block, which is `bash -e`:
  `grep -q X && { exit 1; }` fails the step in the **good** case.
- **`agent` needs a Gen 1 cartridge and says so; `mcp` is the "AI drives it with nothing
  configured" path.** The autonomous loop declines a cartridge it cannot read rather than
  flailing, so against the bundled dmg-acid2 it prints one line and stops — by design. The MCP
  server, by contrast, drives *any* cartridge and works against the bundled ROM with no key and
  no game: that is what the smoke test exercises and what a stranger should be pointed at.
- **The only game content in the image is the vendored MIT dmg-acid2 ROM, with its `LICENSE`
  beside it.** `.dockerignore` excludes `**/*.gb` and re-admits that one file, so a developer's
  cartridges cannot reach a layer even by accident. Nothing is fetched at build time.
- **Measured, so nobody re-measures or over-claims** (20-core x86-64 box, `2e0da0e` era):
  cold `docker build --no-cache` **56 s**; the cargo build alone **37 s** on 20 cores and
  **58 s** under a real 2-CPU `cpuset` with a cold registry and target — only 1.6×, because the
  critical path is the single large `terminalgb` crate. Runtime image **193 MB** on disk /
  **46 MB** pulled (85 MB Debian, 52 MB ca-certificates + python3, 7.9 MB binaries); the `dev`
  image is **2.6 GB** because it contains a toolchain. Container start → 120 emulated frames →
  a decoded 160×144 PNG: **0.74 s**. 6,000 frames of a real cartridge in the image: **1.6 s,
  61× real time**.
- **`docker run --cpus 2` DOES NOT SIMULATE A TWO-CORE MACHINE for a build.** It is a CFS
  quota, so `nproc` still reports the host's core count and cargo spawns that many jobs to
  share two CPUs' worth of time — the number it produces is not the number a small machine
  gives. Use `--cpuset-cpus 0,1`, and check `nproc` inside the container before believing any
  figure.
- **On Fedora (or any SELinux distribution) a bind-mounted workspace is denied to the
  container unless it is relabelled** — `chcon -Rt container_file_t <dir>`, or a `:z` mount
  flag. The failure is silent and reads exactly like a broken devcontainer: the container
  starts, the lifecycle command runs, and every path inside reports as missing. Codespaces runs
  on Ubuntu and never sees it.
- **A GitHub-hosted Codespace itself has NOT been exercised** — this project develops on
  branches that are never pushed, and a Codespace can only be created from a branch on GitHub.
  What was driven is the same `devcontainer.json` and the same Dockerfile stage through
  `@devcontainers/cli`, which is the tool Codespaces runs. Unverified until it lands on `main`:
  the badge URL, GitHub's creation flow, and the secret prompt. `docs/quickstart.md` §8 says so
  in the same words; do not let it be quoted as verified.

## Long runs and episodes (`src/longplay/`, `examples/longplay.rs`, `docs/longplay.md`)

**Full page: [`docs/longplay.md`](docs/longplay.md).** An agent plays unattended for hours at
headless speed and reports in; and an **episode** — a start save state, a success condition, a
failure/timeout — is reset and repeated so an agent becomes measurable instead of anecdotal.
Behind the `longplay` feature (in `default`, out of `--no-default-features`), and it adds **no
third-party crate**: the JSON codec is the agent plugin's and the ntfy push is delegated to the
system `curl`, the same bar `portal` meets.

- **THE HEADLINE IS FRAMES PER MODEL CALL, AND ON AN UNCAPPED HEADLESS RUN IT IS MOSTLY THINK
  TIME.** Measured on a real Pokémon Blue run through the `claude` CLI adapter: **92,654
  frames/call over 45 calls** — and 45 calls in 19m42s is ~26 s each, which at 3,527 emulated
  fps is ~92,000 frames, i.e. *essentially all of it*. The emulator never blocks on the agent
  (right, and load-bearing), so headless the world runs a game-day forward while one decision is
  being made and the plan comes back aimed at a world that moved.
  `Scoreboard::executing_frames_per_call` and `think_share` are what make the number honest and
  are both in `report.json`. **Never quote `frames_per_call` from an uncapped headless run
  without `think_share`.**
- **THE COUNTERFACTUAL, MEASURED, AND IT IS THE STRONGEST EVIDENCE IN THIS SECTION.** The same
  episode (out of Oak's Lab, 12,000-frame timeout, ≤5 calls, real model, same start state) run
  three times each way: **`--pace thinking` 3/3 succeeded at 810 frames/call, median 804;
  unpaced 0/3, every attempt timing out at 12,000 frames having made exactly one call that had
  not come back.** Note which column is bigger — **the failing configuration reports the larger
  frames-per-call**. The runner now prints a warning when an episode's timeout is dwarfed by a
  call's think time. `--pace thinking` is off by default because "the emulator never waits" is
  the property the design rests on, but for an **episode** it is almost always what you want.
- **The agent that spent eight game-hours pinned in Oak's Lab during the long run got out of it
  in ONE decision, three times out of three, once the segment was isolated.** That is the
  argument for episodes in one sentence, and the workflow it implies: a long run's
  `captures/` directory is a list of candidate episodes.
- **The agent's stuck detector and the game's are different questions and neither substitutes.**
  `governor::Stuck` fingerprints the world across *decisions*; an agent walking ten steps north
  then ten south produces a different fingerprint every time and never trips it. `progress::Wall`
  asks "has the *game* got anywhere?" over **game** time via a score that never falls when the
  player merely moves. It escalates on a **doubling** ladder (15m → 30m → 1h of game time,
  observed on the real run) — a linear one still reports every twenty minutes for ever.
- **"Stuck" is two things and the report must say which.** `pinned` (few distinct tiles — walking
  into a wall, reopening a menu) vs `wandering` (plenty of ground, no progress — the maze
  failure). Both observed on one real run: *"going round in 1 tiles"* in Red's house, then
  *"wandering over 17 tiles"* in Pallet Town. The separator is distinct `(map, x, y)` triples,
  not distance, so a two-tile shuffle repeated a thousand times still reads as pinned.
- **MEMORY WATCHES ARE HOST-SIDE REPORTING AND MUST NEVER REACH THE AGENT.** An agent told "the
  opponent has 4 HP" is reading the answer out of RAM, and the leak would be invisible in the
  results — every success rate the harness ever produced would be uninterpretable. The two paths
  are structurally separate (`src/longplay/watch.rs` is read in the *host*; the agent's
  perception is its own observer inside the plugin) and `testharness/longplay.rs` check [13]
  proves it by declaring a watch called `secret_oracle` and grepping **every request the driver
  was handed**. Do not "helpfully" add watches to the snapshot.
- **A resume is a REWIND and is never silent**: counted in the checkpoint, reported as a `Major`
  milestone, and printed in the report's `assistance` block beside `memory_writes`,
  `memory_informed_decisions` and `seed_controlled`. Same rule for an episode's per-attempt
  reset (`state_resets`). **An undeclared advantage makes every number worthless.**
- **`Progress::wiped()` is total party HP, NOT `wIsInBattle == $FF`.** That sentinel is set for a
  moment inside `.allPokemonFainted` and a 10 Hz sampler misses it almost always; total HP is a
  state, not an edge, so it is still true when the sample lands.
- **The Pokédex and bag addresses are derived and the walk lands on a verified anchor from BOTH
  directions**, which is its own proof: forwards from `wPartyCount` `$D163` (+404 bytes of party
  block) and backwards from `wPlayerMoney` `$D347` (−41 bag −1 count −19 seen −19 owned) both
  land on **`$D2F7` = `wPokedexOwned` = `wMainDataStart`**. Pinned by
  `longplay::tests::the_pokedex_walk_lands_on_both_of_its_anchors`.
- **A per-decision dataset is a by-product, not a project.** `steps.ndjson` is one line per
  decision — observation, intent, actions, outcome — every field already measured for the report,
  joined to the agent plugin's own `agent.ndjson` on `decision`. **Fields are omitted when
  `in_game` is false** rather than written as zeros: a learner cannot tell a real zero from a
  not-yet-a-game zero.
- **`AgentPlugin::last_plan()` and `stuck_worst()` were added for this** — additive, read-only.
  `Executor::start` clears its outcomes, so `last_plan()` is exactly the last plan's and never a
  running total.
- **The harness fixture is a cartridge that does NOTHING** (`jr -2`, header `POKEMON BLUE` +
  checksum `$9D0A` so the Gen 1 reader engages) and the harness pokes the WRAM a real game would
  write. Far better than a hand-assembled game simulator when what is under test is the
  *observation and reporting* layer; `testharness/agent.rs` already drives the executor against a
  moving player. 55 checks, no ROM, ~20 s.
- **THE TWO-RLIB TRAP BIT THIS WORK TOO.** `testharness/longplay.rs` importing
  `terminalgb::plugin::Plugin` to call `AgentPlugin::capabilities()` failed with *"the trait
  `Plugin` is not implemented for `AgentPlugin`"* and *"multiple different versions of crate
  `terminalgb_plugin_api`"* — the same `panic = 'abort'` dual-rlib problem `testharness/portal.rs`
  records. The fix was better than the workaround: assert the **observed** behaviour through
  `Gameboy::plugin_statuses()` (`has_pressed && !has_written`) instead of the declaration.
- **`--command` swallows the rest of the command line, so it must be LAST** — checked, like
  `examples/claude_plays.rs`. And **never point a run at `~/roms`**: the runner stages the ROM
  into the run directory, where its `.sav` belongs to the run.
- **Prior art was read rather than rediscovered, and it is cited on the docs page**: the harness
  dominates the model (PokéAgent Challenge `arXiv 2603.15563`: *"without a harness, raw frontier
  VLMs achieve effectively 0% task completion"*; lmgame-Bench `arXiv 2505.15146`: 40% of
  un-harnessed runs fail to beat random play vs 86.7% with one), persistent memory is the most
  dangerous component (Claude 3.7 acting for hours on a false premise it had written down — which
  is why an **episode carries no memory between attempts, deliberately**), and the knowing–doing
  gap is a named reproducible failure. Several widely-repeated Twitch Plays Pokémon figures could
  not be traced to a primary source and are deliberately absent.
- **The control that makes the think-share claim readable**: the same runner with a *canned*
  driver on the same cartridge, nine runs of 75,705 frames over three successive
  emulator revisions, reports **252.35 frames/call and 231.09 executing-frames/call —
  identical to two decimals on every one of the nine, while the wall clock moved 5% —
  at 5,411-5,668 emulated fps, 90.6-94.9x real time**, with
  `think_share` **0.00** and `executing_frames_per_call` 231.09 — with an instant driver the
  headline is very nearly all real play and the two numbers agree. Different workload from
  `docs/mcp.md`'s 70x (fresh boot + agent plugin here, an overworld save through MCP there), so
  quote both with their method rather than treating either as a contradiction.
- **A RATE MUST DIVIDE BY MILLISECONDS, NOT WHOLE SECONDS, AND THIS PAGE ONCE GOT IT WRONG.**
  An earlier draft published "4,000 emulated fps" from a *three-second* run — at whole-second
  resolution 20,000 frames is anywhere from 5,000 to 6,667 fps depending on which side of the
  second it landed, and the report printed it to four significant figures. `Scoreboard::wall_ms`
  is what every rate divides by now; `wall_seconds` survives for display and for the checkpoint.
  Pinned by `a_rate_is_measured_in_milliseconds_not_whole_seconds`. General rule: **never quote
  a rate from a run shorter than the clock's resolution times a hundred.**
- **Nothing here has been run for eight hours.** The longest verified session is twenty minutes;
  the checkpoint/resume path is tested by a second process reading a first's checkpoint. "It
  survives an overnight run" is a prediction, not a measurement — say so.
## GBSelfTest — a test cartridge that lives OUTSIDE this repository

**Reader-facing page: [`docs/gbselftest.md`](docs/gbselftest.md); the artefact and its lock are
[`third_party/gbselftest/`](third_party/gbselftest/README.md).** A Game Boy cartridge that tests
the machine it runs on, prints a verdict on the screen and over the link port at once, and
carries a **stable code per check** that resolves to a heading on our documentation page. Run it:
`make gbselftest`.

- **IT IS IN ITS OWN REPOSITORY (`Alchemy86/GBSelfTest`, MIT) AND THAT IS LOAD-BEARING, NOT
  TIDINESS.** Our whole conformance position is *"no check's expected output may be defined by
  our emulator"* (`docs/conformance-action.md`); a test cartridge kept inside the emulator it
  judges is the easiest possible way to break that. Out of tree it can be adopted without a
  dependency on us, and it is free to disagree with us — which it does, on five rows, on
  purpose. **Consume it as a built artefact, never as source.**
- **Vendored with a lock, not a submodule and not a build-time fetch** — the pattern agreed with
  the AtlasGB crew so the repository has one: `third_party/gbselftest/{gbselftest.gb,
  gbselftest.lock, CHECKS.md}`, refreshed by `tools/fetch-gbselftest.sh`, whose `--verify`
  recomputes the checksum offline and whose `--check-upstream` is advisory and never a build
  failure. `GBSELFTEST_LOCAL=<checkout>` stands in for a release, which is how it was
  bootstrapped and how a cartridge change is tried here before publishing.
- **IT IS DELIBERATELY OUT OF `tools/conformance/checks.json`.** That manifest is gated at 100 %
  by `conformance-self.yml` and its result is the README badge; the **default** picture mode
  loses five of the cartridge's rows *by choice*, so folding them in would force either a
  permanently red gate or a lowered threshold. It has its own manifest
  (`tools/conformance/checks-gbselftest.json`) and its own job. Fold it in the day the default
  models those five — a one-line change, and the file says so.
- **The gate is WHICH rows fail, not whether it passes.** `tools/gbselftest-gate.py` scores both
  picture modes against a per-mode list of KNOWN gaps, each carrying a one-line reason. A row
  that fails and is not on its list is a regression; a row on the list that starts passing is a
  change too, and the script asks for the list and the table in `docs/gbselftest.md` to move in
  the same commit. Teeth proven in both directions.
- **Scores, measured 2026-08-15 and the reason to trust the instrument:** this emulator
  `identical` **86/87** and `standard` **80/87**; SameBoy DMG and MGB **87/87**, CGB-E and AGB
  84/87 with three documented skips; Peanut-GB **64/87**. A cartridge that separates our two
  renderers on precisely the rows our own documentation predicts is worth trusting on the rows
  we did not predict. **That spread is the property to protect** — a suite everybody passes
  measures nothing.
- **Every row we lose was already written down here, which is the point.** Six are the
  whole-scanline renderer's documented trade-off (`GB-PPU-07/08/09/18` the mode-3 fetcher
  penalties, `GB-PPU-10/11` the mode-based blocking) and the per-dot engine passes all six.
  **`GB-PPU-16` is lost by BOTH engines and is the interesting one**: `docs/components/ppu.md`
  says in as many words that the DMG spurious STAT-write interrupt is not modelled, and until
  now **nothing in this project could measure it**. A sentence in a component page is a check
  with a stable code now. Do not read it as a regression, and do not "fix" the note — fix the
  PPU, and the gate will notice.
- **No emulation changed and no baseline moved.** Everything red is a standing trade-off or a
  standing gap, restated by an instrument written somewhere else.
- **The doc page's per-code half is GENERATED, not written here.** `docs/tools/gbselftest_doc.py`
  splices the vendored `CHECKS.md` between markers in `docs/gbselftest.md`; `--check` fails if it
  has drifted, which is what stops a URL the ROM printed landing on a heading that no longer
  means what the ROM said. Edit the cartridge, not the page.
- **Reading a report: `Passed`/`Failed` on the last line is Blargg's convention on purpose**, so
  every existing runner understands it with no contract change. **The one-line explanation and
  the code are the product**; a bare `Failed` is the least useful thing in it. `GB-SER-03`
  deliberately shifts one byte out of the link port, and the link port is the log, so one
  verdict line can begin with a stray `.` — a parser anchored at column zero silently loses a
  check, which is how the gate first reported 75 of 76.

### What a cartridge can and cannot measure, settled by building one

- **The composited picture is unreachable** — no framebuffer exists; the PPU streams to the
  panel. But **rendering leaves a timing fingerprint**: mode 3 lasts exactly as long as the
  fetcher takes, so a scene of documented cost can be laid out and the length measured. Those
  checks turned out to be the most discriminating in the whole suite.
- **The technique, worth stealing:** arm the coincidence interrupt on one scanline, and in the
  handler run a variable-length `NOP` sled before reading `STAT` **once**; binary-search the
  delay to find the machine cycle the mode changes at. The offset from interrupt to first `NOP`
  is unknown and irrelevant because **every result is the difference between two such edges**.
  Mode 3's absolute length falls out of two edges plus two documented constants (456 dots a
  line, 80 of them the object scan) with the unknown cancelling.
- **Four dots is the floor and it is physics, not effort** — the CPU observes only on M-cycle
  boundaries. An `SCX` of 1, 2 or 3 is unmeasurable from inside; 4 is not, which is why the
  fine-scroll check uses 4.
- **Two of our own expectations were wrong and only a FOREIGN emulator found them.** `LY` reads
  153 for four dots and then reads 0, so a poller sees 152 and both readings are correct; and a
  length counter enabled part-way through its period is clocked once immediately, which can
  reload it to a full 64 steps, so a check that waited 60 ms passed or failed by luck. Neither
  was reachable by reasoning. **Validate a test ROM against an implementation nobody here wrote,
  every time.**
- **A model difference we did NOT assert:** on a Color console, clearing `LCDC` bit 1 still
  charges the fetcher for its ten objects; on a DMG it does not. One independent measurement is
  not enough to assert a rule, so that check skips on Color and says why.
- **`tools/sameboy-oracle/` cannot read serial**, so the cartridge repo carries its own
  ~90-line SameBoy runner that accumulates the bit-level callbacks into bytes. If you need a
  foreign opinion on anything serial-reported, that is where it is.

### Build traps this cost real time to find

- **A Makefile whose objects do not depend on the `.inc` files rebuilds nothing when the memory
  map changes, and the half-stale ROM gets blamed on the emulator.** An hour went into
  "debugging" a WRAM shift that was three stale object files. Every object in the cartridge
  depends on every include, deliberately.
- **A test cartridge must not scribble on its own state.** The echo-RAM check wrote to `$C000`,
  which is where the captured boot registers live, and `GB-BOOT-03` then reported a value no
  console has ever handed over. Use the cartridge's own scratch and its echo.
- **`RET` during an OAM transfer whose source is work RAM pops rubbish** — the CPU cannot reach
  that bus, and the stack is on it. Sit in high RAM for the whole transfer. This hung the
  cartridge on SameBoy while passing here.
- **After a teeth test, REBUILD before measuring anything else.** `git checkout` restores the
  source, not the binary; a stale `conformance_adapter` reported `GB-CPU-07` failing long after
  the deliberate break was reverted.

## `docs/picture.md` — the reader-facing picture page, and the overclaim it corrects

**The page: [`docs/picture.md`](docs/picture.md); the gate: `docs/tools/picture_numbers.py`,
wired into `make docs-check` (CI) and refreshed by `make picture-numbers`.** It owns *"how the
picture gets to you, and how to capture it"* for someone who has never opened the source;
`rendering.md` still owns the transport ladder in depth, `terminal-render.md` the performance
fight, `display.md` scaling across the three frontends, `upscaling.md` the algorithms. Cross-link,
do not restate.

- **WE DID NOT BUILD A FRAME TRANSPORT PROTOCOL, AND THE PAGE SAYS SO IN ITS SECOND SECTION.**
  An outside reviewer told the captain we had, and a compliment left unchallenged quietly
  becomes a claim. We **implement four published ones** — the kitty graphics protocol (Kovid
  Goyal), sixel (DEC), iTerm2 inline images (George Nachman), and the `▀` glyph — probe the
  terminal, and pick. What *is* ours is the sizing decision (transmit at the picture's
  resolution, never the window's), writing the sequence outside `ratatui`'s cell buffer, the
  unchanged-frame hash, and `screen::halfblocks`. **`ratatui`/`ratatui-image` are credited
  fairly there too** — the half-block smoothing was a default we should have checked, not a
  fault, and the same crate's sixel encoder does not smooth.
- **The gate has three tiers and only two are re-derived on every check.** *Source facts* (the
  `Graphics` ladder order, the `Algorithm` list and `supported_factors`, and that `c`/`C`/`[`/
  `]`/`}` are still bound to what the page says) are parsed out of `src/` every run — rename a
  key and `--check` names it. *Measurements* live in `docs/picture/measurements.json` and are
  **data**: re-running the benchmark inside `--check` would fail CI on ordinary timing noise, so
  refresh them deliberately with `--measure`. *Terminal captures* are checked for existence only.
  Teeth proven four ways (a new upscaler factor, a renamed key, a perturbed measurement, a
  missing image); re-run those if you doubt it.
- **The blur fix has a fresh, one-key-apart reproduction now**, better than the archived
  before/after: same binary, same WezTerm window, `protocol = halfblocks` either way, and
  `render_path = legacy` alone takes the picture area from **5 distinct colours to 184**
  (`magick shot.png -format '%k unique colours\n' info:`). The 271 figure in `display.md` §1 is
  the same measurement at 80×24 — quote a size with either.
- **`c` and `C` are byte-identical at the defaults**, and that surprises people: `grayscale` +
  `nearest ×1` means the display path is the identity, so 0 of 23,040 pixels differ. They
  diverge the instant `[`, `]` or `}` is pressed. `c` also uses the **selected** upscale factor,
  not `transmit_factor` — with `transmit_scale = auto` a `nearest ×4` sends 160×144 on the wire
  and lets the terminal stretch, which is right for the wire and wrong for a file.
- **The bench's `--path capped` column is NOT faithful for sixel or half-blocks.**
  `examples/render_bench.rs` maps every non-iTerm2 protocol to `Wire::Kitty` there, so
  `--protocol sixel --path capped` measures kitty. Only kitty and iTerm2 use `TermImage` in the
  product; sixel goes through `ratatui-image` and half-blocks through `screen::halfblocks`. The
  page therefore takes sixel and half-block figures from a **real terminal** via the status
  panel, never from that column.
- **`pkill -f 'http.server 8711'` killed this crew's own shell mid-command**, exactly as the
  standing `pgrep -f`/`pkill -f` note warns. Kill by the PID you captured at launch.

## `src/screen/controls.rs` — every key in ONE table, and `docs/controls.md` is generated from it

**Reader-facing page: [`docs/controls.md`](docs/controls.md); the registry is
[`src/screen/controls.rs`](src/screen/controls.rs); regenerate with `make controls-doc`.**
The captain's question — *"do all the controls appear on a written page, with links, so `C`
is listed as screenshots and links to the page that explains it?"* — had the answer **no**.

- **THERE WERE THREE LISTS AND NO RELATIONSHIP BETWEEN THEM, AND TWO OF THEM HAD ALREADY
  FORGOTTEN A KEY.** The in-app **Controls** panel was a hand-written `Vec<Line>`, the **`?`
  overlay** was a second hand-written one, and the feature pages were a third. Reading the
  actual handler rather than either list found **`F2`** (a second screenshot key) and the
  **`Esc` that leaves fullscreen** bound and named *nowhere at all*, plus the quit prompt's
  `y` and the whole trade-offer picker written only inside their own dialogs, and music
  mode's `Home`/`End` written nowhere. All are in the overlay and on the page now.
- **The registry is the source of truth for three things and each is gated**: `panel_lines`
  renders the Controls panel, `markdown` generates the page between its
  `<!-- controls:begin -->` markers, and the `?` overlay is held to it by
  `the_help_overlay_names_every_key_in_the_registry`. Re-bless the page in the SAME commit as
  the key that moved — `UPDATE_CONTROLS_DOC=1 cargo test --lib controls_doc` — a plain
  `cargo test` is red until you do. Teeth proven in both directions (a probe pointed at
  absent text reddens the overlay gate; one edited word reddens the doc gate).
- **The overlay gate reads `App::help_text()`, NOT the rendered buffer, and that is
  deliberate.** The `?` overlay does not scroll and is longer than its popup, so a line past
  the bottom is a *layout* defect; conflating it with "this key is undocumented" would make
  the test unfixable. `render_help`/`render_music_help` are now thin wrappers over
  `help_text()`/`music_help_text()` for exactly this.
- **ADDING A LINE TO THE `?` OVERLAY SILENTLY DELETES ONE AT THE BOTTOM**, and it bit
  immediately: one new `Esc` line pushed the "GAME PICTURE ONLY … titled TerminalGB"
  paragraph off a 140x60 popup and turned
  `the_help_says_the_feed_is_game_only_and_where_to_go_instead` red. The fixes are the
  pattern to copy: fold the new key onto an existing line (`f/esc     fullscreen on / off`),
  and put the pointer to the written page **on the title line's second span** rather than on
  a line of its own — the top is the part of the overlay a real terminal actually shows.
- **The panel is grouped now and one row taller** (18 content rows, was 17): two per row
  *within* a group, a group never sharing a row with the next, `q quit` added to fill a gap.
  The deliberate ordering survives and is asserted — **volume stays in the third packed row**,
  because the panel clips from the *bottom* and an everyday control must not be the first
  casualty. `W` is still conditional and still last.
- **Doc-link rows are checked to resolve** (`every_control_links_to_a_page_that_exists`), and
  a group blurb naming a plugin is checked against `REGISTRY`
  (`every_plugin_a_blurb_names_is_a_real_plugin`) — which caught this work writing
  `plugins = gen1-agent` for a plugin called `agent`.
- **Both doc tests skip when `docs/` is absent**, because the published crate's `include`
  whitelist does not ship it. A skip there is correct; a skip anywhere else is not.
- **The desktop and web frontends bind the game pad and nothing else** — desktop's A/B are
  the literal letter keys, not the terminal's configurable `key_a`/`key_b`, and web's upscale
  *factor* key is `\` where the terminal's is `}`. The page says so rather than implying the
  terminal's list applies everywhere.
- Keys `4`-`9` are swallowed by the slot handler and do nothing (there are three slots) —
  recorded on the page so it does not read as a missing feature.

## Scenarios — rules the HOST enforces, and a run that replays for nothing (`src/scenario/`)

**Reader-facing page: [`docs/scenarios.md`](docs/scenarios.md); gate:
`testharness/scenario.rs` (15 checks, no commercial ROM, in a plain `cargo test`); front end:
`examples/scenario.rs`.** Behind the `scenario` cargo feature (`longplay` plus this module),
in `default`, out of `--no-default-features`, **no third-party crate** (`make embed` still
reports `blip_buf` + first-party `terminalgb-plugin-api`).

- **A RULE IN A PROMPT IS A REQUEST; A RULE THE HOST CAN CHECK IS A RULE.** Every rule is
  decided from evidence the host owns and nothing on that list can be talked out of:
  `no_memory_writes` from `Gameboy::host_writes()` **plus** every plugin's `has_written`;
  `max_calls`/`max_usd`/`max_wall_seconds` from the governor's own counters; `battle
  flee|fight` from whether battles were won or run from; `forbid`/`require` from observed
  state. A violation is a **forfeit, not a bad score** — the referee runs *before* the goal is
  judged, and the report names the rule and the frame.
- **`Gameboy::debug_write` is COUNTED now (`host_writes`), and that counter is the whole
  enforcement primitive.** One increment on a path emulation never touches (checked: every
  caller is an example or a harness), not serialised — it is a property of *this process's
  conduct*, and a save state carries no claim about how it was produced. A recording carries
  that claim instead. **Do not add a second back door into the machine without counting it.**
- **`require` is judged AT THE GOAL, not throughout.** "Must legitimately obtain a Pokémon" is
  false for the first ten minutes of every legitimate playthrough. `Referee::check` and
  `Referee::check_requirements` are separate for that reason; collapsing them makes every
  honest run fail immediately.
- **A rule the host enforces but never states is a trap.** `Referee::instructions()` puts every
  *behavioural* rule at the **head** of the agent's `notes`, and the briefing is set **before
  the first frame**, not on the first map change — the agent's opening decision lands within a
  handful of frames and a rule it hears about afterwards is one it was judged on unheard.
  Found by watching the harness: the first snapshot had `notes: []`.
- **Allowances are declared or the numbers are worthless.** `state_resets`, `rewinds`,
  `memory_writes`, `battle_advice`, `guided_decisions`, `start_state_supplied` — all printed,
  and `clean` means no writes, no rewinds, no privileged advice. Resets are the episode, not
  assistance; everything else is.
- **A RECORDING IS THE DRIVER'S REPLIES AND NOTHING ELSE, and that is why it is tiny.** Frames
  and observations are *derivable*; the replies plus four identities (cartridge title + global
  checksum, start-state hash, scenario hash, final-state hash) are not. 400 decisions over
  21,887 frames is a few hundred KB. Replay pushes them through `ScriptedDriver`, i.e. the
  **same** parse-clamp-execute path a live model takes — so a replay *is* the run, not an
  approximation, and it costs **zero model calls**.
- **`faithful()` requires final state AND frame count AND ending AND decision count.** An
  identical final state reached in a different number of frames is not a replay, and reporting
  it as one would make the artefact meaningless.
- **Measured on the captain's own Pokémon Blue** (20-core x86-64, headless): the shipped
  `scenarios/leave-the-pokemon-center.json` reached its goal in **378 frames / 2 decisions,
  clean, 0 host writes**, and replayed byte-identically (`771b5c43d6066115` both sides, drift
  0). A 400-decision random walk: **21,887 frames, run 7,709 f/s, replay 7,806 f/s = 130.7x
  real time**, same final hash, drift 0. **No live model has solved a scenario** — every run
  measured used a scripted or random driver, which is still only button presses. Say that.
- **Guidance is LEARNED, never transcribed.** A run records the doors it walked through and
  merges them into the scenario's guidance file; the next run is handed the route in words with
  the door coordinate, and is told *"no earlier run has walked from here to the goal — explore"*
  when it has not. `uses` accumulates, so a door two runs found is better evidenced. Deliberate:
  a warp table lifted from `pret/pokered` is closer to bulk game data than to a fact, and a
  graph a console actually walked is *evidence*. **`scenarios/guidance/` is gitignored** — a
  guidance file is true only of the save it was played from (the run here learned "the Viridian
  PC door leads to Pallet Town", which is true of that fixture save and of nothing else).
- **The atlas is loaded from disk, never embedded.** 237 KB of TSV in the published crate and
  the embedding build would break the `make embed` contract. `$GB_ATLAS`, else the vendored path
  walked up from the cwd. **An absent source is REPORTED missing, never silently dropped** — a
  run that planned without the reference it asked for is a different run.
- **The one privileged source is quarantined by name.** `battle_advice` would read the
  opponent's true stats out of the live machine; it is off unless asked for, announced as
  `PRIVILEGED` in the briefing *before* the run, counted per decision in the report, and a
  scenario that asks for it while forbidding fighting is refused. **It is declared but not
  wired** — the report deliberately over-counts (every decision) rather than under.
- **`testharness/support/fake_gen1.rs` is now SHARED** by `testharness/agent.rs` and
  `testharness/scenario.rs` (`#[path]`-included, the `examples/support/suite_baseline.rs`
  pattern). It gained a **working door at (8,8)** that changes `wCurMap`, so "leave the room" is
  a real goal and a map graph is learnable with no commercial ROM. **The staircase at (7,1) was
  left alone on purpose**: `agent.rs`'s "walks to the exit and presses A" case hard-codes it, and
  putting the door there makes the agent's own map-change interrupt cancel the plan — measured,
  it turns that case red. Two warps in the table now; the second is the working one.
- **The stale-rlib trap bites this work constantly** (`the trait bound AgentPlugin: Plugin is
  not satisfied`, with rustc listing `AgentPlugin` as implementing it). `touch src/lib.rs` and
  rebuild — it is the documented `panic = 'abort'` dual-rlib artefact, not your source. Budget
  for hitting it once per harness build.
- **`examples/scenario` subcommands `check` and `knowledge` need no cartridge and cost
  nothing**, and `check` prints the whole cost ceiling (`attempts × max_calls`) before anything
  is spent. A scenario that could never be judged — no goal, a condition on an undeclared watch,
  advice it could never act on — is refused *there*, not after a thousand timeouts.
- **Making a start state from the captain's Blue save, so nobody re-derives it:** the usable
  save is `~/roms/.backup/Pokemon-Blue.sav.185730-verified` (the `~/roms/*.sav` copy is
  essentially blank). `rom_script` stages the ROM as `cart.gb`, so the save must be copied in as
  **`cart.sav`** and `PHASE=relaunch` used, or the run silently starts a new game. The intro
  needs ~900 idle frames before A reaches the title screen. It lands at **Viridian Pokémon
  Center (0x29), (11,3), party of 2** — and its exits are `to_map: 255` (return to `wLastMap`),
  which on that save is **Pallet Town**, not Viridian City. That is why the shipped scenario's
  goal is `map!=0x29` and not a specific destination: a goal tuned to one save's `wLastMap`
  would be wrong everywhere else.

### The walkthrough is `docs/writing-a-scenario.md`, and its example is real

`docs/scenarios.md` is the reference — what a rule is, how a goal is judged, what a recording
holds. It is not a tutorial, and a stranger could not get from an empty file to a running
scenario with it. The walkthrough builds
[`scenarios/tutorial/town-map.json`](scenarios/tutorial/town-map.json) end to end: pick a goal,
find the byte, write the watch, write the rules, `check`, get a start state, run, replay, and
five failure modes with the output each actually prints. Every command in it was run in the
order given.

- **The example is chosen because the mask is load-bearing in it, and provably so.**
  `EVENT_GOT_TOWN_MAP` is event index 24 → `$D74A` bit 0; `EVENT_ENTERED_BLUES_HOUSE` is index
  25, **the next bit of the same byte**, and `BluesHouseDefaultScript` sets it on the frame the
  map loads. Measured both ways from one start state: with `"mask": "0x01"` the run reaches the
  goal after **1662 frames / 10 decisions at (2,4)** beside Daisy; without it, **1116 frames /
  8 decisions at (2,7)** — the doormat. `check` says `verdict: OK` for both, because nothing in
  the file can tell it that bit 1 is a different milestone.
- **Derive an event index by counting the `const_def` / `const_skip` list, then CROSS-CHECK it
  against a flag somebody else derived.** The same count gives `EVENT_GOT_POKEDEX` index 37 →
  `$D74B` bit 5, which is exactly what the shipped `09-receive-the-pokedex.json` declares.
  Agreeing with a file you did not write is what says the counting is right.
- **A goal can be unreachable because the cartridge has a precondition, and the run looks
  perfect while it fails.** `BluesHouseDaisySittingText` checks `EVENT_GOT_POKEDEX` before it
  gives anything. Started from link 08's exit the run walks the whole route, stands beside
  Daisy, talks to her, and the screen reads *"RED is out at Grandpa's lab."* — the flag never
  moves. **Read the screen text in `<out>/agent-N.ndjson` before blaming the route.**
- **A start state for a walkthrough is one command:** `pewter_chain chain --rom <rom> --seeds 1
  --to 9 --out <dir>` plays from a cold cartridge with the expert solver in **~4 seconds** and
  writes `<dir>/states/09.gbss` — Oak's Lab, Pokédex in hand. That is reproducible for any
  reader with their own cartridge, which a save state in the repository would not be.
- **Three route facts, and none is specific to Pallet Town** (all four wrong attempts are in the
  page): **a map change cancels the rest of the plan**, so a `wait` in the same plan as the walk
  that triggers a warp never runs — give the settling its own decision; **never plan from an
  observation taken during a warp** — if the map's reported `width`/`height` do not match the
  map its `id` names, the world is mid-change; and **`goto` walks, it does not navigate** —
  break a route at its turning points, or it stalls against a building.
- **`scenarios/tutorial/guidance/` is gitignored** by the existing `scenarios/*/guidance/` rule,
  so the walkthrough's own learned doors are not committed. The two `*.script.jsonl` route
  scripts *are* — a script is a specification, and it is what makes the run half runnable.
- **It ends by handing the finished scenario to the test rig** (`docs/agent/test-rig.md`), which
  is what turns one run into a measurement, and the rig's `See also` points back. **Measured
  rather than asserted: the rig accepts an arbitrary scenario and both its default arms score
  zero on this one** (`ab --scenario scenarios/tutorial/town-map.json --start-dir
  target/cert2/09/exits --runs 4` → teacher 0.0 %, random 0.0 %). That is the honest answer, not
  a fault: **the `expert` arm is the Pewter chain's own solver and has no route for a scenario
  outside the chain**, so it is not a ceiling there, and a scripted route is a fixed list of
  replies rather than an arm — it would score 100 % on the one start state it was written
  against and nothing on any other. What the rig is saying is that the missing piece is a
  *player*, i.e. a `command` arm. Do not read a zero teacher row as a broken rig.

### The agent-facing front ends are held to the AXI standard, and stderr is the trap

`examples/scenario.rs` and `tools/gen1-tables/gen1_tables.py` are driven by agents through a
shell, so they follow the Agent eXperience Interface: **TOON on stdout** (a
`name[n]{col,col}:` header then one row per line, or a `name:` block of `key: value`), a
**content-first home view** with no arguments, `help <command>` / `--help` per subcommand with
its own flags and examples, contextual `help[n]:` next steps, definitive empty states, and exit
codes 0 / 1 (the work failed) / 2 (the command line was wrong).

- **AN ERROR ON STDERR IS AN ERROR AN AGENT CANNOT ACT ON, AND THAT WAS THE WHOLE PROBLEM.**
  Both tools used to print every complaint with `eprintln!` / argparse, so `scenario run` with
  no `--rom` and `gen1_tables.py --where pikachu` both produced **nothing on stdout** and a
  non-zero exit. Every error is now `error:` plus one or more `help:` lines on stdout naming the
  command that fixes it. `argparse` is subclassed for exactly this — its own message
  (`invalid <lambda> value: 'pikachu'`) names its internals and helps nobody.
- **A resolver refuses with what it *did* understand.** `--where`, `--can-learn`,
  `--effective-against`, `--catch-for`, `--trainer` and `--plan` take a name in any case (or an
  internal index / type number), and a miss answers with `difflib` near-matches or the full list
  — `'stone' is not a Gen 1 type / did you mean: POISON`, `no species named 'pikchu' / did you
  mean: PIKACHU, RAICHU`.
- **`POKE_ROM` is the repo's own convention and `gen1_tables.py` reads it**, so `--rom` is
  optional and a bare invocation can show live content (which cartridge, which version, how many
  maps have grass) rather than a manual.
- **A signature locator's `SystemExit` had to be caught and re-emitted.** Every table is found by
  the bytes it starts with and each locator dies with a sentence *to stderr*; pointing the tool
  at the wrong file is the most likely mistake there is, so `main` wraps the whole locating block
  and reports it as a structured error naming what the tool does read.
- **`scenario`'s home view lists the scenarios it can find** (`scenarios/`, two levels, sorted)
  with goal / rule count / evidence count. **`guidance/` is skipped by name**: a guidance file is
  a `gbguidance/1` document a *run* wrote, and scanning it as a scenario put fourteen rows of red
  beside a repository where nothing was wrong.
- **`check` takes a directory** and prints one row per file plus a summary — the shape wanted
  after editing a shared field. It still exits 1 if any is refused.
- **`--prose` keeps the old paragraph output** for `check`, `run` and `replay`, which is what
  `docs/scenarios.md` §2's console block now shows; `knowledge` and `guidance` are TOON only and
  their blocks on that page were re-captured. The two `gen1_tables.py` output blocks in
  `docs/agent/pewter-chain.md` and `docs/gen1/catching.md` were re-captured for the same reason.
  **Nothing machine-parses either tool's output** — checked before changing it; only docs quote
  them, and `docs/agent/pewter-chain.md`'s check loop reads the exit code.
- **Not done, deliberately:** AXI §7 (a session hook injecting ambient context) — these are
  repository scripts a crew runs from a checkout, not an installed CLI with a setup command, and
  a hook that fires in every session for a tool most sessions never touch is a token cost with no
  reader. And `scenario` has no `--json`; `gen1_tables.py` does.

### What the A/B rig should adopt when it lands (`gameboy-abrig-k7`)

Not applied here — that crew was mid-measurement — but the same six changes are what made the
two front ends drivable, and they are cheap:

1. **Every error on stdout**, as `error: <what> / help: <the command that fixes it>`, with exit
   2 for a bad command line and 1 for work that failed. This is the one that actually breaks an
   agent; the rest are efficiency.
2. **No arguments prints live content**, not usage: what runs exist, what state they are in.
   `bin:` (path with `$HOME` collapsed to `~`) and a one-line `description:` above it.
3. **TOON for anything tabular** — `name[n]{col,col}:` then one row per line — and a `name:`
   block of `key: value` for a single object. Roughly 40 % fewer tokens than JSON and it reads.
4. **`help <subcommand>`** with that subcommand's flags, defaults and two or three examples.
   Not the whole manual.
5. **A definitive empty state**: `runs: 0 — nothing has been measured yet`, so an agent does not
   re-run with different flags to check whether the silence meant success.
6. **`help[n]:` next steps on lists and mutations**, as complete commands with `<placeholders>`
   rather than guessed values — that is how an agent discovers the surface without reading the
   source.

## The documentation has an index now, and it is GATED (`docs/README.md`, `docs/tools/docs_index.py`)

- **`docs/README.md` is the complete index; the top-level `README.md`'s *"Every page, and what
  it is for"* section is a curated shop window.** The split is deliberate and both pages say so:
  the README answers *"why would I read this?"* for a visitor at length, the index answers
  *"what is there?"* tersely for someone already inside. **They are allowed to describe a page
  differently. They may not disagree about which pages exist**, and that is the half that is
  mechanical.
- **THE MEASUREMENT THAT MADE IT NECESSARY: 57 of 101 pages under `docs/` were in no index at
  all**, including `docs/swarm-view.md` — the demo the captain rates most highly. **The failure
  mode is quiet, which is why it needs a gate**: a page nobody links to is not broken, fails no
  build, and reads perfectly well to whoever wrote it. It simply never gets read again.
- **`make docs-check` (in CI) now runs `docs_index.py --check`**, four rules, all teeth-proven:
  (1) every `docs/**/*.md` is linked from the index or on `EXEMPT` **with a stated reason**;
  (2) every row points at a page that exists; (3) a row must *say* what the page is for, so a
  bare link added to silence rule 1 is red; (4) nothing on `EXEMPT` may be linked from the
  top-level README — the one way `EXEMPT` could be abused is to silence rule 1 and then promote
  the page anyway. `make docs-index` runs it alone.
- **There is deliberately NO generation step between the two lists.** Splicing the index into
  the README was considered and rejected: they are prose for different readers and the README's
  descriptions are its own, so generating one from the other would destroy the thing that makes
  the shop window worth having. Only *coverage* is enforced. Rule 4 was also rewritten during
  the work because its first form ("every docs page the README links must be indexed") was
  strictly implied by rule 1 — **a rule that can never fire on its own is dead weight with a
  paragraph of justification attached**; make it catch the case rule 1 cannot.
- **Two `EXEMPT` entries today**: `docs/README.md` (is the index) and
  `docs/brand/exploration/README.md` (retired studies whose own first line is "do not ship
  anything in this directory" — indexing it would invite exactly that).
- **`make docs-check` still does not verify in-page `#anchor` links.** The ~25-line slug checker
  is worth re-running by hand after adding cross-page anchors; GitHub's rule is lowercase → drop
  everything not word/space/hyphen → one hyphen per space, **runs not collapsed**, so `— ` and
  ` / ` each leave a **double** hyphen (`## 4. Decorrelation — the interesting problem` →
  `#4-decorrelation--the-interesting-problem`).
- **`codemap_links.py --check` rejects a `github.com/<owner>/<repo>/blob/…` link that is not
  ours**, so the back-link to AgentGB's `docs/media/README.md` names the file in prose instead.
  Expect this the moment you cite a sibling repository's page.

### The swarm view is the worked example of convergence — say so, do not just link it

- **Not one piece the swarm view depends on was built for it**, and that is a better argument
  for how this project is built than any claim about capability: the shiny hunter's
  `HuntConfig::seed_for`/`Schedule` (built to decorrelate parallel hunts), save states (built to
  reset an attempt), the atlas's `wCurMap`/`wXCoord`/`wYCoord` (built as evidence-graded
  documentation), the raw `C` screenshot path (built for archival frames), `video::card::Canvas`
  and its 5×7 font (built so a **virtual camera** would never sit on a stale frame), the
  categorical colour guard (built to prove the terminal was not blurring half-blocks), and the
  screen anchor at tile (8, 8) (measured by the World Trade Centre work). **The compositor is
  the only new code.** That table lives near the top of `docs/swarm-view.md`.
- **Check the convergence claims against `examples/swarm_view.rs` before repeating them.** A
  brief handed to this work said the swarm view used `--video-out` to make its film; it does
  not — it writes PNGs and ffmpeg encodes them. What it actually took from the video-out work is
  `video::card::Canvas`, which is a *truer and more interesting* dependency. **The plausible
  version of a convergence story is the one that is wrong.**
- **`docs/swarm-view.md` carried two disagreeing figures for the same run** (632x in the prose,
  608x in the table; 24 × 36,000 / 59.7275 / 23.8 s = 607.8, so the table was right). Fixed by
  making the prose *point at the table* rather than restating it — the general answer whenever a
  number has no generated marker to carry it: **reference the one measured place, do not copy
  the digits.**
- **Cross-links now run both ways** from `docs/shiny-hunt.md` (§4, and why a second walker would
  have been the wrong choice), `docs/picture.md` (§4, the raw frame is what stitches the map),
  `docs/video-out.md` (the idle card's renderer outlived the card), `docs/display.md` and
  `docs/pitfalls/techniques.md` (the categorical guard, now written up as a reusable technique),
  `docs/longplay.md` and `docs/agent-environment.md` (a success rate needs a floor).
- **AgentGB publishes this run as a baseline** (`swarm-baseline.png` / `.mp4` under its
  `docs/media/`) and links back here; `docs/swarm-view.md` now links forward. Publishing a null
  model as *the floor* is the point — "our agent reached Viridian City" says very little until
  somebody has measured that undirected walking never does.

## The discovery record — `docs/gen1/discoveries.md`, and its slugs are a CONTRACT

**Every fault, quirk and corrected belief this campaign found in Pokémon Blue, with the reasoning
trail for each.** It is the one door for *"what has this project discovered about the game"*; the
findings used to be scattered across `docs/gen1/sharp-edges.md`, `docs/agent/pewter-chain.md`,
`docs/agent/battle-policy.md`, `docs/conformance-notes.md` and commit messages.

- **A HEADING'S SLUG IS PERMANENT ONCE PUBLISHED**, exactly like a `TGB-` code in
  `docs/pitfalls/codes.md`: **[AtlasGB](https://github.com/Alchemy86/AtlasGB) links back to
  `discoveries.md#<slug>` from the addresses these findings involve**, so a reworded heading
  breaks somebody else's index. Adding is free; renaming is forbidden. If an entry turns out to be
  wrong, correct its *body* and say so in it.
- **Write headings with no em dash and no ` / `.** GitHub's slug rule replaces each space with a
  hyphen *without collapsing runs*, so ` — ` leaves a **double** hyphen and the slug stops being
  guessable. Backticks, quotes and apostrophes are dropped in place and are safe.
- **`discoveries.md` and `sharp-edges.md` are deliberately two documents.** One is how it was
  found — the messy symptom, the wrong turns, the citation, the evidence; the other is the
  one-line warning, grouped by what you were doing when it bit. A finding earns a place on both.
- **An entry that corrects one of OUR beliefs is as valuable as one about the cartridge, and the
  page says which it is.** Eight of the twenty-one are ours, and two of them were found *while
  writing the page* by re-running a measurement instead of copying the number: the TACKLE accuracy
  byte, and the ledge row in link 08's own source comment.
- Link into the atlas by **symbol** (`…/by-name.md#s-<symbol>`, `#a-<ADDR>` for an address), and
  use the `/tree/main/` URL form — `codemap_links.py --check` rejects a `/blob/` link into a repo
  that is not ours.

## The Pewter chain — fourteen scenarios from a cold cartridge (`scenarios/pewter/`)

**The chain runs to link 19 now** — the gym, the badge and the training repair in front of it
are a section of their own further down, ["Past Pewter"](#past-pewter--the-gym-the-badge-and-a-precondition-nobody-had-checked-scenariospewter15-19).
Everything in this section still holds for links 01-14.

**Reader-facing page: [`docs/agent/pewter-chain.md`](docs/agent/pewter-chain.md).** Fourteen
links from the Nintendo logo to standing in Pewter City, an expert solver
(`examples/pewter_chain.rs`) that gets through all of them, and the corpus that comes out.
**Nothing is trained**; the solver is scripted and says so everywhere it is described.

- **Every goal that has an event flag uses the flag, and the ones that do not say why.** Nine of
  the fourteen milestones have no flag behind them at all and use `wCurMap` — the cartridge's own
  most exact record of them. Scenarios gained an optional **`evidence`** list naming the address
  and citation per goal condition; it is additive, so a scenario without it hashes exactly as it
  did before, and `scenario check` prints `evidence: NONE STATED` when it is missing.
- **`Watch` gained a `mask`, and it is what makes an event flag expressible.** `wEventFlags`
  ($D747) is 2,560 bits in 320 bytes, so eight unrelated milestones share `$D74B` — "Oak asked
  you to choose", "you took a starter", "you fought the rival", "you were given the Pokédex". A
  byte comparison cannot name one of them (`>= 32` is true for four different reasons). A mask
  above the bytes the watch reads is **refused where it is written**, not at attempt 1,000.
- **THE ERROR WORTH KEEPING: the Oak link scored 100% on a task the game does not ask for.** Its
  first goal was `map=0x28 AND watch:oak_appeared_in_pallet!=0` — a position plus the flag that
  *arms* the encounter. The cartridge records the milestone itself:
  `PalletTownDefaultScript` fires on `ld a,[wYCoord] / cp 1 ; is player near north exit?`
  (and the tiles at x=10,11 rows 0-1 are tileset tile `$52`, the Overworld grass tile), Oak then
  **escorts** the player, and `OaksLabFollowedOakScript` sets `EVENT_FOLLOWED_OAK_INTO_LAB` when
  the simulated `PAD_UP` x8 walk finishes. The player never walks to the lab at that point. It
  was caught by a human reading the results, not by any check we had — which is why `evidence`
  exists now. The frozen wrong version is at
  `/home/aaron/pokemon-run-versions/v001-prefix-oak-goal-error/`; **a wrong version is evidence.**
- **The route came out of a walkability grid built from the cartridge**, not a walkthrough:
  `maps/<Name>.blk` block ids + the tileset's blockset + its collision list, with the probe on the
  walk tile's **bottom-left** 8x8 tile (block indices 4, 6, 12, 14). Two findings no guide states:
  **Route 1 northbound and southbound are different paths** (one-way ledges), and **Route 2's two
  halves are separate components of the walkable graph** — Viridian Forest is the only way
  between them. A third from the same grid plus the map's object list: **the forest's three bug
  catchers are unavoidable**, because blocking their sight cones at 4, 5 and 6 tiles gives no
  route at any of them.
- **A REPAIR, AND ITS NUMBER CAME OUT OF THE ROM.** The forest link kept ending in a faint.
  `BugCatcherData` read from the cartridge gives the forest's rosters as level 6 (Weedle,
  Caterpie), 7 (Weedle, Kakuna, Weedle) and 9 (Weedle) — six Pokémon, no healing between — so the
  precondition is a lead above their top level and `11-train-for-the-forest` exists to satisfy
  `watch:lead_level>=10`. Threshold derived, not chosen.
- **THE MAP TABLES ARE REACHABLE FROM THE CARTRIDGE ALONE, AND FINDING THEM BY PROPERTY BEATS
  RECALLING THEIR ADDRESSES.** `MapHeaderPointers` is **the only** offset in bank 0 whose 248
  consecutive little-endian words all land in `$4000-$7FFF`, which identifies it uniquely; the
  bank table beside it is `$C23D`, picked out of three neighbouring candidates by which one makes
  map `$33` decode to a sane header. From there a map's header gives its tileset and block map,
  and its object data gives warps, signs and objects — a trainer object carrying **opponent id +
  roster index**, which is what ties a map to a party in `TrainerDataPointers` (`0E:5D3B`). That
  chain is how link 11's threshold is now cited end to end: Viridian Forest declares three
  trainers with opponent `202` (= 200 + `BUG_CATCHER`) and rosters 1-3, and those three parties
  are contiguous at `0E:5DCE`. **My first attempt read the connection blocks' *last* byte as the
  destination map and produced nonsense** (`$C6`-`$C9`, which are pointer high bytes); the map id
  is the **first** byte of an 11-byte block.
- **A WALKABILITY GRID COSTS TWENTY LINES AND SETTLES CLAIMS PROSE CANNOT.** Block map + the
  tileset's blockset + its `$FF`-terminated walkable-tile list, probing each 16x16 walk tile at
  its **bottom-left** 8x8 (block indices 4, 6, 12, 14 — the rule `docs/gen1/authoring-an-area.md`
  records). Two of the chain's own objective claims were verified with it, and both came back
  *stronger* than they were written: Route 2's halves are genuinely two disjoint components (245
  tiles over rows 43-71, 86 over rows 0-11, neither containing the other's gate), and **exactly
  one** of Viridian Forest's three bug catchers is unavoidable — measured per trainer at every
  sight range 1-8, with and without line of sight, only the one at `(2,18)` severs the map on its
  own. Blocking all three severs it too, which is what an earlier note over-read as "all three are
  unavoidable"; a 3,000-attempt certified run settles it at **exactly one trainer battle per
  attempt**. **Their bodies alone do not sever the map** — which is as it must be, since they are
  still standing there once beaten — so the compulsion is the cones, and it does not depend on
  knowing the view range the map script sets. The direction is in the object data: movement `$FF`
  (`STAY`) and range `$D0/$D1/$D2/$D3` = down/up/left/right.
- **`tools/gen1-tables/gen1_tables.py` reads the type chart, the wild encounter tables, the base
  stats, `PokedexOrder` and `MonsterNames` OUT OF THE CARTRIDGE**, located by signature (the same
  technique `plugins/battle-control/src/wram.rs` uses for WRAM). `PokedexOrder` lands at `$41024`,
  which is where this file already records it — that agreement is what says the signatures are on
  the right tables. So *"what beats Rock and where do I get one"* is a query. **Its answer for
  Brock on Blue is a negative one**: nothing super-effective against Rock is catchable before
  Pewter (Route 22 in Blue is Rattata/Spearow/Nidoran, not Red's Mankey), so that precondition
  has to be satisfied at the *starter* — Squirtle is Water, Bulbasaur is Grass.

### Ten things that cost real time, and every one is a general rule

1. **A machine with no APU hangs in the Pokémon Center.** `Gameboy::new` leaves `MMU.sound` at
   `None`, which is a machine with **no sound chip** rather than a quiet one. The nurse's healing
   sequence never finishes: the party heals, "OK. We'll need your POKéMON." stays up, and the game
   accepts no input again — measured, 700 decisions parked at (3,3). Same rule
   `testharness/testroms.rs` records; `sound::SilentPlayer` costs nothing. **Any harness that
   builds a `Gameboy` directly must install one.**
2. **`screen.kind == "text"` blocks movement exactly as `dialogue` does.** A box the script has
   finished with carries no `▼`, so the observer reports `text` with `waiting_for_input` false —
   and a solver that reads that as "I may walk" issues walk plans into a box that swallows them.
   **And the cure needs a bound**: Gen 1 redraws the tilemap when the player *moves*, so pressing
   A at a finished box never clears it either. Three tries at an unchanged screen, then walk.
3. **Never navigate a Gen 1 menu with a burst of presses.** `HandleMenuInput` delays several
   frames after accepting one, so the first of a burst is routinely swallowed and the rest land
   out of phase. A `down, right, A` run policy opened the party list instead — 2,500 decisions of
   it. Read the `▶` cursor and press **one key per decision**: a swallowed press simply repeats.
4. **A heal threshold must BE the fight threshold — and the general form is stronger than the
   rule it came from.** With fighting above half health and healing below a third, the lead
   settled at 8 of 23 HP — too hurt to fight, not hurt enough to heal — and ran from everything
   for 2,402 decisions. **Making the two numbers equal did not close it for good**: once the
   shared battle policy learned to weigh a *critical* hit, its "too hurt" became threat-relative
   and stopped being a number anything outside a battle could compute, and the band re-opened at
   12 of 23 — above the flat half, so no heal — where it declined 94 of 100 encounters on the one
   link whose entire job is to earn experience. **When one component decides whether an action is
   safe, no other component may decide the same thing from its own arithmetic.** The solver's walk
   to the nurse is now triggered by the policy's own refusal (`Action::Run { why: why::TOO_HURT }`),
   not by a threshold beside it. Corollary that fell out: a caller acting on a reason must match a
   **named constant**, never `why.contains("too hurt")` — fine for a report, not for a decision.
5. **A waypoint is reached when it is stood on, never "near enough".** One tile of slack broke two
   links in opposite ways: one parked at (10,2) with Oak's trigger on (10,1), and one oscillated
   for 430 decisions because Viridian City's row 27 is `#.###.##############` and the waypoint it
   skipped *was* the gap. A turning point of a BFS path is the alignment tile by construction.
6. **A stage that ends on a warp tile can never report completion**, because standing on the tile
   *is* the warp and there is no decision afterwards. Advance a per-map program counter on "was
   on the last leg", not on "completed", or a two-visit map repeats its first visit for ever.
7. **A start state captured mid-battle charges the NEXT run with the battle's outcome.** A
   `battle flee` leg then forfeits at three decisions for a fight it never had.
   `scenario::run::Battles` primes on its first sample and disowns an inherited battle.
8. **`seed | 1` collapses half a corpus.** It is `RandomDriver`'s idiom for avoiding a zero state
   and it maps every even seed onto its odd neighbour — seeds 0 and 1 produced byte-identical
   recordings, as did 2 and 3, and **no success rate would ever have shown it**. Mix the seed
   first, then force the low bit. Measuring diversity is what found it.
9. **A KNOCKOUT CENSORS A DAMAGE MEASUREMENT, so a model checked against it is scored wrong
   exactly where it is doing best.** HP stops at zero, so a hit that was going to do 12 to a
   Pokémon with 5 left is only ever *seen* as 5 — and comparing that against the model's minimum
   scores every overkill as a miss. The link that wins three fights in four read 73.9% inside the
   roll range; the one whose battles are trainer battles read 81.3%; the model was the same. Split
   the knockouts out (a knockout agrees with the model when the model's own **maximum** reached
   their remaining HP) and it is 89.5% in range, 0.6% under, and the 9.9% over is Gen 1's own
   critical-hit rate for a base-speed-45 lead. **When a harness and a model disagree, suspect the
   harness first** — it is younger and nobody has run a hundred battles through it.
10. **A METRIC THAT SPANS LINKS MUST DISOWN WHAT IT INHERITED.** A chain link starts from the
   previous link's exit state, and a goal that fires mid-event hands the next link the tail of
   that event. Link 11's goal is `lead_level>=10`, which fires the instant the level ticks over —
   inside the battle that earned it — so every link 12 run opens on "CHARMANDER grew to level
   10!" with the opponent already at 0 HP, and the battle report claimed **60 wins on a leg that
   runs from everything**. The *referee* already disowns it (`scenario::run::Battles` primes on
   its first sample); the reporting did not. Prime on the first **decision**, not on the first
   observation of the thing being counted, or a run that starts in the overworld disowns its
   first real battle instead.

### THE GAME CAN SOFT-LOCK, AND A FROZEN PICTURE IS NOT ALWAYS A FROZEN EMULATOR

**Pokémon Blue locks up for good if the player stands in rows 5-6 of Oak's laboratory during
the starter scene** — the screen keeps being drawn, the play-time counter keeps ticking, and no
button does anything ever again. Measured: 53 of 600 link-04 attempts, every one of them frozen
at `(5,5)`. It is the cartridge's own `PrepareOAMData` overflowing a 40-entry buffer whose end
**is** `wTileMap`, and the collision check then reads the stamped `$A0` as a wall in every
direction. Full mechanism with the pokered citations:
[`docs/gen1/sharp-edges.md`](docs/gen1/sharp-edges.md); the route repair and the numbers:
[`docs/agent/pewter-chain.md`](docs/agent/pewter-chain.md) §8.10.

- **The recipe for telling the two kinds of hang apart, because it took hours the first time.**
  Sample `Gameboy::debug_pc` over many frames — but know that Gen 1 spends most of every frame
  in `DelayFrame`'s spin at `$20B4`, so **PC parked there proves nothing on its own**; a
  *healthy* machine reports it too. Then compare `wPlayTime` (`$DA41`-`$DA45`) against a healthy
  machine: same rate means the game is alive and is refusing input, and the fault is in game
  state. `Gameboy::debug_ime` (added for this) separates the other case — a game genuinely stuck
  in `DelayFrame` reads `IME` low with the VBlank bit set in both `IE` and `IF`.
- **THE TOOL THAT ACTUALLY FOUND IT: diff a frozen machine against a healthy one ON THE SAME
  TILE, then delta-debug the difference down to a minimal set.** Copy the healthy value of a
  candidate byte into the frozen machine, test whether the player can move, and shrink. It went
  from 565 differing bytes to **one** — `$C434`, which is `wTileMap` row 7 column 8, which is
  `GetTileAndCoordsInFrontOfPlayer`'s `lda_coord 8, 7`. Reasoning about which flag "should" be
  wrong got nowhere for an hour; the bisect took two minutes. **Include VRAM and OAM in the
  snapshot** — a first pass over work RAM and high RAM alone reported "copying every differing
  byte does not unfreeze it", which was true and useless.
- **A cluster is a diagnosis.** All 53 failures ended on one tile, and the one classified
  `Stuck` rather than out-of-calls printed the four tiles it was orbiting. Read the endings
  before reading the code.

### A WANDER CAN CHANGE THE ROUTE, and the comment saying it cannot was wrong

`GoTo` is greedy on the larger remaining delta and commits to a run along the other axis when it
is blocked, so **where it starts from decides which obstacle it meets**. The diversity wander was
documented as something that "changes the trajectory and cannot change the route"; measured, one
wandered step up from `(5,4)` to `(5,3)` put a Poké Ball sprite between the player and Oak's
table, the detour ran south into the soft-lock above, and the attempt was over — twice in 600, at
a bar of one in a thousand. A wander now steps **towards** the waypoint on an axis that still has
distance on it, so it cannot leave the rectangle the `goto` was going to cross anyway, and 3,000
distinct trajectories in 3,000 attempts say the diversity survived. Same class as the older
"never skip a waypoint" finding, one level further out.

**And a waypoint one tile from a hazard is a waypoint pointing at it.** `(4,4)` sits behind the
rival's sprite; the only way to it is round, and round is south. The turning point of the path
that actually exists — `(5,4)` — is a tile that no detour is ever needed to reach.

### THE BATTLE POLICY IS A SHARED CAPABILITY, AND IMPROVING IT RE-CERTIFIES EVERY LINK

**Full page: [`docs/agent/battle-policy.md`](docs/agent/battle-policy.md).** Fight, switch,
item or run, decided from the cartridge's own state by
[`plugins/agent/src/battle.rs`](plugins/agent/src/battle.rs) — one implementation links 05, 11,
13 and everything after share. The observer reads the facts into the snapshot; `ChainDriver::fight`
works the *menus* and nothing else. **No link may carry a battle decision of its own**, and the
standing order is explicit: when the policy improves, every already-certified link is
re-certified against it, exactly as a driver change forces.

- **"WILD-ONLY" IS ALMOST NEVER TRUE OF A POLICY CHANGE — WORK OUT THE BLAST RADIUS FROM WHICH
  RULE THE FIELD FEEDS, NOT FROM WHERE THE SYMPTOM WAS.** `decide`'s first statement is
  `if wild && !intent.may_fight_wild { return Run { FORBIDDEN } }`, and `may_fight_wild` is true
  for **link 05 and link 11 only** — so on every other leg a wild battle is decided before any
  damage figure is read, and a wild-only change really is link-11-only. But a **trainer** battle
  never meets that guard, so anything feeding rules 3 or 4 reaches links 05 and 13 as well:
  link 05 spends **2,450 turns on stat moves** in its 3,000 certified attempts, which is rule 3
  reading `incoming_expected`. A change was very nearly landed on the reasoning "this is inside
  the wild-battle branch"; it was true of the *rule* being withdrawn and false of the
  *estimate* being kept, and the two sat in one commit. **Trace the field to its readers.**
- **`View::incoming_expected` IS NOT AN EXPECTATION, AND FIXING IT IS A THREE-LINK JOB — do not
  start it casually.** It is the maximum over the opponent's moves of the top damage roll, i.e.
  a near-worst case, which is what `incoming_max` and `incoming_crit` already carry; both of its
  readers (`theirs_turns`, `useful_stat_move`) ask expectation questions, and
  `SelectEnemyMove`'s `.chooseRandomMove` picks a wild Pokémon's move uniformly, so an honest
  figure is about **half** what it reports. Measured, unlanded, and written up with its numbers
  in [`docs/agent/pewter-chain.md`](docs/agent/pewter-chain.md) §7c — including the trap that
  the obvious correction trades one factor-of-two bias for another (flooring the roll mean, the
  accuracy and the move mean in turn reads **1** where the truth is **2.14**; accumulate at
  200x and divide once, rounded). Left alone because link 11 is signed off and correcting it
  re-opens 05 and 13.
- **`BattleTables` (`plugins/trainer/src/rom.rs`) pulls the two tables a battle needs through
  `Machine::peek_bank`** — `Moves` at `0E:4000` and `TypeEffects` at `0F:6474`, 1.6 KiB, once
  per session — so a plugin that never loaded the cartridge image still decides from the
  cartridge. `verified()` re-checks the chart's opening rows before any of it is believed. Do
  **not** slurp the whole ROM for this; the trainer plugin does that because it wants sprites.
- **The stats come out of `wBattleMon`/`wEnemyMon`, never the party.** Gen 1 copies the active
  Pokémon in and applies stat stages *there*, so those are the numbers it is fighting with and
  nothing has to re-apply a stage table.
- **THE BATTLE MOVE MENU'S CURSOR IS 1-BASED AND THE MAIN MENU'S IS NOT.** `MoveSelectionMenu`
  (`engine/battle/core.asm`) opens with `ld a, [wPlayerMoveListIndex] / inc a / ld [hli], a ;
  wCurrentMenuItem`, so `wCurrentMenuItem` reads 1 with `▶` on the first move. **Getting it
  wrong is silent** — the cursor already equals the target, A fires on the wrong move, and the
  policy's choice is thrown away with nothing in the trace to say so. Measured before it was
  believed.
- **A stat move must FLIP THE RACE, not merely be available.** An ungated "lower their attack
  while losing" spent six GROWLs of a twelve-turn fight and still lost. The rule projects one
  stage down (`StatModifierRatios`' `db 66, 100`) and spends the turn only if they then need
  more turns than we do after paying one; plus a hard cap of two a battle, because the
  projection is an estimate.
- **WATCH THE BATTLE ON EVERY DECISION, NOT ONLY THE ONES THAT OPEN A MENU.** The knock-out
  happens during the text a turn produces, so a tally fed from the menu branch alone never sees
  a party at 0 HP: it reported **300 of 300 battles won** where the exit states said 218. Any
  battle metric has the same trap. And hold a damage prediction until their HP actually moves —
  taking it on the next decision threw every check away and printed a healthy-looking
  "0 checked".
- **THREE RULES THE FIGHTING HALF OF THE POLICY NEEDS, ALL FOUND ON LINK 10** and all
  measured. (1) **A critical hit is the worst case, not the top of the roll range** — Gen 1
  substitutes `2 * Level` for `Level`, so it is ~2x, at base-speed/2 out of 256, about one turn
  in eleven; deciding "am I about to faint?" without it lost 10 of 600. (2) **Too hurt to pick a
  fight** — under half health with no Pokémon Center on the leg, experience is not worth the
  risk of collecting it; the old `fight()` had this and the first policy dropped it, which cost
  the other 6 of 600. Gate it on the battle being *wild*: there is no running from a trainer and
  a rule that leaves the policy with nothing to do is worse than no rule. (3) **A move's
  accuracy is not the number in the move table** — `MoveHitTest` scales it by the attacker's
  accuracy stage and the defender's evasion, so a Pidgey's SAND-ATTACK takes a 95% TACKLE to
  47%, and a policy reading 95 keeps swinging instead of escaping. **That one only appeared at
  3,000 attempts; 600 was clean twice.**
- **`TACKLE`'s accuracy byte is 242 = 95%, and the note here that said "240, not 242 … that is
  94%" was WRONG.** 94% of 255 is 239.7, so 240 is the byte you get by computing it from a
  percentage you already believed rather than reading one; `rom::move_accuracy_percent` rounds to
  nearest and has always reported 95, so only the prose was affected. Re-read at `Moves`
  `0E:4000`, six bytes a move, with POUND/SCRATCH/VINE WHIP/HYPER BEAM/HYDRO PUMP checked in the
  same pass so the offset and stride are not in doubt. **When you write down a number the
  cartridge holds, write down the command that reads it.**
- **AN ESCAPE IS NOT FREE, SO A FINISHER BEATS IT — but the finisher must be the WORST roll and it
  only applies when the escape is the unreliable half.** A level 5 Bulbasaur on 10 of 20 ran from a
  level 3 Rattata on **2 of 14** that its own TACKLE kills outright; the escape failed, the Rattata
  hit for 5, the faint rule ran the same play at 5 HP, that escape failed too, and the lead was
  dead. **Running does not dodge the incoming hit — it dodges it only if it works** (162 failed
  escapes in 3,000 link 11 attempts), so a failed escape and a swing cost the same hit and only one
  removes the opponent. **Two wrong versions, both measured:** on `max >= their HP` it fires on a
  *maybe*-kill and link 11 went 2,999 → **2,995 with three faints**; ungated by speed it broke two
  runs that had been walking away from *slower* Pidgeys safely, because `TryRunningFromBattle`
  scales the odds by our speed against theirs and running from something slower is nearly free.
  Correct form: `min >= their HP && accuracy >= 50 && !faster()`, which is rule 1 with its
  `accuracy == 100` relaxed to "more likely than not". Generally: **when two choices cost the same,
  prefer the one that ends the situation — and check that it really ends it.**
- **THE RETREAT THRESHOLD IS TWO *ORDINARY* HITS, NOT TWO CRITICAL ONES.** `hp <= 2 * incoming_crit`
  evaluates to 14-18 of a level 5 starter's 19 HP bar on Route 1, so the lead refused a fight after
  roughly **one hit**: 5.2 walks to the Pokémon Center per attempt, 16,000 fled battles and 162
  failed escapes in 3,000 runs — and the deaths all happen on those walks, fleeing through grass.
  A critical is one turn in eleven; two in a row is one in 121, which is the faint rule's business
  and it already tests it. `2 * incoming_max` measures better on every secondary figure (won 76.3 →
  **79.0 %**, failed escapes 162 → **111**, worst-case decisions 1,952 → **1,615**) at the same pass
  rate. **The `2 * crit` form came from link 10, which no longer fights wild battles at all — check
  whether a constant's justification still applies before treating it as settled.**
- **A CHANGE THAT MOVES THE MECHANISM THE RIGHT WAY AND THE OUTCOME THE WRONG WAY IS VARIANCE.**
  Narrowing link 11's grind box to the half of the grass nearest the exit cut failed escapes 162 →
  143 exactly as designed, and took the pass rate 2,999 → **2,997**. Each variant's surviving
  failure was a different seed. Four variants gave 5, 3, 1, 1 failures; the floor is ~1 in 3,000
  and it is a tail, not a defect. Revert, and say so.
- **AND THAT QUESTION IS NOW CLOSED WITH FOURTEEN SEEDS: eight measured configurations, fourteen
  distinct failing seeds, not one of them ever failing twice.** A defect reproduces; this does
  not. The counts (1, 3, 4, 5, 7 per 3,000) are a Poisson tail, and **2,999 against 2,996 is
  two-sided p = 0.375** — at equal exposure the conditional test of 1 against 4 failures is
  `Binomial(5, 0.5)`, so the pass rate cannot separate variants at this event rate however many
  times it is re-run. Use the precursor counters (`NEAR_DEATH`, `DOUBLE_FAIL`, `RUN_FAILED`),
  which have ~100x the event rate; move the *outcome* by changing what the leg **has** (a second
  Pokémon, a different route), not how it fights. **Do not re-measure link 11 to settle a
  policy question** — the captain signed it off at 2,999, accepted and not certified.
- **`tally::FAINTS` UNDERCOUNTS BY EXACTLY THE DEATHS THAT END A RUN.** A battle's outcome is
  scored when the battle is seen to *end*, and a party wipe ends the episode on the spot — so a
  run reporting four `party_hp=0` endings printed `faints 1`. Every configuration has the same
  gap so comparisons hold, but **the authoritative death count is the scenario's own ruling**
  (`ending_detail == "party_hp=0"` in `index.ndjson`), never the battle report.
- **NO POTION IS PURCHASABLE BEFORE PEWTER, AND THAT IS READ OUT OF THE CARTRIDGE.** The mart
  inventory table at `$02442` is `$FE`-framed count + items + `$FF`: Viridian sells
  `POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL`, and the first list containing `POTION` is Pewter's.
  Route 1 and Viridian City have no item balls either. So links 11-13 have no healing but the
  Pokémon Center, `Action::Item` has nothing to reach for, and a one-Pokémon party has nothing to
  switch to — which is why link 11's residual cannot be engineered away.
- **`StatModifierRatios` IS READABLE — do not transcribe it.** `0F:76CB`, thirteen
  (numerator, denominator) pairs, and its opening bytes `25,100,28,100,…` occur exactly once in
  the cartridge, so the address is found rather than believed. Same treatment as `TypeEffects`:
  read it, then re-check the first pair is 0.25 and the middle one exactly 1:1.
- **AN ESCAPE THAT FAILED IS NOT THE SAME AS AN ESCAPE.** Checking "did the battle continue?"
  on the next *observation* counts every run as a failure, because a run attempt is always
  followed by the text reporting it: 5,288 fled battles reported 5,372 failed escapes. Check it
  on the next *committed action* instead — 84, which is Gen 1's own run check.
- **Report the battle beside the pass rate**: win rate, HP left at the end, runs by the reason
  given, escapes that failed, faints, item spend, and how many damage predictions landed inside
  Gen 1's 217..255 roll range. A link can pass while the policy is quietly awful. Measured on
  the rival battle: 76.5% won, 32.4% HP left, 81.3% of 22,680 predictions in range, 7.8% over
  (critical hits, which the estimate excludes on purpose — and the rate matching Gen 1's
  base-speed/2-in-256 is itself a check). On Route 1, where every battle is fled: one turn and
  0.3% of the lead's HP per encounter.
- **Item use and switching are implemented and never fire in the Pewter chain** — the party is
  one Pokémon from link 04 to 14 and the chain never buys a potion. Say that rather than
  implying they are exercised.

### A LOST RIVAL BATTLE IS NOT A LOSS, AND THE CARTRIDGE SAYS SO

`HandlePlayerBlackOut` (`engine/battle/core.asm`) special-cases the Oak's Lab starter battle:
`cp OPP_RIVAL1` … then after printing `Rival1WinText`, `ld a,[wCurMap] / cp OAKS_LAB / ret z`,
whose own comment is *"starter battle in oak's lab: don't black out"*. The player stays in the
laboratory, `OaksLabRivalEndBattleScript` heals the party and sets
`EVENT_BATTLED_RIVAL_IN_OAKS_LAB` whoever won. Link 05 used to declare `party_hp=0` a failure
and ended **166 of 600 attempts about two seconds before the goal fired** — the link-03 error
in a different coat, a *failure* authored from a plausible idea rather than a goal, and again
the scenario contradicted its own objective text.

- **The rival parks on ROW 4 after the starter scene** — `(7,4)`, `(8,4)` or `(6,4)` for a
  player who took Charmander, Squirtle or Bulbasaur — and a sprite is solid. An approach
  waypoint on row 4 is therefore sometimes a tile nobody can reach: 50 of 600 attempts timed
  out, every one a seed ≡ 1 (mod 12), which is `seed % 4` (the approach) and `seed % 3` (the
  ball) together. Row 5 carries no sprite at all.
- **The fight is decided by base stats, not by type, and it is not a coin flip.** At level 5
  every starter's damaging move is Normal. Measured over 900 certified exits: Squirtle loses
  **14.7%**, Charmander **23.0%**, Bulbasaur **54.0%** — because Bulbasaur draws Charmander,
  which is faster at every level (65 against 45, so it lands the last hit) and hits 52 into 49.
  One `gen1_tables.py` query away, and it sat behind a sentence of plausible reasoning for
  months.

### Scale, determinism and the bar

- **`pewter_chain corpus` fans out across threads and `verify` proves determinism under it** —
  the same seeds played serially and in parallel must give identical final-state hashes and frame
  counts. Measured: 400 attempts at Route 1 in 72.3 s on 8 workers, **5.53 episodes/second** at
  757% CPU. A run is a function of (entry state, scenario, seed) and nothing else.
- **Quote a rate with its N and its bound.** By the rule of three, zero failures in N bounds the
  failure rate at about 3/N, so 400 clean attempts support only ≤0.75% — nine times worse than the
  99.9% per-link bar, which needs ~3,000. `corpus` prints the bound itself so a "100%" cannot be
  quoted without it.
- **Diversity is measured, not asserted**: 390 distinct end states of 400, 344 distinct routes by
  the set of tiles stood on, mean pairwise Jaccard 0.699.
  `tools/pewter-chain/corpus_report.py` reports it and draws heat maps over PixelGB's map art,
  weighted by **frames** rather than steps so a corner the solver hesitates at is visibly hotter.
- **A film is sampled, and the ratio is derived from the emulated time it covers**, never picked —
  a chain is tens of emulated minutes and compositing every frame gives a film that plays at real
  time and is dull. `tools/pewter-chain/swarm_film.py` prints the ratio it chose. **Watch the file
  before shipping it.**
- **`pewter_chain sprites` reads the player's own overworld sprite out of OAM and VRAM** (tiles
  from `$8000 + 16 * tile`, palette `OBP0`, colour index 0 transparent by the hardware's own
  rule), so a swarm film draws the game's character rather than a dot. `wWalkCounter` ($CFC5)
  counts 7 down to 0 across a 16 px step, which is 2 px a frame and is what makes the motion
  continuous instead of snapping.

### Two map names in the shipped table were wrong

`$33` was labelled "Pewter Pokémon Center" and is **VIRIDIAN_FOREST**; `$3B` was labelled
"Viridian Forest" and is **MT_MOON_1F**. The name goes to the agent in its snapshot and into every
report, so a run that crossed the forest said it was in a Pokémon Center. Fixed in
`plugins/agent/src/observe.rs` with `every_named_map_is_the_map_the_cartridge_says_it_is` pinning
the whole table against `constants/map_constants.asm`'s const-list indices. **An unnamed map is
honest; a wrongly named one is not.**

### The swarm film — five things that were not obvious (`tools/pewter-chain/swarm_film.py`)

Full description: [`docs/agent/pewter-chain.md`](docs/agent/pewter-chain.md) §11. Inspired by
Peter Whidden's swarm view; the credit is his and stays.

1. **A film drawn from `wXCoord`/`wYCoord` TELEPORTS BY CONSTRUCTION.** Measured from the tracks:
   `wWalkCounter` ($CFC5) runs 7,6,5,4,3,2,1 with each value held two frames — fourteen frames —
   and the coordinate updates on the frame it reaches 0. **During a step the coordinate is still
   the tile being left.** Reading the sub-tile offset out of WRAM fixes it only at sampling rates
   fine enough to catch it; **interpolating between consecutive recorded tiles fixes it at every
   rate**, needs no re-simulation, and is what `pwhiddy/pokerl-map-viz`'s `visualizer_live.js`
   does. Measured before/after over 120 agents: ≥12 px snaps **68% → 0.0%**, sub-4 px slides
   **25% → 100.0%**. Run the progress fraction over the last `span` frames before the arrival,
   not the whole gap, or an agent that stood still for a thousand frames drifts the whole time —
   **but `span` is MEASURED from the tracks, never a constant. See "the glide span" below: the
   hard-coded 14 was right about the cartridge and wrong about the data, and it is what "agents
   jump then pause" was.**
2. **HALF A CHAIN DOES NOT HAPPEN ON A MAP.** The starter, the rival battle, the parcel and the
   Pokédex are all indoors and a battle has no map position at all, so a pure overworld view shows
   only the walking between them and every staged link reads as "go north". The interiors are
   placed by **PixelGB's atlas manifest**, which is now the only source of any map's position —
   see "The world comes from the atlas manifest" below. *(This item used to describe a
   hand-written interior table keyed on the cartridge's warp records; that table is deleted.)*
3. **Colour the AGENTS, not the world.** A Game Boy frame is four shades already, so nothing has
   to be desaturated; the hue is **how trodden the tile under the agent's feet is**, from the same
   visitation count the heat maps use. The frontier then lights up on its own. **Declare it as an
   exception to the categorical colour guard** — the guard still applies to the map layer.
4. **Sample from the emulated time to cover, never a round number**, and say the ratio on screen.
   A chain is tens of emulated minutes; compositing every frame plays at real time and is dull.
   Two films rather than one compromise: a showcase at ~2x where the walk cycle is visible, and an
   overview at ~52x labelled a timelapse.
5. **A camera that follows the majority must say who it left behind.** Fit the 5th–95th percentile
   of agent positions (a mean would be dragged backwards by stragglers into an empty town; the
   full extent would zoom out for ever on one lost agent) and draw a persistent count of how many
   agents are off frame. `pewter_chain sprites` reads the player's own sprite out of OAM+VRAM —
   tiles at `$8000 + 16 * tile`, palette `OBP0`, colour index 0 transparent by the hardware's own
   rule — so the crowd is the game's character rather than a dot.

### Certifying a link — independence is the measurement, and it is GATED now

**A certification run that varies nothing the cartridge can see is one attempt counted N
times, and it is invisible in the verdict.** Measured: the first 3,000-attempt run of
`01-new-game` reported `3000 of 3000` and was **a single attempt, 18 decisions, one
final-state hash, three thousand times**. A duplicated corpus and an honest one both print
"3000 of 3000"; only the trajectories tell them apart. So they are counted on every run.

- **THREE AXES, AND A SEED ALONE IS THE WEAKEST OF THEM.** `pewter_chain corpus` varies the
  **entry state** (`--start-dir`, a pool the previous link produced), the **driver seed**, and
  the **idle-frame phase** (`--idle-spread`, default `IDLE_SPREAD` = 3001). The phase is the
  axis that always exists: a start pool needs a previous link and a seed needs the driver to
  reach a seeded branch, but any scenario can be entered a few frames later than last time,
  which moves `rDIV` and therefore every draw `Random_` makes. **3001 is AgentGB's measured
  constant, not a round number** — it recorded 100% under a 0-96 frame offset against 52.7%
  under a 0-3000 one, because every state in a pool is frozen during one traversal and a
  hundred frames is not long enough for a town's people to move. It is prime, so
  `seed * 337 % 3001` is a bijection and 3000 seeds give 3000 distinct phases.
- **`phase_start` burns the frames BEFORE the plugin is attached**, so the agent cannot act
  during them. `phase_for(seed, spread)` is one expression shared by `solve`, `chain`,
  `verify` and `corpus`, so `solve --seed N` reproduces `corpus`'s seed N and `verify`
  exercises the phased path rather than a special-cased unphased one.
- **THE GUARD IS WORTH MORE THAN THE BUG.** `report_independence` hashes every attempt's
  trajectory — the decision replies, the frame each landed on, the length, the ending, the
  final state — and **exits 3** on any collision, naming the colliding seeds. Teeth proven
  both ways: `--idle-spread 0` gives 4 distinct trajectories in 40 attempts and exit 3;
  the default gives 40 in 40 and exit 0.
- **`--exits N` keeps the first N *successful* exits as the next link's pool.** Only
  successes: a pool seeded from failures certifies the following link against worlds no real
  run reaches, which is the whole defect an end-to-end chain figure exists to expose.
- **Reproducing one failing attempt: `solve --start <pool sorted by name>[seed % len] --seed
  N`.** `entry_pool` sorts the paths, so the entry a seed used is `index.ndjson`'s `entry`
  field and nothing else needs recording. **Do not build that path with `$(ls …)`** — `ls` is
  proxied here and prints a size beside every name, so the array is filename/size pairs and
  every index is doubled; the trace you get is a *different* seed's, and it may well pass.
  `printf 'dir/%05d.gbss' $SEED`, or `find`. **And this shell is zsh, which does NOT
  word-split an unquoted `$VAR`** — building up `A="--start-dir X --out Y"` and passing `$A`
  hands the binary one enormous argument, and the four corpus runs that followed produced no
  index file and a stack of confusing `FileNotFoundError`s. Use an array, or write the flags
  out.
- **KILLING THE WORKERS DOES NOT KILL THE LOOP.** `pkill -f 'examples/pewter_chain corpus'`
  matches the binary and not the `for` loop that spawns it, so the shell calmly moves on to the
  next link — and if you then start a fresh run whose first act is `rm -rf target/cert`, the two
  race and you get `rm: cannot remove …: Directory not empty` and a pool of 593 states where
  3,000 were asked for. Stop a background chain run with `TaskStop`, then check
  `ps -eo pid,args | awk '/pewter_chain/ && !/awk/'` is empty *before* wiping anything.
- **A REBASE THAT MOVES THE TRAJECTORY HASH HAS NOT NECESSARILY MOVED BEHAVIOUR — DECOMPOSE IT
  BEFORE CONCLUDING.** Rebasing onto the perf work made links 02-05 reproduce **0 of 150**
  certified trajectories, which reads as a catastrophe. Comparing the *fields* instead: same
  decisions, same frames and same end position for all 150 of each, and only the final-state
  hash different — `ScenarioRun::collecting` turns pixel rendering off when there is no video
  sink, and the framebuffer is in the save state. The emulated game was untouched. The
  certification still has to be re-run so every artefact belongs to one binary, but "the hash
  moved" and "the game moved" are different findings and the index carries enough to tell them
  apart.
- **CHANGING SHARED DRIVER CODE INVALIDATES EVERY LINK ALREADY CERTIFIED, AND THEY MUST BE
  RE-RUN IN THE SAME COMMIT.** A trajectory is a function of the driver, so certifying a new
  link on a driver the earlier ones were never measured against is a claim about neither
  machine. Links 01-04 were re-certified together for exactly this reason (the wander and the
  stuck-recovery sidestep changed), on one binary, and a rebuild from the source as committed
  was checked to reproduce 200 of 200 certified trajectories byte for byte before the numbers
  were written down. Budget for the re-run: 3,000 attempts of a link is 1.5-7 minutes at 12
  workers.
- **Give a link a pool as large as its attempt count when you can.** Links 01-04 are certified
  with 3,000 *distinct* entry states, so no two attempts even start from the same world; that
  costs nothing extra, because `--exits 3000` is the same run that certifies the link before.
- **Measured, 2026-08-16, on the captain's Pokémon Blue:** links **01 to 06** each
  **3000/3000 = 100.0% (95% CI 99.87-100%), 3000 distinct trajectories**, entry pools
  1 / 3000 × 5, phase spread 3001, load average 8-33. Zero failures in 3000 bounds the failure rate
  at about 0.10% by the rule of three. **This repo's link 03 has never failed** — the "17
  budget failures on link 03" in circulation are AgentGB's link 03, a different harness with a
  60-decision cap and a 70-tile start pool, in another repository.
- **AgentGB's own certifications were audited and are SOUND — nothing to retract.** Its
  `eval_specs` varies held-out start tile, seed and idle phase, and its published corpora
  contain 3000 distinct `(tile, phase, path)` triples for 3000 records. The decisive control
  is that `random-cold` scores 541/3000 and `openloop-cold` 2437/3000 on the identical
  harness: a harness replaying one attempt gives 0 or N, never an intermediate number.

### A burst of A must stop at a menu (`plugins/agent/src/action.rs`)

**The third guard of the family that already stops `advance_text` at the naming keyboard and
at a yes/no box, and it was earned the same way.** A in a menu **chooses** something, so a
burst that crosses a menu makes a choice the driver never saw. `Step::Press` now stops a
multi-press `A` the moment `Screen::kind()` is `Menu`, and reports how many presses it had
spent. Scoped to `times > 1` deliberately: `times: 1` is a deliberate single choice and is how
every menu in this codebase is navigated, and a burst of a *direction* key moves a cursor and
chooses nothing.

*Symptom* — the Pewter chain's introduction fired `press a x8`; the "YOUR NAME?" and
"RIVAL's NAME?" preset menus opened and were answered **inside** that burst, so the rival was
named `AAAAA` from the same loop, the driver's one seeded choice in the link was never
reachable, and every certification attempt was byte-identical. *Caught by* — nothing: no test
ROM can see it (the emulator was entirely correct) and the pass rate was 100% throughout. The
independence guard above is what makes it visible.

- **`menu_cursor_row` is a SCREEN row, not a menu item index** (`observe.rs` sets it from the
  tilemap row the `▶` is drawn on). Subtracting it from an item index compares two different
  quantities; on the rival's menu it gave zero and A fell on NEW NAME. The name menu is worked
  **one key per decision, anchored on the row the menu opened at** — immune to that and to
  Gen 1's swallowed first press, because a swallowed press leaves the row where it was.

### A stuck run and a run out of budget are different findings (`src/scenario/run.rs`)

`Ending::Stuck` joins the taxonomy, decided by `looping()`: the last `LOOP_VISITS` (24) tile
changes covering `LOOP_TILES` (4) or fewer distinct tiles. **A budget failure means the
allowance was too small; a stuck failure means a bigger allowance buys a longer loop.** They
are indistinguishable in a pass rate and indistinguishable in a reason histogram with one
bucket for both, which is how a family of observation-aliasing defects stayed hidden — the
evidence being seventeen failures that all used *exactly* the cap on a route the expert walks
in fourteen steps against a sixty-step cap. Pinned by
`oscillation_is_not_a_budget_failure`, whose control case is a straight walk that must keep
its honest budget verdict.

### The chain film — stitched from the certification runs themselves

- **`tools/pewter-chain/chain_tracks.py` joins several links' `path-*.csv` into one continuous
  track per agent**, plus the `agent-NNN.links.csv` span file `swarm_film.py` reads. It is a
  real chain and not three clips: with `--exits N`, link *n+1*'s attempt `s` enters from pool
  slot `s % N`, so for every seed below `N` the mapping is the identity and agent `s` is one
  playthrough that actually happened. **Seeds at or above `N` are dropped rather than
  stitched** — splicing two strangers' runs would show a playthrough that never occurred.
- **ROUND THE FILM'S STRIDE UP, NEVER DOWN.** `step = last // frames_out` floors to 1 for any
  length past half the run, and the film then ends at `frames_out` emulated frames with the
  journey unfinished. Measured: a 2,310-frame cut of a 4,122-frame chain, silently missing the
  last 44% **including the cutscene the film exists to show**.
- **`swarm_view.rs` names map `40` as ROUTE_22 and it is Oak's Lab** (`0x28`, and
  `plugins/agent/src/observe.rs`'s table is gated against the cartridge). Same class as the two
  map names corrected in `observe.rs`: an unnamed map is honest, a wrongly named one is not.

#### The camera: five fixes, and the loudest one is not the one you would guess

Reported as *"it basically vibrates ... make the MAP the focus element and we MOVE slowly
up/down with the mass"*. Measured on the same 600-agent Route 1 tracks, old against new:

| | old | new |
|---|---:|---:|
| distinct crop widths in 2,490 frames | **1,348** | **7** |
| frames where the resample ratio changed | 1,449 (58.2%) | 15 (0.6%) |
| X direction reversals | 0.96/s | **0.00/s** |
| frames the camera is completely still (X / Y) | 19.5% / 12.6% | **86.9% / 86.8%** |
| largest single-frame jump (X) | 35.8 px | **2.0 px** |

- **The zoom width was the loudest cause and it is invisible if you only think about
  movement.** `scale = VIEW_W / cw` was recomputed from a continuously varying width, so the
  pixel art was resampled at a *different ratio on 58% of frames* and the whole picture
  shimmered whether or not the camera moved. The width now snaps to a fixed geometric ladder
  anchored on `MIN_W` and changes only on hysteresis, so a run of frames shares one exact
  scale.
- The other four: an EMA at `CAM_K = 0.028` (τ ≈ 1.2 s at 30 fps, against the old 0.14 ≈
  0.24 s, fast enough to reproduce most of the jitter it was chasing); a deadzone of 0.16 of
  the viewport per axis; a pan cap of 1.6 world px/frame; and `CAM_X_TRACK = 0.22` so a
  vertical corridor tracks Y and holds X nearly fixed.
- **The accumulator stays a float and only the OUTPUT is rounded.** Rounding the accumulator
  looks like the same thing and is a deadlock: a drift just outside the deadzone yields a step
  well under half a pixel, `round` throws it away, and the camera is stuck there for ever while
  the mass walks out of frame.
- **Judge a camera by measuring its track, never by watching and asserting.**
  `--camera-trace <csv> --dry-run` writes the per-frame camera without rendering, so old and
  new are compared on the real code path in seconds. The signature to look at is *direction
  reversals per second* and *fraction of frames completely still*, not mean displacement — a
  camera that drifts steadily and one that vibrates have similar mean displacement.

#### Debt, recorded so it is not lost

- **THREE COMPOSITORS EXIST AND TWO ARE REIMPLEMENTATIONS**: `examples/swarm_view.rs` (Rust,
  the original), `tools/pewter-chain/swarm_film.py` and AgentGB's `chain.py`. The AgentGB
  crew's own note says why: *"AgentGB composites its own ONLY because it is not allowed to
  edit TerminalGB from this worktree."* A worktree permission rule caused a rewrite, the
  rewrite lost qualities of the original, and it then happened again. **Consolidating onto
  `swarm_view.rs` is deferred, not cancelled.** What the Rust original has that the rewrites
  do not: map origins derived from **seams measured on the running cartridge at single-step
  granularity** (watching per *sample* instead let an agent walk one to three tiles in first,
  and Pallet Town landed two tiles under Route 1); map sizes read from `wCurMapWidth`/`Height`
  rather than a table, so a wrong map id cannot produce a plausible rectangle;
  `assert_no_new_colours` after every scaling step; `--verify-determinism`, which runs the
  swarm twice in one process and compares; and backgrounds stitched from the agent's own
  framebuffers rather than taken from a separate art pipeline.

#### The intro has no map, and a film drawn from `wCurMap` believes it does

**`wCurMap` is not "the map is loaded", and this is the fourth thing in the repository to be
caught by it.** Through the whole of link 01 — 825 frames, fourteen seconds of a 1x cut —
`LoadMapHeader` has not run, so the position bytes are whatever WRAM held: map `$FC` at
(26,8) for a few frames, then **map 0 at (0,0)** until the header lands. Drawn literally, all
six hundred agents stand in Pallet Town's top-left corner for the opening fourteen seconds,
the camera parks on that corner, and the bedroom panel they are actually in sits just off the
edge of frame with its agents cut in half.

That was reported as *"the interior panel is clipped off the left edge"* and it is not a
layout fault. `chain_tracks.py::settle` back-fills the leading run with the first position the
cartridge genuinely reports — the player is in Red's bedroom the whole time, only the header
is missing — which is both true and what puts the camera on the room the film opens in. Only
the **leading** run is touched, so a later legitimate visit to a map's corner is untouched.
The same trap link 01's own goal condition guards against by requiring `wCurMapWidth > 0`.

**Two general rules from the same cut:** clamp a camera's crop to the world, or a subject at
an edge is shown half off; and **wrap a caption to the panel, never let the panel cut it** —
the certification line read `links 01/02/03 each certified 3000` and stopped mid-number, and a
truncated measurement is worse than none because a reader cannot tell it was cut.

#### The world comes from the atlas manifest, and the hand-written layout is DELETED

**PixelGB's Kanto atlas landed and `swarm_film.py` reads it.** `WORLD`, `INTERIOR`,
`PANEL_GAP`, `build_layout`, `world_pos`, the connector lines and the per-map `MAP_LABEL`
table are **gone, not kept as a fallback** — two ways to place a map is one way too many, and
every one of them was a presentation guess standing in for a layout that did not exist. Nothing
else moved: the camera, the interpolation, the heat colouring, the link bars, the cohort and
the off-frame line are layout-independent and were untouched by the swap.

- **`Atlas` (`tools/pewter-chain/swarm_film.py`) is the whole interface.** `--atlas` names
  `atlas.json` (226 maps, `id`/`name`/`kind`/`scale`/`tile_px`/`rect`/`tiles`), `--rendering`
  and `--atlas-scale` pick which published image, and `place(map, x, y)` is
  `rect.x + x * tile_px`. **Honour each entry's own `scale` and `tile_px`** rather than
  assuming 16: this atlas is uniform and states it, and the manifest repeats the fields per map
  precisely so a future one that draws interiors smaller cannot silently misplace them.
- **The manifest's image paths are its own publication layout, not a local filename.** It says
  `pokemon-blue/atlas/region-tint/1x/kanto.png` and the local copy is
  `pokemon-blue_atlas_region-tint_1x_kanto.png`. `Atlas._pick_image` tries the stated path, the
  flattened form, the bare name and any file ending in the flattened stem, and prints every
  candidate on failure.
- **Carry only as much atlas as the tracks reach.** It is 7520x8048 here; the three-link cut
  visits four maps that sit in a 656x336 px cluster. The window is the union of the visited
  maps plus `--margin` tiles, so memory, crop cost and the camera's clamp are all set by the
  film rather than by geography nobody visits — and what was dropped is printed.
- **A map the atlas does not place is UNPLACED and drawn nowhere**, counted per frame and
  reported on the same line as the off-frame count. Never the origin: that is the failure that
  once put six hundred agents on Pallet Town's corner.

**THE SWAP IMMEDIATELY FOUND A DEFECT THE GUESS HAD BEEN HIDING, AND IT IS A GENERAL ONE: a
warp changes `wCurMap` one frame before it changes `wXCoord`/`wYCoord`.** Every one of the 600
agents carries exactly one row reading `OAKS_LAB (12,11)` — and (12,11) is *Pallet Town's* own
`warp_event 12,11, OAKS_LAB, 2`, while Oak's Lab is ten tiles wide. It is the same trap
`chain_tracks.py::settle` handles at boot, arriving mid-track instead of at the head. The old
layout hid it because the stray point landed on top of a neighbouring sheet; the atlas has real
gaps, so an agent stood in one, visibly, in the finished film. `Atlas.holds` drops any row whose
coordinate is not on the map it names, prints the count, and the agent holds its last real tile
until the destination arrives — which is what a warp is, and `interpolate` already refuses to
lerp across one. **Any consumer of a position track needs this, not only the compositor.**

#### The compositor measures its own film now (`report_motion`)

Two numbers decide whether a cut is any good and both were previously read off a screenshot,
which is exactly how a figure goes stale unnoticed. They are printed on every run, `--dry-run`
included, before a frame is rendered.

- **Agent displacement per output frame, in source pixels.** On the three-link cut over 975,476
  samples: median 0.00, p90 2.29, p99 2.29, **≥12 px 0.26%, <4 px 99.74%**. The 0.26% are warps
  and a warp is a jump.
- **Camera coverage** — the fraction of live agents on screen, plus the worst frame.

**A STEADY SHOT OF THE WRONG PLACE IS NOT A GOOD SHOT.** The five smoothing rules are every one
of them calibrated for *jitter*; when the swarm genuinely walks somewhere else they are
calibrated for the wrong thing. Measured here: a perfectly steady picture at **78.7%** coverage,
with stretches of hundreds of frames where most of the swarm was outside the frame. Rule 6
keys a catch-up (a larger pan cap, the X bias released, the deadzone suspended) on how far the
5th–95th box pokes past the view — **exactly zero whenever the swarm is already on screen**, so
it is provably a no-op on every frame that was already right. 78.7% → **98.9%**, with smoothness
held: one direction reversal in seventy seconds, 81% of frames still on X, three zoom levels in
the whole cut. A margin (`CAM_INSET`) keeps the mass off the boundary, because a cluster
balanced exactly on the edge is a stable equilibrium the camera will happily sit in.

**And part of that 78.7% was the MEASUREMENT, not the shot.** The off-frame test compared an
agent's *centre* against the boundary, so seventy-one agents sitting 0.15 px outside the left
edge — drawn, plainly, half in frame — were reported missing, and one frame claimed 362 of 592
lost from a picture showing nearly all of them. `on_screen` judges by the sprite. Both halves
were real, and fixing only one of them would have left a wrong number looking right.

**THE PAN CAP IS A FRACTION OF WALKING PACE, NOT A PIXEL COUNT.** It was 1.6 world px per
*output* frame, which is a glide at 1x and a crawl at 8x — each output frame covers eight times
the ground, so the camera cannot keep up with its own subject and coverage fell to 92.5%. Tied
to `walk_px` (`16 / span * step`) it scales with the compression for free and reproduces the
tuned 1.6 exactly at 1x. **And the catch-up has to lift the EMA GAIN, not only the cap**:
`CAM_K` is 2.8% of the error per frame, so a bigger cap alone changes nothing while the gain is
what binds. 92.5% → **98.0% at 8x**, and 1x improved to 99.6% as well.

#### The glide span, and the sprite anchor — two constants that were never checked against data

Both were reported by the captain as visible defects in a cut, both had a plausible derivation
behind them, and both were wrong. **A constant derived from the cartridge is not thereby correct
for the tracks you apply it to.**

- **THE GLIDE SPAN WAS 14 FRAMES AND NO STEP IN THE CORPUS IS THAT SHORT.** It came from
  `wWalkCounter` (7..1, each held two frames) and is right about the console. Measured over the
  certified chain, adjacent-tile arrival gaps are **17 frames (34%), 27-31 (40%), 52-55 (9%)** —
  a track is sampled every 2-3 frames and the solver's decision cadence sits between some steps.
  So a 14-frame glide froze every agent for 3 frames before *every* step and 17 before some:
  "agents jump then pause". `walk_span` takes the modal gap from the tracks in hand (17 here) and
  the glide never exceeds the gap.
- **AN AGENT STANDS IN THE MIDDLE OF ITS TILE AND ITS SPRITE IS LIFTED A QUARTER OF ONE.**
  `Atlas.place` returned the tile's *corner* and the renderer centred the sprite on it, i.e. half
  a tile up and left — reported as sprites straddling the left edge of a path instead of walking
  down it. **Measured, not reasoned about:** `tools/pewter-chain/measure_anchor.py` drives the
  real cartridge through `terminalgb-mcp` (`observe` for the tile, `screenshot` for the frame),
  matches the frame against PixelGB's **four-shade** atlas — photographed from the same
  cartridge, so its tile grid *is* the game's — and sweeps "the player's tile top-left is at
  screen (ax, ay)". One answer by a **9.2x** margin (1.25% of pixels differ at (64,64) against
  11.51% at the runner-up), and the 153 that differ are the player, bounding box x 65..78,
  **y 60..75**: sixteen tall, four above the 64..79 tile box. Hence `SPRITE_LIFT = 4/16`.
  **This technique generalises**: anything that needs to know where the game draws something
  relative to its own tile can be settled this way in one command, because the atlas and the
  emulator are two independent views of one cartridge.

#### Runtime and speed are not independent: `seconds_out x speed = the emulated time there is`

Worth stating as an identity, because it was argued about twice in one sitting. The three
certified links are **86 emulated seconds** for 99% of agents (154 for the slowest single one),
so 8x gives an 11-second film and a 70-second cut at 8x would need 9.3 emulated minutes that do
not exist. The fourteen-link chain's 52x is not a different taste, it is more material.

**`--cover` decides how much of the runtime is empty.** The swarm is essentially finished by
frame 4,000: at 40% through a `--cover 1.0` timeline only 50 of 600 agents are still running and
at 50% only 8, so covering the slowest straggler spends **60% of the film on fewer than ten
agents**. Measured, and it looks exactly like a film that has ended and kept playing. `0.99`.

**Sub-4px steps and a fast cut are mutually exclusive, and the arithmetic says so.** A walking
agent covers 16 px per step, so at timelapse factor X it moves ~1.9·X source px per output
frame: 1.9 px at 1x (99.7% of motion under 4 px) and 15 px at 8x, *by construction*. What
interpolation buys at speed is not small steps, it is the removal of quantisation — at 8x,
**100.0%** of moving samples land exactly on a whole-tile multiple without it and **0.0%** with
it. Quote that pair, not "<4px", whenever a fast cut is being judged. *(The 99.5%/7.6% this
entry used to carry was a one-off script's figure; `report_motion` measures it on every run
now, and the ceiling that makes the pair move at all is the next section but two.)*

#### THE SMOOTHNESS CEILING IS `stride <= one walk step`, and frame rate is the only lever

The identity above says how *long* a film is; this one says how *fast* it may be, and until
now it was unstated — which is why 8x looked like a taste call rather than a wall the previous
cut was already leaning on.

- **`stride = speed x 59.7275 / fps` emulated frames per output frame, and `interpolate` can
  only place an agent inside a step while `stride <= span`.** `span` is the step time measured
  from the tracks in hand (`walk_span`), **17** emulated frames on the certified chain. So at
  30 fps the ceiling is `17 x 30 / 59.7275` = **8.54x**, and the shipped 8x cut was sitting
  four percent underneath it. Past the ceiling an output frame swallows a whole step and the
  agent lands back on the tile grid: 12x at 30 fps measures **18.0%** quantised against 0.0%
  at 8x.
- **THE WAY PAST IT IS FRAME RATE, NOT A SLOWER FILM.** At 60 fps the ceiling is **17.08x**, so
  the original dot film's 12x needs a stride of 12 and is comfortably smooth. Measured on the
  same tracks and the same atlas, **12x/60 fps beats 8x/30 fps on every motion figure while
  being half as long again**: stride 12 vs 16, walking agent 11.3 vs 15.1 source px per output
  frame, quantisation 0.0% both, `>=12 px` snaps **1.25%** vs 20.8%, `<4 px` slides 67.2% vs
  62.8%, camera coverage 98.6% vs 98.1%. Speed and smoothness are not the trade they look like.
  Crossing the ceiling (12x at 30 fps) costs every row at once, the camera included — 96.6%
  coverage, because it has the same deadzone and pan cap and twice the ground per frame.
- **The original film's pace was 12.05x and it is in the repository, not folklore**:
  `examples/swarm_view.rs --sample 24` at 30 fps, 36,000 emulated frames per agent -> 1,500
  output frames, 50 s (`docs/swarm-view.md`). Its stride of 24 is *longer* than a walk step, so
  it teleported by construction — and did not care, because **a dot has no walk cycle to
  contradict**. Drawing the cartridge's own sprite is what made the stride start mattering.
  Runtime is the part that does not carry over: 602 emulated seconds against this chain's 86.3.

#### `report_motion`'s `>=12 px` line CANNOT judge a fast cut — quote the quantisation instead

A walking agent clears 12 px per output frame above about 6x *by construction*, so the snap
figure saturates at "the swarm is walking" exactly where a pace decision gets made. It is a
teleport detector for 1-2x cuts and nothing else. The scale-free measure is **what fraction of
moving samples land on the tile grid**, which `report_motion` prints now; warps are excluded
(a warp really is a whole-tile jump, and counting them credits the renderer for the
cartridge's own teleports). Teeth proven by neutralising `interpolate`: **100.0%** at both 8x
and 12x, against 0.0% and 18.0% with it.

#### The anchor was right and the ATLAS was the lie — and a sweep must be allowed to fail

Reported a second time as *"the sprites are over to the left a little"*. Re-run,
`measure_anchor.py` reproduces to the pixel — screen (64, 64), 1.25%, **9.2x margin**, player
box **x 65..78**, whose centre 71.5 is the tile's own centre of 72. **The sprite is dead centre
on its tile by measurement**; there is no leftward bias to find, and the measurement never
consults where a map is *placed*, so it is structurally immune to a layout change. What the
report was looking at was a stale atlas bundling every Pallet Town interior to the left of the
town whatever side its door is on.

**And the tool printed a confident wrong answer on the way to establishing that.** Pointed at a
save that continues into the Viridian Pokémon Center, whose NPCs are in the frame and not in the
atlas, every hypothesis differs by ~36% and the best is arbitrary — it announced
`SPRITE_LIFT = 0.0000` at a **1.0x** margin in the same sentence shape as the real measurement.
**A sweep always has a winner and a winner is not an answer.** It refuses below `MIN_MARGIN`
(3x) or above `MAX_MISMATCH` (10%) and names the scene that works: a ROM with no `.sav` beside
it starts a new game in Red's bedroom, which has no NPC in shot. Any future sweep-shaped
measurement in this repository needs the same guard.

#### A shorter longest warp is how you check a map layout landed

The corrected Kanto atlas puts each Pallet Town interior on the side its door is actually on
(Red's House left, Blue's House and Oak's Lab right) instead of bundling all of them to the
left of the town. Nothing in the compositor changed, and **the longest displacement in the film
fell from 449.14 to 244.75 source px** — a warp's jump *is* the distance between a door and the
room behind it, so the film's own motion tail is an independent check that a layout is right.
`p99` moved 192.00 -> 144.89 with it. Cheap, and it needs no eye on a picture.

#### ONE LINK OWNS THE RUNTIME OF AN ELEVEN-LINK CUT, AND `--cover` CANNOT HIDE IT

Summed **per seed** over the certified 3,000-attempt runs (the `--exits 3000` identity mapping
means seed `s`'s eleven links really are one playthrough, so this is a distribution over 2,999
whole chains and not a sum of medians): **links 01-10 are 10.7 emulated minutes at p50 and 12.3
at p99 → a 61 s film at 12x; links 01-11 are 28.7 and 45.2 → 3 min 46 s.** Link 11 alone is
18.0 min at p50, i.e. **63% of the median chain and 77% of the p99 one**, so it is also 63-77%
of the film. That is the training grind `docs/agent/pewter-chain.md` §8.8 describes and there is
no pace that fixes it — the identity `seconds_out x speed = the emulated time there is` says the
runtime is the material. **Report both cuts with the split rather than picking one**; 01-10 is
also the chain that is actually certified (link 11 measures 2,999/3,000).

**`--speed 12` NOW, NOT `--seconds`.** `step = ceil(last / frames_out)` is a whole number of
emulated frames, so typing a runtime to hit a pace does not work: `--seconds 29.7` at 60 fps
over the six-link stitch lands on step 13 and a 13.06x cut while the caption says 12x. Measured,
and only caught because the tool prints the ratio it chose — so **read the `Nx timelapse` line
every time** whichever flag you use. `--speed` inverts it exactly (`frames_out = ceil(last /
stride)`) and is the identity used as an interface.

**AND `--cover` IS THE SHARPER HALF OF THE LENGTH QUESTION, NOT THE PACE.** How full the frame
is, measured on the two cuts' own tracks at `--cover 0.99`: at 60% through, **01-10 still has
600/600 agents running and 01-11 has 282/600**; at 80%, 420 against 61. The ten-link cut holds
the whole swarm for 70% of its runtime; the eleven-link one is under half by 60% through,
because link 11's own length is long-tailed (median 63,574 frames against a p99 near 124,000) so
agents dribble rather than crossing a finish line together. So the honest statement of the cost
is **"the back half is a thinning crowd"**, not "63% of it is grinding" — and the dial that
moves it is `--cover`, not the pace.

**`walk_span` MOVES BETWEEN CORPORA and that is the argument for measuring it**: 17 emulated
frames on the three-link chain, **29** on the eleven-link one. A hard-coded 14 would freeze the
long cut for 15 frames before every step and a hard-coded 17 for 12 — which is exactly the
"agents jump then pause" report, one corpus later.

#### `wIsInBattle` IN A TRACK IS THE RAW BYTE — `if battle:` IS A DEFECT

A path row's last column is `$D057` verbatim (`src/scenario/run.rs`, `PathPoint::in_battle`):
**1 wild, 2 trainer, `$FF` blacked out** — and `$FF` is the *end of the run*, not a fight. The
compositor's `if battle:` was truthy for all three, which is the third instance in this
repository of the same class (`plugins/trainer/src/encounter.rs`'s catch-all `_ => Trainer` drew
a whole stale battle panel in the overworld; `BattleKind::from_flag` now enumerates). Only 0, 1
and 2 occur in the corpus today — the point is that the fourth value cannot quietly become a
battle marker. `BATTLE_KIND` maps the two that are battles and everything else is not one.

**And a fighting agent needs the reason on screen, not just a mark.** A battle has no map
position, so links 05 and 11 draw a swarm frozen on its tiles; link 11 is *nothing but* pacing
grass and fighting, so that is most of the film. A halo **under** the sprite (the sprite is 16
source px and already carries the heat colour, so a mark on top fights the encoding the legend
explains, and a 2 px outline ring is invisible in a crowd of 600), alpha so the map still reads,
drawn in one overlay pass before any sprite so a halo cannot land on a neighbour, plus a live
HUD count and a two-swatch legend. Wild and trainer are coloured apart because on this chain
every trainer battle is the rival in Oak's lab and **250 agents turning orange in one room** is
the picture. What is *not* built is the framebuffer collage (`/home/aaron/pokemon-run/
FILM-STAGES.md`) — that is the only thing that can show a lab or a battle as itself, and it is a
different project from placing a marker honestly.

#### A RAW ROW LIST CANNOT HOLD AN ELEVEN-LINK CHAIN — REDUCE AS YOU READ

A track row is written whenever position, facing, the walk counter or the battle flag moves, so
a *walking* agent costs a row every couple of frames. Measured: **65,463 raw rows per agent for
link 11 alone, 39.3M across 600 agents**, about 5 GB of Python tuples — on a box whose free
memory is single-figure gigabytes because several crews share it. The three-link film got away
with holding everything because three links is 5,156 frames. `Corpus` reduces each file as it
streams: the tile-change list (**8,004 rows per agent for the same link, 4.8M across the
swarm**), the battle flag's change points, and the visitation count. **It is a reduction, not a
sampling** — every figure is still computed from the raw rows, the visitation count still
weighted by the gap between consecutive *raw* rows, and the three-link cut reproduces
bit-identically (same 600 straddles, 119 tiles, 558,614 agent-frames on the hottest, same motion
figures, same 98.6% coverage). That equality is the check to repeat after touching the loader.
`positions` is then the ceiling: 13,560 output frames x 600 agents is **1.75 GB measured**, so a
much longer cut needs the camera folded into the position pass (it is already causal) rather
than a bigger box.

#### A MEAN AND A WORST FRAME CANNOT TELL A TRANSIENT FROM A STRETCH

A swarm splitting between two interiors at a warp loses half the crowd for a handful of frames
and recovers; a camera calibrated for the wrong scale loses it for hundreds. **Both report the
same mean and the same worst frame.** So the coverage line carries percentiles and a count of
frames below 90% now, and `report_camera` prints the five smoothness figures the camera rules
were tuned against (distinct crop widths, frames the resample ratio changed, frames still per
axis, direction reversals per second, largest single-frame pan) — they lived in a one-off script,
which is exactly how "1,348 distinct crop widths in 2,490 frames" becomes unverifiable.
**The rules tuned over four rooms needed no change for eleven links**, and the long cut is
*better* covered than the short one — 1.2% of frames below 90% against 4.9%, median 100% both —
because the mass is spread over open ground rather than split between two panels. Measure before
retuning a camera for a bigger journey; the intuition that it must need it is wrong here.

#### THE STILLS ARE IN GIT AND THE VIDEOS ARE NOT (`docs/agent/chain-stills/`)

The captain's gap, and it was real: *"the repo has the numbers but no PICTURES"* — every film and
still lived in his media folder, so a reader of `docs/agent/pewter-chain.md` could read what
happened and not see it. Six stills from the eleven-link cut are committed now (§11a describes
each), and **the mp4s deliberately are not**: a 190 MB artefact does not belong in git whatever
is in it, and the two-line ffmpeg recipe regenerates it.

**It sits on the line `docs/link-trade/` already established, not past it.** These are the
compositor's *own* output — HUD, heat colouring, battle halos, link bars and 600 of our sprites
over a cropped camera view — so the test *"our interface, doing something, with the game
incidental"* is met. They are not an asset comparison, which is why `docs/upscale-samples/` stays
MIT-ROM-only, and nothing in them can be read back as game data. Do not widen this to a gallery
of game frames.

**Sizing, following `docs/shots`' own budget (744 KB for ten):** extract from the mp4, then
`magick <f> -resize 900x -strip -colors 64 PNG8:<out>` — 1280x720 film frames come out ~100-170 KB
each and the HUD text stays fully legible, against ~5 MB raw. **Image directories here carry no
README** — the descriptions go in the page that references them, which is also where the captain
asked for them. And embed *one* inline as a hero plus a table of links: six 900 px images in
table cells is unreadable on GitHub, and a page with no inline picture does not fix the gap.

#### EVERY HUD ROW WRAPS — the rule belongs to the row list, not to one row

The `--method` line was wrapped once because it read *"links 01/02/03 each certified 3000"* and
stopped mid-number. Two more rows then went the same way in one sitting, both caught by *looking
at a frame* and by nothing else: the title (`THE PEWTER CHAIN 01-1`, cut at a digit) was the one
string drawn at a fixed position and never measured, and the new battle count came out as *"in a
battle: 69 trainer, 117 w"*. **A truncated measurement is worse than none, because a reader
cannot tell it was cut** — so `wrap_hud` is applied to the whole row list and to the title, not
to whichever row last embarrassed us. Same rule as the status panel's `fit_bar`: shorten
deliberately or wrap, never clip. And **crop the HUD out of a rendered frame and read it** before
shipping a caption; no test in this repository can see a clipped string.

### The fourteen-link cut broke two things the eleven-link one hid (`swarm_film.py`)

The film to Pewter City is the same compositor over three more links, and extending it exposed
two defects that were invisible while the chain stopped at the forest's door. Full record and
every number: [`docs/agent/pewter-chain.md`](docs/agent/pewter-chain.md) §11b.

- **"THE CAMERA FOLLOWS THE MAJORITY" HAD SILENTLY BECOME "IT FOLLOWS THE SURVIVING MINORITY."**
  An agent that finishes stops being drawn, so as a long chain drains, the 5-95 box is fitted to
  whoever is left — which is the slowest. Measured on the fourteen-link cut: the Pewter arrival
  tile is in frame on **0 of 12,091 frames**, coming within **46 px** of the top edge and never
  arriving, because the slow half is always 2,400 px south in Route 1's grass. Counting arrived
  agents at their arrival tile puts Pewter in frame for the **last 1,033 frames (17 s)**. **None
  of the six camera rules changes** — only the population their box is pointed at — and the trade
  is stated rather than hidden: coverage of the *live* agents 85.9% → 82.9%, its 1st percentile
  58.6% → 32.8%. `--no-follow-arrivals` reproduces the older shot exactly.
- **A FILM THAT ENDS SOMEWHERE MUST DRAW SOMEBODY THERE, AND BY DEFAULT IT DRAWS NOBODY.** Link
  14's goal fires the instant the player crosses into Pewter, so its whole stay is one row of the
  track, and **all 600 agents finish on the same tile** — at most one is ever visible. An agent
  that completes the last link now leaves a **counted marker** where it arrived, plus an
  `N/600 arrived` HUD row. Not a sprite (600 sprites on one tile is one sprite) and **not folded
  into "agents live"** — arrived is not live, and merging them misreports both.
- **A COVERAGE FIGURE MUST SAY WHETHER THE SHOT WAS MISSED OR WAS NEVER AVAILABLE.**
  `report_impossible` measures it: the camera may not widen past the world it crops from (a wider
  crop is black bars), so the widest view is `world.width` by `world.width * OUT_H / VIEW_W`. The
  fourteen-link atlas window is **2496x3936**, so the tallest view is **1755 px**, and the 5-95
  swarm box **does not fit on 5,207 of 12,091 frames (43.1%), every one too tall** — against
  5,211 frames below 90% covered, i.e. the shortfall is *entirely* geometric. The same
  measurement on the eleven-link cut is **0 frames**, which is why it scored 99.3% with identical
  rules. That is the correction to the older note above claiming the rules needed no change for a
  bigger journey: true for eleven links, and it was true because the geometry still fitted.

#### A STAGE'S SHARE OF A FILM HAS TWO RIGHT ANSWERS AND THEY DIVERGE BADLY

`report_stages` prints both, because picking one is how a share becomes wrong. **agent-time** is
the stage's share of all the emulated time the swarm spends — the chain's own shape. **on
screen** is the share of output frames on which that stage is what most of the *live* swarm is
doing — what a viewer watches, and the partition the on-screen timeline draws. Measured over
fourteen links they disagree by a factor of nearly two in both directions: link 11 is 58.2% of
the journey and 64.7% of the film, link 13 is 9.1% and **15.6%**, and links 12 and 14 are real
stages that are **never the plurality of anybody** (0.0% on screen) because agents that finish
stop being drawn and the surviving population drifts towards the later links. A stage with no
timeline segment is still in the HUD's per-link bars every frame.

**The timeline is what stops a long stage reading as a stuck picture** — one pixel column per
output frame, coloured by that frame's stage, labelled, playhead crossing it, `now: link 11 …
65% of this film` above. The segment widths *are* the "on screen" column, so the picture and the
printed table cannot disagree.

#### `--cover` CUTS STRAGGLERS, NEVER A STAGE — and a runtime estimate needs it

Measured on the fourteen-link chain: **the training stretch is 130 s of film at every cover
value**, because it is the same stretch of emulated time either way; all `--cover` decides is how
much thinning tail follows. So it is the only honest lever on a film whose middle is one long
stage — running link 11 faster than the walking would make the film lie about the most
interesting thing the chain has measured. `report_cover` prints the trade at `--dry-run`, with
the column that decides it: **how many agents are seen to finish the last link**. Chosen 0.90 —
541/600 land the ending, 3 m 21 s.

**AND A RUNTIME ESTIMATE THAT CARRIES THE IDENTITY BUT NOT THE COVER IS AN ESTIMATE OF A
DIFFERENT FILM.** `docs/agent/pewter-chain.md` predicted "another 9 emulated minutes and about
45 s of film" for the fourteen-link cut. The material did grow (median chain 28.7 → 31.3
emulated min) and the film got **shorter** — 3 m 36 s → 3 m 21 s — because the cover went
0.99 → 0.90. Quote `seconds_out x speed = the emulated time there is` *and* the cover, or the
arithmetic is right about a film nobody made.

## Past Pewter — the gym, the badge, and a precondition nobody had checked (`scenarios/pewter/15-19`)

**Reader-facing page: [`docs/agent/brock.md`](docs/agent/brock.md).** Five links from arriving
in Pewter to standing on Route 3 with the Boulder Badge, plus a repair link before them.
**Certified at the chain's own gate — 3,000 attempts a link, in order, each entering from the
previous link's exit pool, 3,000 distinct trajectories every time:** 15 train **3000/3000**,
16 heal **3000/3000**, 17 gym **2659/3000 (88.6 %, [87.4-89.7])**, 18 heal **3000/3000**,
19 east **3000/3000**. **Four of the five clear the 99.9 % bar; the gym does not, and that is a
finding rather than a defect** — see the Charmander entry below.

- **BROCK IS THE FIRST LINK THE CHAIN HAS TO *WIN*, and that is what exposed everything below.**
  Every battle link before it has a goal the cartridge sets whoever won —
  `HandlePlayerBlackOut` special-cases the starter battle, so link 05's flag is set after a loss
  as much as after a win, and the test rig measured the expert passing that link 300 of 300
  while losing the fight up to 48 % of the time. A badge bit only sets if Brock is beaten, so
  this is the first link whose pass rate measures battling rather than surviving.
- **THE CHAIN'S PRECONDITION WAS BROCK-BLIND, AND THE CAUSE IS A MOVE, NOT A NUMBER.** Link 11
  raises the lead to level 10 because Viridian Forest's bug catchers top out at 9. Nothing had
  ever asked what Brock needs. Measured at level 10, 40 attempts per starter: **SQUIRTLE 90.0 %,
  CHARMANDER 20.0 %, BULBASAUR 0.0 %**. Brock's GEODUDE Lv12 and ONIX Lv14 are both
  **ROCK/GROUND**, so WATER/GRASS/FIGHTING/GROUND/ICE are 2x against each half and **4x** in
  total; reading `EvosMovesPointerTable` for the three lines, SQUIRTLE holds BUBBLE from level
  **8**, BULBASAUR holds VINE WHIP from **13**, and CHARMANDER's line holds **nothing on that
  list ever** — EMBER is FIRE, which is 0.5x on ROCK. A Bulbasaur below 13 does **one damage a
  turn** with 35 PP against 33 HP; it does not lose slowly, it cannot win.
- **Brock's stats were read back out of `wEnemyMon` in the running game, not merely computed.**
  The table gives the level and the species; the stats come from a formula and a DV pair, so
  they were checked at `$CFE5..$CFFD` on real mid-fight states: GEODUDE Lv12 33/26/30/11/14,
  ONIX Lv14 36/20/52/26/15, and `wEnemyMonDVs` reading **`$98` `$88`** — the fixed trainer DVs,
  out of the machine rather than out of a memory. **Geodude curls to Defence 120**, four times
  its base, which is why a Normal move is worthless against it and the policy takes the Fire
  one — special, against a Special of 14 — despite it being 0.5x.
- **CHARMANDER CANNOT REACH THE 99.9 % BAR AND MORE LEVELS DO NOT FIX IT.** Certified over a
  thousand attempts per line: **WARTORTLE 1000/1000 [99.6-100], IVYSAUR 989/1000 [98.0-99.4],
  CHARMELEON 670/1000 [64.0-69.8]** — 330 of link 17's 341 failures are Charmeleon, 11 are
  Ivysaur, **none is Wartortle**, and **every one of the 341 ended on Brock's own tile** from 341
  *distinct* entry states. Across thresholds it is 35 % at 14, **67 % at 16**, 65 % at 18 — a
  plateau, not a curve. What would fix it is a second Pokémon
  that answers ROCK/GROUND or items; both change the chain well before Pewter (the bag is empty
  at every link, and `gen1_tables.py --plan brock` puts every catchable answer on Route 4, Mt
  Moon or Route 24, all past the gym). Neither is taken; both are named. `BROCK_TRAIN_TO` is
  **16**, which is where all three lines evolve — the only species-independent number in the
  neighbourhood, and the best that line does.
- **THE BATTLE NUMBERS ARE THE POINT ON THIS ARC, AND THREE OF THEM ARE WORTH READING TWICE.**
  **Zero faints across link 15's 216,850 training battles** — all 11,829 of its runs were the
  policy declining a fight it judged unsafe, not escaping one it was losing (7 escapes failed in
  the whole run). **The gym is won at 92.6 % HP or lost outright**: there is no middle, because a
  Wartortle or an Ivysaur two-shots both of Brock's Pokémon and a Charmeleon does not. And **not
  one item was used in 220,000 battles**, which is the empty bag stated as a measurement rather
  than as a premise. The damage model checks out at **98.2 % of 103,669 non-knockout predictions
  inside the 217-255 roll range** on link 15 (86.2 % of 11,354 in the gym, its higher over-rate
  being Onix's base speed of 70 against a wild Pidgey's 56) — measured against **the move the
  policy chose**, which it would not have been before the fix below.
- **THE BADGE BIT WAS EARNED, NOT TRANSCRIBED.** Snapshot, play the gym, snapshot, diff **all
  320 bytes** of `wEventFlags` plus the badge bytes: exactly five move, and every one is written
  by a single block of `PewterGym.asm`'s `.gymVictory`. `wObtainedBadges` `$D356` bit 0
  (`BIT_BOULDERBADGE`), `wBeatGymFlags` `$D72A` bit 0, `$D755` → `$C4`
  (`EVENT_BEAT_PEWTER_GYM_TRAINER_0`, `EVENT_GOT_TM34`, **`EVENT_BEAT_BROCK`**), and `$D7EB`
  `$81` → `$00` — two bits **cleared**, which are the Route 22 rival flags the same block
  resets. A clearing in a flag diff is not always a loss; check the script before alarming.

### `wLastBlackoutMap` proves the FIRST heal at a Center and nothing proves a repeat

- **`$D719` holds the TOWN, not the Center's interior map** — `$01` (Viridian City) all the way
  to Pewter on the certified pool, `$02` after Pewter's nurse. Measured both halves apart:
  **entering the building leaves it at `$01`**, talking to the nurse flips it. That is a proof
  of the interaction that "the party is at full HP" cannot give, because a party that was never
  hurt is at full HP too.
- **It flips at the START of the dialogue, before `HealParty` runs.** A run whose goal was the
  flip alone stopped at the counter with the lead on 26/29. So the goal is the flip **and**
  `party_missing_hp=0` **and** being in the building.
- **`party_missing_hp` is new** (`Progress`, `src/longplay/progress.rs`): total maximum less
  total current HP across the party. A goal compares one number against a constant, so "current
  equals maximum" is otherwise inexpressible — the maximum moves with the party and with every
  level. `party_missing_hp=0` is how a healing link is judged.
- **A REPEAT HEAL AT THE SAME CENTER WRITES NOTHING THE CARTRIDGE KEEPS**, so link 18's goal is
  the strongest claim available and not a proof: *whole, and standing inside the Pokémon
  Center*. It forces anyone hurt through the nurse and cannot be satisfied by walking past the
  door; it cannot tell a party that healed from one that arrived whole. Stated, not papered over.
- **Both heal links therefore FINISH INSIDE the building** and links 17/19 begin by walking out.
  That removed a real failure: an earlier link 18 with `map=0x02` fired at the **gym** door for
  every attempt that came out whole — 23 of 60, in under ten decisions, never going near the
  nurse. **A goal satisfiable at entry is a goal that measures nothing.**

### Route 2's NORTHERN grass trains fast — and cannot help link 11

- **Map `$0D`, rows 2-7, 48 grass tiles** (found by reading the tileset's own grass tile `$52`
  out of the block map), nine steps from the map's north exit and fifteen more to Pewter's
  Pokémon Center. **Level 10 → 14 in 63,666 frames**, 625 decisions, 35 battles, no faints —
  against a Route 1 attempt that **could not reach level 12 in 600,000 frames, 30 of 30**.
- **IT IS THE NORTHERN HALF, i.e. AFTER THE FOREST.** Route 2 is two disjoint components of the
  walkable graph with Viridian Forest the only way between them; re-checked here, a BFS from the
  southern grass to the northern returns **no path**. Nobody may read this as a fix for link 11,
  whose whole job is training *before* the forest.
- **ROUTE 22 IS ABOUT 8 % FASTER THAN ROUTE 1 FOR LINK 11's JOB, AND ROUTE 2 SOUTH IS ABOUT
  11 % SLOWER.** Both run on that job — level 5-6 to 10, the certified link-10 pool, same seeds,
  same workers, **paired on the seed**: Route 1 **40/40** at a median 65,904 frames; **Route 22
  40/40 at 60,474**, mean paired difference **−7,709 (se 1,723, t = −4.47)**, faster on **29 of
  40 seeds**; Route 2 south 34/40 at 72,294, **+13,999 (t = +3.46)**, faster on 6 of 34. Link 11
  is 58 % of the chain's emulated time, so 8 % off it is ~4.7 % off the whole run — modest, real,
  and **link 11 was NOT changed**: the instruction was to measure and say.
- **The exp-per-step table alone does not predict the winner, which is why they were run.** Exp
  per step: Route 2 2.87, Route 22 2.64, **Route 1 2.44**, Viridian Forest 1.24 (its *rate* is
  8/256 against everyone else's 25, so despite being 59 % richer per encounter it is half as
  good per step). Steps to the Viridian nurse: Route 1 **25**, Route 22 **43**, Route 2 south
  **56**. Route 2 south has the richest table of the three and still loses, because its grass is
  24 steps from its own exit before the city crossing starts.
- **AND AN ARGUMENT FROM TWO MEASURED NUMBERS IS STILL AN ARGUMENT.** An earlier draft did not
  run Route 22: it reasoned that Route 22 "loses on both terms to a candidate that already lost",
  from a nurse distance of **79-82 steps**. That number was wrong — the breadth-first search
  behind it started from a tile that is not walkable, and the real distance is **43** — and the
  conclusion inverted the moment it was fixed. Run the experiment.
- `scenarios/experiments/` holds both; `chain_files` reads only `scenarios/pewter`, so nothing
  there can join the chain by accident.
- **AND THE LEVER ON LINK 11 IS NOT THE LOCATION.** Per-map frames from one run's own position
  track: **65.7 % of it is inside battles**, 16.8 % is the round trip to the nurse. A perfect
  nurse saves at most a sixth, and Route 1 already has the closest one. The cost driver is
  **~1,880 frames — 31 seconds of game time — per wild battle**, and the Route 2 link measures
  the same 1,819: it is the game's own animations and text, identical on every map.

### Two counters were reporting the wrong thing, and both fix published numbers

- **A RUN THAT ENDS MID-BATTLE SCORED THE BATTLE AS A WIN.** `Drop for ChainDriver` scores the
  last battle from the driver's last view, which is decisive in an ordinary win or loss because
  the driver keeps deciding while the "…fainted!" text plays. A run the harness stops on
  `party_hp=0` has no such view — the last decision was one hit before the end, so the lead is
  standing in it and dead in the machine. Measured: a level 10 BULBASAUR fainted to the junior
  trainer, final state `wIsInBattle = 2` and 0/30 HP, and the report said
  `battles 1: won 1 (100.0%) … faints 0`. An indecisive view is **unresolved** now and named;
  link 05's report is unchanged (29 won, 11 lost of 40, both sides).
- **THE DAMAGE-PREDICTION COLUMN MEASURED THE WRONG MOVE.** It recorded the first *damaging
  usable* move rather than the one chosen — the same move whenever the best one is in slot 1,
  which is every link before the gym, because a starter's only attack is slot 1. A SQUIRTLE
  holds TACKLE, TAIL WHIP, BUBBLE and the policy picks BUBBLE. The column read **29.9 % inside
  the roll range with 474 of 700 "over"**; against the chosen move it reads **87.9 % with 68
  over**, which is what a critical-hit rate explains. Link 05's control moved 85.8 → 87.8 %.
  **So the 81-85 % damage agreement this project has published was measured against the wrong
  move on any turn the policy did not pick slot 1.** Both fixes change only the measurement —
  the gym link scored 39/120 before and after.

### Two more route traps of the shape this chain keeps finding

- **Pewter's south-west pocket.** Column 18 is solid on rows 18-20 and row 21 is solid from x=0
  to x=18, so the left half of those rows has one way in and out, and a `goto` heading
  north-east from the Pokémon Center walks into it: **23 of 23 failures of the Route 3 link were
  stuck on row 20 between x=13 and x=17**, with no battle involved. East along row 26 first,
  then straight up column 19, is 3000/3000.
- **The gym's junior trainer IS avoidable and column 1 is what avoids him.** Rows 3 and 9 of map
  `$36` are solid but for x=4 and x=5, so the straight climb up column 4 crosses his rightward
  sight line at (4,6); column 1 runs from row 8 to row 4 entirely to his left. Both measured at
  120 attempts from the level-10 pool: **fighting him 39/120 with 69 of 81 failures on his own
  tile**, avoiding him 44/120 with every failure at Brock's. At level 10 the junior trainer
  killed more of the chain than Brock did.

### `pewter_chain states` — check a pool before you write a goal against it

- **`states --start-dir D [--peek NAME@0xADDR]`** prints what a pool of entry states actually
  holds — lead species, level, HP, moves, money, badges — and any raw address you name. No
  frames are emulated; a save state carries the party. **A link's precondition is a claim about
  its entry pool**, and until this existed the only way to check one was to run the link and
  read the failures. It is also how the badge bit and the heal flag above were settled: *read a
  byte out of real states before writing a goal against it.*

### The gym arc films SEPARATELY from the journey to Pewter

Captain's instruction, and it is arithmetic rather than taste: the fourteen-link cut is 190 MB
and 3 m 21 s, and **58 % of its emulated time is link 11 alone**, so appending Brock makes a
longer film in which the new material is a *smaller* fraction. Same tooling
(`tools/pewter-chain/swarm_film.py`), separate output. **Do not re-stitch them for tidiness.**

- **`pewter_chain film` takes `--from`/`--to` now, and that is what makes either cut producible
  on purpose.** It used to play every link there was, which was right while the chain ended at
  Pewter and stopped being right the moment it did not — one command that always plays
  everything can produce neither film, and the published 01-14 cut would have become
  irreproducible the day links 15-19 landed. **A slice that does not begin at link 1 needs
  somewhere to begin**, so `--from` above 1 requires `--start-dir` (agent *ag* takes pool slot
  `ag % len`, the same rule `corpus` uses) and refuses without one rather than filming agents on
  the Nintendo logo.
- **`film` writes the `agent-NN.links.csv` span file too**, so `swarm_film.py`'s per-link bars
  and its stage timeline work from a `film` run alone; `chain_tracks.py` is only needed when a
  film is stitched from separate corpus runs. **The spans carry the link's number in the whole
  chain, not its position in the slice** — a span file saying "link 1" for `15-train-for-brock`
  is one nobody can line up against a report.
- **The gym arc's own shape, from `swarm_film.py --dry-run` on a `film` run:** the training
  link is **88.5 % of the swarm's emulated time and 91.3 % of the film's runtime** (2608.0 s
  against the gym's 205.8, the two heals' 80.8 and 43.3, and the walk east's 28.1). The same
  shape link 11 has in the main chain, and the reason the two cuts do not belong together.
- **AND IT IS WHY THE GYM ARC NEEDS A HIGHER PACE THAN THE MAIN CHAIN, NOT A LOWER ONE.** The
  arc is **47.6 emulated minutes**, so `seconds_out x speed = the emulated time there is` puts
  8x at **5 m 55 s** — longer than the whole fourteen-link cut. The ceiling is
  `stride <= walk_span`, and `walk_span` measures **31** emulated frames on these tracks, so at
  60 fps anything to **~31x** stays smooth: **24-30x gives a 1½-2 minute film**, which is the
  right pace for material that is seven-eighths grinding.

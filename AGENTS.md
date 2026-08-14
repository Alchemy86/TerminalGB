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
- Known-inaccurate **on purpose (documented non-goals — see component pages)**: DMG
  OAM-corruption bug not modelled (`oam_bug` red-by-design — 3 of its 8 sub-tests pass; it
  is a DMG hardware *defect*); PPU mode 3 fixed-length (needs a cycle-stepped PPU — it
  costs us Mealybug 0/71); the CGB double-speed switch does not take effect (see the
  `interrupt_time` note below).
- **The timer, interrupt dispatch and OAM DMA are no longer non-goals** — all three are
  modelled now; see the "Core timing" section below. Two claims that used to live in this
  list are gone for good: the timer divergence did *not* have "no wired test" (Mooneye
  `acceptance/timer/*` wires it, and we were failing 10 of 13 — now 13/13), and OAM DMA
  being instantaneous was not merely a DMA inaccuracy, it made 14 Mooneye *instruction*-
  timing tests unmeasurable. `interrupt_time` still prints **Failed**, but no longer for
  the dispatch-timing reason recorded here before: it now measures the correct 13 cycles
  and fails on the double-speed switch instead.
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
- **Save-state `FORMAT_VERSION` is now 4** (timer became `div`/`prev_div`/reload latches;
  `Cpu::setdi` removed; `OamDma` added).
- **What is still red is not ours:** 11 PPU tests (mode-3 timing, STAT blocking, LCD-on)
  and 5 boot-state tests.
- **DMG mode flips two of the five boot-state tests**, not five — my earlier estimate was
  optimistic. `boot_div-dmgABCmgb` and `boot_hwio-dmgABCmgb` need the exact machine state
  the *boot ROM* leaves behind at `PC=$0100` (the system counter's value, and every I/O
  register), which is a strictly harder claim than the boot registers, and
  `serial/boot_sclk_align` needs the serial clock's phase against that same counter. Those
  three plus the 11 PPU tests are the whole remaining red list.
- **`blargg/interrupt_time` moves `$0C` → `$0D` and `$0D` is CORRECT** —
  `tests/interrupt_time/interrupt_time.s` says "Should take 13 cycles at either speed". Any
  baseline note blaming "16-vs-20 T-cycle dispatch" is stale; re-bless it. The ROM still
  says `Failed` for an unrelated reason its own output shows: rounds 3 and 4 print speed
  `00` after `cpu_fast`, so the **CGB double-speed switch is not taking effect**. That is a
  separate finding and it also explains `daid/speed_switch_timing_*`.
- **Do not read wall-clock timings from a run with the `audio` feature enabled** — a crew is
  fixing a ~62× slowdown there (17.9 ms/frame vs 0.29 ms). The ~6 % figure above was
  measured with no APU installed, so it isolates this change.

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
- **A frame-paced loop runs the machine unbounded while the LCD is off** (`Gpu::do_cycle`
  returns immediately when `!lcd_on`, so `updated` never fires). Measured on Pokémon Blue's
  boot: one "frame" ran **845,836** cycles instead of 70,224, and the APU emitted the matching
  burst — which the queue's latency bound then discards. That is a **frontend pacing** defect,
  not an audio one, and it is the only remaining source of playback discards (all inside the
  first 20 s of a cold boot; zero thereafter). Cap an iteration by cycles as well as by
  `gpu.updated` if it ever matters. Do not "fix" it in `sound.rs`.
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
  power-on resets the frame sequencer to step 0 (DIV divider keeps running); a powered-off
  APU still emits silence at full sample rate (never stall the stream). Save-state format is
  v3 (v2 added APU shadow regs + flags; v3 added `WaveChannel::since_fetch`).
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
- **Audio knobs for re-measuring (and for A/B by ear):** `GB_AUDIO_DEVICE_FRAMES` (0 = host
  default), `GB_AUDIO_START_FILL`, `GB_AUDIO_HIGH_WATER`, `GB_AUDIO_STATS=1`. The old,
  laggy configuration is `GB_AUDIO_DEVICE_FRAMES=0 GB_AUDIO_START_FILL=7000
  GB_AUDIO_HIGH_WATER=12000`.
- **Next structural step for latency:** production is still bursty at frame granularity —
  a frame emits 16.7 ms of audio in one go, and that burst dominates the cushion. Stepping
  emulation in sub-frame chunks would roughly halve the required cushion again (towards
  ~30 ms end-to-end). That changes the frontends, not the audio code.

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
  session's lifetime. Non-kitty-keyboard terminals **fail loudly** (clear error, exit 1).
  Protocol selection uses `build_picker()` (prefer the advertised protocol, e.g. Kitty over an
  env-guessed iTerm2; never panic on query timeout), then a config `protocol` may override.
  Frame pacing targets **59.7275 Hz** (`FRAME_TIME = 16_743µs`), not a rounded 60.
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
  - *MBC3 latches on any write to 0x6000-0x7FFF*, not on the 0x00→0x01 sequence. Not fixed;
    harmless for real games, recorded as a conformance check.
- **A real cartridge is a reference oracle for saves** now that our format matches: play in
  the emulator, write the `.sav`, restore it to the cart with a dumper, read it back, compare.
  No emulator-only suite can make that check.
- **Do not vendor FlashGBX's `db_DMG.json`** (GPL-3.0 compiled database, and our repo is
  MPL-2.0). Formats, header constants and hardware behaviour are facts and were adopted; no
  code was taken.

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

## Config parsing — fail loudly

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
- **The emulator never writes to `/dev/videoN`.** A V4L2 output device needs `VIDIOC_S_FMT`
  before the first write, and that cannot be tested without the kernel module (root to install)
  — so `video_out = /dev/video*` is refused with the exact `ffmpeg -f v4l2` recipe.
  `tools/virtual-camera.sh` is the supported route. This box has **no v4l2loopback** (only a
  real UVC webcam on video0/1), so the camera end has never been run here; everything up to the
  device has (YUYV 640×480 bytes decoded by ffmpeg).
- **A v4l2loopback device cannot be renamed while it exists** (upstream `v4l2loopback-ctl` has
  `add`/`delete`/`set-fps`/`set-caps`/`set-timeout-image` and no rename) — the label comes from
  `card_label` at modprobe time or `v4l2loopback-ctl add -n`. So `video_device = /dev/videoN`
  only *verifies*: sysfs label vs cartridge title, with the delete+add commands if they differ.
- **Naming:** `clean_title` (human/device label) and `file_safe_title` (filename) live in
  `video.rs` — cartridge titles are 16 padded bytes and can hold anything. A recording is named
  `TITLE-stamp-WxH-fmt.raw` so the consuming `ffmpeg` arguments can be read off the filename;
  `%t`/`%s` expand in any configured path; a directory destination gets the same treatment.
- **`v` toggles the stream live**, `c`/`C` (and `F2`) take screenshots — displayed look and raw
  framebuffer. Screenshots use the **selected** upscale factor, not `transmit_factor` (with
  `transmit_scale = auto` those differ on purpose), and go through the `image` crate, not
  `termimage::png_rgb_into` — the latter is *stored*-deflate (uncompressed), right for a 60 Hz
  wire and wrong for a file someone keeps.
- Costs measured over 3000 frames (emulation alone 0.216 ms/frame): rgba 160×144 **+0.007 ms**,
  yuyv 640×480 +0.55 ms, yuyv 640×480 with the LCD upscaler +1.58 ms — all inside 16.74 ms.
  Writes are **blocking**: a stalled consumer back-pressures the emulator, a closed pipe
  disables the sink permanently rather than erroring every frame.
- Telemetry gained a `"video"` object (`frames`/`duplicated`/`dropped`/`bytes`/`ms`) — an
  addition, so no `SCHEMA_VERSION` bump. The invariant to assert: `video.frames ≈ elapsed ×
  59.7275` under `realtime` pacing, whatever `fps` does.

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

## Live trainer view — the read-only plugin (`src/trainer/`)

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
  `src/trainer/gen1.rs` carries its symbol name. `pokered.sym` gives identical addresses, which
  is why one table serves Red and Blue. **Re-derive rather than edit by hand.**
- **Three traps that produce plausible-but-wrong data** (all handled, all worth remembering):
  Gen 1 species indices are **not** Pokédex numbers (`MonsterNames` is indexed by internal
  index, `BaseStats` by dex number, converted via `PokedexOrder` — as `GetMonHeader` does, Mew
  special-cased); every HP and stat is **big-endian**; and `party_struct` (44 bytes) and
  `battle_struct` (28) put level and stats at **different offsets**.
- **Sprites come from the ROM, not VRAM**, because VRAM only holds what is on screen and a
  party panel would be blank in the overworld. `src/trainer/sprite.rs` implements Gen 1's
  picture compression from the format documented in `pokered`. Proven **byte-identical** to
  `pokered`'s own 2bpp output for 13 species covering every "Pics" bank and both special cases.
- **The sprite is a SECOND terminal image.** It has its own Kitty image id (`0x00676202` vs the
  game's `0x00676201`), its own cell rectangle, and its cells marked `skip`. All three rules
  from `docs/terminal-render.md` apply to it. It is also drawn **after** `render_display`,
  because `render_display` clears `pending_wire` for the frame.
- **Cost discipline:** nothing is read per frame — the plugin samples every 6 emulated frames
  (~10 Hz) and **only while its panel is visible** (`set_active`). Sprites decompress once per
  species and cache. A session with no plugin attached pays one `is_empty` test per frame.
- **Graceful refusal is a feature.** The cartridge is identified by title **and header global
  checksum**, so a hack, translation or randomiser with a shifted layout is refused with an
  explanation rather than displayed wrongly. Yellow (`pret/pokeyellow`, same structures at
  different addresses) and Gen 2 (`pret/pokecrystal`, different structures) are named as
  extensions, not pretended.
- **Derived values are the point** (`src/trainer/derived.rs`): shiny-if-traded-to-Gen-2 from the
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

## Plugins & the Gen 1 virtual link partner (`src/plugin/`)

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

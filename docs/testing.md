# Accuracy testing — the yardstick we measure against

> The shared reference for every component page's **"Expected behaviours as tests"** and
> **"Test ROMs that exercise it"** sections. Read this before writing tests or changing any
> emulation code. · [← Component index](components/README.md) · [← Top-level README](../README.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`bb6bca5`](https://github.com/Alchemy86/gameboy/tree/bb6bca551af229f6aae1b67da5ce8052ac12f8fa) · regenerate with `make docs-links`, verify with `make docs-check`.

## Why this page exists

The captain's ask: *"pull up Game Boy logic and design / known quirks and tests so we can
WRITE expected KNOWN outcomes as TESTS BEFORE we look to improve or correct the running of
games."* This page names the **yardstick** — the published, deterministic test suites the
Game Boy emulator community treats as ground truth — so that every "expected behaviour" in
the component pages traces back to an executable specification, not to opinion.

## A myth to clear up first (so we adopt the *right* yardstick)

The prompt for this work recalled a YouTube video that benchmarked "essentially every Game
Boy emulator" and found that an emulator **Nintendo** shipped scored near-perfect. We
researched this carefully, and the recollection has drifted — stating it plainly matters,
because adopting the wrong yardstick would be worse than none:

- The video is **100thCoin — "How Inaccurate are Nintendo's Official Emulators?" (2025)**,
  and it tests **NES**, not Game Boy. Its benchmark is **AccuracyCoin**, a self-authored
  NES test cartridge (~125–141 on-screen `PASS`/`FAIL` tests).
  [Video](https://www.youtube.com/watch?v=oYjYmSniQyM) ·
  [AccuracyCoin repo](https://github.com/100thCoin/AccuracyCoin).
- Nintendo's official emulators did **not** score near-100%. Most passed ~4–17 of 125; the
  best was the **NES Classic Edition at 94/125 (~75%)** — good *relative to Nintendo's
  other emulators*, not perfect.
- **No Nintendo-shipped Game Boy emulator is an accuracy reference.** The Pokémon Stadium /
  Stadium 2 Transfer Pak emulator is documented as *inaccurate*
  ([Emulation General wiki](https://emulation.gametechwiki.com/index.php/Emulators_in_games)).
  The **Super Game Boy** and **Game Boy Player** *are* effectively perfect — because they
  contain a **real Game Boy CPU on a chip**. They are hardware, not emulators, so they
  can't legitimately headline an emulator benchmark.

The useful answer — the one the captain actually wants — is below: the suites the community
does use, and which real emulators (SameBoy, BGB, Gambatte, mGBA) are the references.
Community aggregation points: [gbdev GBEmulatorShootout](https://gbdev.io/GBEmulatorShootout/)
(9 suites × 17 emulators; no Nintendo emulator in it) and
[TASVideos GB accuracy tests](https://tasvideos.org/EmulatorResources/GBAccuracyTests).

## The yardstick: four suites, in adoption order

No single ROM proves accuracy; the canonical stack is **Blargg + Mooneye GB +
dmg-acid2/cgb-acid2**, with **Game Boy Doctor** as an on-ramp and **Mealybug Tearoom /
SameSuite** for the deepest PPU/APU corners. Adopt them in this order — cheapest signal and
biggest coverage first.

### 1. Blargg's test ROMs — CPU, timing, sound

The classic battery. Sub-suites: `cpu_instrs` (11 sub-tests), `instr_timing`, `mem_timing`
/ `mem_timing-2`, `halt_bug`, `oam_bug`, `dmg_sound`, `cgb_sound`, `interrupt_time`.

- **Proves:** every opcode's result and flags, instruction/memory cycle timing, the HALT
  and OAM-corruption quirks, and APU behaviour.
- **Headless detection:** each ROM prints its result to the **serial port** (write byte to
  `SB` `0xFF01`, then `0x81` to `SC` `0xFF02`), ending in the ASCII string **`Passed`** or
  **`Failed N`**. Capture serial bytes and match the string. (The same text is drawn
  on-screen too.) → depends on the [Serial](components/serial.md) capture hook.
- **This repo:** the `retrio/gb-test-roms` submodule in [`.gitmodules`](../.gitmodules) is
  now **wired into CI** — the harness runs `cpu_instrs` (11/11 pass), `instr_timing` (pass),
  `mem_timing`/`mem_timing-2` (**pass**), `halt_bug` (**pass**); `oam_bug` and
  `interrupt_time` stay red as documented non-goals. See
  [What Phase 1 actually built](#what-phase-1-actually-built--the-harness-in-this-repo).
- Sources: [gg8 Test ROMs](https://gbdev.gg8.se/wiki/articles/Test_ROMs) ·
  [c-sp collection](https://github.com/c-sp/game-boy-test-roms).

### 2. Mooneye GB acceptance suite (Gekkio) — cycle-exact timing

The fine-grained hardware-timing suite: `acceptance/` (timer, PPU mode/STAT timing,
interrupt dispatch, OAM DMA, per-model DMG/MGB/SGB/CGB behaviour) and `emulator-only/`
(MBC1/2/3/5).

- **Proves:** T-cycle-exact edge behaviour — the difference between "runs games" and
  "cycle-accurate." This is where most of our known approximations (fixed PPU mode 3,
  instant OAM DMA, separate timer counters, 16-vs-20 T-cycle interrupt dispatch) will show
  up as **red**.
- **Headless detection:** on completion the ROM executes the **`LD B,B` magic breakpoint
  (opcode `0x40`)** — trap it as "test done" (with a ~120-emulated-second hang timeout). It
  signals **pass by loading the Fibonacci sequence into registers: B=3, C=5, D=8, E=13,
  H=21, L=34** (`03 05 08 0D 15 22`). Any other register values = fail. No serial or LCD
  needed.
- **This repo:** **not** the declared submodule; a separate suite, not wired.
- Sources: [mooneye-gb](https://github.com/Gekkio/mooneye-gb) ·
  [Gekkio — test ROM dos & don'ts](https://gekkio.fi/blog/2016/game-boy-test-rom-dos-and-donts/).

### 3. dmg-acid2 / cgb-acid2 (Matt Currie) — the PPU "face test"

A single rendered frame that draws a smiley face; any PPU compositing bug visibly distorts
it.

- **Proves:** background/window/object compositing in one frame — object flip flags and
  priority, the 10-sprites-per-line limit, tile-data region selection, palettes, window
  position and the window line counter. cgb-acid2 adds CGB attributes.
- **Headless detection:** **image-based.** Render the frame and compare the framebuffer to
  the repo's **reference PNG** (exact pixel match), or eyeball the face. The READMEs ship a
  "failure examples" table mapping each broken feature to how the face breaks.
- **This repo:** **wired in** — the prebuilt ROM + `reference-dmg.png` are vendored under
  [`third_party/dmg-acid2/`](../third_party/dmg-acid2/) and the harness renders it every run
  (currently renders correctly). The cheapest, highest-value *visual* regression net for
  [PPU](components/ppu.md) work.
- Sources: [dmg-acid2](https://github.com/mattcurrie/dmg-acid2) ·
  [cgb-acid2](https://github.com/mattcurrie/cgb-acid2).

### 4. Game Boy Doctor — a CPU on-ramp before the PPU/serial work

Not a suite; a way to pass **Blargg `cpu_instrs`** *before* the PPU or serial capture is
trustworthy.

- **How:** hardcode `LY` (`0xFF44`) to return `0x90`, and after **every** opcode print one
  fixed-format line of full CPU state:
  `A:00 F:11 B:22 C:33 D:44 E:55 H:66 L:77 SP:8888 PC:9999 PCMEM:AA,BB,CC,DD`.
- **Detection:** the tool diffs your per-instruction log against its bundled reference log
  and pinpoints the **exact instruction** where CPU state first diverges — pass/fail *and*
  a precise failure location, no LCD/serial required.
- Source: [gameboy-doctor](https://github.com/robert/gameboy-doctor).

**Deep-cut suites** (adopt later): **Mealybug Tearoom** (mid-scanline PPU writes — exactly
our whole-line-render approximation) and **SameSuite** (APU + CGB corners). Reference
emulators to diff behaviour against: **SameBoy** ("over 99.9%" accuracy, passes Mooneye +
Blargg + Wilbert Pol — [features](https://sameboy.github.io/features/)), **BGB**,
**Gambatte**, **mGBA**.

## How a headless CI harness would run them

This is documentation, not code — but the shape the roadmap (Phase 1) should build:

1. **Feature-gate audio off** so a headless runner needs no sound card (the whole native
   build currently requires ALSA — see the top-level README). Mooneye/Blargg need no audio.
2. **Wire the serial capture hook.** Install a [`Serial`](components/serial.md) callback
   (`set_callback`, [`src/mmu/serial.rs:66`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/serial.rs#L66)) that appends each byte
   to a string. Today the default is a no-op, so **no Blargg result is readable** until
   this is done — it is the first prerequisite.
3. **Blargg harness:** load ROM → run frames until the serial string contains `Passed`/
   `Failed` or a frame budget expires → assert `Passed`.
4. **Mooneye harness:** run until opcode `0x40` (`LD B,B`) executes or a cycle budget
   expires → assert registers `B,C,D,E,H,L == 3,5,8,13,21,34`.
5. **acid2 harness:** run ~1 frame → hash/compare the framebuffer (`Gameboy::image()`,
   [`src/gameboy.rs:290`](https://github.com/Alchemy86/gameboy/blob/main/src/gameboy.rs#L290)) against the reference PNG.
6. **Report a pass count** (`Blargg cpu_instrs: 11/11`, `Mooneye timer: 6/13`, …). Start
   from an **allow-list of currently-passing ROMs** so CI is green from day one; a
   regression that breaks a listed ROM turns CI red.

## What Phase 1 actually built — the harness in this repo

The section above is the design sketch. What is now checked in and runs in CI:

- **Runner:** [`testharness/testroms.rs`](../testharness/testroms.rs), wired as a
  `harness = false` Cargo integration test (`[[test]] name = "testroms"` in
  [`Cargo.toml`](../Cargo.toml)) so it owns its own `main()`, prints a pass count and sets
  the process exit code. It lives in `testharness/` rather than `tests/` because `tests/`
  is the `retrio/gb-test-roms` submodule — Cargo cannot track `.rs` files inside a
  submodule path. Run it with:

  ```sh
  cargo test --release --test testroms          # verify against the baseline
  cargo test --release --test testroms -- --nocapture   # (harness=false: output always shows)
  ```

- **ROMs:** Blargg comes from the already-declared `tests/` submodule (now initialised).
  dmg-acid2 ships as source-only (needs an RGBDS build + a nested submodule), so its
  **prebuilt, MIT-licensed** ROM and reference PNG are vendored under
  [`third_party/dmg-acid2/`](../third_party/dmg-acid2/) with the upstream `LICENSE`. Mooneye
  is **not** wired yet (its ROMs are a separate suite, not in the submodule) — that is the
  obvious next increment.

- **Two result mechanisms, both implemented:**
  - *Serial* — a `Serial::set_callback` sink captures bytes; the run stops when the text
    contains `Passed`/`Failed` or a **250M-cycle budget** (~1 s wall) is exhausted (timeout
    = fail). Used for `cpu_instrs` (11), `instr_timing`, `mem_timing`.
  - *Frame hash* — run a **fixed 600 frames**, hash the RGBA framebuffer with FNV-1a, and
    compare against the baseline. Used for dmg-acid2 and for the Blargg ROMs that only draw
    their result to the LCD (`mem_timing-2`, `interrupt_time`, `halt_bug`, `oam_bug`). On any
    change the actual frame is written to `target/testrom-artifacts/<id>.png`; acid2 is
    always dumped so its rendered face can be compared to `reference-dmg.png` by eye.

- **Reproducibility is enforced, not assumed.** Emulator state is deterministic (WRAM is
  seeded with a fixed value, audio is never enabled, no wall-clock or RNG in the loop). The
  harness *proves* it: it hashes acid2 twice at startup and aborts if the two hashes differ.
  The acid2 hash `01acf78fe505f31c` was verified identical twice-in-a-run, across separate
  processes, and across a rebuild.

- **No new dependencies.** The FNV-1a hash and the PNG writer (uncompressed zlib blocks +
  CRC32) are hand-rolled in the harness — a few dozen lines each — rather than pulling in a
  hashing or image crate.

- **The audio pipeline has the same treatment:**
  [`testharness/audiocapture.rs`](../testharness/audiocapture.rs)
  (`cargo test --release --test audiocapture`) runs ROMs headlessly with a device-free
  capture `AudioPlayer`, records every sample the APU + blip_buf pipeline produces,
  writes listenable WAVs to `target/testrom-artifacts/audio/`, and checks objective
  properties: sample count == cycles×rate/4194304, RMS/peak, silence runs,
  discontinuities, dominant frequency (a built-in 440 Hz tone ROM must measure 440 Hz),
  plus Blargg `dmg_sound`'s memory-mapped `$A000` verdict. Captures are byte-identical
  across runs and profiles; the FNV hash of the PCM stream is pinned in
  [`testharness/audio_baseline.txt`](../testharness/audio_baseline.txt)
  (re-bless with `UPDATE_AUDIO_BASELINE=1`). A third mechanism, alongside serial and
  frame-hash: **sample-stream hash**. Full story in [docs/audio.md](audio.md).

### The known-failures baseline — how to keep CI honest

Most ROMs fail today, on purpose (six documented divergences below). A CI job that is red
from day one gets ignored, so the harness compares every result against a checked-in
expected-results file, [`testharness/baseline.txt`](../testharness/baseline.txt):

- Serial ROMs record `PASS` / `FAIL`; frame-hash ROMs record `HASH:<fnv1a64>`.
- CI is **green** when every result matches the baseline, and **red only when a result
  changes** — either a **regression** (was `PASS`, now `FAIL`; or a frame hash moved) or an
  **improvement** (was `FAIL`, now `PASS`). Both are surfaced loudly; an improvement prints
  a `🎉` and is *not* silently swallowed — the baseline is meant to be updated deliberately:

  ```sh
  UPDATE_BASELINE=1 cargo test --release --test testroms
  ```

  Review the diff to `baseline.txt`, confirm the change is real, and commit it. That single
  file is the mechanism that keeps the suite honest as the emulator improves.

### Results today (the honest baseline)

> **See [`testharness/README.md`](../testharness/README.md) for the full per-ROM
> compatibility & progress scoreboard** (every ROM, its status, and the documented
> non-goals). The summary below is the same data at suite granularity.

| Suite | Result | Notes |
|---|---|---|
| Blargg `cpu_instrs` (01–11) | **11/11 PASS** | every opcode group |
| Blargg `instr_timing` | **PASS** | |
| Blargg `mem_timing` | **PASS** | timer stepped per memory access (was FAIL) |
| Blargg `mem_timing-2` | **PASS** (screen) | draws `01:ok 02:ok 03:ok Passed` (was FAIL) |
| Blargg `interrupt_time` | incomplete (screen) — **documented non-goal** | hangs headless: `REQUIRE_CGB`, polls `NR52` with no APU. Would test 16-vs-20 T-cycle dispatch if it could run |
| Blargg `halt_bug` | **PASS** (screen) | renders `Passed` — HALT bug now modelled (was FAIL) |
| Blargg `oam_bug` | FAIL (screen) — **documented non-goal** | draws "Failed #2" — DMG OAM corruption deliberately not modelled |
| dmg-acid2 | **renders correctly** | **zero** differing pixels vs `reference-dmg.png` (4-level pixel diff) |

**Blargg serial suite: 13/13 passing** (was 12/13 — `mem_timing` fixed). The remaining
frame-hash reds are now **deliberate, documented non-goals**, not outstanding bugs:
`interrupt_time` cannot run headless (no APU → `sync_apu`/`get_cpu_speed` spin on `NR52`),
and its 16-vs-20 T-cycle interrupt-dispatch gap has no other ROM in the wired suite to
prove a fix against; `oam_bug` is a DMG-only hardware defect the CGB-booting emulator has
~no reason to reproduce. Both are pinned in `baseline.txt` so they can't silently change.
The mode-3-length and OAM-DMA-bus-restriction approximations need a cycle-stepped PPU/CPU
(a structural commitment the project has not made). The whole suite runs in **~1.8 s**.

acid2 rendering correctly is a genuinely good result — verified by a pixel diff, not just
by eye — the PPU composites background, window and objects correctly enough to pass the
face test exactly, despite the fixed mode-3 / whole-line-render approximations.

## Mapping suites to components

| Component | Primary suites | The approximations they will catch |
|---|---|---|
| [CPU](components/cpu.md) | Blargg `cpu_instrs`, `instr_timing`, `mem_timing` (**pass**), `halt_bug` (**pass**), `interrupt_time` (non-goal); Mooneye `acceptance/interrupts`, `instr`; Game Boy Doctor | 16-vs-20 T-cycle interrupt dispatch (no runnable test) |
| [PPU](components/ppu.md) | dmg-acid2 (**pass, pixel-verified**), cgb-acid2; Mooneye `acceptance/ppu`; Mealybug Tearoom | Fixed mode-3 length, whole-line render, STAT IRQ blocking (structural non-goals) |
| [MMU](components/mmu.md) | Blargg `mem_timing` (**pass**), `oam_bug` (non-goal); Mooneye `acceptance/oam_dma`, `bits` | Instant/unguarded OAM DMA, no VRAM/OAM mode-blocking, OAM bug (structural / non-goals) |
| [MBC](components/mbc.md) | Mooneye `emulator-only/mbc1/2/5`; Blargg `bits-bank1` | Banking bit maths; the MBC3-for-everything web/FFI bug |
| [APU](components/apu.md) | Blargg `dmg_sound`, `cgb_sound`; SameSuite | Length-counter extra-clock edge case |
| [Timer](components/timer.md) | Mooneye `acceptance/timer`; Blargg `interrupt_time` | DIV/TAC glitches, TIMA reload delay window |
| [Serial](components/serial.md) | Blargg (as its reporting channel); Mooneye `acceptance/serial` | Instantaneous transfer; the capture hook must be wired first |
| [Joypad](components/input.md) | Mooneye joypad (limited) | Mostly proven by games booting |

## Sources

- gbdev GBEmulatorShootout — https://gbdev.io/GBEmulatorShootout/ · https://github.com/gbdev/GBEmulatorShootout
- TASVideos GB accuracy tests — https://tasvideos.org/EmulatorResources/GBAccuracyTests
- Blargg / Test ROMs — https://gbdev.gg8.se/wiki/articles/Test_ROMs · https://github.com/c-sp/game-boy-test-roms
- Mooneye GB — https://github.com/Gekkio/mooneye-gb · https://gekkio.fi/blog/2016/game-boy-test-rom-dos-and-donts/
- dmg-acid2 / cgb-acid2 — https://github.com/mattcurrie/dmg-acid2 · https://github.com/mattcurrie/cgb-acid2
- Game Boy Doctor — https://github.com/robert/gameboy-doctor
- SameBoy accuracy — https://sameboy.github.io/features/
- Pan Docs (hardware behaviour) — https://gbdev.io/pandocs/
- The NES video (source of the misremembered "Nintendo won") — https://www.youtube.com/watch?v=oYjYmSniQyM · https://github.com/100thCoin/AccuracyCoin
- Pokémon Stadium emulator (documented inaccurate) — https://emulation.gametechwiki.com/index.php/Emulators_in_games

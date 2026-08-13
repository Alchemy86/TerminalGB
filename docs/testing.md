# Accuracy testing — the yardstick we measure against

> The shared reference for every component page's **"Expected behaviours as tests"** and
> **"Test ROMs that exercise it"** sections. Read this before writing tests or changing any
> emulation code. · [← Component index](components/README.md) · [← Top-level README](../README.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`58310e0`](https://github.com/Alchemy86/gameboy/tree/58310e02f64cd4674a97ac6424f2fdde3226f1c4) · regenerate with `make docs-links`, verify with `make docs-check`.

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
  plus Blargg `dmg_sound`'s memory-mapped `$A000` verdict (**12/12 pass** — the
  expected verdict is pinned per fixture, so a pass→fail regression is reported by
  name, never as an opaque hash change). Captures are byte-identical
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

## A suite no public test ROM covers: battery-save durability

Blargg, Mooneye GB and the acid2 ROMs all run inside a single process and never look at
what reaches the filesystem. **Persistence is emulator behaviour, not hardware behaviour**,
so no public suite catches the single worst bug a Game Boy emulator can have: the player
saves inside the game, quits, relaunches, and is offered only "NEW GAME". That gap is
itself a finding worth carrying into a standalone conformance tool.

[`testharness/battery.rs`](../testharness/battery.rs) (`cargo test --release --test battery`) fills it, using a
**hand-assembled MBC3+RAM+BATTERY test ROM built in the harness itself** — no commercial
ROM, no vendored binary. The ROM enables cartridge RAM, stamps a distinct marker into the
first and last byte of each of the four 8 KiB banks, then spins. The harness then checks:

| Case | Player-visible symptom if it fails |
|---|---|
| The save file appears **while the emulator runs**, with no `Drop` (`std::mem::forget`) | Closing the window, a crash, or a `kill` loses everything you saved |
| It is a raw `.sav`, exactly `ramsize` bytes, no header | Your save cannot be moved to or from any other emulator |
| Every bank reads back through the bank register on a fresh machine | The file looks the right size but the game's own checksum rejects it — "NEW GAME" only |
| A session that writes no cartridge RAM leaves the save file untouched | One launch that failed to load erases a real save on its next clean exit |
| A legacy `.gbsave` loads, migrates to `.sav`, and stays on disk as a backup | Upgrading the emulator loses your old saves |
| A foreign `.sav` loads (including a trailing 48-byte VBA-M/BGB/FlashGBX RTC footer) | Saves from other emulators are rejected |
| A plain `.sav` loads at offset 0, with nothing prepended | Every save from another emulator or a real cartridge arrives 8 bytes shifted — and everything we write is shifted for everyone else |
| Malformed input does not crash: RAM-less MBC5+RUMBLE bank write, unofficial ROM-size code, short `.sav`, truncated ROM, no-mapper `0x09` battery cart | The emulator aborts (`panic = 'abort'`) the moment you load an unusual cart or a save from elsewhere — and the abort also loses the save |

Each failure prints *why* it matters and what hardware behaviour is being violated (a
battery-backed cart holds SRAM from the instant the game writes it — Pan Docs,
[MBC3](https://gbdev.io/pandocs/MBC3.html)), not just a red line. The harness was validated
by disabling the per-frame autosave hook (2/5 at the time, with those messages) and by
reintroducing the MBC5 divide-by-zero (the harness reproduces the panic).

The **save-file format itself is settled, not invented here**: `.sav` is raw cartridge RAM
with no header, and an MBC3 clock is a 48-byte little-endian footer appended to the same
file (five registers as `u32`, a latched copy, then a `u64` UNIX timestamp). That is what
FlashGBX writes when it dumps a real cartridge, and what VBA-M and BGB read — so a real
cartridge is usable as a **reference oracle**: play in the emulator, write the `.sav`,
restore it to the cartridge with a dumper, read it back and compare. No emulator-only suite
can make that check. The exact layout is pinned by a unit test in
[`src/mbc/battery.rs`](../../src/mbc/battery.rs) so a refactor cannot quietly change what we
hand to other tools.

For an end-to-end check against a real cartridge there is
[`examples/rom_script.rs`](../examples/rom_script.rs): a headless scripted-input driver
(`idle` / `tap` / `mash` / `shot`) that can play a commercial game to an in-game save, dump
PNG frames, and — with `EXIT=kill` — abort the process without running a single destructor.
That is how this behaviour was proven in Pokémon Blue: save, kill, relaunch, **CONTINUE**.
The script that gets Pokémon Blue from a cold boot to an in-game save (roughly 5 800
frames, ~1.5 s headless) is:

```text
idle 300
mash a 20 6 12     # title screen -> NEW GAME
tap a 1
idle 120
mash a 80 6 20     # Oak's intro; mashing A also accepts the default names
mash a 60 6 20
idle 120
tap b 3
idle 60
tap b 4            # close the "playing the SNES" box — A would just reopen it
idle 60
mash down 10 14 10 # step off the TV tile
idle 30
tap start 1        # start menu
idle 60
mash down 3 8 14   # POKéMON / ITEM / <name> / SAVE
idle 30
tap a 1
idle 90
tap a 1            # "Would you like to SAVE the game?" -> YES
idle 90
tap a 1
idle 240           # "Now saving..."
```

Run it once with `EXIT=kill` (the emulator dies mid-game, exactly like a closed window),
then again with `PHASE=relaunch` and a script that just boots and presses A — the title
menu goes from `NEW GAME / OPTION` to `CONTINUE / NEW GAME / OPTION`.

## Mapping suites to components

| Component | Primary suites | The approximations they will catch |
|---|---|---|
| [CPU](components/cpu.md) | Blargg `cpu_instrs`, `instr_timing`, `mem_timing` (**pass**), `halt_bug` (**pass**), `interrupt_time` (non-goal); Mooneye `acceptance/interrupts`, `instr`; Game Boy Doctor | 16-vs-20 T-cycle interrupt dispatch (no runnable test) |
| [PPU](components/ppu.md) | dmg-acid2 (**pass, pixel-verified**), cgb-acid2; Mooneye `acceptance/ppu`; Mealybug Tearoom | Fixed mode-3 length, whole-line render, STAT IRQ blocking (structural non-goals) |
| [MMU](components/mmu.md) | Blargg `mem_timing` (**pass**), `oam_bug` (non-goal); Mooneye `acceptance/oam_dma`, `bits` | Instant/unguarded OAM DMA, no VRAM/OAM mode-blocking, OAM bug (structural / non-goals) |
| [MBC](components/mbc.md) | Mooneye `emulator-only/mbc1/2/5`; Blargg `bits-bank1`; **`testharness/battery.rs`** (**pass**, battery-save durability + `.sav` interop) | Banking bit maths; the MBC3-for-everything web/FFI bug |
| [APU](components/apu.md) | Blargg `dmg_sound` (**9/12 pass**), `cgb_sound` (unwired); SameSuite | Wave-RAM access-window timing (`dmg_sound` 09/10/12) — needs a T-cycle-stepped APU, **planned next** |
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

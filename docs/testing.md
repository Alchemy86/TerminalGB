# Accuracy testing — the yardstick we measure against

> The shared reference for every component page's **"Expected behaviours as tests"** and
> **"Test ROMs that exercise it"** sections. Read this before writing tests or changing any
> emulation code. · [← Component index](components/README.md) · [← Top-level README](../README.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`72a5454`](https://github.com/Alchemy86/TerminalGB/tree/72a545438d2a2422f1b55c3452032d76259993be) · regenerate with `make docs-links`, verify with `make docs-check`.

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
  `mem_timing`/`mem_timing-2` (**pass**), `halt_bug` (**pass**), `oam_bug` (**pass on
  `@dmg`; expected to fail on `@cgb`, which has no such defect**). See
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
- **This repo:** not the declared submodule — a separate suite, **fetched at run time and
  gated in CI** since 2026-08-14 (`make test-external`; see
  [Gating the external suites](#gating-the-external-suites--mooneye-gb-samesuite-mealybug-tearoom-cpp-and-the-thirteen-in-c-sp)).
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
- **Why a picture rather than a number**, and a gallery of how the face deforms under each
  individual PPU bug — measured here by breaking them one at a time, not copied from upstream:
  [`docs/emulation/test-roms.md`](emulation/test-roms.md).
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

**Deep-cut suites** (both now fetched and gated, see
[Gating the external suites](#gating-the-external-suites--mooneye-gb-samesuite-mealybug-tearoom-cpp-and-the-thirteen-in-c-sp)):
**Mealybug Tearoom** (mid-scanline PPU writes — exactly our whole-line-render
approximation) and **SameSuite** (APU + CGB corners). Reference
emulators to diff behaviour against: **SameBoy** ("over 99.9%" accuracy, passes Mooneye +
Blargg + Wilbert Pol — [features](https://sameboy.github.io/features/)), **BGB**,
**Gambatte**, **mGBA**.

## How a headless CI harness would run them

This is documentation, not code — but the shape the roadmap (Phase 1) should build:

1. **Feature-gate audio off** so a headless runner needs no sound card (the whole native
   build currently requires ALSA — see the top-level README). Mooneye/Blargg need no audio.
2. **Wire the serial capture hook.** Install a [`Serial`](components/serial.md) callback
   (`set_callback`, [`src/mmu/serial.rs:66`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/serial.rs#L66)) that appends each byte
   to a string. Today the default is a no-op, so **no Blargg result is readable** until
   this is done — it is the first prerequisite.
3. **Blargg harness:** load ROM → run frames until the serial string contains `Passed`/
   `Failed` or a frame budget expires → assert `Passed`.
4. **Mooneye harness:** run until opcode `0x40` (`LD B,B`) executes or a cycle budget
   expires → assert registers `B,C,D,E,H,L == 3,5,8,13,21,34`.
5. **acid2 harness:** run ~1 frame → hash/compare the framebuffer (`Gameboy::image()`,
   [`src/gameboy.rs:290`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gameboy.rs#L290)) against the reference PNG.
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

- **Nothing is judged against our own previous output.** This was the harness's central
  defect and it is worth stating as a rule: four ROMs used to be pinned as an FNV hash of
  *our* framebuffer, and the documented response to a changed hash is to re-bless it — so CI
  would have stayed green straight through a regression in any of them, and `oam_bug` (really
  3 of 8 sub-tests) was an opaque number nobody could read. Every expectation now comes from
  the ROM or from its author.

- **Three result mechanisms, all implemented:**
  - *Serial* — a `Serial::set_callback` sink captures bytes; the run stops when the text
    contains `Passed`/`Failed` or a **250M-cycle budget** (~1 s wall) is exhausted (timeout
    = fail). Used for `cpu_instrs` (11), `instr_timing`, `mem_timing`.
  - *Verdict* — the Blargg ROMs that never touch the link port still **print** their result,
    and the harness reads it. First choice is Blargg's memory-mapped result block: status
    byte at `$A000` behind the `$DE $B0 $61` signature at `$A001..$A003`, zero-terminated
    text from `$A004`. `halt_bug` and `interrupt_time` are cartridge type `0x02` with RAM
    size `0x00` — **no cartridge RAM at all** — so for those the **screen** is read instead:
    Blargg's print routine loads its font so that a tile id *is* the ASCII code of the glyph
    it draws, and writes characters straight into the BG tilemap, so the tilemap reads back
    as text. Used for `mem_timing-2`, `interrupt_time`, `halt_bug`, `oam_bug`.
  - *Image* — dmg-acid2 is run a **fixed 600 frames** and compared pixel by pixel against
    `third_party/dmg-acid2/reference-dmg.png`, the reference shipped by the ROM's author.
    The comparison is by **luminance rank, not RGB** (`testharness/pngdec.rs`, the same rule
    as `tools/conformance/image.py`): our shades are `000000/606060/c0c0c0/ffffff` and the
    reference's are `000000/555555/aaaaaa/ffffff`, so a literal compare would fail every run
    for a reason the ROM is not asserting. Result: **0 of 23,040 pixels differ**.
    The rendered frame is always dumped to `target/testrom-artifacts/<id>__<model>.png`.

- **Every machine has an APU.** The harness used to leave `MMU.sound` at `None`, which is not
  a real Game Boy and silently changed what ROMs reported: `interrupt_time` spins on `NR52`,
  so with no APU it stalled on its title screen and was written off as "cannot run headless"
  — with one it runs to completion and prints `Failed`, which is a result worth having. (The
  audio harness had always installed one, which is why the two harnesses disagreed.)

  **What that costs, measured:** `+0.02 ms per emulated frame` (0.245 → 0.265 ms on
  dmg-acid2, +0.004 ms on `cpu_instrs/09`) against a 16.743 ms frame budget — 1.1x, and the
  whole 35-row suite runs in **2.9 s**. Worth stating explicitly because a separate
  investigation found that with the **`audio` cargo feature** enabled a frame costs ~17.9 ms,
  which is a ~62x slowdown and why the terminal frontend shows ~5 fps. **That is a different
  code path from this one** and the harness is not affected by it: `cargo test` builds
  without the `audio` feature, so `cpal` and `sample_queue` are not compiled at all, the
  harness' `SilentPlayer::play` is a no-op, and nothing can reach the producer-side
  high-watermark throttle that paces the emulator against the audio clock
  (`sample_queue`, `#[cfg(feature = "audio")]` — see [`docs/audio.md`](audio.md) §4). If that
  changes — if the suite's wall-clock time jumps — the APU install here is the first thing to
  re-measure, not to remove: the verdicts depend on it.

- **Every ROM runs on both consoles.** Baseline keys are `<rom>@dmg` and `<rom>@cgb`; a
  cartridge whose header says it will not run on a DMG (`0x0143 == 0xC0`, e.g.
  `interrupt_time`) has no `@dmg` row. This also settles which *binary* we are testing: the
  `retrio/gb-test-roms` submodule ships `cpu_instrs`, `instr_timing`, `halt_bug` and
  `oam_bug` with the CGB flag patched to `0x80` (the shootout uses the unpatched DMG
  originals), so detection alone would run all four as Color games. Forcing the model covers
  both machines without mutating or vendoring a ROM.

- **Reproducibility is enforced, not assumed.** Emulator state is deterministic (WRAM is
  seeded with a fixed value, no wall-clock or RNG in the loop). The harness *proves* it at
  startup by rendering acid2 twice and requiring the two framebuffers to be byte-identical.

- **The harness proves it can fail, every run.** Before any ROM is executed it decodes the
  reference image, requires it to be 160×144 with more than one shade, requires it to compare
  equal to itself, and requires a **single flipped pixel** to be detected. Both mechanisms
  were also validated by deliberately breaking the emulator: clearing the HALT-bug latch
  turns `halt_bug` red; ignoring sprite X-flip turns dmg-acid2 red at 119/23,040 pixels.

- **No new dependencies.** The FNV-1a hash, the PNG writer (uncompressed zlib blocks +
  CRC32) and the PNG **reader** — a full RFC 1951 inflater, since the upstream reference is
  2-bit greyscale — are hand-rolled in the harness rather than pulling in a crate.

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

- Every row is `<rom>@<model> = PASS|FAIL | <the ROM's own report>`. Keeping the report in
  the file is deliberate: the diff is human-readable, and a change in a single sub-test, a
  register-dump row or a checksum line is caught even when the overall verdict does not move.
- CI is **green** when every result matches the baseline, and **red on any change** — a
  regression *or* an improvement. Both are surfaced loudly; the baseline is meant to be
  updated deliberately:

  ```sh
  UPDATE_BASELINE=1 cargo test --release --test testroms
  ```

  Review the diff to `baseline.txt`, confirm the change is real, and commit it. That single
  file is the mechanism that keeps the suite honest as the emulator improves.

- **Before blessing a baseline move, rule out the console.** Both this baseline and
  [`testharness/audio_baseline.txt`](../testharness/audio_baseline.txt) depend on which
  machine a cartridge boots, and that is now chosen from the cartridge header. `$GB_MODEL`
  forces the old behaviour, so the A/B is one command:

  ```sh
  GB_MODEL=cgb cargo test --release --test audiocapture   # the pre-DMG-mode console
  ```

  When the DMG mode landed, both `dmg_sound` hashes moved and their sample counts dropped by
  exactly 256 — and `GB_MODEL=cgb` reproduced the old hashes **byte-for-byte**, which proves
  the move was the console and not a synthesis regression. Do that before you re-bless
  anything; a hash that changed for a reason you can name is a different thing from a hash
  that changed.

### Results today (the honest baseline)

> **See [`testharness/README.md`](../testharness/README.md) for the full per-ROM
> compatibility & progress scoreboard** (every ROM, its status, and the documented
> non-goals). The summary below is the same data at suite granularity.

Rows are ROM x console, so 30 ROMs give 47 rows (`interrupt_time` and the twelve
`cgb_sound` singles are CGB-only cartridges — `0x0143 == 0xC0` — so they have no `@dmg` row).

| Suite | DMG | CGB | Notes |
|---|---|---|---|
| Blargg `cpu_instrs` (01–11) | **11/11 PASS** | **11/11 PASS** | every opcode group |
| Blargg `instr_timing` | **PASS** | **PASS** | |
| Blargg `mem_timing` | **PASS** | **PASS** | timer stepped per memory access |
| Blargg `mem_timing-2` | **PASS** | **PASS** | prints `01:ok 02:ok 03:ok Passed` |
| Blargg `halt_bug` | **PASS** | **PASS** | prints `Passed` — HALT bug modelled |
| Blargg `interrupt_time` | — (CGB-only ROM) | **PASS** | prints `… 08 0D 01 … Passed` — 13 cycles at either speed, which is the ROM's own stated answer |
| Blargg `oam_bug` | **PASS** | FAIL — **expected** | `@dmg` prints `01:ok … 08:ok Passed`, i.e. **8 of 8 sub-tests**. `@cgb` is expected to fail: the OAM-corruption bug is DMG/MGB/SGB silicon only, so a real CGB fails this ROM too |
| Blargg `cgb_sound` (01–12) | — (CGB-only ROMs) | **12/12 PASS** | the Color half of the APU suite; the DMG half is scored by `audiocapture` |
| dmg-acid2 | **PASS** | **PASS** | **0 of 23,040** pixels differ from `reference-dmg.png` |

**46 of 47 rows pass**, measured. The one remaining red — `oam_bug@cgb` — is *supposed*
to be red: the OAM-corruption bug is a defect of DMG/MGB/SGB silicon that Game Boy Color
hardware does not have, even running a monochrome cartridge, so a real CGB fails this ROM
too. Turning that row green would mean emulating a machine nobody has. Every row here is
measured rather than pinned as a hash, so `oam_bug` reports which of its eight sub-tests
pass rather than an opaque number. The mode-3-length approximation applies to the **default
`standard` [picture mode](picture-modes.md)**; the cycle-stepped one is the selectable
`identical` mode and is gated (`GB_PPU=identical`, and the `external-suites-accurate` job
below) — it is simply not the default yet, because it costs about 6.8x the CPU, not because it
is behind on anything. See
[`docs/measured/ppu.md`](measured/ppu.md). The whole suite runs
in **~3.6 s** (the twelve `cgb_sound` rows cost about 0.8 s of that).

acid2 rendering correctly is a genuinely good result — the PPU composites background, window
and objects correctly enough to match the reference exactly, despite the fixed mode-3 /
whole-line-render approximations. It is now **asserted in CI**; before, the reference PNG was
checked in and never actually compared.

### These numbers are 30 ROMs, not a suite total

"We pass Blargg" is true and is a narrow slice. The wider community suites are scored — and,
since 2026-08-14, **gated** — separately; see the next section for the measured figures.

## Gating the external suites — Mooneye GB, SameSuite, Mealybug Tearoom, `cpp`, and the thirteen in c-sp

Everything above is the *in-repo* harness. The community suites are 270 further
rows — plus **6,045 more** from the thirteen suites in the
[c-sp aggregation](#the-c-sp-aggregation--thirteen-more-suites) below —
and until this landed **nothing in CI looked at any of them**: every point they measure was
protected only by somebody remembering to re-run a scout by hand. A night that gained +96
rows could have lost them again silently, and a cycle-stepped-PPU rewrite would have had no
way to show either its progress or its regressions.

```text
make test-external           # fetches the suites if needed, then gates all five groups
make test-external-accurate  # the two PPU suites again, in the `identical` picture mode
make fetch-test-suites       # just the download (~20 s, cached in target/test-suites)
```

**Both [picture modes](picture-modes.md) are scored.** `test-external` runs whichever mode is
the default (`standard`, `RenderMode::Fast`); `test-external-accurate` re-scores Mooneye and
Mealybug in the per-dot `identical` mode (`GB_PPU=identical`, or the older spelling
`GB_PPU=accurate`, which the Makefile still uses) against `mooneye_accurate_baseline.txt` and
`mealybug_accurate_baseline.txt`. Without the second pass the mode whose entire purpose is
these two suites would be gated by nobody. SameSuite is not re-scored — it is APU and DMA and
does not distinguish the modes. Every baseline file **opens with the mode that produced it
and the target that re-blesses it**: the same ROMs score differently on the two, so the obvious
command would otherwise rewrite a file with the other machine's numbers. Where `identical`
stands is [`mealybug.md`](mealybug.md) §7.

**The ROMs are fetched, never vendored.** They are other people's work under other people's
licences — the same reason [`conformance.yml`](../.github/workflows/conformance.yml)
downloads Blargg's ROMs instead of committing them. Every version is pinned in
[`tools/fetch-test-suites.sh`](../tools/fetch-test-suites.sh) and nowhere else, so CI, the
Makefile and a laptop all measure the same bytes: Mooneye from Gekkio's own prebuilt archive
(checksummed), Mealybug from the author's repository at a pinned commit (his prebuilt zip and
his hardware-photographed reference PNGs), SameSuite from the copies vendored by the GB
Emulator Shootout at a pinned commit — LIJI32 ships source only and there is no upstream
binary release, and that is also the exact corpus our published 264-row score is measured on.
That same shootout checkout brings the **`cpp`** group — CasualPokePlayer's four ROMs (three
MBC3/RTC tests and `sgb-ext-test`) with his reference screenshots — because those are four
published shootout rows that nothing here was gating.

**What is checked in is the expectations**, one file per group, under the same rules as
[`testharness/baseline.txt`](../testharness/baseline.txt): every value is the ROM's *own*
judgement, never a hash of our output; **any** change is red, in either direction (a test
that starts passing is a baseline change too); re-blessing is deliberate
(`UPDATE_EXTERNAL_BASELINE=1 make test-external`) and the diff is what the reviewer reads.

| group | file | rows | measured 2026-08-14 |
|---|---|---:|---|
| Mooneye `acceptance` + `emulator-only` | `testharness/mooneye_baseline.txt` | 103 | **95 pass** — acceptance **67/75**, `emulator-only` **28/28** |
| Mooneye `misc` (`GB_MODEL=cgb`) | `testharness/mooneye_cgb_baseline.txt` | 8 | **8 pass** |
| SameSuite | `testharness/samesuite_baseline.txt` | 76 | **72 pass** |
| Mealybug Tearoom | `testharness/mealybug_baseline.txt` | 79 | **3 pixel-exact** (DMG 1/24, CGB-C 1/31, CGB-D 1/24) |
| shootout `cpp` | `testharness/cpp_baseline.txt` | 4 | **4 pixel-exact** |

Notes worth having before you read a red build:

* **Every ROM runs on the console it names, and this is not optional.** Mooneye names each
  ROM after the machines it is written for and each one *fails on the others*, so
  `mooneye_scout::model_for` boots the machine the filename asks for — spelled out
  (`-dmg0`, `-mgb`, `-sgb`, `-sgb2`, `-cgb0`) or as one of Gekkio's compatibility letters
  (`-G` DMG, `-M` MGB, `-S` SGB, `-C` CGB, `-A` AGB). A suffix naming **several** machines
  (`-GS`, `-cgbABCDE`) is left on whatever `GB_MODEL` asked for; forcing one of those would
  boot the wrong console for the rest. A row that is red because the runner picked the wrong
  machine is a runner bug wearing an emulator bug's clothes.
  The two SameSuite `sgb/` tests set the cartridge SGB flag (`$0146 == 0x03`) and get an SGB
  too, which is what the shootout's own manifest gives them; `Model::Auto` never picks one
  by itself, and never picks a revision (see AGENTS.md, "Console revisions").
* **Mooneye's 66-row denominator excludes nine ROMs, and we no longer need it to.** Those
  nine are the named-machine variants; all nine pass now, so quote **67/75** rather than the
  community figure — the smaller denominator exists because most emulators model one or two
  consoles, not because the ROMs are unfair.
* **The Mealybug baseline pins the pixel count, not just PASS/FAIL.** At 3/79 the count is
  the only number that can show a PPU rewrite making progress instead of a flat wall of red —
  so an intermediate PPU change *will* turn that file red. Read the diff, then re-bless with
  the new counts in the same commit as the change that caused them.
* **Wiring the suite immediately found a real bug.** Three Mooneye tests that *pass*
  (`add_sp_e_timing`, `ld_hl_sp_e_timing`, `oam_dma_start`) execute code from OAM on
  purpose, which tripped the runaway-PC heuristic in `Cpu::operation` — and that heuristic
  poisons the battery, so a game legitimately running a routine out of OAM silently stopped
  saving for the rest of the session. The heuristic now covers `$FEA0-$FF7F` only; see
  [`docs/conformance-notes.md`](conformance-notes.md).

### The c-sp aggregation — thirteen more suites

[c-sp/game-boy-test-roms](https://github.com/c-sp/game-boy-test-roms) bundles eighteen suites
into one pinned release. Five were already gated above; the other thirteen — AGE, Bully,
cgb-acid2, cgb-acid-hell, Gambatte, GBMicrotest, MBC3 Tester, the wilbertpol Mooneye fork,
(parts of) little-things-gb, rtc3test, Scribbltests, Strikethrough and TurtleTests — are
4,510 ROMs and **6,045 rows**, all of them scored by one runner,
[`examples/csp_suite.rs`](../examples/csp_suite.rs), against
`testharness/csp_<suite>_baseline.txt` (and `csp_<suite>_accurate_baseline.txt` for the
`identical` picture mode). The scores and what each red group means are on
[`docs/measured/scoreboard.md`](measured/scoreboard.md#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time);
this section is the mechanics.

**The release is taken WHOLE, as one pin, and that is a deliberate departure** from the
per-suite pins above. Three reasons: one immutable artefact with one checksum; it is the corpus
other emulators' published c-sp scores are measured on; and — the one that decided it — **every
suite in it ships the author's own `game-boy-test-roms-howto.md`** stating that author's exit
condition and pass criterion. Those documents are the specification the runner implements, and
thirteen separate upstreams would leave it implementing thirteen conventions from memory. The
five suites already pinned are *not* re-pointed at this release: their baselines were measured
against those exact builds.

**Five conventions, none of them ours**, one per group of suites:

| Convention | Suites | What the ROM does |
|---|---|---|
| `LD B,B` + Fibonacci registers | AGE | stops on `$40` with `B=3 C=5 D=8 E=13 H=21 L=34` on success |
| `$ED` + Fibonacci registers | Mooneye (wilbertpol) | the same idea, with the undefined opcode the 2016 suite used |
| a verdict byte at `$FF82` | GBMicrotest | `$01` pass / `$FF` fail; its README says to read **only** that byte, because several tests leave `$FF80 == $FF81` on failure |
| a hex digit in the corner | Gambatte | the file name carries the value; Gambatte's own `testrunner.cpp` compares the first 8x8 tiles against a built-in font, transcribed bit for bit into `OUT_TILES` |
| the author's reference screenshot | everything else | the same rule Mealybug and `cpp` already follow |

**Colour is the part that is easy to get wrong**, so it is decided from evidence rather than
assumed, and the reference is always converted *towards* us so our framebuffer is never
re-ranked. A frame we drew in `GbMode::Color` is real colour; anything else is four grey
shades, and then the reference is either the DMG palette (`#000000 #555555 #AAAAAA #FFFFFF`) or
the Color boot ROM's eight compatibility colours — told apart from the reference's own palette,
which is a fact about the picture. **A shade-rank compare cannot do the compatibility case**:
eight distinct luminances in the reference against four in our frame re-ranks every pixel. A
true colour reference is taken back to 15-bit RGB (`(X << 3) | (X >> 2)` is exactly invertible)
and pushed through `Gpu::setrgb`'s correction; Gambatte's own screenshots are already in our
space, because it uses the same correction, and are compared under its `& 0xF8F8F8` mask.

**A `SKIP` row counts in neither column and says why.** 96 of the 6,045 are skipped: 95
Gambatte ROMs that upstream itself disables (an `_xout0` name makes its own `find("_out")` miss
it) or that are state dumpers with no expectation, and `tellinglys`, which only reaches a
verdict after a human has pressed every button in an arbitrary order. Counting those as
failures would invent a denominator nobody measures; dropping them would hide them.

**Two runner traps worth knowing before adding a suite here**, both of which produced wrong
verdicts before they were fixed:

* **A reference belongs to the ROM with the longest matching stem.** Several suites put
  `foo.gb` and `foo-nocgb.gb` in one directory, and a shortest-match rule hands
  `foo-nocgb-ncmBCE.png` to `foo.gb` — a wrong reference scored as a wrong emulator.
* **Gambatte's own runner opens exactly three reference names** (`_dmg08_cgb04c.png`,
  `_cgb04c.png`, `_dmg08.png`). The suite also ships bare `<stem>.png` and `<stem>_gba.png`
  files it never looks at; picking those up scores pictures upstream does not treat as
  expectations, and for the pair that differ only by suffix produces two rows claiming the
  same console.

**The Gambatte baseline is 5,320 rows, about 400 KB, twice over.** That is the same trade the
Mealybug baselines make with their pixel counts: at that size a PASS/FAIL total is a flat wall,
and what a PPU campaign needs is to see *which* rows moved.

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

## A second gap no test ROM covers: does the frontend start at all?

Every harness above builds a `Gameboy` directly and drives it. **None of them enters
`screen::tui::run`**, so the whole entry path — the capability handshakes, what the frontend
does when a terminal says "no", how a keypress becomes a button — was untested. That is not a
theoretical gap: a commit that made the frontend `return Err` on any terminal without the
kitty keyboard protocol left the emulator unable to start in GNOME Terminal, Ptyxis, Konsole
and xterm, and the whole suite stayed green through it.

[`testharness/basicterm.rs`](../testharness/basicterm.rs)
(`cargo test --release --test basicterm`, ~19 s) fills it, and the shape is worth copying for
anything else that has to be judged from the outside:

* It allocates a **pty** and *is* the terminal on the far end. No window is opened and no
  synthetic input is sent to the desktop, so it runs in CI and cannot disturb whoever is
  sitting at the machine.
* The child is the **real** `terminalgb::run_terminal`, re-executed from the harness binary
  itself with an env flag — no separate artifact to build, no nested `cargo` (which would
  deadlock on the build lock). It gets its own session (`setsid` + `TIOCSCTTY`) so even its
  `/dev/tty` probes land on the pty.
* Two scenarios differ by **one reply**. The fake terminal answers `CSI c`, `CSI 5 n`,
  `CSI 6 n`, `CSI 14 t` and `CSI 16 t` in both, and answers the kitty keyboard query
  `CSI ? u` in only one. The frontend must report `kitty` in that run and `basic` in the
  other — which is what makes it a test *of* the fallback rather than an assumption about it.
* It then checks the things a user would: it drew, the degradation notice appears only where
  it applies, a keypress reaches the game, a held control key does not strobe, and `q` quits
  with status 0.

Three traps, all of which produced convincing-but-wrong results before they were understood:

1. **A silent fake terminal is not a neutral one.** Every unanswered query costs the frontend
   a multi-second timeout in a blocking path; leaving out `CSI 5 n` / `CSI 14 t` / `CSI 16 t`
   drove the emulator at **1 % of real speed** and made input look lost when it was only
   queued.
2. **Never assert on a word in the raw byte stream.** `ratatui` diffs per *cell*, so
   `grayscale` → `green` transmits `een`, and `green` → `amber` transmits `amb`, a skip, `r`.
   Resize the pty (`TIOCSWINSZ`) to force a full repaint, then read.
3. **Mind the modulus.** The palette cycle is three long and the first press of a burst is a
   real press, so ten suppressed repeats and ten unsuppressed ones land on the same colour.
   The harness sends twelve.

## Mapping suites to components

| Component | Primary suites | The approximations they will catch |
|---|---|---|
| [CPU](components/cpu.md) | Blargg `cpu_instrs`, `instr_timing`, `mem_timing` (**pass**), `halt_bug` (**pass**), `interrupt_time` (**pass**); Mooneye `acceptance/interrupts`, `instr` (gated) | — |
| [PPU](components/ppu.md) | dmg-acid2 (**pass, pixel-verified**), cgb-acid2; Mooneye `acceptance/ppu` (**4/12**, gated); Mealybug Tearoom (**3/79**, gated) | Fixed mode-3 length, whole-line render, STAT IRQ blocking (structural — needs a cycle-stepped PPU) |
| [MMU](components/mmu.md) | Blargg `mem_timing` (**pass**), `oam_bug@dmg` (**pass**); Mooneye `acceptance/oam_dma` (**3/3**), `bits` (**3/3**), SameSuite `dma` (**4/4**) — all gated | No VRAM/OAM mode-blocking (structural) |
| [MBC](components/mbc.md) | Mooneye `emulator-only/mbc1/2/5`; Blargg `bits-bank1`; **`testharness/battery.rs`** (**pass**, battery-save durability + `.sav` interop) | Banking bit maths; the MBC3-for-everything web/FFI bug |
| [APU](components/apu.md) | Blargg `dmg_sound` (**12/12 pass**, `audiocapture`), `cgb_sound` (**12/12 pass**, wired into `testroms` 2026-08-14); SameSuite APU (**63/69**, gated) | 6 SameSuite APU rows still red |
| [Timer](components/timer.md) | Mooneye `acceptance/timer` (**13/13**, gated); Blargg `interrupt_time` (**pass**) | — one 16-bit system counter with falling-edge detection, since 2026-08-14 |
| [Serial](components/serial.md) | Blargg (as its reporting channel); Mooneye `acceptance/serial` (**1/1**, gated) | Instantaneous transfer; `boot_sclk_align` needs the boot ROM's own clock phase |
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

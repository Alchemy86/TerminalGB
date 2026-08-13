# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

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
- Known-inaccurate on purpose: HALT bug and DMG OAM-corruption bug not modelled; OAM DMA is
  instantaneous and unguarded; PPU mode 3 is fixed-length; timer uses separate DIV/TIMA
  counters (no falling-edge quirks); interrupt dispatch charges 16 T-cycles vs hardware's 20.
- `get_mbc` (`src/mbc/mod.rs`) returns MBC3 for **any** cartridge when built without a
  filepath — always the case for the web and FFI builds, so non-MBC3 games are mis-driven.
- `src/gpu/old_gpu.rs` (961 lines) is dead/unreferenced; `src/cpu/z80` is a non-compiled
  reference file, not a module.
- The `retrio/gb-test-roms` submodule (`.gitmodules`) holds the Blargg ROMs; it is now
  initialised and wired into CI (see the test-ROM harness below).

## Test-ROM correctness harness

- **Runner:** `testharness/testroms.rs`, a `harness = false` Cargo integration test
  (`[[test]] name = "testroms"`). Run it with `cargo test --release --test testroms`. It
  lives in `testharness/` (not `tests/`) because **`tests/` is the gb-test-roms submodule** —
  Cargo cannot track `.rs` files inside a submodule path.
- **Two mechanisms:** Blargg → serial string (`Serial::set_callback`) capped at 250M cycles;
  dmg-acid2 + Blargg screen-only ROMs → FNV-1a hash of the RGBA framebuffer over a fixed 600
  frames. On mismatch it dumps the actual frame to `target/testrom-artifacts/<id>.png`.
- **Baseline:** `testharness/baseline.txt` records the expected result of every ROM. CI is
  red only on a *change* from it (regression or improvement). Update deliberately with
  `UPDATE_BASELINE=1 cargo test --release --test testroms`. Full explanation in
  `docs/testing.md`.
- **Sharp edges learned:** (1) always drive test ROMs via `Cpu::new(data, Some(path))` — with
  `None`, `get_mbc` forces MBC3 and mis-drives them. (2) The MBC writes a `.gbsave` next to
  the ROM path, so the harness **copies each ROM into `target/` first** to keep the submodule
  checkout clean. (3) Emulation is deterministic (WRAM seeded with a fixed value, audio never
  enabled), so frame hashes reproduce exactly — the harness asserts this at startup.
- **Vendored ROM:** `third_party/dmg-acid2/` holds the prebuilt MIT acid2 ROM + reference PNG
  (upstream ships source only); `.gitignore`'s `**/*.gb` rule is negated for that one file.
- No new crate dependencies: the hash and PNG writer are hand-rolled.
- `src/cpu/z80` is a non-compiled reference file, not a module. (`src/gpu/old_gpu.rs`,
  961 lines of dead/unreferenced code, was deleted — no `mod old_gpu` ever existed.)
- The `retrio/gb-test-roms` submodule (`.gitmodules`) is declared but uninitialised.

- Add durable project-specific notes here as they are discovered through real work.

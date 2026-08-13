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
  → **Blargg `mem_timing` + `mem_timing-2` PASS**. Only the timer is per-access; PPU/APU
  stay batched, so frame rendering is unchanged. Costs ~15% on a pure-CPU benchmark
  (headless still ~60× realtime). The **HALT bug IS modelled** now (`halt_bug` flag; PC
  rewind after the post-HALT fetch) → **Blargg `halt_bug` PASSES**.
- Known-inaccurate **on purpose (documented non-goals — see component pages)**: DMG
  OAM-corruption bug not modelled (`oam_bug` red-by-design — DMG defect, we boot CGB); OAM
  DMA instantaneous/unguarded and PPU mode 3 fixed-length (both need a cycle-stepped
  PPU/CPU); timer uses separate DIV/TIMA counters (no falling-edge quirks — no wired test);
  interrupt dispatch charges 16 T-cycles vs hardware's 20 (its only test, `interrupt_time`,
  can't run headless — it needs an APU: `sync_apu` spins on `NR52`).
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
  **The terminal frontend (`src/screen/tui.rs`, another crew) has the same bug: a 5 ms tick =
  ~200 fps target, uncapped.** Same fix applies there (gate `frame()` on 16.743 ms).
- **cpal negotiation gotcha:** `device.supported_output_configs()` often lists the first `F32`
  range as `ch=1, min=1, max=384000` (mono!). Never break on the first F32 range. Prefer
  `device.default_output_config()` when it's F32 (real channel count + a rate the HW actually
  runs); else scan all F32 ranges preferring span-44100 then max channels. `examples/audio_probe.rs`
  dumps a machine's configs + negotiated rate (`cargo run --features audio --example audio_probe`).
- **Bound the hand-off queue.** `CpalPlayer`'s `Arc<Mutex<Vec>>` cap governs worst-case latency
  before samples are dropped; keep it small (~4 output blocks ≈ 180 ms), not 1 s.
- **Latent bug (not fixed — would touch accuracy baseline):** `LengthCounter::wb` for the
  64-max channels (`sound.rs`, `0xFF11|0xFF16|0xFF20`) does `self.max - (v as u16)` without
  masking the duty bits, so a duty-carrying write (e.g. NR11=0x80) panics in debug / wraps in
  release. Mask `v & 0x3F` if ever touched; re-bless `testharness/baseline.txt` if hashes move.

- Add durable project-specific notes here as they are discovered through real work.

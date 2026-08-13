# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: minor versions may
carry breaking changes).

This changelog begins where this repository diverged from upstream — the emulator did **not**
appear from nothing. Its foundation is **[raphamorim/gameboy](https://github.com/raphamorim/gameboy)**
by Raphael Amorim, itself closely modelled on **[mvdnes/rboy](https://github.com/mvdnes/rboy)**;
the entries below record only the changes made in this fork and do not reconstruct the upstream
history. See [Credits & lineage](README.md#credits--lineage).

## [Unreleased]

### Added
- Repository furniture: this `CHANGELOG.md`, `CONTRIBUTING.md`, and GitHub issue / pull-request
  templates.
- README badges (CI status, licence, Rust version) and a "Credits & lineage" section naming
  the fork parent and rboy.

### Changed
- `Cargo.toml` metadata: `repository` now points at this fork, authorship represents both the
  original author and this fork's contributors, `version` bumped to `0.2.0`, and the
  description updated.

### Removed
- `.github/FUNDING.yml`, which pointed sponsorship at the upstream author from this fork (see
  the notes in the pull request).

## [0.2.0] - 2026-08-13

A large accuracy, audio, rendering, and tooling pass on top of the fork baseline.

### Added
- **Headless test-ROM correctness harness** (`testharness/testroms.rs`): Blargg serial-string
  capture + dmg-acid2/Blargg-screen frame hashing, gated in CI against a checked-in
  known-failures baseline (`testharness/baseline.txt`); red only on a change from baseline.
- **Battery saves**: cartridge RAM now persists to a `.sav` file *while the game runs*
  (atomically, not only on a clean exit), using the ecosystem `.sav` format, with a durability
  harness (`testharness/battery.rs`).
- **Save states**: a versioned, ROM-checked native `GBSS` format in the core
  (`Gameboy::save_state`/`load_state`), with a byte-identical round-trip proof harness
  (`testharness/savestate.rs`), exposed as quick-save/-load slots in the terminal frontend.
- **Pixel-art upscaler** (`src/upscale/`): runtime-selectable `nearest`/`scale2x`/`mmpx`/`lcd`,
  dependency-free and wasm/FFI-safe, wired into the desktop, terminal, and web frontends.
- **Audio pipeline**: an offline audio-capture/analysis harness (`testharness/audiocapture.rs`)
  and a measured queue-policy/pacing implementation for real host playback.
- **Telemetry**: NDJSON run data (`src/telemetry.rs`) mirroring the stats overlay, off by
  default (`GB_TELEMETRY=<path>`); a one-shot `render_bench --json` summary mode for CI.
- **Terminal frontend**: a proper `ratatui` TUI with a ROM picker, recents, in-app ROM
  switching, per-session audio, stats overlay, speed ladder, and save slots.
- **Documentation**: per-component reference pages (`docs/components/`), a top-level code map
  with generated GitHub links (`docs/codemap.md` + `make docs-links`/`docs-check`), and design
  docs for testing, audio, save states, upscaling, terminal rendering, telemetry, debugging,
  and the planned conformance tool.

### Changed
- **Audio is now off by default**: `cpal` is an optional dependency behind the `audio` feature,
  so a fresh clone builds/tests/runs with no system audio library (no ALSA on Linux).
- **Timer stepped per CPU memory access** and the **APU clocked per access**, so reads/writes
  land on the correct sub-instruction cycle.
- **Terminal render path**: transmitted image sized by the frame, not the window; emulation
  decoupled from rendering; correct 59.7275 Hz pacing.
- **Desktop rendering**: crisp integer-scaled, aspect-correct (10:9) output.
- Removed dead code (`src/gpu/old_gpu.rs`).

### Fixed
- **Accuracy**: Blargg `mem_timing`/`mem_timing-2` pass (per-access timer); the DMG/CGB
  **HALT bug** is modelled so `halt_bug` passes; `dmg_sound` improved 1/12 → 12/12 via
  per-access APU clocking and the DMG wave-RAM access window.
- **Audio**: fixed "fast and rough" playback (frame pacing + device negotiation) and cut
  end-to-end latency from ~299 ms to ~53 ms with no underruns.
- **Terminal HiDPI**: corrected the "tiny picture" on scaled Wayland displays via a
  `cell_scale` configuration key.
- **MBC**: three crashes fixed alongside adopting the ecosystem `.sav` format.

[Unreleased]: https://github.com/Alchemy86/gameboy/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Alchemy86/gameboy/releases/tag/v0.2.0

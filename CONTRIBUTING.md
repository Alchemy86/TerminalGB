# Contributing

Thanks for your interest. This is a Game Boy / Game Boy Color emulator in Rust; it runs on
the desktop, in the terminal, and in the browser (WebAssembly). This guide covers the things
you'll actually hit building and testing it.

## Prerequisites

- **Rust 1.83.0** — pinned in [`rust-toolchain`](rust-toolchain), so `rustup` selects it
  automatically in this directory. `make lint` uses `rustfmt` and `clippy`; add them with
  `rustup component add rustfmt clippy`.
- A `.gb`/`.gbc` ROM of your own. The repo ships **no** commercial ROMs — only the vendored
  MIT `dmg-acid2` test ROM under `third_party/`.

### Audio is off by default (no ALSA needed for a normal build)

Host audio playback lives behind the **`audio`** cargo feature, which is **off by default**.
`cpal` (the audio backend) is an *optional* dependency, so a plain build/test/run needs **no
system audio library**:

```bash
cargo build --release          # no ALSA required
```

To hear sound you enable `audio`, and on **Linux** that pulls in the ALSA development headers:

```bash
sudo apt install libasound2-dev      # Debian/Ubuntu
sudo dnf install alsa-lib-devel      # Fedora
cargo run --release --features audio --example desktop -- path/to/game.gb
```

Without the headers, an `audio` build fails with `could not find system library 'alsa'`.
The APU itself is *always* emulated; only speaker playback is gated. See the README's
audio section for the full rationale.

## Building & running

```bash
cargo run --release --example desktop  -- path/to/game.gb   # OpenGL window
cargo run --release --example terminal -- path/to/game.gb   # terminal (kitty-protocol)
cargo build --release --target wasm32-unknown-unknown       # WebAssembly (audio compiled out)
make web                                                    # serve the web build (see web/)
```

The `Makefile` wraps the common flows: `make desktop`, `make terminal`, `make build-wasm`,
`make web`, `make ffi-build`.

Terminal configuration (protocol, palette, upscaler, key bindings, HiDPI `cell_scale`) lives
at `${XDG_CONFIG_HOME:-~/.config}/gameboy/config.toml` — documented in the README's
"Terminal config file" table.

## Tests

```bash
cargo test  --release --lib                    # library unit tests
cargo test  --release --test testroms          # headless test-ROM accuracy harness
cargo test  --release --test savestate         # save-state round-trip proof
cargo test  --release --test audiocapture      # offline APU/audio capture + analysis
```

### The test-ROM harness and its baseline

`cargo test --release --test testroms` drives Blargg and dmg-acid2 ROMs headlessly and
compares each result against a checked-in **known-failures baseline**,
[`testharness/baseline.txt`](testharness/baseline.txt). **CI is red only on a *change* from
that baseline** — a regression *or* an unrecorded improvement — not on the raw pass/fail
count. Some ROMs are pinned red on purpose (documented non-goals such as `oam_bug` and
`interrupt_time`); see [`testharness/README.md`](testharness/README.md).

If your change moves a result **for a good reason**, re-bless the baseline deliberately and
explain why in your commit:

```bash
UPDATE_BASELINE=1 cargo test --release --test testroms
```

The audio harness has the same model with `UPDATE_AUDIO_BASELINE=1`. Any change to
`src/sound.rs` synthesis moves its hashes — **listen to the WAVs in
`target/testrom-artifacts/audio/` before re-blessing.**

> The Blargg ROMs live in the `tests/` git submodule. Clone with `--recurse-submodules`, or
> run `git submodule update --init --recursive`. The harness copies each ROM into `target/`
> before running so `.gbsave` files never dirty the submodule checkout.

### The standing convention: every emulation fix feeds the conformance tool

When you fix an **emulation** bug, capture it as a test case *in the same change*: record the
user-visible symptom, which test ROM catches it (or note that none does — a coverage gap is a
finding), the hardware behaviour violated with a citation, and any new test the fix produced.
The full convention is in [`CLAUDE.md`](CLAUDE.md); this is what makes the planned standalone
[conformance tool](docs/conformance.md) worth extracting.

## Formatting & lint

Both are enforced in CI and must pass:

```bash
make lint      # cargo fmt --check  +  cargo clippy --all-targets -D warnings
```

`clippy` runs with `-D warnings`, so warnings fail the build. Run `cargo fmt` before you
commit.

## Documentation

Per-component docs live in [`docs/components/`](docs/components/README.md) and the top-level
map in [`docs/codemap.md`](docs/codemap.md). Every `path:line` reference in the docs is a
**generated** GitHub link. **After editing source that shifts line numbers, regenerate them:**

```bash
make docs-links     # rewrite path:line references + restamp the "documented as of" markers
make docs-check     # CI check: fails on a missing file or out-of-range line number
```

When you touch emulation code, keep the relevant component page's "how it works here"
references and "honest approximations" in sync.

## Commit & PR

- Keep commits focused; use a clear, imperative subject (the history uses
  `feat(...)`/`fix(...)`/`docs(...)` prefixes — follow suit).
- Make sure `make lint`, the unit tests, and `cargo test --release --test testroms` pass
  (unless you deliberately re-blessed the baseline — say so).
- Note user-facing changes in [`CHANGELOG.md`](CHANGELOG.md) under `## [Unreleased]`.
- Fill in the pull-request template.

## Third-party material & licences

This repository is **MPL-2.0**, and it is itself a fork — getting attribution and licence
compatibility right is part of the project's standard, not an afterthought. Two rules:

- **Credit upstream, don't strip it.** This project builds on
  [raphamorim/gameboy](https://github.com/raphamorim/gameboy) and
  [mvdnes/rboy](https://github.com/mvdnes/rboy); that lineage stays credited in the README and
  `Cargo.toml`. If you port a discrete piece of someone else's work, keep its attribution
  comment (see the MIT MMPX port in `src/upscale/`).

- **Facts and formats, yes — incompatible code, no.** You may learn hardware behaviour, file
  and packet **formats**, protocol knowledge, and header constants from any source and
  reimplement them cleanly. You may **not** copy source code under a licence incompatible with
  MPL-2.0 into this tree. Concretely:
  - Scale2x was **reimplemented** from its published rule because the reference is GPL-2.0;
    xBRZ (GPLv3) and hqx (LGPL tables) are deliberately **excluded** — do not vendor them.
  - When studying GPL-licensed tooling (e.g. FlashGBX, GPL-3.0) or other emulators/link tools
    for cartridge or link-protocol work, take the **formats, constants, and documented
    behaviour** only — never their code or bundled data files. Reimplement from the spec.

When in doubt about a licence, ask before vendoring.

## Licence

By contributing you agree that your contributions are licensed under the project's
[MPL-2.0](LICENSE). Upstream copyright and the licence text in [`LICENSE`](LICENSE) must not be
removed.

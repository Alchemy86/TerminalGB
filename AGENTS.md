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
- **"Blocky and broken" was the pacing again, plus queue policy.** Both frontends reset their
  frame timer with `last = Instant::now()` *after* stepping — the period silently included
  draw/poll overhead, so the emulator ran a few % **slow**, the audio queue drained, and each
  underrun stepped to 0.0 (a click) then played the next 45 ms block: machine-gun gaps. Both
  loops now use a **deadline accumulator** (`next += FRAME_TIME`, bounded catch-up) — keep it
  that way; never reset a pacing timer to `now()`. The queue is `sample_queue::SampleQueue`
  (`src/sound.rs`): prime to 7000 samples before draining, hold-and-fade + re-prime on
  underrun, producer *waits* at a 12000-sample high watermark (audio clock = master clock),
  drop only when the callback counter proves the consumer stalled. Sizes came from the
  simulation in `mod queue_tests` + a real 2-min soak (`examples/audio_soak.rs`,
  `GB_AUDIO_STATS=1`): zero underruns/drops, depth 5731–8717, residual drift −0.013 %
  (≈ one smooth ~160 ms re-prime per ~20 min; full DRC is the documented next step).
- **cpal negotiation gotcha:** `device.supported_output_configs()` often lists the first `F32`
  range as `ch=1, min=1, max=384000` (mono!). Never break on the first F32 range. Prefer
  `device.default_output_config()` when it's F32 (real channel count + a rate the HW actually
  runs); else scan all F32 ranges preferring span-44100 then max channels. `examples/audio_probe.rs`
  dumps a machine's configs + negotiated rate (`cargo run --features audio --example audio_probe`).
- **Offline audio harness:** `cargo test --release --test audiocapture`
  (`testharness/audiocapture.rs`) captures the APU's sample stream device-free, writes WAVs to
  `target/testrom-artifacts/audio/`, checks rate/pitch/level/continuity (built-in 440 Hz tone
  ROM), reads Blargg `dmg_sound`'s memory verdict at `$A000` (**currently 1/12 pass** — the
  accuracy frontier), and pins an FNV hash in `testharness/audio_baseline.txt`
  (re-bless: `UPDATE_AUDIO_BASELINE=1`). Any change to `src/sound.rs` synthesis will move
  these hashes — listen to the WAVs before re-blessing.
- **APU semantic fixes to preserve:** NRx1 length writes mask duty bits (`v & 0x3F`; NR11=0x80
  used to panic in debug); NR52 power-off zeroes NR10–NR51 *while still on* (clearing after
  `on=false` is a no-op because `wb` ignores writes when off); a powered-off APU still emits
  silence at full sample rate (never stall the stream).
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
  error. `rom_dir` defaults to `~/roms`.
- The picker opens with the most-recently-played ROM pre-selected (recents sorted first), so
  **Enter loads the last game**; `.` resumes the last-played ROM directly even if it lives
  outside `rom_dir`.
- **ALSA is only needed with `--features audio`** (default builds/tests need none). For that
  feature without system dev headers, a shim exists at `~/.local/lib/alsashim/libasound.so` +
  `~/.local/lib/pkgconfig/alsa.pc`; build with `PKG_CONFIG_PATH=~/.local/lib/pkgconfig
  LIBRARY_PATH=~/.local/lib/alsashim LD_LIBRARY_PATH=~/.local/lib/alsashim cargo build
  --features audio`.

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

- Add durable project-specific notes here as they are discovered through real work.

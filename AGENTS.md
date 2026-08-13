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
  (`src/sound.rs`): prime before draining, hold-and-fade + re-prime on underrun, producer
  *waits* at a high watermark (audio clock = master clock), drop only when the consumer is
  proven stalled. Sizes come from the simulation in `mod queue_tests` + real soaks
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
  3. **`high_water()`, not `start_fill()`, sets steady-state latency** — the producer *blocks*
     there, so a slightly-fast frame timer parks the queue at the top. Now 1792/2816 samples
     (41/64 ms), sized to a stated promise: *absorb one dropped frame* (a dropped frame costs
     the frame **plus** the normal 16.7 ms period, since the queue drains continuously).
  Result, 5-min real soak: **52.7 ms end-to-end (46–60), 0 underruns, 0 drops, drift −0.0005 %**.
  Measured stall reality on this box: 4 of 17899 frames >1 ms late, p99.9 = 0.0 ms — so
  `queue_tests`' profile is derived from that, not from an assumed 90 ms stall.
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

- Add durable project-specific notes here as they are discovered through real work.

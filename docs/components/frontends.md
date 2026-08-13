# Frontends — desktop, terminal, web & FFI

> Source: [`src/screen/`](../../src/screen), [`examples/`](../../examples), [`web/`](../../web), [`src/lib.rs`](../../src/lib.rs) · Related: [Joypad](input.md) · [APU](apu.md) · [Display scaling](../display.md)
> [← Component index](README.md)

---

## 1. What it is

A frontend is the "outside" of the emulator: it opens a window (or a terminal, or a browser
canvas), pushes the emulator forward one frame at a time, draws the resulting 160×144 image,
and feeds key presses back in. The emulator **core** is frontend-agnostic — every frontend
drives the same tiny surface: `Gameboy::new(rom) → frame() → image()`, plus `keydown` /
`keyup` ([`src/gameboy.rs`](../../src/gameboy.rs)). This emulator ships four:

- **Desktop** — a native OpenGL window.
- **Terminal** — pixels drawn in your terminal via the kitty/sixel image protocols.
- **Web** — WebAssembly drawing to an HTML `<canvas>`.
- **FFI** — a C ABI so other languages (there's a Go example) can embed the core.

## 2. The real hardware

There is no single "frontend" chip — a frontend stands in for the parts of the *console
around* the CPU: the **LCD** (a 160×144, 59.7 Hz reflective panel with no backlight on DMG),
the **speaker/headphone jack**, and the **button pad**. The relevant realities a frontend
must respect:

- The screen is **160×144** and refreshes at **~59.7 Hz**; a frame is one PPU pass.
- Output is **4 grey shades** (DMG) or **15-bit RGB** (CGB) — the core hands over an RGBA
  framebuffer already resolved to colour.
- Audio is stereo, generated continuously (see [APU](apu.md)).

## 3. History and versions

Frontends are an emulator concern, not Game Boy hardware, so there's no DMG/CGB lineage
here. What's worth recording is **how this project's frontends evolved**: the core began as
a web/WASM study (the crate is still described as "Gameboy emulator written in Rust and
WebAssembly"), then grew a native desktop path (glutin + raw OpenGL), a terminal renderer
(ratatui + image protocols), and a C FFI. Sound was added last (native only, Aug 2025) and
never reached the web build.

## 4. How it works here

The renderers live in `src/screen/`; the `examples/` binaries are thin `main()`s that pick
a `RenderMode`; `web/` is the JS/webpack shell around the WASM build.

**Desktop** — [`src/screen/desktop.rs`](../../src/screen/desktop.rs). `Glcx`
([`desktop.rs:17`](../../src/screen/desktop.rs)) sets up a textured quad and uploads each
frame via `gl::TexImage2D` in `draw()` ([`desktop.rs:301`](../../src/screen/desktop.rs)).
The event loop lives in `Gameboy::render_desktop` ([`src/gameboy.rs:84`](../../src/gameboy.rs)),
which also initialises `cpal` audio. Key mapping is in `process_window`
([`desktop.rs:81`](../../src/screen/desktop.rs)): **Z→Select, X→Start, A→A, B→B**, arrows →
D-pad. Frame pacing targets ~60 FPS ([`gameboy.rs:125`](../../src/gameboy.rs)).

**Terminal** — [`src/screen/tui.rs`](../../src/screen/tui.rs). Uses `ratatui` +
`ratatui-image` to draw the framebuffer with the terminal's image protocol (sixel/kitty).
`q` quits, `i` cycles image protocol, `o` scales.

**Web** — [`src/screen/web.rs`](../../src/screen/web.rs). `#[wasm_bindgen] render(rom)`
([`web.rs:44`](../../src/screen/web.rs)) creates a 160×144 `<canvas>`, builds
`Gameboy::new(rom, None)`, and in a `requestAnimationFrame` loop calls `frame()` and blits
via `put_image_data` ([`web.rs:123`](../../src/screen/web.rs)). Keys are read from a shared
`Rc<RefCell<i32>>` keycode. The JS shell ([`web/index.js`](../../web/index.js)) is a file
picker + a Play button that calls `render(rom)`.

**FFI** — [`src/lib.rs`](../../src/lib.rs) exposes `#[no_mangle]` C functions `load`,
`frame`, `keydown`, `keyup`, `image` over a `OnceLock<Mutex<Option<Gameboy>>>`. The header
is [`gameboy.h`](../../gameboy.h); [`examples/ffi-go/main_static.go`](../../examples/ffi-go/main_static.go)
statically links `libgameboy.a` and calls them from Go.

### Honest problems & approximations

These are the frontends' rough edges (from the scout report, confirmed in code):

- **The examples don't take a ROM path.** `examples/desktop/bin.rs:9` hardcodes
  `/Users/rapha/Downloads/dragon-ball.gbc` and `examples/terminal/bin.rs:8` hardcodes
  another `/Users/rapha/...` path, each with a `// TODO: Allow receive path by arguments`
  ([`examples/desktop/bin.rs:6`](../../examples/desktop/bin.rs)). `make desktop`/`make
  terminal` fail for anyone but the original author until edited.
- **Terminal needs a kitty-keyboard-protocol terminal.** `tui.rs:46` returns immediately
  with a WARN if `supports_keyboard_enhancement()` is false — works in kitty/WezTerm/Rio/
  foot, silently no-ops elsewhere ([`src/screen/tui.rs:46`](../../src/screen/tui.rs)).
- **No web audio.** The `sound` module is compiled out on wasm ([`src/lib.rs:16`](../../src/lib.rs)).
- **Web mis-detects cartridges.** The FFI/web path always builds MBC3 (see [MBC](mbc.md) §4).
- **`RenderMode::WebAssembly` is a no-op arm** ([`src/gameboy.rs:73`](../../src/gameboy.rs))
  and `lib.rs::start()` is a TODO stub — the real web path enters through
  `#[wasm_bindgen] render()`, bypassing `render()`.
- **Inconsistent key mapping for "B".** The desktop/web builds map B to a specific host
  key, the terminal binds B to the physical `b`/`B` keys, and the TUI's on-screen help text
  (`tui.rs:358`) says something different again. Worth reconciling.
- **Dead code:** commented-out `render` in `desktop.rs`, WebGL helpers in `web.rs`, and the
  `docs/*.wasm` prebuilt blobs (~2.2 MB) are stale build artifacts.
- **Display scaling is rough:** desktop upscales with bilinear `LINEAR` (blurry) and never
  calls `gl::Viewport`, so window resize doesn't fill correctly; the web canvas is a fixed
  1:1 160×144 with no `image-rendering: pixelated`. Full analysis and fixes:
  **[docs/display.md](../display.md)**.

## 5. Known quirks and edge cases

Frontends don't emulate hardware quirks, but they must respect these console realities:

- **Frame pacing vs. real 59.7 Hz.** The frame loop ([`gameboy.rs:178`](../../src/gameboy.rs))
  runs `~67,109` cycles or until the PPU signals a finished frame — a whole-frame batch,
  not real-time audio-synced pacing. Fast/slow hosts can drift from 59.7 Hz.
- **Colour resolution happens in the core**, so a frontend must not re-gamma the RGBA it
  receives (the CGB Gambatte correction already ran in the [PPU](ppu.md)).
- **Audio/video sync** is loose — audio is drained opportunistically by the `cpal` callback;
  there's no A/V clock.

## 6. Expected behaviours as tests

> Frontends are I/O-bound and largely untestable without a display/audio device, so the
> testable assertions here are about the **core surface** the frontends share. **Coverage
> today: none.**

1. **`frame()` produces a full framebuffer.** After `Gameboy::new(rom)` and one `frame()`,
   `image()` returns `160*144*4` bytes. *(Not covered.)*
2. **`image()` length is stable** across frames (`160×144×4 = 92,160`). *(Not covered.)*
3. **Key mapping round-trip.** `keydown(A)` then reading `0xFF00` with the button row
   selected shows A pressed (see [Joypad](input.md)). *(Not covered.)*
4. **Examples accept a ROM path argument.** `cargo run --example desktop -- game.gb` loads
   `game.gb`. **Expected to FAIL — paths are hardcoded.** *(Not covered; known bug.)*
5. **Headless build without ALSA.** With audio feature-gated off, `cargo build
   --no-default-features` succeeds on a box with no sound headers. **Expected to FAIL
   today — `cpal` is non-optional.** *(Not covered; known issue — prerequisite for the
   test harness in [testing](../testing.md).)*
6. **Web uses the correct MBC.** A loaded MBC1 ROM is driven as MBC1, not MBC3. **Expected
   to FAIL — see [MBC](mbc.md).** *(Not covered; known bug.)*

## 7. Test ROMs that exercise it

Frontends are validated by **the acid2 image tests** (they need a real framebuffer path to
render — see [testing](../testing.md)) and by **real commercial games booting to a playable
state**. There is no Blargg/Mooneye suite for windowing, terminals, or FFI — those are
integration concerns, not hardware behaviour. The most valuable frontend "test" is the one
the [testing](../testing.md) harness enables: a headless frontend that renders a frame and
compares it to a reference PNG.

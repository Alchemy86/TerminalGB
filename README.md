# gameboy

[![CI](https://github.com/Alchemy86/gameboy/actions/workflows/ci.yml/badge.svg)](https://github.com/Alchemy86/gameboy/actions/workflows/ci.yml)
[![License: MPL-2.0](https://img.shields.io/badge/license-MPL--2.0-blue.svg)](LICENSE)
[![Rust 1.83+](https://img.shields.io/badge/rust-1.83%2B-orange.svg)](rust-toolchain)

**A Game Boy / Game Boy Color emulator written in Rust — runs on the desktop, in your terminal, and in the browser via WebAssembly.**

> This project is a fork of **[raphamorim/gameboy](https://github.com/raphamorim/gameboy)** by
> Raphael Amorim, itself closely modelled on **[mvdnes/rboy](https://github.com/mvdnes/rboy)**.
> It stands on their work — see **[Credits & lineage](#credits--lineage)**.

<p align="center">
  <img src="resources/demo-pokemon.gif" alt="Pokémon running in the browser build" width="480" />
</p>

| Web (WASM) | Desktop | Terminal |
|:---:|:---:|:---:|
| ![Web](resources/demo-web.png) | ![Desktop](resources/demo-desktop-1.png) | ![Terminal](resources/demo-terminal-1.png) |

> ⚠️ **Status: playable, work in progress.** Many commercial games boot and play. The CPU
> passes Blargg's `cpu_instrs`, `instr_timing`, `mem_timing` and `halt_bug`; a few
> edge-cases (interrupt-dispatch cycle count, the DMG OAM-corruption bug, parts of CGB
> sound, variable mode-3 timing) remain known-inaccurate as documented non-goals — see
> **[Status](#status)**, the [test scoreboard](testharness/README.md), and the honest
> per-component notes. This project is a study of emulation, not a piracy tool: it ships no
> ROMs and no BIOS.

---

## Quick start

Requires **Rust 1.83+** (pinned via [`rust-toolchain`](rust-toolchain)). Bring your own
`.gb`/`.gbc` ROM.

> **Audio is opt-in.** By default the emulator builds and runs **without** any system
> audio library — a fresh `cargo build`/`cargo test`/`cargo run` needs no extra packages.
> The APU is still fully emulated; only speaker playback is off.
>
> **To hear sound, enable the `audio` feature** (this pulls in `cpal`, which on **Linux**
> requires the ALSA development headers):
> ```bash
> sudo apt install libasound2-dev      # Debian/Ubuntu
> sudo dnf install alsa-lib-devel      # Fedora
> cargo run --features audio --example desktop -- path/to/game.gb
> ```
> Audio ships **off by default** so that a fresh clone builds with zero system
> dependencies; turn it on explicitly once ALSA (or your platform's audio backend) is
> available.

### 🖥️ Desktop (OpenGL window)

```bash
cargo run --release --example desktop -- path/to/game.gb
```

Controls: **Z** = Select · **X** = Start · **A** = A · **B** = B · **Arrow keys** = D-pad ·
**U**/**I** cycle the pixel-art upscaler (algorithm / factor). The window is resizable and
scales crisply (nearest-neighbour, integer-letterboxed 10:9).

> The ROM path is the first argument; run with no argument and it prints a usage line and
> exits cleanly. Add `--features audio` for sound (see the ALSA note above).

### ⌨️ Terminal

```bash
cargo run --release --example terminal -- path/to/game.gb          # no sound
cargo run --release --features audio --example terminal -- path/to/game.gb   # with sound
```

> The ROM path is the first argument; run with no argument and it prints a usage line and
> exits cleanly. Needs a terminal that supports the **kitty keyboard protocol** for input
> and, ideally, the **kitty graphics protocol** for a crisp picture (kitty, WezTerm,
> Ghostty, foot). If the kitty keyboard protocol is unavailable it exits with a clear error
> — often it's just disabled in your terminal config (e.g. WezTerm needs
> `enable_kitty_keyboard = true`) rather than truly missing
> ([`src/screen/tui.rs`](src/screen/tui.rs)). It opens sized to the terminal, auto-selects the
> sharpest graphics protocol the terminal advertises, and **auto-fits the picture to the pane**
> (it re-fits on resize — there is no manual zoom key). Launch with no ROM path and it opens an
> in-app **ROM picker**. Press **`?`** in-game for the full, always-current key list; the
> highlights are save-state slots (**`1`**–**`3`**, **`F5`** save / **`F8`** load), a speed
> ladder (**`+`**/**`-`**), a stats overlay (**`t`**), palette cycling (**`[`**), pixel-art
> upscaler cycling (**`]`** algorithm / **`}`** factor), **screenshots** (**`c`** what you see,
> **`C`** the raw 160×144 frame) and the **video stream** on/off (**`v`**). Audio works here too
> with `--features audio` (see the ALSA note above).
>
> *(The terminal frontend is under active development — the render crew is adding faster
> rendering and a machine-readable telemetry stream — so treat the exact keys as the `?`
> overlay reports them.)*
>
> **HiDPI displays:** on a *scaled* Wayland session some terminals (e.g. WezTerm) report their
> cell size in logical pixels but draw inline images in physical pixels, so the picture comes
> out shrunk by the scale factor. If the game renders far smaller than the window, add
> `cell_scale = auto` (or an explicit number like `cell_scale = 2.0`) to
> `~/.config/gameboy/config.toml`. The default is `1.0` (no correction).
>
> **Frame rate is independent of window size, and the picture barely touches the wire.**
> The frame is transmitted at the resolution the emulator computed (160×144, or the pixel-art
> filter's own output) and the terminal scales it to the cell rectangle, so a fullscreen
> window costs the same as a small one. On top of that the *transport* is chosen by asking
> the terminal what it supports: **81 bytes per frame** over Kitty shared memory, ~1.4 KB as
> a palette-indexed PNG, against 118 KB for the raw path this replaced — and an unchanged
> picture (a menu, a pause screen) costs nothing at all. Emulation is paced separately from
> drawing: if the terminal cannot repaint fast enough the *display* drops frames while the
> game itself stays at exactly 59.7275 fps. Press **t** for live stats — the `Proto` line
> names the transport in use — and read
> [**how a frame gets into a terminal**](docs/rendering.md), which explains the whole ladder
> with measured numbers and a terminal-compatibility table. If your terminal's own scaling
> looks too soft, `transmit_scale = 2` sends more pixels; `render_path = legacy` restores the
> old path.
>
> Config problems are reported rather than ignored: an unknown key, or a value that cannot be
> applied, prints a warning at startup and shows in the picker footer and the `?` overlay.
> Values may be bare, `"double-quoted"` or `'single-quoted'`.

#### 🔴 Live trainer view (Pokémon Red/Blue)

Press **`T`** while playing Pokémon Red or Blue and a panel appears beside the game showing
your **party live** — nickname, species, level, HP bar, status, types — your **badges**, and,
while you are in a battle, the **opponent's level, HP, stats and moves**. **`d`** opens a
detail view for one Pokémon with its moves and PP, its DVs, the stat experience the game keeps
hidden, its Hidden Power, and whether it **would be shiny if traded to Gen 2**; **`Tab`**
cycles the party. The focused Pokémon's sprite is decompressed straight out of your own
cartridge, so it works in the overworld and not just in battle.

It is **strictly read-only** — the plugin is handed a borrowing view of the machine and cannot
write to it — and it reads memory ten times a second rather than every frame, so it costs the
render path nothing. Every address is derived from the
[`pret/pokered`](https://github.com/pret/pokered) disassembly and cited by symbol name; any
cartridge that is not verified English Red or Blue is **refused with an explanation** instead
of shown wrong numbers. Full design, address table and verification:
[docs/trainer.md](docs/trainer.md).

#### Terminal config file

The terminal frontend reads `${XDG_CONFIG_HOME:-~/.config}/gameboy/config.toml` (a small
`key = value` file — a strict subset of TOML). A missing file just means defaults; unknown or
malformed keys are ignored, never an error. Values may be **bare or double-quoted** — both
`palette = green` and `palette = "green"` work (the parser strips optional surrounding quotes).
The recognised keys:

| Key | Values | Meaning |
|---|---|---|
| `rom_dir` | a path (`~` expands) | Directory the ROM picker browses. Default `~/roms`. |
| `protocol` | `auto` / `kitty` / `sixel` / `iterm2` … | Force a graphics protocol; `auto` (default) picks the best advertised. |
| `palette` | `grayscale` / `green` / `amber` | Recolour classic (DMG) games. Default `grayscale`. |
| `upscale` | `nearest` / `scale2x` / `mmpx` / `lcd` | Pixel-art upscaler run each frame. Default `nearest` (a no-op at factor 1). |
| `upscale_factor` | an integer | Scale factor, clamped to what the algorithm supports. |
| `cell_scale` | `auto` / a number ≥ 1.0 | HiDPI device-pixel-per-cell correction (see above). Default `1.0`. |
| `key_a` / `key_b` / `key_start` / `key_select` | a single character | Rebind the Game Boy buttons (movement is always the arrow keys). |
| `screenshot_dir` | a path (`~` expands) | Where **`c`** writes PNGs. Default `$XDG_PICTURES_DIR/gameboy`, else `~/Pictures/gameboy`. |
| `video_out` | `-` / a file / a FIFO / a directory / `off` | Stream raw frames out (see [video-out.md](docs/video-out.md)). Default `off`. The TUI refuses `-` — it owns stdout. |
| `video_format` | `rgba` / `rgb24` / `yuyv` | Wire pixel format. Default `rgba`; `yuyv` for the virtual-webcam route. |
| `video_size` | `WxH` | Letterbox the stream into this size, e.g. `640x480`. Default: the source size. |
| `video_source` | `native` / `display` | Stream the raw 160×144 framebuffer, or what you see (palette + upscaler). Default `native`. |
| `video_pace` | `realtime` / `emulated` | One frame per 1/59.7275 s of wall clock (fast-forward drops frames), or one per emulated frame. Default `realtime`. |
| `video_dir` | a path (`~` expands) | Where **`v`** records when no `video_out` is set. Default `$XDG_VIDEOS_DIR/gameboy`, else `~/Videos/gameboy`. |
| `video_device` | `/dev/videoN` | A `v4l2loopback` node to *verify*: the emulator warns if its camera label names a different game. |

### 🌐 Web (WebAssembly)

```bash
cd web
npm install
npm run serve     # dev server; open the printed localhost URL and choose a ROM
```

To produce an optimized bundle: `make build-wasm` (requires
[`wasm-opt`](https://github.com/WebAssembly/binaryen)). The web build has **no audio yet**
and currently mis-detects non-MBC3 cartridges — see [Status](#status).

### 📦 As a library

```toml
[dependencies]
gameboy = { git = "https://github.com/Alchemy86/gameboy" }
```

```rust
use gameboy::gameboy::{load_rom, Gameboy, RenderMode};

fn main() {
    let (rom, path) = load_rom("./game.gb").expect("load ROM");
    let gb = Gameboy::new(rom, Some(path));
    gb.render(RenderMode::Desktop); // or RenderMode::Terminal
}
```

---

## What's inside — the components

The core is a library (`src/`) that any frontend drives through a tiny surface:
`Gameboy::new(rom) → frame() → image()`, plus `keydown`/`keyup`. Under that surface each
piece of the console is a distinct component with **its own deep-dive page** under
[`docs/components/`](docs/components/README.md) — covering what it is, the real hardware,
its history across DMG/MGB/SGB/CGB, how our implementation works (with honest notes on where
it isn't cycle-perfect), the known hardware quirks, and **a numbered list of expected
behaviours phrased as future tests.**

| Component | Source | What it does | Deep dive |
|---|---|---|---|
| **CPU** | [`src/cpu/`](src/cpu) | Sharp **LR35902 (SM83)** core — registers, flags, full opcode + `CB` tables, interrupts. *It is not a Z80.* | **[docs](docs/components/cpu.md)** |
| **PPU (GPU)** | [`src/gpu/mod.rs`](src/gpu/mod.rs) | Scanline pixel engine — the mode cycle, background/window/sprites, DMG + CGB palettes, STAT/LYC timing. | **[docs](docs/components/ppu.md)** |
| **MMU** | [`src/mmu/mod.rs`](src/mmu/mod.rs) | The 64 KB memory bus — address routing, OAM DMA, CGB HDMA/GDMA, and the `IF`/`IE` interrupt wiring. | **[docs](docs/components/mmu.md)** |
| **Cartridges (MBC)** | [`src/mbc/`](src/mbc) | Bank switching for MBC0/1/2/3/5, MBC3's real-time clock, battery-backed saves. | **[docs](docs/components/mbc.md)** |
| **APU (Sound)** | [`src/sound.rs`](src/sound.rs) | Four channels (2 square + wave + noise), the 512 Hz frame sequencer, `blip_buf` → `cpal`. *Native only.* | **[registers](docs/components/apu.md)** · **[audio pipeline & quality](docs/audio.md)** |
| **Timer & Divider** | [`src/mmu/timer.rs`](src/mmu/timer.rs) | `DIV`/`TIMA`/`TMA`/`TAC` — the programmable interrupt heartbeat. | **[docs](docs/components/timer.md)** |
| **Serial** | [`src/mmu/serial.rs`](src/mmu/serial.rs) | The link cable — and the channel Blargg test ROMs report results through. | **[docs](docs/components/serial.md)** |
| **Joypad (Input)** | [`src/input.rs`](src/input.rs) | The 2×4 button matrix at `0xFF00` and the joypad interrupt. | **[docs](docs/components/input.md)** |
| **Frontends** | [`src/screen/`](src/screen) | Desktop (OpenGL), terminal (ratatui), web (canvas), and the C/Go FFI. | **[docs](docs/components/frontends.md)** |

```
frontend ──▶ Gameboy ──▶ CPU ──▶ MMU ──┬─▶ PPU   ─▶ framebuffer ─▶ frontend
   │                       │             ├─▶ MBC   (cartridge)
 keys                   interrupts       ├─▶ APU   (native)
                        (IF / IE)        ├─▶ Timer / Serial
                                         └─▶ Joypad
```

The **MMU is the hub**: the CPU only reads and writes addresses, and the MMU routes each to
a component. Components raise interrupts by setting a bit the MMU folds into `IF`; the CPU
dispatches them (priority VBlank → STAT → Timer → Serial → Joypad).

🗺️ **New here? Read the [code map](docs/codemap.md)** — one orientation page that traces a
frame through the system, follows a memory access from CPU → MMU → device, and shows where the
frontends attach, with every reference a clickable link into the source.

👉 **Start at the [component reference index](docs/components/README.md)**, read the
[accuracy-testing yardstick](docs/testing.md) that every component's test list is built on,
see [how a frame gets into a terminal](docs/rendering.md) — the transport ladder from
shared memory (81 bytes/frame) through indexed PNG, how the transport is negotiated, and
which terminals support what — and the
[terminal render-performance notes](docs/terminal-render.md) — why a large terminal
window used to cost frames and what caps the cost now — the
[telemetry schema](docs/telemetry.md) — machine-readable NDJSON run data (fps, per-stage
breakdown, emulated speed) for automated checks — the
[video-out guide](docs/video-out.md) — the picture as a raw frame stream for `ffmpeg`/OBS and as
a virtual webcam, with the exact commands — or the
[display-scaling design](docs/display.md) — how each frontend enlarges the 160×144
screen today and how to make it crisp (nearest/integer), smooth, or "vector-like",
or see the [debug-tooling design](docs/debugging.md) — how to inspect, step, and capture the
running machine (the deterministic frame-hashing that ties into the tests, plus a public
extension surface external tools can emit from, control, and plug into).

---

## 🔊 Audio

The Game Boy is authentically **4-bit, chiptune lo-fi** — two square waves, a
32-step wavetable, and an LFSR noise channel, band-limited through `blip_buf` and
played out via `cpal`. That grit is genuine DMG character and is meant to stay.

Sound is **opt-in** (`--features audio`) so a fresh clone needs no system audio
library. What the emulator produces is pitch-accurate: `blip_buf` resamples the
4.194304 MHz APU to whatever rate the device negotiates, so **a programmed 440 Hz
tone comes out at 440 Hz** (asserted by `cargo test --lib sound`).

A "rough and slightly fast" report (Pokémon Blue) traced to **three** issues, now
fixed:

| Symptom | Cause | Fix |
|---|---|---|
| Only one channel / odd rates | Device negotiation broke on the *first* `F32` config — often mono, or forced 96–384 kHz | Prefer the device's **stereo default** config; else scan for span-44100 + most channels |
| "Aged", laggy, skippy | Hand-off queue allowed up to **1 s** of latency before dumping it | A measured queue policy (prime / hold-and-fade / high-watermark wait); end-to-end latency later cut to **~53 ms** (a tighter device buffer + smaller blocks) — see [`docs/audio.md`](docs/audio.md) |
| **Slightly fast + rough** | Desktop paced at **16 ms = 62.5 fps** vs the real **59.73 fps** (**+4.64 %**) — the APU overproduced and the queue overflowed | Pace at the true **16.743 ms** frame period |

> The **fast** was the *emulator*, not the audio — audio was a symptom. Both
> frontends now pace on a **deadline accumulator** at the true 16.743 ms frame
> period (the terminal's old too-fast ~5 ms tick is fixed too), so the long-run
> rate is exactly 59.7275 Hz.

📖 **Full breakdown — the signal path, the negotiation logic, the pacing maths,
what's genuine DMG character, and how to measure it yourself — in
[`docs/audio.md`](docs/audio.md).** Probe your own machine's negotiated rate with
`cargo run --features audio --example audio_probe`.

<details>
<summary><strong>Memory map</strong> (click to expand)</summary>

```
0x0000–0x3FFF  ROM bank 0        0xE000–0xFDFF  Echo RAM
0x4000–0x7FFF  ROM bank N        0xFE00–0xFE9F  OAM (sprites)
0x8000–0x97FF  Tile RAM          0xFEA0–0xFEFF  Unused
0x9800–0x9FFF  BG map            0xFF00–0xFF7F  I/O registers
0xA000–0xBFFF  Cartridge RAM     0xFF80–0xFFFE  High RAM
0xC000–0xDFFF  Work RAM          0xFFFF         Interrupt Enable
```
![MMU map](resources/jsgb-mmu-map.png)

*(Image from [Imran Nazar — GameBoy Emulation in JavaScript](http://imrannazar.com/GameBoy-Emulation-in-JavaScript:-Memory).)*
</details>

---

## 🎥 Video out & virtual webcam

The picture can leave the emulator as a **raw frame stream** that any standard tool consumes —
`ffmpeg`, OBS, a v4l2 virtual camera. **Off by default**, and free when off.

```bash
# record a headless run straight into an MP4
cargo run --release --example video_out -- --rom game.gb --frames 600 \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - -pix_fmt yuv420p gb.mp4

# stream a game you are PLAYING in the terminal, through a FIFO (or press `v` in-game)
mkfifo /tmp/gb.raw
ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i /tmp/gb.raw gb.mp4 &
cargo run --release --example terminal -- game.gb --video-out /tmp/gb.raw

# a virtual webcam OBS/Zoom/a browser can pick up (needs the v4l2loopback module)
tools/virtual-camera.sh --name "POKEMON BLUE" --size 640x480
```

The emulator prints the exact consuming command for whatever settings are in force, so it is
never guesswork. Native 160×144 or the upscaled/palette-mapped picture, `rgba`/`rgb24`/`yuyv`,
letterboxed (never stretched) into any size, paced to wall clock so fast-forward drops frames
instead of producing a 4× speed video. **`v`** toggles it live and the Status panel says
`● LIVE`, so you always know whether you are streaming.

The virtual camera is **Linux-only** and needs the `v4l2loopback` kernel module, which needs
root to install; the emulator detects it and prints the exact install command rather than
failing obscurely, and `ffmpeg` is what feeds the device. Full guide, measured costs, platform
limits and the captured proof: **[`docs/video-out.md`](docs/video-out.md)**.

---

## Status

**Platforms**

| Frontend | State |
|---|---|
| Desktop (glutin/GL) | ✅ Working; audio with `--features audio` |
| Terminal (ratatui) | ✅ Working (kitty-protocol terminals); audio with `--features audio` |
| Web (WASM) | 🟡 Video + input work; **no audio yet**; non-MBC3 cartridges mis-detected |
| FFI (C/Go) | 🟡 Static lib + header build; the Go example takes a ROM path |

**Accuracy — hardware test ROMs**

A **headless Blargg/acid2 harness is now wired in** ([`testharness/`](testharness/), run
with `cargo test --release --test testroms`) and gated in CI against a checked-in
baseline. Full per-ROM breakdown: **[testharness/README.md](testharness/README.md)** ·
rationale: **[docs/testing.md](docs/testing.md)**.

| Suite | Result |
|---|---|
| Blargg `cpu_instrs` (11) | ✅ 11/11 pass |
| Blargg `instr_timing` | ✅ Pass |
| Blargg `mem_timing` / `mem_timing-2` | ✅ Pass (timer stepped per memory access) |
| Blargg `halt_bug` | ✅ Pass (HALT bug modelled) |
| `dmg-acid2` (PPU) | ✅ Pixel-correct vs reference |
| Blargg `dmg_sound` (12) | 🟡 9/12 pass (01–08 + 11); 09/10/12 (wave-RAM timing) need a T-cycle-stepped APU, **planned next** |
| Blargg `interrupt_time`, `oam_bug` | 🚫 Documented non-goals ([why](testharness/README.md#documented-non-goals-pinned-red-on-purpose)) |

**Blargg serial suite: 13/13.** APU accuracy is measured by a separate offline audio harness
(`cargo test --release --test audiocapture`) — see [`docs/audio.md`](docs/audio.md).

**Known limitations** (each is documented honestly on the relevant component page)
- No boot ROM / logo scroll; the post-boot register state is written directly.
- The DMG OAM-corruption bug is not modelled (deliberate non-goal); the HALT bug **is**.
- OAM DMA is instantaneous and does not restrict the bus; PPU mode 3 is fixed-length. Making
  these cycle-exact needs a cycle-stepped PPU/CPU — the same structural step the APU wave-RAM
  tests (`dmg_sound` 09/10/12) are waiting on; that APU work is now **planned next**.
- Web build has no sound and always treats cartridges as MBC3.
- Battery saves work only on the native (filepath) path — the web and FFI builds have no
  filesystem, so in-game saves are not persisted there. **Save states are implemented** in the
  core (versioned, ROM-checked `GBSS` format; byte-identical round-trip proof) and exposed in
  the terminal frontend as quick-save/-load slots — see [`docs/savestates.md`](docs/savestates.md).

**Battery saves** are written to `<rom>.sav` — **raw cartridge RAM with no header**, the
format every other emulator uses and the one real-cartridge dumpers produce, so saves can be
carried in either direction. For MBC3 cartridges that have a real-time clock, the clock is
appended to the same file as the standard 48-byte footer. `.srm` files are read too, as are
saves from older builds of this emulator (`<rom>.gbsave`), which are migrated automatically
with the old file left in place untouched as a backup. Saves are written **while the
emulator runs** — about a second after the game finishes writing cartridge RAM — and
atomically, so closing the window, a crash or a `kill` cannot lose or truncate them.

---

## Development

```bash
cargo build --release          # native; no ALSA needed (audio is off by default)
cargo test  --release --lib    # library unit tests (76 today)
cargo test  --release --test testroms      # headless accuracy harness (see testharness/)
make lint                      # rustfmt --check + clippy -D warnings
cargo build --release --target wasm32-unknown-unknown
cargo build --release --features audio     # host audio (needs ALSA dev headers on Linux)
```

A fresh clone builds and tests with **no system audio library** — `cpal` is an optional
dependency behind the `audio` feature. CI (GitHub Actions) still installs `libasound2-dev` so
it can exercise the `audio` path, then runs fmt, clippy, the test build and tests, the
test-ROM harness, and a desktop build on every push/PR.

---

## Roadmap & goals

Near-term, in priority order:

- **A T-cycle-stepped APU** — the next accuracy step, and the one the captain has asked for. It
  is what the remaining `dmg_sound` tests (09/10/12, the wave-RAM access-window quirks) need, and
  it shares a structure with a cycle-stepped PPU/CPU (which the PPU mode-3 and OAM-DMA non-goals
  are also waiting on). See [`docs/components/apu.md`](docs/components/apu.md).
- **Wire Mooneye GB** into the harness — the fine-grained timing suite that will turn our current
  approximations into visible, trackable reds. See [`docs/testing.md`](docs/testing.md).
- **Web audio** and correct **web/FFI MBC detection** (both currently forced to MBC3).

Bigger picture:

- 🧭 **A standalone Game Boy emulator conformance tool.** Once we pass the standard suites, turn
  this project's test harness into a tool anyone can point at *their own* emulator — to see what
  fails, and **why** — and eventually extract it into its own crate/repo. The design thinking
  (what it reports, how it attaches to a foreign emulator, what we already have, the extraction
  and licensing path, and why our own honest scores are the proof) is written up in
  **[`docs/conformance.md`](docs/conformance.md)**.

---

## Credits & lineage

This repository ([**Alchemy86/gameboy**](https://github.com/Alchemy86/gameboy)) is a fork of
**[raphamorim/gameboy](https://github.com/raphamorim/gameboy)** by Raphael Amorim, which is
itself closely modelled on **[mvdnes/rboy](https://github.com/mvdnes/rboy)**. It is licensed
**[MPL-2.0](LICENSE)**, the same licence as upstream — under which the original authorship
stays credited. The lineage:

> **[mvdnes/rboy](https://github.com/mvdnes/rboy)** (core emulation model)
> → **[raphamorim/gameboy](https://github.com/raphamorim/gameboy)** (Raphael Amorim — Rust +
> WebAssembly + FFI + terminal frontends)
> → **this fork** (accuracy work, save states, upscaler, audio pipeline, telemetry, terminal
> render path, test-ROM harness, and the per-component docs — see the
> [`CHANGELOG`](CHANGELOG.md)).

**References.** Hardware behaviour is grounded in [Pan Docs](https://gbdev.io/pandocs/),
[gbdev/awesome-gbdev](https://github.com/gbdev/awesome-gbdev), and
[rylev/DMG-01](https://rylev.github.io/DMG-01/); [alexcrichton/jba](https://github.com/alexcrichton/jba/tree/rust)
was a further emulation reference. The accuracy suites are Blargg, Gekkio's
[Mooneye GB](https://github.com/Gekkio/mooneye-gb), and Matt Currie's
[dmg-acid2/cgb-acid2](https://github.com/mattcurrie/dmg-acid2). Web CSS from
[Daniel Snow's Game Boy CSS](https://codepen.io/daniel-snows).

Want to help? See **[CONTRIBUTING.md](CONTRIBUTING.md)**.

<details>
<summary><strong>A bit of Game Boy history</strong> (the long version)</summary>

The Game Boy's CPU is a Sharp **LR35902** (core: **SM83**) — a custom hybrid that sits
between the Intel 8080 and the Zilog Z80, and is *not* itself a Z80 (see the
[CPU page](docs/components/cpu.md) for why the difference matters). It runs at 4.194304 MHz,
drives a 160×144 4-shade screen, and has four sound channels. Emulating it is mostly about
moving a lot of individual bytes: reading joypad state from I/O registers, pushing pixels
into VRAM, and banking through ROM/RAM.

<img src="resources/internals-cpu-section.gif" alt="Game Boy internals" width="360" />

*(c) Jeff Frohwein — [devrs.com](http://www.devrs.com/gb/hardware.php#hardgb). More
long-form history and the DMG board schematics live in [`resources/`](resources/).*
</details>

## License

MPL-2.0. See [`LICENSE`](LICENSE). Not affiliated with or endorsed by Nintendo.

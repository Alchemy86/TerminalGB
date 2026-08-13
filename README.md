# gameboy

**A Game Boy / Game Boy Color emulator written in Rust — runs on the desktop, in your terminal, and in the browser via WebAssembly.**

<p align="center">
  <img src="resources/demo-pokemon.gif" alt="Pokémon running in the browser build" width="480" />
</p>

| Web (WASM) | Desktop | Terminal |
|:---:|:---:|:---:|
| ![Web](resources/demo-web.png) | ![Desktop](resources/demo-desktop-1.png) | ![Terminal](resources/demo-terminal-1.png) |

> ⚠️ **Status: playable, work in progress.** Many commercial games boot and play. The CPU
> passes Blargg's `cpu_instrs`; several hardware edge-cases (the HALT bug, parts of CGB
> sound, cycle-exact timing) are known-inaccurate — see **[Status](#status)** and the
> honest per-component notes. This project is a study of emulation, not a piracy tool: it
> ships no ROMs and no BIOS.

---

## Quick start

Requires **Rust 1.83+** (pinned via [`rust-toolchain`](rust-toolchain)). Bring your own
`.gb`/`.gbc` ROM.

> **Linux:** all native builds currently need the ALSA development headers (the `cpal`
> audio dependency is not yet optional):
> `sudo apt install libasound2-dev` (Debian/Ubuntu) · `sudo dnf install alsa-lib-devel`
> (Fedora). This is required for *every* native build today — desktop, terminal, FFI, and
> `cargo test` — not just the audio one.

### 🖥️ Desktop (OpenGL window)

```bash
cargo run --release --example desktop
```

Controls: **Z** = Select · **X** = Start · **A** = A · **B** = B · **Arrow keys** = D-pad.

> ⚠️ **Today the example hardcodes a ROM path** at
> [`examples/desktop/bin.rs:9`](examples/desktop/bin.rs) (`// TODO: Allow receive path by
> arguments`). Until CLI-argument parsing lands, edit that line to point at your ROM. The
> Go FFI example ([`examples/ffi-go`](examples/ffi-go)) already takes a path argument.

### ⌨️ Terminal

```bash
cargo run --release --example terminal
```

> Needs a terminal that supports the **kitty keyboard protocol** (kitty, WezTerm, Rio,
> foot); elsewhere it prints a warning and exits
> ([`src/screen/tui.rs:46`](src/screen/tui.rs)). Same hardcoded-path caveat as desktop
> ([`examples/terminal/bin.rs:8`](examples/terminal/bin.rs)). No audio.

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
| **APU (Sound)** | [`src/sound.rs`](src/sound.rs) | Four channels (2 square + wave + noise), the 512 Hz frame sequencer, `blip_buf` → `cpal`. *Native only.* | **[docs](docs/components/apu.md)** |
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

👉 **Start at the [component reference index](docs/components/README.md)**, read the
[accuracy-testing yardstick](docs/testing.md) that every component's test list is built on,
or see the [debug-tooling design](docs/debugging.md) — how to inspect, step, and capture the
running machine (the deterministic frame-hashing that ties into the tests, plus a public
extension surface external tools can emit from, control, and plug into).

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

## Status

**Platforms**

| Frontend | State |
|---|---|
| Desktop (glutin/GL) | ✅ Working, with audio |
| Terminal (ratatui) | ✅ Working (kitty-protocol terminals); no audio |
| Web (WASM) | 🟡 Video + input work; **no audio yet**; non-MBC3 cartridges mis-detected |
| FFI (C/Go) | 🟡 Static lib + header build; the Go example takes a ROM path |

**Accuracy — hardware test ROMs**

Today these are **manual OpenEmu screenshots** ([`resources/tests/`](resources/tests)), not
an automated harness. The [`retrio/gb-test-roms`](https://github.com/retrio/gb-test-roms)
submodule is declared in [`.gitmodules`](.gitmodules) but **uninitialised**. Wiring a
headless Blargg/Mooneye/acid2 harness is the highest-value next step — see
**[docs/testing.md](docs/testing.md)**.

| Test | Result |
|---|---|
| Blargg `cpu_instrs` | ✅ Pass |
| Blargg `instr_timing` / `mem_timing` | ⚠️ Not yet verified |
| Blargg `bits-unused`, `bits-bank1` | ❌ Failing |
| Blargg `halt_bug` | ❌ Failing (the HALT bug is not modelled) |
| Blargg `cgb_sound` | ❌ Failing |

**Known limitations** (each is documented honestly on the relevant component page)
- No boot ROM / logo scroll; the post-boot register state is written directly.
- The HALT bug and DMG OAM-corruption bug are not modelled.
- OAM DMA is instantaneous and does not restrict the bus; PPU mode 3 is fixed-length.
- Web build has no sound and always treats cartridges as MBC3.
- Save states are not implemented; battery saves work only for the native (filepath) path.

---

## Development

```bash
cargo build --release          # native (needs ALSA dev headers on Linux)
cargo test  --release          # unit tests (4 today — see docs/testing.md)
make lint                      # rustfmt --check + clippy -D warnings
cargo build --release --target wasm32-unknown-unknown
```

CI (GitHub Actions) installs `libasound2-dev`, then runs fmt, clippy, the test build and
tests, and a desktop build on every push/PR.

---

## Credits & references

Emulation logic is heavily indebted to **[mvdnes/rboy](https://github.com/mvdnes/rboy)** and
**[alexcrichton/jba](https://github.com/alexcrichton/jba/tree/rust)**. Hardware behaviour is
grounded in [Pan Docs](https://gbdev.io/pandocs/), [gbdev/awesome-gbdev](https://github.com/gbdev/awesome-gbdev),
and [rylev/DMG-01](https://rylev.github.io/DMG-01/); the accuracy suites are Blargg, Gekkio's
[Mooneye GB](https://github.com/Gekkio/mooneye-gb), and Matt Currie's
[dmg-acid2/cgb-acid2](https://github.com/mattcurrie/dmg-acid2). Web CSS from
[Daniel Snow's Game Boy CSS](https://codepen.io/daniel-snows). Original project by
[raphamorim](https://github.com/raphamorim/gameboy).

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

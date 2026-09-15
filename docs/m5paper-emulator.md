# Pokémon Blue on the M5Stack PaperColor

**The emulator runs on the device. A 1 MiB cartridge lives in flash and costs no
RAM at all, a frame takes 81 ms to emulate and 15.953 s to draw, and the picture
on the paper is TerminalGB's own core — not a port of somebody else's.**

[First boot](m5paper-first-boot.md) proved the toolchain, the panel and the
buttons and deliberately ran no emulator. This is the step it was written for.

## What is measured

Everything below was read off the captain's own device (MAC `28:84:85:43:d2:84`)
over USB serial, not estimated. Reproduce it with:

```sh
espup install --targets esp32s3                       # once
TERMINALGB_ROM=~/roms/blue.gb tools/m5paper-build.sh flash
```

With no `TERMINALGB_ROM` the build uses the MIT dmg-acid2 image in
`third_party/`, which is the one cartridge this project may redistribute — so a
fresh clone builds, flashes and shows a picture with no game at all.

| | Pokémon Blue | dmg-acid2 |
|---|---:|---:|
| cartridge | 1024 KiB | 32 KiB |
| in RAM | **0 bytes** | **0 bytes** |
| mapper | `$13` (MBC3+RAM+BATTERY) | `$00` |
| heap after construction | 301 KiB | 269 KiB |
| emulation | 1000 frames in 81.051 s — **12 fps** | 30 frames in 2.761 s — 10 fps |
| against a real Game Boy | **20 %** | 18 % |
| one frame onto the panel | **15.953 s** | 15.953 s |

The panel figure agrees with bring-up's 15.967 s to eleven milliseconds, which
is the point: nothing about running an emulator changed what the glass costs.

## The cartridge is in flash and costs no RAM

`include_bytes!` puts the ROM in `.rodata`, and the ESP32-S3 memory-maps that
out of flash. The `embedded` cargo feature makes the emulator's mappers hold a
`&'static [u8]` instead of an `Arc<[u8]>`, so the megabyte is simply *read where
it already is*:

```
size      : 1024 KiB, in flash (.rodata) — no RAM copy
```

That is worth more than the megabyte it saves. The feasibility scout's third
named risk was a Rust-atomics hazard on this chip's external PSRAM, reached
through the reference count of a shared `Arc` cartridge. With the cartridge
borrowed rather than counted there is no reference count anywhere, so the hazard
is **absent rather than avoided**, and no argument about task counts is needed.

## Four greys onto six inks, and why the device picks

The panel has black, white, yellow, red, blue and green, and **no grey at all**.
A DMG draws in four greys. First boot left that open — "it needs frames on the
glass, not reasoning" — and this does not pretend to have settled it from a
desk. It ships five looks and **button C cycles them**, repainting the same
frame so they can be compared by eye:

| | |
|---|---|
| `dither` (default) | no ink for the middle shades — a 2×2 cell of black and white per Game Boy pixel, which is the only look here that makes apparent greys on a panel that has none |
| `red/green` | black, red, green, white |
| `green/yellow` | black, green, yellow, white |
| `blue/green` | black, blue, green, white |
| `red/yellow` | black, red, yellow, white |

What is *not* left to taste is checked instead, in
`platform/m5paper/src/gbscreen.rs`, which carries no `esp-hal` dependency so
`tools/m5paper-build.sh test` compiles the very code the firmware runs. Eight
tests: every ramp is monotonic in luminance (a darker Game Boy shade drawn
lighter than a lighter one inverts part of the picture, and on a six-ink panel
that is easy to do by accident because hue is loud and luminance is not); the
four DMG greys rank 0-3 and a *coloured* palette still ranks the same way; the
dither cell reaches both ends and gives four distinct tones; the picture lands
centred, exactly ×2, the right way up — and **boxed**, because the test card
already learned that white ink on white paper is invisible without an edge.

×2 is the largest whole number that fits: 160×3 = 480 fits across a 600-wide
screen but 144×3 = 432 does not fit down 400. A fractional scale is not the
alternative — a Game Boy draws its shading as a one-pixel checkerboard, and
resampling that at a non-integer ratio is moiré, which is the same finding
`chainrender`'s camera rule 8 records for the swarm films.

## Where the RAM went, which is the whole story

This is the part that cost the day, and every number in it came from a fault
report rather than a calculation.

An app on this chip gets about **335 KiB** of internal DRAM, and esp-hal gives
the main stack whatever `.data` and `.bss` leave. Three things wanted it:

| | |
|---|---|
| panel framebuffer | 120,000 (a static, 4bpp, 400×600) |
| emulator heap | ~115,000 (the PPU's RGBA frame is 92,160 of it) |
| **constructing a `Gameboy`** | **~322,928** |

That third number is not a typo and it is measured: the stack guard printed
`sp=0x3fc8cb90` against `top=0x3fcdb700`, which is a 322,928-byte prologue, and
it fired before `esp_hal::init` could print a word. Every constructor in the
chain returns a large value by value — `Gameboy` alone is 51,296 bytes — and
with fat LTO each level's temporary lands in one merged frame.

Two things followed:

**The heap moved to PSRAM, and so did the panel buffer.** First boot kept that
buffer in internal SRAM deliberately, and this port cannot: with it and the
machine both inside, the stack was 154,824 bytes for a 317,000-byte frame. Out
there, internal SRAM is almost entirely stack (325,784 bytes) and construction
fits.

**`Box::new([0u8; 92160])` had to stop building on the stack.** The second fault
was an exception inside a `memcpy` whose length register read `92160` — the PPU
framebuffer, to the byte. `Gpu::blank_framebuffer` goes through `vec!` instead,
which reaches `alloc_zeroed`, so nothing is copied at all. On a host it is the
same allocation it always was; `docs/throughput.md` measured that difference at
the quantisation floor.

**What was tried and did not work**, so nobody repeats it: `#[inline(never)]` on
every constructor in the chain (`Gameboy`, `Cpu`, `MemoryManagementUnit::raw`,
`Gpu`, `Sound`) still needed more than the 268 KiB left once 51 KiB of internal
SRAM is reserved for the machine — and it made things *worse* rather than
better, because out of line each nested frame keeps its own copy where LLVM
could previously coalesce them. Turning LTO off did not help either. The machine
therefore lives on the PSRAM heap, which is the one real compromise here: its
VRAM, WRAM, HRAM and OAM are touched several times per emulated instruction and
they are on an external bus.

## The clock was most of the speed

`esp_hal::Config::default()` does not run this chip at 240 MHz. Setting
`CpuClock::max()` took dmg-acid2 from **3 fps to 10 fps** — measured, same
binary otherwise — and is the single largest change in this work. Anyone
porting the core to an esp-hal target should set it before measuring anything.

PSRAM at 80 MHz instead of its 40 MHz default was tried in the same pass and
**hung the device**; it is left at the default and is not recommended without
the flash/PSRAM mode combinations Espressif documents.

## What this does not show

**Nobody plays a game on this.** Twelve emulated frames a second is a fifth of
real time, and each picture then takes sixteen seconds. Pressing START and
seeing what happened is a twenty-two-second round trip. The interesting build
remains the one where no human is holding it.

**The picture has not been photographed**, and the shade mapping has not been
chosen. Both are one person in the room away: the frames are on the glass and
button C cycles the looks.

**The emulation was not held to any accuracy suite on this device.** Nothing in
`testharness/` runs on an ESP32-S3, and the frame that reaches the panel is not
compared against a reference. What *is* checked is that the `embedded` feature
changes nothing on a host: `cargo test --release --lib` is 915 passed, 0 failed,
exactly as before.

**The machine is in PSRAM and should not be.** ~12 fps is an external-bus
figure; the same core in internal SRAM would be several times faster. Closing
that needs the construction peak brought under 268 KiB, which means the core
constructing in place rather than returning 51 KiB by value — a real change to
`Gameboy::try_new_with_model` and everything under it, not a flag.

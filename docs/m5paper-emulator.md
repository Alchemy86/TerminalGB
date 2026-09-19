# Pokémon Blue on the M5Stack PaperColor

**The emulator runs on the device. A 1 MiB cartridge lives in flash and costs no
RAM at all, a frame takes 81 ms to emulate and 15.953 s to draw, and the picture
on the paper is TerminalGB's own core — not a port of somebody else's.**

[First boot](m5paper-first-boot.md) proved the toolchain, the panel and the
buttons and deliberately ran no emulator. This is the step it was written for.

## What is measured

Everything below was read off the maintainer's own device (MAC `28:84:85:43:d2:84`)
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

## The game runs on its own, and "every second" is not on offer

The ask was *"our game though is not playing in the background.. we press a
button we get a part of the next thing. Can we not just it continue to refresh
the screen every second."*

Half of that is a defect and half of it is not available.

The defect: the loop only emulated and only repainted **on a button's rising
edge**. Between presses the machine sat idle with a stale picture on the glass,
which is not a Game Boy that happens to be slow — it is a Game Boy that is
stopped. That is fixed. The firmware now runs a batch of frames, paints, and
does it again, for ever, with no button involved.

Not available: one second. The panel is measured at **7.72 s a repaint** at the
fastest frame-rate setting that still separates the inks, and
[`m5paper-refresh.md`](m5paper-refresh.md) is the sweep that says so — the
waveform is how long the pigment gets to move, not a number anybody here can
choose. Nine other routes to a shorter update were tried and measured identical.
A repaint every second is off by a factor of eight before a single frame is
emulated.

### The measured cadence

25 consecutive free-running turns, no button touched, across two runs of the
shipped firmware on the maintainer's device:

| | min | median | max |
|---|---:|---:|---:|
| emulating 60 frames | 4.710 s | **4.933 s** | 6.554 s |
| the panel | 7.723 s | **7.726 s** | 7.727 s |
| **one whole turn** | 12.844 s | **13.027 s** | 14.686 s |

So: **a fresh picture every 13.0 seconds, each one 1.005 s of game time after
the last.** The panel is 59.3 % of that and does not vary by more than four
milliseconds; the emulator is the part that wobbles, because how long 60 frames
take depends on what the game is doing in them.

The game therefore advances at **7.7 % of real time**. That is the honest
headline and it is worth stating next to the 12 fps above: the emulator alone is
20 % of real time, and sharing the CPU with a panel that wants eight seconds a
picture takes it to under eight.

`STEP_FRAMES` stays at 60 for a reason that survives being asked about: 60
frames is 1.005 s of Game Boy time, so every picture is one game-second after
the one before it, and the slideshow plays at a constant, explainable interval.
Lowering it repaints sooner and shows less happening; raising it does the
reverse. The floor is **~8.1 s** — the 7.72 s of waveform plus ~0.37 s of SPI,
blit and the one frame `show` runs — and no value of `STEP_FRAMES` goes below
it.

### A button is sampled in both places the loop spends time

A press that nothing looked at never happened. At these timings that is the
whole problem: a turn is thirteen seconds, a human press is two or three hundred
milliseconds, and a loop that only checked the pins between turns would miss
almost all of them.

So the buttons are sampled in both of the places a turn spends time — between
every emulated frame, and **inside the panel driver's own wait loop**, which is
why `Ed2208::refresh_auto` takes a closure. Measured, per turn:

| | samples | interval |
|---|---:|---|
| through the frame batch | 60 | ~82 ms |
| through the repaint | **7,510** | **~1.03 ms** |

That leaves **0.368 s of a 13.0 s turn — 2.8 % — with nothing watching**: the
SPI write of 120,000 bytes, the blit, and the settle either side, none of which
has a wait loop to hang a poll on.

A press is *latched*, never acted on where it is seen: the sample that catches
it is usually inside a bus transaction that cannot repaint anything. It is spent
at the top of the next turn, and appears in that turn's repaint.

### How the press path was actually checked

Nobody was in the room, so this could not be confirmed by pressing a button. It
was confirmed by standing in for one: a temporary build set the latch from
inside the panel's own wait loop — poll #2000 of 7,510, four seconds into a
waveform, a moment a press could not have survived before this change — and the
serial log shows what followed.

```
  [test] synthetic A press, mid-waveform, poll #2000
cycle    3  frame    1180  emu 4.713 s  panel 7.726 s  turn 12.848 s  (dither, PLL 0x07)
button A -> Start
cycle    4  frame    1252  emu 4.918 s  panel 7.727 s  turn 14.192 s  (dither, PLL 0x07)
```

The frame count goes 1180 → 1252, which is 60 for the batch and 12 for
`PRESS_FRAMES`: the press was caught mid-repaint, held, and spent on the next
turn. A synthetic C did the same and produced the preview behaviour below.

**What this does not prove is the pin itself.** `buttons[i].2.is_low()` is
unchanged from the build that was confirmed by hand at first boot, and the
instrumentation is not committed. A person with the device in their hands is
still the only check on the physical button, and it is one press long.

### Button C holds the frame

Button C is a palette preview: it exists to show what the six inks look like. So
a C turn runs **no frames at all** — the picture that is already on the glass
goes back on, slowly, at the vendor's `0x08` waveform — and the game carries on
from exactly where it was on the turn after. A frame that had also moved on
would be changing two things at once, and then the picture answers neither
question. It is the same argument the slow waveform is there for.

Measured: a C turn is `emu 0.000 s, panel 14.939 s`, with the frame counter
standing still and the look advanced. A pad press that arrives on a preview turn
stays latched rather than being spent, for the same reason.

## What this does not show

**Nobody plays a game on this.** Twelve emulated frames a second is a fifth of
real time, and each picture then takes eight seconds — sixteen, before
[`m5paper-refresh.md`](m5paper-refresh.md) found the frame-rate register the
vendor leaves at its slowest and the second it spends sequencing by hand.
Pressing START and seeing what happened is a thirteen-second round trip. What
the device is good at is the thing above: running unattended and putting a fresh
picture up whenever the paper is ready for one.

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

# TerminalGB on the M5Stack PaperColor

```sh
espup install --targets esp32s3     # once; upstream rustc cannot target Xtensa
tools/m5paper-build.sh backup       # ONCE, before you overwrite the factory image
TERMINALGB_ROM=~/roms/blue.gb tools/m5paper-build.sh flash
```

**This runs the emulator.** The same `terminalgb` core the desktop and terminal
frontends use, built `#![no_std]` through its `embedded` cargo feature, with the
cartridge in flash and the picture on E Ink.
[`docs/m5paper-emulator.md`](../../docs/m5paper-emulator.md) has the measured
results; [`docs/m5paper-first-boot.md`](../../docs/m5paper-first-boot.md) is the
bring-up that came before it. What follows is how it works.

## What it does

Boots, powers the panel, builds a machine from the cartridge compiled into
flash, emulates a thousand frames as fast as the chip can, and puts one of them
on the paper. Then three buttons: **A** presses Game Boy START, **B** presses
Game Boy A, and **C** cycles the shade-to-ink mapping and repaints the same
frame so the five looks can be compared by eye.

Each gameplay repaint takes about **eight** seconds and the look preview fifteen.
Two things got it there: the panel's frame-rate register, which the vendor's init
leaves at its slowest value, and handing the power sequence to the controller
instead of driving it by hand. The look preview keeps the vendor's settings
because judging a palette wants true colour.
[`docs/m5paper-refresh.md`](../../docs/m5paper-refresh.md) has both, the seven
things that measured identical — a black-and-white waveform among them — and the
one check nobody here can make.

## The other thing the panel is for

```sh
pixelgb species --rom ~/roms/blue.gb --out /tmp/species
TERMINALGB_SPECIES_DIR=/tmp/species/pokemon-blue/sprites/four-shade/1x \
  tools/m5paper-build.sh pokedex
```

A separate binary, and a still: every species' own front sprite out of the
cartridge, magnified whole-number sizes, held on the glass one at a time with
the power off. **C** advances to the next Pokédex entry, **A** goes back, **B**
cycles the look. It repaints at the **vendor's** slow PLL rather than the
emulator's fast one, because fidelity is the whole feature there and seconds
are not. [`docs/m5paper-pokedex.md`](../../docs/m5paper-pokedex.md) has the
verification — 0 of 3,136 pixels against `pret/pokered`'s own source art for
Charizard, the sample it was checked against — and is plain that the colour is
a recolour, since Gen 1 had none.

## The cartridge

`TERMINALGB_ROM` is an absolute path, read at **build** time and compiled into
`.rodata`, which the ESP32-S3 memory-maps out of flash — so a 1 MiB cartridge
costs no RAM at all. **No ROM is committed to this repository**, so the default
is the MIT dmg-acid2 image in `third_party/` and a fresh clone builds, flashes
and shows a picture with no game.

## The panel is faster than the vendor ships it

```sh
tools/m5paper-build.sh probe        # ~7 min, times everything, leaves a test card
```

`0x30` is a PLL divider that sets how fast the waveform is clocked, and M5GFX —
like Pimoroni and Seeed — leaves it at the default. Measured here, `POWER_ON`
through `POWER_OFF`: **15.954 s at the vendor's `0x08`, 8.741 s at `0x07`**, and
`0x08` lands on `0x03` to two milliseconds because the field is only four bits
wide. Command `0x17` (AUTO) then takes **another 1012 ms** by letting the
controller sequence its own power-up instead of doing it by hand with fixed
delays; it does not touch the waveform.

Everything else measured identical: the addressing window `0x83` that GxEPD2
calls partial refresh, the `DISPLAY_REFRESH` parameter byte, a forced
temperature, and — with the captain's explicit authorisation, twice — the
`LUT_SEL` bit that would have exposed a black-and-white waveform if this
controller had one. It does not.

**What no timer here can check is whether the faster picture is still right.** A
truncated waveform loses ink separation rather than time, and this board has no
MISO to read anything back. The probe ends by leaving the six-ink test card on
the glass for that reason; compare it against `tools/m5paper-build.sh testcard`.
If it is muddy, `PLAY_PLL` in `src/main.rs` is the one constant to move.

## Two things a porter should know before measuring anything

**`esp_hal::Config::default()` does not run this chip at 240 MHz.** Setting
`CpuClock::max()` took the emulator from 3 fps to 10 — the largest single change
in this port, and it looks like nothing.

**The heap is PSRAM and the panel framebuffer is out there too.** Constructing a
`Gameboy` needs about 322,928 bytes of stack (measured, from the stack guard's
own `sp`), because every constructor in the chain returns a large value by value
and fat LTO merges the temporaries into one frame. An app gets ~335 KiB of
internal DRAM in total, and esp-hal gives the stack whatever `.data` and `.bss`
leave. The full account, including what was tried and did not work, is in
[`docs/m5paper-emulator.md`](../../docs/m5paper-emulator.md).

## The three things that were not obvious

**The panel has no power-enable GPIO.** It is switched by a separate M5PM1 PMIC
over I²C at address `0x6E` (SDA `GPIO3`, SCL `GPIO2`). Until that chip is told to
raise *its* GPIO0, the panel is unpowered and every SPI command is written into
the void with no error of any kind. `src/pmic.rs`.

**`GPIO43` is data/command, not reset.** M5GFX's panel config carries
`pin_rst = 43`, which reads like a contradiction with the bus's `pin_dc = 43`
until you notice `Panel_ED2208::init` passes `use_reset = false` to its base
class — so that field is never used. The real reset line is `GPIO12`.

**`GPIO1` rises much more slowly than `GPIO9`/`GPIO10`.** Released, it takes
milliseconds to climb through the internal pull-up rather than microseconds,
which is the signature of a hardware debounce capacitor on that button. A 500 µs
settle read it as still-held and looked exactly like a broken pin.

## Pinout

Every number here was read out of M5GFX's own `board_M5PaperColor` bring-up, not
from a schematic photo.

| | |
|---|---|
| Panel | 400×600 E Ink Spectra 6, controller ED2208-DOA |
| SPI | SCLK `GPIO15`, MOSI `GPIO13`, 4 MHz, mode 0, SPI2 |
| CS / DC / RST / BUSY | `GPIO44` / `GPIO43` / `GPIO12` / `GPIO11` |
| PMIC | M5PM1 @ `0x6E`, SDA `GPIO3`, SCL `GPIO2` |
| Buttons | A `GPIO10`, B `GPIO9`, C `GPIO1` — active low |

`BUSY` reads low while the controller works and high when it is ready.

## Colour and orientation

Six inks and **no grey**: black `0x0`, white `0x1`, yellow `0x2`, red `0x3`,
blue `0x5`, green `0x6`. Codes `0x4` and `0x7` are unused by the controller. The
framebuffer is 4bpp, two pixels per byte, 120,000 bytes — internal SRAM, never
PSRAM, so the `s32c1i`-on-PSRAM atomics hazard cannot be in play here.

The glass is portrait. Landscape is a coordinate mapping applied on the way in
(`Rotation` in `src/geom.rs`), and since nobody on the development end can see
the panel, that mapping is **tested rather than asserted**:

```sh
tools/m5paper-build.sh test        # 67 tests, no device needed
tools/m5paper-build.sh testcard    # render what the panel should show
```

`src/gbscreen.rs` is held to the same standard for the same reason — nobody on
the development end can see the paper, so the Game Boy picture's placement, its
orientation, its box and every shade-to-ink ramp are checked rather than
claimed. A ramp that is not monotonic in luminance inverts part of the picture,
and on a six-ink panel that is easy to do by accident.

`src/geom.rs` is deliberately free of any `esp-hal` dependency so `rustc --test`
can compile the very file the firmware runs. The tests check that the logical
top-left lands on the panel's top-right, that the mapping is a bijection in every
rotation, that neighbouring pixels do not clobber each other in the shared byte,
and that the test card really contains all six inks — the white band is invisible
against white paper unless it is boxed, and an unboxed card shows five inks while
claiming six.

## Layout

| | |
|---|---|
| `src/geom.rs` | inks, rotation, framebuffer, the test card, and the host tests |
| `src/panel.rs` | the ED2208 wire protocol: init list, transfer, refresh |
| `src/pmic.rs` | the M5PM1 sequence that powers the panel |
| `src/gbscreen.rs` | four Game Boy shades onto six inks, the ×2 blit, and its host tests |
| `src/main.rs` | power, init, the machine, the emulation burst, the button loop |
| `src/sprite.rs` | a species picture onto the panel: magnification, layout, inks, and its host tests |
| `src/font.rs` | a 5x7 font for the caption, and why it is not `video::card`'s |
| `src/sprite_host.rs` | PNG to the packed blob the firmware reads, and the page preview |
| `src/pokedex.rs` | the archive directory over many sprite blobs, and its host tests |
| `src/pokedex_host.rs` | a whole `pixelgb species` directory to one archive file |
| `src/bin/pokedex.rs` | the Pokédex browser, as its own firmware |
| `terminalgb-rlib/` | one manifest, no source: the emulator as an rlib only — see its own comment |
| `src/testcard_host.rs` | renders the expected picture on the host |

This crate is **not a workspace member and must never become one**, for the same
reason `platform/psp/` is not: it needs a toolchain the emulator's own build must
never depend on. `make embed` and `cargo test` do not see it. It declares its own
empty `[workspace]` so cargo does not adopt it.

## Restoring the factory firmware

There is no secure boot and no flash encryption on this device, so a full image
restores exactly:

```sh
esptool --port /dev/ttyACM0 write-flash 0x0 papercolor-factory-16mb.bin
```

## What this does not show

**Nobody plays a game on this**, and the numbers say why: twelve emulated frames
a second is a fifth of real time, and each picture then takes sixteen seconds.
The picture has also not been photographed and the shade mapping has not been
chosen — both are one person in the room away. See
[`docs/m5paper-emulator.md`](../../docs/m5paper-emulator.md)'s closing section
for the honest list.

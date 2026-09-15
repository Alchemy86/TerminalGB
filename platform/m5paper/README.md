# TerminalGB on the M5Stack PaperColor — first boot

```sh
espup install --targets esp32s3     # once; upstream rustc cannot target Xtensa
tools/m5paper-build.sh backup       # ONCE, before you overwrite the factory image
tools/m5paper-build.sh flash
```

This is **not** an emulator port. It is the step before one: a bring-up binary
that proves the toolchain, the panel and the buttons on the real device, and
touches no TerminalGB code at all.

[`docs/m5paper-first-boot.md`](../../docs/m5paper-first-boot.md) has the measured
results and the serial logs. What follows is how it works.

## What it does

Boots, powers the panel, paints a solid colour, paints a landscape test card,
then repaints in the next ink every time you press a button. Each repaint takes
about sixteen seconds, which is the panel, not the software.

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
tools/m5paper-build.sh test        # 9 tests, no device needed
tools/m5paper-build.sh testcard    # render what the panel should show
```

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
| `src/main.rs` | bring-up: power, init, two refreshes, the button loop |
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

Nothing here runs the emulator. The next step, and the reason the refresh time
above matters, is in
[`docs/m5paper-first-boot.md`](../../docs/m5paper-first-boot.md).

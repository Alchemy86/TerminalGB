# First boot on the M5Stack PaperColor

**Custom Rust firmware runs on the device, drives the e-paper panel, and reads
its buttons. The panel takes 15.967 seconds to draw one picture — measured from
our own code, on the captain's own device, not quoted from anywhere.**

That number is the whole story of what this hardware can and cannot be, and it
is the reason this page exists before any emulator port does. A Game Boy frame
lasts 16.743 ms. This panel is **954 times slower than that**, and no software
moves it.

The feasibility scout measured 16.280 s from M5Stack's own factory firmware and
predicted that custom firmware would see the same. It does, to within two per
cent, and the small difference is explained below rather than waved at.

## What was proven, and how

Four things, in order, each on the real device at `/dev/ttyACM0`:

| | proven by |
|---|---|
| The forked Xtensa toolchain builds this | `espup install --targets esp32s3`, then a clean `cargo build --release` |
| A Rust binary boots and talks | the serial log below, over USB Serial/JTAG |
| Rust drives this exact panel | two full refreshes, timed by the firmware itself |
| The buttons read back | two physical presses, on two different GPIOs, each repainting the panel |

Reproduce all of it with `tools/m5paper-build.sh`. The wiring, the three
non-obvious hardware facts and the code layout are in
[`platform/m5paper/README.md`](../platform/m5paper/README.md).

## The serial log

Verbatim, from a clean reset:

```
=== TerminalGB / M5PaperColor first boot ===
chip      : ESP32-S3, Xtensa LX7
built with: espup Xtensa Rust fork (upstream rustc cannot target this)
panel     : 400x600 native, 600x400 logical (Landscape)
pmic      : enabling panel power (M5PM1 @ 0x6E)... ok
panel     : reset + init sequence... ok (BUSY responded)

--- refresh 1: solid red ---
solid    red: 15.968 s

--- refresh 2: landscape test card ---
test card  : 15.967 s

--- buttons ---
  A = GPIO10 idle level 1 (1 = released)
  B = GPIO9  idle level 1 (1 = released)
  C = GPIO1  idle level 1 (1 = released)
read-path self-test: ok (all three read both levels)
idle levels after self-test: A=1 B=1 C=1
press a button: the panel repaints in the next ink.
button A (GPIO10) pressed -> repainting black
   black: 15.967 s
```

Those last two lines are a human finger on the device. A separate run caught
button C as well:

```
button A (GPIO10) pressed -> repainting black
   black: 15.967 s
button C (GPIO1) pressed -> repainting white
   white: 15.967 s
```

## The refresh time

Four different pictures, one device, every refresh timed by the firmware itself:

| picture | measured |
|---|---|
| solid red | 15.968 s |
| landscape test card (6 inks) | 15.967 s |
| solid black (button A) | 15.967 s |
| solid white (button C) | 15.967 s |

**It does not vary with the picture.** Across every sample taken — sixteen
refreshes over five firmware builds — the spread is **15.957 s to 15.968 s**: 11 ms on
sixteen seconds, or 0.07 %. A flat fill and a six-colour card both land inside
that band, so the Spectra 6 waveform is fixed-length rather than
content-dependent. M5Stack's "approximately 15~30 s, depending on the
complexity of the colour distribution" is not what this device does; the scout's
three factory-firmware samples (16.280, 16.280, 17.380 s) already hinted at that,
and sixteen from our own code settle it.

**Sixteen seconds is the vendor's setting, though, not the panel's floor — found
later.** The waveform is clocked by a PLL divider the init leaves at its slowest
value, and the same refresh takes **8.741 s** one step faster.
[`m5paper-refresh.md`](m5paper-refresh.md) has the sweep and the two things that
looked like they should have helped and did not. Every number on this page is at
the vendor default, which is what this firmware shipped when they were taken.

Our figure is 0.3 s under the factory firmware's because it brackets a slightly
different window: the firmware's log spans its own "Refresh start" to "Refresh
complete", ours spans `POWER_ON` through `DISPLAY_REFRESH` to `POWER_OFF`. The
agreement is the point, not the gap.

**Against a 16.743 ms Game Boy frame: 0.0626 fps, 954x too slow.** The scout said
972x from the factory number. Nothing here rescues that, and nothing was expected
to.

## Landscape

The captain asked for landscape. The glass is physically portrait — 400 wide by
600 tall — so landscape is a coordinate mapping, and the logical screen is
600x400.

Nobody on the development end can see the panel, so that mapping is **checked,
not claimed**. `platform/m5paper/src/geom.rs` carries no `esp-hal` dependency,
which lets `rustc --test` compile the very file the firmware runs:

```
$ tools/m5paper-build.sh test
running 9 tests
...
test result: ok. 9 passed; 0 failed
```

The tests pin that the logical top-left lands on the panel's top-*right*, that
the mapping is a bijection in every rotation (a rotation that collides silently
loses half the picture), that neighbouring pixels do not clobber each other in
their shared byte, and that the test card contains all six inks. They have teeth:
replacing the rotation with a transpose — the classic mistake, and one that still
*looks* landscape — fails two of them, and reporting portrait dimensions fails
three.

`tools/m5paper-build.sh testcard` renders what the panel should show, from that
same code, so the picture on the glass can be compared against something other
than an assertion.

## What the panel can show

Six inks and **no grey at all**: black, white, yellow, red, blue, green. The
Game Boy draws in four greys, so the four-shades-into-six-colours question is
real and is not answered here — it needs frames on the glass, not reasoning.

The test card boxes each band in black on purpose. White ink on white paper is
invisible without it, and an unboxed card shows five inks while claiming six.
That is a test, not a comment.

## Three things the hardware does that the documentation does not say

**The panel has no power-enable GPIO.** It is gated by a separate M5PM1 PMIC over
I²C. Until that chip raises its own GPIO0 the panel is unpowered and every SPI
write is discarded with no error — the failure mode is silence, which is the
worst kind to debug. Six register writes, in `platform/m5paper/src/pmic.rs`.

**`GPIO43` is data/command, not reset.** M5GFX's panel config sets
`pin_rst = 43`, contradicting the bus's `pin_dc = 43`, until you find that
`Panel_ED2208::init` passes `use_reset = false` and never uses the field. The
real reset is `GPIO12`.

**`GPIO1` rises milliseconds slower than `GPIO9` and `GPIO10`** through the
internal pull-up — a hardware debounce capacitor on that button. A 500 µs settle
read it as still-held, which looked exactly like a dead pin and was not.

## An instrument that lied, and what it cost

The first button self-test reported `C FAILED` and then immediately detected a
press nobody had made. Neither was the device.

`OutputConfig::default()` carries `Pull::None`, so applying it to drive a pin low
silently dropped the pull-up that had been set. All three pins were left
floating, and a floating pin reads as a permanently-held button. The fix is to
re-apply the input config afterwards, and to print the idle levels **again** after
the self-test so a disturbed pin is visible rather than inferred:

```
read-path self-test: ok (all three read both levels)
idle levels after self-test: A=1 B=1 C=1
```

If those two lines disagree with the idle levels printed above them, nothing
below is trustworthy. The self-test proves the *read path* only — that the GPIO
can be read and that a low is seen. It cannot prove a button is attached to that
pin. Only the finger in the log above does that.

## What this does not show

**No emulator ran.** Nothing here touches TerminalGB's own code, deliberately:
the scout named three independent risks — the toolchain, the panel, and Rust
atomics in PSRAM — and this closes the first two without the third in play. The
framebuffer is a 120,000-byte static in internal SRAM and PSRAM is never
allocated from, so the `s32c1i` hazard cannot be hiding in these numbers.

**The picture has not been photographed.** The refresh timing, the `BUSY`
handshake and a button press changing the picture are strong evidence the panel
is being driven correctly, and the geometry is tested — but "the test card looks
right on the glass" is a claim only a human in the room can make.

**Emulation speed on this chip is unmeasured.** The PSP anchor in
[`portability.md`](portability.md) says it should be comfortable. That is an
anchor, not a measurement.

## Before flashing anything

The device was backed up first — it had never been. There is no secure boot and
no flash encryption, so the image restores exactly:

```sh
tools/m5paper-build.sh backup       # 16 MB, about 90 s
esptool --port /dev/ttyACM0 write-flash 0x0 papercolor-factory-16mb.bin
```

The backup taken for this work is `papercolor-factory-16mb.bin`,
sha256 `cdfcae991b5356b919bcbd953e6f562260fa32f7f790ddbde0f8eec2aacb514c`, and it
carries the factory app descriptor `48a1057-dirty` the scout recorded — which is
how we know it is the image that was on the device and not a partial read.

## Next

The scout's step two: one Game Boy frame on the panel. That is where the
four-greys-into-six-inks question gets answered by looking at it, and where the
`#[cfg(unix)]` question in its §4 either bites or does not.

The scout's own conclusion stands and this work does not soften it: at sixteen
seconds a picture nobody plays Pokémon on this device. The interesting build is
the one where no human is holding it.

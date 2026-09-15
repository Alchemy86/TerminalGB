# Making the PaperColor's panel faster

**The refresh went from 15.954 s to 8.741 s. It is the frame rate, it is one
register, and it is the only thing that worked.**

Bring-up measured 15.967 s a picture and called it the panel. It is not quite:
it is the panel *as the vendor ships it*. The waveform is clocked by a PLL
divider at command `0x30`, M5Stack's init leaves it at `0x08`, and `0x08` is the
slow end of the range. At `0x07` this device draws the same picture in 8.741 s.

**This does not make the device playable and nothing will.** A Game Boy frame is
16.743 ms; 8.741 s is **522 times** that, against 954 before. What it buys is a
button press that answers in nine seconds instead of sixteen — worth having on a
thing you hold, and not a frame rate.

Two other things were tried and both cost exactly what a full refresh costs.
That is most of this page, because a negative measured properly is the part
nobody else publishes.

## What was tried

| | result |
|---|---|
| **PLL `0x30`** — the waveform's frame rate | **15.954 s → 8.741 s** |
| `0x83` — the addressing window ("partial refresh") | 14.454 s of waveform at every size from 8 rows to 600 |
| the `DISPLAY_REFRESH` (`0x12`) parameter byte | 14.454 s at `0x00`, `0x01`, `0x02` and `0x04` alike |

Every number is from
[`platform/m5paper/src/bin/panel_probe.rs`](../platform/m5paper/src/bin/panel_probe.rs)
on the captain's own device. Reproduce the lot with `tools/m5paper-build.sh probe`;
it takes about seven minutes and leaves a test card on the glass.

## The frame rate

`0x30` is a PLL divider whose low four bits set how fast the waveform's frames
are clocked out. Swept slow-to-fast, `POWER_ON` through `POWER_OFF`:

| `0x30` | refresh | | `0x30` | refresh |
|---|---|---|---|---|
| `0x00` | 30.415 s | | `0x04` | 12.634 s |
| `0x02` | 18.294 s | | `0x05` | 11.127 s |
| `0x03` | 15.952 s | | `0x06` | 10.003 s |
| **`0x08` (vendor)** | **15.954 s** | | **`0x07`** | **8.741 s** |

Monotonic, and the interesting row is the last pair: **`0x08` lands on `0x03` to
two milliseconds.** The field is `FRS[3:0]`, so `0x08`–`0x0F` alias onto `0x03`
— the vendor default is not a tuned choice, it is the top four bits being
ignored. Nobody ships this tuned: M5GFX sends `0x08`, Pimoroni's `inky_e640.py`
sends `0x08`, Seeed's `ED2208_Init.h` sends a slower `0x02`.

The sweep runs in both directions on purpose. `0x00` taking **twice** as long is
what proves the register is really the frame rate, rather than a faster number
being a mismeasurement somewhere.

## The half that is not measured here

**A too-fast waveform fails by looking wrong, not by taking longer.** The frames
are how long the pigments get to move; truncate them and the picture comes out
dark, or with the six inks not properly separated — and `BUSY` rises exactly on
time regardless. **No timer on this board can see that, and there is no MISO to
read a register back through** (`GPIO15` SCLK and `GPIO13` MOSI, and nothing
else), so nothing in this repository can tell you whether `0x07` looks right.

So the probe ends by drawing the six-ink test card at `0x07` and leaving it
there. That card is the check:

```sh
tools/m5paper-build.sh probe        # ends with the card on the glass at 0x07
tools/m5paper-build.sh testcard     # the same card rendered on the host
```

Six bands — black, white, yellow, red, blue, green — each boxed in black. If
they are distinct and solid, `0x07` is honest on this panel. If they are muddy
or missing, `PLAY_PLL` in
[`platform/m5paper/src/main.rs`](../platform/m5paper/src/main.rs) is the one
constant to move; `0x06` is 10.003 s and `0x05` is 11.127 s.

The corroboration `0x07` does have is somebody who could see his panel: Patrick
Siegl swept the same 4.0" glass and
[proposed `0x07` for it to Waveshare](https://github.com/waveshareteam/e-Paper/pull/387).
His absolute numbers are not ours — he measures 9.819 s at `0x07` and 19.083 s
at the default where we get 8.741 and 15.954 — which is the reason to sweep your
own board rather than paste his table. His larger 13.3" panel *does* break at
aggressive values ("picture just dark", "picture not finished"), so the failure
is real and gradual, not theoretical.

## Partial refresh: accepted, and free of charge in the wrong direction

Command `0x83` sets an addressing window for both RAM writes and the refresh.
Only GxEPD2 uses it; this panel's own two drivers have never heard of it. It
works — `BUSY` goes low, the panel refreshes — and it does not save a
millisecond:

| window | waveform |
|---|---|
| 8 rows of 600 | 14.455 s |
| 64 rows | 14.454 s |
| 150 rows | 14.454 s |
| 300 rows | 14.455 s |
| 600 rows (whole panel) | 14.455 s |

A 75× range of window heights, a 3 ms spread. The colour waveform runs to
completion whatever area it drives.

This is worth stating plainly because the opposite is in circulation. GxEPD2
advertises `hasPartialUpdate = true` for its Spectra 6 panel — and in the same
header declares `partial_refresh_time` **equal to** `full_refresh_time`, with
`hasFastPartialUpdate = false`. Partial here means *spatially* partial: only the
window flickers and the rest stays legible. It has never meant faster, and its
author never claimed it did.

Two independent confirmations arrived after the measurement above, which is the
right order:
[zephyrproject-rtos/zephyr#118340](https://github.com/zephyrproject-rtos/zephyr/issues/118340)
measured 34.94 s for a full panel and 34.945 s for a sixteenth of one on a 7.3"
Spectra 6 — the same area-independence, on other hardware. And the
[Seeed forum thread](https://forum.seeedstudio.com/t/ee04-and-spectra-6-7-3-partial-refresh/295260)
that prompted this investigation turns out to contain **no timing at all**: it is
eight posts about partial refresh making the white fade blue, ending "not even
one partial refresh is usable".

So `0x83` is implemented here — [`Ed2208::refresh_window`](../platform/m5paper/src/panel.rs)
— and nothing uses it. It is kept because the measurement is the point.

## The `DISPLAY_REFRESH` parameter

`0x12` takes a data byte and every driver for this family sends `0x00` without
saying why. The JD79661, a 4-colour cousin whose opcodes overlap this family's,
documents that byte as `D0 = AC/DC VCOM`, so there is one known non-zero meaning
in the neighbourhood. On this panel `0x00`, `0x01`, `0x02` and `0x04` all refresh
in 14.454–14.456 s. If the byte does anything here, it is not the duration.

## What was deliberately not touched

**No waveform, drive voltage or LUT register was written.** `0x83` is an address
range and `0x30` is a documented frame-rate divider. The registers that would
really be dangerous — `BTST1`/`BTST2`/`BTST3` (`0x05`/`0x06`/`0x08`, the
booster), `PWRR` (`0x01`) — are left exactly as the vendor init leaves them,
because driving this glass outside its waveform can damage it permanently and a
guess at a register is not worth that on hardware you cannot read back.

A LUT-from-register bit is the one surface that might hide a black-and-white-only
waveform, which would be the real prize: mono e-paper does a full update in
about 300 ms. The cousins have one — the SPD1656 and JD79661 both define
`LUT_SEL` in the second PSR byte — and **this panel's second PSR byte is `0x69`,
with that bit clear.** Setting it is two bytes. It was not done: if the bit means
the same thing here and no LUT has been loaded into the registers, the next
refresh drives the panel from whatever those registers happen to contain, and E
Ink's own ED2208 manual describes the part as a *"Hardware TCON"*, which argues
there is no register LUT to select. That is a coin-flip with permanent damage on
one face, and it needs a controller datasheet nobody has published rather than
courage.

Which controller it even is remains unknown. E Ink says only "an all-in-one IC";
Good Display lists the part as "NC"; Waveshare
[declined to answer when asked](https://github.com/waveshareteam/e-Paper/issues/369).
The JD79661 and SPD1656 datasheets were both checked against the init list here
and neither fits: both lack `CMDH` (`0xAA`), `PWRR` and `POFS`.

## What shipped

Gameplay repaints run at `0x07`; the look preview on button C stays at the
vendor's `0x08`. That split is deliberate — the preview's whole job is to show
what the six inks look like, and the risk a faster waveform carries is precisely
that the inks stop separating, so judging a palette through it would be judging
the wrong thing.

Measured end to end with Pokémon Blue on the device, which is the number that
counts rather than the probe's synthetic fill:

```
emulated  : 1000 frames in 81.056 s = 12 fps (20% of a real Game Boy)
--- frame 1000 onto the panel (dither) ---
panel     : 8.734 s (PLL 0x07; the vendor's 0x08 is 15.954 s)
```

8.734 s against the probe's 8.741 s on a flat fill — the refresh does not care
what the picture is, at this PLL as at the vendor's.

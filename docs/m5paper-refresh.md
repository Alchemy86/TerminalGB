# Making the PaperColor's panel faster

**The refresh went from 15.954 s to 7.721 s. Two things did it — the frame rate
and who sequences the power — and seven others were tried and did nothing.**

Bring-up measured 15.967 s a picture and called it the panel. It is not quite:
it is the panel *as the vendor ships it*. The waveform is clocked by a PLL
divider at command `0x30` that M5Stack leaves at the slow end of its range, and
the second and a half M5GFX spends hand-driving the power sequence around each
refresh is a second more than the controller needs if you let it do that itself.

**This does not make the device playable and nothing will.** A Game Boy frame is
16.743 ms; 7.721 s is **461 times** that, against 954 before. What it buys is a
button press that answers in eight seconds instead of sixteen — worth having on
a thing you hold, and not a frame rate.

The rest of this page is the seven things that did nothing, because a negative
measured properly is the part nobody else publishes — and because two of them
are claimed to work by people who never timed them.

## What was tried

| | result |
|---|---|
| **PLL `0x30`** — the waveform's frame rate | **15.954 s → 8.741 s** |
| **`0x17` AUTO** — let the controller sequence its own power | **8.737 s → 7.725 s** |
| `0x83` — the addressing window ("partial refresh") | 14.454 s of waveform at every size from 8 rows to 600 |
| the `DISPLAY_REFRESH` (`0x12`) parameter byte | 14.454 s at `0x00`, `0x01`, `0x02` and `0x04` alike |
| `LUT_SEL` — PSR's second byte, bit 7 | −2 ms. inert |
| PSR's first byte, bit 4 (`BWR`) | −3 ms. inert |
| PSR written as one byte instead of two | +11 ms. accepted, and inert |
| a forced temperature through `TSE` (`0x41`) | inert at three values |
| cutting the power part-way through the waveform | the panel survives it; the picture is an open question |

Every number is from the three probes in
[`platform/m5paper/src/bin/`](../platform/m5paper/src/bin/) on the maintainer's own
device. `tools/m5paper-build.sh probe` is the safe one and takes about seven
minutes; `deepprobe` writes the undocumented registers; `autoprobe` interleaves
the AUTO comparison. All three end by leaving a test card on the glass.

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

## Who sequences the power

A refresh is not only its waveform. M5GFX drives the sequence by hand — power
on, wait on `BUSY`, settle 200 ms, retune the booster, settle 200 ms, refresh,
power off, wait, settle — and that scaffolding is about a second and a half.
Command `0x17` (AUTO) with `0xA5` hands the whole of it to the controller.

Interleaved, three rounds each at `0x07`, so a drifting panel could not favour
either path:

| | mean of 3 |
|---|---|
| hand-driven | 8.737 s |
| AUTO | **7.725 s** |
| AUTO, replaying the booster retune | **7.725 s** |

**−1012 ms, and not one of the six samples varied by more than a millisecond.**
The waveform is untouched: AUTO's own window measures 7.525 s against the
hand-driven 7.247 s precisely *because* it contains the power-on the other one
times separately.

The check that it is not cheating: the hand-driven rounds sat *between* the AUTO
ones and kept their own time exactly. Had AUTO skipped the power-off and left
the panel live, the next hand-driven `POWER_ON` would have returned early and
its total would have dropped. It did not.

AUTO is in no vendor driver for this panel. It is in Patrick Siegl's list of
commands that respond on this exact glass, and both probe runs here confirm the
controller accepts it. The booster values M5GFX rewrites before every refresh
are replayed through it — they cost nothing measurable and keeping them keeps
that intent, since the vendor writes a different last byte for a refresh
(`0x27`) than for init (`0x17`).

## The registers that did nothing

The maintainer authorised writing registers nobody has documented for this
controller — *"All options are ok to test. It's my test board."* — so they were,
twice, and the answers replicate to within a few milliseconds of each other.

**`LUT_SEL` was the prize and it is not there.** The refresh is long because a
Spectra 6 waveform separates six pigments; a black-and-white waveform would be a
fraction of it, and the only surface that could expose one is a
LUT-from-register bit. Two cousins whose opcodes overlap this family's — the
SPD1656 and the JD79661 — put `LUT_SEL` in PSR's second byte, bit 7. Ours is
`0x5F, 0x69`, with that bit clear. Setting it (`0x69` → `0xE9`) moved the
waveform by **−2 ms in one run and −2 ms in the other**. It is inert.

Two more readings of PSR came back the same way. Bit 4 of the first byte, which
Siegl's reverse engineering of this panel calls `BWR` and which picks
black/white against black/white/red in the UC81xx family, is **−3 ms and −2 ms**.
And Siegl's single-byte `0x1B`, which works on his 4.0" where the vendor sends
two, is **accepted here** and **+11 ms and +7 ms** — which is its own small
finding: if the controller is content with one byte, the second one is not
carrying a mode this panel implements.

**Temperature is not a lever either.** Many controllers pick a waveform by
temperature and warmer means faster, and `0x41` (TSE) responds on this panel.
Three forced values, both runs: 14.453 s, 14.453 s, 14.455 s. That agrees with
what the frame-rate sweep had already hinted — seven minutes of back-to-back
refreshes never drifted by more than 3 ms, which is not what a
temperature-compensated waveform does on a panel warming itself up.

**Nothing was damaged.** Each experiment ran once, was followed by a hardware
reset, a full re-init and a normal refresh at the vendor's settings, and both
runs closed on a clean test card measuring 14.456 s and 14.452 s — the same as
the baselines that opened them, 14.448 s and 14.453 s.

## Cutting the waveform short

The one route to a shorter update that needs no undocumented register at all:
start the normal waveform and cut the power part-way. The pigments end up
wherever they had got to.

Four cuts — 1, 2, 4 and 7 seconds — and the panel kept responding through every
one, on both runs, with the next full refresh measuring its usual time. **So it
is survivable.** Whether it is *legible* is a picture, and no timer on this
board can report it, so it is not answered here.
[`platform/m5paper/src/bin/panel_truncate.rs`](../platform/m5paper/src/bin/panel_truncate.rs)
is the instrument for it: `tools/m5paper-build.sh truncate` steps through a
ladder of cuts on the buttons, drawing the six-ink test card each time, so
whoever is holding the device can watch how much waveform a legible picture
needs. **Nobody has run it yet.** If black and white resolve well before the
colours do, that is another four times on top of everything above; if it is a
grey smear, the waveform is the wall.

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

`LUT_SEL` was left alone on the first pass for exactly that reason and then
authorised and tried; it is inert, and the section above has the numbers. E
Ink's own ED2208 manual describing the part as a *"Hardware TCON"* now looks
like the right reading: there is no register LUT here to select, so there is no
black-and-white waveform to reach.

**One category stays untouched whatever the authorisation, and it is not
timidity.** `PWRR` (`0x01`), `BTST1`/`BTST2`/`BTST3` and `VDCS` (`0x82`) set
drive voltages. Over-voltage is the one thing that damages e-paper outright and
**it cannot make a refresh shorter**, so writing it would be risk with no
possible upside. Nor was the opcode space swept for undocumented commands:
these controllers carry OTP-programming commands (the family has a `ROTP` at
`0xA2`), and an OTP write is permanent. Fishing for a fast mode by sending bytes
at a chip whose datasheet nobody has is how you brick a panel, not how you
measure one.

Which controller it even is remains unknown. E Ink says only "an all-in-one IC";
Good Display lists the part as "NC"; Waveshare
[declined to answer when asked](https://github.com/waveshareteam/e-Paper/issues/369).
The JD79661 and SPD1656 datasheets were both checked against the init list here
and neither fits: both lack `CMDH` (`0xAA`), `PWRR` and `POFS`.

## What shipped

Gameplay repaints run at PLL `0x07` through AUTO; the look preview on button C
keeps the vendor's `0x08`. That split is deliberate — the preview's whole job is
to show what the six inks look like, and the risk a faster waveform carries is
precisely that the inks stop separating, so judging a palette through it would be
judging the wrong thing. Pressing C is therefore also the A/B.

Measured end to end with Pokémon Blue on the device, which is the number that
counts rather than a probe's synthetic fill:

```
emulated  : 1000 frames in 81.019 s = 12 fps (20% of a real Game Boy)
--- frame 1000 onto the panel (dither) ---
panel     : 7.721 s (PLL 0x07 + AUTO; the vendor's own sequence is 15.954 s)
```

7.721 s against the probes' 7.725 s on a flat fill — the refresh does not care
what the picture is, at this PLL as at the vendor's.

Both are one constant each in
[`platform/m5paper/src/main.rs`](../platform/m5paper/src/main.rs): `PLAY_PLL`
and `USE_AUTO_SEQUENCE`. If the inks come out weak, `PLAY_PLL` is the one to
move first — `0x06` is 10.003 s of waveform — because AUTO does not touch the
waveform at all and cannot be what spoiled it.

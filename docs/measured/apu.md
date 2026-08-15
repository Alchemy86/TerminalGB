# The APU — ten constants, each measured against a named ROM

← [The measured Game Boy](README.md)

This is the page that goes furthest past the hardware reference, because sub-sample APU timing
is where the references stop and the test ROMs start. Pan Docs' [Audio
Details](https://gbdev.io/pandocs/Audio_details.html) is excellent on *what* each unit does;
it does not tell you that a square channel's frequency-timer reload is latched five or six
T-cycles late, because that is not a fact anybody has needed to write down until an emulator
tried to pass [SameSuite](https://github.com/LIJI32/SameSuite).

SameSuite's APU group is **<!--score:samesuite_apu-->66/69<!--/score-->** here today; the first campaign over it took this emulator
from **2 to 49** of the 61 ROMs that run scored. (Two figures, two denominators — stated
separately on purpose, per rule 2 on the [entry page](README.md#reading-the-numbers-honestly).)
Every constant below is a number that was *swept* until a ROM agreed, with the ROM named and
the cost of moving it measured. **Do not "tidy" any of them.**

Hardware reference: [Audio](https://gbdev.io/pandocs/Audio.html) ·
[Audio Registers](https://gbdev.io/pandocs/Audio_Registers.html) ·
[Audio Details](https://gbdev.io/pandocs/Audio_details.html). Register-level component page:
[`docs/components/apu.md`](../components/apu.md). Signal path, pacing and latency:
[`docs/audio.md`](../audio.md).

---

## The constants, and what moving each one costs

All in [`src/sound.rs`](../../src/sound.rs), in T-cycles unless stated. (That file is in
`codemap_links.py`'s `SRC_EXCLUDE`, so its references stay file-level rather than carrying a
line anchor that would rot.)

| Constant | Value | What pins it | Measured consequence of moving it |
|---|---:|---|---|
| `SQUARE_TRIGGER_DELAY` | **6** | `channel_{1,2}_delay` — "(sample length + 2) ticks from enabling channel 1 until PCM12 is affected", stepped one M-cycle at a time | `5` → **56/76** · `7` → 62/76 (grid rounding absorbs it) · `8` → **39/76**, most of the square suite |
| `SQUARE_TRIGGER_ALIGN` | **0** | `channel_{1,2}_align`, at **double speed** — where the CPU can write NRx4 on either half of a 1 MHz grid cell and the first duty step must *not* move with it | invisible at single speed; the reason it is a separate constant at all |
| `SQUARE_RESTART_SPEEDUP` | **4** | `channel_{1,2}_restart` — a re-trigger's delay is one tick shorter, and does **not** reset the duty position | `0` → 58/76, losing `channel_{1,2}_restart` and `channel_{1,2}_restart_nrx2_glitch`. It used to be recorded here as "what `channel_1_sweep_restart` round 1 wants"; [that was wrong](#channel_1_sweep_restart--one-duty-step-in-4100) |
| `SQUARE_RELOAD_LATCH` | **6** | `channel_1_freq_change_timing-cgbDE`, sweeping the write across the expiry one `nop` at a time at **both** clock speeds, reading PCM12 twice | passing band is `5..=6`, **uniform at both speeds**; `≥7` fails every variant of the ROM |
| `SQUARE_RELOAD_LATCH_AGB` | **4** | `channel_1_freq_change_timing-A` — the same program, the same sweep, on a **Game Boy Advance** | band `3..=4`; the two bands do not overlap, and *nothing else in the 76-ROM suite moves anywhere in `1..=6`* |
| `SWEEP_RESTART_HOLD` | **12** | `channel_1_sweep_restart_2` — re-triggers CH1 at `$7FF` a fixed 0–7 M-cycles before a sweep event | an event 12 T after the trigger kills the channel; one at 8 T does not |
| `sweep_calc_delay_for(shift)` | `(shift+1) × 4` | `channel_1_sweep` — the channel still plays for exactly 8 M-cycles after a shift-7 sweep updates the frequency, then goes silent | the overflow check happens `shift + 1` M-cycles **after** the frequency write, not with it |
| `WAVE_TRIGGER_DELAY` | **4** | `channel_3_delay` **and** Blargg `dmg_sound` 09/10/12 — doubly constrained | ±2 flips all three `dmg_sound` CRCs |
| `NOISE_TRIGGER_DELAY` | **2** | `channel_4_delay` | — |
| `NOISE_TRIGGER_ALIGN` | **0** | `channel_4_align` | — |

Plus a constant that is not a delay but is calibrated the same way:

| | | |
|---|---|---|
| `Sound::grid_phase` | `0..=15` | Bits 0–1 are the 1 MHz phase the square channels align to; **bit 2 is the half-tick the CH4 divisor stage races against**. `SquareChannel::trigger` masks `& 3` itself. Anything new reading `grid_phase` must mask deliberately. |

### Why `SQUARE_RELOAD_LATCH` is the best-evidenced of them

A frequency write does not merely "take effect at the next expiry". The timer samples its
reload value a few T-cycles *after* the expiry, so a write inside that window supplies the
period the timer has **just** reloaded with — and the next sample then lands one whole *new*
period from that expiry, instead of the old countdown finishing first. One duty step earlier
than the naive rule gives.

The ROM pins the window **from both ends at both clock rates**: it sweeps the write across the
expiry one `nop` at a time, single *and* double speed, reading PCM12 immediately and again
once the channel has frozen on the sample it will hold.

**And `period - delay` is not "time since the last expiry".** Inferring it arithmetically
turned two green rows red: a freshly triggered channel walks that same range with no expiry
behind it (a trigger loads the timer itself), and `channel_{1,2}_freq_change` write NRx3
exactly there. Hence a separate `since_expiry`, parked at `u32::MAX` by a trigger — live
machine state, and a save-state format bump.

**Cost: ~1.3 µs/frame** (`apu_bench`, min of 5: 26.7 → 28.0 µs/frame) — 0.008 % of a 16.743 ms
frame, for one comparison per NRx3/NRx4 write.

> **Nothing outside SameSuite can see this.** Blargg `dmg_sound` stays 12/12 with every
> captured audio hash unmoved on both sides. A whole class of sub-sample APU timing is
> invisible to a suite judged by ear or by checksum. **That gap is the finding, not an aside.**

---

## Structures that had to be right before any constant could be

Calibrating a constant against the wrong structure fits a number that then breaks four other
rows. Three cases where the structure was the fix:

**The frame sequencer is `DIV`-driven.** Falling edge of system-counter bit 12 (bit 13 at
double speed) — so a `$FF04` write *fires an event* when the bit was set, and resets the
phase. `Timer::wb($FF04)` must zero the sub-`DIV` counter as well as the visible byte;
leaving it running desynchronises everything clocked off the same counter and made the whole
`div_*` group non-deterministic.

**The volume envelope is two things, and separating them is the whole fix**: a 3-bit period
counter clocked at 64 Hz, and a *pending-step latch*
(`VolumeEnvelope::clock` in [`src/sound.rs`](../../src/sound.rs)) set when that counter reads zero and acted on at
the next **512 Hz** event. The counter is free-running — it reloads from NRx2 when it reads
zero rather than stopping — so an envelope parked at period 0 resumes the instant a non-zero
period is written, **with no trigger**. That is what every real music driver's mid-note volume
ramp depends on.

- The period counter runs on frame-sequencer **step 6, not 7**. Length (even steps) and sweep
  (step 2 of 4) were already one step ahead of the event that acts on them; the envelope was
  not, and that inconsistency looked exactly like an extra 512 Hz of latency. Putting it back
  on 7 costs six rows.

**Square duty output is a latch**, not `DUTY_PATTERN[duty][duty_pos]` read on demand. It
updates only at a frequency-timer expiry, so a freshly powered channel reads 0 for its whole
first position. The duty table is the canonical Pan Docs one — ours used to be rotated, which
sounds identical and is measurable.

**A CH4 trigger reloads neither frequency counter — it halves both.** The first LFSR step
lands at `(period - divisor)/2 + divisor/2`. The one exception is that the divisor stage
cannot be left holding less than one of its own ticks (16 T, or 8 for code 0, itself a half
tick), so codes 0 and 1 are floored and code 3 is not. **That floor alone is why the four
encodings of one 16 M-cycle sample (`$0C`/`$1A`/`$29`/`$38`) start 0 / 0 / +2 / +1 M-cycles
apart** — `channel_4_frequency_alignment` and `channel_4_equivalent_frequencies`, both green.

Gated device-free by `the_noise_trigger_delay_matches_samesuite`: eleven `(NR43, delay)` pairs
read off the ROMs, **not** off our output — nothing in `cargo test` runs SameSuite.

**NRx2 does not reload the live volume**; only a trigger does. Writing it while the channel
plays is "zombie mode", and the rule implemented here is **CGB revision E's**, derived from
`channel_N_volume`'s twelve old/new direction+period combinations. The gg8-wiki DMG formula
does not fit and must not be substituted.

---

## PCM12 / PCM34 are the whole interface

`$FF76` / `$FF77` ([CGB Registers](https://gbdev.io/pandocs/CGB_Registers.html)). **57 of the
61 APU tests are unreachable without them** — low nibble CH1/CH3, high nibble CH2/CH4, zero
when the channel or its DAC is off. The MMU routes them to the APU with a `catch_up()` first,
exactly like NRxx.

Two more facts a foreign implementation gets wrong in the same way:

- **The APU only runs when a player is installed.** With none, `Sound::do_cycle` returns
  immediately — so a harness that never calls `enable_audio` has *no APU at all*, not a quiet
  one. **Any APU measurement without a player is measuring nothing.** This cost this project
  months: `interrupt_time` was recorded as "cannot run headless" when in fact it spins on
  `NR52` waiting for a chip that was not there.
- **The APU runs off the base clock**, not the CPU clock. Before that was fixed, CGB
  double-speed games played an octave high *and*, because the audio queue paces the emulator,
  ran at about half speed: Blargg `cpu_instrs` measured **33.7 fps / 48 % of frames late
  before, 59.70 fps / 0 % after**.

---

## The Game Boy Color shipped in revisions, and its APU changed between them

`CgbRevision` (in [`src/sound.rs`](../../src/sound.rs)), default `CgbC` (= C, D, E, and what
every DMG does). `$GB_CGB_REVISION=cgb0|cgbB|cgbC`; the suite runner reads it off the ROM
filename, the same move the in-repo harness makes with `@dmg`/`@cgb`.

On CPU CGB 0 and B, extra length clocking drops the *"and now enabled"* half of its condition:
an `NRx4 = $00` write clocks length anyway. **That is a silicon bug — it breaks Prehistorik
Man — fixed in CPU CGB C.** CH3 differs again between 0 and B: on B the clock that reaches
zero leaves the channel playing and the *next* one kills it.

**`-cgb0` and `-cgbB` contradict each other, so a runner that ignores the filename can never
score more than four of the five.** The ROM still judges itself; the knob only picks which
machine it is judging. A DMG is never affected, pinned by `the_revision_quirk_never_reaches_a_dmg`.

`cgb_revision` is machine *configuration*, not cartridge state, so it is deliberately not
serialised: a save state restores into whatever machine is running.

---

## The frequency-reload latch is narrower on an AGB

`channel_1_freq_change_timing` ships **three** ROMs — `-cgbDE`, `-cgb0BC` and `-A` — that are
the same program with three different `CorrectResults` tables. They disagree about one thing:
how wide the window is in which a frequency write can still edit the reload that has just
happened. Sweeping `SQUARE_RELOAD_LATCH` over the whole suite says so exactly:

| latch | 1 | 2 | **3** | **4** | **5** | **6** | 7 |
|---|---|---|---|---|---|---|---|
| `-A` | fail | fail | **pass** | **pass** | fail | fail | fail |
| `-cgbDE` | fail | fail | fail | fail | **pass** | **pass** | fail |
| rest of the suite | — | — | unmoved | unmoved | unmoved | unmoved | −1 |

Two adjacent 2 T-wide bands that do not overlap, and **nothing else in the 76 ROMs moves
anywhere in `1..=6`** — so the console selection is the whole of the difference, and no single
machine can pass more than one of the three. That is the same shape as the suite's
`-cgb0`/`-cgbB` pair, which this emulator already answers by booting the console the ROM's
filename names; `A` is Gekkio's own single letter for a Game Boy Advance, and
`examples/mooneye_scout.rs` has read it that way for as long as the console revisions have
existed. `examples/samesuite.rs` now does too, and `Model::Agb` — which this repository has
modelled for its **boot state** since the console-revisions work — reaches the APU through
`MemoryManagementUnit::enable_audio`. `GB_MODEL=agb` is the whole of the user-facing knob;
`Model::Auto` still never infers an AGB.

The behaviour behind the two numbers is a correction the reload applies to the duty position
when a frequency write drops NRx4's high bits from 7. SameBoy applies it unconditionally on
CPU CGB D and E, and makes it depend on the 2 MHz clock phase on an AGB and on CPU CGB C and
older (`Core/apu.c`, the `GB_IO_NR14` case: *"On an AGB, as well as on CGB C and earlier, it
behaves slightly different on double speed"*). Read there, reimplemented here as the width of
the window itself, which is what this ROM measures. Gated device-free by
`sound::tests::an_agb_latches_its_frequency_reload_two_cycles_earlier`, teeth proven by
widening the AGB constant back to 6.

---

## Three rows left red, all on evidence

See [scoreboard.md § SameSuite](scoreboard.md#samesuite--three-evidenced-reds) for the table.
All three were re-opened on 2026-08-15 and checked against SameBoy at a pinned commit
([`tools/sameboy-oracle/`](../../tools/sameboy-oracle/README.md)); two of the excuses that used
to be recorded here did not survive it.

### `channel_1_sweep_restart` — one duty step in 4100

**143 of its 144 cells match.** Round 1 sweeps `$7FF` down to `$7F0` in negate mode,
re-triggers, and reads PCM12 at 0–15 M-cycles; every cell must read `$00`. Ours reads `$08` at
the last one.

This page used to blame `SQUARE_TRIGGER_DELAY` / `SQUARE_RESTART_SPEEDUP` and say the row
"wants `SPEEDUP = 0`". **That was wrong, and tracing both emulators through the same subtest
says what is actually happening.** Ours and SameBoy's, from their own NR14 writes, in T-cycles:

| | SameBoy (CGB-E) | here |
|---|---:|---:|
| fresh trigger, freq `$7FF` (period 4 T) → first duty step | 12 | 10 |
| sweep writes `$7F0` (2nd DIV-APU event) | **16400**, duty index **2** | **16404**, duty index **3** |
| re-trigger → first duty step | 68, index **4** | 66, index **5** |

The re-trigger's step is not the problem: duty 2's positions 3 and 4 both output 0, which is
why SameBoy reads `$00` at 68 T *even though the step happened there*. The problem is that we
arrive at the re-trigger **one duty position further on**, having taken one extra 4 T step in
the ~4100 the channel makes at period 4 T before the sweep event.

And the cause is that the two clocks are 2 T out of step with each other. On hardware the
DIV-APU edge and the channel's frequency timer come off the same counter chain: the edge is a
multiple of 8192 T from the counter's zero, so it can only ever land *on* a 1 MHz boundary,
and a period-4 channel steps on every 1 MHz boundary. Here it lands 2 T off one. Both halves
are individually pinned by rows that pass, and **neither can be moved, alone or together**:

| | DIV-APU event unmoved | 4 T earlier | 6 T earlier |
|---|---:|---:|---:|
| `SQUARE_TRIGGER_DELAY` 6 (shipped) | **73/76** | 59/76 | 59/76 |
| 7 | 72/76 | 58/76 | 58/76 |
| 8 (steps on the M-cycle grid, SameBoy's number) | 47/76 | 39/76 | 39/76 |

Rounding the trigger delay up to the grid instead of adding a phase pad, and moving
`SQUARE_TRIGGER_ALIGN` to 2, both give the same 47/76: at single speed they are `DELAY = 8` by
another name. The residual is a **structural** 2 T offset between where this APU thinks a bus
access lands and where the 1 MHz grid is, and it is invisible to all 75 other rows because
every one of them measures a *relative* interval, which the offset cancels out of. Fixing it
means re-deriving the APU's access phase and re-calibrating every constant in the table above
— not moving one of them.

### `channel_4_freq_change` — the counter model gets 61 of 64 cells

The old note here said the suite's own author does not pass it, quoting SameSuite's
`apu/README.md`. **Measured 2026-08-15: current SameBoy on CGB-E passes it** — and passes
`channel_1_sweep_restart` too. That README predates its own emulator; do not quote it as
evidence again.

The structure is known and this repository predicted it: the LFSR is clocked from a
**free-running 14-bit counter**, ticked every `(NR43 & 7) × 8` T-cycles (4 for code 0), with
the LFSR stepping on the 0→1 edge of the counter's bit `NR43 >> 4`. Our single `period`
countdown is *algebraically identical* to that for a fixed NR43 — which is why ten CH4 rows
are green — and differs only when NR43 changes mid-flight, which is the whole of this ROM.

What blocks it is that the clean counter model is not enough. Searched exhaustively over both
reload policies (the write leaves the in-flight countdown alone, or restarts it), every trigger
countdown in `1..=39` for both divisor codes the ROM uses, and every read-point offset in
`−8..=8`: the best fit is **61 of the 64 cells**, and the parameters that reach it contradict
the trigger countdown `channel_4_delay` and `channel_4_frequency_alignment` already pin. The
rest is SameBoy's NR43-write glitch table, whose own comments read *"non-deterministic"*,
*"this quirk seems way too specific"* and *"No clue why that specific bit is tested"*. That is
a calibration problem, not a modelling one, and the suite carries no evidence to calibrate it
against.

### `channel_1_freq_change_timing-cgb0BC` — two behaviours, on a machine we do not boot

CPU CGB 0/B/C. It differs from `-cgbDE` in three of its sixteen cells, and they are **not all
the same kind**: two are the high nibble (the PCM12 read taken in the same M-cycle the
amplitude changes — the glitch SameSuite's README lists as understood by nobody, and which
SameBoy models by masking the read on `model <= CGB_C`), but the third is the *settled* value
at double speed, which is a real timing difference. So the latch alone cannot do it, and the
sweep above confirms it: **no value in `1..=7` turns this row green.**

Passing it needs the read glitch, a double-speed condition on the reload correction, and
`CgbRevision` to stop lumping C with D and E — three changes for one row, on a machine nothing
else here boots. Scoped, not attempted.

> **Read `apu/README.md` inside SameSuite before grinding any row — and then check it.** It
> states which revisions pass what, and the `.asm` `CorrectResults` tables plus the header
> comments are where every expectation on this page came from. But its claims *about other
> emulators* are stale, and `tools/sameboy-oracle/` exists so nobody quotes them again.

---

## Two rules that are not about accuracy at all

Both were learned by shipping the opposite, and both are load-bearing.

**An `AudioPlayer` must never block — it runs inside the emulated frame.** `play_samples` is
called from `Cpu::do_cycle`, so anything the player does is charged to emulation time. Making
the sound card the master clock cost **17.93 ms/frame against a 16.743 ms budget** while the
no-audio build measured 0.29 — and **97.6 % of that was blocked, not executing**. Two
compounding effects: the wait's equilibrium is *slower* than real time, and the frontend then
enters catch-up, which fills the queue faster, which blocks every frame — pinned at ~5 fps
with no escape. **Never put a second clock inside the emulator's inner loop.** The hand-off is
now push-then-discard-oldest. Gated by `make test-audio-perf`, which needs the `audio` feature
but **no sound card**.

**Playback volume is not `NR50`, and must never become it.** `NR50` is *emulated behaviour*
that the accuracy suites measure, that a game reads back, that is in save states, and that is
baked into the audio baseline. Attenuation is applied **past the end of emulation**, in the
device callback, so emulation stays bit-identical at every level — no baseline, no suite and
no capture harness can see it. Scale in `f32` *before* the device-format conversion:
attenuating an integer sample throws away its low bits, and that is exactly where "crunchy
when quiet" comes from.

> **The audio-cost folklore is stale — re-measure before repeating it.** "Audio costs ~8
> ms/frame" and "17.9 ms/frame with sound" describe the blocking player, fixed long ago.
> Measured today with a real device attached: **0.256 ms/frame**, against **0.219** for the
> same run with no audio at all. The single biggest performance lever in this emulator is the
> [PPU engine](ppu.md) (0.219 → 0.467, 2.13×), not audio.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| CGB double-speed games play an octave high and run at half speed | the APU clocked at CPU speed instead of the base clock |
| A music driver's mid-note volume ramps never resume after a period-0 write | the envelope's period counter implemented as stopping rather than free-running |
| Everything sounds right and 57 of 61 APU tests cannot even start | `PCM12`/`PCM34` (`$FF76`/`$FF77`) not implemented |
| `NR52` reads back "powered off" on a machine that has audio | no `AudioPlayer` installed — the APU is not quiet, it is absent |
| Prehistorik Man breaks | modelling CPU CGB 0/B's extra-length-clocking bug on a machine that should be revision C or later — or the reverse |
| The first note after a channel is powered on is missing | duty output read on demand instead of latched at a timer expiry |

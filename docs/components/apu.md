# APU — sound (audio processing unit)

> Source: [`src/sound.rs`](../../src/sound.rs) · Related: [MMU](mmu.md) · [Frontends](frontends.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`6c20451`](https://github.com/Alchemy86/TerminalGB/tree/6c20451306663018288d34253c199fad507ae6f6) · regenerate with `make docs-links`, verify with `make docs-check`.

> ⚠️ **`src/sound.rs` is being rewritten right now** (two audio crews). To avoid minting
> line links that break every rebase, this page's `sound.rs` references are **file-level
> links** — the `:line` in the text is a hint, and the link opens the file, not an exact
> line. `sound.rs` is in the `SRC_EXCLUDE` set of `docs/tools/codemap_links.py`; drop it and
> re-run `make docs-links` once the APU settles to restore precise anchors. The APU "why"
> (frame sequencer, length counter, frequency sweep) lives in the prose below.

## 1. What it is

The APU is the sound chip. It has four independent voices ("channels") that each generate
a simple waveform in hardware — two square-wave tone generators, one programmable
wave-table channel, and one noise generator — mixed into left/right stereo. The game
doesn't send audio samples; it writes a handful of registers ("play this pitch, this
volume, fade it out over this long") and the chip produces the sound continuously.

Emulating it means running those four generators cycle-by-cycle and turning their output
into a stream of samples the host sound card can play. This emulator uses band-limited
synthesis (`blip_buf`) for clean output and `cpal` for playback — **on native builds
only**.

## 2. The real hardware

Four channels, each with a **length counter** (auto-stop after a set time) and most with a
**volume envelope** (fade in/out). Clocking them is a **512 Hz frame sequencer** that
ticks the sub-units at different rates (Pan Docs — [Audio](https://gbdev.io/pandocs/Audio.html)):

- **Length counters** clocked at **256 Hz**.
- **Volume envelopes** clocked at **64 Hz**.
- **Channel 1's frequency sweep** clocked at **128 Hz**.

The channels:

- **Channel 1 — square + sweep:** a square wave (4 selectable duty cycles) with envelope
  *and* an automatic frequency sweep (used for "pew" sounds).
- **Channel 2 — square:** the same, minus the sweep.
- **Channel 3 — wave:** plays 32 4-bit samples from 16 bytes of wave RAM
  (`0xFF30–0xFF3F`) — arbitrary one-cycle waveforms.
- **Channel 4 — noise:** a linear-feedback shift register (LFSR) producing pseudo-random
  noise (drums, explosions), with a 15-bit or 7-bit width mode.

Registers live at **`0xFF10–0xFF3F`** (`NR10`–`NR52` plus wave RAM). `NR52` (`0xFF26`) is
the master on/off + per-channel status. Each channel is **triggered** by writing bit 7 of
its high register (`NRx4`).

## 3. History and versions

| Model | APU differences |
|---|---|
| **DMG / MGB** | The 4-channel APU above. Mono internally, stereo panning via `NR51`. |
| **SGB** | The SNES can also play SNES audio, but the Game Boy APU itself is unchanged. |
| **CGB** | Same four channels, but several **register read-back masks and trigger/length edge behaviours differ subtly** from DMG — enough that Blargg ships *separate* `dmg_sound` and `cgb_sound` suites. Wave-RAM access timing and the "obscure" length-counter behaviours are the main divergences. |

**This emulator targets the CGB register model**; its known-failing screenshot is
`cgb_sound`, i.e. exactly these subtle edge behaviours.

## 4. How it works here

The design closely follows rboy's APU: a per-channel `BlipBuf` with `add_delta`/`last_amp`
band-limited synthesis and the standard frame-sequencer structure.

**The `Sound` struct** — [`src/sound.rs:458`](../../src/sound.rs): four channel structs,
volume/panning registers, `frame_seq_counter`, an `Option<Box<dyn AudioPlayer>>`, and the
timing bookkeeping (`CLOCKS_PER_FRAME = 4194304 / 512`, [`sound.rs:11`](../../src/sound.rs)).

**The channels** — `SquareChannel` (used for **both** channel 1 and 2; sweep fields only
stepped for channel 1) [`sound.rs:134`](../../src/sound.rs); `WaveChannel` with
`waveram: [u8; 16]` [`sound.rs:274`](../../src/sound.rs); `NoiseChannel` with the LFSR
[`sound.rs:371`](../../src/sound.rs).

**The frame sequencer** — `Sound::run` ([`sound.rs:569`](../../src/sound.rs)) advances a
0–7 counter, one step per `CLOCKS_PER_FRAME`:

```
frame_seq_counter % 2 == 0  → length.step()   (all 4 channels)   // 256 Hz
frame_seq_counter % 4 == 2  → channel1.sweep_step()              // 128 Hz
frame_seq_counter == 7      → volume.step()    (ch 1,2,4)        //  64 Hz
```

matching the hardware rates exactly. Length reload on trigger is modelled
([`sound.rs:117`](../../src/sound.rs)); envelope ([`sound.rs:68`](../../src/sound.rs)) and
sweep with overflow-disable ([`sound.rs:234`](../../src/sound.rs)) are present.

**Output** — each channel feeds a `blip_buf::BlipBuf` (`add_delta` at
[`sound.rs:200`,`439`,`439`](../../src/sound.rs)); `play_samples`
([`sound.rs:612`](../../src/sound.rs)) drains them to the `AudioPlayer`.

**Playback** — the `AudioPlayer` trait ([`sound.rs:16`](../../src/sound.rs)) is implemented
by `CpalPlayer` ([`sound.rs:922`](../../src/sound.rs)), which opens a 44.1 kHz `cpal`
output stream. The desktop frontend wires it up in `render_desktop`
([`src/gameboy.rs:103`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gameboy.rs#L103)).

### Honest approximations & caveats

- ~~**No sound on web.**~~ **Fixed.** The whole module used to be
  `#[cfg(not(target_arch = "wasm32"))]`, so the browser build read `0xFF10–0xFF3F` as
  `0xFF`, dropped every write and had **no sound chip at all** — not a quiet one, which
  a cartridge waiting on `NR52` can tell apart. The APU is compiled on every target now
  and the browser plays it through the Web Audio API; see
  [`docs/web-audio.md`](../web-audio.md) for the design, the measured numbers, and why the
  `cfg` was there (a `blip_buf` version that bound to a C library).
- **Register accuracy is measured: Blargg `dmg_sound` passes 12 of 12 subtests**,
  per the offline audio harness
  ([`testharness/audiocapture.rs`](../../testharness/audiocapture.rs)), which reads the
  suite's memory-mapped verdict at `$A000`. This was 1/12 before the APU accuracy pass
  (which took it to 9/12) plus the wave-window pass (9→12). The accuracy pass:
  the register-shadow read-back model (`regs` + `NR_READBACK_MASKS`), catch-up-on-access
  synchronisation (`Sound::catch_up`, called from the MMU before every APU register
  access so NR52/length/sweep state is never stale), length-expiry disabling the channel,
  the NRx4 extra-length-clocking quirks (`apply_nrx4_length`), full sweep semantics
  (always-reloaded shadow/timer/enable on trigger, negate-used tracking, the NR10
  negate-exit disable), DAC-gated triggers, and the power-on frame-sequencer reset.
- **The last three (09/10/12, the DMG wave-RAM access quirks) did NOT need a wholesale
  T-cycle-stepped APU** — the once-documented non-goal was resolved with two contained
  changes. (1) The **APU clock is stepped per CPU memory access**, exactly like the
  timer: `MemoryManagementUnit::bus_tick` advances `Sound::do_cycle(4)` before each
  bus access and `MMU::do_cycle` applies only the instruction's remaining cycles, so
  `catch_up()` syncs the APU to the exact sub-instruction cycle of a wave-RAM or NR5x
  access. Synthesis stays event-driven (blip_buf deltas, skip-ahead between channel
  timer events) — nothing runs 4.19 M times a second, and the measured cost is
  ~25 µs/frame with audio enabled (0.15 % of the 16.74 ms frame budget), zero without.
  (2) The wave channel tracks `since_fetch` (T-cycles since its last wave-RAM byte
  fetch) and models the **DMG access window**: while CH3 plays, CPU reads/writes of
  `$FF30–$FF3F` map to the byte CH3 just fetched for the 2 T-cycles after a fetch and
  are `$FF`/dropped otherwise; retriggering while a fetch is imminent (`delay == 0`)
  corrupts the first bytes of wave RAM (byte 0, or the aligned 4-byte block, per Pan
  Docs "Audio Details"). Trigger reloads the fetch timer with period + 4 T-cycles
  (`WAVE_TRIGGER_DELAY`, calibrated against dmg_sound 09's 2-cycle phase sweep).
- **The user's volume control is NOT in this component, and must never be.** NR50's master
  volume is emulated behaviour the suites measure and a game can read back from `$FF24`;
  playback volume is a host-side attenuation applied in the cpal device callback, past the
  end of emulation. Everything in this page is identical at every level. See
  `sound::output_volume` and [audio.md §6b](../audio.md).
- **NRx1 length writes mask the duty bits** (`v & 0x3F`) — a duty-carrying write like
  `NR11=0x80` used to underflow (debug-mode panic).
- **NR52 power-off now actually zeroes NR10–NR51** and the channel-status bits; the old
  clear-loop ran after `on = false` and was silently a no-op.
- **A powered-off APU keeps emitting (silent) samples at the full rate** instead of
  stalling the stream — hardware keeps driving the DACs, and a stalled stream starves
  the host audio queue and breaks the samples==cycles×rate/4194304 invariant the
  harness checks.
- **DAC-disable interplay** is partially handled (clearing the upper 5 bits of `NRx2`
  disables the channel, [`sound.rs:807`](../../src/sound.rs)).
- **Cosmetic:** two `unused variable` warnings in release builds at
  [`sound.rs:1013`,`1047`](../../src/sound.rs) (`err`/`e` used only inside a
  `#[cfg(debug_assertions)] eprintln!`).

## 5. Known quirks and edge cases

- **Trigger behaviour (`NRx4` bit 7).** A trigger restarts the channel: reloads a zero
  length (with the first-half quirk below), reloads the frequency timer and envelope,
  resets the sweep unit's shadow copy/timer/enable, performs an immediate sweep
  calculation + overflow check when shift > 0, and **cannot enable a channel whose DAC
  is off** — all modelled (dmg_sound 02/03/04 pass). The shadow reload is **not
  instantaneous**: for `SWEEP_RESTART_HOLD` (12 T) afterwards the overflow check reads the
  shadow as zero, so a 128 Hz sweep event landing in that window cannot kill the channel
  (`channel_1_sweep_restart_2`).
- **Length-counter "extra clock" quirks** (`Sound::apply_nrx4_length`): enabling a
  previously-disabled length counter while the sequencer's next step won't clock length
  decrements it immediately (disabling the channel on a no-trigger zero), and a trigger
  reloading a zero counter in that phase loads max−1 — modelled (dmg_sound 03 passes).
- **Length expiry disables the channel** (the NR52 status flip-flop): reloading or
  disabling length afterwards does not re-enable it; only a trigger does — modelled.
- **Powering off the APU** (`NR52` bit 7 = 0) zeroes `0xFF10–0xFF25` (while-on clear,
  then the flag flips) and blocks non-length register writes while off; on DMG the
  length counters survive power-off and NRx1 length loads still land while off.
  Powering **on** resets the frame sequencer to step 0 while the 8192-cycle DIV divider
  keeps running — modelled (dmg_sound 07/08/11 pass).
- **Sweep negate quirk**: clearing NR10's negate bit after any negate-mode calculation
  since the last trigger disables the channel — modelled (dmg_sound 05 passes).
- **Wave-RAM access while channel 3 is on — modelled** (dmg_sound 09/10/12 pass).
  On DMG every `$FF30–$FF3F` address maps to the byte CH3 is currently reading, but
  only during the 2 T-cycles after the channel's own fetch (`WaveChannel::since_fetch`
  + `dmg_access_byte`); outside the window reads return `$FF` and writes are dropped.
  On CGB (`dmg_mode == false`) the access always maps to the current byte. Retriggering
  CH3 on DMG while a fetch is imminent corrupts wave RAM's first bytes (see §4).
- **Noise LFSR width** (7-bit vs 15-bit) via `NR43` bit 3 — modelled
  ([`sound.rs:371`](../../src/sound.rs)).

## 6. Expected behaviours as tests

> **Coverage today: the headless capture harness exists.**
> [`testharness/audiocapture.rs`](../../testharness/audiocapture.rs) records every
> sample the APU produces with a device-free `AudioPlayer`, writes WAVs, analyses
> pitch/level/continuity/rate, runs Blargg `dmg_sound` and reads its `$A000`
> verdict (**12/12 pass**, pinned explicitly in the harness so a pass→fail
> regression is named, not just a hash change), and pins a determinism hash in
> `testharness/audio_baseline.txt`.
> Single-ROM TDD mode: `AUDIO_ROM=path/to/rom.gb cargo test --release --test
> audiocapture` prints one ROM's verdict. `src/sound.rs` unit tests additionally
> cover pitch, rate-honesty, and the playback queue policy.

1. **Frame-sequencer rates.** Over one second the length counter steps 256×, the envelope
   64×, and channel-1 sweep 128×. *(Covered indirectly: dmg_sound 07 measures the
   length/sweep periods and their sync.)*
2. **Length auto-stop.** A channel triggered with a finite length and length-enable set
   goes silent after exactly `length` 256 Hz ticks. *(Covered: dmg_sound 02 passes.)*
3. **Trigger reloads a zero length** to its max (`64`, or `256` for the wave channel).
   *(Covered: dmg_sound 02/03 pass.)*
4. **Envelope fade.** With an increasing envelope, volume steps up by 1 every `period`
   envelope ticks until 15, then holds. *(Not covered directly.)*
5. **Sweep overflow disables channel 1.** A sweep configuration whose next frequency
   exceeds 2047 clears `NR52` bit 0 (channel-1 status). *(Covered: dmg_sound 04/06 pass.)*
6. **`NR52` status bits.** Reading `0xFF26` reflects which channels are currently active;
   powering off zeroes `NR10`–`NR25`. *(Covered: dmg_sound 01/08/11 pass.)*
7. **Register read-back masks.** Each `NRxx` read ORs the correct "unused bit" mask.
   *(Covered: dmg_sound 01 passes — see `NR_READBACK_MASKS`.)*
8. **Wave RAM access window while playing.** On DMG, reads/writes to `0xFF30–0xFF3F`
   while channel 3 is enabled hit the channel's current byte only within 2 T-cycles of
   its own fetch — reads otherwise return `$FF`, writes are otherwise dropped — and a
   retrigger coinciding with a fetch corrupts the first bytes of wave RAM. *(Covered:
   dmg_sound 09/10/12 pass; the save/load survival of the window state is covered by
   `sound::tests::wave_window_state_survives_save_load`.)*
9. **The length-counter extra-clock edge case.** Matches hardware. *(Covered:
   dmg_sound 03 passes — `apply_nrx4_length`.)*
10. **Web build is silent** (module compiled out). *(Documented behaviour, not a bug.)*
11. **PCM12 / PCM34 (`$FF76`/`$FF77`, CGB only).** Each register reports the *live*
    4-bit digital output of two channels — CH1/CH3 in the low nibble, CH2/CH4 in the
    high nibble — sampled at the exact cycle of the read, and zero for a channel whose
    channel-enable or DAC is off. *(Covered: `sound::tests::pcm_registers_report_each_channel_digital_output`,
    and every SameSuite APU test observes the APU through these.)*
12. **The frame sequencer is clocked by DIV, not by a free-running divider.** It steps
    on the falling edge of system-counter bit 12 (bit 13 at double speed), so writing
    `DIV` while that bit is set clocks it immediately. *(Covered:
    `sound::tests::a_div_write_clocks_the_frame_sequencer`; SameSuite `div_write_trigger`
    and `div_write_trigger_volume`.)*
13. **Powering the APU on while the DIV-APU bit is set skips the first event.**
    The sequencer starts at step 0 and one event passes doing nothing — which also
    puts the machine in the "first half" of a length period, so the next NRx4
    length-enable write clocks length itself. *(Covered: SameSuite
    `div_write_trigger_10`, `div_trigger_volume_10`.)*
14. **The APU runs off the base clock.** At CGB double speed it advances at half the
    CPU's tick rate; one emulated second yields the same number of samples either way.
    *(Covered: `sound::tests::double_speed_halves_the_apu_clock`.)*
15. **Square duty is a latch, not a function of the position.** The duty output only
    takes a new value at a frequency-timer expiry, so a freshly powered-on channel
    reads 0 for its whole first pattern position even where the duty table says high;
    an APU power-off resets the duty counters. *(Covered: SameSuite `channel_N_duty`.)*
16. **Trigger delays are measured, not assumed.** A square channel's first duty step is
    `period + 6` T-cycles after the trigger, rounded up to the APU's own free-running
    1 MHz grid; a *re*-trigger of a running channel is 4 T-cycles earlier and does not
    reset the duty position. CH3's first fetch is `period + 4` and lands on wave sample
    **1**, never 0. CH4's first LFSR step is `divisor + (period - divisor)/2`, because a
    trigger reloads only the NR43 divisor and leaves the shift stage half-way through.
    *(Covered: SameSuite `channel_N_delay`, `channel_N_align`, `channel_N_restart`,
    `channel_3_first_sample`, `channel_4_delay`.)*
17. **CH3's sample buffer empties on APU power-off** and is *not* refilled by a trigger,
    so a channel re-triggered after a power cycle must not report the sample it was
    playing before. *(Covered: SameSuite `channel_3_first_sample`.)*
18. **Writing NRx2 does not reload the live volume** — it sets what the next trigger
    will load. Writing it while the channel plays perturbs the running volume instead
    ("zombie mode"), by the CGB revision-E rule in `VolumeEnvelope::write_nrx2`.
    *(Covered: SameSuite `channel_N_volume`.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg `dmg_sound`** (12 sub-tests, **12/12 pass**) | Registers, trigger, length, sweep, envelope, wave-RAM access timing on DMG | Any failure is a regression — the harness pins the `$A000` verdict. |
| **Blargg `cgb_sound`** (not wired) | The CGB APU edge cases | CGB-specific power/length rules — `Sound::dmg_mode` selects per-model behaviour, but the CGB side is untested. |
| **SameSuite `apu`** (61 published tests, **49/61 pass**, was 2/61) | Sample-accurate channel behaviour read back through PCM12/PCM34 | The deep end: trigger and re-trigger timing to the T-cycle, the duty output latch, the DIV-APU's exact phase, the NRx2 zombie glitch, the wave sample buffer, the noise divisor/shift split. Run it with `cargo run --release --example samesuite -- <dir>`; see §8. |
| **Mooneye `acceptance/` (no dedicated APU dir)** | — | Mooneye focuses on timing/PPU/CPU; Blargg is the APU authority. |
| **SameBoy APU tests** | Reference-emulator APU behaviour | Useful cross-check but require SameBoy tooling. |

The `retrio/gb-test-roms` submodule (initialised; wired into CI) includes the Blargg
sound suites. Unlike the CPU suites, `dmg_sound` reports through **cartridge RAM**, not
serial: `$A000` holds the status (`$80` = running, `0` = pass), `$A001–$A003` the
signature `$DE $B0 $61`, and a result string follows at `$A004` — this is how
[`testharness/audiocapture.rs`](../../testharness/audiocapture.rs) reads the verdict
headlessly. See [testing](../testing.md).

## 8. SameSuite: the sample-accurate suite

[SameSuite](https://github.com/LIJI32/SameSuite)'s APU sub-suite is a much deeper test
of this component than Blargg's `dmg_sound`, and it is where the honest number lives:
**65 of the 69 APU ROMs in the shootout's vendored set pass, up from 2** (72/76 across
the whole SameSuite directory, which also holds its dma, ppu and sgb tests). Measured by
`make test-external`, which is CI-gated against `testharness/samesuite_baseline.txt` and
red on a change in *either* direction.

**Run it headlessly:**

```sh
cargo run --release --example samesuite -- <dir-with-samesuite-roms> [--dump]
```

The runner reads SameSuite's own `RESULT_CODE` at `$CFFE` (`"P"` / `"F"`), written by
the ROM after it compares its measurements against the `CorrectResults` table assembled
into it — so a verdict is the ROM's judgement, never ours, and no screenshot comparison
is involved. `--dump` prints the raw measurement table so a failure can be read against
the expectations in the test's `.asm` source. **The APU must be installed** (the runner
attaches a device-free player): with no player `Sound::do_cycle` returns immediately and
the APU never advances at all.

**How the suite observes the APU.** Almost every test writes some registers, waits an
exact number of M-cycles, and reads `PCM12`/`PCM34` — the CGB-only registers that expose
each channel's live 4-bit digital output. 57 of the 61 are unreachable without them.
Several run at CGB **double speed** deliberately, because that halves the CPU's cycle
relative to the APU's and so resolves timing to 2 T-cycles instead of 4.

**Constants calibrated against it** (all in `src/sound.rs`, all measured rather than
guessed — each test steps its read point one cycle at a time, so a wrong constant is
visible as a whole column shifting):

| Constant | Value | Fixed by |
|---|---|---|
| `SQUARE_TRIGGER_DELAY` | 6 T | `channel_N_delay` |
| `SQUARE_TRIGGER_ALIGN` | grid phase 0 | `channel_N_align` (double speed) |
| `SQUARE_RESTART_SPEEDUP` | 4 T | `channel_N_restart` |
| `WAVE_TRIGGER_DELAY` | 4 T | `channel_3_delay`, and still Blargg `dmg_sound` 09/10/12 |
| `NOISE_TRIGGER_DELAY` | 2 T | `channel_4_delay` |
| `NOISE_TRIGGER_ALIGN` | grid phase 0 | `channel_4_align` |
| a trigger halves **both** noise counters, floored at one divisor tick | `NoiseChannel::trigger_divisor` | `channel_4_frequency_alignment`, `channel_4_equivalent_frequencies` |
| noise re-trigger costs one extra whole sample | +1 period | `channel_4_lfsr_restart`, `_fast` |
| `sweep_calc_delay_for(shift)` | `(shift + 1)` M-cycles, `+1` more on a trigger | `channel_1_sweep`, `channel_1_sweep_restart` |
| `SWEEP_RESTART_HOLD` | 12 T (the sweep shadow reads zero for three M-cycles after a trigger) | `channel_1_sweep_restart_2` |
| envelope period counter runs on frame-sequencer step | 6 (the step it schedules lands on 7) | `channel_N_volume_div`, `channel_N_nrx2_speed_change` |

**Still failing, and why** — these are scoped, not half-built. Full per-fix record with
citations: [`../conformance-notes.md`](../conformance-notes.md) §10-18.

* `channel_4_freq_change` (1) — **left red on evidence.** SameSuite's own `apu/README.md`
  says SameBoy, the emulator the suite exists to develop, fails exactly this test and
  `channel_1_sweep_restart_2`; the ROM's own header says "the logic behind it is still
  unclear". Diagnosis for whoever tries: our single `period` countdown cannot move the
  *next* LFSR step after an NR43 write (it fires ≤ 16 T later), while the ROM measures
  ~56–70 T — a two-stage counter keeping its position and counting on to the new
  `2^shift`. That is a `NoiseChannel` restructure, a save-state format bump and a
  re-calibration of the ten green CH4 rows, for one row nothing passes.
* `channel_1_freq_change_timing-{A,cgb0BC}` (2) — **`-cgbDE`, the machine we model,
  passes** since the frequency-reload latch landed
  ([`../conformance-notes.md`](../conformance-notes.md), 2026-08-14). The other two are
  not a revision knob we are missing, which an earlier note here claimed: **`-A` is the
  Game Boy Advance** (every CGB stepping tag in this suite is spelled `cgb…`, and
  `apu/README.md` names the AGB line as the family still to be verified) — a console this
  emulator does not have. **`-cgb0BC` differs in the PCM12 *read-back*, not the frequency
  timer**: it is the "PCM12/PCM34 report a glitched amplitude if read in the same M-cycle
  they change" quirk of revisions C and older, which the same README lists as needing to
  *"be understood, tested and documented"* by anyone at all. Splitting `CgbRevision` was
  tried on paper and buys neither row.
* `channel_1_sweep_restart` (1) — down to a **single cell of 144**: round 1's last
  subtest, the duty phase of a re-trigger that immediately follows a sweep frequency
  change. It is a **square-timing** question, not a sweep one — the re-trigger's first
  duty step lands 66 T after the write and hardware's is at 69 or later. Both levers are
  already pinned by rows that pass: `SQUARE_RESTART_SPEEDUP = 0` costs four
  (`channel_{1,2}_restart`, `channel_{1,2}_restart_nrx2_glitch`) and
  `SQUARE_TRIGGER_DELAY = 8` costs most of the square suite. A correct fix needs a
  *conditional* re-trigger reload rule and the suite carries no evidence for the
  condition. Full measurement: [`../conformance-notes.md`](../conformance-notes.md) §18.

  (`channel_1_sweep_restart_2` **passes** since the restart hold landed — §17. The note
  that used to sit here, blaming a two-M-cycle DIV-APU phase error, was wrong.)

**The `-cgb0B` / `-cgbB` / `-cgb0BC` / `-A` variants name a CPU CGB revision in the
filename, and the runner honours it.** The APU's extra-length-clocking rule changed in CPU
CGB C, and `-cgb0` and `-cgbB` contradict each other, so *no single machine can pass both*
— a runner that ignores the name can score at most four of the five however accurate it
is. `terminalgb::sound::CgbRevision` (default `CgbC` — C/D/E, and what every DMG does) models
the difference, `examples/samesuite.rs::revision_for` picks it from the suffix exactly as
the `testroms` harness picks `@dmg`/`@cgb` from a cartridge header, and
`$GB_CGB_REVISION=cgb0|cgbB|cgbC` exposes the same knob to a frontend. The ROM still
judges itself; the knob picks only which machine it is judging, and `dmg_mode` pins the
modern rule so a DMG game can never see the bug. `-cgb0BC` / `-cgbDE` are a *different*
revision axis and are not modelled yet (see the failing list above).

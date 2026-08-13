# APU — sound (audio processing unit)

> Source: [`src/sound.rs`](../../src/sound.rs) · Related: [MMU](mmu.md) · [Frontends](frontends.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`303f15f`](https://github.com/Alchemy86/gameboy/tree/303f15fe64435d9724d20e2ac651d480e8a7eaca) · regenerate with `make docs-links`, verify with `make docs-check`.

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
([`src/gameboy.rs:103`](https://github.com/Alchemy86/gameboy/blob/main/src/gameboy.rs#L103)).

### Honest approximations & caveats

- **No sound on web.** The whole module is `#[cfg(not(target_arch = "wasm32"))]`
  ([`src/lib.rs:17`](https://github.com/Alchemy86/gameboy/blob/main/src/lib.rs#L17)), so the browser build reads `0xFF10–0xFF3F` as
  `0xFF` and is silent. The terminal frontend also runs without audio.
- **Register accuracy is measured: Blargg `dmg_sound` passes 9 of 12 subtests**
  (01–08 and 11), per the offline audio harness
  ([`testharness/audiocapture.rs`](../../testharness/audiocapture.rs)), which reads the
  suite's memory-mapped verdict at `$A000`. This was 1/12 before the APU accuracy pass:
  the register-shadow read-back model (`regs` + `NR_READBACK_MASKS`), catch-up-on-access
  synchronisation (`Sound::catch_up`, called from the MMU before every APU register
  access so NR52/length/sweep state is never stale), length-expiry disabling the channel,
  the NRx4 extra-length-clocking quirks (`apply_nrx4_length`), full sweep semantics
  (always-reloaded shadow/timer/enable on trigger, negate-used tracking, the NR10
  negate-exit disable), DAC-gated triggers, and the power-on frame-sequencer reset.
- **The remaining three failures (09/10/12, the wave-RAM access quirks) are a documented
  non-goal**: they sweep a CPU access across a ~2-T-cycle window around the wave
  channel's sample fetch (read-returns-current-byte / retrigger corruption / write
  lands only in the window) and CRC the result. That needs a **T-cycle-stepped APU**
  with a modelled wave sample-buffer/fetch pipeline plus cycle-exact CPU access
  positions inside instructions — the same class of restructure as the cycle-stepped
  PPU the video non-goals (oam_bug, mode-3 timing) are waiting on. Half-modelling it
  would be guesswork; stopped deliberately, like the CPU crew's non-goals.
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
  is off** — all modelled (dmg_sound 02/03/04 pass).
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
- **Wave-RAM access while channel 3 is on.** Reads return the byte currently being
  played and writes are blocked — a simplification. The real DMG behaviour is a
  ~2-T-cycle access window around the channel's fetch; **not modelled** (the 09/10/12
  non-goal above).
- **Noise LFSR width** (7-bit vs 15-bit) via `NR43` bit 3 — modelled
  ([`sound.rs:371`](../../src/sound.rs)).

## 6. Expected behaviours as tests

> **Coverage today: the headless capture harness exists.**
> [`testharness/audiocapture.rs`](../../testharness/audiocapture.rs) records every
> sample the APU produces with a device-free `AudioPlayer`, writes WAVs, analyses
> pitch/level/continuity/rate, runs Blargg `dmg_sound` and reads its `$A000`
> verdict (**9/12 pass** — 01–08 + 11; 09/10/12 are the cycle-stepped-APU
> non-goal), and pins a determinism hash in `testharness/audio_baseline.txt`.
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
8. **Wave RAM write-block while playing.** Writes to `0xFF30–0xFF3F` are ignored while
   channel 3 is enabled. *(Approximated; the exact DMG access window is the 09/10/12
   non-goal.)*
9. **The length-counter extra-clock edge case.** Matches hardware. *(Covered:
   dmg_sound 03 passes — `apply_nrx4_length`.)*
10. **Web build is silent** (module compiled out). *(Documented behaviour, not a bug.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg `dmg_sound`** (12 sub-tests, **9/12 pass**: 01–08 + 11) | Registers, trigger, length, sweep, envelope on DMG | 09/10/12 = the wave-RAM timing non-goal (§4). |
| **Blargg `cgb_sound`** (not wired) | The CGB APU edge cases | CGB-specific power/length rules — `Sound::dmg_mode` selects per-model behaviour, but the CGB side is untested. |
| **Mooneye `acceptance/` (no dedicated APU dir)** | — | Mooneye focuses on timing/PPU/CPU; Blargg is the APU authority. |
| **SameBoy APU tests** | Reference-emulator APU behaviour | Useful cross-check but require SameBoy tooling. |

The `retrio/gb-test-roms` submodule (initialised; wired into CI) includes the Blargg
sound suites. Unlike the CPU suites, `dmg_sound` reports through **cartridge RAM**, not
serial: `$A000` holds the status (`$80` = running, `0` = pass), `$A001–$A003` the
signature `$DE $B0 $61`, and a result string follows at `$A004` — this is how
[`testharness/audiocapture.rs`](../../testharness/audiocapture.rs) reads the verdict
headlessly. See [testing](../testing.md).

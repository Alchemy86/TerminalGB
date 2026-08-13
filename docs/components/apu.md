# APU — sound (audio processing unit)

> Source: [`src/sound.rs`](../../src/sound.rs) · Related: [MMU](mmu.md) · [Frontends](frontends.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`9162be3`](https://github.com/Alchemy86/gameboy/tree/9162be32ec660ef0c10a9e4164b9b5f6792950a8) · regenerate with `make docs-links`, verify with `make docs-check`.

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
- **The subtle length-counter edge case is NOT modelled.** Only "trigger reloads length
  when the counter is zero" is handled ([`sound.rs:117`](../../src/sound.rs)); the
  hardware "extra length clock when enabling length in the first half of a frame-sequencer
  period" is absent — a classic `cgb_sound`/`dmg_sound` failure.
- **DAC-disable interplay** is partially handled (clearing the upper 5 bits of `NRx2`
  disables the channel, [`sound.rs:807`](../../src/sound.rs)).
- **Cosmetic:** two `unused variable` warnings in release builds at
  [`sound.rs:1013`,`1047`](../../src/sound.rs) (`err`/`e` used only inside a
  `#[cfg(debug_assertions)] eprintln!`).

## 5. Known quirks and edge cases

- **Trigger behaviour (`NRx4` bit 7).** Writing a trigger restarts the channel: reloads
  length if zero, reloads the frequency timer, re-arms the envelope, and (channel 1)
  performs an immediate sweep overflow check that can disable the channel instantly
  ([`sound.rs:224`](../../src/sound.rs)).
- **Length-counter "extra clock" edge case.** Enabling the length counter in the first
  half of a sequencer step gives one extra length tick on hardware — **not modelled** (§4).
- **Powering off the APU** (`NR52` bit 7 = 0) zeroes `0xFF10–0xFF25` and blocks most
  register writes while off ([`sound.rs:785`,`902`](../../src/sound.rs)) — modelled.
- **Wave-RAM access while channel 3 is on.** Reads return the byte currently being played,
  and writes are blocked while playing ([`sound.rs:888`,`888`](../../src/sound.rs)) — a
  simplified version of the (model-dependent, timing-sensitive) hardware behaviour.
- **Noise LFSR width** (7-bit vs 15-bit) via `NR43` bit 3 — modelled
  ([`sound.rs:371`](../../src/sound.rs)).

## 6. Expected behaviours as tests

> **Coverage today: none.** (Audio is also hard to unit-test without a headless
> `AudioPlayer` capture stub — worth building.)

1. **Frame-sequencer rates.** Over one second the length counter steps 256×, the envelope
   64×, and channel-1 sweep 128×. *(Not covered.)*
2. **Length auto-stop.** A channel triggered with a finite length and length-enable set
   goes silent after exactly `length` 256 Hz ticks. *(Not covered.)*
3. **Trigger reloads a zero length** to its max (`64`, or `256` for the wave channel).
   *(Not covered.)*
4. **Envelope fade.** With an increasing envelope, volume steps up by 1 every `period`
   envelope ticks until 15, then holds. *(Not covered.)*
5. **Sweep overflow disables channel 1.** A sweep configuration whose next frequency
   exceeds 2047 clears `NR52` bit 0 (channel-1 status). *(Not covered.)*
6. **`NR52` status bits.** Reading `0xFF26` reflects which channels are currently active;
   powering off zeroes `NR10`–`NR25`. *(Not covered.)*
7. **Register read-back masks.** Each `NRxx` read ORs the correct "unused bit" mask
   (e.g. `NR52` reads back with bits 4–6 set). *(Not covered.)*
8. **Wave RAM write-block while playing.** Writes to `0xFF30–0xFF3F` are ignored while
   channel 3 is enabled. *(Not covered.)*
9. **The length-counter extra-clock edge case.** Matches hardware. **Expected to FAIL —
   not modelled.** *(Not covered; known-divergent — this is the `cgb_sound` failure.)*
10. **Web build is silent** (module compiled out). *(Documented behaviour, not a bug.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg `dmg_sound`** (12 sub-tests) | Registers, trigger, length, sweep, envelope on DMG | Any of the §5 edge behaviours. |
| **Blargg `cgb_sound`** (repo screenshot: *failing*) | The CGB APU edge cases | The repo's known APU failure — points straight at the length/trigger edge cases (§4/§5). |
| **Mooneye `acceptance/` (no dedicated APU dir)** | — | Mooneye focuses on timing/PPU/CPU; Blargg is the APU authority. |
| **SameBoy APU tests** | Reference-emulator APU behaviour | Useful cross-check but require SameBoy tooling. |

The declared `retrio/gb-test-roms` submodule includes the Blargg sound suites but is
**uninitialised**. Audio pass/fail is reported over the serial port like other Blargg
tests — see [Serial](serial.md) and [testing](../testing.md).

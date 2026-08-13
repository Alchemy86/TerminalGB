# Audio — the whole sound pipeline, and why it sounded rough & fast

> **Scope.** This page is the end-to-end story of **sound as you hear it**: how a
> square wave inside the APU becomes samples in your speakers, where quality is
> won or lost, and the investigation that fixed a "rough and slightly fast"
> report on Pokémon Blue. For the **register-level channel model** (duty, sweep,
> envelope, LFSR, the frame sequencer) see the component page
> **[docs/components/apu.md](components/apu.md)** — this page picks up where that
> one ends: at the sample buffer.

- **Code:** [`src/sound.rs`](../src/sound.rs) — the APU *and* the cpal player.
- **Feature-gated:** host playback is behind the `audio` cargo feature (off by
  default). The APU itself always runs; only speaker output is gated. See
  [Build & run](#build--run).
- **Probe your machine:** [`examples/audio_probe.rs`](../examples/audio_probe.rs).
- **Tests:** [`src/sound.rs`](../src/sound.rs) `mod tests` — pitch + sample-rate
  checks that run under a plain `cargo test` (no audio device needed).

---

## 1. The signal path at a glance

```
 emulated CPU cycles (4.194304 MHz)
        │  do_cycle(n)                          src/sound.rs
        ▼
 ┌──────────────┐  band-limited synthesis (add_delta / last_amp)
 │ 4 channels   │  each owns a blip_buf::BlipBuf
 │ sq sq wave n │
 └──────┬───────┘
        │  blip.set_rates(4_194_304, sample_rate)   ← resample happens HERE
        ▼
 ┌──────────────┐  play_samples(): read 2000-sample blocks, apply NR50/NR51
 │  L / R mix   │  panning + master volume → interleaved (f32, f32)
 └──────┬───────┘
        │  AudioPlayer::play()
        ▼
 ┌──────────────┐  Arc<Mutex<Vec<(f32,f32)>>>  — the hand-off queue
 │ CpalPlayer   │  producer = emulator thread, consumer = cpal callback
 └──────┬───────┘
        │  cpal_thread(): copy into device frames, drain what was used
        ▼
   sound card  (plays at its real hardware rate)
```

Two clocks meet in the queue: the **producer** (the emulator, running as fast as
the frontend paces it) and the **consumer** (the sound card, running at its fixed
crystal rate). **Everything about "rough" and "fast" lives in how those two
clocks line up.**

---

## 2. The key idea: pitch is correct for *any* rate — if the number is honest

The APU synthesises in **emulated time**. `blip_buf` then resamples from the CPU
clock to an output rate:

```rust
blip.set_rates(CLOCKS_PER_SECOND as f64, sample_rate as f64); // 4_194_304 → e.g. 44_100
```

So for **every emulated second** (4,194,304 cycles) the APU emits **exactly
`sample_rate` samples**. `Sound::set_player` reads the rate back from the player
and retunes all four channels to it ([`sound.rs`](../src/sound.rs),
`set_player`). The consequence is worth stating plainly:

> **Whatever rate the device negotiates, pitch is correct — provided the number
> we report is the rate the stream truly runs at.** If we *say* 44100 and the
> card *plays* at 48000, every tone is stretched up by 48000/44100 = **+8.8 %**
> (sharp) *and* the queue starves (rough). That specific mismatch is the classic
> "fast and rough" signature — so the investigation's first job was to rule it
> out.

We verified pitch directly with a tone test (now
[`sound.rs` `tone_pitch_is_correct`](../src/sound.rs)): program channel 1 to the
register value for A4 and measure the produced stream by zero-crossings.

```
channel1 freq reg x=1750  expected=439.84 Hz
MEASURED pitch = 441.5 Hz   (0.4 % — zero-crossing noise on a 12.5 % duty wave)
```

**Synthesis and resampling are pitch-accurate.** So any perceived "fast" is *not*
the APU — it is wall-clock pacing (see §5).

---

## 3. Device negotiation — the confirmed bug

`CpalPlayer::new` must pick an output configuration. The original code walked the
device's supported configs and **broke on the first `F32` config, suitable or
not**:

```rust
for f in supported_configs {
    if f.sample_format() == cpal::SampleFormat::F32 {
        if f.min_sample_rate() <= wanted && wanted <= f.max_sample_rate() {
            supported_config = Some(f.with_sample_rate(wanted));
        } else {
            supported_config = Some(f.with_max_sample_rate()); // ← 96k / 192k / 384k!
        }
        break; // ← first F32 config, whatever it is
    }
}
```

Two real failure modes, both confirmed with
[`examples/audio_probe.rs`](../examples/audio_probe.rs) on this machine
(ALSA/PipeWire `default`):

```
device DEFAULT config: channels: 2, sample_rate: 44100, F32     ← the sane choice
[96] fmt=F32 ch=1  min=1 max=384000                             ← what the old code took
[97] fmt=F32 ch=2  min=1 max=384000
 ...
```

1. **Wrong channel count.** The first enumerated `F32` range here is **`ch=1`
   (mono)**. The old loop would build a **mono** stream and drop the right
   channel entirely — even though the device's advertised default is stereo.
2. **Wild sample rate on other hardware.** On any device whose first `F32` range
   does not span 44100, `with_max_sample_rate()` opens the stream at **96/192/384
   kHz**. Pitch would still be *correct* (the rate is reported honestly and blip
   matches it), but at 4–8× the sample throughput for no benefit — wasted CPU,
   and a real "fast + starved" risk if that huge rate is ever mis-reported.

**The fix** (in [`sound.rs`](../src/sound.rs), `CpalPlayer::new`): prefer the
device's **advertised default config** when it is `F32` — it carries the real
channel count and a rate the hardware genuinely runs at, so there is no host-side
resample and no channel surprise. Only if the default is missing or non-`F32` do
we scan every `F32` range and keep the best, **preferring ranges that span 44100
and then the most channels**. After the fix the probe reports:

```
>>> CpalPlayer negotiated sample_rate = 44100     (stereo, from the device default)
```

> **On the captain's machine the negotiated rate was 44100, matching the real
> stream rate** — so the negotiation bug was *not* the pitch culprit there (it
> was silently costing the right channel). The pitch culprit is §5.

---

## 4. The hand-off queue — roughness and latency

Producer and consumer share `Arc<Mutex<Vec<(f32,f32)>>>`:

- **`CpalPlayer::play`** pushes 2000-sample blocks onto the tail.
- **`cpal_thread`** copies from the head into each device callback and drains what
  it used. If the queue is **short**, the remaining device frames are filled with
  **silence** — an underrun, heard as a click/crackle. If the queue **grows**, you
  hear **latency** (input-to-sound lag), the "aged/laggy" feel.

The original cap was **`MAX_BUFFER_SIZE = 44100` — a full second** of audio before
anything was dropped. That is a lot of allowable lag, and when the producer
overruns (see §5) it lets a whole second accumulate, then dumps it in one abrupt
`drain`, i.e. a large audible skip.

**The fix:** bound the queue at **4 output blocks (~180 ms)** — enough slack to
ride frame-timing jitter without underrunning, small enough that lag stays
imperceptible; on overflow the oldest samples are dropped so the stream snaps
back to low latency instead of drifting ever further behind
([`sound.rs`](../src/sound.rs), `CpalPlayer::play`).

> Note this is *damage control*, not the cure. With correct pacing (§5) the queue
> barely moves and neither underruns nor overflows matter. The tighter cap simply
> guarantees the failure is graceful if the two clocks ever drift.

---

## 5. The real cause of "fast": frame pacing, not audio

Audio production is driven by emulation: the frontend calls `Gameboy::frame()`
(one DMG frame) on a timer, and `frame()` runs `do_cycle` — which feeds the APU.
So **however fast the frontend paces frames, the APU produces samples that much
faster.**

The DMG refreshes every **70224 cycles at 4.194304 MHz = 59.7275 Hz**, i.e. one
frame every **16.7427 ms**. The desktop frontend paced at **16 ms**:

| pace | fps | vs real 59.7275 Hz |
|---|---|---|
| `16 ms` (old) | **62.50** | **+4.64 % fast** |
| `16_743 µs` (fixed) | 59.7265 | −0.002 % |

At 62.5 fps the emulator emulates 4.64 % more cycles per wall-second, so the APU
produces **~46,145 samples/s while the sound card drains 44,100/s**. The extra
~2,000 samples/s pile into the queue until the cap, then get dropped — **~21 tiny
skips a second**. That is *exactly* "rough": grainy, aged. And because content is
being skipped and the game logic itself runs 4.64 % fast, the music plays
**slightly fast in tempo** — *exactly* the captain's ear.

**One cause, both symptoms.** The fix is to pace at the true frame period
([`src/gameboy.rs`](../src/gameboy.rs), `render_desktop`):

```rust
// 70224 cycles / 4.194304 MHz = 16.7427 ms per frame (59.7275 Hz)
let frame_duration = std::time::Duration::from_micros(16_743); // was 16 ms = 62.5 fps
```

### ⚠️ The terminal frontend has the same disease (owned elsewhere)

[`src/screen/tui.rs`](../src/screen/tui.rs) drives `frame()` on a **5 ms** tick
(`tick_rate`), i.e. it targets **200 fps** and runs as fast as the terminal can
redraw — uncapped, and typically **faster than 59.73 Hz**. That frontend is owned
by another crew; **this change does not touch it.** The recommended fix is
identical: gate `frame()` on the real 16.743 ms period (a frame budget), so audio
inherits true speed there too.

---

## 6. What is genuine DMG character (do **not** "fix" this)

The Game Boy APU is authentically lo-fi; the following are **correct**, not
defects, and should be preserved:

- **4-bit channels.** Volume is a 0–15 envelope; square/noise are hard-edged.
  Quantisation buzz is real hardware.
- **Two square waves + one 32-step 4-bit wavetable + an LFSR noise channel.** No
  more. Chiptune texture is the point.
- **Band-limited but still bright.** `blip_buf` removes *aliasing*, not the
  console's inherent brightness.

The bugs above (mono downmix, 62.5 fps pacing, 1-second queue) made it sound
*worse than* a real DMG — muffled to one channel, sped up, and skippy. Fixing them
makes it sound like a DMG, which is the goal. Resist "improving" the 4-bit grit
into something smooth and inauthentic.

An honest remaining gap (tracked in [apu.md](components/apu.md), not fixed here):
the length-counter "extra clock" edge case is unmodelled, so Blargg
`dmg_sound`/`cgb_sound` do not fully pass. That is APU *accuracy*, not the
playback *quality* this page is about.

---

## 7. Build & run

Host playback is behind the `audio` feature (off by default so a fresh clone
builds with **no** ALSA/system audio dependency):

```bash
# Desktop, with sound:
cargo run --release --features audio --example desktop -- path/to/game.gb

# On Linux, --features audio needs ALSA dev headers:
sudo apt install libasound2-dev        # Debian/Ubuntu
sudo dnf install alsa-lib-devel        # Fedora
```

On a box with only the runtime library (`libasound.so.2`, no `-dev`), you can
build by pointing `pkg-config` at a hand-written `alsa.pc` and symlinking
`libasound.so → libasound.so.2` — see the project `CLAUDE.md`.

`cargo build` / `cargo test` / the **wasm** build all work **without** the
feature; audio is compiled out entirely on `wasm32`.

---

## 8. Measure it yourself

**What rate/channels does my machine negotiate?**

```bash
cargo run --features audio --example audio_probe
# → device DEFAULT config, every supported config, and the negotiated rate.
```

**Is the synthesis pitch-accurate and rate-honest?** (no audio device needed):

```bash
cargo test --lib sound
# tone_pitch_is_correct        — a programmed 440 Hz tone comes out at ~440 Hz
# output_matches_reported_rate — N emulated seconds yield ~N×rate samples,
#                                and the count tracks the reported rate (44.1/48/22.05 k)
```

**Did I break emulation?** The test-ROM harness must still match baseline:

```bash
cargo test --release --test testroms      # see docs/testing.md
```

---

## 9. Change log (this fix)

| Area | File | Change |
|---|---|---|
| Device negotiation | [`src/sound.rs`](../src/sound.rs) | Prefer the device default `F32` config; otherwise scan all `F32` ranges preferring span-44100 then max-channels. Kills mono downmix and 96/192/384 kHz surprises. |
| Queue discipline | [`src/sound.rs`](../src/sound.rs) | Cap the hand-off queue at 4 blocks (~180 ms) instead of 1 s; bounded, graceful latency. |
| Frame pacing (root cause of "fast") | [`src/gameboy.rs`](../src/gameboy.rs) | Desktop frame period 16 ms → 16.743 ms (62.5 → 59.73 fps). |
| Tests | [`src/sound.rs`](../src/sound.rs) | `tone_pitch_is_correct`, `output_matches_reported_rate` (device-free). |
| Probe | [`examples/audio_probe.rs`](../examples/audio_probe.rs) | Report a machine's real configs + negotiated rate. |

**Not changed on purpose:** the terminal frontend's 5 ms tick (§5, other crew's
file) and APU length-counter accuracy (§6, tracked in apu.md).

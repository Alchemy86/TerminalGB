# Audio — the whole sound pipeline, and why it sounded rough & fast

> **Keys:** `,` / `.` volume · `m` mute · `+` / `-` speed (fast-forward drops samples). [Every control →](controls.md)

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
  [Build & run](#7-build--run).
- **Probe your machine:** [`examples/audio_probe.rs`](../examples/audio_probe.rs).
- **Tests:** [`src/sound.rs`](../src/sound.rs) `mod tests` — pitch + sample-rate
  checks that run under a plain `cargo test` (no audio device needed).
- **In a browser:** the `wasm32` build plays through the Web Audio API instead of
  `cpal`, and everything from §1 to §6 applies to it except the device layer. Its own
  page — including why the desktop's cushion sizing is *wrong* there — is
  **[docs/web-audio.md](web-audio.md)**.

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

## 4. The hand-off queue — the "blocky and broken" regression and its cure

Producer and consumer share a mutex-guarded queue:

- **`CpalPlayer::play`** pushes `OUTPUT_SAMPLE_COUNT`-sample blocks onto the
  tail — audio arrives in lumps, not a smooth stream. This was **2000 samples
  (~45 ms)**; it is now **256 samples (~5.8 ms)**, for the reason in §4b.
- **`cpal_thread`** drains from the head at exactly the sound card's rate.

### Post-mortem: why "~180 ms cap" made it *worse*

An earlier fix bounded the queue at 4 blocks (~180 ms) to kill the 1-second
latency of the original code. The captain then reported the music as **"blocky
and broken"** — a regression. The measured causes (see §5a for the numbers):

1. **The producer ran systematically slow.** Both frontends reset their frame
   timer with `last = Instant::now()` *after* stepping, so every frame period
   silently included the draw/poll/scheduling overhead. The emulator therefore
   produced *fewer* than 44,100 samples per wall second. Under the old regime
   (62.5 fps = +4.6 % overproduction, 1 s of buffer) this deficit was masked;
   with exact-rate pacing and a 180 ms cap it meant the queue **drained to
   empty over and over**.
2. **Underrun emitted zeros.** The callback stepped from the last sample value
   straight to 0.0 — a wideband click — then played ~45 ms of tone when the
   next block landed, then clicked again. A starved producer plus
   step-to-silence underruns *is* "blocky and broken", literally: ~45 ms of
   sound alternating with gaps.
3. **No priming.** Playback started draining the moment the stream opened, so
   the very first frames underran before the queue ever built a cushion.

### The policy that replaced it (`sample_queue::SampleQueue`)

Chosen from simulation (`src/sound.rs`, `mod queue_tests`) and verified on real
hardware (§5a) — not picked as "a nicer buffer number":

| Rule | Why |
|---|---|
| **Prime to `start_fill()` = 1792 samples (~41 ms) before draining** | The cushion's promise: the frontend may drop **one whole frame** without an audible glitch. A dropped frame costs more than a frame of audio — the queue drains continuously, so the gap is the missed frame *plus* the normal 16.7 ms period — so one frame + the measured 8 ms of scheduler jitter is ~41 ms. See §4b for why this used to be 7000 (~159 ms). |
| **On underrun: hold the last sample and fade it (~0.999/sample), then re-prime** | A step to zero is itself a click ("play silence" is SDL's *baseline* behaviour, not the best). Re-priming turns one hiccup into one smooth gap instead of an on/off machine-gun. |
| **Above `high_water()` = 2816 (~64 ms), discard the oldest samples down to `start_fill()` — never wait** | The producer runs *inside* `Cpu::do_cycle`, so a wait is charged to the emulated frame. It used to wait here; see §4c, which is the bug that made the captain's session emulate at 17.93 ms/frame and display 6 fps. |
| **Clip back to `start_fill()`, not to the ceiling** | Draining to the ceiling leaves zero headroom, so the next production burst clips again and every frame sheds a few samples for ever (measured: 668 over a 120 s soak with a perfectly healthy consumer). |
| **Discards before the device's first callback are counted separately** | Opening a cpal stream takes ~150–250 ms, during which the emulator is already synthesising. Dropping that backlog is what makes playback start from *current* audio instead of a fifth of a second behind, and it is not a glitch — `dropped_before_playback`. |
| **Everything is counted** | `underruns`, `held`, `dropped`, depth min/max, and now device-side latency. `GB_AUDIO_STATS=1` logs a line every 5 s; `examples/audio_latency.rs` reports the full path. |

The queue policy itself is device-free (`sample_queue` module) and unit-tested,
including a 5-minute virtual-time replay of the real producer/consumer cadences
with jitter and stalls.

---

## 4b. "Sounds appear just AFTER they should" — the latency fix

The queue fix above removed the stuttering. The captain then reported the next
problem precisely:

> "sounds appear to be playing just AFTER they should. Like a click to scroll
> text, and a moment after I hear the A button sound. And I feel it on every
> action."

That is the classic trade: underruns were cured by holding more samples, and
**every held sample is delay**. So this had to be measured as one thing, not
two. `examples/audio_latency.rs` reports end-to-end latency *and* underruns from
the same run, on a real device, so a change cannot win one by losing the other.

### Measure the whole path, not the part we own

```text
  APU  --block-->  SampleQueue  --callback-->  host/device  -->  speaker
        BLOCK           depth/rate                cpal timestamp
```

The third hop was never measured. cpal reports both `timestamp().callback` (now)
and `timestamp().playback` (when this buffer is heard); the difference is the
host's own latency. Measured on Pokémon Blue, 45 s, as shipped:

| Hop | Latency |
|---|---|
| `SampleQueue` depth | **199.7 ms** (123–251) |
| host/device | **99.5 ms** |
| **end to end** | **299.3 ms** (223–351) |

Two findings fall straight out:

1. **The device buffer was a third of the problem and nobody had looked.**
   `BufferSize::Default` let the host choose, and it chose 1882 frames per
   callback (42.7 ms) with ~99.5 ms in flight. Asking for `BufferSize::Fixed(256)`
   — one line — took that hop from **99.5 ms to 5.9 ms**, with zero underruns.
2. **`HIGH_WATER`, not `START_FILL`, was setting the queue latency.** The
   producer *blocks* at the watermark, so whenever the frame timer runs even
   slightly fast (it did, +0.47 %) the queue parks near the top: 272 ms, not the
   159 ms cushion.

### The 45 ms block was the floor under everything

The queue cannot be smaller than a few producer blocks, so a 2000-sample
(45 ms) block *forces* a six-figure-microsecond cushion. `OUTPUT_SAMPLE_COUNT`
is now **256 samples (5.8 ms)** — comfortably shorter than one emulated frame,
which is the real granularity at which a frame-paced frontend produces audio.
With that floor removed the cushion could be sized against the actual
requirement (§4 table) instead of against the block size.

The synthesis is unchanged by this: the offline harness measures the same RMS
(0.2467), peak (0.267), silence fraction and dominant frequency (440.0 Hz) as
before, and the sample count got *more* accurate (221184 for 5.0157 s, against
an exact 221192 — the tail shortfall fell from ~3000 samples to ~8). Only the
fixture hashes moved, because the block boundaries did.

### A bug the smaller block exposed

The producer's "is the consumer stalled?" test compared the callback counter
against the value seen at the *previous* `play()`. That was sound at 45 ms per
block — consecutive blocks were always more than a callback apart — but at
5.8 ms one emulated frame hands over several blocks back-to-back within
microseconds. No callback can occur between them, the counter is unchanged, and
every block after the first in a frame was misread as "consumer stalled" and
**dropped instead of waited for**: 14539 samples (330 ms of audio) discarded per
90 s with a perfectly healthy device. The test is now elapsed-time based
(50 ms without a callback = genuinely stalled).

### Result, measured

Pokémon Blue, real device, **5-minute** soak:

| | as shipped | after |
|---|---|---|
| queue latency | 199.7 ms | **46.8 ms** (40.3–53.7) |
| device/host latency | 99.5 ms | **5.9 ms** (5.6–6.2) |
| **end to end** | **299.3 ms** (223–351) | **52.7 ms** (46.2–59.8) |
| underruns | 0 | **0** |
| dropped samples | 3187 | **0** |
| residual drift | +0.470 % | **−0.0005 %** |
| frame rate | 59.662 fps | 59.660 fps |

Production stalls over those 5 minutes: **4 frames of 17899 ran more than 1 ms
late** (p99 and p99.9 both 0.0 ms). The worst, 129.6 ms, is at t = 0.20 s —
startup, during priming, before anything is heard. The genuine mid-run stalls
were 30.7, 45.4 and 46.1 ms, and all were absorbed with no underrun; the cushion
has margin but not a great deal, which is the honest cost of 52 ms instead of
299 ms.

### What is *not* fixed

- **The terminal frontend's redraw cost is unmeasured.** Production stalls come
  from whatever the frame loop does besides emulating, and re-encoding a
  160×144 image for a terminal graphics protocol every frame is far more work
  than a GL blit. If the TUI stalls longer than ~41 ms it will cost one
  re-prime (a smooth fade and rebuild, not a click). Measuring it needs a real
  terminal.
- **Production is still bursty at frame granularity.** The cushion is dominated
  by the 16.7 ms of audio a frame emits in one go. Stepping emulation in
  sub-frame chunks would cut the required cushion roughly in half again and
  take end-to-end towards 30 ms. That is the next structural step, and it
  changes the frontends rather than the audio code.

Everything above is overridable for re-measurement:

```text
GB_AUDIO_DEVICE_FRAMES=0     # let the host pick the device buffer (old behaviour)
GB_AUDIO_START_FILL=7000     # queue cushion, samples
GB_AUDIO_HIGH_WATER=12000    # latency ceiling, samples
```

---

## 4c. "The game lags with sound on" — the producer was pacing the emulator

The captain's report, and the one that had been misdiagnosed four times: with
`--features audio` the terminal frontend displayed **6 fps**. Same binary, same
window, same ROM; the only variable was the feature.

| build | `emulate_ms` | display fps |
|---|---|---|
| `--features audio` | **17.93** | **5.98** |
| without audio | **0.29** | **56.75** |

The obvious suspect was the T-cycle-stepped APU that took `dmg_sound` from 9/12
to 12/12 (§ [apu.md](components/apu.md)), whose own estimate had been "+25 µs
per frame" against a measured 17,600 µs. **It was not the APU.**

### What the profile actually said

`examples/audio_frame_cost.rs` times each emulated frame against a real device
and reports wall time *and* `CLOCK_THREAD_CPUTIME_ID` beside it. On Pokémon
Blue, 600 frames, unpaced:

```text
emulate_ms   : avg 17.285  p50 16.520  p90 19.046  max 335.053
  executing  : 0.419 ms/frame — 97.6% of the frame cost was NOT executing
emulated fps : 57.85 (real time = 59.73)
queue        : depth 2817..3072  (high_water 2816)
```

**97.6 % of the frame was blocked, not computing.** The APU's whole per-access
cost is inside that 0.419 ms — 0.26 ms of it is emulation the no-audio build
does too. Removing only the wait (the fast-forward path, which already skipped
it) gave **0.259 ms/frame**, i.e. the no-audio number.

### The mechanism

`Sound::play_samples` → `AudioPlayer::play` runs inside `Cpu::do_cycle`, inside
`Gameboy::frame`. `CpalPlayer::play` waited in a 1 ms poll loop until the queue
had drained to `high_water`, to make the audio crystal the master clock. So:

1. Every frame's wait was charged to `emulate_ms`, which is what the frontend
   measures and what telemetry publishes.
2. The wait's equilibrium was **slower than real time** — it polls at 1 ms
   granularity and only resumes once the queue is at or below the watermark, so
   it overshoots a little every block. Measured unpaced: 57.85 fps against
   59.7275. An emulator that cannot reach real time can never repay frame debt.
3. The frontends pace emulation *themselves* with a deadline accumulator, so a
   frame that costs more than 16.743 ms puts the loop into catch-up. Catch-up
   asks for more frames back-to-back, which fills the queue faster, which makes
   every one of them block. The loop pinned at `MAX_CATCHUP` (12) emulated
   frames per draw: 59.73 / 12 ≈ **5 fps**, and it can never escape.

Two clocks, and the emulator was slaved to the slower one from inside its own
inner loop.

### The fix

The hand-off is now non-blocking, and it is the *whole* of it
(`sample_queue::hand_off`): push the block, and if the queue is above
`high_water()` discard the oldest samples down to `start_fill()`. Latency stays
bounded, the frontend's deadline accumulator is the only clock, and
`emulate_ms` measures emulation again.

Dropping instead of waiting is not a new policy — it is the one already
documented for fast-forward ("drop samples rather than pitch-shift"), now
applied uniformly. `set_fast_forward` consequently needs no special case.

### Measured, before and after

Real device, back to back on this machine.

`examples/audio_soak.rs`, 120 s, quiet 440 Hz tone:

| | before | after |
|---|---|---|
| frames emulated | 59.6620 Hz | **59.7264 Hz** (target 59.7275) |
| underruns | 0 | **0** |
| dropped | 768 | **0** |
| queue depth | 2742..3072 (~65 ms) | 256..2058 (~27 ms) |
| verdict | FAIL | **PASS** |

`examples/audio_latency.rs`, 90 s, Pokémon Blue from cold boot:

| | before | after |
|---|---|---|
| end-to-end latency | 44.8 ms | **36.3 ms** |
| our queue | 38.9 ms | 30.4 ms |
| underruns | 2 | 2 |
| production stalls > 1 ms | 6 / 5342 | **2 / 5369** |
| discarded during playback | 768 | 8,513 |
| discarded before playback | — | 6,400 |

A cold-boot ROM run varies run to run (the boot sequence's stalls are not
deterministic); across four pairs the latency win ranged 1.4–8.5 ms, underruns
were equal or better (6→4, 4→3, 2→2), held samples were consistently lower and
production stalls consistently about half. The *soak* is the deterministic
comparison — same tone ROM, same tree — and it goes FAIL → PASS.

`emulate_ms` with audio on: **17.93 → 0.34** (paced against the real device;
0.26 with no consumer at all). `dmg_sound` is unchanged at **12/12** and every
audio-capture hash still matches its baseline — nothing in synthesis moved.

### The one thing that got worse, and why

Discards during playback rose from 768 to ~8,500 over a 90 s Pokémon Blue run.
Every one of them is in the **first 20 seconds**; from t=20 s to t=90 s the
count is zero, with the queue stable at 1,180..2,180 samples.

They are LCD-off bursts. A frame-paced loop advances the machine until the GPU
raises `updated`, and `Gpu::do_cycle` returns immediately while the LCD is off —
so during Pokémon's boot and its screen transitions one "frame" ran **845,836
cycles** (twelve frames' worth of audio in one go) instead of 70,224. The
emulator genuinely runs fast there, and the queue does the documented thing.
The old code absorbed the same burst by stalling the emulator for ~130 ms
instead, which is the behaviour being removed.

Two honest consequences, both the same coin as the 8.5 ms latency win: the
steady-state queue sits ~8 ms shallower, so a stall substitutes more held
samples before it recovers (157 ms vs 82 ms across the run), while measured
underruns are unchanged. `GB_AUDIO_START_FILL` raises the cushion for anyone who
prefers the other side of that trade.

**Follow-ups this leaves on the table**, in order of value:

1. ~~**Frame-paced frontends run the machine unbounded while the LCD is off.**~~
   **DONE.** It was a frontend pacing bug, not an audio one, and the fix was the
   one predicted here: cap an iteration by cycles as well as by `gpu.updated`.
   `Gameboy::frame` now stops at `FRAME_TICKS_CAP` (70,224 + 64). The margin
   means the `updated` break always arrives first while the LCD is **on**, so
   that path is byte-for-byte unchanged and every baseline — `testroms`,
   `audiocapture`, `savestate` — held. Worth knowing how bad it really was: with
   the LCD off *permanently*, `frame()` never returned at all, which
   `a_frame_with_the_lcd_off_still_lasts_exactly_one_frame` demonstrates by
   timing out when the cap is removed.
2. **Dynamic rate control** (§10) would remove the last of them and the residual
   crystal drift with them, by nudging blip's resample ratio ≤0.5 % from the
   queue depth instead of discarding. It belongs behind an optional
   `AudioPlayer` method so the device-free harnesses keep producing exactly
   `cycles × rate / 4194304` samples and their hashes stay pinned.

### Conformance record

Per the standing convention in [`AGENTS.md`](../AGENTS.md):

1. **Symptom** — with sound enabled the game runs at a fraction of speed and the
   picture crawls; without it, full speed. Telemetry shows `emulate_ms` at or
   above the frame period while every render stage is ~0 ms.
2. **Caught by** — **nothing**, and that is the finding. No test ROM can see it:
   the framebuffer is perfect, and the defect only exists when a host audio
   player is attached, which no ROM-driven suite does. Our own benchmarks missed
   it for the same reason — `render_bench` is headless and reported 0.246 ms.
   Now caught by `testharness/audioperf.rs` (`make test-audio-perf`), which
   attaches the real producer half with no consumer and fails if a frame costs
   more than 4 ms or emulation runs slower than 4× real time. It fails at
   29.6 ms / 0.57× against the old policy.
3. **Contract violated** — not hardware, but the host's: a real Game Boy's APU
   is driven by the machine's clock and cannot stall it. An emulator's audio
   sink is a consumer, never the emulator's clock source, unless it is *the*
   clock (this project paces on a frame deadline instead). Near, *Dynamic Rate
   Control* — <https://bsnes.org/articles/dynamic-rate-control/> — states the
   trilemma directly: block on both clocks and you get both artifacts.
4. **New tests** — `testharness/audioperf.rs` (CI, no device needed);
   `examples/audio_frame_cost.rs` (real device, reports wall vs CPU time per
   frame); `sample_queue::clipping_drains_to_the_cushion_not_to_the_ceiling`.
   Conformance candidates: **emulated speed must not depend on whether an audio
   device is attached**, and **`speed_pct` must stay at 100 whatever `fps`
   does** (already the stated telemetry invariant — it was violated here).

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

### 5a. Pacing must be a *deadline accumulator* — and the measured proof

Setting the period to 16.743 ms is necessary but not sufficient. Both frontends
originally did `last = Instant::now()` after each frame, which adds the
draw/poll/scheduling overhead of every loop iteration to the period — the
emulator runs a few percent **slow**, and audio production is slaved to
emulation speed (§4, cause 1). Both loops now advance a deadline by exactly one
period (`next_frame += FRAME_TIME`) and run bounded catch-up frames when late,
so the *long-run average* is exactly 59.7275 Hz regardless of per-iteration
overhead ([`src/gameboy.rs`](../src/gameboy.rs) `render_desktop`,
[`src/screen/tui.rs`](../src/screen/tui.rs) main loop — pacing only; the
terminal *display* work is owned elsewhere).

**Measured** (`cargo run --release --features audio --example audio_soak -- 120`
on real hardware, 2026-08-13, device negotiated 44100 Hz stereo):

```
frames emulated:      7168 (59.7264 Hz vs 59.7275 target)   → −18 ppm
underruns:            0
dropped (overflow):   0
held (substituted):   11937  — all from the one-time initial prime
queue depth:          min 5731  max 8717  (bounds 0 / 12000)
```

- The −18 ppm frame-rate deficit is the `16_743 µs` rounding of the true
  16 742.706 µs period — **0.0018 % slow**, three orders of magnitude below the
  old +4.64 % bug and far below audibility (a semitone is ~6 %).
- Residual producer-vs-consumer drift, measured from the queue-depth trend over
  100 s: **≈ −5.8 samples/s (−0.013 %)** — the sum of the pacing rounding and
  the host-timer-vs-audio-crystal difference. At that rate the ~7000-sample
  cushion lasts ≈ 20 minutes, after which the queue re-primes **once**: one
  smooth ~160 ms fade, not a blocky cascade, then another ~20 quiet minutes.
  If the drift ever runs the other way, the high-watermark throttle absorbs it
  with zero artifacts. Full dynamic rate control (§10) would remove even this
  and is the documented next step if that rare hiccup bothers anyone.

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

The register-visible accuracy corners (§10 item 4) were then driven green by a
TDD pass against the harness's `$A000` verdict: **Blargg `dmg_sound` passes
12 of 12 subtests** (1/12 → 9/12 via register read-back masks, length-counter
clocking including the extra-clock quirks, trigger semantics, sweep details
including the negate quirk, and NR52 power semantics; 9/12 → 12/12 via the DMG
wave-RAM access window). The wave-window work did *not* need the feared
wholesale T-cycle-stepped APU: the APU *clock* is stepped per CPU memory
access (like the timer), synthesis stays event-driven, and the wave channel
tracks its fetch phase (`since_fetch`) so `$FF30–$FF3F` accesses resolve
against the real 2-T-cycle DMG window. Measured cost ~25 µs/frame with audio
enabled; zero without. Details in [apu.md](components/apu.md).

---

## 6b. Playback volume — and why it is **not** NR50

There was no volume control at all until now; the only lever was the desktop
mixer. There is one now: `,` / `.` move it a rung, `m` mutes, `volume = 50` in
`config.toml` sets where a fresh install starts, and whatever you last set is
remembered across sessions.

**The one thing that must not change: volume is applied on the OUTPUT side.**
The Game Boy has its own master volume — NR50's two 3-bit fields — and turning
*that* down to quieten the speakers is the obvious implementation and the wrong
one. NR50 is emulated behaviour: the accuracy suites measure it (Blargg
`dmg_sound`, SameSuite), a game can read it back from `$FF24`, it is captured in
save states, and it is baked into `testharness/audio_baseline.txt`. Attenuating
there would make the emulator model a console nobody owns.

So the gain lives past the end of emulation, in the cpal device callback
(`cpal_audio::cpal_thread`), applied to samples the APU has already produced and
handed over. `sound::output_volume` holds the level (a global, because the
callback runs on cpal's own audio thread — the same reason `host_status` is one).
Consequences, all deliberate:

| | |
|---|---|
| **Emulation** | bit-identical at every level. Nothing upstream of the device — APU state, register read-back, the offline capture harness, its hashes — can observe it. |
| **Frame path** | untouched. `Sound::play_samples` runs inside `Cpu::do_cycle`, where audio work is charged to the emulated frame (§4c: a blocking hand-off there once cost 17.9 ms of every 16.743 ms frame). The volume costs two relaxed atomic loads **per device callback**, on the audio thread. |
| **Latency** | a change is heard immediately, rather than after the ~40 ms already queued. |
| **Quality** | scaling is `f32`, *before* the device-format conversion. Attenuating an integer sample throws away its low bits — that is where "clean at full, crunchy when quiet" comes from. |

The taper is `gain = (level/100)²`, the usual approximation of an audio-taper
pot: 100 % is unity gain (exactly what the emulator did before this existed),
50 % is −12 dB, 10 % is −40 dB. The default is **50 %**, not full scale — the
report that produced the control was "so bloody loud", so a default nobody has
ever asked to be raised is itself wrong.

Measured through the real sound card (`parec` on the sink's monitor while
`examples/audio_volume.rs` walks the ladder with a full-envelope 440 Hz square):

| level | RMS | ÷ 100 % | expected `(l/100)²` | crest factor | 3rd harmonic / fundamental |
|---:|---:|---:|---:|---:|---:|
| 100 % | 0.24665 | 1.00000 | 1.00 | 1.1196 | 0.0517 |
| 70 % | 0.12014 | 0.48709 | 0.49 | 1.1263 | 0.0521 |
| 30 % | 0.02220 | 0.09000 | 0.09 | 1.1191 | 0.0516 |
| 20 % | 0.00987 | 0.04001 | 0.04 | 1.1195 | 0.0516 |
| 10 % | 0.00247 | 0.01000 | 0.01 | 1.1132 | 0.0514 |
| 0 % / mute | 0.00000 | — | 0 | — | — |

Peak at 100 % is 0.276 — the APU's own full-scale output, so there is no
clipping to introduce; every rung below it is strictly quieter. The crest factor
and harmonic ratio are **constant across the ladder**, which is the objective
form of "a low setting is still clean rather than crunchy": the waveform is
scaled, not degraded. Mute and 0 % are digital silence, and unmuting returns to
the level you left rather than to full.

Re-measure it:

```bash
cargo run --release --features audio --example audio_volume       # walk the ladder
cargo run --release --features audio --example audio_volume -- 30 # sit on one rung
```

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
# queue_tests::*               — virtual-time replay of the hand-off queue:
#                                zero underruns under realistic jitter+stalls,
#                                hold-and-fade on underrun, re-prime behaviour
```

**A moved audio baseline is a console change until proven otherwise.**
`testharness/audio_baseline.txt` pins an FNV hash of the PCM stream, and the hashes are
**model-dependent**: the fixtures boot whichever console the cartridge header asks for, and
`dmg_sound` is a DMG cartridge (`0x0143 == 0x00`), so it boots a DMG. When the DMG mode
landed, both `dmg_sound` hashes moved and the sample counts dropped by exactly 256 (5.8 ms
at 44.1 kHz) because a DMG's boot state differs from the Color console's compatibility mode.
Before re-blessing, run the A/B:

```bash
GB_MODEL=cgb cargo test --release --test audiocapture
```

`GB_MODEL=cgb` forces the pre-DMG-mode console. If the old hashes come back **byte-for-byte**,
the move is the console and nothing else — no synthesis regression. If they do not, something
in `src/sound.rs` changed and you owe the WAVs a listen before blessing anything. This is the
cheapest honest check available and it takes one command; the verdict pinned per fixture
(`dmg_sound` **12/12**) is what actually asserts accuracy, the hash only pins "the samples did
not change".

**Capture what a ROM actually produces** (no audio device needed — this is the
offline audio harness, wired into CI like the video frame-hash harness):

```bash
cargo test --release --test audiocapture
# Runs each fixture headlessly, captures every APU sample, writes WAVs to
# target/testrom-artifacts/audio/ (LISTEN to them!), and checks:
#   - sample count == cycles × rate / 4194304 (a drifting count is a rate bug)
#   - tone440: dominant frequency ≈ 439.84 Hz, peak ≈ 0.25, no unfiltered steps,
#     no silence gaps — absolute expectations, known a priori
#   - dmg_sound: non-silent; Blargg's $A000 memory verdict is reported
#   - byte-identical determinism across runs, FNV-hash pinned in
#     testharness/audio_baseline.txt (re-bless: UPDATE_AUDIO_BASELINE=1)
```

**Is emulation still inside the frame budget with audio attached?** — the check
that was missing, and the reason §4c shipped. Needs `--features audio` but **no
sound card**, so it runs in CI:

```bash
make test-audio-perf     # cargo test --release --features audio --test audioperf
# Attaches the real producer half with no consumer, so the queue saturates and
# stays saturated — the state the captain's machine was in. Fails if a frame
# costs more than 4 ms, or if emulation runs slower than 4x real time, or if the
# queue grows past high_water. Against the old wait-at-the-watermark policy it
# reports 29.6 ms/frame at 0.57x and fails.
```

**What does one frame cost against a real device?** (needs a card):

```bash
PACED=1 cargo run --release --features audio --example audio_frame_cost -- <rom.gb> 3600
# Per-frame wall time AND thread CPU time side by side: if they diverge,
# emulation is not what is slow. Also reports cycles/frame, which is not always
# 70224 (see §4c on LCD-off bursts).
```

**Does the real device pipeline underrun?** (needs `--features audio` + a card):

```bash
GB_AUDIO_STATS=1 cargo run --release --features audio --example audio_soak -- 120
# Real 2-minute run against the sound card: reports underruns, drops, queue
# depth min/max and the achieved production rate vs the device rate.
```

**How far behind is the sound?** — end-to-end latency *and* underruns together,
because they trade off and a fix that wins one by losing the other is not a fix:

```bash
cargo run --release --features audio --example audio_latency -- <rom.gb> 300
# Reports, per second and in summary:
#   - our queue's latency          (depth / rate)
#   - the host/device's latency    (cpal playback-minus-callback timestamp)
#   - END TO END                   (the number the player actually feels)
#   - underruns, drops, drift
#   - the production-stall distribution, with the timestamp of each late frame
#     (a stall during startup priming is harmless; one mid-run is not)
```

**Did I break emulation?** The test-ROM harness must still match baseline:

```bash
cargo test --release --test testroms      # see docs/testing.md
```

---

## 9. Change log

### First pass ("rough & fast")

| Area | File | Change |
|---|---|---|
| Device negotiation | [`src/sound.rs`](../src/sound.rs) | Prefer the device default `F32` config; otherwise scan all `F32` ranges preferring span-44100 then max-channels. Kills mono downmix and 96/192/384 kHz surprises. |
| Frame pacing (root cause of "fast") | [`src/gameboy.rs`](../src/gameboy.rs) | Desktop frame period 16 ms → 16.743 ms (62.5 → 59.73 fps). |
| Tests | [`src/sound.rs`](../src/sound.rs) | `tone_pitch_is_correct`, `output_matches_reported_rate` (device-free). |
| Probe | [`examples/audio_probe.rs`](../examples/audio_probe.rs) | Report a machine's real configs + negotiated rate. |

### Second pass ("blocky and broken" regression + offline harness)

| Area | File | Change |
|---|---|---|
| Queue policy | [`src/sound.rs`](../src/sound.rs) `sample_queue` | Prime-before-play (7000 samples), hold-and-fade + re-prime on underrun, high-watermark producer wait (audio clock as master), stall-detected drop as last resort, full instrumentation. Sizes from simulation + real-device soak, §4/§5a. |
| Frame pacing (root cause of "blocky") | [`src/gameboy.rs`](../src/gameboy.rs), [`src/screen/tui.rs`](../src/screen/tui.rs) | Deadline-accumulator scheduling (`next += FRAME_TIME`, bounded catch-up) in both frontends — the average rate is now exactly the period, with no draw/poll overhead bias. TUI change is the pacing block only. |
| APU semantics | [`src/sound.rs`](../src/sound.rs) | NRx1 length writes mask duty bits (`v & 0x3F` — was a debug-mode panic on `NR11=0x80`); NR52 power-off actually zeroes NR10–NR51 and channel status (the old clear-loop ran *after* `on=false` and was a no-op); a powered-off APU emits silence at full rate instead of stalling the sample stream. |
| Offline audio harness | [`testharness/audiocapture.rs`](../testharness/audiocapture.rs) | Device-free capture → WAV + analysis (count/RMS/silence/steps/pitch) + Blargg `$A000` verdict + pinned determinism baseline ([`testharness/audio_baseline.txt`](../testharness/audio_baseline.txt)). |
| Soak evidence | [`examples/audio_soak.rs`](../examples/audio_soak.rs) | Real-device instrumented run; the §5a numbers. |

### Third pass (perceptible latency)

| Area | File | Change |
|---|---|---|
| Device buffer | [`src/sound.rs`](../src/sound.rs) `cpal_audio` | Ask for `BufferSize::Fixed(256)` instead of letting the host choose. Measured host latency **99.5 ms → 5.9 ms**. Override with `GB_AUDIO_DEVICE_FRAMES` (`0` = host default). |
| Producer block size | [`src/sound.rs`](../src/sound.rs) | `OUTPUT_SAMPLE_COUNT` 2000 → **256** (45 ms → 5.8 ms). The block size is the floor under the queue and therefore under latency; `play_samples` now reuses scratch buffers rather than allocating six `Vec`s per block. |
| Queue sizing | [`src/sound.rs`](../src/sound.rs) `sample_queue` | `START_FILL`/`HIGH_WATER` consts → `start_fill()`/`high_water()` (env-overridable), 7000/12000 → **1792/2816**. `high_water` is now a latency *ceiling* the producer clips to `start_fill` at, never a point it waits at (§4c). Sized to a stated promise — absorb one dropped frame — with the simulation profile re-derived from the *measured* stall distribution instead of an assumed 90 ms stall. |
| Consumer-stall detection | [`src/sound.rs`](../src/sound.rs) | Elapsed-time based, not callback-counter based: the counter test misfired once a frame pushed several blocks back-to-back, discarding 330 ms of healthy audio per 90 s. |
| Full-path measurement | [`examples/audio_latency.rs`](../examples/audio_latency.rs) | End-to-end latency (queue + device, from cpal timestamps), underruns and the production-stall distribution from one real-device run. |

**Not verified by ear.** Every number above is measured; nobody has listened to
the result. What a human must confirm: that sound now follows a button press
without a felt delay, and that nothing crackles.

---

## 10. Why emulator audio breaks — the community's map (with sources)

The failure modes above are all well-documented in the emulation community;
this project's bugs were textbook instances. All links verified 2026-08-13.

1. **Aliasing → band-limited synthesis (why blip_buf exists).** The APU changes
   amplitude at up to 4.19 MHz; naively downsampling that to 44.1 kHz folds
   everything above Nyquist back into the audible band as inharmonic buzz.
   Blargg's fix — replace each transition with a precomputed band-limited step —
   is the basis of his Blip_Buffer/blip_buf libraries, used across the emulation
   scene (and by this emulator via the `blip_buf` crate). With that in place,
   resampling *quality* is solved; what an emulator can still get wrong is
   **rate and pacing**.
   - Blargg, *Band-Limited Sound Synthesis*: http://slack.net/~ant/bl-synth/
   - Blargg's audio libraries: http://slack.net/~ant/libs/audio.html
   - blip-buf readme (mirror): https://github.com/nesbox/blip-buf

2. **The producer/consumer contract: sync to audio, or drift forever.** The
   sound card is a hard-realtime consumer draining at its own crystal's rate;
   the display (or a frame timer) ticks at a different crystal. Near's article
   spells out the trilemma: sync to video only → the audio buffer slowly
   drains/grows (periodic crackle or mounting latency); sync to audio only →
   video stutter; block on both → both artifacts. **Dynamic rate control** —
   nudge the resample ratio ≤0.5 % based on buffer fill so it hovers half-full —
   is the polished answer (bsnes, RetroArch). *This emulator's structural
   choice:* pace video by a deadline accumulator (§5a) and make the queue's
   high watermark a **latency bound rather than a wait** — the producer runs
   inside `Cpu::do_cycle`, so waiting there makes the audio clock the master
   from inside the emulator's own inner loop, which is exactly the "sync to
   audio only" corner of the trilemma and cost 17.6 ms of every frame (§4c).
   Residual crystal drift is therefore absorbed by hold-and-fade below and by
   discarding above; measured over 120 s at −0.006 %, that is zero of either.
   Full DRC is the documented upgrade path if it ever matters.
   - Near (byuu), *Dynamic Rate Control*: https://bsnes.org/articles/dynamic-rate-control/
     (article archive: https://github.com/higan-emu/emulation-articles)
   - Libretro docs, *Dynamic Rate Control*: https://docs.libretro.com/development/cores/dynamic-rate-control/
   - NESdev, *audio buffering and timing discrepancies*: https://forums.nesdev.org/viewtopic.php?t=9454

3. **Buffer sizing and underrun handling.** Too small → any scheduling jitter
   finds the buffer empty (gap/click); too big → audible input-to-sound lag.
   The classic symptom description — "periods of silence after one segment of
   audio has finished playing but before the next one is queued" — is exactly
   the "blocky" report. On underrun, "play silence" is the platform *default*
   (SDL documents it), and a hard step to zero is itself a wideband transient;
   hold-last-sample / short fades are the standard softenings. Keep a cushion
   of a couple of callback blocks and target roughly half-full.
   - NESdev, *Cracking Sound for APU*: https://forums.nesdev.org/viewtopic.php?p=234578&t=18493
   - SDL2 `SDL_QueueAudio` docs (silence on underrun): https://wiki.libsdl.org/SDL2/SDL_QueueAudio

4. **Game Boy APU correctness traps** (the accuracy frontier `dmg_sound`
   measures, distinct from playback quality): NRx4 trigger semantics (reload
   length/envelope/period but *not* the wave sample buffer; no enable if DAC
   off); length-counter extra clocking in the frame-sequencer's off-phase;
   DMG-vs-CGB length behaviour across power-off; the CH1 sweep negate quirk;
   DMG wave-RAM corruption on mid-read retrigger; NR52 power-off zeroing
   NR10–NR51; and the DAC-off vs channel-enabled distinction (digital 0 is a
   *non-silent* analog level — the DAC-toggle pop is real hardware).
   - Pan Docs: https://gbdev.io/pandocs/Audio_details.html and
     https://gbdev.io/pandocs/Audio_Registers.html
   - gg8 wiki, *Gameboy sound hardware* (Obscure Behavior):
     https://gbdev.gg8.se/wiki/articles/Gameboy_sound_hardware
   - Blargg's `dmg_sound` ROMs + readme (memory-mapped `$A000` reporting):
     https://github.com/retrio/gb-test-roms

**How this project's bugs map onto that list:** first "rough & fast" was §2's
map item 2 (video-timer pacing +4.6 % fast, buffer overrun → drops); "blocky
and broken" was items 2+3 combined (reset-to-now pacing → producer slow →
starvation, step-to-zero underruns, no priming, 45 ms producer granularity
against a 4-block cap). The 4-bit grit that remains is item-4 territory —
genuine DMG character, not a defect.

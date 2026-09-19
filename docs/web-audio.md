# Web audio — sound in the browser, and the APU that was missing entirely

> **Scope.** How the `wasm32` build makes a noise: why it made none before, what the
> browser makes hard, the design that came out of it, and the numbers a real Chrome run
> produced. The register-level channel model is
> [`docs/components/apu.md`](components/apu.md); the desktop signal path and the whole
> "rough & fast" investigation is [`docs/audio.md`](audio.md). This page is the browser's
> half of the same pipeline.

- **Code:** [`src/sound.rs`](../src/sound.rs) `mod web_audio` (the reusable half),
  [`src/screen/webaudio.rs`](../src/screen/webaudio.rs) (the `AudioContext` shell),
  [`src/screen/pace.rs`](../src/screen/pace.rs) (frame pacing),
  [`src/screen/web.rs`](../src/screen/web.rs) (the loop that drives both).
- **Tests:** `cargo test` — `sound::web_audio::tests` and `screen::pace::tests`. Both
  compile on the host under `cfg(test)` precisely so the logic is testable without a
  browser.
- **Build:** `cd web && npm run serve`, or `make build-wasm`. No feature flag: on
  `wasm32` this *is* the audio path.

---

## 1. The finding: the web build had no sound chip, not a quiet one

The roadmap said "web audio" as though it were a missing output stage. It was not. The
whole APU was compiled out of the `wasm32` build:

```rust
// src/mmu/mod.rs, before
#[cfg(not(target_arch = "wasm32"))]
pub sound: Option<Sound>,
```

…with matching `cfg`s on every register access, so on `wasm32` `$FF10-$FF3F` read back
`$FF`, `$FF76`/`$FF77` (PCM12/PCM34) read back `$00`, and every write was dropped.

That is not "silent". It is a **different machine**, and a cartridge can tell:

- `NR52` reads back as powered off however many times the game writes `$80` to it.
  Blargg's `interrupt_time` spins on exactly that read, and so does real game code —
  the sound-driver initialisation of a Gen 1 cartridge waits on the APU.
- The frame sequencer never advances, so length counters never expire and sweeps never
  fire.
- PCM12/PCM34 report nothing, which is the whole interface 57 of SameSuite's 61 APU
  ROMs use.

This is the same principle [`crate::sound::SilentPlayer`](../src/sound.rs) exists for on
native builds, applied to a target where it had never been applied.

**Why it was like that, and why it no longer needs to be.** The APU's resampler is
`blip_buf`, and the version this project pinned — `0.1.4` — was a *binding to the C
library*: it depends on `blip_buf-sys` and `libc` and builds C through a `cc` build
script, which does not cross-compile to `wasm32-unknown-unknown` without a C toolchain
aimed at wasm. **`blip_buf` 0.1.5 is a pure-Rust rewrite with no dependencies and no
build script** ([mvdnes/blip_buf-rs](https://github.com/mvdnes/blip_buf-rs)); the
lockfile had already resolved to it. Moving the requirement to `>= 0.1.5` and out of the
`cfg(not(target_arch = "wasm32"))` dependency block was the entire blocker.

The core's dependency contract is unchanged and still gated by `make embed`:

```text
terminalgb v0.2.0
├── blip_buf v0.1.5
└── terminalgb-plugin-api v0.1.0 (first-party)
```

---

## 2. What the browser makes hard

### 2a. There is no callback thread we may use

On the desktop, cpal runs a callback on the audio thread that pulls samples at exactly
the device rate, and the emulator's frame loop only has to keep a queue topped up. The
browser offers three shapes and none of them is that:

| Mechanism | Why not |
|---|---|
| `AudioWorklet` + `SharedArrayBuffer` | `SharedArrayBuffer` requires cross-origin isolation (`COOP`/`COEP` response headers). A static host — GitHub Pages, which is where this build is published — cannot set them. |
| `AudioWorklet` + `postMessage` | Works without isolation, but ships a JS worklet as a second artefact and moves the ring buffer out of Rust for no accuracy gain. Kept as a future option, not needed. |
| `ScriptProcessorNode` | Deprecated, and runs on the **main** thread anyway — so it is starved by exactly the same stalls as the frame loop. |

What is left, and what is the standard technique, is to hand the context short
`AudioBuffer`s with an explicit start time and keep a scheduling cursor a fixed distance
ahead of `AudioContext.currentTime`. The audio thread then plays them without any
further help from us.

### 2b. Nothing that touches JS may go near the APU

`AudioPlayer` is `Send` (the FFI build parks a whole `Gameboy` in a `static Mutex`), and
a `web_sys::AudioContext` is not. So the split is:

- **`sound::web_audio::WebAudioPlayer`** — the producer. Holds a
  `sample_queue::SampleQueue` and nothing else, and its `play()` is the same
  non-blocking hand-off cpal uses: push, then bound latency by *discarding* the oldest.
  It must never wait — `play` is called from inside `Cpu::do_cycle`, and a wait there is
  charged to the emulated frame ([`docs/audio.md` §4c](audio.md)).
- **`sound::web_audio::Scheduler`** — the consumer's *decisions*, with no `web-sys` in
  sight. Unit-tested under a plain `cargo test`.
- **`screen::webaudio::WebAudio`** — about twenty lines that call `AudioContext`,
  living in `screen/` with every other host assumption.

`sound::web_audio::tests::the_player_half_holds_nothing_that_is_not_send` is a
compile-time assertion of the first point, because holding the context in the player is
the obvious first design and the error it produces appears a long way from the cause.

### 2c. `requestAnimationFrame` is not 59.7275 Hz

The old web loop stepped one emulated frame per animation-frame callback, i.e. at **the
display's refresh rate**:

| display | emulated rate | what a player gets |
|---|---|---|
| 60 Hz | 60 Hz | +0.46 % |
| 120 Hz | **120 Hz** | the game at **double speed** |
| 144 Hz | **144 Hz** | 2.4× |

A high-refresh laptop or phone is ordinary now, so this is not a corner case — and it
also makes correct audio impossible, because the APU emits a fixed number of samples per
*emulated* second and a machine running 2× fast produces audio twice as fast as any
device can play it.

[`screen::pace::FramePacer`](../src/screen/pace.rs) is the same deadline accumulator the
terminal frontend uses: advance the deadline by exact periods, run however many frames
are due, cap the catch-up so a backgrounded tab does not replay its whole absence inside
one event handler. `std::time::Instant` panics on `wasm32-unknown-unknown`, so the clock
is passed in — which is also what lets the loop pace on `AudioContext.currentTime` and
share **one** clock with the audio consumer, so producer and consumer cannot drift apart.

---

## 3. The two rules that stop it crackling

Both are the `Scheduler`'s job and both have tests.

1. **Never schedule in the past.** `AudioBufferSourceNode.start(when)` with `when`
   already gone plays *immediately* rather than being skipped
   ([Web Audio API, `start()`](https://www.w3.org/TR/webaudio/#dom-audioscheduledsourcenode-start)),
   so a late block **overlaps** the one before it — a click, not the gap it really is.
   The cursor is reset (and an underrun counted) the moment it is not strictly ahead of
   `currentTime`.
2. **Never substitute samples to fill a block.** The cursor advances by exactly
   `block / sample_rate` per block, so it *is* the output clock. `SampleQueue::next_sample`
   correctly pads with a decaying hold sample for a fixed-rate device callback; doing
   that here would advance the cursor by time no real audio occupies and slide the
   timeline for the rest of the run. A short queue means "schedule nothing this tick".
   That is what `SampleQueue::drain_block` is for — all of `n` or none of it.

---

## 4. Three numbers that were measured, not chosen

Every one of these was got wrong first, in a real browser, and the wrong version is
recorded because it is the argument for the right one.

### `TARGET_LEAD = 4096` samples (93 ms at 44.1 kHz, 85 ms at 48 kHz)

First value tried: 2048, by analogy with the desktop cushion (1792 samples / 40.6 ms,
sized to absorb one dropped frame). **Measured in Chrome: ~2 underruns per second**, with
the emulator itself at a perfect 59.6 fps. The analogy is wrong, and the reason is §2a:
on the desktop a cpal callback thread keeps pulling while the frame loop stalls, but in
a browser one `requestAnimationFrame` callback produces the audio **and** schedules it,
so a single stall starves both halves and the cushion has to cover the whole stall.

The ceiling is [`pace::MAX_CATCHUP`](../src/screen/pace.rs) frames (134 ms): a gap longer
than that is resynchronised rather than replayed, so its audio is never produced and no
cushion can hide it. 4096 samples sits just under that and measured **0 underruns**.

The cost is honest latency. It is the trade the browser forces: a gap is heard as a
click; 93 ms of delay is heard as nothing at all unless you are looking for it.

### The queue is `TARGET_LEAD` / `4 × TARGET_LEAD`, not the desktop's `start_fill()` / `high_water()`

The desktop sizing (1792 / 2816) is for a queue that a device callback drains at exactly
the sample rate, where the queue *is* the whole latency. Here the scheduler owns the
latency and the queue is only the transfer buffer between two halves of one animation
frame — so clipping it at 2816 destroys precisely the backlog the scheduler needs to
rebuild its cushion after a stall. Measured with the desktop sizing: **~2,560 samples
discarded per second** on an otherwise healthy run.

Worse, with the lead raised to 4096 the two *deadlock*: the scheduler primes on 4096
samples and the queue is clipped every time it approaches 2816, so not one sample is ever
played. `the_queue_can_hold_a_whole_scheduling_lead` pins that.

### Prime once, and **never re-prime after an underrun**

Starting the moment one block exists puts the cursor 11.6 ms ahead of a producer that
delivers 16.7 ms of audio every 16.7 ms, so the first block runs out before the second
can be filled: measured, exactly one underrun on every run until the scheduler waited for
a whole lead before its first block.

Re-priming *after* an underrun is the mirror-image mistake and was also measured. A
cushion can only be rebuilt out of a backlog, and after a stall the backlog is exactly
what exists (the pacer catches the missed frames up in one go). Waiting for a full lead
instead adds tens of milliseconds of further silence, during which the producer keeps
filling until the queue's latency bound clips it — so one stall became a self-sustaining
cycle: **~2 underruns and ~2,560 dropped samples per second, indefinitely.**

---

## 5. Measured in a real browser

Chrome, `AudioContext` at 48 kHz, a hand-assembled tone cartridge (`NR13`/`NR14` set to
period `1750`, i.e. 439.84 Hz), with a `ScriptProcessorNode` tapped onto the destination
so the numbers describe what was **played**, not what we think we scheduled:

| | |
|---|---|
| run length | **311 s** |
| emulated rate | **59.6 fps** (a true DMG is 59.7275) |
| underruns | **0** |
| samples dropped by the latency bound | **0** |
| end-to-end scheduling lead | **85–96 ms**, stable |
| gaps in the played audio | **1** — the 96 ms priming silence at start-up, never again |
| peak / RMS | 0.1333 / 0.1234 (crest 1.08, right for a 50 % duty square) |
| largest sample-to-sample step | 0.1667 against a 0.267 peak-to-peak wave — a band-limited edge, not a click |
| measured pitch | **439.7 Hz** vs the 439.84 Hz the ROM programmed (the estimator counts zero crossings over the whole run including the 96 ms of start-up silence; 439.84 × (311 − 0.096) / 311 = 439.70) |

And with the earlier, wrong constants, the same harness on the same build: **2 underruns
and ~2,560 dropped samples per second**, with audible gaps every half-second. The
difference between the two is the whole of §4.

The picture path was checked in the same browser with the vendored MIT dmg-acid2 ROM: the
canvas comes back with exactly the four DMG shades and the histogram
`0→3749, 1→188, 2→6254, 3→12849`, which is **identical to Matt Currie's
hardware-photographed reference PNG**, decoded and counted the same way.

### Reproduce it

There is no committed browser harness — a page that drives a headless Chrome is not
something to keep green in CI — so this is the recipe rather than a script:

```bash
cargo build --release --target wasm32-unknown-unknown --lib
wasm-bindgen --target web --out-dir pkg target/wasm32-unknown-unknown/release/terminalgb.wasm
# serve pkg/ + a page that imports { render, status } and calls render(romBytes)
```

`status()` is exported from the wasm module and returns one line: fps, the
`AudioContext`'s state and rate, the current lead in milliseconds, queue depth, underruns
and drops. The shipped page ([`web/index.html`](../web/index.html)) shows it above the
picture. To measure what was *heard* rather than what was scheduled, hook
`AudioNode.prototype.connect` and fan anything bound for `context.destination` into a
`ScriptProcessorNode` as well.

---

## 6. The browser will refuse to start audio, and it must say so

An `AudioContext` created outside a user gesture starts `suspended` and stays there
([Chrome's autoplay policy](https://developer.chrome.com/blog/autoplay/#webaudio),
[the spec's "allowed to start" rule](https://www.w3.org/TR/webaudio/#allowed-to-start)).
The page's Play button is a gesture, so the ordinary path works — but a context can also
be suspended later, so every key press calls `resume()` again, and the status line reports
`suspended` when it is.

This is the same rule as the terminal frontend's `Audio`/`Plugin`/`Term` rows and as
`sound::host_status` itself: **"silent" and "broken" must never look the same**. A web
build that made no sound and said nothing about why is what this page exists to prevent
happening again.

---

## 7. Conformance record

Per the [standing convention](../AGENTS.md). Two defects, both in the *frontend*, so
point 3 cites the platform contract rather than Pan Docs where that is what was violated.

**(a) The `wasm32` build had no APU.**

- *Symptom in user terms* — the browser build is silent, and beyond that any cartridge
  that waits on the sound chip stalls: `NR52` never reads back powered on however many
  times the game writes `$80`, so a sound-driver initialisation loop spins for ever and
  the game appears to hang at its title screen.
- *Which test catches it* — **nothing in this repository did, and nothing could**: every
  test-ROM harness, every external-suite scout and every unit test runs on the host
  target, where the APU was always present. `make check-wasm` type-checks the build; a
  type-check cannot see a missing peripheral. That is a real coverage gap and it is still
  open: there is no `wasm32` test runner here. What exists now is
  `sound::web_audio::tests` (the scheduler, on the host) and the manual browser
  measurement in §5.
- *Hardware behaviour violated* — the APU is part of the DMG/CGB silicon and `NR52` bit 7
  reflects the power state a program writes to it (Pan Docs,
  [Sound Overview](https://gbdev.io/pandocs/Audio_Registers.html#ff26--nr52-audio-master-control)).
  A console with no sound chip does not exist.
- *New test* — none that can run headlessly; see the gap above. The nearest guard is the
  dependency requirement `blip_buf = "0.1.5"` with the reason written at the declaration,
  because dropping to 0.1.4 silently reintroduces the C toolchain requirement that made
  the `cfg` necessary in the first place.

**(b) Emulation paced by `requestAnimationFrame`.**

- *Symptom in user terms* — on a 120 Hz display the game runs at **double speed**: music
  and animation twice as fast, timers twice as fast, the whole machine wrong. On an
  ordinary 60 Hz display it is 0.46 % fast, which is inaudible on its own and is exactly
  enough to make an audio queue shed a cushion's worth of samples on a timer.
- *Which test catches it* — no test ROM, and none can: the emulated machine is entirely
  correct, and a framebuffer hash is identical whatever rate the frames are produced at.
  Same blind spot as the terminal render defects recorded in `CLAUDE.md`. Now
  `screen::pace::tests::a_fast_display_does_not_speed_the_machine_up`, which reads 1200
  emulated frames per 10 s against the old loop and ~597 against this one.
- *Contract violated* — `requestAnimationFrame` is specified as firing before the next
  *repaint* ([HTML, "Animation frames"](https://html.spec.whatwg.org/multipage/imagebitmap-and-animations.html#animation-frames)),
  i.e. at the display's rate; it is not and has never been a timer. A DMG frame is
  70224 cycles / 4.194304 MHz = 59.7275 Hz.
- *New test* — the four cases in `screen::pace::tests`. Conformance candidate: **an
  emulator's frame rate must come from a clock, never from a presentation callback.**

**(c) The web/FFI mapper — closed earlier, verified here.** `mbc::get_mbc` used to return
an MBC3 for *any* cartridge when no save path was given, which is always the case for the
web and FFI builds. That was fixed when persistence and mapper selection were separated;
what this work added is the end-to-end check —
`lib::ffi_tests::the_ffi_entry_point_takes_the_mapper_from_the_cartridge_header` drives
the real `extern "C" load()` symbol with an MBC1 cartridge that bank-switches, because
"the core is right" and "the entry point is right" are different claims. The web path
constructs its machine with the identical `Gameboy::new(rom, None)` call
([`src/screen/web.rs`](../src/screen/web.rs)).

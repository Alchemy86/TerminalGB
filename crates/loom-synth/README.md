# loom-synth

A small subtractive/wavetable synthesis engine, in Rust, with **no
dependencies**. It exists to give ShowReel's `music.rs` (which generates one
fixed idiom — a duty-cycled-pulse NES/SID chiptune, and says in its own docs
"it is not a general instrument system and does not want to become one") a
general set of primitives to grow other moods on, without touching that
file's design: music as a *source*, one seam into the film pipeline, nothing
committed that cannot be regenerated from the spec.

## What's here

| Module | What it is | The theory behind it |
|---|---|---|
| `osc` | Sine, saw, pulse/square (any duty), triangle, white noise, arbitrary wavetable | Naive phase-accumulator oscillators alias badly at audio rates; saw and square are corrected with **PolyBLEP** (Välimäki & Huovilainen, *Antialiasing Oscillators in Subtractive Synthesis*, IEEE SPM 24(2), 2007) |
| `envelope` | A real four-stage ADSR (attack/decay/sustain/release), with a `shape` parameter for exponential-feeling curves | The standard envelope generator every subtractive synth (and this project's own reference book) covers |
| `filter` | A resonant low-pass biquad | Robert Bristow-Johnson's *Audio EQ Cookbook* — the canonical two-pole IIR design |
| `voice` | `Instrument`: oscillator + ADSR + optional filter (with its own envelope) + vibrato + gain | The abstraction a piece asks for by name, instead of a bespoke function per voice |
| `sequence` | `Piece`/`Part`/`Pattern`: a chord progression, sixteen steps a bar, one pattern per part | Enough to write an actual piece — not a DAW |
| `mix` | Additive placement + a `tanh` soft limiter into headroom | The same simple technique `music.rs` already uses |
| `wav` | Hand-rolled 16-bit PCM WAV reader/writer | Public file format, no dependency needed |
| `compare` | Windowed loudness- and brightness-correlation between two renders | How `presets::chiptune_funk` is checked against ShowReel's own reference render, below |
| `presets` | Named instrument sets built from the above | `chiptune_funk` (the reference idiom, reproduced through general primitives) and `analog_pad` (an original piece in a texture the old system had no filter to make) |

Every render is pure and deterministic: no clock, no environment, no
filesystem access inside `Piece::render` itself — the same property
`music.rs`'s own docs insist on, for the same reason (reproducible builds
from source, content-addressable output).

## Using it

```bash
cargo run -p loom-synth --release --bin loom-synth -- render chiptune-funk -o funk.wav --bars 8
cargo run -p loom-synth --release --bin loom-synth -- render analog-pad -o pad.wav --bars 8 --seed 7
cargo run -p loom-synth --release --bin loom-synth -- compare a.wav b.wav
```

Or as a library:

```rust
use loom_synth::{presets, wav};

let piece = presets::analog_pad(8, 7);      // 8 bars, seed 7
let samples = piece.render(48_000.0);        // mono f32, pure, deterministic
wav::write_mono("pad.wav".as_ref(), &samples, 48_000)?;
```

## Proof this is a superset, not a competitor

`chiptune_funk` reproduces `music.rs`'s `Voicing::of(Mood::Funk)` — the same
Am-F-C-G progression, the same `bass_gate`/`lead_pat` step patterns, the same
kick/hat rhythm — through this engine's general oscillator/ADSR/filter
primitives instead of that file's four bespoke functions (`pulse`,
`held_tone`, `noise`, `kick`). It is **not** a byte-for-byte port: the
oscillators are band-limited where the reference's are naive, the envelope
is a real ADSR rather than a fixed millisecond ramp, and the kick reaches for
this engine's filter instead of amplitude-modulating noise by a sine.

Measured against a WAV rendered by ShowReel's own, completely unmodified
`showreel music --mood funk` CLI (built and run as-is — nothing in ShowReel
was edited to produce it), same progression, same tempo, same 15-second
length:

```
envelope correlation:   0.9831
brightness correlation: 0.9586
```

("Envelope correlation" is the Pearson correlation of the two renders'
20ms-windowed RMS loudness curves — whether notes land and decay at the same
moments in both. "Brightness correlation" is the same, over each window's
energy above/below ~1kHz — whether the two pieces brighten and darken
together. See `src/compare.rs`'s module docs for the exact method and why
it's the honest question to ask of an independent reimplementation, rather
than demanding identical samples.)

Reproduce it yourself:

```bash
# From a ShowReel checkout, unmodified:
cargo build --release --bin showreel
./target/release/showreel music --mood funk --duration 15 --seed 3111 -o /tmp/ref-funk.wav

# From this crate:
cargo run -p loom-synth --release --bin loom-synth -- render chiptune-funk -o /tmp/mine.wav --bars 8 --seed 3111
cargo run -p loom-synth --release --bin loom-synth -- compare /tmp/ref-funk.wav /tmp/mine.wav
```

## Licensing

MPL-2.0, matching every sibling crate in this workspace (`terminalgb-chiptune`,
`terminalgb-plugin-api`). ShowReel itself is PolyForm-Noncommercial-1.0.0;
MPL-2.0 is file-level copyleft and does not restrict what license a work that
*uses* an MPL-2.0 crate carries, so there is no conflict in ShowReel adding
this as an ordinary dependency later. No code here was copied from either
project — every primitive is a fresh implementation of a published, citable
formula (see the table above), and the musical *data* `chiptune_funk` carries
(the chord progression, the step patterns) is taken from ShowReel's own
original composition, reproduced here for the specific, stated purpose of
proving equivalence, not as a third-party asset.

## Where this goes next

This crate deliberately does not know ShowReel exists — no dependency on it,
no `Music`/`Audio`/`Film` types, nothing here that could fork ShowReel's own
audio path. Wiring it in is a small, separate, mechanical change that
belongs in ShowReel's own tree, done there rather than here (this crate was
built from a `TerminalGB` worktree with no authorization to modify a
different project's checkout):

1. Add `loom-synth` as a path (or later, git) dependency in
   `showreel/Cargo.toml`.
2. Give `music::Mood` a new variant (or a parallel `Voice`/`Instrument`
   enum, if moods and instruments turn out to want to vary independently)
   that resolves to a `loom_synth::sequence::Piece` instead of one of
   `Voicing::of`'s hardcoded tables.
3. In `Music::render_samples`, branch on that variant and call
   `piece.render(SAMPLE_RATE as f64)` instead of the current bar-walking
   loop — `render_to_temp`'s content-addressed WAV cache, `resolve_audio_tracks`'s
   seam into `Audio`, and everything downstream of "here is a `Vec<i16>`"
   need no change at all.
4. `chiptune_funk` above is the existing `Mood::Funk` proven equivalent
   through the new engine; the `analog_pad` preset is a ready-made second
   mood a film could ask for immediately, with no further work.

Nothing here forks that file's audio path or its seam into the timeline —
that property is exactly what step 3 preserves.

//! **Loom** — a small subtractive/wavetable synthesis engine, in Rust, with
//! no dependencies.
//!
//! # Why this exists
//!
//! ShowReel's `music.rs` generates one idiom well — a duty-cycled-pulse
//! NES/SID chiptune — and says so in its own docs: "it is not a general
//! instrument system and does not want to become one." That was the right
//! call for what it needed to be at the time; it is also exactly the gap
//! this crate fills, by taking the fixed voices out of that file's shape and
//! putting real synthesis primitives underneath instead:
//!
//! - [`osc`] — band-limited oscillators (sine, saw, pulse/square at any
//!   duty, triangle, noise, an arbitrary wavetable).
//! - [`envelope`] — a real four-stage ADSR, not a fixed ramp or a fixed
//!   exponential.
//! - [`filter`] — a resonant low-pass biquad, the one thing a Game Boy-style
//!   chip idiom structurally cannot have (no Game Boy channel is filtered).
//! - [`voice`] — [`voice::Instrument`], the abstraction a piece actually asks
//!   for: an oscillator, an envelope, an optional filter with its own
//!   envelope, vibrato, gain. This is what turns "hardcode three voice
//!   functions" into "name an instrument".
//! - [`sequence`] — a small step sequencer (a chord progression, sixteen
//!   steps a bar, a pattern per part) — enough to write an actual piece,
//!   deliberately not enough to be a DAW.
//! - [`mix`] — additive placement plus a soft (`tanh`) limiter into
//!   headroom.
//! - [`wav`] — a hand-rolled 16-bit PCM WAV reader/writer, so producing or
//!   auditing a render needs no dependency either.
//! - [`compare`] — quantitative similarity between two renders (windowed
//!   loudness and brightness correlation), used to check
//!   [`presets::chiptune_funk`] against ShowReel's own reference render —
//!   see that module and `README.md`.
//! - [`presets`] — named instrument sets built from the above:
//!   [`presets::chiptune_funk`] (the reference idiom, reproduced through
//!   general primitives rather than bespoke functions — proof this is a
//!   superset, not a replacement) and [`presets::analog_pad`] (an original
//!   piece in a texture the old system had no way to produce at all).
//!
//! # What every render promises
//!
//! Pure and deterministic: [`sequence::Piece::render`] takes no clock, no
//! environment, no filesystem, and a fixed seed. The same spec renders to
//! the same samples every time — the property ShowReel's own `music.rs`
//! insists on (its docs call it out explicitly, for exactly the same
//! reason: reproducible builds from source). Nothing here writes a file on
//! its own; [`wav::write_mono`] is an explicit, separate step, matching
//! `Music::render_to_temp`'s own separation between "produce samples" and
//! "put them somewhere".
//!
//! # Where this fits, and where it deliberately does not reach
//!
//! This crate does not know about ShowReel's `Music`/`Audio`/`Film` types
//! and does not depend on ShowReel — a synth engine has no business knowing
//! what a film's audio-track seam looks like, the same argument
//! `terminalgb-chiptune` makes for staying independent of the emulator it
//! renders through. A caller (ShowReel's `music.rs`, or anything else) is
//! meant to call [`sequence::Piece::render`] and hand the result to its own
//! WAV/asset pipeline; wiring that up on ShowReel's side is a small,
//! separate, mechanical change and is intentionally not made here — see
//! `README.md`'s "Where this goes next" section for the reasoning.

pub mod compare;
pub mod envelope;
pub mod filter;
pub mod mix;
pub mod osc;
pub mod presets;
pub mod rng;
pub mod sequence;
pub mod voice;
pub mod wav;

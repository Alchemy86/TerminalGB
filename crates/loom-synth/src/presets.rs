//! Named instrument sets — what a caller reaches for instead of hand-wiring
//! oscillators, envelopes and filters every time.
//!
//! [`chiptune_funk`] is not a demo: it is this crate's proof that the old,
//! single fixed idiom is now one preset among several rather than the whole
//! system. Every value below (the chord progression, the bass/lead step
//! patterns, the duty cycles, the gains) is taken **verbatim** from
//! `Voicing::of(Mood::Funk)` and `bass_on`/`lead_on`/`kick_on`/`hat_on` in
//! ShowReel's `showreel/src/music.rs` — this project's own earlier,
//! original composition (its own docs say plainly it is "not sampled,
//! transcribed or borrowed"), reproduced here through this engine's general
//! oscillator/envelope/filter primitives instead of that file's four
//! bespoke functions (`pulse`, `held_tone`, `noise`, `kick`). What differs
//! is *how* each voice is built, not what it plays — which is the whole
//! point: the same piece should be expressible without the bespoke code.
//!
//! [`analog_pad`] is the other half of the proof: an original piece (a fresh
//! D-major progression, not shared with anything ShowReel already has) in a
//! texture the old system could not produce at all, because it has no
//! filter — a slow, filtered, ADSR-shaped pad instead of a duty-cycled
//! pulse.

use crate::envelope::Adsr;
use crate::osc::Waveform;
use crate::sequence::{Bar, NoteSource, Part, Pattern, Piece};
use crate::voice::{FilterSpec, Instrument};

/// ShowReel's reference chiptune — the funk mood, sized to `bars_to_play`
/// bars at its own 128 BPM. `seed` reproduces exactly one texture for the
/// noise-based drums; the reference's own default is `0xC17`.
pub fn chiptune_funk(bars_to_play: usize, seed: u64) -> Piece {
    // Am - F - C - G, `music.rs`'s `Voicing::of(Mood::Funk)`.
    let bars = vec![
        Bar::new(45, [57, 60, 64, 69]), // Am : A C E A
        Bar::new(41, [57, 60, 65, 69]), // F  : A C F A
        Bar::new(48, [60, 64, 67, 72]), // C  : C E G C
        Bar::new(43, [59, 62, 67, 71]), // G  : B D G B
    ];

    // Bass: dropped an octave, on `bass_gate`. `music.rs`'s `pulse()` shapes
    // every note as a flat 4ms-in/4ms-out ramp — full volume for the whole
    // gate, not a decay — so the faithful envelope here is a linear ADSR
    // with `sustain = 1.0` and both attack and release pinned to 4ms, *not*
    // `Adsr::pluck` (whose whole point, used elsewhere in this module, is a
    // continuous decay across the gate — the wrong shape for this voice).
    let bass = Part::new(
        Instrument::new(
            Waveform::Square { duty: 0.5 },
            Adsr::linear(0.004, 0.0, 1.0, 0.004),
        )
        .with_gain(0.42),
        Pattern::gated("x.x.x..xx.x.x.x.", &[0; 16], 0.9),
        NoteSource::Root,
    )
    .transposed(-12);

    // Lead: full arpeggio every step, a thin 30% duty, the same flat-sustain
    // envelope as the bass, a semitone-scaled vibrato standing in for the
    // reference's `vib/freq` phase wobble (a different formula, the same
    // audible ~2-6 Hz warble — see `Instrument`'s own docs on vibrato
    // depth), an octave up (the chip "bite" `lead_octave` gives).
    let lead = Part::new(
        Instrument::new(
            Waveform::Square { duty: 0.30 },
            Adsr::linear(0.004, 0.0, 1.0, 0.004),
        )
        .with_vibrato(6.0, 0.012)
        .with_gain(0.20),
        Pattern::gated(
            "xxxxxxxxxxxxxxxx",
            &[0, 2, 1, 3, 2, 3, 1, 2, 0, 2, 3, 2, 1, 3, 2, 3],
            0.95,
        ),
        NoteSource::Tone,
    )
    .transposed(12);

    // Kick: four to the bar. The reference amplitude-modulates noise by a
    // 90 Hz sine for its "punch"; this engine has a filter instead, which is
    // the more conventional way a subtractive synth gets a kick out of
    // noise — a short, resonant low-pass sweeping down from ~340 Hz to
    // ~120 Hz gives the same pitched thump. `shape` 0.3 bows the decay
    // segment so it drops fast and tails long, the closest this crate's
    // power-curve envelope gets to the reference's `exp(-55*t)`.
    let kick = Part::new(
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.11, 0.0, 0.02, 0.3))
            .with_filter(FilterSpec {
                base_cutoff_hz: 120.0,
                q: 2.0,
                env_amount_hz: 220.0,
                env: Adsr::shaped(0.0, 0.05, 0.0, 0.02, 0.3),
            })
            .with_gain(0.5),
        Pattern::gated("x...x...x...x...", &[0; 16], 0.9),
        NoteSource::Root,
    );

    // Hat: every offbeat eighth, unfiltered noise, fast decay.
    let hat = Part::new(
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.04, 0.0, 0.01, 0.3))
            .with_gain(0.14),
        Pattern::gated(".x.x.x.x.x.x.x.x", &[0; 16], 0.9),
        NoteSource::Root,
    );

    Piece {
        bars,
        parts: vec![bass, lead, kick, hat],
        bpm: 128.0,
        bars_to_play,
        key_offset_semitones: 0,
        seed,
    }
}

/// A new style the chiptune idiom cannot produce: a slow, filtered pad —
/// three stacked saw/triangle voices held across each bar through a
/// resonant low-pass whose cutoff opens on the attack, a soft sine "bell"
/// picking out a two-note motif over the top, and a heavily filtered noise
/// bed underneath for air. Original D-major progression (I-vi-IV-V), not
/// shared with any mood already in `music.rs`.
pub fn analog_pad(bars_to_play: usize, seed: u64) -> Piece {
    let bars = vec![
        Bar::new(62, [62, 66, 69, 74]), // D    : D F# A D
        Bar::new(59, [59, 62, 66, 71]), // Bm   : B D F# B
        Bar::new(55, [55, 59, 62, 67]), // G    : G B D G
        Bar::new(57, [57, 61, 64, 69]), // A    : A C# E A
    ];

    // A slow attack (the filter opening under it is what gives this its
    // "swell" character) and a long release so one chord's tail overlaps
    // the next chord's attack, rather than a hard-edged chip pluck.
    let pad_env = Adsr::linear(1.2, 0.3, 0.85, 1.5);
    let pad_filter = FilterSpec {
        base_cutoff_hz: 260.0,
        q: 0.9,
        env_amount_hz: 2400.0,
        env: Adsr::linear(1.4, 0.6, 0.55, 1.2),
    };

    let pad_low = Part::new(
        Instrument::new(Waveform::Saw, pad_env)
            .with_filter(pad_filter.clone())
            .with_gain(0.30),
        Pattern::sustained(0),
        NoteSource::Tone,
    )
    .transposed(-12);

    let pad_mid = Part::new(
        Instrument::new(Waveform::Saw, pad_env)
            .with_filter(pad_filter.clone())
            .with_gain(0.24),
        Pattern::sustained(1),
        NoteSource::Tone,
    )
    .transposed(-12);

    let pad_high = Part::new(
        Instrument::new(Waveform::Triangle, pad_env)
            .with_filter(FilterSpec {
                base_cutoff_hz: 500.0,
                q: 0.7,
                env_amount_hz: 3200.0,
                ..pad_filter.clone()
            })
            .with_gain(0.16),
        Pattern::sustained(2),
        NoteSource::Tone,
    );

    // A sparse two-note motif over the top, a full octave up, ringing out
    // rather than plucked — [`Adsr`]'s exponential-feeling `shape` used the
    // opposite way from the drums above: near 1.0 for a natural, slow
    // string-like decay instead of a fast percussive one.
    let bell = Part::new(
        Instrument::new(Waveform::Sine, Adsr::shaped(0.01, 1.6, 0.0, 0.6, 0.6))
            .with_gain(0.22),
        Pattern::gated(
            "x.....x...x.....",
            &[0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0],
            2.5,
        ),
        NoteSource::Tone,
    )
    .transposed(12);

    // Filtered noise air, barely audible on its own, glues the pad's attack
    // transient together — the same instinct that makes real pad patches
    // layer a little breath noise under the tone.
    let air = Part::new(
        Instrument::new(Waveform::Noise, Adsr::linear(2.0, 0.0, 0.4, 2.0))
            .with_filter(FilterSpec {
                base_cutoff_hz: 1400.0,
                q: 0.6,
                env_amount_hz: 0.0,
                env: Adsr::linear(0.0, 0.0, 1.0, 0.0),
            })
            .with_gain(0.05),
        Pattern::sustained(0),
        NoteSource::Root,
    );

    Piece {
        bars,
        parts: vec![pad_low, pad_mid, pad_high, bell, air],
        bpm: 80.0,
        bars_to_play,
        key_offset_semitones: 0,
        seed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chiptune_funk_renders_bounded_finite_audio() {
        let piece = chiptune_funk(4, 0xC17);
        let out = piece.render(48_000.0);
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| s.is_finite()));
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(peak <= 0.95, "{peak}");
    }

    #[test]
    fn chiptune_funk_is_deterministic_across_renders() {
        let a = chiptune_funk(2, 42).render(24_000.0);
        let b = chiptune_funk(2, 42).render(24_000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_seed_changes_the_drum_texture_but_not_the_length() {
        let a = chiptune_funk(2, 1).render(24_000.0);
        let b = chiptune_funk(2, 2).render(24_000.0);
        assert_eq!(a.len(), b.len());
        assert_ne!(a, b);
    }

    #[test]
    fn analog_pad_renders_bounded_finite_audio() {
        let piece = analog_pad(4, 7);
        let out = piece.render(48_000.0);
        assert!(!out.is_empty());
        assert!(out.iter().all(|s| s.is_finite()));
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(peak <= 0.95, "{peak}");
    }

    #[test]
    fn analog_pad_and_chiptune_funk_are_audibly_different_pieces() {
        // Same duration budget, wildly different envelope character: the pad
        // should have far less short-timescale amplitude variance than the
        // gated, plucky chiptune — a cheap, honest way to assert "different
        // style" without needing a human to listen for the unit test.
        let chip = chiptune_funk(4, 1).render(24_000.0);
        let pad = analog_pad(4, 1).render(24_000.0);
        let variance_of_deltas = |s: &[f32]| -> f64 {
            let deltas: Vec<f64> = s.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
            let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
            deltas.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / deltas.len() as f64
        };
        let chip_v = variance_of_deltas(&chip);
        let pad_v = variance_of_deltas(&pad);
        assert!(
            chip_v > pad_v,
            "chiptune ({chip_v}) should be spikier than the pad ({pad_v})"
        );
    }
}

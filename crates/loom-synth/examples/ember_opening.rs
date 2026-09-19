//! A ~31s DMG-hardware-authentic mimic of the *opening* of Kubbi's "Ember"
//! (2015), for a maintainer-requested short. Full derivation — the licence
//! finding, the measured tempo/key/structure of the reference, and exactly
//! what was taken versus written fresh — is in `docs/ember-cover/README.md`;
//! this file is the composition itself, following the same method
//! `opening_theme.rs` established (scale-degree composition, no melody
//! transcribed from a reference, `place_at_steps` for the `tone_index`
//! pitfall — see that file's own module docs for the full explanation of
//! why the naive `0..n` array silently plays wrong notes).
//!
//! # What is mimicked and what is original
//!
//! Tempo (~99.4 BPM measured, rounded to a clean 100), key centre (C
//! natural minor), the specific harmonic gesture of the opening (a long
//! drone on the flat-sixth — Ab, the "♭VI" chord — resolving to the tonic
//! only at the very end), and the arrangement's *shape* (a slow, layered
//! build with almost no rhythmic content early, brightening over time, one
//! energy/harmony "turn" landing right at the close) are all measured facts
//! about the reference, reproduced here. No melodic phrase, motif, or
//! specific pitch sequence was transcribed from the reference at any
//! point — there isn't a strong independent melody in its own opening 30s
//! to transcribe (see the measured analysis), and every note below was
//! chosen fresh, in scale-degree terms, the same way `opening_theme.rs`
//! writes its own melodies.
//!
//! # The four DMG channels, and why each got what it got
//!
//! - **Wave (triangle)**: the Ab pedal, later resolving to C. The
//!   reference's bass sits on one pitch class for its entire opening
//!   (measured dominance 0.83-0.97 out of 1.0 in the low band across 0-36s)
//!   — a genuine drone, not a moving bassline — so the wave channel here
//!   does the same: one held note per bar for eleven of this piece's
//!   twelve bars, only walking (the twelfth, second-to-last bar) to
//!   resolve onto the tonic in the last one.
//! - **Pulse 2 (50% duty, "comping")**: the steady eighth-note-ish pulse
//!   measured in the reference from its very first bar (onset spacing
//!   ~0.28-0.30s, which is almost exactly a 100bpm eighth note, 0.30s) —
//!   present quietly under the drone from early on, then deliberately
//!   thinned out in the "hold" section, mirroring the measured *drop* in
//!   onset density from ~2.4 to ~1.3 (onset-strength units) between 8-30s
//!   even as loudness kept climbing: the reference gets louder there by
//!   sustaining more, not by hitting more.
//! - **Pulse 1 (duty widens then narrows, "brightening")**: stands in for
//!   the reference's rising spectral centroid (1200Hz -> 2200Hz across
//!   roughly 4-20s) as new pad/texture layers audibly enter. A DMG pulse
//!   channel's only timbral control *is* duty cycle — narrower duty pushes
//!   energy into higher harmonics — so narrowing it over the piece is the
//!   hardware-native way to represent "the mix is getting brighter",
//!   rather than reaching for a filter sweep the real chip does not have.
//! - **Noise**: almost silent until the very end. The reference has no
//!   real drum kit in its first 30 seconds either (checked: onset count per
//!   5s bucket is flat, ~30-40, not climbing the way a beat entering would
//!   look) — a few near-inaudible ticks late, then one clear accented hit
//!   timed to the bass's resolution, standing in for the loudness/bass
//!   jump measured just past the reference's own 30s mark (RMS 0.14->0.26
//!   around 36-42s — this piece compresses that turn to land right at its
//!   own close, since a ~31s piece has no room to also play out a further
//!   ~6s of held drone before it).
//!
//! What is deliberately dropped, because a DMG has four channels and the
//! reference is "a full modern chiptune production with far more than
//! four voices": any reverb/wash (no DMG channel has one), whatever
//! additional simultaneous pad/synth layers the real mix carries beyond
//! one bass drone and one comping pulse (collapsed into those two), and any
//! full percussion kit (there isn't a clear one to keep in this window
//! anyway).

use loom_synth::envelope::Adsr;
use loom_synth::osc::Waveform;
use loom_synth::sequence::{Bar, NoteSource, Part, Pattern, Piece};
use loom_synth::voice::Instrument;
use loom_synth::wav;

/// C natural-minor intervals (semitones above the tonic), degrees 1-7:
/// C, D, Eb, F, G, Ab, Bb. Parallel to `opening_theme.rs`'s own `MAJOR`
/// table and `deg` helper, just built on the minor third/sixth/seventh the
/// measured reference actually centres on (a sustained Ab/♭VI drone makes
/// no sense read against a major scale).
const MINOR: [i32; 7] = [0, 2, 3, 5, 7, 8, 10];

/// Scale degree `n` (`1` = tonic) to a MIDI note relative to `root` — same
/// contract as `opening_theme.rs::deg`, against `MINOR` instead of `MAJOR`.
fn deg(root: i32, degree: i32) -> i32 {
    let d = degree - 1;
    let octave = d.div_euclid(7);
    let idx = d.rem_euclid(7) as usize;
    root + MINOR[idx] + 12 * octave
}

fn degs(root: i32, degrees: &[i32]) -> Vec<i32> {
    degrees.iter().map(|&d| deg(root, d)).collect()
}

/// The diatonic triad on scale degree `d`. `triad(root, 6)` (the reference's
/// own opening chord) comes out `[Ab, C, Eb]` — an Ab *major* triad, exactly
/// because degree 6's own diatonic third and fifth land on naturally major
/// intervals in a natural minor scale. `triad(root, 1)` comes out `[C, Eb,
/// G]`, C minor — the arrival chord. Same mechanism as
/// `opening_theme.rs::triad`, just against `MINOR`.
fn triad(root: i32, d: i32) -> [i32; 3] {
    [deg(root, d), deg(root, d + 2), deg(root, d + 4)]
}

/// Build one bar whose `tones` are `chord_root`'s triad on degree `d`
/// (indices `0..3`), followed by any extra pitches at `extra_root` (indices
/// `3..`) — used only by the closing "turn" bar's walking bassline. Returns
/// the bar plus the extra range, the value [`Pattern::gated`]'s
/// `tone_index` actually needs (see `place_at_steps`).
fn mk_bar(
    bass_root: i32,
    chord_root: i32,
    d: i32,
    extra_root: i32,
    extra_degrees: &[i32],
) -> (Bar, std::ops::Range<usize>) {
    let mut tones = triad(chord_root, d).to_vec();
    let extra = degs(extra_root, extra_degrees);
    let n = extra.len();
    tones.extend(extra);
    (Bar::new(deg(bass_root, d), tones), 3..3 + n)
}

/// Build a full step-indexed `tone_index` array (length `gate.len()`) that
/// assigns `values[0]` to the first `'x'` step of `gate`, `values[1]` to
/// the second, and so on. `Pattern::gated`'s own `tone_index` is read by
/// *absolute step position*, not by which active hit number a step is, so
/// handing it a short `0..n` array indexed as if hit-order mattered reads
/// out of range on every step past the array's length and silently
/// defaults to `0` — a valid tone index, so the failure is "the wrong,
/// plausible-looking note plays," never a panic. See `opening_theme.rs`'s
/// module docs for the fuller account of this pitfall.
fn place_at_steps(gate: &str, values: &[usize]) -> Vec<usize> {
    let mut out = vec![0usize; gate.len()];
    let mut hit = 0;
    for (step, ch) in gate.bytes().enumerate() {
        if ch == b'x' {
            out[step] = values.get(hit).copied().unwrap_or(0);
            hit += 1;
        }
    }
    out
}

/// Defensive check: every bar a shared `Pattern` plays over must have the
/// same `tones.len()`, or that pattern's tone-index values (sized for one
/// bar's layout) silently wrap into a *different* bar's tones once
/// `bars_to_play` cycles onto it. Same check `opening_theme.rs` runs before
/// every `Piece` is built.
fn assert_uniform_tone_layout(label: &str, bars: &[Bar]) {
    let Some(first) = bars.first().map(|b| b.tones.len()) else {
        return;
    };
    for (i, b) in bars.iter().enumerate() {
        assert_eq!(
            b.tones.len(),
            first,
            "{label}: bar {i} has {} tones, bar 0 has {first} — a shared \
             tone_index would read the wrong bar's chord the moment this \
             pattern repeats",
            b.tones.len()
        );
    }
}

fn piece(
    label: &str,
    bars: Vec<Bar>,
    parts: Vec<Part>,
    bpm: f64,
    bars_to_play: usize,
) -> Piece {
    assert_uniform_tone_layout(label, &bars);
    Piece {
        bars,
        parts,
        bpm,
        bars_to_play,
        key_offset_semitones: 0,
        seed: {
            // A stable, distinct seed per section label so re-running this
            // file always renders the same bytes (the noise channel is the
            // only part that reads the RNG at all).
            let mut h: u64 = 1469598103934665603; // FNV offset basis
            for b in label.bytes() {
                h ^= b as u64;
                h = h.wrapping_mul(1099511628211);
            }
            h
        },
    }
}

fn render_pieces(pieces: &[Piece], sample_rate_hz: f64) -> Vec<f32> {
    let mut out = Vec::new();
    for p in pieces {
        out.extend(p.render(sample_rate_hz));
    }
    out
}

const BASS: i32 = 48; // C3 — wave-channel register (transposed -12 to C2 in the part below)
const CHORD: i32 = 60; // C4 — pulse-2 comping register
const BPM: f64 = 100.0;

const G8: &str = "x.x.x.x.x.x.x.x.";
const G4: &str = "x...x...x...x...";
const SPARSE2: &str = "x.......x......."; // two hits/bar — half notes
const SPARSE1: &str = "x..............."; // one hit/bar

pub fn ember_opening() -> Vec<Piece> {
    // Bass instrument (wave/triangle): a soft-attack, long-release pad —
    // deliberately NOT variant_c's punchy pluck-style bass. The reference's
    // bass is a held drone, not a rhythmic bassline, so the envelope needs
    // to sound like one.
    let bass_inst = || {
        Instrument::new(Waveform::Triangle, Adsr::linear(0.03, 0.08, 0.85, 0.35))
            .with_gain(0.40)
    };
    // Pulse 2, the steady comping pulse — wide (50%) duty throughout, since
    // it never needs to carry "brightening" on its own; only its rhythmic
    // density changes (dense early, sparse in the hold, dense again at the
    // turn).
    let comp_inst = || {
        Instrument::new(Waveform::Square { duty: 0.5 }, Adsr::pluck(0.006, 0.16))
            .with_gain(0.13)
    };
    // Pulse 1, the "brightening" layer: three instruments sharing a
    // pattern shape, differing only in duty cycle and gain — the one
    // parameter a real DMG pulse channel has for timbre.
    let lead_warm = || {
        Instrument::new(
            Waveform::Square { duty: 0.5 },
            Adsr::linear(0.04, 0.06, 0.7, 0.25),
        )
        .with_gain(0.12)
    };
    let lead_bright = || {
        Instrument::new(
            Waveform::Square { duty: 0.125 },
            Adsr::linear(0.02, 0.05, 0.75, 0.2),
        )
        .with_gain(0.19)
    };
    let lead_turn = || {
        Instrument::new(
            Waveform::Square { duty: 0.25 },
            Adsr::linear(0.01, 0.04, 0.8, 0.2),
        )
        .with_gain(0.26)
    };
    // Noise — near-silent shimmer ticks, then one clear accented hit at
    // the resolution (variant_c's own kick_inst shape, at a lower gain
    // for the ticks).
    let shimmer_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.03, 0.0, 0.02, 0.3))
            .with_gain(0.055)
    };
    let accent_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.08, 0.0, 0.02, 0.3))
            .with_gain(0.40)
    };

    // -- 1. pedal_solo (1 bar) — the wave channel alone, matching the
    //    reference's own thinnest possible opening.
    let (b_pedal, _) = mk_bar(BASS, CHORD, 6, BASS, &[]);
    let pedal_solo = piece(
        "ember pedal solo",
        vec![b_pedal.clone()],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
        ],
        BPM,
        1,
    );

    // -- 2. pedal_pulse (2 bars) — the comping pulse enters, alternating
    //    the drone's root and fifth (Ab, Eb) on the measured ~0.3s pulse.
    let pedal_pulse = piece(
        "ember pedal pulse",
        vec![b_pedal.clone()],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G8, &[0, 2, 0, 2, 0, 2, 0, 2], 0.7),
                NoteSource::Tone,
            ),
        ],
        BPM,
        2,
    );

    // -- 3. brighten_a (2 bars) — pulse 1 enters, wide/warm duty, a slow
    //    held tone (no independent melody: the reference doesn't have a
    //    strong one here either).
    let brighten_a = piece(
        "ember brighten a",
        vec![b_pedal.clone()],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G8, &[0, 2, 0, 2, 0, 2, 0, 2], 0.7),
                NoteSource::Tone,
            ),
            Part::new(
                lead_warm(),
                Pattern::gated(SPARSE2, &[0, 0], 3.5),
                NoteSource::Tone,
            )
            .transposed(12),
        ],
        BPM,
        2,
    );

    // -- 4. brighten_b (2 bars) — pulse 1 narrows to bright duty, noise's
    //    first faint ticks appear.
    let brighten_b = piece(
        "ember brighten b",
        vec![b_pedal.clone()],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G8, &[0, 2, 0, 2, 0, 2, 0, 2], 0.7),
                NoteSource::Tone,
            ),
            Part::new(
                lead_bright(),
                Pattern::gated(SPARSE2, &[0, 0], 3.5),
                NoteSource::Tone,
            )
            .transposed(12),
            Part::new(
                shimmer_inst(),
                Pattern::gated(SPARSE2, &[0, 0], 0.5),
                NoteSource::Root,
            ),
        ],
        BPM,
        2,
    );

    // -- 5. hold (3 bars) — sustained plateau; the comping pulse THINS
    //    OUT (measured onset-density drop, 8-30s: the mix gets louder by
    //    sustaining, not by hitting more), everything else holds steady.
    let hold = piece(
        "ember hold",
        vec![b_pedal.clone()],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(SPARSE2, &[0, 2], 1.5),
                NoteSource::Tone,
            ),
            Part::new(
                lead_bright(),
                Pattern::gated(SPARSE1, &[0], 3.9),
                NoteSource::Tone,
            )
            .transposed(12),
            Part::new(
                shimmer_inst(),
                Pattern::gated(SPARSE1, &[0], 0.5),
                NoteSource::Root,
            ),
        ],
        BPM,
        3,
    );

    // -- 6. turn (2 bars) — the one harmonic event in the whole piece: the
    //    wave channel walks Ab-G-F-Eb (degrees 6,5,4,3) across bar 1's four
    //    beats and resolves onto held C (degree 1, the tonic) in bar 2,
    //    while pulse 1/2 both step up in density and gain (the measured
    //    RMS jump) and the noise channel lands its one accented hit
    //    exactly on the resolution.
    let (b_turn1, walk_range) = mk_bar(BASS, CHORD, 6, BASS, &[6, 5, 4, 3]);
    let turn_a = piece(
        "ember turn walk",
        vec![b_turn1],
        vec![
            Part::new(
                bass_inst(),
                Pattern::gated(
                    G4,
                    &place_at_steps(G4, &walk_range.collect::<Vec<_>>()),
                    3.9,
                ),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G8, &[0, 2, 0, 2, 0, 2, 0, 2], 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                lead_turn(),
                Pattern::gated(G4, &[0, 2, 0, 2], 3.5),
                NoteSource::Tone,
            )
            .transposed(12),
        ],
        BPM,
        1,
    );
    let (b_turn2, _) = mk_bar(BASS, CHORD, 1, BASS, &[]);
    let turn_b = piece(
        "ember turn arrival",
        vec![b_turn2],
        vec![
            Part::new(bass_inst(), Pattern::sustained(0), NoteSource::Root)
                .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G8, &[0, 2, 0, 2, 0, 2, 0, 2], 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                lead_turn(),
                Pattern::gated(SPARSE1, &[0], 15.5),
                NoteSource::Tone,
            )
            .transposed(12),
            Part::new(
                accent_inst(),
                Pattern::gated("x...............", &[0], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
    );

    vec![
        pedal_solo,
        pedal_pulse,
        brighten_a,
        brighten_b,
        hold,
        turn_a,
        turn_b,
    ]
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = argv
        .first()
        .cloned()
        .unwrap_or_else(|| "docs/ember-cover".to_string());
    std::fs::create_dir_all(&out_dir).expect("create output dir");
    let sample_rate = 48_000u32;

    let pieces = ember_opening();
    let mut t = 0.0;
    print!("ember-dmg-opening sections:");
    for p in &pieces {
        let d = p.duration_secs();
        print!(" [{t:.3}-{:.3}]", t + d);
        t += d;
    }
    println!();

    let samples = render_pieces(&pieces, sample_rate as f64);
    let path = std::path::PathBuf::from(&out_dir).join("ember-dmg-opening.wav");
    wav::write_mono(&path, &samples, sample_rate).expect("write wav");
    let secs = samples.len() as f64 / sample_rate as f64;
    let peak = samples.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
    let rms = (samples.iter().map(|&v| (v as f64).powi(2)).sum::<f64>()
        / samples.len().max(1) as f64)
        .sqrt();
    println!(
        "{}: {:.2}s, peak {:.3}, rms {:.3}",
        path.display(),
        secs,
        peak,
        rms
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bar_in_every_piece_has_a_consistent_tone_layout() {
        // If this test needs to be added by hand, `piece()` already asserts
        // it on every call in `ember_opening()` — this just proves the
        // whole set builds without panicking, which is the same check.
        let _ = ember_opening();
    }

    #[test]
    fn rendering_is_deterministic() {
        let a = render_pieces(&ember_opening(), 24_000.0);
        let b = render_pieces(&ember_opening(), 24_000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn nothing_exceeds_the_soft_limiter_headroom() {
        let out = render_pieces(&ember_opening(), 22_050.0);
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(
            peak <= 0.95,
            "peak {peak} should sit inside 0.9 headroom plus a little margin"
        );
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn the_piece_is_roughly_thirty_seconds() {
        let pieces = ember_opening();
        let total: f64 = pieces
            .iter()
            .map(|p| p.bars_to_play as f64 * p.bar_secs())
            .sum();
        assert!(
            (28.0..=33.0).contains(&total),
            "total bar time {total}s should be ~30s"
        );
    }
}

//! A small step sequencer over a chord progression — enough to write an
//! actual piece (notes, timing, patterns) without becoming a DAW. One bar is
//! sixteen steps (sixteenth notes at the piece's tempo), the same grid a
//! tracker or a step sequencer conventionally uses; a [`Piece`] cycles
//! through its [`Bar`]s and, on each, triggers whichever [`NoteEvent`]s each
//! [`Part`]'s [`Pattern`] schedules for that bar — from a short gated pluck
//! (a chiptune arpeggio) to a single note held across the whole bar (a pad).
//! Both are the same mechanism at different `length_steps`, not two features.

use crate::rng::Rng;
use crate::voice::Instrument;

pub const STEPS_PER_BAR: usize = 16;

/// One bar of a progression: a root note (used by a bass-style [`Part`]) and
/// the chord tones a lead or pad arpeggiates or sustains over it. All in
/// MIDI note numbers (`60` = middle C).
#[derive(Debug, Clone)]
pub struct Bar {
    pub root_midi: i32,
    pub tones: Vec<i32>,
}

impl Bar {
    pub fn new(root_midi: i32, tones: impl IntoIterator<Item = i32>) -> Self {
        Bar {
            root_midi,
            tones: tones.into_iter().collect(),
        }
    }
}

/// One scheduled note within a bar.
#[derive(Debug, Clone, Copy)]
pub struct NoteEvent {
    /// Which of the bar's [`STEPS_PER_BAR`] sixteenth-note steps this note
    /// starts on.
    pub step: usize,
    /// Index into the current bar's `tones` (wrapped by length, so a pattern
    /// need not know how many tones a chord has).
    pub tone_index: usize,
    /// How many steps long the note's *gate* is — its envelope's release
    /// still rings on past this, see [`Instrument::render_note`]. `16.0`
    /// (or more) sustains across the whole bar; a short chiptune pluck is
    /// well under `1.0`.
    pub length_steps: f64,
}

/// A bar's worth of scheduled notes for one [`Part`].
#[derive(Debug, Clone, Default)]
pub struct Pattern {
    pub events: Vec<NoteEvent>,
}

impl Pattern {
    pub fn new(events: impl IntoIterator<Item = NoteEvent>) -> Self {
        Pattern {
            events: events.into_iter().collect(),
        }
    }

    /// A gated arpeggio from the `x`/`.` shorthand a step sequencer or
    /// tracker conventionally uses: one character per of [`STEPS_PER_BAR`]
    /// steps, `x` triggers a note at `tone_index[step] % tones.len()`
    /// (resolved later, against whichever bar is playing), `.` rests.
    /// `note_len_steps` is each triggered note's gate length.
    pub fn gated(gate: &str, tone_index: &[usize], note_len_steps: f64) -> Self {
        let g: Vec<u8> = gate.bytes().collect();
        let events = (0..STEPS_PER_BAR.min(g.len()))
            .filter(|&s| g[s] == b'x')
            .map(|s| NoteEvent {
                step: s,
                tone_index: tone_index.get(s).copied().unwrap_or(0),
                length_steps: note_len_steps,
            })
            .collect();
        Pattern { events }
    }

    /// A single note sustained across the whole bar, at `tone_index`.
    pub fn sustained(tone_index: usize) -> Self {
        Pattern {
            events: vec![NoteEvent {
                step: 0,
                tone_index,
                length_steps: STEPS_PER_BAR as f64,
            }],
        }
    }
}

/// One voice's part across the whole piece: an instrument, the pattern it
/// repeats every bar, and how far its chord tone sits from the bar's written
/// pitch (a bass typically wants `-12`, dropping the root an octave).
#[derive(Debug, Clone)]
pub struct Part {
    pub instrument: Instrument,
    pub pattern: Pattern,
    /// Semitones added to every note this part plays — octave shifts and the
    /// "chip bite" lift ShowReel's reference lead uses both fall out of this
    /// one field.
    pub transpose_semitones: i32,
    /// Whether this part reads the bar's *root* (a bass line) or its
    /// `tones` (a lead or pad).
    pub source: NoteSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteSource {
    Root,
    Tone,
}

impl Part {
    pub fn new(instrument: Instrument, pattern: Pattern, source: NoteSource) -> Self {
        Part {
            instrument,
            pattern,
            transpose_semitones: 0,
            source,
        }
    }

    pub fn transposed(mut self, semitones: i32) -> Self {
        self.transpose_semitones = semitones;
        self
    }
}

/// A complete piece: a chord progression, the parts playing over it, and the
/// tempo/length that turns bars into seconds.
#[derive(Debug, Clone)]
pub struct Piece {
    pub bars: Vec<Bar>,
    pub parts: Vec<Part>,
    pub bpm: f64,
    /// How many bars to play, cycling through `bars` — the piece's total
    /// length is `bars_to_play` bars at `bpm`, independent of `bars.len()`.
    pub bars_to_play: usize,
    pub key_offset_semitones: i32,
    pub seed: u64,
}

impl Piece {
    /// MIDI note number to frequency, A4 (69) = 440 Hz.
    pub fn midi_hz(n: i32) -> f64 {
        440.0 * 2f64.powf((n - 69) as f64 / 12.0)
    }

    /// Seconds per bar (four beats) at `bpm`.
    pub fn bar_secs(&self) -> f64 {
        4.0 * 60.0 / self.bpm.max(1.0)
    }

    /// The piece's own natural length in seconds — `bars_to_play` bars, plus
    /// the release tail of the longest envelope in play so nothing is cut
    /// off mid-ring.
    pub fn duration_secs(&self) -> f64 {
        let bars = self.bars_to_play as f64 * self.bar_secs();
        let longest_release = self
            .parts
            .iter()
            .map(|p| p.instrument.amp_env.release_secs)
            .fold(0.0f64, f64::max);
        bars + longest_release
    }

    /// Render the whole piece to mono `f32` samples at `sample_rate_hz`,
    /// mixed and soft-limited into headroom (see [`crate::mix::soft_limit`]).
    pub fn render(&self, sample_rate_hz: f64) -> Vec<f32> {
        if self.bars.is_empty() || self.parts.is_empty() || self.bpm <= 0.0 {
            return Vec::new();
        }
        let bar_secs = self.bar_secs();
        let step_secs = bar_secs / STEPS_PER_BAR as f64;
        let total_secs = self.duration_secs();
        let mut mix = vec![0.0f32; (total_secs * sample_rate_hz).round() as usize];
        let mut rng = Rng::new(self.seed);

        for bar_idx in 0..self.bars_to_play {
            let bar = &self.bars[bar_idx % self.bars.len()];
            let t0 = bar_idx as f64 * bar_secs;
            for part in &self.parts {
                for ev in &part.pattern.events {
                    let base_note = match part.source {
                        NoteSource::Root => bar.root_midi,
                        NoteSource::Tone => {
                            if bar.tones.is_empty() {
                                bar.root_midi
                            } else {
                                bar.tones[ev.tone_index % bar.tones.len()]
                            }
                        }
                    };
                    let note =
                        base_note + part.transpose_semitones + self.key_offset_semitones;
                    let freq = Self::midi_hz(note);
                    let gate_secs = ev.length_steps * step_secs;
                    let at = t0 + ev.step as f64 * step_secs;
                    let note_samples = part.instrument.render_note(
                        freq,
                        gate_secs,
                        sample_rate_hz,
                        &mut rng,
                    );
                    place(&mut mix, &note_samples, at, sample_rate_hz);
                }
            }
        }
        crate::mix::soft_limit(&mut mix, 0.9);
        mix
    }
}

/// Additively place `sig` into `buf` starting at `at_secs`, clamped to the
/// buffer's end.
fn place(buf: &mut [f32], sig: &[f32], at_secs: f64, sample_rate_hz: f64) {
    let i = (at_secs * sample_rate_hz).round().max(0.0) as usize;
    if i >= buf.len() {
        return;
    }
    let j = (i + sig.len()).min(buf.len());
    for k in i..j {
        buf[k] += sig[k - i];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Adsr;
    use crate::osc::Waveform;

    fn simple_piece() -> Piece {
        let bar = Bar::new(60, [60, 64, 67]); // C major triad
        let bass = Part::new(
            Instrument::new(Waveform::Square { duty: 0.5 }, Adsr::pluck(0.001, 0.08)),
            Pattern::gated("x...x...x...x...", &[0, 0, 0, 0], 0.8),
            NoteSource::Root,
        )
        .transposed(-12);
        let lead = Part::new(
            Instrument::new(Waveform::Square { duty: 0.25 }, Adsr::pluck(0.002, 0.1)),
            Pattern::gated("x.x.x.x.x.x.x.x.", &[0, 1, 2, 0, 1, 2, 0, 1], 0.5),
            NoteSource::Tone,
        );
        Piece {
            bars: vec![bar],
            parts: vec![bass, lead],
            bpm: 120.0,
            bars_to_play: 2,
            key_offset_semitones: 0,
            seed: 1,
        }
    }

    #[test]
    fn a_piece_renders_something_bounded_and_finite() {
        let piece = simple_piece();
        let out = piece.render(48_000.0);
        assert!(!out.is_empty());
        let peak = out.iter().fold(0.0f32, |m, &v| m.max(v.abs()));
        assert!(
            peak <= 0.95,
            "peak {peak} should be inside the 0.9 headroom target plus a little margin"
        );
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn duration_matches_bars_to_play_plus_release() {
        let piece = simple_piece();
        let want = piece.bars_to_play as f64 * piece.bar_secs() + 0.08_f64.max(0.1);
        // Both parts release within 0.1s here; the exact value is asserted
        // via `duration_secs`, which `render`'s buffer length must match.
        let got_len = piece.render(1000.0).len();
        let expect_len = (piece.duration_secs() * 1000.0).round() as usize;
        assert_eq!(got_len, expect_len);
        assert!(piece.duration_secs() >= want - 0.2);
    }

    #[test]
    fn rendering_is_deterministic() {
        let piece = simple_piece();
        let a = piece.render(24_000.0);
        let b = piece.render(24_000.0);
        assert_eq!(a, b);
    }

    #[test]
    fn a_sustained_pattern_holds_across_the_whole_bar() {
        let bar = Bar::new(48, [48, 52, 55]);
        let pad = Part::new(
            Instrument::new(Waveform::Sine, Adsr::linear(0.05, 0.0, 1.0, 0.05)),
            Pattern::sustained(0),
            NoteSource::Tone,
        );
        let piece = Piece {
            bars: vec![bar],
            parts: vec![pad],
            bpm: 60.0,
            bars_to_play: 1,
            key_offset_semitones: 0,
            seed: 0,
        };
        let out = piece.render(1000.0);
        // A whole bar at 60 BPM is 4 seconds; well into the bar the pad
        // should still be sounding (not silent), unlike a short pluck would be.
        let mid = out[2000]; // 2.0s in
        assert!(
            mid.abs() > 0.05,
            "pad should still be sounding at 2s, got {mid}"
        );
    }

    #[test]
    fn empty_piece_renders_no_samples_rather_than_panicking() {
        let piece = Piece {
            bars: vec![],
            parts: vec![],
            bpm: 120.0,
            bars_to_play: 4,
            key_offset_semitones: 0,
            seed: 0,
        };
        assert!(piece.render(48_000.0).is_empty());
    }
}

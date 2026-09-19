//! Three original ~30s opening-theme sketches for a TerminalGB short — the
//! maintainer's ask was "the feeling of the Pokémon opening, with a melody
//! that's genuinely ours." Full derivation, and exactly what each variant
//! borrowed from the era versus wrote fresh, is in
//! `docs/opening-theme/README.md`; this file is the composition itself.
//!
//! # The one rule this file exists to satisfy
//!
//! No phrase here is a transcription, transposition, or rhythmic
//! rearrangement of any published Pokémon melody (game or anime). What *is*
//! deliberately borrowed, because none of it is anyone's property: the
//! driving mid-tempo march feel, a triumphant major-key progression, a
//! short-fanfare/build/arrival structure, and late-80s/early-90s handheld
//! chiptune instrumentation. Every pitch sequence below was authored fresh
//! for this file, in scale-degree terms, using ordinary tonal-composition
//! devices (arpeggiated calls, stepwise answers, sequences, a rising
//! "lift") — never by recalling and altering an existing tune.
//!
//! # How the melodies are written
//!
//! [`sequence::Bar::tones`] is one flat `Vec<i32>`. Each bar below packs a
//! **diatonic triad** (indices `0..3`, for a comping/arpeggio part) followed
//! by that bar's **melody phrase** (indices `3..`, for the lead part) into
//! that one vector — two independent voices sharing a bar's harmonic
//! content without the engine needing to know the difference. [`deg`] turns
//! a scale-degree number (`1` = tonic, `8` = the octave, negative/large
//! values wrap through octaves via `div_euclid`/`rem_euclid`) into a MIDI
//! note relative to a voice's own register root, so every melody below reads
//! as a list of scale steps rather than hand-computed pitch numbers — which
//! also makes it impossible to write an out-of-key note by mistake.
//!
//! `Pattern::gated`'s `tone_index` array is read **by absolute step
//! position** (`tone_index[step]`), not by which active hit number a step
//! is — so [`place_at_steps`] is what turns "the Nth active hit plays
//! melody note N" into the step-indexed array the engine actually wants.
//! Getting this backwards doesn't panic, it silently wraps into the wrong
//! bar's chord tones, so [`assert_uniform_tone_layout`] catches the
//! precondition it depends on (every bar a shared pattern plays over having
//! the same `tones.len()`) at build time instead.
//!
//! Each variant is composed from several short, independently-instrumented
//! [`sequence::Piece`]s — one per *rhythmic idea* — rendered separately and
//! concatenated. A `Part`'s `Pattern` is one template reused for every bar
//! of its `Piece`, so wherever the arrangement wants a genuinely different
//! rhythm from one bar to the next (the intro's dense run answered by a
//! sparser response; the arrival's rhythmic flourish giving way to a long
//! held chord) that has to be a new `Piece`, not a longer one — seams that
//! are inaudible because [`envelope::Adsr::render`] always tapers a
//! section's buffer to exactly `0.0` by the end of its own release, so
//! concatenation needs no crossfade (see `render_variant`).

use loom_synth::envelope::Adsr;
use loom_synth::osc::Waveform;
use loom_synth::sequence::{Bar, NoteSource, Part, Pattern, Piece};
use loom_synth::voice::{FilterSpec, Instrument};
use loom_synth::wav;

/// Diatonic major-scale intervals (semitones above the tonic), degrees 1-7.
const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];

/// Scale degree `n` (`1` = tonic, `8` = the octave above, `0`/negative =
/// below it) to a MIDI note, relative to `root` (the MIDI note *of* degree
/// 1 in whichever register this voice plays in).
fn deg(root: i32, degree: i32) -> i32 {
    let d = degree - 1;
    let octave = d.div_euclid(7);
    let idx = d.rem_euclid(7) as usize;
    root + MAJOR[idx] + 12 * octave
}

fn degs(root: i32, degrees: &[i32]) -> Vec<i32> {
    degrees.iter().map(|&d| deg(root, d)).collect()
}

/// The diatonic triad built on scale degree `d` (stacking two more thirds:
/// `d`, `d+2`, `d+4`) — this is what makes a `vi` bar come out minor and a
/// `V` bar come out major with no chord-quality bookkeeping anywhere else:
/// the quality falls straight out of which scale steps `MAJOR` has at those
/// three degrees.
fn triad(root: i32, d: i32) -> [i32; 3] {
    [deg(root, d), deg(root, d + 2), deg(root, d + 4)]
}

/// Build one bar: `bass_root`'s scale degree `d` is the bar's harmonic root
/// (what a `NoteSource::Root` part plays), `chord_root`'s triad on the same
/// degree fills tone-indices `0..3` (a comping/arpeggio part), and
/// `melody_root`'s `melody_degrees` fill everything from `3` on (the lead).
/// Returns the bar plus the melody's *absolute* tone-index range (`3..3+n`)
/// — the value [`Pattern::gated`]'s `tone_index` actually needs, not `0..n`.
fn mk_bar(
    bass_root: i32,
    chord_root: i32,
    d: i32,
    melody_root: i32,
    melody_degrees: &[i32],
) -> (Bar, std::ops::Range<usize>) {
    let mut tones = triad(chord_root, d).to_vec();
    let melody = degs(melody_root, melody_degrees);
    let n = melody.len();
    tones.extend(melody);
    (Bar::new(deg(bass_root, d), tones), 3..3 + n)
}

/// `0, 1, 2, 0, 1, 2, ..` up to `hits` entries — a comping/arpeggio part's
/// absolute tone-index sequence cycling the three chord tones at `0..3`.
fn cycle3(hits: usize) -> Vec<usize> {
    (0..hits).map(|i| i % 3).collect()
}

/// Build a full step-indexed `tone_index` array (length `gate.len()`, `0`
/// — unread, since only active steps are ever looked up — elsewhere) that
/// assigns `values[0]` to the first `'x'` step of `gate`, `values[1]` to
/// the second, and so on. This is the mapping [`Pattern::gated`] actually
/// needs: its own `tone_index.get(step)` is indexed by absolute step
/// position, not by which active hit number a step is, so handing it a
/// short `0..n` array (indexed as if hit-order mattered) silently reads
/// out-of-range on every step past the array's length and defaults to `0`
/// — which is a valid chord-tone index, so the failure is "the wrong,
/// plausible-looking note plays," never a panic.
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

/// Defensive check, run before every `Piece` is built: every bar a shared
/// `Pattern` plays over must have the same `tones.len()`, or that pattern's
/// tone-index values (sized for one bar's chord+melody layout) wrap into a
/// *different* bar's chord tones the moment `bars_to_play` cycles onto it —
/// silently, since `%` never panics. See this file's module docs.
fn assert_uniform_tone_layout(label: &str, bars: &[Bar]) {
    let Some(first) = bars.first().map(|b| b.tones.len()) else {
        return;
    };
    for (i, b) in bars.iter().enumerate() {
        assert_eq!(
            b.tones.len(),
            first,
            "{label}: bar {i} has {} tones, bar 0 has {first} — a shared \
             Tone-source pattern would wrap into the wrong bar's chord/melody split",
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
    seed: u64,
) -> Piece {
    assert_uniform_tone_layout(label, &bars);
    Piece {
        bars,
        parts,
        bpm,
        bars_to_play,
        key_offset_semitones: 0,
        seed,
    }
}

/// Render every piece in sequence and concatenate — see the module docs for
/// why no crossfade is needed at the seams.
fn render_variant(pieces: &[Piece], sample_rate_hz: f64) -> Vec<f32> {
    let mut out = Vec::new();
    for p in pieces {
        out.extend(p.render(sample_rate_hz));
    }
    out
}

// ---------------------------------------------------------------------
// Variant A — "safer / more generic": D major, 112 BPM, moderate energy.
// A bright pop-chiptune march: square lead with light vibrato, a simple
// arpeggio comping part, steady pulse bass, kick/hat. I-V-vi-IV — about as
// common and unclaimable a progression as exists.
// ---------------------------------------------------------------------

pub fn variant_a() -> Vec<Piece> {
    const BASS: i32 = 50; // D3
    const CHORD: i32 = 62; // D4
    const LEAD: i32 = 74; // D5
    const BPM: f64 = 112.0;

    let lead_inst = || {
        Instrument::new(
            Waveform::Square { duty: 0.28 },
            Adsr::linear(0.006, 0.02, 0.75, 0.05),
        )
        .with_vibrato(5.5, 0.010)
        .with_gain(0.30)
    };
    let comp_inst = || {
        Instrument::new(Waveform::Square { duty: 0.5 }, Adsr::pluck(0.004, 0.12))
            .with_gain(0.16)
    };
    let bass_inst = || {
        Instrument::new(Waveform::Triangle, Adsr::linear(0.004, 0.0, 1.0, 0.03))
            .with_gain(0.40)
    };
    let kick_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.10, 0.0, 0.02, 0.3))
            .with_filter(FilterSpec {
                base_cutoff_hz: 110.0,
                q: 2.0,
                env_amount_hz: 200.0,
                env: Adsr::shaped(0.0, 0.05, 0.0, 0.02, 0.3),
            })
            .with_gain(0.46)
    };
    let hat_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.035, 0.0, 0.01, 0.3))
            .with_gain(0.11)
    };

    // -- Intro: a bold rising call across two bars — D5-A5, then B5-D6 —
    //    the very first thing a listener hears, deliberately a leap-based
    //    call rather than a repeated-note hook. Both bars have a 2-note
    //    melody over the same 2-hit gate, so one Piece covers both.
    const G2: &str = "x.......x.......";
    let (b1, r1) = mk_bar(BASS, CHORD, 1, LEAD, &[1, 5]);
    let (b2, _) = mk_bar(BASS, CHORD, 5, LEAD, &[6, 8]);
    let intro = piece(
        "A intro",
        vec![b1, b2],
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(
                    G2,
                    &place_at_steps(G2, &(r1.start..r1.end).collect::<Vec<_>>()),
                    7.0,
                ),
                NoteSource::Tone,
            ),
            Part::new(
                kick_inst(),
                Pattern::gated(G2, &[0, 0], 6.0),
                NoteSource::Root,
            ),
        ],
        BPM,
        2,
        101,
    );

    // -- Hook: the four-bar I-V-vi-IV phrase, looped twice. Driving eighths,
    //    every bar an 8-note melody over the same 8-hit gate.
    const G8: &str = "x.x.x.x.x.x.x.x.";
    const G4: &str = "x...x...x...x...";
    const G8OFF: &str = ".x.x.x.x.x.x.x.x";
    let hook_bars = [
        mk_bar(BASS, CHORD, 1, LEAD, &[1, 3, 5, 6, 5, 3, 2, 1]),
        mk_bar(BASS, CHORD, 5, LEAD, &[5, 6, 7, 8, 7, 6, 5, 4]),
        mk_bar(BASS, CHORD, 6, LEAD, &[6, 8, 9, 8, 6, 5, 6, 3]),
        mk_bar(BASS, CHORD, 4, LEAD, &[4, 6, 8, 6, 4, 3, 2, 1]),
    ];
    let hook_range = hook_bars[0].1.clone();
    let hook_ti = place_at_steps(G8, &hook_range.collect::<Vec<_>>());
    let hook = piece(
        "A hook",
        hook_bars.iter().map(|(b, _)| b.clone()).collect(),
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(G8, &hook_ti, 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                comp_inst(),
                Pattern::gated(G4, &place_at_steps(G4, &cycle3(4)), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
            Part::new(
                hat_inst(),
                Pattern::gated(G8OFF, &[0; 8], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        8,
        102,
    );

    // -- Lift: a rising run over V, then a shimmering trill, building into
    //    the arrival. Both bars are 8-note melodies over the same gate.
    let (bl1, rl1) = mk_bar(BASS, CHORD, 5, LEAD, &[5, 6, 7, 8, 9, 10, 11, 12]);
    let (bl2, _) = mk_bar(BASS, CHORD, 5, LEAD, &[9, 8, 9, 8, 9, 10, 9, 8]);
    let lift_ti = place_at_steps(G8, &rl1.clone().collect::<Vec<_>>());
    let lift = piece(
        "A lift",
        vec![bl1, bl2],
        vec![
            Part::new(
                lead_inst().with_gain(0.34),
                Pattern::gated(G8, &lift_ti, 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G8, &[0; 8], 0.45),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                hat_inst(),
                Pattern::gated("xxxxxxxxxxxxxxxx", &[0; 16], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        2,
        103,
    );

    // -- Arrival, part 1: a descending IV flourish (4 hits) resolving
    //    toward the tonic, with the drums pushing through.
    let (ba1, ra1) = mk_bar(BASS, CHORD, 4, LEAD, &[8, 6, 5, 4]);
    let arrival_a = piece(
        "A arrival flourish",
        vec![ba1],
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(G4, &place_at_steps(G4, &ra1.collect::<Vec<_>>()), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        104,
    );

    // -- Arrival, part 2: the big held tonic chord — full triad plus the
    //    melody's own octave note, one bar, long release for a landing.
    let (ba2, ra2) = mk_bar(BASS, CHORD, 1, LEAD, &[8]);
    let arrival_b = piece(
        "A arrival hold",
        vec![ba2],
        vec![
            Part::new(
                Instrument::new(
                    Waveform::Square { duty: 0.5 },
                    Adsr::linear(0.02, 0.1, 0.8, 0.9),
                )
                .with_gain(0.30),
                Pattern::sustained(ra2.start),
                NoteSource::Tone,
            ),
            Part::new(comp_inst(), Pattern::sustained(0), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(1), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(2), NoteSource::Tone),
            Part::new(
                Instrument::new(Waveform::Triangle, Adsr::linear(0.02, 0.0, 1.0, 0.9))
                    .with_gain(0.42),
                Pattern::sustained(0),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated("x...............", &[0], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        105,
    );

    vec![intro, hook, lift, arrival_a, arrival_b]
}

// ---------------------------------------------------------------------
// Variant B — "leaning harder into fanfare energy": G major, 150 BPM.
// A layered square+saw "brass stack" lead, a bugle-call arpeggio intro,
// pure-major I-IV-V-I (no minor colour at all — the most unambiguously
// triumphant progression there is), a drum fill into the arrival.
// ---------------------------------------------------------------------

pub fn variant_b() -> Vec<Piece> {
    const BASS: i32 = 43; // G2
    const CHORD: i32 = 67; // G4
    const LEAD: i32 = 79; // G5
    const BPM: f64 = 150.0;

    // "Brass stack": the lead voice is two unison oscillators (a bright
    // narrow-duty square carrying the pitch, a saw doubling an octave down
    // for body), which is the cheap, standard way a subtractive synth fakes
    // a horn section — the closest this crate's palette gets to brass.
    let lead_hi = || {
        Instrument::new(
            Waveform::Square { duty: 0.22 },
            Adsr::linear(0.004, 0.03, 0.7, 0.06),
        )
        .with_vibrato(6.5, 0.014)
        .with_gain(0.28)
    };
    let lead_lo = || {
        Instrument::new(Waveform::Saw, Adsr::linear(0.004, 0.05, 0.6, 0.08))
            .with_filter(FilterSpec {
                base_cutoff_hz: 2200.0,
                q: 0.8,
                env_amount_hz: 0.0,
                env: Adsr::linear(0.0, 0.0, 1.0, 0.0),
            })
            .with_gain(0.18)
    };
    let comp_inst = || {
        Instrument::new(Waveform::Square { duty: 0.5 }, Adsr::pluck(0.003, 0.10))
            .with_gain(0.14)
    };
    let bass_inst = || {
        Instrument::new(
            Waveform::Square { duty: 0.5 },
            Adsr::linear(0.003, 0.0, 1.0, 0.02),
        )
        .with_gain(0.40)
    };
    let kick_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.09, 0.0, 0.02, 0.3))
            .with_filter(FilterSpec {
                base_cutoff_hz: 130.0,
                q: 2.4,
                env_amount_hz: 260.0,
                env: Adsr::shaped(0.0, 0.04, 0.0, 0.02, 0.3),
            })
            .with_gain(0.50)
    };
    let hat_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.03, 0.0, 0.008, 0.3))
            .with_gain(0.10)
    };
    let snare_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.07, 0.0, 0.03, 0.4))
            .with_filter(FilterSpec {
                base_cutoff_hz: 1800.0,
                q: 1.2,
                env_amount_hz: 0.0,
                env: Adsr::linear(0.0, 0.0, 1.0, 0.0),
            })
            .with_gain(0.30)
    };

    const G4: &str = "x...x...x...x...";
    const G8: &str = "x.x.x.x.x.x.x.x.";
    const G8OFF: &str = ".x.x.x.x.x.x.x.x";
    const G16: &str = "xxxxxxxxxxxxxxxx";

    // -- Intro, part 1: a rising tonic-triad bugle call (4 hits). Gated at
    //    3.2 steps (not the usual short pluck) — each note rings almost to
    //    the next one, so the very first ~3s of this variant is a bold,
    //    near-continuous call rather than four isolated stabs with dead air
    //    between them (measured: a 0.9-step gate here left ~480-640ms
    //    silence gaps inside a 1.68s bar — the opposite of "strong first 3
    //    seconds"). A soft continuous hat underneath fills what little gap
    //    remains.
    const CALL_LEN: f64 = 3.2;
    let (b1, r1) = mk_bar(BASS, CHORD, 1, LEAD, &[1, 3, 5, 8]);
    let ti1 = place_at_steps(G4, &r1.collect::<Vec<_>>());
    let intro_a = piece(
        "B intro call",
        vec![b1],
        vec![
            Part::new(
                lead_hi(),
                Pattern::gated(G4, &ti1, CALL_LEN),
                NoteSource::Tone,
            ),
            Part::new(
                lead_lo(),
                Pattern::gated(G4, &ti1, CALL_LEN),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
            Part::new(
                hat_inst(),
                Pattern::gated(G8OFF, &[0; 8], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        201,
    );

    // -- Intro, part 2: a descending V answer (4 hits), same treatment.
    let (b2, r2) = mk_bar(BASS, CHORD, 5, LEAD, &[8, 7, 6, 5]);
    let ti2 = place_at_steps(G4, &r2.collect::<Vec<_>>());
    let intro_b = piece(
        "B intro answer",
        vec![b2],
        vec![
            Part::new(
                lead_hi(),
                Pattern::gated(G4, &ti2, CALL_LEN),
                NoteSource::Tone,
            ),
            Part::new(
                lead_lo(),
                Pattern::gated(G4, &ti2, CALL_LEN),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
            Part::new(
                hat_inst(),
                Pattern::gated(G8OFF, &[0; 8], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        202,
    );

    // -- Hook: I-IV-V-I, looped three times (faster tempo needs more bars
    //    to fill the same wall-clock length). The lead reuses the intro's
    //    arpeggio motif at bar 1 for cohesion, then develops it.
    let hook_bars = [
        mk_bar(BASS, CHORD, 1, LEAD, &[1, 3, 5, 8, 5, 3, 2, 1]),
        mk_bar(BASS, CHORD, 4, LEAD, &[4, 6, 8, 9, 8, 6, 5, 4]),
        mk_bar(BASS, CHORD, 5, LEAD, &[5, 7, 9, 10, 9, 7, 6, 5]),
        mk_bar(BASS, CHORD, 1, LEAD, &[1, 3, 5, 8, 6, 5, 3, 1]),
    ];
    let hook_ti = place_at_steps(G8, &hook_bars[0].1.clone().collect::<Vec<_>>());
    let hook = piece(
        "B hook",
        hook_bars.iter().map(|(b, _)| b.clone()).collect(),
        vec![
            Part::new(
                lead_hi(),
                Pattern::gated(G8, &hook_ti, 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                lead_lo(),
                Pattern::gated(G8, &hook_ti, 0.9),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(
                comp_inst(),
                Pattern::gated(G4, &place_at_steps(G4, &cycle3(4)), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated("x.x.x...x.x.x...", &[0; 8], 0.9),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
            Part::new(
                hat_inst(),
                Pattern::gated(G8OFF, &[0; 8], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        12,
        203,
    );

    // -- Lift, part 1: a dense rising sixteenth-note run over V.
    let (bl1, rl1) = mk_bar(
        BASS,
        CHORD,
        5,
        LEAD,
        &[5, 6, 7, 8, 9, 10, 9, 8, 9, 10, 11, 12, 11, 10, 9, 8],
    );
    let lift_a = piece(
        "B lift run",
        vec![bl1],
        vec![
            Part::new(
                lead_hi().with_gain(0.32),
                Pattern::gated(G16, &place_at_steps(G16, &rl1.collect::<Vec<_>>()), 0.45),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G8, &[0; 8], 0.45),
                NoteSource::Root,
            )
            .transposed(-12),
        ],
        BPM,
        1,
        204,
    );

    // -- Lift, part 2: a suspended trill over V, then a snare fill in.
    let (bl2, rl2) = mk_bar(BASS, CHORD, 5, LEAD, &[9, 11, 9, 11, 10, 12, 10, 12]);
    let lift_b = piece(
        "B lift trill",
        vec![bl2],
        vec![
            Part::new(
                lead_hi().with_gain(0.30),
                Pattern::gated(G8, &place_at_steps(G8, &rl2.collect::<Vec<_>>()), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G8, &[0; 8], 0.45),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                snare_inst(),
                Pattern::gated("...........x.xxx", &[0; 16], 0.5),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        205,
    );

    // -- Arrival, part 1: a IV-then-V rising flourish (4 hits).
    let (ba1, ra1) = mk_bar(BASS, CHORD, 4, LEAD, &[9, 8, 6, 5]);
    let arrival_a = piece(
        "B arrival flourish",
        vec![ba1],
        vec![
            Part::new(
                lead_hi(),
                Pattern::gated(
                    G4,
                    &place_at_steps(G4, &ra1.clone().collect::<Vec<_>>()),
                    0.9,
                ),
                NoteSource::Tone,
            ),
            Part::new(
                lead_lo(),
                Pattern::gated(G4, &place_at_steps(G4, &ra1.collect::<Vec<_>>()), 0.9),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        206,
    );

    // -- Arrival, part 2: the big held tonic chord, full brass stack, long
    //    release.
    let (ba2, ra2) = mk_bar(BASS, CHORD, 1, LEAD, &[8]);
    let arrival_b = piece(
        "B arrival hold",
        vec![ba2],
        vec![
            Part::new(
                Instrument::new(
                    Waveform::Square { duty: 0.4 },
                    Adsr::linear(0.015, 0.1, 0.75, 0.9),
                )
                .with_gain(0.28),
                Pattern::sustained(ra2.start),
                NoteSource::Tone,
            ),
            Part::new(
                Instrument::new(Waveform::Saw, Adsr::linear(0.015, 0.1, 0.6, 0.9))
                    .with_filter(FilterSpec {
                        base_cutoff_hz: 2600.0,
                        q: 0.8,
                        env_amount_hz: 0.0,
                        env: Adsr::linear(0.0, 0.0, 1.0, 0.0),
                    })
                    .with_gain(0.18),
                Pattern::sustained(ra2.start),
                NoteSource::Tone,
            )
            .transposed(-12),
            Part::new(comp_inst(), Pattern::sustained(0), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(1), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(2), NoteSource::Tone),
            Part::new(
                Instrument::new(
                    Waveform::Square { duty: 0.5 },
                    Adsr::linear(0.015, 0.0, 1.0, 0.9),
                )
                .with_gain(0.40),
                Pattern::sustained(0),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated("x.......x.......", &[0, 0], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        207,
    );

    vec![intro_a, intro_b, hook, lift_a, lift_b, arrival_a, arrival_b]
}

// ---------------------------------------------------------------------
// Variant C — DMG-hardware-authentic: C major, 150 BPM. Deliberately mapped
// onto the real Game Boy's four channels (two pulse, one "wave"/triangle,
// one noise) — no filter anywhere (the DMG has none), no vibrato (a plain
// duty-cycle pulse has none), narrower duty cycles throughout. Thinner and
// more note-dense than A/B on purpose: real title-screen chiptune themes
// are busy because a bare pulse wave has so little else to say with.
// I-vi-ii-V — a classic "video game" progression with a bit more motion
// than a plain I-IV-V-I, still entirely generic.
// ---------------------------------------------------------------------

pub fn variant_c() -> Vec<Piece> {
    const BASS: i32 = 48; // C3
    const CHORD: i32 = 60; // C4
    const LEAD: i32 = 72; // C5
    const BPM: f64 = 150.0;

    // Channel 1: pulse lead. Channel 2: pulse harmony (comping). Channel 3:
    // "wave" — a triangle bass, standing in for the DMG's programmable-wave
    // channel, which real games very often use exactly this way. Channel 4:
    // noise, unfiltered — a real DMG has no filter stage at all.
    let lead_inst = || {
        Instrument::new(Waveform::Square { duty: 0.125 }, Adsr::pluck(0.001, 0.10))
            .with_gain(0.30)
    };
    let comp_inst = || {
        Instrument::new(Waveform::Square { duty: 0.5 }, Adsr::pluck(0.001, 0.09))
            .with_gain(0.16)
    };
    let bass_inst = || {
        Instrument::new(Waveform::Triangle, Adsr::linear(0.001, 0.0, 1.0, 0.02))
            .with_gain(0.38)
    };
    let kick_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.07, 0.0, 0.015, 0.3))
            .with_gain(0.44)
    };
    let hat_inst = || {
        Instrument::new(Waveform::Noise, Adsr::shaped(0.0, 0.02, 0.0, 0.006, 0.3))
            .with_gain(0.10)
    };

    const G4: &str = "x...x...x...x...";
    const G8: &str = "x.x.x.x.x.x.x.x.";
    const G8OFF: &str = ".x.x.x.x.x.x.x.x";
    const G16: &str = "xxxxxxxxxxxxxxxx";

    // -- Intro, part 1: a fast two-octave diatonic run — the classic
    //    chiptune "boot chime" gesture.
    let (b1, r1) = mk_bar(
        BASS,
        CHORD,
        1,
        LEAD,
        &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    );
    let intro_a = piece(
        "C intro run",
        vec![b1],
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(G16, &place_at_steps(G16, &r1.collect::<Vec<_>>()), 0.6),
                NoteSource::Tone,
            ),
            Part::new(
                kick_inst(),
                Pattern::gated("x...............", &[0], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        301,
    );

    // -- Intro, part 2: a sparser descending answer.
    let (b2, r2) = mk_bar(BASS, CHORD, 5, LEAD, &[8, 7, 6, 5, 4, 3, 2, 1]);
    let intro_b = piece(
        "C intro answer",
        vec![b2],
        vec![Part::new(
            lead_inst(),
            Pattern::gated(G8, &place_at_steps(G8, &r2.collect::<Vec<_>>()), 0.9),
            NoteSource::Tone,
        )],
        BPM,
        1,
        302,
    );

    // -- Hook: I-vi-ii-V, looped three times. Dense sixteenth arpeggios in
    //    the lead, a slower comping line, the wave-channel bass on the beat.
    let hook_bars = [
        mk_bar(
            BASS,
            CHORD,
            1,
            LEAD,
            &[1, 3, 5, 8, 5, 3, 1, 3, 5, 8, 9, 8, 5, 3, 2, 1],
        ),
        mk_bar(
            BASS,
            CHORD,
            6,
            LEAD,
            &[6, 8, 10, 6, 8, 10, 13, 10, 8, 6, 8, 10, 8, 6, 5, 6],
        ),
        mk_bar(
            BASS,
            CHORD,
            2,
            LEAD,
            &[2, 4, 6, 9, 6, 4, 2, 4, 6, 9, 11, 9, 6, 4, 3, 2],
        ),
        mk_bar(
            BASS,
            CHORD,
            5,
            LEAD,
            &[5, 7, 9, 12, 9, 7, 5, 7, 9, 12, 9, 7, 5, 4, 3, 1],
        ),
    ];
    let hook_ti = place_at_steps(G16, &hook_bars[0].1.clone().collect::<Vec<_>>());
    let hook = piece(
        "C hook",
        hook_bars.iter().map(|(b, _)| b.clone()).collect(),
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(G16, &hook_ti, 0.6),
                NoteSource::Tone,
            ),
            Part::new(
                comp_inst(),
                Pattern::gated(G4, &place_at_steps(G4, &cycle3(4)), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
            Part::new(
                hat_inst(),
                Pattern::gated(G8OFF, &[0; 8], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        12,
        303,
    );

    // -- Lift: a rising tremolo ("charge-up"), then a fast ascending run —
    //    both bars are 16-note melodies over the same dense gate.
    let (bl1, rl1) = mk_bar(
        BASS,
        CHORD,
        5,
        LEAD,
        &[8, 10, 8, 10, 8, 10, 8, 10, 9, 11, 9, 11, 9, 11, 9, 11],
    );
    let (bl2, _) = mk_bar(
        BASS,
        CHORD,
        5,
        LEAD,
        &[
            10, 11, 12, 13, 14, 15, 16, 17, 16, 15, 14, 13, 12, 11, 10, 9,
        ],
    );
    let lift_ti = place_at_steps(G16, &rl1.collect::<Vec<_>>());
    let lift = piece(
        "C lift",
        vec![bl1, bl2],
        vec![
            Part::new(
                lead_inst().with_gain(0.32),
                Pattern::gated(G16, &lift_ti, 0.45),
                NoteSource::Tone,
            ),
            Part::new(
                bass_inst(),
                Pattern::gated(G8, &[0; 8], 0.45),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                hat_inst(),
                Pattern::gated(G16, &[0; 16], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        2,
        304,
    );

    // -- Arrival, part 1: a descending V flourish (4 hits) with a closing
    //    accent.
    let (ba1, ra1) = mk_bar(BASS, CHORD, 5, LEAD, &[8, 6, 5, 3]);
    let arrival_a = piece(
        "C arrival flourish",
        vec![ba1],
        vec![
            Part::new(
                lead_inst(),
                Pattern::gated(G4, &place_at_steps(G4, &ra1.collect::<Vec<_>>()), 0.9),
                NoteSource::Tone,
            ),
            Part::new(
                kick_inst(),
                Pattern::gated(G4, &[0; 4], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        305,
    );

    // -- Arrival, part 2: the held tonic octave over the full triad.
    let (ba2, ra2) = mk_bar(BASS, CHORD, 1, LEAD, &[8]);
    let arrival_b = piece(
        "C arrival hold",
        vec![ba2],
        vec![
            Part::new(
                Instrument::new(
                    Waveform::Square { duty: 0.5 },
                    Adsr::linear(0.008, 0.08, 0.75, 0.7),
                )
                .with_gain(0.30),
                Pattern::sustained(ra2.start),
                NoteSource::Tone,
            ),
            Part::new(comp_inst(), Pattern::sustained(0), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(1), NoteSource::Tone),
            Part::new(comp_inst(), Pattern::sustained(2), NoteSource::Tone),
            Part::new(
                Instrument::new(Waveform::Triangle, Adsr::linear(0.008, 0.0, 1.0, 0.7))
                    .with_gain(0.40),
                Pattern::sustained(0),
                NoteSource::Root,
            )
            .transposed(-12),
            Part::new(
                kick_inst(),
                Pattern::gated("x...............", &[0], 0.9),
                NoteSource::Root,
            ),
        ],
        BPM,
        1,
        306,
    );

    vec![intro_a, intro_b, hook, lift, arrival_a, arrival_b]
}

type VariantBuilder = fn() -> Vec<Piece>;

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let out_dir = argv
        .first()
        .cloned()
        .unwrap_or_else(|| "docs/opening-theme".to_string());
    std::fs::create_dir_all(&out_dir).expect("create output dir");
    let sample_rate = 48_000u32;

    let variants: [(&str, VariantBuilder); 3] = [
        ("variant-a-generic", variant_a),
        ("variant-b-fanfare", variant_b),
        ("variant-c-dmg-authentic", variant_c),
    ];

    for (name, build) in variants {
        let pieces = build();
        // Print exact section boundaries (seconds) before rendering — this
        // is ground truth for slicing the WAV in analysis, rather than a
        // hand-computed estimate.
        let mut t = 0.0;
        print!("{name} sections:");
        for p in &pieces {
            let d = p.duration_secs();
            print!(" [{t:.3}-{:.3}]", t + d);
            t += d;
        }
        println!();
        let samples = render_variant(&pieces, sample_rate as f64);
        let path = std::path::PathBuf::from(&out_dir).join(format!("{name}.wav"));
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
}

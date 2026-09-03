# Opening theme — three sketches for the short

Three original ~30s opening-theme sketches, built with
[`loom-synth`](../../crates/loom-synth/README.md) for a TerminalGB short. The
captain's ask: *"the feeling of the Pokémon opening, with a melody that's
genuinely ours."* This page is the honesty account the task itself asked
for — exactly what each variant borrowed from the era versus what was
written fresh — plus how to reproduce or re-render them.

**Ship: `variant-a-generic.mp3`, `variant-b-fanfare.mp3`,
`variant-c-dmg-authentic.mp3` (the WAVs sit beside them — kept in the repo
so the exact audio under review is committed, not regenerated at review
time). Pick one; all three are ready to cut against.**

## The line this whole exercise sits on

This goes on YouTube publicly. YouTube's Content ID matches the Pokémon
theme instantly, and a recognisable reproduction of that melody — even
transposed, even rhythmically altered — gets the video flagged or muted. So
the rule followed throughout: **no phrase here is a transcription,
transposition, or rhythmic rearrangement of any published Pokémon melody**,
game or anime.

What *was* deliberately taken, because none of it is anyone's property:

- **Tempo and feel** — a driving, marching, mid-tempo pace (112-150 BPM
  across the three).
- **Key and chord progression** — I-V-vi-IV, I-IV-V-I, I-vi-ii-V. None of
  these are copyrightable; they're the common property of the entire
  triumphant-fanfare and video-game-music genres (I-V-vi-IV alone is often
  called "the four-chord song," used across thousands of unrelated pieces).
- **Instrumentation and timbre** — bright pulse/square leads, a pulse or
  triangle bass, noise-channel drums; Variant C goes further and maps
  literally onto the real Game Boy's four sound channels (two pulse, one
  triangle standing in for the "wave" channel, one unfiltered noise).
- **Structure** — a short fanfare-like opening call, a driving main hook, a
  rising "lift," a resolving arrival on a held chord. This is the brief's own
  requested shape, and it's also just how fanfares are built generally.
- **The era** — late-80s/early-90s handheld chiptune convention: narrow duty
  cycles, PolyBLEP-corrected square/saw waves (see `loom-synth`'s own docs
  for the citation), short percussive envelopes, no reverb/production sheen
  beyond what `loom-synth`'s filter can add.

## How the melodies were actually written — and why that method matters here

Every melody was composed **from scratch in scale-degree terms**, not by
recalling an existing tune and altering it. Concretely:
[`crates/loom-synth/examples/opening_theme.rs`](../../crates/loom-synth/examples/opening_theme.rs)
picks a scale-degree sequence per bar (e.g. `[1, 3, 5, 6, 5, 3, 2, 1]` — up a
third, up a step, back down, resolve to the tonic) using ordinary
tonal-composition devices — arpeggiated calls, stepwise answers, sequences
that shift a shape up with the harmony, a rising scalar "lift" — and a small
helper (`deg`) turns each degree into a MIDI pitch relative to the current
key. Two consequences worth stating plainly:

1. **Every note is diatonic by construction.** `deg`/`triad` can only ever
   produce a note that belongs to the piece's own major scale, so there is
   no way to accidentally write an out-of-key (wrong-sounding) note — the
   only thing composition *skill* affects here is the elegance of the
   contour, never correctness.
2. **The specific interval sequences are original.** They were chosen for
   musical shape (a rising call, a falling answer, a sequence up a fourth
   following the harmony) without reference to — or attempt to reproduce —
   the actual pitch/rhythm content of any Pokémon melody. No transcription
   of the real tune was consulted or worked from at any point.

## Where I'd ask for a second check before publishing

I have no way to run an actual audio similarity comparison against the real
Pokémon themes in this environment (no reference audio, no network), so the
strongest claim I can honestly make is a *methodological* one: I never
worked from — or attempted to recall — the real melody's note sequence, so
there's no transformation-of-an-existing-tune risk here. Two things worth a
human ear-check before this goes out, in the interest of catching it now
rather than after publish, exactly as the brief asked:

- **The opening rising-triad arpeggio device** (used to open Variant B's
  intro, and echoed at the top of Variant B's hook): `1-3-5-8`, a tonic
  triad climbed straight up. This is an extremely common fanfare/bugle-call
  device — it predates video games by centuries (military calls, countless
  classical fanfares) — and it is very likely *also* the shape of Pokémon's
  own game-title chime, since a rising tonic arpeggio is close to the only
  way to open a "something is starting!" fanfare. Sharing a **device** this
  generic is not the same as sharing a **melody** (the specific pitches
  continuing from it diverge immediately, and the rhythm/register/harmony
  around it are all original), but it's the one place in this set where the
  contour-shape overlap with the *type* of gesture a game fanfare opens on
  is closest, so it's named here rather than left for someone else to spot.
- **Variant C's intro** is a fast two-octave ascending diatonic scale run
  (`1` through `16` in scale-degree terms) — the classic chiptune
  "boot chime" gesture. Scale runs are about as generic a device as exists
  in music, but it's an unmistakably "video game start-up" sound, so it's
  flagged for the same reason as above: genre-generic, not melody-specific,
  but worth an ear on it.

Neither is a melody match — a scale run and a triad arpeggio have no
distinctive pitch *sequence* to match against, they're closer to a drum
fill than a tune — but "I structurally cannot have copied a melody I never
worked from" is a different claim from "a human confirmed it doesn't ring a
bell," and the second is cheap to get and worth getting before this is
public.

## The three variants

| | Variant A — generic | Variant B — fanfare | Variant C — DMG-authentic |
|---|---|---|---|
| Key | D major | G major | C major |
| Tempo | 112 BPM | 150 BPM | 150 BPM |
| Progression | I–V–vi–IV | I–IV–V–I (all major, no minor colour at all) | I–vi–ii–V |
| Lead | one square (28% duty), light vibrato | two-oscillator "brass stack" (narrow square + saw an octave down) | pure pulse, 12.5% duty, no vibrato |
| Filter used? | kick drum only | kick drum + saw's own top-end roll-off | **none anywhere** — a real DMG has no filter stage |
| Character | bright pop-chiptune march, moderate energy | the biggest, most "triumphant ceremony" of the three | thin, dense, arpeggio-heavy — literally mapped onto the Game Boy's 4 channels (2 pulse, triangle "wave," noise) |
| Duration | 31.2s | 30.2s | 30.0s |

All three share the same four-part shape the brief asked for: a short
fanfare call, a driving hook (looped 2-3 times to fill the runtime), a
rising "lift," and an arrival on a long-held final chord — built as
several short, independently-instrumented pieces concatenated end to end
(see the module docs in `opening_theme.rs` for why: a `loom-synth` `Part`'s
rhythmic pattern is one template reused every bar of a piece, so wherever
the arrangement genuinely changes rhythm from one bar to the next — the
intro's call answered by a different-shaped response, the arrival's
flourish giving way to a long held chord — that has to be a new piece, not
a longer one).

## Checked before shipping

- **No clipping**: peak `0.776` on all three (the engine's own soft-limiter
  targets `0.9` headroom; measured well under it).
- **Deterministic**: re-rendering produces byte-identical WAVs (checked).
- **No long silence gaps**: the first version had this exact problem in
  Variant B's intro — a 4-hit punctuating call with a short (0.9-step) gate
  left 480-640ms of true silence between hits inside the first 3.36s,
  measured with a 10ms-window RMS sweep. Fixed by lengthening the gate to
  3.2 steps (each note now rings almost to the next) and adding a soft
  continuous hi-hat underneath; re-measured afterward at 20-180ms, and the
  first-3-seconds RMS rose from ~0.20 to 0.415 — now the loudest opening of
  the three. Full before/after numbers are in the commit; the technique
  (10ms-window RMS to find dead air) is worth reusing for anything built
  this way in future — there's no audio playback in this environment, so
  numeric envelope/spectral analysis is the check available.
- **Brightness for phone speakers**: spectral centroid sits ~4-6.5kHz
  through the busy sections on all three — mid-heavy and bright, not
  bass-reliant (the brief's own "must sound good on a phone speaker"
  requirement). No content of consequence below ~150Hz on anything but the
  kick drum's fundamental.
- **Register sanity**: nothing goes below a low bass note (~G2, ~98Hz) or
  above the high end of Variant B's lift (~G6, ~1568Hz) — bright without
  tipping into shrill.

## Reproducing / regenerating

```bash
cargo run -p loom-synth --release --example opening_theme -- docs/opening-theme
```

Renders all three WAVs (deterministic — re-running reproduces them
byte-for-byte) with their section boundaries printed to stdout. Then, for
the MP3s:

```bash
cd docs/opening-theme
for f in variant-a-generic variant-b-fanfare variant-c-dmg-authentic; do
  ffmpeg -y -i "$f.wav" -codec:a libmp3lame -qscale:a 2 "$f.mp3"
done
```

# A DMG mimic of the opening of Kubbi's "Ember"

The captain's ask: *"my tune. Can we mimic the opening in Gameboy music
style."* The reference is Kubbi's "Ember" (2015). This page is the honesty
account the brief itself asked for, in the order the brief asked for it:
the licence finding first, then the measured analysis, then what was
actually taken versus written fresh.

**Ship: `ember-dmg-opening.mp3`** (the WAV sits beside it — the exact audio
under review is committed, not regenerated at review time). 31.25s.

## 1. The licence — established, not assumed

**Kubbi's "Ember" is licensed CC BY-SA 4.0 (Creative Commons
Attribution-ShareAlike 4.0 International).**

Source: [kubbimusic.com/track/ember](https://kubbimusic.com/track/ember) and
[kubbimusic.com/album/ember](https://kubbimusic.com/album/ember) — Kubbi's
own Bandcamp storefront on his own custom domain (redirects from
`kubbi.bandcamp.com`, and is the "Kubbi Home Base" his own YouTube
description and social profiles all link back to). Verified directly against
the raw page HTML, not a summary of it:

```html
<h3 class="license-label">license</h3>
<div id="license" class="info license">
  <a class="cc-icons" href="https://creativecommons.org/licenses/by-sa/4.0/">
    <span class="attribution"></span><span class="share-alike"></span>
  </a>
  <a href="https://creativecommons.org/licenses/by-sa/4.0/">some rights reserved</a>
</div>
```

and the page's own embedded JSON-LD: `"license_name":"attribution_share_alike"`.
Both the individual track page and the album page carry the identical
license block. Released 2015-02-05, track 2 of the album *Ember*.

**A reference audio file was NOT present at the path the brief named**
(`data/reference/kubbi-ember.mp3`) — it doesn't exist anywhere on this
machine. Rather than compose from an unverified impression, I located the
track's own official upload — Kubbi's own YouTube channel
([`youtube.com/watch?v=uGcjAKZ5TUk`](https://www.youtube.com/watch?v=uGcjAKZ5TUk),
uploader `@KUBBIkthxbai`, channel `UCNCVus37D-NmMC3_fFJlzRQ`, description
linking `kubbimusic.com`) — confirmed as the same track by duration: **295.0s
(4:55.0)**, matching the brief's stated "4m55s" file length exactly.
Downloaded audio-only, analysed it with real DSP (below), and did not keep
or commit the reference audio anywhere in this repo — only the analysis
numbers.

### What CC BY-SA 4.0 actually requires, stated plainly

Two obligations, not one:

1. **Attribution (BY)**: credit the creator, link the license, note if
   changes were made.
2. **ShareAlike (SA)**: if you distribute an *adaptation* — a derivative
   work that carries forward the original's own protected expression (its
   specific melody, its specific arrangement) — you must license **your**
   adaptation under CC BY-SA 4.0 too. You can't build on a ShareAlike work
   and then lock the result down.

That second clause is the one worth being precise about, because it's easy
to read "CC BY, we can sit close" (the brief's own phrasing for this branch)
and miss it. **This piece does not transcribe or arrange Kubbi's melody** —
partly by choice (see §3), and partly because there isn't a strong
independent melody in the reference's own opening 30 seconds to transcribe
(see §2: it's a drone, not a tune, for almost the whole window). What's
reproduced here is tempo, key centre, and structural shape — none of which
is the kind of "expression" copyright (and so ShareAlike) protects; a tempo
and a key are facts, not authorship. On that basis this piece is an
independent work informed by, not a legal adaptation of, "Ember," and
ShareAlike's relicensing obligation is very likely not triggered. That's a
reasoned read of the license terms, not a legal opinion — if this is ever
monetised or the captain wants certainty, that's worth a real check.

**Attribution is done regardless**, both because it's simply correct
(Ember was the explicit creative reference) and because it costs nothing.
For a ~30-35s Short, the standard CC BY-SA "TASL" line, short enough for an
on-screen card or a description line:

> Inspired by "Ember" by Kubbi — kubbi.bandcamp.com — CC BY-SA 4.0

Mechanism: a text card for the last ~3 seconds of the video (the source's
own [`video::card`](../../src/video.rs) hand-rolled font module, already
used elsewhere in this project for exactly this kind of overlay, is the
obvious tool if the Short is assembled in this repo's own pipeline), *and*
the same line in the video description — description text is always
satisfiable regardless of how the video itself is cut, so it's the fallback
that can't fail.

## 2. Measured analysis of the reference's opening

Everything below is measured from the actual downloaded audio (librosa +
numpy/scipy — a proper DSP toolchain, not an impression), not recalled or
guessed. Reproduce with `tools/kubbi-ember-analysis.py` (§5).

- **Tempo: ~99.4 BPM**, rounded to a clean **100** for the piece below.
  Confirmed three independent ways that agree to within noise: librosa's
  beat tracker (99.38 BPM, stable across `start_bpm` biases of 90 and 120 —
  it only flips to the octave-double 198.8 above `start_bpm=150`); and the
  actual kick-range onset sequence itself, which alternates ~0.163s/~0.139s
  (a syncopated pair, summing to ~0.302s — a swung eighth-note feel, not a
  straight one). Doubled (one quarter-note beat, since two of these
  alternating onsets make one beat), that's 0.604s -> 99.4 BPM, matching
  the beat tracker's figure independently.
- **Key: C natural minor**, with the *entire opening built over a static
  Ab (♭VI) pedal* rather than a moving chord progression. This is the
  single most load-bearing finding: isolating the sub-260Hz band and taking
  its dominant pitch class gives **Ab at 0.97 correlation in 0-18s and 0.83
  in 18-36s** (a near-total drone, not "mostly Ab with some movement") —
  the bass genuinely does not move for the first 30-plus seconds. Aggregate
  chroma key-profile correlation independently agrees: G#/Ab major and C
  minor are the two closest matches throughout, which is exactly what a
  ♭VI-drone-in-a-minor-key reads as (an Ab **major** triad — Ab-C-Eb — is
  the diatonically-correct chord built on C natural minor's sixth degree).
  A beat-synced chord-template sweep of the first 30 seconds confirms it
  directly: G#(Ab) major is the best-scoring chord on essentially every
  half-bar window from t=1.2s to t=30s, with no change of chord anywhere
  in between.
- **Chord movement in the opening 30s: effectively none.** The harmony
  only starts to move at ~31s, where the chord-template sweep flips from
  Ab to C minor — right at, or just past, the edge of the 30-second window
  the brief asked about.
- **Arrangement shape over 0-30s**, from RMS loudness, spectral centroid
  and onset-strength measured in 2-second windows:
  - **0-18s**: a slow, smooth, continuous loudness build (RMS climbs
    0.077 → 0.128, no jumps), not a stepped one. Spectral centroid *dips*
    around 4-8s (1700Hz → ~1200Hz) — something warm/low entering, thinning
    the average — then climbs back past 2000Hz by 18-20s as brighter
    texture layers in.
  - **18-30s**: loudness plateaus (~0.12-0.13 RMS), centroid stays bright
    and elevated (~2100-2250Hz) — a sustained hold, not further layering.
    Tellingly, **onset-strength actually falls** here (from ~2.9 down to
    ~1.3, onset-strength units) even as the mix holds loud: the added
    density is sustained/textural (pads, held tones), not more hits.
  - **The hook does not land within the first 30 seconds.** RMS stays flat
    through 30s and only jumps sharply — 0.14 → 0.26 — at 36-42s, right
    where the chord finally moves off the Ab drone. That 36-42s moment,
    just past the 30-second mark, is the real "energy arrival"; a second,
    even bigger jump (RMS to ~0.33-0.35) follows a breakdown-and-rebuild
    around 76-88s. Onset-count-per-5s-bucket over the first 30s is
    essentially flat (33, 34, 40, 34, 31, 29) — there's no drum kit
    entering in this window, just the same steady pulsing texture the
    whole time.

**Net: "mimicking Ember's opening 30 seconds" means mimicking a patient,
static-harmony, texturally-building drone that has *not yet* reached its
payoff by the 30-second mark** — not a rising melodic hook. That's an
unglamorous but accurate finding, and it's what the piece below actually
follows.

## 3. What was taken, what was written fresh, and the DMG arrangement

**Taken** (measured facts, none of them copyrightable): the tempo (100
BPM), the key centre and its specific opening gesture (a held ♭VI/Ab drone
resolving to the tonic only at the very end), and the arrangement's shape
(slow layered build, receding rhythmic density mid-build, one harmonic/
energy turn landing at the close — compressed to fit a ~31-second piece
instead of the reference's own ~36-42 seconds, since there's no room to
also hold a further six-plus seconds of static drone before the payoff).

**Written fresh**: every actual note. There is no melody transcribed from
Kubbi's track — not out of caution alone, but because the measured reality
is that the reference's own opening barely has one (§2): it's harmony and
texture, not a tune, for almost the whole 30 seconds. The piece below was
composed the same way `opening_theme.rs` (the project's earlier
"mimic a feeling without copying a melody" precedent, against the Pokémon
theme) was: in scale-degree terms, against a hand-picked scale table
(`MINOR`, C natural minor — see the module docs in
[`ember_opening.rs`](../../crates/loom-synth/examples/ember_opening.rs) for
the exact mechanism, including the `place_at_steps` fix for
`Pattern::gated`'s absolute-step-position `tone_index` pitfall the previous
crew hit).

**The four real DMG channels, and why each got what it got** (the brief's
own "arrangement problem," answered directly rather than "it sounds
thinner"):

| Channel | Role | Why |
|---|---|---|
| Wave (triangle) | The Ab pedal, resolving to C only in the final bar | The reference's own bass sits on one pitch class for its whole opening (0.83-0.97 dominance, §2) — a genuine drone. One channel, one held note, is the honest hardware-native version of that, not a simplification of something busier. |
| Pulse 2 (50% duty) | A steady comping pulse, alternating the drone's root and fifth | Stands in for the ~0.30s pulsing texture measured from the reference's very first bar. Deliberately *thinned* (fewer hits/bar) through the "hold" section, mirroring the measured *drop* in onset-strength at 18-30s even as loudness holds — the reference gets louder there by sustaining, not by hitting more, so this part does the same. |
| Pulse 1 (duty widens → narrows) | The "brightening" layer | A real DMG pulse channel's only timbral control is duty cycle — narrower duty pushes energy into higher harmonics. Narrowing it over the piece is the hardware-native substitute for the reference's rising spectral centroid (measured ~1200Hz → ~2200Hz across roughly 4-20s), rather than reaching for a filter sweep the actual chip cannot do. |
| Noise | Near-silent until the very end, then one accented hit | The reference has no audible drum kit in its first 30 seconds either (onset count per 5s bucket is flat, not climbing). A few faint ticks late, then one clear hit timed to the bass's resolution — standing in for the loudness/bass jump measured just past the reference's own 30-second mark. |

**Dropped, because a DMG has four channels total and the reference is "a
full modern chiptune production with far more than four voices"**: any
reverb or wash (no DMG channel has one at all); whatever additional
simultaneous pad/synth layers the real mix carries beyond one drone and one
comping pulse (collapsed into those two — the piece never tries to
represent more than four simultaneous elements, because the hardware
can't); a full percussion kit (there isn't a clear one to preserve in this
window regardless, per the onset-count measurement above).

## 4. Checked before shipping

Same discipline `opening_theme.rs` established — there's no audio playback
available in this environment, so every check below is numeric, run
against the actual rendered WAV (both channels of the file are
byte-identical mono-duplicated-to-stereo, confirmed before analysing).

- **No clipping**: peak `0.7755` — inside the engine's `0.9` soft-limiter
  headroom, and matches the peak all three `opening_theme.rs` variants
  independently converged on (`0.776`), which is a good sign the limiter
  is behaving consistently rather than this piece sitting in unusually
  hot or cold territory.
- **No long silence gaps**: a 10ms-window RMS sweep across the whole
  31.25s render finds no silent run longer than 30ms anywhere (one section
  boundary briefly touches near-zero — a single window landing on a
  wave-channel zero-crossing at a piece concatenation seam, not an audible
  gap).
- **Deterministic**: `cargo test -p loom-synth --release --example
  ember_opening` — `rendering_is_deterministic` re-renders and diffs
  byte-for-byte.
- **Brightness**: spectral centroid runs roughly 1900-4100Hz through the
  busy middle section (2-27s), climbing as pulse 1's duty narrows — in the
  same "bright, present on a phone speaker" territory the earlier variants
  targeted, and rising through the piece the way the brightening
  arrangement decision intends.
- **Register sanity**: lowest note 65.4Hz (C2, the final resolved bass),
  highest 1244.5Hz (Eb6, pulse 1's fifth in the "turn" section) — comparable
  order of magnitude to `opening_theme.rs`'s own G2-G6 register check, no
  extremes. (Verified by recomputing every note's MIDI number and frequency
  directly from the same scale/transpose logic the Rust source uses, not by
  hand — a first pass at this number had misattributed the highest note to
  the wrong channel.)
- **Section timing** (printed by the render itself, ground truth for
  slicing): `[0.000-2.750] [2.750-7.900] [7.900-13.050] [13.050-18.200]
  [18.200-25.750] [25.750-28.500] [28.500-31.250]` — pedal solo, pedal +
  pulse, brighten (warm), brighten (bright), hold, turn (walking bass),
  turn (arrival).

## 5. Reproducing / regenerating

```bash
# The composition (WAV):
cargo run -p loom-synth --release --example ember_opening -- docs/ember-cover

# The MP3:
cd docs/ember-cover
ffmpeg -y -i ember-dmg-opening.wav -codec:a libmp3lame -qscale:a 2 ember-dmg-opening.mp3

# The unit tests (determinism, headroom, tone-layout consistency, duration):
cargo test -p loom-synth --release --example ember_opening
```

```bash
# The §2 measured analysis, reproduced from scratch:
pip install --user librosa   # not a dependency of anything else in this repo
python3 tools/kubbi-ember-analysis.py
```

`tools/kubbi-ember-analysis.py` downloads Kubbi's own official YouTube
upload of "Ember" audio-only (`yt-dlp`, to a temp directory, deleted when
the script exits — never written anywhere inside this repo) and runs the
same `librosa` measurements §2's numbers were read off. It is deliberately
not wired into any test or build; it exists to make the licence/analysis
claims above independently checkable, not as ongoing project
infrastructure.

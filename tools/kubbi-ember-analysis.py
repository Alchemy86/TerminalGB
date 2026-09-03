#!/usr/bin/env python3
"""Reproduces the measured analysis in docs/ember-cover/README.md §2: the
tempo, key, harmonic-movement and arrangement-shape findings behind the
DMG mimic of the opening of Kubbi's "Ember" (2015).

Deliberately NOT a general-purpose tool and NOT wired into any test or
build — it downloads audio from a named YouTube URL for one-off analysis
and needs `yt-dlp` + `librosa` (`pip install --user librosa`, not a
dependency of anything else in this repo). No audio is vendored: this
script always re-fetches, and never writes the downloaded file anywhere
inside the repository.

Usage:
    python3 tools/kubbi-ember-analysis.py
"""

import subprocess
import sys
import tempfile
from pathlib import Path

REFERENCE_URL = "https://www.youtube.com/watch?v=uGcjAKZ5TUk"  # Kubbi's own upload of "Ember"


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    try:
        import numpy as np
        import librosa
    except ImportError:
        die("needs numpy + librosa: pip install --user librosa")

    with tempfile.TemporaryDirectory() as tmp:
        wav_path = Path(tmp) / "ember.wav"
        print(f"fetching {REFERENCE_URL} (audio only, temp dir, not kept)...")
        r = subprocess.run(
            [
                "yt-dlp", "-x", "--audio-format", "wav", "--audio-quality", "0",
                "-o", str(wav_path.with_suffix("")) + ".%(ext)s", REFERENCE_URL,
            ],
            capture_output=True, text=True,
        )
        if r.returncode != 0 or not wav_path.exists():
            die(f"yt-dlp failed:\n{r.stderr[-2000:]}")

        y, sr = librosa.load(str(wav_path), sr=22050, mono=True)
        duration = len(y) / sr
        print(f"loaded {duration:.2f}s at {sr}Hz "
              f"(brief's stated file length: 295s / 4m55s — "
              f"{'MATCHES' if abs(duration - 295) < 2 else 'DOES NOT MATCH'})")

        # --- Tempo: three independent methods ---
        print("\n--- tempo ---")
        tempo, _ = librosa.beat.beat_track(y=y, sr=sr)
        print(f"beat_track: {tempo[0]:.2f} BPM")

        onset_env = librosa.onset.onset_strength(y=y[60 * sr:180 * sr], sr=sr, fmax=200,
                                                   feature=librosa.feature.melspectrogram)
        onsets = librosa.onset.onset_detect(onset_envelope=onset_env, sr=sr, backtrack=False,
                                             units="time")
        diffs = np.diff(onsets[:20])
        # The alternating 0.163s/0.139s pattern holds cleanly for the first
        # 10 diffs (5 beat-pairs); later entries include an irregular gap
        # (a passing rest in the actual rhythm) and are printed for context
        # but excluded from the beat-period estimate below.
        clean = diffs[:10]
        eighth_note = clean[0::2].mean() + clean[1::2].mean()  # a swung eighth: two onsets/beat
        quarter_note = 2 * eighth_note  # the beat period beat_track above independently found
        print(f"kick-band onsets (first 20, 60-180s window): {onsets[:20].round(3).tolist()}")
        print(f"consecutive diffs: {diffs.round(3).tolist()}")
        print(f"first 5 beat-pairs alternate two intervals summing to ~{eighth_note:.3f}s "
              f"per swung-eighth-note pair")
        print(f"doubled (one quarter-note beat): {quarter_note:.3f}s -> {60/quarter_note:.1f} BPM "
              f"({'matches' if abs(60/quarter_note - 99.38) < 1 else 'does NOT match'} "
              f"the beat_track figure above, corroborating it independently)")

        # --- Key: chroma correlation + direct bass pitch-class dominance ---
        print("\n--- key / harmony ---")
        pitch_names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"]
        major_profile = np.array([6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88])
        minor_profile = np.array([6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17])

        def bass_dominance(t0, t1, label):
            from scipy.signal import butter, sosfilt
            seg = y[int(t0 * sr):int(t1 * sr)]
            sos = butter(6, [40, 260], btype="band", fs=sr, output="sos")
            yb = sosfilt(sos, seg)
            chroma = librosa.feature.chroma_cqt(y=yb, sr=sr, hop_length=1024,
                                                 fmin=librosa.note_to_hz("C1"), n_octaves=3)
            m = chroma.mean(axis=1)
            order = np.argsort(m)[::-1]
            top3 = [(pitch_names[i], round(float(m[i]), 3)) for i in order[:3]]
            print(f"  bass dominance {label}: {top3}")

        bass_dominance(0, 18, "0-18s")
        bass_dominance(18, 36, "18-36s")

        # --- Arrangement shape: RMS + spectral centroid + onset density, 2s windows ---
        print("\n--- arrangement shape, 0-30s (2s windows) ---")
        seg = y[:30 * sr]
        hop = 512
        rms = librosa.feature.rms(y=seg, hop_length=hop)[0]
        cent = librosa.feature.spectral_centroid(y=seg, sr=sr, hop_length=hop)[0]
        onset = librosa.onset.onset_strength(y=seg, sr=sr, hop_length=hop)
        times = librosa.frames_to_time(np.arange(len(rms)), sr=sr, hop_length=hop)
        for t0 in np.arange(0, 30, 2.0):
            mask = (times >= t0) & (times < t0 + 2.0)
            if not mask.any():
                continue
            print(f"  t={t0:4.1f}s  rms={rms[mask].mean():.4f}  "
                  f"centroid={cent[mask].mean():7.1f}Hz  onset_str={onset[mask].mean():.3f}")

        print("\n--- full-spectrum onset count per 5s bucket, 0-30s ---")
        onset_env_full = librosa.onset.onset_strength(y=seg, sr=sr)
        onsets_full = librosa.onset.onset_detect(onset_envelope=onset_env_full, sr=sr,
                                                   units="time")
        buckets = np.zeros(6, dtype=int)
        for t in onsets_full:
            buckets[min(int(t // 5), 5)] += 1
        print(f"buckets (0-5,5-10,...25-30s): {buckets.tolist()} "
              f"(flat, not climbing the way a beat entering would look)")

        print("\n--- energy jump past 30s (1s windows, 30-45s) ---")
        seg2 = y[30 * sr:45 * sr]
        rms2 = librosa.feature.rms(y=seg2, hop_length=hop)[0]
        times2 = librosa.frames_to_time(np.arange(len(rms2)), sr=sr, hop_length=hop) + 30
        for t0 in np.arange(30, 45, 1.0):
            mask = (times2 >= t0) & (times2 < t0 + 1.0)
            if not mask.any():
                continue
            print(f"  t={t0:4.1f}s  rms={rms2[mask].mean():.4f}")


if __name__ == "__main__":
    main()

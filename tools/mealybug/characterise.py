#!/usr/bin/env python3
"""Characterise *how* each Mealybug row differs from its hardware reference.

A pixel count says a row is red; it does not say why. This reads the frames
`examples/mealybug` writes into `target/mealybug/` beside the author's reference
photographs and reports the **shape** of each difference, so rows that share a
signature can be grouped and worked as one bug rather than 62.

    python3 tools/mealybug/characterise.py [--dir <suite>] [--frames <dir>] [filter...]

Signatures reported per row:

* `shift dx,dy`  — the (dx, dy) in -8..8 that minimises the diff, and what the
  diff falls to there. A row that goes to near zero under a uniform offset is a
  phase error, not a content error.
* `rank`         — distinct shade counts on both sides. Ours having a different
  count from the reference re-ranks the whole frame, so the pixel figure is a
  rank artefact and not a distance (docs/mealybug.md §8).
* `lines`        — how many of the 144 scanlines carry any difference, and the
  first/last.
* `cols`         — first and last differing column, and whether the difference
  begins at the same column on every affected line (a "boundary is N pixels
  late" signature).
* `perline`      — for the affected lines, the run of first-differing columns.

Nothing here writes a baseline or judges pass/fail; it is a lens.
"""

import os
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image

W, H = 160, 144

REFS = [
    ("DMG", "DMG-blob", "dmg"),
    ("CGB-C", "CPU CGB C", "cgb-c"),
    ("CGB-D", "CPU CGB D", "cgb"),
]


def ranks(path):
    """Reduce an image to four shade indices by luminance rank.

    Mirrors `shade_diff`/`ranks` in examples/mealybug.rs exactly, including the
    even-bucket fallback for a reference with more than four luminances (the
    Color photographs of a boot-ROM-colourised frame carry 125).
    """
    img = Image.open(path).convert("RGB")
    a = np.asarray(img, dtype=np.uint32)
    lum = (a[..., 0] * 299 + a[..., 1] * 587 + a[..., 2] * 114) // 1000
    order = np.unique(lum)
    if len(order) > 4:
        lo, hi = int(order[0]), int(order[-1])
        span = max(hi - lo, 1) + 1
        return np.minimum((lum - lo) * 4 // span, 3).astype(np.int8), len(order)
    idx = np.searchsorted(order, lum)
    return idx.astype(np.int8), len(order)


def best_shift(ours, ref, span=8):
    """The (dx, dy) offset of `ours` that minimises the difference.

    Only the overlapping region is compared and the score is scaled back to a
    whole-frame count, so a large shift is not flattered by having less to
    compare.
    """
    best = (0, 0, int((ours != ref).sum()))
    for dy in range(-span, span + 1):
        for dx in range(-span, span + 1):
            if dx == 0 and dy == 0:
                continue
            a = ours[max(0, dy):H + min(0, dy), max(0, dx):W + min(0, dx)]
            b = ref[max(0, -dy):H + min(0, -dy), max(0, -dx):W + min(0, -dx)]
            n = a.size
            if n == 0:
                continue
            d = int((a != b).sum()) * (W * H) // n
            if d < best[2]:
                best = (dx, dy, d)
    return best


def describe(name, label, ours_path, ref_path):
    ours, nours = ranks(ours_path)
    ref, nref = ranks(ref_path)
    diff = ours != ref
    total = int(diff.sum())
    if total == 0:
        return None
    lines = np.flatnonzero(diff.any(axis=1))
    cols = np.flatnonzero(diff.any(axis=0))
    firsts = [int(np.flatnonzero(diff[y])[0]) for y in lines]
    lasts = [int(np.flatnonzero(diff[y])[-1]) for y in lines]
    dx, dy, shifted = best_shift(ours, ref)
    return {
        "row": f"{name}@{label}",
        "px": total,
        "ranks": (nours, nref),
        "nlines": len(lines),
        "line_span": (int(lines[0]), int(lines[-1])),
        "col_span": (int(cols[0]), int(cols[-1])),
        "first_cols": sorted(set(firsts)),
        "last_cols": sorted(set(lasts)),
        "shift": (dx, dy, shifted),
        "per_line": int(np.median(diff.sum(axis=1)[lines])),
    }


def main():
    argv = sys.argv[1:]
    suite = Path(os.environ.get("MEALYBUG_DIR", "target/test-suites/mealybug"))
    frames = Path("target/mealybug")
    filters = []
    it = iter(argv)
    for a in it:
        if a == "--dir":
            suite = Path(next(it))
        elif a == "--frames":
            frames = Path(next(it))
        else:
            filters.append(a)

    # Re-render first. `target/mealybug/` holds whichever engine ran last, and
    # scoring `identical`'s frames against `standard`'s numbers (or the reverse)
    # invents both regressions and gains — it cost an hour once. Set
    # `MEALYBUG_NO_RUN=1` to read the frames already on disk.
    if not os.environ.get("MEALYBUG_NO_RUN"):
        env = dict(os.environ, GB_PPU=os.environ.get("GB_PPU", "accurate"))
        subprocess.run(
            ["./target/release/examples/mealybug", "--dir", str(suite)],
            env=env, stdout=subprocess.DEVNULL, check=True,
        )

    out = []
    for png in sorted(frames.glob("*.dmg.png")):
        name = png.name[: -len(".dmg.png")]
        if filters and not any(f in name for f in filters):
            continue
        for label, sub, suffix in REFS:
            ref = suite / "expected" / sub / f"{name}.png"
            ours = frames / f"{name}.{suffix}.png"
            if not ref.exists() or not ours.exists():
                continue
            d = describe(name, label, ours, ref)
            if d:
                out.append(d)

    out.sort(key=lambda d: -d["px"])
    hdr = f"{'row':<46} {'px':>6} {'rank':>7} {'lines':>6} {'cols':>11} {'best shift':>18} {'first-diff cols':<28}"
    print(hdr)
    print("-" * len(hdr))
    for d in out:
        dx, dy, sd = d["shift"]
        shift = f"({dx:+d},{dy:+d})->{sd}" if (dx or dy) else "-"
        fc = d["first_cols"]
        fcs = str(fc) if len(fc) <= 5 else f"[{fc[0]}..{fc[-1]}] ({len(fc)})"
        print(
            f"{d['row']:<46} {d['px']:>6} {d['ranks'][0]}v{d['ranks'][1]:>4} "
            f"{d['nlines']:>6} {str(d['col_span']):>11} {shift:>18} {fcs:<28}"
        )
    print(f"\n{len(out)} failing rows")


if __name__ == "__main__":
    main()

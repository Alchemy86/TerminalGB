#!/usr/bin/env python3
"""Print one Mealybug row's frame beside its hardware reference, as shade ranks.

`characterise.py` says which rows share a signature; this says what the
signature actually looks like. Two views:

    python3 tools/mealybug/inspect.py <rom> [DMG|CGB-C|CGB-D] [--cols A B] [--lines A B]

* the two frames side by side as digits 0-3, with a `^` under every differing
  column;
* a summary of, for each differing scanline, where the difference starts and
  ends and what the two sides hold there.
"""

import os
import sys
from pathlib import Path

import numpy as np
from PIL import Image

W, H = 160, 144
SUB = {"DMG": ("DMG-blob", "dmg"), "CGB-C": ("CPU CGB C", "cgb-c"), "CGB-D": ("CPU CGB D", "cgb")}


def ranks(path):
    a = np.asarray(Image.open(path).convert("RGB"), dtype=np.uint32)
    lum = (a[..., 0] * 299 + a[..., 1] * 587 + a[..., 2] * 114) // 1000
    order = np.unique(lum)
    if len(order) > 4:
        lo, hi = int(order[0]), int(order[-1])
        span = max(hi - lo, 1) + 1
        return np.minimum((lum - lo) * 4 // span, 3).astype(np.int8)
    return np.searchsorted(order, lum).astype(np.int8)


def main():
    args = sys.argv[1:]
    if not args:
        print(__doc__)
        return
    rom = args[0]
    label = args[1] if len(args) > 1 and args[1] in SUB else "DMG"
    c0, c1, l0, l1 = 0, 40, 0, 144
    it = iter(args)
    for a in it:
        if a == "--cols":
            c0, c1 = int(next(it)), int(next(it))
        elif a == "--lines":
            l0, l1 = int(next(it)), int(next(it))
    suite = Path(os.environ.get("MEALYBUG_DIR", "target/test-suites/mealybug"))
    sub, suffix = SUB[label]
    ours = ranks(Path("target/mealybug") / f"{rom}.{suffix}.png")
    ref = ranks(suite / "expected" / sub / f"{rom}.png")
    diff = ours != ref

    print(f"{rom}@{label}: {int(diff.sum())} px differ; showing lines {l0}..{l1}, cols {c0}..{c1}")
    print(f"{'ly':>4}  {'ours':<{c1-c0}}  {'hardware':<{c1-c0}}  diff")
    for y in range(l0, min(l1, H)):
        if not diff[y].any():
            continue
        o = "".join(str(v) for v in ours[y, c0:c1])
        r = "".join(str(v) for v in ref[y, c0:c1])
        d = "".join("^" if diff[y, x] else "." for x in range(c0, c1))
        first = int(np.flatnonzero(diff[y])[0])
        last = int(np.flatnonzero(diff[y])[-1])
        print(f"{y:>4}  {o}  {r}  {d}  [{first}..{last}] n={int(diff[y].sum())}")


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Measure a Pewter-chain corpus: how well it solves, and how *differently*.

    python3 tools/pewter-chain/corpus_report.py <corpus-dir> [--maps DIR] [--out DIR]

Diversity is the product, so it is measured rather than asserted. A corpus of
one trajectory replayed teaches a learner nothing, and "we varied the seed" is
not evidence that anything varied.

What is reported, and why each one:

* **distinct final states** — the strongest single number. Two runs with the
  same final-state hash ended the machine in byte-identical condition.
* **distinct trajectories** — the set of tiles each run stood on. Two runs can
  finish identically having walked different ways.
* **mean pairwise Jaccard** over those tile sets: 1.0 is one path replayed,
  0.0 is no shared ground. What a corpus wants is neither.
* **decisions and frames** — spread, not just the mean.
* **the failure table**, because a corpus of only successes hides the hard part.

Heat maps are drawn over PixelGB's map art when a directory of it is given,
and over a plain grid when it is not. They answer the question a table cannot:
*where did the time actually go?*
"""
import argparse
import collections
import json
import os
import random
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chainrender as cr  # noqa: E402

TILE = 16  # a Gen 1 coordinate unit is 16x16 pixels


def load_index(d):
    rows = []
    with open(os.path.join(d, "index.ndjson")) as f:
        for line in f:
            line = line.strip()
            if line:
                rows.append(json.loads(line))
    return rows


def load_paths(d):
    """seed -> [(frame, map, x, y, facing, battle)]

    Reads whichever form the corpus holds — `path-NNNNN.gbtrk`, which every run
    writes now, or the `.csv` `--paths` writes. `chainrender.tracks` is the one
    reader; see `terminalgb::scenario::track` for why the walk counter is not
    in a track any more.
    """
    return {n: cr.read_track(p) for n, p in cr.track_files(d).items()}


def tiles_of(path):
    return {(p[1], p[2], p[3]) for p in path}


def dwell(path):
    """(map,x,y) -> frames stood there. The row's frame is when it began, so a
    row's dwell is the gap to the next row — which is what makes a heat map a
    picture of *time* rather than of *steps*."""
    out = collections.Counter()
    for a, b in zip(path, path[1:]):
        out[(a[1], a[2], a[3])] += max(1, b[0] - a[0])
    if path:
        out[(path[-1][1], path[-1][2], path[-1][3])] += 1
    return out


def jaccard(a, b):
    if not a and not b:
        return 1.0
    return len(a & b) / len(a | b)


def pct(xs, q):
    if not xs:
        return 0
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(q * len(xs)))]


def wilson(k, n, z=1.96):
    """A success rate with an interval, because "100% of 400" without one is
    not a measurement. Rule of three is the k == n case."""
    if n == 0:
        return (0.0, 0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    hw = z * ((p * (1 - p) / n + z * z / (4 * n * n)) ** 0.5) / d
    return (p, max(0.0, c - hw), min(1.0, c + hw))


# --- heat maps ---------------------------------------------------------------

def map_art(maps_dir, map_id):
    """PixelGB names its sheets `<id>-<slug>.png`."""
    if not maps_dir:
        return None
    for name in os.listdir(maps_dir):
        if name.startswith(f"{map_id:03d}-") and name.endswith(".png"):
            return os.path.join(maps_dir, name)
    return None


def draw_heat(counts, map_id, maps_dir, out_png, title):
    try:
        from PIL import Image
    except ImportError:
        print("PIL not available; skipping heat maps", file=sys.stderr)
        return False
    art = map_art(maps_dir, map_id)
    pts = {(x, y): n for (m, x, y), n in counts.items() if m == map_id}
    if not pts:
        return False
    if art:
        base = Image.open(art).convert("RGB")
        w, h = base.size
    else:
        w = (max(x for x, _ in pts) + 2) * TILE
        h = (max(y for _, y in pts) + 2) * TILE
        base = Image.new("RGB", (w, h), (24, 26, 30))
    heat = Image.new("RGB", base.size, (0, 0, 0))
    hp = heat.load()
    top = max(pts.values())
    for (x, y), n in pts.items():
        # Perceptually ordered ramp: dark blue -> cyan -> yellow -> white.
        t = (n / top) ** 0.4
        if t < 0.5:
            r, g, b = int(40 * t * 2), int(120 * t * 2), int(200 * t * 2 + 40)
        else:
            u = (t - 0.5) * 2
            r, g, b = int(40 + 215 * u), int(120 + 135 * u), int(240 - 140 * u)
        for py in range(y * TILE, min(h, (y + 1) * TILE)):
            for px in range(x * TILE, min(w, (x + 1) * TILE)):
                hp[px, py] = (r, g, b)
    out = Image.blend(base, heat, 0.62)
    out.save(out_png)
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("--maps", default=os.path.expanduser("~/pixelgb-out/four-shade/1x"))
    ap.add_argument("--out", default=None)
    ap.add_argument("--sample", type=int, default=200,
                    help="pairs to sample for the Jaccard mean")
    a = ap.parse_args()

    rows = load_index(a.corpus)
    paths = load_paths(a.corpus)
    out_dir = a.out or a.corpus
    os.makedirs(out_dir, exist_ok=True)
    name = os.path.basename(os.path.normpath(a.corpus))

    n = len(rows)
    goals = [r for r in rows if r["goal"]]
    p, lo, hi = wilson(len(goals), n)
    print(f"# {name}")
    print(f"attempts            {n}")
    print(f"reached the goal    {len(goals)}  ({p*100:.1f}%, "
          f"95% CI {lo*100:.1f}-{hi*100:.1f}%)")
    if len(goals) == n and n:
        print(f"                    zero failures in {n} bounds the failure rate at "
              f"about {300.0/n:.2f}% (rule of three)")

    endings = collections.Counter(r["ending"] for r in rows if not r["goal"])
    if endings:
        print("failures            " + ", ".join(f"{k} x{v}" for k, v in
                                                 endings.most_common()))
        where = collections.Counter(
            (r["map"], r["x"], r["y"]) for r in rows if not r["goal"])
        print("                    stopped at " + ", ".join(
            f"map 0x{m:02X} ({x},{y}) x{c}" for (m, x, y), c in where.most_common(5)))

    states = {r["final_state"] for r in rows}
    print(f"distinct end states {len(states)} of {n}")
    dec = [r["decisions"] for r in rows]
    fr = [r["frames"] for r in rows]
    print(f"decisions           min {min(dec)}  p50 {pct(dec,0.5)}  "
          f"p90 {pct(dec,0.9)}  max {max(dec)}")
    print(f"emulated frames     min {min(fr)}  p50 {pct(fr,0.5)}  "
          f"p90 {pct(fr,0.9)}  max {max(fr)}")

    if paths:
        sets = {s: tiles_of(p) for s, p in paths.items()}
        uniq = len({frozenset(v) for v in sets.values()})
        print(f"distinct routes     {uniq} of {len(sets)} (by the set of tiles "
              f"stood on)")
        keys = list(sets)
        rnd = random.Random(1)
        js = []
        for _ in range(min(a.sample, len(keys) * (len(keys) - 1) // 2 or 1)):
            i, j = rnd.sample(keys, 2) if len(keys) > 1 else (keys[0], keys[0])
            js.append(jaccard(sets[i], sets[j]))
        if js:
            print(f"mean pairwise overlap {sum(js)/len(js):.3f}  "
                  f"(1.0 = one path replayed, 0.0 = no shared ground)")

        total = collections.Counter()
        for p in paths.values():
            total += dwell(p)
        by_map = collections.Counter()
        for (m, _, _), c in total.items():
            by_map[m] += c
        print("time by map         " + ", ".join(
            f"0x{m:02X} {100*c/sum(by_map.values()):.0f}%"
            for m, c in by_map.most_common(6)))
        made = []
        for m, _ in by_map.most_common(6):
            png = os.path.join(out_dir, f"heat-{name}-map{m:02X}.png")
            if draw_heat(total, m, a.maps, png, name):
                made.append(png)
        for png in made:
            print(f"heat map            {png}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

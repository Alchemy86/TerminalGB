#!/usr/bin/env python3
"""Stitch several links' certification runs into one continuous track per agent.

    python3 tools/pewter-chain/chain_tracks.py --out DIR \
        01:target/cert/01 02:target/cert/02 03:target/cert/03

`pewter_chain corpus --paths` writes one `path-NNNNN.csv` per attempt, per
link. A film of a *chain* wants one continuous stream per agent, so this joins
them end to end and writes the `agent-NNN.csv` / `agent-NNN.links.csv` pair
`swarm_film.py` reads.

## Why the seeds line up, and why that is not a convenience

**This is a real chain, not three clips laid end to end.** `corpus --exits N`
keeps the first `N` successful exits of a link as its `.gbss` pool, and the
next link's attempt `s` enters from pool slot `s % N`. So for every seed below
`N` the mapping is the identity: attempt `s` of link 02 genuinely began from
the machine attempt `s` of link 01 left behind, and attempt `s` of link 03 from
attempt `s` of link 02. Agent `s` is therefore one continuous playthrough that
really happened, frame for frame, and nothing here interpolates between links
or invents a join.

Above `N` the mapping stops being the identity — attempts share entry states —
so those agents are **dropped rather than stitched**. A film that quietly
spliced two strangers' runs together would be showing a playthrough that never
occurred, which is exactly the class of thing this project measures rather than
implies.

An agent that failed a link stops there. That is the honest picture and it is
what makes the per-link population on screen mean something.
"""
import argparse
import csv
import json
import os
import sys


def index_goals(d):
    """seed -> reached the goal, from the corpus index."""
    out = {}
    p = os.path.join(d, "index.ndjson")
    with open(p) as f:
        for line in f:
            if line.strip():
                r = json.loads(line)
                out[r["seed"]] = bool(r["goal"])
    return out


def read_path(d, seed):
    p = os.path.join(d, f"path-{seed:05d}.csv")
    if not os.path.exists(p):
        return None
    with open(p) as f:
        r = csv.reader(f)
        next(r, None)
        return [tuple(int(v) for v in row) for row in r if row]


def settle(path):
    """Back-fill the frames before the map header exists.

    **`wCurMap` is not "the map is loaded", and a film believes it.** Through
    the whole of Pokémon Blue's introduction — 825 frames, fourteen seconds of
    a 1x cut — `LoadMapHeader` has not run, so the position bytes are whatever
    was in WRAM: map `$FC` at (26,8) for a few frames and then **map 0 at
    (0,0)** until the header lands. Drawn literally, all six hundred agents
    stand in Pallet Town's top-left corner for the first fourteen seconds,
    the camera parks on that corner, and the bedroom panel they are actually
    in sits just off the edge of frame with its agents cut in half.

    That was reported as a clipped panel and it is not a layout fault: it is
    this. The same trap link 01's own goal condition guards against by
    requiring `wCurMapWidth > 0` rather than trusting `wCurMap` — see
    `scenarios/pewter/01-new-game.json`'s evidence note.

    The player is in Red's bedroom the whole time; only the header is missing.
    So the leading run is back-filled with the first position the cartridge
    genuinely reports, which is both where the player is and what puts the
    camera on the room the film opens in. Only the **leading** run is touched,
    so a later, legitimate visit to a map's corner is untouched.
    """
    real = next(
        (i for i, r in enumerate(path)
         if r[1] != 0xFC and not (r[1] == 0 and r[2] == 0 and r[3] == 0)),
        None,
    )
    if real is None or real == 0:
        return path
    _, m, x, y, fa, _, _ = path[real]
    return [(r[0], m, x, y, fa, 0, 0) for r in path[:real]] + path[real:]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("links", nargs="+",
                    help="NAME:DIR per link, in chain order")
    ap.add_argument("--out", required=True)
    ap.add_argument("--agents", type=int, default=600)
    a = ap.parse_args()

    parts = []
    for spec in a.links:
        name, _, d = spec.partition(":")
        if not d:
            sys.exit(f"expected NAME:DIR, got {spec!r}")
        parts.append((name, d, index_goals(d)))

    os.makedirs(a.out, exist_ok=True)
    written = 0
    reached = [0] * len(parts)
    for seed in range(a.agents):
        rows, spans, base = [], [], 0
        for n, (name, d, goals) in enumerate(parts, start=1):
            path = read_path(d, seed)
            if not path:
                break
            path = settle(path)
            last = 0
            for f, m, x, y, fa, w, b in path:
                rows.append((base + f, m, x, y, fa, w, b))
                last = base + f
            goal = goals.get(seed, False)
            spans.append((n, name, base, last, goal))
            reached[n - 1] += 1
            # The next link starts where this one finished, so its frame
            # numbers continue rather than restart.
            base = last + 1
            if not goal:
                # Stopped here. Not a gap in the film — the agent is simply
                # still standing wherever it gave up, which is the picture.
                break
        if not rows:
            continue
        with open(os.path.join(a.out, f"agent-{seed:03d}.csv"), "w") as f:
            f.write("frame,map,x,y,facing,walking,battle\n")
            for r in rows:
                f.write(",".join(str(v) for v in r) + "\n")
        with open(os.path.join(a.out, f"agent-{seed:03d}.links.csv"), "w") as f:
            f.write("link,name,start,end,goal\n")
            for lk, name, s0, s1, g in spans:
                f.write(f"{lk},{name},{s0},{s1},{'true' if g else 'false'}\n")
        written += 1

    print(f"{written} continuous agent track(s) -> {a.out}")
    for i, (name, _, _) in enumerate(parts):
        print(f"  link {i + 1} {name}: {reached[i]} agents entered it")


if __name__ == "__main__":
    main()

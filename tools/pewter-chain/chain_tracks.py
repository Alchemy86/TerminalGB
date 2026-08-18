#!/usr/bin/env python3
"""Stitch several links' certification runs into one continuous track per agent,
and SAMPLE which of them a film gets.

    python3 tools/pewter-chain/chain_tracks.py --out DIR \
        01:target/cert/01 02:target/cert/02 03:target/cert/03

Every run records where the player was — `pewter_chain` writes
`path-NNNNN.gbtrk` for each attempt whether or not anybody asked — so a film of
a *certified* run is now a selection over a certification that already
happened, not a second pass that re-runs the work with `--paths` on. That is the
whole point of the change: a certification's failures, blackouts and tail used
to be thrown away at the moment they happened and could not be recovered.

This joins the per-link tracks end to end and writes the `agent-NNN.gbtrk` /
`agent-NNN.links.csv` pair `swarm_film.py` reads.

## Selecting runs — the four questions

    --select goal|failed|all   by OUTCOME  (default: all)
    --sort   fastest|slowest|seed          (default: seed)
    --count  N                 how many to keep
    --seeds  3,7,11-14         exactly these

**A film of the failures is now a one-liner**, which it could not be at all
before:

    chain_tracks.py --select failed --count 60 --out target/film-failures \
        20:target/cert/20 21:target/cert/21

An agent's outcome is the outcome of the **last link it attempted**: it
"reached the goal" only if it reached the goal of every link asked for, and it
"failed" the moment one of them stopped it. Speed is the total emulated frames
of the stitched track, so `--sort fastest` is the shortest playthrough and not
the shortest wall clock — a wall clock measures the box's afternoon.

`--seeds` names attempts of the **last** link and is exact: a seed that cannot
be stitched honestly is refused by name rather than silently dropped, because
a named seed is a claim about a particular run.

## Why the seeds line up, and why that is NOT an assumption any more

**This is a real chain, not several clips laid end to end**, and the rule that
makes it one is now computed rather than believed. `corpus --exits N` keeps the
first N *successful* exits of a link and names each after the seed that
produced it; the next link's attempt `s` enters from slot `s % len` of that
pool. So slot `i` came from seed `names[i]`, and an agent's whole ancestry
walks backwards exactly — see `provenance`.

The old rule was "agent `s` is seed `s` at every link", and **it is only true
of a chain that never failed**. A link that failed even once leaves a pool with
gaps (`00000, 00001, 00003, …`), so attempt 2 of the next link entered from
seed **3**, and stitching it against seed 2 would show a playthrough that never
happened. Reading the names instead costs nothing and is right either way; the
corpus's own recorded entry slot is cross-checked against the arithmetic, and
an attempt whose provenance cannot be proved is **refused rather than
stitched**. That is the property sampling had to survive, and it survives it
because selection happens over the set that was proved, never over the raw
seeds.

An agent that failed a link stops there. That is the honest picture and it is
what makes the per-link population on screen mean something. The
`agent-NNN.links.csv` beside each track names the seed each link really was.
"""
import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chainrender as cr  # noqa: E402


def index_rows(d):
    """seed -> the row `corpus` wrote about it."""
    out = {}
    p = os.path.join(d, "index.ndjson")
    with open(p) as f:
        for line in f:
            if line.strip():
                r = json.loads(line)
                out[r["seed"]] = r
    return out


def pool_seeds(d):
    """Which seed produced each slot of this link's exit pool, in slot order.

    **This is what makes a stitch provable instead of assumed.** `--exits N`
    keeps the first N *successful* exits and names each after its own seed, so
    a link that ever failed leaves a pool with gaps — `00000, 00001, 00003, …`
    — and the next link's attempt `s` enters from slot `s % len`, which is
    seed **3** for `s = 2`. The old rule ("agent `s` is seed `s` all the way
    down") is therefore only true of a chain that never failed, and it silently
    stops being true the moment one does.

    Reading the names turns that into arithmetic: slot `i` came from seed
    `names[i]`, so the whole provenance of an agent can be walked back link by
    link and the film shows the playthrough that really happened.

    `pewter_chain pool pack` keeps the names, so a packed pool answers too.
    A link with neither answers `None`, and every agent whose ancestry runs
    through it is refused rather than guessed at.
    """
    ed = os.path.join(d, "exits")
    if os.path.isdir(ed):
        names = sorted(f[:-5] for f in os.listdir(ed) if f.endswith(".gbss"))
        if names:
            return [int(n) for n in names]
    for name in ("exits.gbpool", "exits.pool"):
        p = os.path.join(d, name)
        if os.path.exists(p):
            return pool_names_from_archive(p)
    return None


def pool_names_from_archive(p):
    """The member names of a `GBPL` pool, in slot order, without unpacking it.

    See `terminalgb::scenario::pool` for the format; this walks the members'
    headers and steps over their delta runs.
    """
    with open(p, "rb") as f:
        b = f.read()
    if len(b) < 14 or b[:4] != b"GBPL" or b[4] != 1 or b[5] != 0:
        return None

    at = [14 + int.from_bytes(b[10:14], "little")]

    def varint():
        n = shift = 0
        while True:
            byte = b[at[0]]
            at[0] += 1
            n |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return n

    names = []
    try:
        for _ in range(int.from_bytes(b[6:10], "little")):
            nl = b[at[0]]
            at[0] += 1
            names.append(int(b[at[0]:at[0] + nl]))
            at[0] += nl
            varint()                       # the member's own length
            for _ in range(varint()):      # its differing runs
                varint()                   # offset
                # NOT `at[0] += varint()`: Python reads the left operand
                # before calling the right, so the varint's own advance would
                # be thrown away.
                n = varint()               # length, then the bytes
                at[0] += n
    except (IndexError, ValueError):
        return None
    return names


def read_track(d, seed):
    for ext in (".gbtrk", ".csv"):
        p = os.path.join(d, f"path-{seed:05d}{ext}")
        if os.path.exists(p):
            return cr.read_track(p), cr.track_last_frame(p)
    return None, 0


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
    _, m, x, y, fa, _ = path[real]
    return [(r[0], m, x, y, fa, 0) for r in path[:real]] + path[real:]


def parse_seeds(spec):
    """`3,7,11-14` -> [3, 7, 11, 12, 13, 14]."""
    out = []
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        if "-" in part[1:]:
            a, _, b = part.partition("-")
            out.extend(range(int(a), int(b) + 1))
        else:
            out.append(int(part))
    return out


def provenance(parts, seed):
    """Which seed of each link the agent that ends as `seed` really was.

    **The honesty property, computed rather than assumed.** Attempt `s` of
    link *k* entered from slot `s % len` of link *k-1*'s exit pool, and slot
    `i` of that pool was produced by seed `names[i]` — so the chain walks
    backwards exactly. For a pool with no gaps this is the identity and the
    answer is `[s, s, …, s]`, which is what the chain has always relied on;
    for a pool with gaps it is not, and the difference is a film showing a
    playthrough that happened against one that did not.

    Returns `None` when it cannot be proved: a link with no exit listing, or a
    link whose index disagrees with the slot the arithmetic predicts. Refusing
    is the point — the alternative is splicing two strangers' runs together
    and calling it a playthrough.
    """
    seeds = [None] * len(parts)
    seeds[-1] = seed
    for i in range(len(parts) - 1, 0, -1):
        names = parts[i - 1][3]
        if not names:
            return None
        slot = seeds[i] % len(names)
        row = parts[i][2].get(seeds[i])
        if row is None:
            return None
        # **How big the pool WAS, not how big it is.** The slot the attempt
        # actually took is recorded, and so is the pool it indexed into; a pool
        # that has since lost or gained a state maps that slot to a different
        # seed, and every agent above the gap would be stitched against the
        # wrong run. Refusing the lot is right — the numbers no longer describe
        # the directory. A corpus written before that field existed is checked
        # the weaker way it allows.
        if row.get("pool", len(names)) != len(names):
            return None
        if row.get("entry", slot) != slot:
            return None
        seeds[i - 1] = names[slot]
    return seeds


def stitch(parts, seed):
    """One agent's whole chain, or `None` if it cannot be stitched honestly.

    Returns `(rows, spans, frames, reached, goal, seeds)`.
    """
    seeds = provenance(parts, seed)
    if seeds is None:
        return None
    rows, spans, base, reached = [], [], 0, 0
    goal = False
    for n, ((name, d, rowsx, _names), s) in enumerate(zip(parts, seeds), start=1):
        path, last_frame = read_track(d, s)
        if not path:
            break
        path = settle(path)
        for f, m, x, y, fa, b in path:
            rows.append((base + f, m, x, y, fa, b))
        end = base + max(last_frame, path[-1][0])
        goal = bool(rowsx.get(s, {}).get("goal", False))
        spans.append((n, name, base, end, goal))
        reached = n
        # The next link starts where this one finished, so its frame
        # numbers continue rather than restart.
        base = end + 1
        if not goal:
            # Stopped here. Not a gap in the film — the agent is simply
            # still standing wherever it gave up, which is the picture.
            break
    if not rows:
        return None
    return rows, spans, spans[-1][3], reached, goal and reached == len(parts), seeds


def main():
    ap = argparse.ArgumentParser(
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="examples:\n"
               "  # every agent of a two-link certification\n"
               "  chain_tracks.py --out F 20:cert/20 21:cert/21\n"
               "  # the sixty fastest that finished\n"
               "  chain_tracks.py --out F --select goal --sort fastest "
               "--count 60 20:cert/20 21:cert/21\n"
               "  # a film of the FAILURES\n"
               "  chain_tracks.py --out F --select failed 20:cert/20\n")
    ap.add_argument("links", nargs="+",
                    help="NAME:DIR per link, in chain order")
    ap.add_argument("--out", required=True)
    ap.add_argument("--agents", type=int, default=0,
                    help="cap on how many seeds of the LAST link to consider")
    ap.add_argument("--select", choices=("all", "goal", "failed"),
                    default="all", help="by outcome (default: all)")
    ap.add_argument("--sort", choices=("seed", "fastest", "slowest"),
                    default="seed")
    ap.add_argument("--count", type=int, default=0, help="how many to keep")
    ap.add_argument("--seeds", default="",
                    help="exactly these seeds of the last link: 3,7,11-14")
    a = ap.parse_args()

    parts = []
    for spec in a.links:
        name, _, d = spec.partition(":")
        if not d:
            sys.exit(f"expected NAME:DIR, got {spec!r}")
        parts.append((name, d, index_rows(d), pool_seeds(d)))

    last_seeds = sorted(parts[-1][2])
    if a.seeds:
        wanted = parse_seeds(a.seeds)
        missing = [s for s in wanted if s not in parts[-1][2]]
        if missing:
            sys.exit("no such attempt(s) in "
                     f"{parts[-1][1]}: {', '.join(str(s) for s in missing)}")
        last_seeds = wanted
    if a.agents:
        last_seeds = last_seeds[:a.agents]

    os.makedirs(a.out, exist_ok=True)
    stitched, refused, reached = [], [], [0] * len(parts)
    for seed in last_seeds:
        r = stitch(parts, seed)
        if r is None:
            refused.append(seed)
            continue
        rows, spans, frames, got, goal, seeds = r
        for i in range(got):
            reached[i] += 1
        stitched.append((seed, rows, spans, frames, goal, seeds))

    if a.seeds and refused:
        # A named seed is a claim about a particular run, so it is refused by
        # name rather than quietly dropped.
        sys.exit(
            f"cannot stitch seed(s) {', '.join(str(s) for s in refused)} "
            "honestly: the entry pool of an earlier link cannot say which "
            "attempt produced the state they started from")

    # --- the selectors -----------------------------------------------------
    pool = stitched
    if a.select == "goal":
        pool = [s for s in pool if s[4]]
    elif a.select == "failed":
        pool = [s for s in pool if not s[4]]
    if a.sort == "fastest":
        pool.sort(key=lambda s: (s[3], s[0]))
    elif a.sort == "slowest":
        pool.sort(key=lambda s: (-s[3], s[0]))
    else:
        pool.sort(key=lambda s: s[0])
    if a.count:
        pool = pool[:a.count]

    for seed, rows, spans, _frames, _goal, seeds in pool:
        cr.write_track(os.path.join(a.out, f"agent-{seed:03d}.gbtrk"), rows,
                       last_frame=spans[-1][3])
        with open(os.path.join(a.out, f"agent-{seed:03d}.links.csv"), "w") as f:
            f.write("link,name,start,end,goal,seed\n")
            for (lk, name, s0, s1, g), s in zip(spans, seeds):
                f.write(f"{lk},{name},{s0},{s1},{'true' if g else 'false'},{s}\n")

    print(f"{len(pool)} continuous agent track(s) -> {a.out}")
    n_goal = sum(1 for s in stitched if s[4])
    print(f"  eligible: {len(stitched)} of {len(last_seeds)} attempt(s) of "
          f"{parts[-1][0]} stitch back to a provable playthrough; "
          f"{n_goal} reached every link, {len(stitched) - n_goal} did not")
    if refused:
        print(f"  refused: {len(refused)} attempt(s) whose provenance could "
              "not be proved — see `provenance`")
    if a.select != "all" or a.count or a.seeds or a.sort != "seed":
        print(f"  selected: --select {a.select} --sort {a.sort}"
              + (f" --count {a.count}" if a.count else "")
              + (f" --seeds {a.seeds}" if a.seeds else ""))
    if pool:
        fr = [s[3] for s in pool]
        print(f"  frames: {min(fr)} shortest, {max(fr)} longest, "
              f"{sum(fr) // len(fr)} mean")
    for i, (name, _, _, _) in enumerate(parts):
        print(f"  link {i + 1} {name}: {reached[i]} agent(s) entered it")
    if not pool:
        print("  nothing selected — try --select all")


if __name__ == "__main__":
    main()

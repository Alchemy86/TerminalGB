"""Reading agent position tracks, and the walk-step arithmetic.

Moved here verbatim from swarm_film.py so there is ONE of it.
See chainrender/__init__.py for why this package exists.
"""

import collections
import csv
import os
import re

# The Game Boy's own walk tile. Used only where nothing else can answer — the
# camera's minimum width and the sprite's nominal size. **Every position on
# screen comes from the atlas entry's own `tile_px`**, never from this.
TILE = 16
FPS_GB = 59.7275
# **Measured on the cartridge, from the tracks themselves.** `wWalkCounter`
# ($CFC5, atlas `rom,live`) runs 7,6,5,4,3,2,1 with each value held two frames —
# fourteen frames of animation — and `wXCoord`/`wYCoord` ($D362/$D361) update on
# the frame it reaches 0. So **during a step the coordinate is still the tile
# being left**, and the sprite is somewhere between it and the next one.
#
# That is the whole reason a film drawn from coordinates alone teleports, at
# any compression and any frame rate: the coordinate does not move during the
# walk, it snaps at the end of it. The offset from the coordinate towards the
# facing direction is `(8 - counter) * 2` pixels, which is 2 px at counter 7 and
# 14 px at counter 1 — a sixteen-pixel tile crossed in eight steps of two.
STEP_PX = 2
WALK_STEPS = 8
# Fallback glide span. **The real one is MEASURED from the tracks at run time**
# (`walk_span`) — see there for why a constant was the wrong shape.
WALK_FRAMES = 14
FACING = {0: "down", 4: "up", 8: "left", 12: "right"}


def track_files(d):
    """`agent -> path`, for every track file in a directory.

    `agent-NNN` from a whole-chain film run, or `path-NNNNN` from a corpus of
    one link — the same track either way, and a showcase of one busy stretch
    wants the second. Either may be `.gbtrk` (what every run writes now) or
    `.csv` (what `--paths` writes, and what corpora collected before the binary
    format existed). **A `.gbtrk` wins where both are present**: they carry the
    same points and the binary one is the source.
    """
    out = {}
    for name in sorted(os.listdir(d)):
        if name.endswith(".links.csv"):
            continue
        if name.endswith(".gbtrk"):
            stem, rank = name[:-6], 0
        elif name.endswith(".csv"):
            stem, rank = name[:-4], 1
        else:
            continue
        if stem.startswith("agent-"):
            n = int(stem[6:])
        elif stem.startswith("path-"):
            n = int(stem[5:])
        else:
            continue
        if n not in out or rank < out[n][0]:
            out[n] = (rank, os.path.join(d, name))
    return {n: p for n, (_, p) in out.items()}


GBTRK_MAGIC = b"GBTK"


def read_track(path):
    """Every recorded point of one track, as `(frame, map, x, y, facing, battle)`.

    **The one reader**, so a tool never has to know which of the two forms it
    was handed. See `terminalgb::scenario::track` for the binary format and for
    the measurement that produced it — 35-43x smaller than the CSV, against
    4.4x for `gzip -9` on the same files, because most of what the CSV held was
    the walk counter ticking and nothing has ever read it.

    A legacy seven-column CSV (`frame,map,x,y,facing,walking,battle`) is
    accepted and its walk counter dropped on the way in, so a corpus collected
    before this still renders.
    """
    with open(path, "rb") as f:
        head = f.read(4)
        if head == GBTRK_MAGIC:
            return decode_track(head + f.read())
    with open(path) as f:
        r = csv.reader(f)
        next(r, None)
        out = []
        for row in r:
            if not row:
                continue
            v = [int(x) for x in row]
            if len(v) == 7:            # legacy: drop the walk counter
                v = v[:5] + v[6:]
            v = tuple(v)
            # With the counter gone a legacy CSV holds runs of rows that changed
            # nothing. Collapsing them here is what makes one reader mean one
            # shape, whichever form the track was stored in.
            if out and out[-1][1:] == v[1:]:
                continue
            out.append(v)
        return out


def decode_track(b):
    """`GBTK` bytes -> `[(frame, map, x, y, facing, battle)]`.

    Refuses rather than returning an empty list: a track that silently decodes
    to nothing is a film of an empty world, which reads as a broken renderer
    rather than as a broken file.
    """
    if len(b) < 14 or b[:4] != GBTRK_MAGIC:
        raise ValueError("not a GBTK track")
    if b[4] != 1:
        raise ValueError(f"track version {b[4]} — this reader understands 1")
    if b[5] != 0:
        raise ValueError(f"track flags {b[5]:#04x} are not understood")
    count = int.from_bytes(b[6:10], "little")
    at = 14
    cur = [0, 0, 0, 0, 0]      # map, x, y, facing, battle
    frame = 0
    out = []
    for i in range(count):
        mask = b[at]
        at += 1
        n = 0
        shift = 0
        while True:
            byte = b[at]
            at += 1
            n |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                break
        frame += n
        for bit in range(5):
            if mask & (1 << bit):
                cur[bit] = b[at]
                at += 1
        out.append((frame, cur[0], cur[1], cur[2], cur[3], cur[4]))
    return out


def track_last_frame(path):
    """The last emulated frame the run reached, which is not the last record.

    A run that ends standing still has its final record long before its final
    frame. The binary form carries it; a CSV cannot, so the last record's frame
    is the best a legacy track can answer.
    """
    with open(path, "rb") as f:
        head = f.read(14)
        if len(head) >= 14 and head[:4] == GBTRK_MAGIC:
            return int.from_bytes(head[10:14], "little")
    pts = read_track(path)
    return pts[-1][0] if pts else 0


def write_track(path, points, last_frame=None):
    """`[(frame, map, x, y, facing, battle)]` -> a `GBTK` file."""
    body = bytearray()
    prev = None
    pf = 0
    n = 0
    last = last_frame or 0
    for p in points:
        fr, cur = p[0], tuple(p[1:6])
        last = max(last, fr)
        if cur == prev:
            continue
        if prev is None:
            mask, fields = 0x1F, list(cur)
        else:
            mask, fields = 0, []
            for i, (a, c) in enumerate(zip(prev, cur)):
                if a != c:
                    mask |= 1 << i
                    fields.append(c)
        body.append(mask)
        d = fr - pf
        while True:
            x = d & 0x7F
            d >>= 7
            body.append(x | (0x80 if d else 0))
            if not d:
                break
        body += bytes(fields)
        prev, pf, n = cur, fr, n + 1
    with open(path, "wb") as f:
        f.write(GBTRK_MAGIC + bytes([1, 0])
                + n.to_bytes(4, "little") + last.to_bytes(4, "little") + body)


def flag_at(pairs, frame):
    """The battle flag in force at `frame`, from its change points.

    Zero before the track starts, which is "not in a battle" and is what the
    renderer wants for an agent that has not entered the film yet.
    """
    if not pairs or frame < pairs[0][0]:
        return 0
    lo, hi = 0, len(pairs) - 1
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if pairs[mid][0] <= frame:
            lo = mid
        else:
            hi = mid - 1
    return pairs[lo][1]

def load_links(d):
    """agent -> [(link, name, start, end, goal)], so the film can say which of
    the fourteen an agent is on. Half the links happen on the same map, so a
    position track alone cannot answer it."""
    out = {}
    for name in sorted(os.listdir(d)):
        if not name.endswith(".links.csv"):
            continue
        n = int(name.split("-")[1].split(".")[0])
        with open(os.path.join(d, name)) as f:
            r = csv.reader(f)
            next(r, None)
            # Six columns since the stitcher started recording which seed each
            # link really was (`chain_tracks.provenance`); five before it.
            out[n] = [(int(row[0]), row[1], int(row[2]), int(row[3]),
                       row[4] == "true")
                      for row in r if row]
    return out

def link_at(spans, frame):
    for link, name, start, end, goal in spans:
        if start <= frame <= end:
            return link, name, goal
    return (spans[-1][0], spans[-1][1], spans[-1][4]) if spans else (0, "", False)

def walk_span(paths, lo=8, hi=48, fallback=WALK_FRAMES):
    """How long one step takes, **measured from the tracks in hand**.

    The glide span used to be a hard-coded 14 frames, reasoned from
    `wWalkCounter` running 7..1 with each value held two frames. Measured over
    the whole certified chain, **no adjacent-tile arrival is ever that close
    together**: the gaps are 17 (34%), 27-31 (40%) and 52-55 (9%), because a
    track is sampled every 2-3 frames and the solver's own decision cadence
    sits between some steps. So a 14-frame window guaranteed a freeze before
    *every* step, of 3 frames at best and 17 at worst — and that is exactly the
    reported "agents jump then pause". The animation constant was right about
    the cartridge and wrong about the data.

    Taking the mode rather than the mean because the distribution is
    multi-modal, one mode per number of steps' worth of waiting; the first is
    the step itself. Bounded, so a corpus of nothing but long waits cannot
    produce a span that glides an agent across a minute of standing still.
    """
    gaps = collections.Counter()
    for p in paths.values():
        for a, b in zip(p, p[1:]):
            if a[1] == b[1] and abs(a[2] - b[2]) + abs(a[3] - b[3]) == 1:
                g = b[0] - a[0]
                if lo <= g <= hi:
                    gaps[g] += 1
    return gaps.most_common(1)[0][0] if gaps else fallback

def interpolate(path, frame, span=WALK_FRAMES):
    """Where an agent is, between the tile it left and the tile it is going to.

    **Renderer-side interpolation, not a memory read.** Positioning from
    `wXCoord`/`wYCoord` alone teleports by construction — the coordinate does
    not move during a walk, it snaps at the end of it — and reading the
    sub-tile offset out of WRAM only helps at sampling rates fine enough to
    catch it. Lerping between consecutive *recorded* tiles is independent of
    the sampling rate entirely, works on tracks already on disk, and is the
    same four lines Peter Whidden's own map visualiser uses
    (`pwhiddy/pokerl-map-viz`, `visualizer_live.js`).

    The progress fraction runs over the last `span` frames before the arrival
    rather than over the whole gap, so an agent that stood still for a thousand
    frames and then took one step does exactly that instead of drifting slowly
    the whole time — **but `span` is the step time MEASURED from these tracks
    (`walk_span`), never a constant**, and never longer than the gap. A span
    shorter than the real cadence freezes the agent before every step, which is
    what "jump then pause" was.

    Note what this is *not*: it is not per-decision, and it is not sampled. `t`
    is recomputed from the output frame's own emulated frame number every time
    a frame is drawn, so at any compression the agent advances by exactly the
    fraction of the step that elapsed.
    """
    lo, hi = 0, len(path) - 1
    if frame <= path[0][0]:
        return path[0][1], float(path[0][2]), float(path[0][3]), path[0][4]
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if path[mid][0] <= frame:
            lo = mid
        else:
            hi = mid - 1
    cur = path[lo]
    if lo + 1 >= len(path):
        return cur[1], float(cur[2]), float(cur[3]), cur[4]
    nxt = path[lo + 1]
    # Only between adjacent tiles on the same map. A warp is a jump, not a walk.
    if nxt[1] != cur[1] or abs(nxt[2] - cur[2]) + abs(nxt[3] - cur[3]) != 1:
        return cur[1], float(cur[2]), float(cur[3]), cur[4]
    w = min(span, max(1, nxt[0] - cur[0]))
    t = (frame - (nxt[0] - w)) / float(w)
    t = 0.0 if t < 0.0 else (1.0 if t > 1.0 else t)
    return (
        cur[1],
        cur[2] + (nxt[2] - cur[2]) * t,
        cur[3] + (nxt[3] - cur[3]) * t,
        nxt[4] if t > 0 else cur[4],
    )

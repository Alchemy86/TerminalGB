#!/usr/bin/env python3
"""Draw many agents playing the whole Pewter chain, on one continuous world.

    python3 tools/pewter-chain/swarm_film.py <tracks-dir> --sprites <dir> \
        --atlas <atlas.json> --out <dir> [--seconds 70] [--fps 30]

Inspired by Peter Whidden's *Training AI to Play Pokémon with Reinforcement
Learning* — the swarm view is his idea and the credit is his. What is different
here, and each of these is a property of the substrate rather than a flourish:

* the map art is **photographed from the running cartridge** (PixelGB), not
  reconstructed from data files;
* the agents are the game's **own player sprite**, read out of OAM and VRAM,
  facing the way the game says they face;
* motion is **continuous** — `wWalkCounter` counts a step down over eight
  frames, so a walking agent is drawn between tiles rather than snapping;
* the whole thing is **deterministic**: same seeds, same cartridge, same film;
* and every agent carries a **number**, so one of them can be followed and
  checked against its own row in the corpus.

## The world is READ, never improvised

Every map's place comes from PixelGB's atlas manifest (`atlas.json`) and from
nothing else. An agent at `(map, x, y)` is drawn at
`rect.x + x * tile_px`, and that is the whole of the placement.

This replaces a hand-written layout that this compositor used to carry: five
outdoor origins transcribed from the map headers' `connection` lines, a lookup
table putting each interior beside its own door, a gutter-packer, and a
connector line drawn from each panel to the door it belonged to. All of it was
a **presentation guess** standing in for a layout that did not exist yet. The
atlas is the layout: outdoor maps at the offsets the cartridge's own connection
records give, interiors grouped into tidy blocks beside their own area. There
is deliberately **one** way to place a map here now.

Two rules the manifest interface earns, both of which the old code got wrong at
some point:

* **honour each entry's own `scale` and `tile_px`** rather than assuming 16 —
  the manifest repeats them per map precisely so nothing has to be assumed;
* **a map the atlas does not place is UNPLACED**, never drawn at the origin. An
  agent on one is counted and reported, exactly like an agent off frame. That is
  the same failure that once put six hundred agents on Pallet Town's corner.

## Sampling is not optional

A chain is minutes of emulated time. Compositing every frame gives a film that
plays at real time and is boring — the rule `docs/swarm-view.md` states and
which is easy to lose in a rebuild. The sampling rate is **derived from the
emulated time to cover and the length wanted**, not picked.
"""
import argparse
import collections
import csv
import json
import math
import os
import sys

# The Game Boy's own walk tile. Used only where nothing else can answer — the
# camera's minimum width and the sprite's nominal size. **Every position on
# screen comes from the atlas entry's own `tile_px`**, never from this.
TILE = 16
FPS_GB = 59.7275

FACING = {0: "down", 4: "up", 8: "left", 12: "right"}
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

# **How far above its own tile the player sprite is drawn, in tiles. MEASURED
# AGAINST THE CARTRIDGE, not eyeballed.** A real game screen was captured at a
# known tile through `terminalgb-mcp` (`observe` for the coordinate,
# `screenshot` for the frame) and matched against PixelGB's four-shade atlas —
# which is photographed from the same cartridge, so its tile grid *is* the
# game's. Sweeping the hypothesis "the player's tile top-left is at screen
# (ax, ay)" over the whole frame gives exactly one answer:
#
#     screen ( 64, 64)     153 of 12288 px differ =  1.25%
#     screen ( 64, 56)    1297 of 11264 px differ = 11.51%
#
# — a 9.2x margin over the runner-up, and the 153 pixels that do differ are the
# player, absent from the atlas. Their bounding box is x 65..78, **y 60..75**:
# sixteen pixels tall, sitting four pixels ABOVE the 64..79 tile box. So a tile
# is 16 px, an agent stands in the middle of it, and its sprite is lifted a
# quarter of a tile. Reproduce with `tools/pewter-chain/measure_anchor.py`.
SPRITE_LIFT = 4 / 16

BG = (16, 18, 22)

# **The world stays grey and the agents carry the colour.** That is what makes
# a crowd readable without labels: the eye separates agent from world instantly
# and the colour carries the meaning. A Game Boy frame is four shades already,
# so nothing has to be desaturated to get there.
#
# **The hue is the heat map, read at the agent's own feet** — how many
# agent-frames the swarm has already spent on the tile it is standing on. Not
# progress: novelty. Standing where thousands have already been is one end of
# the scale, standing on rarely-trodden ground is the other, so **the frontier
# lights up on its own** and an agent doing something genuinely new stands
# apart from the crowd milling around Pallet Town.
#
# It is the same visitation count `corpus_report.py` draws its heat maps from —
# one computation, one source of truth — and it is count-based exploration
# novelty made visible: the colour *is* the signal an RL agent would be
# rewarded on.
#
# **This is a declared exception to the categorical colour guard**, not a
# silent one. That guard says no colour may appear that was not in the source
# framebuffer, and it still applies **to the map layer**: the map art is
# reproduced exactly. The agent overlay is deliberately outside it.
HEAT_HUE = [
    (70, 110, 210), (70, 130, 215), (70, 150, 220), (70, 175, 215),
    (70, 195, 195), (80, 205, 160), (110, 210, 120), (150, 215, 90),
    (195, 210, 70), (225, 195, 65), (240, 165, 60), (245, 130, 60),
    (250, 100, 70), (255, 80, 110),
]


def tint(sprite, rgb, Image):
    """Recolour a four-shade sprite onto one hue, keeping its own shading and
    its alpha. Nothing is invented: the luminance ramp is the sprite's."""
    px = sprite.load()
    out = Image.new("RGBA", sprite.size)
    op = out.load()
    for y in range(sprite.height):
        for x in range(sprite.width):
            r, g, b, al = px[x, y]
            if al == 0:
                op[x, y] = (0, 0, 0, 0)
                continue
            # 0 = the sprite's darkest ink, 1 = its lightest.
            t = r / 255.0
            op[x, y] = (
                int(rgb[0] * (0.35 + 0.65 * t)),
                int(rgb[1] * (0.35 + 0.65 * t)),
                int(rgb[2] * (0.35 + 0.65 * t)),
                al,
            )
    return out


def load_tracks(d):
    out = {}
    for name in sorted(os.listdir(d)):
        # `agent-NNN.csv` from a whole-chain film run, or `path-NNNNN.csv`
        # from a corpus of one link — the same format either way, and a
        # showcase of one busy stretch wants the second.
        if name.endswith(".links.csv") or not name.endswith(".csv"):
            continue
        if name.startswith("agent-"):
            n = int(name[6:-4])
        elif name.startswith("path-"):
            n = int(name[5:-4])
        else:
            continue
        with open(os.path.join(d, name)) as f:
            r = csv.reader(f)
            next(r, None)
            rows = [tuple(int(v) for v in row) for row in r if row]
        if rows:
            out[n] = rows
    return out


class Atlas:
    """PixelGB's Kanto atlas: one image, and where every map sits in it.

    The manifest is the whole of the layout. Nothing here searches, packs or
    guesses — `rect` says where a map is drawn and `tile_px` says how big its
    walk tile is, so `place()` is one multiply and one add.
    """

    def __init__(self, manifest_path, image_path=None,
                 rendering="region-tint", scale=1):
        with open(manifest_path) as f:
            man = json.load(f)
        d = os.path.dirname(os.path.abspath(manifest_path))
        self.cartridge = man.get("cartridge", {})
        self.placement = man.get("placement", {})
        geom_scale = man.get("geometry", {}).get("scale", 1)

        if image_path is None:
            image_path, scale = self._pick_image(man, d, rendering, scale)
        # "the 2x, 4x … images are nearest-neighbour enlargements of this same
        # layout: multiply any rect by the scale" — geometry, atlas.json.
        self.mul = scale / float(geom_scale)
        self.image_path = image_path
        self.scale = scale

        self.origin, self.tile_px, self.kind, self.label = {}, {}, {}, {}
        self.size, self.tiles = {}, {}
        for m in man["maps"]:
            r = m["rect"]
            self.tiles[m["id"]] = (m["tiles"]["width"], m["tiles"]["height"])
            self.origin[m["id"]] = (r["x"] * self.mul, r["y"] * self.mul)
            self.size[m["id"]] = (r["width"] * self.mul, r["height"] * self.mul)
            # Per-entry, as the manifest asks. This atlas happens to be uniform;
            # honouring the field costs a dictionary and means a future atlas
            # that draws interiors smaller cannot silently misplace them.
            self.tile_px[m["id"]] = m["tile_px"] * self.mul
            self.kind[m["id"]] = m["kind"]
            self.label[m["id"]] = m["name"].replace("_", " ").title()

    @staticmethod
    def _pick_image(man, d, rendering, scale):
        want = [im for im in man.get("images", [])
                if im.get("rendering") == rendering and im.get("scale") == scale]
        if not want:
            sys.exit(f"atlas has no {rendering} image at {scale}x; it has "
                     + ", ".join(f"{im['rendering']} {im['scale']}x"
                                 for im in man.get("images", [])))
        p = want[0]["path"]
        # The manifest's paths are the atlas's own publication layout. A local
        # copy may be flattened, so try the stated path, the flattened form and
        # the bare name before giving up — and say which was tried.
        cands = [os.path.join(d, p),
                 os.path.join(d, p.replace("/", "_")),
                 os.path.join(d, os.path.basename(p))]
        cands += [os.path.join(d, f"{os.path.basename(d.rstrip('/'))}_{p.replace('/', '_')}")]
        stem = p.replace("/", "_")
        cands += [os.path.join(d, n) for n in sorted(os.listdir(d))
                  if n.endswith(stem)]
        for c in cands:
            if os.path.exists(c):
                return c, want[0]["scale"]
        sys.exit("atlas image not found; tried:\n  " + "\n  ".join(cands))

    def holds(self, map_id, x, y):
        """Is this tile inside the map it claims to be on?

        **A warp changes `wCurMap` one frame before it changes the
        coordinates**, so on that frame a track says the destination map and
        the *departure's* door tile. Measured on this chain: every one of the
        600 agents carries exactly one row reading `OAKS_LAB (12,11)`, and
        (12,11) is Pallet Town's own `warp_event 12,11, OAKS_LAB, 2` — Oak's
        Lab is ten tiles wide. It is the same trap as the boot-time one
        `chain_tracks.py::settle` handles, arriving mid-track instead of at the
        head, and a hand-placed layout hid it because the stray point landed on
        top of a neighbouring sheet. The atlas has real gaps, so it shows.
        """
        t = self.tiles.get(map_id)
        return t is not None and 0 <= x < t[0] and 0 <= y < t[1]

    def place(self, map_id, x, y):
        """Pixel position of a **fractional** tile coordinate, or `None` when
        the atlas does not place this map.

        **The CENTRE of the tile, not its corner** — `rect` gives a tile's
        top-left, and an agent standing on a tile is in the middle of it.
        Returning the corner and then drawing a sprite centred on it puts every
        agent half a tile up and to the left, which is what "the sprites sit
        left of the path, straddling its edge" was. Measured against the
        cartridge, not eyeballed: see `SPRITE_LIFT`.

        `None` means unplaced and must never be drawn at the origin: the ids a
        track carries include transients the cartridge writes before a map
        header exists, and a battle has no map position at all.
        """
        o = self.origin.get(map_id)
        if o is None:
            return None
        t = self.tile_px[map_id]
        return (o[0] + (x + 0.5) * t, o[1] + (y + 0.5) * t,
                self.kind[map_id] != "outdoor")

    def open_image(self, Image):
        Image.MAX_IMAGE_PIXELS = None      # a region atlas is legitimately huge
        return Image.open(self.image_path).convert("RGB")


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
            out[n] = [(int(a), b, int(c), int(e), g == "true")
                      for a, b, c, e, g in (row for row in r if row)]
    return out


def link_at(spans, frame):
    for link, name, start, end, goal in spans:
        if start <= frame <= end:
            return link, name, goal
    return (spans[-1][0], spans[-1][1], spans[-1][4]) if spans else (0, "", False)


def tile_path(rows, holds=None):
    """The tile changes only: `(frame, map, x, y, facing)` each time the
    coordinate moves. This is what the renderer interpolates along.

    `holds(map, x, y)` drops a row whose coordinate is not on the map it names
    — the one-frame warp straddle. Dropping it rather than drawing it is what
    makes the agent hold its last real tile until the destination arrives,
    which is what a warp is: a jump, and `interpolate` already refuses to lerp
    across one.
    """
    out, dropped = [], 0
    for r in rows:
        if holds is not None and not holds(r[1], r[2], r[3]):
            dropped += 1
            continue
        key = (r[1], r[2], r[3])
        if not out or (out[-1][1], out[-1][2], out[-1][3]) != key:
            out.append((r[0], r[1], r[2], r[3], r[4]))
    return out, dropped


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


def wrap_hud(text, font, width):
    """Break `text` onto as many lines as the HUD column can hold."""
    out, line = [], ""
    for word in text.split():
        trial = f"{line} {word}".strip()
        if line and font.getbbox(trial)[2] > width:
            out.append(line)
            line = word
        else:
            line = trial
    if line:
        out.append(line)
    return out


def state_at(rows, frame):
    """The row in force at `frame`. Rows are change points, ascending."""
    lo, hi = 0, len(rows) - 1
    if frame < rows[0][0]:
        return None
    while lo < hi:
        mid = (lo + hi + 1) // 2
        if rows[mid][0] <= frame:
            lo = mid
        else:
            hi = mid - 1
    return rows[lo]


def on_screen(px, py, x0, y0, cw, ch):
    """Is any of this agent's sprite in frame?

    **Judged by the sprite, not by its centre.** A film's off-frame warning is
    a claim about what a viewer can see, and an agent one tenth of a pixel past
    the boundary is drawn, plainly, half in frame. Counting it as lost made a
    knife-edge camera position report 362 of 592 agents missing from a picture
    that showed nearly all of them — the number was wrong, not the shot.
    """
    h = TILE / 2.0
    return (x0 - h <= px <= x0 + cw + h) and (y0 - h <= py <= y0 + ch + h)


def report_motion(positions, cams, lost, n_agents, out_h, view_w, tile=TILE):
    """What the film actually does, measured — never asserted.

    Two numbers decide whether a cut is any good and both were previously read
    off a screenshot: how far an agent moves between consecutive output frames
    (a teleport is a step of most of a tile in one frame, which is what drawing
    from `wXCoord`/`wYCoord` alone produces), and how much of the swarm the
    camera is not showing. Printed on every run, dry or not.

    **The absolute `>=12 px` threshold cannot judge a fast cut, and quoting it
    as if it could is how a pace argument goes wrong.** A walking agent covers
    `16 / span * step` source pixels per output frame — 15 px at 8x and 23 px
    at 12x — so above about 6x *every walking sample* clears 12 px by
    construction and the figure saturates at "the swarm is walking". It still
    earns its place at 1-2x, where it is the teleport detector it was built as.

    What separates a teleport from a fast glide at any compression is whether
    the displacement lands on the tile grid. Drawing straight from
    `wXCoord`/`wYCoord` puts *every* moving sample on a whole-tile multiple,
    because those bytes only ever hold whole tiles; interpolating across the
    step leaves only the samples that happen to coincide. So the quantised
    fraction is scale-free, and it is the number a compression decision turns
    on. Warps are excluded — a warp really is a jump, and it is a whole-tile
    multiple, so counting them would credit the renderer for the cartridge's
    own teleports.
    """
    steps, deltas = [], []
    for a, b in zip(positions, positions[1:]):
        prev = {p[0]: (p[1], p[2]) for p in a}
        for p in b:
            q = prev.get(p[0])
            if q is not None:
                dx, dy = p[1] - q[0], p[2] - q[1]
                steps.append(math.hypot(dx, dy))
                deltas.append((dx, dy))
    if steps:
        steps.sort()
        def q(f):
            return steps[min(len(steps) - 1, int(f * len(steps)))]
        snaps = sum(1 for s in steps if s >= 12) / len(steps)
        slides = sum(1 for s in steps if s < 4) / len(steps)
        print(f"agent displacement per output frame over {len(steps)} samples, "
              f"in source pixels (a walk tile is {tile:g}):")
        print(f"  median {q(0.5):.2f}  p90 {q(0.9):.2f}  p99 {q(0.99):.2f}  "
              f"max {steps[-1]:.2f}")
        print(f"  >=12 px (a snap) {snaps*100:.2f}%   <4 px (a slide) "
              f"{slides*100:.2f}%")

        def on_grid(v):
            return abs(v / tile - round(v / tile)) * tile <= 0.25

        # A warp crosses maps and is several tiles at once; a walk is at most
        # one tile per emulated step, so anything past two tiles in a frame is
        # the cartridge teleporting, not the renderer failing to interpolate.
        walk = [(dx, dy) for dx, dy in deltas
                if 0.5 <= math.hypot(dx, dy) <= 2 * tile]
        if walk:
            grid = sum(1 for dx, dy in walk if on_grid(dx) and on_grid(dy))
            print(f"  whole-tile quantisation {100*grid/len(walk):.1f}% of "
                  f"{len(walk)} moving samples land on the tile grid "
                  f"(drawn from wXCoord/wYCoord alone it is 100%)")

    off = []
    for (here, (cx, cy, cw)) in zip(positions, cams):
        ch = cw * out_h / view_w
        x0, y0 = cx - cw / 2, cy - ch / 2
        off.append(sum(1 for p in here
                       if not on_screen(p[1], p[2], x0, y0, cw, ch)))
    live = [len(h) for h in positions]
    worst = max(range(len(off)), key=lambda i: off[i]) if off else 0
    frac = [o / max(1, l) for o, l in zip(off, live)]
    print(f"camera coverage: mean {100*(1-sum(frac)/max(1,len(frac))):.1f}% of "
          f"live agents on screen, worst frame {worst} with {off[worst]} of "
          f"{live[worst]} off; {sum(lost)} agent-frame(s) unplaced by the atlas")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("tracks")
    ap.add_argument("--sprites", default="/home/aaron/pokemon-run/sprites")
    ap.add_argument("--atlas", default="/home/aaron/pokemon-run/atlas/atlas.json",
                    help="PixelGB atlas manifest. It is the ONLY source of map "
                         "placement; there is no fallback layout")
    ap.add_argument("--rendering", default="region-tint",
                    choices=("region-tint", "four-shade"))
    ap.add_argument("--atlas-scale", type=int, default=1)
    ap.add_argument("--atlas-image", default=None,
                    help="override the image the manifest names")
    ap.add_argument("--margin", type=int, default=24,
                    help="tiles of atlas kept around the maps the tracks "
                         "actually visit. The atlas is a whole region; a film "
                         "of four rooms should not carry Kanto in memory")
    ap.add_argument("--out", required=True)
    ap.add_argument("--seconds", type=float, default=70.0)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--width", type=int, default=960)
    ap.add_argument("--title", default="THE PEWTER CHAIN")
    ap.add_argument("--cohort", type=int, default=16,
                    help="how many agents carry a number. At 24 everybody can; "
                         "at 500 numbers are noise, so a named cohort stays "
                         "followable inside an anonymous crowd")
    ap.add_argument("--method", default=None,
                    help="one line of method, stated on screen. A film of a "
                         "measurement should carry the measurement.")
    ap.add_argument("--camera-trace", default=None,
                    help="write the per-frame camera to CSV and, with "
                         "--dry-run, stop there. The camera is judged by "
                         "measuring this, never by watching and asserting.")
    ap.add_argument("--dry-run", action="store_true",
                    help="compute positions and camera, write any trace, and "
                         "stop before rendering")
    ap.add_argument("--cover", type=float, default=0.6,
                    help="fraction of agents whose whole run must fit in the "
                         "film; the rest are simply still going when it ends")
    a = ap.parse_args()
    from PIL import Image, ImageDraw

    tracks = load_tracks(a.tracks)
    if not tracks:
        sys.exit(f"no agent-*.csv in {a.tracks}")
    atlas = Atlas(a.atlas, a.atlas_image, a.rendering, a.atlas_scale)
    paths, straddles = {}, 0
    for n, r in list(tracks.items()):
        p, d = tile_path(r, atlas.holds)
        straddles += d
        if p:
            paths[n] = p
        else:
            # Every row named a map the atlas cannot vouch for. Dropping the
            # agent is right, and saying so is the point of the count above.
            del tracks[n]
    span = walk_span(paths)
    spans = load_links(a.tracks)
    n_links = max((lk for v in spans.values() for lk, *_ in v), default=0)
    cohort = set(sorted(tracks)[: a.cohort])
    # **The timeline is not the longest agent.** A few agents stall and pace a
    # patch of grass for half a million frames, and letting them set the length
    # compresses the journey every other agent made into the first seconds.
    # Cover most agents' whole run; the rest are still going when it ends, and
    # the caption says so.
    ends = sorted(r[-1][0] for r in tracks.values())
    last = ends[min(len(ends) - 1, int(a.cover * len(ends)))]
    frames_out = int(a.seconds * a.fps)
    # **Sampling derived, not picked**: cover the whole run in the length
    # wanted, and say what that ratio came out at.
    # **Round the stride UP, or the film stops short of its own subject.**
    # Flooring gives step 1 for any length past half the run, and the film then
    # ends at `frames_out` emulated frames with the journey unfinished — a
    # 2,310-frame cut of a 4,122-frame chain, silently missing the last 44%
    # including the cutscene it exists to show. Measured on this chain.
    step = max(1, -(-last // max(1, frames_out)))
    print(f"{len(tracks)} agents; longest run {ends[-1]} frames, filming to "
          f"{last} ({last/FPS_GB/60:.1f} emulated minutes) so "
          f"{int(a.cover*100)}% of runs finish inside the film")
    print(f"{frames_out} output frames at {a.fps} fps -> every {step}th emulated "
          f"frame, a {step*a.fps/FPS_GB:.0f}x timelapse")
    # How fast an agent walks, in world pixels per OUTPUT frame. The camera is
    # tuned against this rather than against a pixel count, so every constant
    # below survives a change of compression.
    walk_px = 16.0 / span * step
    print(f"glide span {span} emulated frames, measured from these tracks "
          f"(a walking agent therefore covers {walk_px:.1f} source px "
          f"per output frame)")

    # --- the world, read out of PixelGB's atlas ----------------------------
    visited = {r[1] for rows in tracks.values() for r in rows}
    placed = {m for m in visited if m in atlas.origin}
    unplaced = sorted(visited - placed)
    if not placed:
        sys.exit(f"the atlas places none of the maps these tracks visit: "
                 f"{[hex(m) for m in unplaced]}")

    # **Carry only as much atlas as the tracks reach.** It is a whole region —
    # 7520x8048 here — and a film of four rooms has no business holding Kanto
    # in memory or letting the camera's clamp be set by geography nobody
    # visits. The window is the union of the visited maps plus a stated margin,
    # so what is dropped is dropped deliberately and is printed.
    pad = a.margin * TILE * atlas.mul
    X0 = int(max(0, min(atlas.origin[m][0] for m in placed) - pad))
    Y0 = int(max(0, min(atlas.origin[m][1] for m in placed) - pad))
    X1 = int(max(atlas.origin[m][0] + atlas.size[m][0] for m in placed) + pad)
    Y1 = int(max(atlas.origin[m][1] + atlas.size[m][1] for m in placed) + pad)
    full = atlas.open_image(Image)
    X1, Y1 = min(X1, full.width), min(Y1, full.height)
    world = full.crop((X0, Y0, X1, Y1))
    full.close()
    print(f"atlas {os.path.basename(atlas.image_path)} "
          f"({a.rendering} {atlas.scale}x, {atlas.cartridge.get('title','?')}); "
          f"world {world.width}x{world.height} px, the {len(placed)} map(s) "
          f"these tracks visit plus {a.margin} tiles")
    for m in sorted(placed):
        print(f"  0x{m:02X} {atlas.label[m]:24s} {atlas.kind[m]:8s} "
              f"at {int(atlas.origin[m][0])},{int(atlas.origin[m][1])} "
              f"tile {atlas.tile_px[m]:g}px")
    if unplaced:
        print(f"  UNPLACED, drawn nowhere: {[hex(m) for m in unplaced]}")
    if straddles:
        print(f"{straddles} row(s) named a map their coordinate is not on — the "
              f"one frame a warp has changed wCurMap and not wXCoord/wYCoord. "
              f"Held at the last real tile, not drawn in the gap.")

    # A real font, so the caption is legible at 1280 rather than six pixels
    # tall. Falls back to PIL's bitmap font when none is installed.
    def load_font(px):
        from PIL import ImageFont
        for f in ("/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Bold.ttf",
                  "/usr/share/fonts/google-noto-vf/NotoSansMono[wght].ttf"):
            if os.path.exists(f):
                try:
                    return ImageFont.truetype(f, px)
                except Exception:
                    pass
        return ImageFont.load_default()

    sprites = {}
    for f in os.listdir(a.sprites):
        for k in FACING.values():
            if f.startswith(f"player-{k}-"):
                sprites[k] = Image.open(os.path.join(a.sprites, f)).convert("RGBA")
    if not sprites:
        sys.exit(f"no player-*.png in {a.sprites}")

    os.makedirs(a.out, exist_ok=True)
    OUT_W, OUT_H = a.width, int(a.width * 9 / 16)
    HUD_W = max(210, OUT_W // 5)
    VIEW_W = OUT_W - HUD_W
    font = load_font(max(12, OUT_W // 62))
    num_font = load_font(max(10, OUT_W // 90))
    tints = [(255, 96, 96), (96, 200, 255), (140, 255, 140), (255, 220, 90),
             (220, 140, 255), (255, 160, 60), (120, 255, 220), (255, 120, 190)]

    # --- pass zero: the visitation count, which is the colour scale ---------
    # Weighted by frames, exactly as `corpus_report.py` weights its heat maps:
    # a tile the swarm hesitates on is hotter than one it walks through.
    visits = collections.Counter()
    for n, rows in tracks.items():
        for r0, r1 in zip(rows, rows[1:]):
            if atlas.holds(r0[1], r0[2], r0[3]):
                visits[(r0[1], r0[2], r0[3])] += max(1, r1[0] - r0[0])
    hottest = max(visits.values()) if visits else 1
    print(f"{len(visits)} distinct tiles visited; hottest carries "
          f"{hottest} agent-frames")

    def heat_band(map_id, x, y):
        """Which of the fourteen colour bands this tile falls in. Log-scaled,
        because visitation is heavy-tailed — a linear scale puts everything
        except Pallet Town in the first band."""
        import math
        v = visits.get((map_id, x, y), 0)
        if v <= 0:
            return 0
        t = math.log1p(v) / math.log1p(hottest)
        return min(13, int((1.0 - t) * 14))

    # --- pass one: where everybody is, per output frame --------------------
    # `lost[i]` is how many live agents the atlas could not place on frame `i`.
    # Counted rather than dropped silently, for the same reason the off-frame
    # line exists: a tidy crowd must never hide anyone.
    positions, lost = [], []
    for i in range(frames_out):
        gbf = max(1, i * step)
        here, gone = [], 0
        for n in sorted(tracks):
            if gbf > tracks[n][-1][0]:
                continue
            st = state_at(tracks[n], gbf)
            if st is None:
                continue
            battle = st[6]
            m, x, y, facing = interpolate(paths[n], gbf, span)
            wp = atlas.place(m, x, y)
            if wp is None:
                gone += 1
                continue
            lk, lname, _ = link_at(spans.get(n, []), gbf)
            here.append((n, wp[0] - X0, wp[1] - Y0, wp[2], facing, battle, lk,
                         lname, heat_band(m, round(x), round(y)), m))
        positions.append(here)
        lost.append(gone)

    # --- the camera ---------------------------------------------------------
    # **It follows the majority, and the certification gate is what makes that
    # the right shot**: a link is not left behind until nearly every agent
    # clears it, so the mass is guaranteed to advance and the story is wherever
    # the mass is. No hand-authored shot list.
    #
    # Median rather than mean, and the interquartile spread rather than the
    # full extent — a handful of stragglers left in Pallet Town would drag a
    # mean backwards and park the camera in an empty town, and one lost agent
    # would force a permanent zoom-out. Outliers are allowed to leave frame,
    # **and the HUD says how many did**, which is the condition for this being
    # honest rather than flattering.
    MIN_W = 26 * TILE

    def spread(vals):
        vals = sorted(vals)
        n = len(vals)
        lo = vals[max(0, int(0.05 * n))]
        hi = vals[min(n - 1, int(0.95 * n))]
        return lo, hi, (lo + hi) / 2

    # **The map is the focus element: the world is steady and the camera
    # drifts over it.** Reported as "it basically vibrates right now ... move
    # slowly up/down with the mass, not vibrate up and down / side to side
    # rapidly", and every one of the five causes below is a standard one.
    #
    # 1. THE TARGET IS SMOOTHED, and slowly. The old follow was an EMA at
    #    k = 0.14, i.e. a time constant of about seven frames — a quarter of a
    #    second at 30 fps, which is fast enough to reproduce most of the
    #    per-frame jitter of the median it was chasing. `CAM_K` is a ~1.2 s
    #    constant instead, so the camera lags the mass and glides.
    # 2. A DEADZONE. Below it the camera does not move at all, so the pixel or
    #    two the median wanders every frame produces no movement whatever.
    # 3. THE ZOOM IS QUANTISED AND HELD. This is the one that is easy to miss
    #    and was the loudest: `scale = VIEW_W / cw` was recomputed from a
    #    continuously-varying width, so *every* frame resampled the pixel art
    #    at a slightly different ratio and the whole picture shimmered — not
    #    only the moving part. The width now snaps to a ladder and changes only
    #    when it must (hysteresis), so a run of frames shares one exact scale.
    # 4. THE PAN SPEED IS CAPPED, so nothing the mass does can make the camera
    #    lurch.
    # 5. AXIS BIAS. Route 1 is a vertical corridor; the shot wants Y tracked
    #    and X nearly fixed. Chasing both axes is what turns a corridor walk
    #    into side-to-side sway.
    # 6. CATCH-UP, KEYED ON THE MASS BEING OUT OF FRAME — not on time, not on a
    #    threshold on the centre error. Every constant in 1-5 is calibrated for
    #    *jitter*, and when the swarm genuinely walks somewhere else they are
    #    calibrated for the wrong thing: measured on this chain, the deadzone
    #    plus a 1.6 px/frame cap plus a 0.22 X bias left the camera **78.7%**
    #    covered and one frame with 362 of 592 agents off screen, for hundreds
    #    of frames at a time, while the picture itself was perfectly steady.
    #    A steady shot of the wrong place is not a good shot.
    #
    #    `outside` is how far the 5-95 box pokes past the current view, in
    #    viewports. It is **exactly zero** whenever the swarm is already on
    #    screen, so this is provably a no-op on every frame that was already
    #    right, and rules 1-5 keep those frames bit-identical.
    CAM_K = 0.028          # EMA per frame: tau ~= 36 frames ~= 1.2 s at 30 fps
    CAM_DEAD = 0.16        # of the viewport, per axis, before anything moves
    # **The pan cap is a fraction of WALKING PACE, not a pixel count.** It was
    # 1.6 world px per output frame, which is a glide at 1x and a crawl at 9x —
    # at 9x each output frame covers nine times the ground, so a cap tuned at
    # 1x cannot keep up with the subject and the coverage fell to 92.5%. Tying
    # it to how fast an agent actually walks on screen makes it scale with the
    # compression for free, and at 1x it reproduces the tuned 1.6 exactly.
    CAM_MAX_PAN = 0.85 * walk_px
    CAM_X_TRACK = 0.22     # how much of the X drift to follow at all
    CAM_STEP = 1.10        # zoom ladder ratio
    CAM_GROW = 1.06        # widen only when the mass needs this much more
    CAM_SHRINK = 0.74      # tighten only when it needs this much less
    CAM_CATCHUP = 14.0     # how much faster it may glide when the mass is out
    CAM_UNBIAS = 4.0       # how fast the X bias is released when it is out
    # A margin (6) measures against, so the mass is kept *comfortably* inside
    # rather than exactly on the edge. Without it the camera settles with a
    # cluster balanced on the boundary — measured on this chain, seventy-one
    # agents sat 0.15 px outside the left edge for hundreds of frames, which is
    # a stable equilibrium and looks exactly like a botched shot.
    CAM_INSET = 0.06

    def quantise(w):
        """Snap a wanted width onto a fixed geometric ladder.

        Anchored on MIN_W rather than on the frame, so the same wanted width
        always yields the same exact scale — which is the property that stops
        the resample ratio moving under the picture.
        """
        k = max(0, round(math.log(max(w, MIN_W) / MIN_W) / math.log(CAM_STEP)))
        return MIN_W * (CAM_STEP ** k)

    cams, prev = [], None
    box = None
    for here in positions:
        if here:
            lx, hx_, mx = spread([p[1] for p in here])
            ly, hy_, my = spread([p[2] for p in here])
            tx, ty = mx, my
            box = (lx, hx_, ly, hy_)
            want = max(MIN_W, (hx_ - lx) * 1.25,
                       (hy_ - ly) * 1.25 * VIEW_W / OUT_H)
        elif prev:
            tx, ty, want = prev
        else:
            tx, ty, want = world.width / 2, world.height / 2, MIN_W
        want = min(want, world.width * 1.0)

        if prev is None:
            prev = (float(tx), float(ty), quantise(want))
            cams.append((round(prev[0]), round(prev[1]), prev[2]))
            continue

        px, py, pw = prev
        # (3) hold the zoom unless the mass genuinely outgrew or fell well
        # inside it, then snap to the ladder.
        cw = pw
        if want > pw * CAM_GROW or want < pw * CAM_SHRINK:
            cw = quantise(want)
        cw = min(cw, world.width * 1.0)
        chh = cw * OUT_H / VIEW_W

        # (6) how far the mass pokes past the frame it is being shown in.
        # Measured against the view the camera has *now*, before it moves.
        outside = 0.0
        if box:
            ix, iy = cw * (0.5 - CAM_INSET), chh * (0.5 - CAM_INSET)
            outside = max(
                0.0,
                (px - ix - box[0]) / cw, (box[1] - px - ix) / cw,
                (py - iy - box[2]) / chh, (box[3] - py - iy) / chh,
            )

        # (2) deadzone, per axis, in world pixels — but only while the mass is
        # inside the frame. A deadzone that holds a camera still while the
        # swarm is off screen is not suppressing jitter, it is refusing to look.
        dx = tx - px
        dy = ty - py
        if outside <= 0.0:
            if abs(dx) < cw * CAM_DEAD:
                dx = 0.0
            if abs(dy) < chh * CAM_DEAD:
                dy = 0.0
        # (5) axis bias: the corridor is vertical, so X barely moves — until
        # (6) says the mass has left the frame sideways, which a corridor's
        # never does.
        dx *= min(1.0, CAM_X_TRACK + CAM_UNBIAS * outside)
        # (1) smooth, then (4) cap — both opened up by (6). **The gain has to
        # rise, not only the cap**: `CAM_K` is 2.8% of the error per frame, so
        # a bigger cap alone changes nothing while the EMA is the binding
        # constraint. `gain` is 1.0 whenever the mass is in frame, so this is
        # still a no-op on every frame that was already right.
        gain = 1.0 + CAM_CATCHUP * outside
        cap = CAM_MAX_PAN * gain
        vx = max(-cap, min(cap, dx * CAM_K * gain))
        vy = max(-cap, min(cap, dy * CAM_K * gain))
        # **The accumulator stays a float and only the OUTPUT is rounded.**
        # Rounding the accumulator instead looks like the same thing and is a
        # deadlock: a drift just outside the deadzone yields a step well under
        # half a pixel, `round` throws it away, and the camera is stuck there
        # for ever — visibly, since the mass keeps walking out of frame.
        prev = (px + vx, py + vy, cw)
        # (3, again) whole world pixels, so the crop lands on the same lattice
        # every frame and a held camera is bit-identical frame to frame.
        cams.append((round(prev[0]), round(prev[1]), cw))

    if a.camera_trace:
        with open(a.camera_trace, "w") as fh:
            fh.write("frame,cx,cy,cw\n")
            for i, (cx, cy, cw) in enumerate(cams):
                fh.write(f"{i},{cx},{cy},{cw:.4f}\n")
        print(f"camera trace: {a.camera_trace} ({len(cams)} frames)")

    # The grid a displacement is measured against is the atlas's own, not this
    # file's constant — an atlas published at a different scale draws a walk
    # tile at a different number of world pixels.
    report_motion(positions, cams, lost, len(tracks), OUT_H, VIEW_W,
                  tile=TILE * atlas.mul)
    if a.dry_run:
        return

    tinted = {}
    for i, (here, (cx, cy, cw)) in enumerate(zip(positions, cams)):
        ch = cw * OUT_H / VIEW_W
        # **Clamp the crop to the world.** Reported as an interior panel
        # running off the left edge with the agents inside it cut in half:
        # the camera centres on the mass, the mass is inside a panel in the
        # gutter, and an unclamped crop then hangs past the world's edge and
        # takes the panel's outer half with it. Clamping pulls the view back
        # inside, which shows the whole panel and costs nothing anywhere else.
        x0 = int(round(cx - cw / 2))
        y0 = int(round(cy - ch / 2))
        if cw < world.width:
            x0 = max(0, min(x0, world.width - int(round(cw))))
        if ch < world.height:
            y0 = max(0, min(y0, world.height - int(round(ch))))
        # `crop` takes only the region, rather than pasting a whole region
        # atlas behind a small window. Out-of-range is legal and pads with
        # black, which is what the clamp above is there to avoid anyway.
        crop = world.crop((x0, y0, x0 + int(round(cw)), y0 + int(round(ch))))
        scale = VIEW_W / cw
        img = Image.new("RGB", (OUT_W, OUT_H), (10, 12, 16))
        img.paste(crop.resize((VIEW_W, OUT_H), Image.NEAREST), (0, 0))
        d = ImageDraw.Draw(img)
        for (n, px, py, indoors, facing, battle, lk, lname, band, _m) in here:
            # The sprite is drawn at the size of the walk tile it stands on, so
            # an atlas that ever draws a map at a different tile_px puts its
            # agents at the right size without a second decision.
            sw = max(8, int(atlas.tile_px[_m] * scale))
            sx = int((px - x0) * scale)
            sy = int((py - y0) * scale)
            if not (-sw <= sx <= VIEW_W + sw and -sw <= sy <= OUT_H + sw):
                continue
            key = (FACING.get(facing, "down"), band, sw)
            sp = tinted.get(key)
            if sp is None:
                base_sp = sprites.get(key[0], next(iter(sprites.values())))
                sp = tint(base_sp, HEAT_HUE[key[1]], Image)
                sp = sp.resize((sw, sw), Image.NEAREST)
                tinted[key] = sp
            # Centred on the tile's centre and lifted a measured quarter-tile,
            # which is where the cartridge draws the player relative to the
            # tile it is standing on. See `SPRITE_LIFT`.
            lift = int(round(SPRITE_LIFT * sw))
            img.paste(sp, (sx - sw // 2, sy - sw // 2 - lift), sp)
            r = sw // 2 + 2
            # **A ring for state, a number only for the cohort.** At two dozen
            # agents everybody can carry a number; at several hundred they are
            # noise, and the density *is* the visualisation. A named cohort
            # keeps an individual followable inside a real crowd.
            if battle:
                d.ellipse([sx - r, sy - r, sx + r, sy + r],
                          outline=(255, 80, 80), width=2)
            elif n in cohort:
                d.text((sx + r + 1, sy - r - 1), str(n),
                       fill=(240, 244, 248), font=num_font,
                       stroke_width=1, stroke_fill=(0, 0, 0))

        # **The camera follows the majority; it must not hide the minority.**
        # An agent the atlas does not place is invisible for a different
        # reason, and is counted on the same line rather than vanishing.
        offscreen = [p for p in here
                     if not on_screen(p[1], p[2], x0, y0, cw, ch)]
        if offscreen or lost[i]:
            warn = f"{len(offscreen)} agent(s) off frame"
            if offscreen and spans:
                behind = min(p[6] for p in offscreen)
                warn += f", furthest behind at link {behind}"
            if lost[i]:
                warn += f"; {lost[i]} on a map the atlas does not place"
            bb = num_font.getbbox(warn)
            d.rectangle([0, OUT_H - bb[3] - 10, VIEW_W, OUT_H],
                        fill=(40, 16, 16))
            d.text((8, OUT_H - bb[3] - 6), warn, fill=(255, 190, 190),
                   font=num_font)

        # --- the HUD, in the space the crop does not use ---------------------
        gbf = max(1, i * step)
        hx = VIEW_W + 10
        d.rectangle([VIEW_W, 0, OUT_W, OUT_H], fill=(10, 12, 16))
        d.text((hx, 8), a.title, fill=(240, 244, 248), font=font)
        rows = [
            f"{len(here)}/{len(tracks)} agents live",
            f"{gbf / FPS_GB / 60:.1f} emulated minutes",
            f"{step * a.fps / FPS_GB:.0f}x timelapse",
            f"{int(cw / TILE)} tiles across",
        ]
        if a.method:
            # **Wrap it, never let the panel cut it.** It read "links
            # 01/02/03 each certified 3000" and stopped mid-number — a
            # truncated measurement is worse than none, because a reader
            # cannot tell it was cut. Same rule the status panel's
            # `fit_bar` follows: shorten deliberately or wrap, never clip.
            rows.extend(wrap_hud(a.method, num_font, OUT_W - hx - 12))
        y = 8 + int(font.size * 1.8)
        for t in rows:
            d.text((hx, y), t, fill=(170, 185, 200), font=num_font)
            y += int(num_font.size * 1.5)

        # How many agents are on each link — the shape of the chain's
        # attrition. Only on a whole-chain film; a showcase of one link would
        # draw fourteen empty bars and say nothing.
        y += 6
        if spans:
            # **The count comes from the tracks, never from a constant.** A
            # film of three links that says "link of 14" is describing a
            # different measurement from the one on screen.
            d.text((hx, y), f"link of {n_links}", fill=(200, 215, 230),
                   font=num_font)
            y += int(num_font.size * 1.7)
            per = collections.Counter(p[6] for p in here)
            top = max(per.values()) if per else 1
            bw = OUT_W - hx - 12
            for lk in range(1, n_links + 1):
                c = per.get(lk, 0)
                d.rectangle([hx, y, hx + int(bw * c / top), y + num_font.size],
                            fill=(60, 120, 190) if c else (34, 40, 48))
                d.text((hx + 2, y), f"{lk:2d}  {c}", fill=(225, 232, 240),
                       font=num_font)
                y += num_font.size + 3
        else:
            d.text((hx, y), "agents by map", fill=(200, 215, 230),
                   font=num_font)
            y += int(num_font.size * 1.7)
            per = collections.Counter(p[9] for p in here)
            top = max(per.values()) if per else 1
            bw = OUT_W - hx - 12
            for mp, c in per.most_common(8):
                d.rectangle([hx, y, hx + int(bw * c / top), y + num_font.size],
                            fill=(60, 120, 190))
                d.text((hx + 2, y), f"{atlas.label.get(mp, hex(mp))[:16]}  {c}",
                       fill=(235, 240, 245), font=num_font)
                y += num_font.size + 3

        # The colour scale itself, so the film explains its own encoding.
        y += 8
        d.text((hx, y), "colour = how trodden", fill=(200, 215, 230),
               font=num_font)
        y += int(num_font.size * 1.6)
        cwid = (OUT_W - hx - 12) / 14
        for b in range(14):
            d.rectangle([hx + b * cwid, y, hx + (b + 1) * cwid,
                         y + num_font.size], fill=HEAT_HUE[b])
        y += num_font.size + 2
        d.text((hx, y), "well-trodden        new", fill=(150, 165, 180),
               font=num_font)
        y += int(num_font.size * 1.6)

        # And where the tracked cohort actually is.
        y += 8
        d.text((hx, y), "tracked", fill=(200, 215, 230), font=num_font)
        y += int(num_font.size * 1.7)
        for (n, _, _, _, _, _, lk, lname, band, _m) in sorted(
                here, key=lambda p: p[0]):
            if n not in cohort or y > OUT_H - num_font.size * 2:
                continue
            c = HEAT_HUE[band]
            d.rectangle([hx, y + 2, hx + 6, y + num_font.size - 1], fill=c)
            label = f"L{lk:02d} {lname[3:][:14]}" if spans else \
                atlas.label.get(_m, hex(_m))[:16]
            d.text((hx + 10, y), f"{n:3d}  {label}",
                   fill=(215, 225, 235), font=num_font)
            y += num_font.size + 2
        img.save(os.path.join(a.out, f"f{i:05d}.png"))
    print(f"wrote {frames_out} frames to {a.out}")
    print("ffmpeg -y -framerate %d -i %s/f%%05d.png -c:v libopenh264 -b:v 6M out.mp4"
          % (a.fps, a.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())

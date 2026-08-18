#!/usr/bin/env python3
"""Every tile the swarm walked, drawn as a directional arrow over the atlas.

    python3 tools/pewter-chain/path_arrows.py still /home/aaron/pokemon-run/chain-film \
        --out /home/aaron/pokemon-run/pewter-path-arrows.png
    python3 tools/pewter-chain/path_arrows.py video /home/aaron/pokemon-run/chain-film \
        --out /home/aaron/pokemon-run/pewter-path-arrows --mp4 --speed 900

A movement map, not a film. `swarm_film.py` answers *where is everybody right
now*; this answers *where did anybody ever go, which way, and how much of the
swarm went there* — one arrow per walked tile, over PixelGB's map art.

## It needs no emulator run

The tracks are already on disk. `pewter_chain film` / `corpus --paths` write one
`frame,map,x,y,facing,walking,battle` row per change, and
`tools/pewter-chain/chain_tracks.py` stitches them per agent. Nothing here
emulates a frame; it reads those CSVs.

## The direction is over-determined, and that is measured

A step's direction can be read two independent ways: the `facing` byte the
cartridge held while the step animated, and the delta between the tile left and
the tile arrived at. **Over the 149,741 steps in `chain-film` the two agree on
every single one** — 100.0000%, zero disagreements (`summary` re-measures it on
whatever tracks you point it at and prints the figure). So the arrow is not a
guess dressed up: two readings of the machine say the same thing.

The delta is what is *used*, because it is the one that cannot be fooled — a
`facing` row is also written when the player turns on the spot without moving,
and only the delta distinguishes turning from walking.

## What the colour is, and why it is not step count

**Hue and size both encode how many DISTINCT AGENTS stepped off the tile.**
That is the question this picture exists to answer — "a tile many agents
crossed must read differently from one agent's detour" — and it is the one the
existing tooling does not already answer.

Step count was the obvious alternative and it is measurably the wrong unit
here. Measured on `chain-film`: the busiest tile is `VIRIDIAN_CITY (22,28)` at
**20,529 steps**, because eight agents spent ten emulated hours pacing one band
of grass and never left. A corridor the entire swarm walked once scores 24. A
step-count ramp therefore draws the training grind as the whole map and the
route as nothing — it is a *dwell* map, which is what
`tools/pewter-chain/corpus_report.py` already draws, weighted by frames.
Distinct agents is bounded by the swarm size, so the ramp spends its whole
range on the thing being asked about.

Colour and size carry the same number on purpose. That redundancy is this
repository's own rule for its panels — every encoding is readable a second way —
and it means the picture survives being printed small, or in grey.

## A paced tile gets a double-headed arrow

A single arrow claims a *flow*. Where the two commonest directions through a
tile are opposite and comparable (the weaker at least `--paced 0.40` of the
stronger), no such flow exists: the swarm walked back and forth over it. Those
tiles are drawn with a head at each end, so a grinding band does not read as a
one-way corridor. The count is printed.
"""
import argparse
import array
import collections
import csv
import importlib.util
import json as _json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def _swarm_film():
    """`swarm_film.py` as a module, for `Atlas` and nothing else.

    The directory has a hyphen in it, so it is not a package and cannot be
    imported by name. Importing the file directly runs only its constants and
    its `def`s — `main()` is behind the usual guard — and **its own rendering
    path is not touched**: this tool is a sibling, not a fork of it.
    """
    p = os.path.join(HERE, "swarm_film.py")
    spec = importlib.util.spec_from_file_location("swarm_film", p)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


# --- AXI: TOON on stdout, errors on stdout, help lines -----------------------
# An agent drives this through a shell and does not read stderr, so an error
# sent there is an error it cannot act on. Exit codes: 0 success, 1 the work
# failed, 2 the command line was wrong.
_JSON = False
_BLOCKS = []
SELF = "python3 tools/pewter-chain/path_arrows.py"


def _cell(v):
    if v is None:
        return "-"
    if isinstance(v, float):
        return f"{v:.4g}"
    return str(v)


def table(name, rows, fields):
    if _JSON:
        _BLOCKS.append((name, [{f: r.get(f) for f in fields} for r in rows]))
        return
    print(f"{name}[{len(rows)}]{{{','.join(fields)}}}:")
    for r in rows:
        print("  " + ",".join(_cell(r.get(f)) for f in fields))


def obj(name, pairs):
    if _JSON:
        _BLOCKS.append((name, dict(pairs)))
        return
    print(f"{name}:")
    for k, v in pairs:
        print(f"  {k}: {v}")


def note(text):
    if _JSON:
        _BLOCKS.append(("note", text))
        return
    print(text)


def helps(lines):
    if _JSON or not lines:
        return
    print(f"help[{len(lines)}]:")
    for line in lines:
        print(f"  {line}")


def flush_json():
    if not _JSON:
        return
    out = {}
    for name, payload in _BLOCKS:
        if name in out and isinstance(out[name], list):
            out[name].extend(payload)
        else:
            out[name] = payload
    print(_json.dumps(out, indent=2))


def die(message, *suggestions, code=1):
    if _JSON:
        print(_json.dumps({"error": message, "help": list(suggestions)}, indent=2))
    else:
        print(f"error: {message}")
        for s in suggestions:
            print(f"help: {s}")
    sys.exit(code)


# --- the data ----------------------------------------------------------------
# `PathPoint::facing` — 0 down, 4 up, 8 left, 12 right, the game's own encoding
# (`src/scenario/run.rs`). Indexed 0..3 here so it fits an `array('B')`.
DIRS = ["down", "up", "left", "right"]
DELTA = {(0, 1): 0, (0, -1): 1, (-1, 0): 2, (1, 0): 3}
FACING_DIR = {0: 0, 4: 1, 8: 2, 12: 3}
OPPOSITE = {0: 1, 1: 0, 2: 3, 3: 2}
# Unit vector per direction, in screen axes (y grows down).
VEC = {0: (0, 1), 1: (0, -1), 2: (-1, 0), 3: (1, 0)}


class Steps:
    """Every step the swarm took, reduced as the tracks stream past.

    A step is a move between two **adjacent tiles of the same map**. Anything
    else — a map change, or a coordinate that jumps more than one tile — is a
    warp and is counted separately rather than drawn as a walk: an arrow across
    a doorway would claim the player walked through the wall beside it.

    Memory is bounded on purpose. A whole eleven-link corpus is millions of
    steps, so the per-step record for the video is four parallel `array`s
    (frame, tile index, direction, agent) rather than a list of tuples —
    about 11 bytes a step instead of a Python object each.
    """

    def __init__(self, d, holds):
        self.tile_ix = {}            # (map,x,y) -> index
        self.tiles = []              # index -> (map,x,y)
        self.dir_counts = []         # index -> [down,up,left,right]
        self.agents_on = []          # index -> set(agent)
        # **Steps are kept PER AGENT, each already in frame order.** A single
        # flat list appended agent by agent is *not* sorted by frame, and the
        # video replays it by frame — so a flat list plays agent 0's whole
        # journey, then agent 1's, and stalls the moment one long-running agent
        # is reached. Measured before it was fixed: the accumulation stopped at
        # 279 of 978 tiles because the cursor was stuck inside one grinder's
        # half-million frames and never reached the fifteen agents behind it.
        # A per-agent cursor merges them correctly and costs no sort.
        self.ev = {}                 # agent -> (frames, tiles, dirs)
        self.end = {}                # agent -> last frame
        self.end_map = {}            # agent -> last map it was placed on
        self.warps = 0
        self.offmap = 0              # rows whose coordinate is not on its map
        self.visited = set()         # every map any row was placed on
        self.facing_agrees = 0
        self.facing_steps = 0
        self.rows = 0
        self.dropped = []

        for n, p in _track_files(d).items():
            cur = (array.array("I"), array.array("I"), array.array("B"))
            prev = None
            last = None
            lastmap = None
            with open(p) as f:
                r = csv.reader(f)
                next(r, None)
                for row in r:
                    if not row:
                        continue
                    fr, m, x, y, facing, _walk, _battle = (int(v) for v in row)
                    self.rows += 1
                    if not holds(m, x, y):
                        # The one frame a warp has changed `wCurMap` and not the
                        # coordinates, plus the boot rows before a map header
                        # exists. `swarm_film.Atlas.holds` documents both.
                        self.offmap += 1
                        continue
                    last = fr
                    lastmap = m
                    self.visited.add(m)
                    if prev is not None and (prev[1], prev[2], prev[3]) != (m, x, y):
                        if prev[1] == m:
                            delta = (x - prev[2], y - prev[3])
                            d_ix = DELTA.get(delta)
                        else:
                            d_ix = None
                        if d_ix is None:
                            self.warps += 1
                        else:
                            self.facing_steps += 1
                            if FACING_DIR.get(facing) == d_ix:
                                self.facing_agrees += 1
                            self._add(cur, prev[1], prev[2], prev[3], d_ix, n,
                                      fr)
                    prev = (fr, m, x, y)
            if last is None:
                # Every row named a map the atlas cannot vouch for.
                self.dropped.append(n)
                continue
            # An agent that was placed and never walked is KEPT, with no steps.
            # It is part of the population the picture is a picture of, and
            # `n_agents` is what the colour ramp is scaled against.
            self.ev[n] = cur
            self.end[n] = last
            self.end_map[n] = lastmap

    def _add(self, cur, m, x, y, d_ix, agent, frame):
        key = (m, x, y)
        ix = self.tile_ix.get(key)
        if ix is None:
            ix = len(self.tiles)
            self.tile_ix[key] = ix
            self.tiles.append(key)
            self.dir_counts.append([0, 0, 0, 0])
            self.agents_on.append(set())
        self.dir_counts[ix][d_ix] += 1
        self.agents_on[ix].add(agent)
        cur[0].append(frame)
        cur[1].append(ix)
        cur[2].append(d_ix)

    @property
    def n_agents(self):
        return len(self.end)

    @property
    def n_steps(self):
        return sum(len(v[0]) for v in self.ev.values())

    def upto(self, frame):
        """`tile index -> set of agents` that had stepped off it by `frame`."""
        seen = collections.defaultdict(set)
        for n, (fr, tl, _d) in self.ev.items():
            i = 0
            end = len(fr)
            while i < end and fr[i] <= frame:
                seen[tl[i]].add(n)
                i += 1
        return seen

    def dominant(self, counts):
        """(direction, share of steps, paced?) for one tile's direction counts."""
        order = sorted(range(4), key=lambda i: -counts[i])
        top = order[0]
        total = sum(counts)
        other = counts[OPPOSITE[top]]
        return top, counts[top] / total, other


def _track_files(d):
    """`agent -> path`, for every track file in a directory.

    `agent-NNN.csv` from a film run or `path-NNNNN.csv` from one link's corpus;
    the format is identical. Mirrors `swarm_film.track_files`, which is a
    private helper there — a copy of five lines is cheaper than making its
    rendering path load this one.
    """
    out = {}
    if not os.path.isdir(d):
        die(f"no such tracks directory: {d}",
            f"{SELF} summary <dir>   # a dir of agent-NNN.csv or path-NNNNN.csv",
            code=2)
    for name in sorted(os.listdir(d)):
        if name.endswith(".links.csv") or not name.endswith(".csv"):
            continue
        if name.startswith("agent-"):
            out[int(name[6:-4])] = os.path.join(d, name)
        elif name.startswith("path-"):
            out[int(name[5:-4])] = os.path.join(d, name)
    if not out:
        die(f"no agent-NNN.csv or path-NNNNN.csv in {d}",
            "a film run writes agent-NNN.csv; "
            "`pewter_chain corpus --paths` writes path-NNNNN.csv")
    return out


# --- colour ------------------------------------------------------------------
# **Sequential, cool to warm, and it is DISTINCT AGENTS.** Deliberately its own
# ramp rather than `swarm_film.HEAT_HUE`: that one is agent-*frames* and is read
# inverted (blue = heavily trodden, warm = novel, so an agent doing something
# new lights up). Reusing the same swatches for the opposite polarity and a
# different unit would be the worst of both. Ten bands, because a swarm of 24
# has 24 possible values and ten is as many as an eye separates in a legend.
TRAFFIC_RAMP = [
    (64, 96, 176), (62, 124, 188), (60, 152, 190), (64, 178, 176),
    (86, 196, 140), (132, 204, 100), (186, 204, 76), (226, 186, 68),
    (243, 142, 62), (248, 88, 84),
]
BG = (16, 18, 22)
PANEL = (24, 27, 33)
INK = (226, 230, 238)
DIM = (140, 148, 162)


def band_of(k, n_agents):
    """Which ramp band a distinct-agent count falls in. Linear, because the
    quantity is bounded by the swarm size and is not heavy-tailed — unlike
    dwell, which is, and which is why the heat maps log-scale."""
    if n_agents <= 1:
        return len(TRAFFIC_RAMP) - 1
    t = (k - 1) / (n_agents - 1)
    return max(0, min(len(TRAFFIC_RAMP) - 1, int(round(t * (len(TRAFFIC_RAMP) - 1)))))


def arrow_points(cx, cy, d_ix, length, width, double):
    """Polygon for one arrow, centred on (cx, cy) and pointing `d_ix`.

    Built along +y and rotated, so the four directions are the same shape
    rather than four hand-drawn ones.
    """
    half = length / 2.0
    hw = width / 2.0
    head = min(half, width * 0.85)
    shaft = hw * 0.42
    pts = [(-shaft, -half + (head if double else 0.0)),
           (-hw, -half + (head if double else 0.0)),
           (0.0, -half),
           (hw, -half + (head if double else 0.0)),
           (shaft, -half + (head if double else 0.0)),
           (shaft, half - head),
           (hw, half - head),
           (0.0, half),
           (-hw, half - head),
           (-shaft, half - head)]
    if not double:
        # A flat tail rather than a second head.
        pts = [(-shaft, -half), (shaft, -half)] + pts[5:]
    ux, uy = VEC[d_ix]
    # Explicit per-direction rotation: clearer than a matrix nobody can read.
    out = []
    for px, py in pts:
        if (ux, uy) == (0, 1):        # down
            rx, ry = px, py
        elif (ux, uy) == (0, -1):     # up
            rx, ry = -px, -py
        elif (ux, uy) == (1, 0):      # right
            rx, ry = py, -px
        else:                          # left
            rx, ry = -py, px
        out.append((cx + rx, cy + ry))
    return out


# --- the world ---------------------------------------------------------------
class World:
    """The atlas crop these tracks reach, and where a tile lands inside it."""

    def __init__(self, atlas, steps, tile_px, margin, Image, dim=1.0,
                 maps=None, mask=0.30):
        from PIL import ImageDraw
        # **The crop is every map the tracks were ON, not every map they walked
        # ACROSS.** Pewter City is the worked example: the chain's goal fires
        # the instant the player steps over the boundary, so nine agents have a
        # single row there and never take a step *off* a Pewter tile — it has
        # no arrows and it still has to be in the picture, or a map of a journey
        # to Pewter City does not contain Pewter City.
        placed = [m for m in steps.visited if m in atlas.origin]
        unplaced = [m for m in steps.visited if m not in atlas.origin]
        seen = sorted(set(placed))
        if maps:
            seen = [m for m in seen if m in maps]
            if not seen:
                die("none of the maps these tracks walk match --maps",
                    f"{SELF} summary <dir>   # lists the maps with their ids")
        self.maps = set(seen)
        self.unplaced = sorted(set(unplaced))
        tps = {atlas.tile_px[m] for m in seen}
        if len(tps) != 1:
            die("this atlas draws these maps at different tile sizes: "
                + ", ".join(f"{t:g}px" for t in sorted(tps)),
                "help: use --maps to render one tile size at a time")
        atlas_tile = tps.pop()
        self.factor = tile_px / atlas_tile
        self.tile_px = tile_px
        pad = margin * atlas_tile
        x0 = max(0, min(atlas.origin[m][0] for m in seen) - pad)
        y0 = max(0, min(atlas.origin[m][1] for m in seen) - pad)
        x1 = max(atlas.origin[m][0] + atlas.size[m][0] for m in seen) + pad
        y1 = max(atlas.origin[m][1] + atlas.size[m][1] for m in seen) + pad
        full = atlas.open_image(Image)
        x1, y1 = min(x1, full.width), min(y1, full.height)
        crop = full.crop((int(x0), int(y0), int(x1), int(y1)))
        full.close()
        self.x0, self.y0 = x0, y0
        w = max(1, int(round(crop.width * self.factor)))
        h = max(1, int(round(crop.height * self.factor)))
        # NEAREST: the art is pixel art photographed from the cartridge, and a
        # smooth filter invents shades that are not in it. Same rule the
        # frontend's own scaler follows.
        img = crop.resize((w, h), Image.NEAREST)
        if mask < 1.0:
            # **Everything the swarm never entered is dimmed, and the legend
            # says so.** The crop is a rectangle of a whole-region atlas, so it
            # necessarily contains maps nobody in this corpus set foot on —
            # Kanto's other interiors sit in the gutter between Viridian and
            # Pewter. Leaving them at full strength makes the picture mostly
            # about places the measurement has nothing to say about. This is a
            # presentation change to the MAP LAYER and it is stated on screen;
            # `--mask 1.0` leaves the art exact.
            keep = Image.new("L", img.size, int(round(mask * 255)))
            kd = ImageDraw.Draw(keep)
            for m in seen:
                ox = (atlas.origin[m][0] - x0) * self.factor
                oy = (atlas.origin[m][1] - y0) * self.factor
                kd.rectangle([ox, oy,
                              ox + atlas.size[m][0] * self.factor - 1,
                              oy + atlas.size[m][1] * self.factor - 1],
                             fill=255)
            dark = Image.new("RGB", img.size, BG)
            img = Image.composite(img.convert("RGB"), dark, keep)
        if dim < 1.0:
            # **A stated presentation change to the MAP LAYER.** The arrows have
            # to read over it; `--dim 1.0` reproduces the art exactly and is
            # what the four-shade rendering is for.
            px = img.load()
            for yy in range(img.height):
                for xx in range(img.width):
                    r, g, b = px[xx, yy][:3]
                    px[xx, yy] = (int(BG[0] + (r - BG[0]) * dim),
                                  int(BG[1] + (g - BG[1]) * dim),
                                  int(BG[2] + (b - BG[2]) * dim))
        self.image = img.convert("RGB")
        self.atlas = atlas

    def place(self, m, x, y):
        p = self.atlas.place(m, x, y)
        if p is None:
            return None
        return ((p[0] - self.x0) * self.factor, (p[1] - self.y0) * self.factor)


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


def draw_arrow(dr, world, steps, ix, k, n_agents, paced_at, alpha_floor=0.55):
    """One tile's arrow onto `dr`. Returns False when the tile is unplaced."""
    m, x, y = steps.tiles[ix]
    if m not in world.maps:
        return False
    c = world.place(m, x, y)
    if c is None:
        return False
    counts = steps.dir_counts[ix]
    top, _share, other = steps.dominant(counts)
    double = other >= paced_at * counts[top] and other > 0
    band = band_of(k, n_agents)
    col = TRAFFIC_RAMP[band]
    # **Size carries the same number as the hue.** A one-agent detour is a small
    # dim arrow and a whole-swarm corridor is a big bright one, so the contrast
    # survives grey printing and a colour-blind reader.
    # A one-agent corpus IS the whole swarm, so it sits at the top of the ramp
    # in both channels rather than reading as a lone detour.
    t = 1.0 if n_agents <= 1 else (k - 1) / (n_agents - 1)
    scale = 0.46 + 0.54 * t
    tp = world.tile_px
    length = tp * 0.94 * scale
    width = max(2.0, tp * 0.52 * scale)
    pts = arrow_points(c[0], c[1], top, length, width, double)
    outline = (10, 12, 16) if tp >= 9 else None
    dr.polygon(pts, fill=col, outline=outline)
    return True


def panel_width(font, small, title, subtitle, lines, tile_px):
    """Wide enough for the longest line there is.

    **A clipped caption is worse than a short one**, because a reader cannot
    tell it was cut — the rule `fit_bar` follows in the terminal frontend and
    the one the film compositor's HUD had to learn. So the panel is measured
    from its own contents rather than being a constant somebody has to keep in
    step with the text.
    """
    from PIL import Image, ImageDraw
    probe = ImageDraw.Draw(Image.new("RGB", (8, 8)))
    w = probe.textlength(title, font=font)
    for t in list(subtitle) + [t for t, _c in lines]:
        w = max(w, probe.textlength(t, font=small))
    # The ramp is drawn at a fixed swatch size, so it has a floor of its own.
    w = max(w, len(TRAFFIC_RAMP) * max(10, int(tile_px * 0.9)))
    return int(w + tile_px * 2.6)


def legend(img, dr, x, y, w, steps, n_agents, tile_px, title, subtitle, lines,
           font, small):
    dr.rectangle([x, 0, x + w, img.height], fill=PANEL)
    pad = int(tile_px * 1.2)
    cx = x + pad
    yy = pad
    dr.text((cx, yy), title, font=font, fill=INK)
    yy += int(font.size * 1.5)
    for s in subtitle:
        dr.text((cx, yy), s, font=small, fill=DIM)
        yy += int(small.size * 1.35)
    yy += int(small.size * 0.9)

    dr.text((cx, yy), "COLOUR + SIZE = agents that", font=small, fill=INK)
    yy += int(small.size * 1.3)
    dr.text((cx, yy), "stepped off this tile", font=small, fill=INK)
    yy += int(small.size * 1.7)
    sw = max(10, int((w - 2 * pad) / len(TRAFFIC_RAMP)))
    for i, col in enumerate(TRAFFIC_RAMP):
        dr.rectangle([cx + i * sw, yy, cx + (i + 1) * sw - 2, yy + sw],
                     fill=col)
    yy += sw + int(small.size * 0.4)
    dr.text((cx, yy), "1", font=small, fill=DIM)
    lab = f"{n_agents}"
    dr.text((cx + len(TRAFFIC_RAMP) * sw - small.size * len(lab) * 0.62, yy),
            lab, font=small, fill=DIM)
    yy += int(small.size * 2.0)

    # The two arrow shapes, drawn with the same code that draws the map.
    ax = cx + tile_px
    for shape, text in (("one", "one-way"), ("two", "walked both ways")):
        pts = arrow_points(ax, yy + tile_px * 0.5, 3, tile_px * 1.6,
                           tile_px * 0.8, shape == "two")
        dr.polygon(pts, fill=TRAFFIC_RAMP[-3], outline=(10, 12, 16))
        dr.text((ax + tile_px * 1.6, yy + tile_px * 0.12), text,
                font=small, fill=INK)
        yy += int(tile_px * 1.7)
    yy += int(small.size * 1.0)

    for line, colour in lines:
        dr.text((cx, yy), line, font=small, fill=colour)
        yy += int(small.size * 1.35)
    return yy


def endings(steps, atlas):
    """Where each agent's track stops, grouped. **This is the survivorship
    statement**, and it is the first thing the docs and the legend say: an
    arrow map drawn from tracks that end in different places is a map of
    several different populations laid on top of each other."""
    g = collections.Counter()
    for n, m in steps.end_map.items():
        g[m] += 1
    return [(m, c, atlas.label.get(m, f"0x{m:02X}"))
            for m, c in sorted(g.items(), key=lambda kv: -kv[1])]


def build(a, want_video):
    from PIL import Image, ImageDraw
    sf = _swarm_film()
    if not os.path.exists(a.atlas):
        die(f"no atlas manifest at {a.atlas}",
            f"{SELF} {'video' if want_video else 'still'} <dir> --atlas <atlas.json>",
            code=2)
    atlas = sf.Atlas(a.atlas, a.atlas_image, a.rendering, a.atlas_scale)
    steps = Steps(a.tracks, atlas.holds)
    if not steps.n_steps:
        die(f"no walked steps in {a.tracks}",
            "help: every row named a map the atlas does not place, or nobody moved")
    maps = None
    if a.maps:
        maps = {int(t, 0) for t in a.maps.split(",")}
    world = World(atlas, steps, a.tile_px, a.margin, Image, a.dim, maps,
                  a.mask)
    return sf, atlas, steps, world, Image, ImageDraw


def hud_lines(steps, world, atlas, drawn, extra=()):
    lines = []
    for m, c, lab in endings(steps, atlas):
        mark = "" if m in world.maps else "  (off this crop)"
        lines.append((f"{c:>3} stopped in {lab}{mark}", DIM))
    return list(extra) + lines


# --- commands ----------------------------------------------------------------
def cmd_summary(a):
    from PIL import Image  # noqa: F401  (fails early and clearly if absent)
    sf = _swarm_film()
    atlas = sf.Atlas(a.atlas, a.atlas_image, a.rendering, a.atlas_scale)
    steps = Steps(a.tracks, atlas.holds)
    n = steps.n_agents
    if not steps.n_steps:
        note(f"{a.tracks}: {n} track(s), 0 walked steps — nobody moved on a map "
             f"the atlas places")
        helps([f"{SELF} summary <dir>   # try another tracks directory"])
        return 0
    agree = steps.facing_agrees / steps.facing_steps
    per_tile = sorted(len(s) for s in steps.agents_on)
    paced = 0
    for c in steps.dir_counts:
        top, _s, other = steps.dominant(c)
        if other >= a.paced * c[top] and other > 0:
            paced += 1
    obj("tracks", [
        ("dir", a.tracks),
        ("agents", n),
        ("rows read", steps.rows),
        ("walked steps", steps.n_steps),
        ("walked tiles", len(steps.tiles)),
        ("warps and map changes", steps.warps),
        ("rows off their own map", steps.offmap),
        ("last emulated frame", max(steps.end.values())),
    ])
    obj("direction", [
        ("facing agrees with the tile delta",
         f"{steps.facing_agrees}/{steps.facing_steps} = {agree*100:.4f}%"),
        ("read from", "the tile delta; facing is the cross-check"),
        ("tiles walked both ways", f"{paced} ({paced/len(steps.tiles)*100:.1f}%) "
                                   f"at --paced {a.paced}"),
    ])
    obj("traffic", [
        ("busiest tile, by agents", f"{max(per_tile)} of {n}"),
        ("median agents per tile", per_tile[len(per_tile) // 2]),
        ("tiles walked by exactly one agent",
         sum(1 for v in per_tile if v == 1)),
        ("busiest tile, by steps", max(sum(c) for c in steps.dir_counts)),
    ])
    rows = []
    by_map = collections.defaultdict(lambda: [0, 0, set()])
    for ix, (m, _x, _y) in enumerate(steps.tiles):
        e = by_map[m]
        e[0] += 1
        e[1] += sum(steps.dir_counts[ix])
        e[2] |= steps.agents_on[ix]
    for m, (t, s, ags) in sorted(by_map.items(), key=lambda kv: -kv[1][1]):
        rows.append({"map": f"0x{m:02X}", "name": atlas.label.get(m, "UNPLACED"),
                     "tiles": t, "steps": s, "agents": len(ags),
                     "placed": "yes" if m in atlas.origin else "NO"})
    table("maps", rows, ["map", "name", "tiles", "steps", "agents", "placed"])
    table("endings", [{"map": f"0x{m:02X}", "name": lab, "agents": c}
                      for m, c, lab in endings(steps, atlas)],
          ["map", "name", "agents"])
    note("endings is the survivorship statement: agents stop where they failed "
         "a link, so the arrows are several populations laid on one map.")
    helps([
        f"{SELF} still {a.tracks} --out /home/aaron/pokemon-run/pewter-path-arrows.png",
        f"{SELF} video {a.tracks} --out /home/aaron/pokemon-run/pewter-path-arrows --mp4",
        f"{SELF} summary {a.tracks} --json   # the same figures, parsed",
    ])
    return 0


def render_still(a, steps, world, atlas, Image, ImageDraw, upto=None):
    """One composite. `upto` limits it to steps taken at or before that
    emulated frame, which is what makes the video an accumulation of the
    still rather than a different picture."""
    tp = world.tile_px
    if upto is None:
        counts = {ix: len(s) for ix, s in enumerate(steps.agents_on)}
        n_steps = steps.n_steps
    else:
        seen = steps.upto(upto)
        n_steps = sum(len(v) for v in seen.values())
        counts = {ix: len(s) for ix, s in seen.items()}

    # **Every figure on the panel is a figure of what is DRAWN.** With --maps
    # the arrows are a subset, and reporting the whole corpus's step count
    # beside a filtered tile count is two different populations in one line.
    drawn_ix = [ix for ix in counts
                if steps.tiles[ix][0] in world.maps
                and world.place(*steps.tiles[ix]) is not None]
    if upto is None:
        n_steps = sum(sum(steps.dir_counts[ix]) for ix in drawn_ix)

    font = load_font(max(11, int(tp * 1.5)))
    small = load_font(max(9, int(tp * 1.05)))
    sub = [f"{steps.n_agents} agents from {os.path.basename(a.tracks.rstrip('/'))}",
           f"{len(drawn_ix)} tiles drawn, {n_steps} steps"]
    lines = hud_lines(steps, world, atlas, len(drawn_ix), extra=[("", INK)])
    lines.append(("", INK))
    lines.append(("direction from the tile delta;", DIM))
    lines.append((f"facing agrees on "
                  f"{steps.facing_agrees/max(1,steps.facing_steps)*100:.2f}% of steps",
                  DIM))
    lines.append((f"map art: PixelGB, {a.rendering};", DIM))
    lines.append(("maps nobody entered are dimmed", DIM))
    panel_w = panel_width(font, small, a.title, sub, lines, tp)

    img = Image.new("RGB", (world.image.width + panel_w, world.image.height), BG)
    img.paste(world.image, (0, 0))
    dr = ImageDraw.Draw(img)
    drawn = 0
    for ix, k in sorted(counts.items(), key=lambda kv: kv[1]):
        if draw_arrow(dr, world, steps, ix, k, steps.n_agents, a.paced):
            drawn += 1
    legend(img, dr, world.image.width, 0, panel_w, steps, steps.n_agents, tp,
           a.title, sub, lines, font, small)
    return img, len(drawn_ix), n_steps


def cmd_still(a):
    sf, atlas, steps, world, Image, ImageDraw = build(a, False)
    img, drawn, drawn_steps = render_still(a, steps, world, atlas, Image,
                                           ImageDraw)
    os.makedirs(os.path.dirname(os.path.abspath(a.out)) or ".", exist_ok=True)
    img.save(a.out)
    obj("still", [
        ("out", a.out),
        ("size", f"{img.width}x{img.height}"),
        ("agents", steps.n_agents),
        ("tiles walked", len(steps.tiles)),
        ("tiles drawn", drawn),
        ("steps on the tiles drawn", drawn_steps),
        ("steps in the corpus", steps.n_steps),
        ("maps drawn", len(world.maps)),
    ])
    if world.unplaced:
        note("UNPLACED, drawn nowhere: "
             + ", ".join(f"0x{m:02X}" for m in world.unplaced))
    table("endings", [{"map": f"0x{m:02X}", "name": lab, "agents": c}
                      for m, c, lab in endings(steps, atlas)],
          ["map", "name", "agents"])
    helps([
        f"{SELF} video {a.tracks} --out {os.path.splitext(a.out)[0]} --mp4",
        f"{SELF} still {a.tracks} --maps 0x0C,0x01 --out <png>   # one stretch",
        f"{SELF} summary {a.tracks}",
    ])
    return 0


def cmd_video(a):
    sf, atlas, steps, world, Image, ImageDraw = build(a, True)
    last = max(steps.end.values())
    fps_gb = sf.FPS_GB
    if a.speed:
        stride = max(1, int(round(a.speed * fps_gb / a.fps)))
        frames_out = -(-last // stride)
    else:
        frames_out = max(1, int(a.seconds * a.fps))
        stride = max(1, -(-last // frames_out))
    frames_out = max(1, -(-last // stride))
    tp = world.tile_px
    font = load_font(max(11, int(tp * 1.5)))
    small = load_font(max(9, int(tp * 1.05)))
    # **Size the panel from the WIDEST line the film will ever hold**, not from
    # the first frame's: the counters only grow, so a panel measured at frame 0
    # clips its own numbers by the end. The widest values are the totals, which
    # are already known.
    widest_sub = [f"{steps.n_agents} agents from "
                  f"{os.path.basename(a.tracks.rstrip('/'))}",
                  f"{last/sf.FPS_GB/60:6.1f} emulated minutes",
                  f"{len(steps.tiles)} tiles lit, {steps.n_steps} steps",
                  f"{steps.n_agents} of {steps.n_agents} still running"]
    widest_lines = hud_lines(steps, world, atlas, 0, extra=[("", INK)])
    widest_lines += [("", INK), ("9999x timelapse", DIM),
                     (f"map art: PixelGB, {a.rendering};", DIM),
                     ("maps nobody entered are dimmed", DIM)]
    panel_w = panel_width(font, small, a.title, widest_sub, widest_lines, tp)
    W = world.image.width + panel_w
    H = world.image.height
    # h264 wants even dimensions.
    W += W & 1
    H += H & 1

    os.makedirs(a.out, exist_ok=True)
    for f in os.listdir(a.out):
        if f.endswith(".png"):
            os.remove(os.path.join(a.out, f))

    # **Incremental.** An arrow only changes when its agent count crosses a band,
    # so the map layer is painted once and only changed tiles are repainted.
    # Redrawing every arrow every frame is O(tiles x frames) and a whole-corpus
    # run has a hundred thousand tiles.
    canvas = Image.new("RGB", (W, H), BG)
    canvas.paste(world.image, (0, 0))
    dr = ImageDraw.Draw(canvas)
    seen = collections.defaultdict(set)
    drawn_k = {}
    cursor = {n: 0 for n in steps.ev}
    n_steps = 0
    written = 0
    live_end = sorted(steps.end.values())

    for i in range(frames_out):
        gbf = min(last, (i + 1) * stride)
        changed = set()
        for n, (fr, tl, _d) in steps.ev.items():
            # NOT `i`: that is the output-frame counter this file is named
            # after, and shadowing it wrote 355 frames with names taken from
            # an agent's cursor.
            c = cursor[n]
            end = len(fr)
            while c < end and fr[c] <= gbf:
                ix = tl[c]
                before = len(seen[ix])
                seen[ix].add(n)
                if len(seen[ix]) != before:
                    changed.add(ix)
                n_steps += 1
                c += 1
            cursor[n] = c
        for ix in changed:
            k = len(seen[ix])
            if drawn_k.get(ix) == k:
                continue
            drawn_k[ix] = k
            m, x, y = steps.tiles[ix]
            if m not in world.maps:
                continue
            c = world.place(m, x, y)
            if c is None:
                continue
            # Repaint the clean tile before the bigger arrow goes over it.
            box = (int(c[0] - tp), int(c[1] - tp),
                   int(c[0] + tp) + 1, int(c[1] + tp) + 1)
            bx = (max(0, box[0]), max(0, box[1]),
                  min(world.image.width, box[2]), min(world.image.height, box[3]))
            if bx[2] > bx[0] and bx[3] > bx[1]:
                canvas.paste(world.image.crop(bx), (bx[0], bx[1]))
            # Neighbours may overlap this rect, so redraw them too.
            for jx in _near(steps, world, ix, tp):
                kk = drawn_k.get(jx)
                if kk:
                    draw_arrow(dr, world, steps, jx, kk, steps.n_agents, a.paced)
            draw_arrow(dr, world, steps, ix, k, steps.n_agents, a.paced)

        live = sum(1 for e in live_end if e >= gbf)
        panel = Image.new("RGB", (panel_w, H), PANEL)
        pd = ImageDraw.Draw(panel)
        sub = [f"{steps.n_agents} agents from "
               f"{os.path.basename(a.tracks.rstrip('/'))}",
               f"{gbf/fps_gb/60:6.1f} emulated minutes",
               f"{len(drawn_k)} tiles lit, {n_steps} steps",
               f"{live} of {steps.n_agents} still running"]
        lines = hud_lines(steps, world, atlas, len(drawn_k), extra=[("", INK)])
        lines.append(("", INK))
        lines.append((f"{stride*a.fps/fps_gb:.0f}x timelapse", DIM))
        lines.append((f"map art: PixelGB, {a.rendering};", DIM))
        lines.append(("maps nobody entered are dimmed", DIM))
        legend(panel, pd, 0, 0, panel_w, steps, steps.n_agents, tp,
               a.title, sub, lines, font, small)
        canvas.paste(panel, (world.image.width, 0))
        canvas.save(os.path.join(a.out, f"f{i:05d}.png"))
        written += 1

    obj("video", [
        ("frames", written),
        ("size", f"{W}x{H}"),
        ("fps", a.fps),
        ("timelapse", f"{stride*a.fps/fps_gb:.0f}x"),
        ("emulated frames", last),
        ("runtime", f"{written/a.fps:.1f}s"),
        ("frames dir", a.out),
    ])
    mp4 = a.out.rstrip("/") + ".mp4"
    # **Fedora's ffmpeg has no libx264** (patent-clean build); libopenh264 is
    # the substitute and does not accept -crf. Recorded in AGENTS.md.
    cmd = ["ffmpeg", "-y", "-framerate", str(a.fps), "-i",
           os.path.join(a.out, "f%05d.png"), "-c:v", "libopenh264",
           "-b:v", a.bitrate, "-pix_fmt", "yuv420p", mp4]
    tips = []
    if a.mp4:
        r = subprocess.run(cmd, capture_output=True, text=True)
        if r.returncode:
            die("ffmpeg failed: " + (r.stderr.strip().splitlines() or ["?"])[-1],
                " ".join(cmd))
        obj("encoded", [("out", mp4),
                        ("bytes", os.path.getsize(mp4))])
    else:
        tips.append(" ".join(cmd))
    tips.append(f"{SELF} still {a.tracks} --out {a.out.rstrip('/')}.png")
    helps(tips)
    return 0


def _near(steps, world, ix, tp):
    """Tile indices whose arrow can overlap `ix`'s repaint rect."""
    m, x, y = steps.tiles[ix]
    out = []
    for dx in (-1, 0, 1):
        for dy in (-1, 0, 1):
            if dx == 0 and dy == 0:
                continue
            j = steps.tile_ix.get((m, x + dx, y + dy))
            if j is not None:
                out.append(j)
    return out


def cmd_home(a):
    note("path_arrows — every tile the swarm walked, as a directional arrow "
         "over the atlas map.")
    obj("this tool", [
        ("reads", "agent-NNN.csv / path-NNNNN.csv position tracks already on disk"),
        ("emulates", "nothing"),
        ("colour and size", "how many DISTINCT agents stepped off the tile"),
        ("arrow", "the direction of travel, from the tile delta"),
        ("double-headed", "the tile was walked both ways"),
    ])
    roots = []
    for d in ("/home/aaron/pokemon-run/chain-film",
              "/home/aaron/pokemon-run/chain-film-big",
              "target/cert"):
        if os.path.isdir(d):
            try:
                n = len([f for f in os.listdir(d)
                         if f.endswith(".csv") and not f.endswith(".links.csv")])
            except OSError:
                n = 0
            roots.append({"dir": d, "tracks": n})
    if roots:
        table("track directories found", roots, ["dir", "tracks"])
    else:
        note("no track directory found in the usual places — pass one.")
    helps([
        f"{SELF} summary <tracks-dir>",
        f"{SELF} still <tracks-dir> --out <png>",
        f"{SELF} video <tracks-dir> --out <frames-dir> --mp4",
        f"{SELF} help still",
    ])
    return 0


HELP = {
    "summary": (
        "What the tracks contain, before anything is rendered.",
        [f"{SELF} summary /home/aaron/pokemon-run/chain-film",
         f"{SELF} summary /home/aaron/pokemon-run/chain-film --json"],
        ["--paced F   how equal the two opposite directions must be before a "
         "tile counts as walked both ways (default 0.40)"],
    ),
    "still": (
        "One composite PNG: every walked tile, one arrow each.",
        [f"{SELF} still /home/aaron/pokemon-run/chain-film "
         f"--out /home/aaron/pokemon-run/pewter-path-arrows.png",
         f"{SELF} still <dir> --maps 0x0C --tile-px 20 --out <png>"],
        ["--tile-px N    output pixels per walk tile (default 12)",
         "--maps LIST    only these map ids, e.g. 0x0C,0x01",
         "--dim F        darken the map layer, 1.0 leaves the art exact",
         "--rendering    four-shade (default) or region-tint"],
    ),
    "video": (
        "PNG frames in which the arrows appear as the swarm reaches the tiles.",
        [f"{SELF} video /home/aaron/pokemon-run/chain-film "
         f"--out /home/aaron/pokemon-run/pewter-path-arrows --mp4 --speed 900"],
        ["--speed X      film at X times real time; the runtime falls out of it",
         "--seconds S    how long the film should be instead",
         "--fps N        output frame rate (default 30)",
         "--mp4          encode with ffmpeg as well as writing the frames"],
    ),
}


def cmd_help(topic):
    if topic not in HELP:
        die(f"no such command: {topic}",
            f"{SELF} help " + " | ".join(sorted(HELP)), code=2)
    what, ex, flags = HELP[topic]
    note(f"{topic}: {what}")
    table("flags", [{"flag": f} for f in flags], ["flag"])
    helps(ex)
    return 0


def main():
    global _JSON
    ap = argparse.ArgumentParser(add_help=False)
    ap.add_argument("command", nargs="?", default=None)
    ap.add_argument("tracks", nargs="?", default=None)
    ap.add_argument("--atlas", default="/home/aaron/pokemon-run/atlas/atlas.json")
    ap.add_argument("--atlas-image", default=None)
    ap.add_argument("--rendering", default="four-shade",
                    choices=("region-tint", "four-shade"))
    ap.add_argument("--atlas-scale", type=int, default=1)
    ap.add_argument("--maps", default=None)
    ap.add_argument("--margin", type=int, default=2)
    ap.add_argument("--tile-px", type=int, default=16)
    ap.add_argument("--dim", type=float, default=1.0)
    ap.add_argument("--mask", type=float, default=0.30,
                    help="how bright a map nobody in this corpus entered is drawn; 1.0 leaves the whole crop exact")
    ap.add_argument("--paced", type=float, default=0.40)
    ap.add_argument("--title", default="THE SWARM'S PATHS")
    ap.add_argument("--out", default=None)
    ap.add_argument("--fps", type=int, default=30)
    ap.add_argument("--speed", type=float, default=None)
    ap.add_argument("--seconds", type=float, default=25.0)
    ap.add_argument("--bitrate", default="4M")
    ap.add_argument("--mp4", action="store_true")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("-h", "--help", action="store_true")
    a = ap.parse_args()
    _JSON = a.json

    if a.help and not a.command:
        cmd_home(a)
        flush_json()
        return 0
    if a.command is None:
        cmd_home(a)
        flush_json()
        return 0
    if a.command == "help":
        r = cmd_help(a.tracks) if a.tracks else cmd_home(a)
        flush_json()
        return r
    if a.command not in ("summary", "still", "video"):
        die(f"no such command: {a.command}",
            f"{SELF}                    # what this is, and what tracks exist",
            f"{SELF} help summary|still|video", code=2)
    if not a.tracks:
        die(f"{a.command} needs a tracks directory",
            f"{SELF} {a.command} /home/aaron/pokemon-run/chain-film", code=2)
    if a.command in ("still", "video") and not a.out:
        die(f"{a.command} needs --out",
            f"{SELF} {a.command} {a.tracks} --out "
            + ("/home/aaron/pokemon-run/pewter-path-arrows.png"
               if a.command == "still"
               else "/home/aaron/pokemon-run/pewter-path-arrows"), code=2)
    try:
        r = {"summary": cmd_summary, "still": cmd_still,
             "video": cmd_video}[a.command](a)
    except SystemExit:
        raise
    except FileNotFoundError as e:
        die(str(e), f"{SELF} help {a.command}")
    flush_json()
    return r


if __name__ == "__main__":
    sys.exit(main())

#!/usr/bin/env python3
"""Draw many agents playing the whole Pewter chain, on one continuous world.

    python3 tools/pewter-chain/swarm_film.py <tracks-dir> --sprites <dir> \
        --maps <pixelgb-dir> --out <dir> [--seconds 70] [--fps 30]

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

## Sampling is not optional

A chain is minutes of emulated time. Compositing every frame gives a film that
plays at real time and is boring — the rule `docs/swarm-view.md` states and
which is easy to lose in a rebuild. The sampling rate is **derived from the
emulated time to cover and the length wanted**, not picked.
"""
import argparse
import collections
import csv
import math
import os
import sys

TILE = 16
FPS_GB = 59.7275

# Outdoor world layout, derived from the map headers' own `connection` lines
# (pokered data/maps/headers/*.asm) and the map sizes in blocks:
#   PalletTown  north Route1        offset  0
#   Route1      north ViridianCity  offset -5   (blocks; 10 tiles)
#   ViridianCity north Route2       offset  5
#   Route2      north PewterCity    offset -5
# Origins are in tiles, y increasing downwards, Pallet Town at (0,0).
WORLD = {
    0x00: (0, 0),        # Pallet Town      20x18
    0x0C: (0, -36),      # Route 1          20x36
    0x01: (-10, -72),    # Viridian City    40x36
    0x0D: (10, -144),    # Route 2          20x72
    0x02: (-10, -180),   # Pewter City      40x36
}
# **Interiors are laid out beside the door they belong to**, not scattered.
# The placement is a lookup rather than a search, because the cartridge carries
# the warp table: `warp_event 12,11, OAKS_LAB, 2` in
# `data/maps/objects/PalletTown.asm` says exactly which door leads where, so a
# panel can be put next to its own door and a reader can see that the
# laboratory sits beside the laboratory's front step.
#
#   interior map id -> (outdoor map, door x, door y, which side of the door)
INTERIOR = {
    0x26: (0x00, 5, 5, "left"),    # Red's house 2F   (via 1F)
    0x25: (0x00, 5, 5, "left"),    # Red's house 1F   <- Pallet Town
    0x28: (0x00, 12, 11, "right"), # Oak's Lab        <- Pallet Town
    0x2A: (0x01, 29, 19, "right"), # Viridian Mart    <- Viridian City
    0x29: (0x01, 23, 25, "left"),  # Viridian PC      <- Viridian City
    0x32: (0x0D, 3, 43, "left"),   # Forest south gate<- Route 2
    0x33: (0x0D, 3, 43, "left"),   # Viridian Forest  (through the gate)
    0x2F: (0x0D, 3, 11, "left"),   # Forest north gate<- Route 2
}
# Where a panel sits relative to its door, in tiles, and the gap it leaves.
PANEL_GAP = 2 * TILE

# Short names for the HUD; the full table lives in the agent plugin.
MAP_LABEL = {
    0x00: "Pallet Town", 0x01: "Viridian City", 0x02: "Pewter City",
    0x0C: "Route 1", 0x0D: "Route 2", 0x25: "Red's 1F", 0x26: "bedroom",
    0x28: "Oak's Lab", 0x29: "Viridian PC", 0x2A: "Viridian Mart",
    0x2F: "forest N gate", 0x32: "forest S gate", 0x33: "Viridian Forest",
}

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
# Frames a step takes, measured above. Used by the renderer's interpolation.
WALK_FRAMES = 14

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


def build_layout(sheets, maps_dir, Image):
    """Outdoor maps at their connection offsets, interiors beside their doors.

    Returns `origin[map_id] = (x, y)` in world pixels, and the sheets it
    loaded. Panels are placed left or right of their door and are never
    allowed to overlap an outdoor map, which is what keeps the layout readable
    rather than arbitrary.
    """
    origin = {m: (WORLD[m][0] * TILE, WORLD[m][1] * TILE) for m in sheets}
    ox0 = min(x for x, _ in origin.values())
    ox1 = max(origin[m][0] + sheets[m].width for m in sheets)
    for m, (host, dx, dy, side) in INTERIOR.items():
        if host not in origin:
            continue
        for name in os.listdir(maps_dir):
            if name.startswith(f"{m:03d}-") and name.endswith(".png"):
                sheets[m] = Image.open(os.path.join(maps_dir, name)).convert("RGB")
                break
        if m not in sheets:
            continue
        hx, hy = origin[host]
        w, h = sheets[m].size
        door_y = hy + dy * TILE
        if side == "left":
            x = ox0 - w - PANEL_GAP
            ox0 = x
        else:
            x = ox1 + PANEL_GAP
            ox1 = x + w
        origin[m] = (x, door_y - h // 2)
    return origin


def world_pos(origin, map_id, x, y):
    """Pixel position from a **fractional** tile coordinate. `None` if the map
    has no panel — a battle, which genuinely has no map position at all."""
    if map_id not in origin:
        return None
    ox, oy = origin[map_id]
    return ox + x * TILE, oy + y * TILE, map_id not in WORLD


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


def tile_path(rows):
    """The tile changes only: `(frame, map, x, y, facing)` each time the
    coordinate moves. This is what the renderer interpolates along."""
    out = []
    for r in rows:
        key = (r[1], r[2], r[3])
        if not out or (out[-1][1], out[-1][2], out[-1][3]) != key:
            out.append((r[0], r[1], r[2], r[3], r[4]))
    return out


def interpolate(path, frame):
    """Where an agent is, between the tile it left and the tile it is going to.

    **Renderer-side interpolation, not a memory read.** Positioning from
    `wXCoord`/`wYCoord` alone teleports by construction — the coordinate does
    not move during a walk, it snaps at the end of it — and reading the
    sub-tile offset out of WRAM only helps at sampling rates fine enough to
    catch it. Lerping between consecutive *recorded* tiles is independent of
    the sampling rate entirely, works on tracks already on disk, and is the
    same four lines Peter Whidden's own map visualiser uses
    (`pwhiddy/pokerl-map-viz`, `visualizer_live.js`).

    The one refinement: the progress fraction is taken over the **last
    `WALK_FRAMES` before the arrival** rather than over the whole gap between
    tiles, so an agent that stood still for a thousand frames and then took one
    step does exactly that, instead of drifting slowly the whole time.
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
    t = (frame - (nxt[0] - WALK_FRAMES)) / float(WALK_FRAMES)
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


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("tracks")
    ap.add_argument("--sprites", default="/home/aaron/pokemon-run/sprites")
    ap.add_argument("--maps", default="/home/aaron/pixelgb-out/four-shade/1x")
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
    paths = {n: tile_path(r) for n, r in tracks.items()}
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

    # --- the world, stitched from PixelGB's sheets -------------------------
    sheets = {}
    for m in WORLD:
        for name in os.listdir(a.maps):
            if name.startswith(f"{m:03d}-") and name.endswith(".png"):
                sheets[m] = Image.open(os.path.join(a.maps, name)).convert("RGB")
                break
    origin = build_layout(sheets, a.maps, Image)
    X0 = min(x for x, _ in origin.values())
    Y0 = min(y for _, y in origin.values())
    X1 = max(origin[m][0] + sheets[m].width for m in sheets)
    Y1 = max(origin[m][1] + sheets[m].height for m in sheets)
    world = Image.new("RGB", (X1 - X0, Y1 - Y0), BG)
    for m, im in sheets.items():
        world.paste(im, (origin[m][0] - X0, origin[m][1] - Y0))
    # **A connector per panel, from its own door to its own edge.** A panel in
    # a gutter is only "beside its door" if a reader can see which door — and
    # the cartridge answers that exactly, because the warp table in
    # `data/maps/objects/*.asm` names the tile. Drawn into the map layer so it
    # crops, pans and zooms with everything else rather than floating over it.
    cd = ImageDraw.Draw(world)
    for m, (host, dx, dy, side) in INTERIOR.items():
        if m not in sheets or host not in origin:
            continue
        hx, hy = origin[host]
        door = (hx + dx * TILE + TILE // 2 - X0, hy + dy * TILE + TILE // 2 - Y0)
        px, py = origin[m][0] - X0, origin[m][1] - Y0
        w, h = sheets[m].size
        edge = (px + w if side == "left" else px, py + h // 2)
        cd.line([edge, (door[0], edge[1]), door], fill=(64, 76, 90), width=2)
        cd.ellipse([door[0] - 4, door[1] - 4, door[0] + 4, door[1] + 4],
                   outline=(120, 140, 160), width=2)
    print(f"world {world.width}x{world.height} px from {len(sheets)} sheets "
          f"({len(sheets) - len(WORLD)} interior panels beside their own doors, "
          f"each joined to its door by a connector)")

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
    positions = []
    for i in range(frames_out):
        gbf = max(1, i * step)
        here = []
        for n in sorted(tracks):
            if gbf > tracks[n][-1][0]:
                continue
            st = state_at(tracks[n], gbf)
            if st is None:
                continue
            battle = st[6]
            m, x, y, facing = interpolate(paths[n], gbf)
            wp = world_pos(origin, m, x, y)
            if wp is None:
                continue
            lk, lname, _ = link_at(spans.get(n, []), gbf)
            here.append((n, wp[0] - X0, wp[1] - Y0, wp[2], facing, battle, lk,
                         lname, heat_band(m, round(x), round(y)), m))
        positions.append(here)

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
    CAM_K = 0.028          # EMA per frame: tau ~= 36 frames ~= 1.2 s at 30 fps
    CAM_DEAD = 0.16        # of the viewport, per axis, before anything moves
    CAM_MAX_PAN = 1.6      # world pixels per frame — a glide, never a lurch
    CAM_X_TRACK = 0.22     # how much of the X drift to follow at all
    CAM_STEP = 1.10        # zoom ladder ratio
    CAM_GROW = 1.06        # widen only when the mass needs this much more
    CAM_SHRINK = 0.74      # tighten only when it needs this much less

    def quantise(w):
        """Snap a wanted width onto a fixed geometric ladder.

        Anchored on MIN_W rather than on the frame, so the same wanted width
        always yields the same exact scale — which is the property that stops
        the resample ratio moving under the picture.
        """
        k = max(0, round(math.log(max(w, MIN_W) / MIN_W) / math.log(CAM_STEP)))
        return MIN_W * (CAM_STEP ** k)

    cams, prev = [], None
    for here in positions:
        if here:
            lx, hx_, mx = spread([p[1] for p in here])
            ly, hy_, my = spread([p[2] for p in here])
            tx, ty = mx, my
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

        # (2) deadzone, per axis, in world pixels
        dx = tx - px
        dy = ty - py
        if abs(dx) < cw * CAM_DEAD:
            dx = 0.0
        if abs(dy) < chh * CAM_DEAD:
            dy = 0.0
        # (5) axis bias: the corridor is vertical, so X barely moves
        dx *= CAM_X_TRACK
        # (1) smooth, then (4) cap
        vx = max(-CAM_MAX_PAN, min(CAM_MAX_PAN, dx * CAM_K))
        vy = max(-CAM_MAX_PAN, min(CAM_MAX_PAN, dy * CAM_K))
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
        crop = Image.new("RGB", (int(round(cw)), int(round(ch))), BG)
        crop.paste(world, (-x0, -y0))
        scale = VIEW_W / cw
        img = Image.new("RGB", (OUT_W, OUT_H), (10, 12, 16))
        img.paste(crop.resize((VIEW_W, OUT_H), Image.NEAREST), (0, 0))
        d = ImageDraw.Draw(img)
        sw = max(8, int(16 * scale))
        for (n, px, py, indoors, facing, battle, lk, lname, band, _m) in here:
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
            img.paste(sp, (sx - sw // 2, sy - sw // 2), sp)
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
        offscreen = [
            p for p in here
            if not (0 <= (p[1] - x0) * scale <= VIEW_W
                    and 0 <= (p[2] - y0) * scale <= OUT_H)
        ]
        if offscreen:
            behind = min(p[6] for p in offscreen)
            warn = f"{len(offscreen)} agent(s) off frame"
            if spans:
                warn += f", furthest behind at link {behind}"
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
                d.text((hx + 2, y), f"{MAP_LABEL.get(mp, hex(mp))[:16]}  {c}",
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
                MAP_LABEL.get(_m, hex(_m))[:16]
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

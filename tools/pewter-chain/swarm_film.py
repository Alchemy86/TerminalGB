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
INDOOR_DOOR = {         # where an agent went in, so it can be drawn dimmed
    0x28: (0x00, 12, 11),   # Oak's Lab          <- Pallet Town
    0x25: (0x00, 5, 5),     # Red's house 1F     <- Pallet Town
    0x26: (0x00, 5, 5),     # Red's house 2F
    0x2A: (0x01, 29, 19),   # Viridian Mart      <- Viridian City
    0x29: (0x01, 23, 25),   # Viridian PC        <- Viridian City
    0x32: (0x0D, 3, 43),    # Forest south gate  <- Route 2
    0x33: (0x0D, 3, 43),    # Viridian Forest
    0x2F: (0x0D, 3, 11),    # Forest north gate  <- Route 2
}
FACING = {0: "down", 4: "up", 8: "left", 12: "right"}
# `wWalkCounter` counts 7 down to 0 across a 16 px step, so a frame is 2 px.
STEP_PX = 2

BG = (16, 18, 22)


def load_tracks(d):
    out = {}
    for name in sorted(os.listdir(d)):
        if not (name.startswith("agent-") and name.endswith(".csv")):
            continue
        n = int(name[6:-4])
        with open(os.path.join(d, name)) as f:
            r = csv.reader(f)
            next(r, None)
            rows = [tuple(int(v) for v in row) for row in r if row]
        if rows:
            out[n] = rows
    return out


def world_pos(map_id, x, y, facing, walking):
    """Pixel position on the stitched outdoor world, sub-tile during a step.

    An agent inside a building is drawn at the door it went in by, so it stays
    on screen and its absence from the outdoors is legible rather than a
    disappearance.
    """
    dim = False
    if map_id in INDOOR_DOOR:
        map_id, x, y = INDOOR_DOOR[map_id]
        walking = 0
        dim = True
    if map_id not in WORLD:
        return None
    ox, oy = WORLD[map_id]
    px = (ox + x) * TILE
    py = (oy + y) * TILE
    if walking:
        # The coordinate updates at the *start* of a step, so a walking agent
        # is drawn behind its coordinate by however much of the step is left.
        back = walking * STEP_PX
        dx, dy = {"up": (0, back), "down": (0, -back),
                  "left": (back, 0), "right": (-back, 0)}[FACING.get(facing, "down")]
        px += dx
        py += dy
    return px, py, dim


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
    a = ap.parse_args()
    from PIL import Image, ImageDraw

    tracks = load_tracks(a.tracks)
    if not tracks:
        sys.exit(f"no agent-*.csv in {a.tracks}")
    last = max(r[-1][0] for r in tracks.values())
    frames_out = int(a.seconds * a.fps)
    # **Sampling derived, not picked**: cover the whole run in the length
    # wanted, and say what that ratio came out at.
    step = max(1, last // max(1, frames_out))
    print(f"{len(tracks)} agents, {last} emulated frames "
          f"({last/FPS_GB/60:.1f} emulated minutes)")
    print(f"{frames_out} output frames at {a.fps} fps -> every {step}th emulated "
          f"frame, a {step*a.fps/FPS_GB:.0f}x timelapse")

    # --- the world, stitched from PixelGB's sheets -------------------------
    sheets = {}
    for m in WORLD:
        for name in os.listdir(a.maps):
            if name.startswith(f"{m:03d}-") and name.endswith(".png"):
                sheets[m] = Image.open(os.path.join(a.maps, name)).convert("RGB")
                break
    xs = [WORLD[m][0] * TILE for m in sheets]
    ys = [WORLD[m][1] * TILE for m in sheets]
    xe = [WORLD[m][0] * TILE + sheets[m].width for m in sheets]
    ye = [WORLD[m][1] * TILE + sheets[m].height for m in sheets]
    X0, Y0, X1, Y1 = min(xs), min(ys), max(xe), max(ye)
    world = Image.new("RGB", (X1 - X0, Y1 - Y0), BG)
    for m, im in sheets.items():
        world.paste(im, (WORLD[m][0] * TILE - X0, WORLD[m][1] * TILE - Y0))
    print(f"world {world.width}x{world.height} px from {len(sheets)} sheets")

    # A real font, so the caption is legible at 1280 rather than six pixels
    # tall. Falls back to PIL's bitmap font when none is installed.
    def load_font(px):
        for f in ("/usr/share/fonts/adwaita-mono-fonts/AdwaitaMono-Bold.ttf",
                  "/usr/share/fonts/google-noto-vf/NotoSansMono[wght].ttf"):
            if os.path.exists(f):
                try:
                    from PIL import ImageFont
                    return ImageFont.truetype(f, px)
                except Exception:
                    pass
        from PIL import ImageFont
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
    font = load_font(max(12, OUT_W // 62))
    num_font = load_font(max(10, OUT_W // 90))
    tints = [(255, 96, 96), (96, 200, 255), (140, 255, 140), (255, 220, 90),
             (220, 140, 255), (255, 160, 60), (120, 255, 220), (255, 120, 190)]

    # --- pass one: where everybody is, per output frame --------------------
    positions = []
    for i in range(frames_out):
        gbf = i * step
        here = []
        for n in sorted(tracks):
            if gbf > tracks[n][-1][0]:
                continue
            st = state_at(tracks[n], gbf)
            if st is None:
                continue
            _, m, x, y, facing, walking, battle = st
            wp = world_pos(m, x, y, facing, walking)
            if wp is None:
                continue
            here.append((n, wp[0] - X0, wp[1] - Y0, wp[2], facing, battle))
        positions.append(here)

    # --- the camera --------------------------------------------------------
    # **It follows the swarm and pulls back as it spreads**, which is the whole
    # point of the view: twenty-four agents in one room is a close shot, and
    # twenty-four agents strung down Route 1 is a wide one. Smoothed over time,
    # because a camera that snaps per frame is unwatchable.
    MIN_W = 22 * TILE
    cams, prev = [], None
    for here in positions:
        if here:
            xs = [p[1] for p in here]
            ys = [p[2] for p in here]
            cx, cy = (min(xs) + max(xs)) / 2, (min(ys) + max(ys)) / 2
            need = max(MIN_W, (max(xs) - min(xs)) * 1.5,
                       (max(ys) - min(ys)) * 1.5 * OUT_W / OUT_H)
        elif prev:
            cx, cy, need = prev
        else:
            cx, cy, need = world.width / 2, world.height / 2, MIN_W
        need = min(need, world.width * 1.0)
        if prev:
            k = 0.10
            cx = prev[0] + (cx - prev[0]) * k
            cy = prev[1] + (cy - prev[1]) * k
            need = prev[2] + (need - prev[2]) * k
        prev = (cx, cy, need)
        cams.append(prev)

    for i, (here, (cx, cy, cw)) in enumerate(zip(positions, cams)):
        ch = cw * OUT_H / OUT_W
        x0 = int(round(cx - cw / 2))
        y0 = int(round(cy - ch / 2))
        crop = Image.new("RGB", (int(round(cw)), int(round(ch))), BG)
        crop.paste(world, (-x0, -y0))
        scale = OUT_W / cw
        img = crop.resize((OUT_W, OUT_H), Image.NEAREST)
        d = ImageDraw.Draw(img)
        sw = max(8, int(16 * scale))
        for (n, px, py, dim, facing, battle) in here:
            sx = int((px - x0) * scale)
            sy = int((py - y0) * scale)
            if not (-sw <= sx <= OUT_W + sw and -sw <= sy <= OUT_H + sw):
                continue
            sp = sprites.get(FACING.get(facing, "down"),
                             next(iter(sprites.values())))
            sp = sp.resize((sw, sw), Image.NEAREST)
            if dim:
                sp = sp.copy()
                sp.putalpha(sp.getchannel("A").point(lambda v: v // 3))
            img.paste(sp, (sx - sw // 2, sy - sw // 2), sp)
            c = tints[n % len(tints)]
            r = sw // 2 + 2
            d.ellipse([sx - r, sy - r, sx + r, sy + r],
                      outline=(255, 80, 80) if battle else c, width=2)
            d.text((sx + r + 1, sy - r - 1), str(n), fill=c, font=num_font,
                   stroke_width=1, stroke_fill=(0, 0, 0))
        gbf = i * step
        cap = (f"{len(here)}/{len(tracks)} agents   "
               f"{gbf / FPS_GB / 60:5.1f} emulated minutes   "
               f"{step * a.fps / FPS_GB:.0f}x   "
               f"{int(cw / TILE)} tiles across")
        bar = font.getbbox(cap)
        d.rectangle([0, 0, OUT_W, bar[3] + 12], fill=(10, 12, 16))
        d.text((10, 6), cap, fill=(235, 240, 245), font=font)
        img.save(os.path.join(a.out, f"f{i:05d}.png"))
    print(f"wrote {frames_out} frames to {a.out}")
    print("ffmpeg -y -framerate %d -i %s/f%%05d.png -c:v libopenh264 -b:v 6M out.mp4"
          % (a.fps, a.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())

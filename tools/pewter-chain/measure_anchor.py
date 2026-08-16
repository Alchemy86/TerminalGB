#!/usr/bin/env python3
"""Measure where the cartridge draws the player relative to its own tile.

    python3 tools/pewter-chain/measure_anchor.py --rom <blue.gb> \
        [--atlas /home/aaron/pokemon-run/atlas/atlas.json]

`swarm_film.py` has to decide, per agent, which pixel of the atlas a sprite
goes on. That decision was wrong — every agent sat half a tile up and to the
left of the tile it was standing on, visible as sprites straddling the left
edge of a path instead of walking down the middle of it.

**It is decided by measurement, not by reasoning about OAM.** The method needs
nothing but the two artefacts we already have, and it is exact:

1. Drive the real cartridge through `terminalgb-mcp` — `observe` gives the
   player's `(map, x, y)`, `screenshot` gives the 160x144 frame at that instant.
2. PixelGB's **four-shade** atlas is photographed from the same cartridge, so
   its tile grid *is* the game's and its shades are the game's four.
3. Sweep the hypothesis "the player's tile top-left is at screen `(ax, ay)`"
   over the whole frame, and for each one count how many pixels of the
   overlapping region disagree.

The right hypothesis makes the background line up exactly, so only the player —
who is not in the atlas — differs. Everything else is off by a tile or more and
disagrees everywhere. Measured on Red's house 2F at (3, 6):

    screen ( 64, 64)     153 of 12288 px differ =  1.25%
    screen ( 64, 56)    1297 of 11264 px differ = 11.51%
    screen ( 64, 48)    1469 of 10240 px differ = 14.35%

A 9.2x margin, and the 153 pixels that differ have bounding box x 65..78,
**y 60..75** — sixteen tall, sitting four pixels *above* the 64..79 tile box.
So: a tile is 16 px, an agent stands in the middle of one, and its sprite is
lifted a quarter of a tile. That is `Atlas.place` returning the tile centre and
`SPRITE_LIFT`.

Needs `target/release/terminalgb-mcp` (`cargo build --release --features mcp
--bin terminalgb-mcp`) and a cartridge. Nothing is written beside the ROM: it
is staged into `target/anchor/` first, because the MBC writes a `.sav` next to
whatever it is handed.
"""
import argparse
import base64
import json
import os
import shutil
import subprocess
import sys


class Mcp:
    def __init__(self, server, rom):
        self.p = subprocess.Popen(
            [server, "--rom", rom], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        self.n = 0
        self.call("initialize", {"protocolVersion": "2024-11-05",
                                 "capabilities": {},
                                 "clientInfo": {"name": "measure-anchor",
                                                "version": "1"}})
        self.call("notifications/initialized", {}, notify=True)

    def call(self, method, params=None, notify=False):
        self.n += 1
        m = {"jsonrpc": "2.0", "method": method}
        if params is not None:
            m["params"] = params
        if not notify:
            m["id"] = self.n
        self.p.stdin.write(json.dumps(m) + "\n")
        self.p.stdin.flush()
        return None if notify else json.loads(self.p.stdout.readline())

    def tool(self, name, args):
        return self.call("tools/call", {"name": name, "arguments": args})["result"]

    def observe(self):
        r = self.tool("observe", {})
        return json.loads([c for c in r["content"] if c["type"] == "text"][0]["text"])

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)


def capture(server, rom, out):
    """Play the opening until the player is standing on a loaded map."""
    gb = Mcp(server, rom)
    gb.tool("step", {"frames": 400})
    seen = None
    for _ in range(160):
        gb.tool("press_buttons", {"buttons": ["a"]})
        gb.tool("step", {"frames": 24})
        d = gb.observe()
        seen = d
        pos = d.get("position")
        if pos and pos.get("map_width") and d.get("screen", {}).get("kind") == "no-text":
            break
    gb.tool("step", {"frames": 90})       # settle, so nothing is mid-step
    d = gb.observe()
    pos = d.get("position") or (seen or {}).get("position")
    if not pos or not pos.get("map_width"):
        gb.close()
        sys.exit("never reached a loaded map; the opening did not play through")
    sh = gb.tool("screenshot", {})
    img = [c for c in sh["content"] if c["type"] == "image"][0]
    png = os.path.join(out, "frame.png")
    with open(png, "wb") as f:
        f.write(base64.b64decode(img["data"]))
    gb.close()
    return png, pos


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--atlas", default="/home/aaron/pokemon-run/atlas/atlas.json")
    ap.add_argument("--server", default="target/release/terminalgb-mcp")
    ap.add_argument("--out", default="target/anchor")
    a = ap.parse_args()
    from PIL import Image
    Image.MAX_IMAGE_PIXELS = None

    os.makedirs(a.out, exist_ok=True)
    # Never point the emulator at somebody's ROM directory: the MBC writes a
    # .sav beside whatever it is given.
    rom = os.path.join(a.out, "cart.gb")
    shutil.copyfile(a.rom, rom)
    sav = os.path.splitext(a.rom)[0] + ".sav"
    if os.path.exists(sav):
        shutil.copyfile(sav, os.path.join(a.out, "cart.sav"))

    png, pos = capture(a.server, rom, a.out)
    print(f"captured {png} at map {pos['map']} ({pos.get('map_name')}) "
          f"tile ({pos['x']},{pos['y']})")

    man = json.load(open(a.atlas))
    entry = {m["id"]: m for m in man["maps"]}.get(pos["map"])
    if entry is None:
        sys.exit(f"the atlas does not place map {pos['map']}")
    r, T = entry["rect"], entry["tile_px"]

    # The four-shade rendering, because it is the console's own four greys.
    # The region-tint one is an overlay and would disagree on every pixel.
    img = next((i for i in man["images"]
                if i["rendering"] == "four-shade" and i["scale"] == 1), None)
    d = os.path.dirname(os.path.abspath(a.atlas))
    sheet_path = next(
        (c for c in (os.path.join(d, img["path"]),
                     os.path.join(d, img["path"].replace("/", "_")),
                     os.path.join(d, os.path.basename(img["path"])))
         if os.path.exists(c)), None)
    if sheet_path is None:
        sys.exit(f"four-shade atlas image not found under {d}")

    frame = Image.open(png).convert("L")
    sheet = Image.open(sheet_path).convert("L")

    results = []
    for ax in range(0, frame.width + 1, 8):
        for ay in range(0, frame.height + 1, 8):
            ox, oy = r["x"] + pos["x"] * T - ax, r["y"] + pos["y"] * T - ay
            x0, x1 = max(ox, r["x"]), min(ox + frame.width, r["x"] + r["width"])
            y0, y1 = max(oy, r["y"]), min(oy + frame.height, r["y"] + r["height"])
            if x1 - x0 < 64 or y1 - y0 < 64:
                continue          # too little overlap to mean anything
            atl = sheet.crop((x0, y0, x1, y1)).load()
            frm = frame.crop((x0 - ox, y0 - oy, x1 - ox, y1 - oy)).load()
            bad = sum(1 for yy in range(y1 - y0) for xx in range(x1 - x0)
                      if atl[xx, yy] != frm[xx, yy])
            results.append((bad / ((x1 - x0) * (y1 - y0)), bad,
                            (x1 - x0) * (y1 - y0), ax, ay))
    results.sort()
    print("\nwhere the player's tile top-left sits on screen:")
    for frac, bad, n, ax, ay in results[:4]:
        print(f"  screen ({ax:3d},{ay:3d})  {bad:6d} of {n:6d} px differ "
              f"= {frac*100:5.2f}%")
    _, _, _, ax, ay = results[0]
    print(f"\nmargin over the runner-up: {results[1][0]/max(results[0][0],1e-9):.1f}x")

    ox, oy = r["x"] + pos["x"] * T - ax, r["y"] + pos["y"] * T - ay
    x0, x1 = max(ox, r["x"]), min(ox + frame.width, r["x"] + r["width"])
    y0, y1 = max(oy, r["y"]), min(oy + frame.height, r["y"] + r["height"])
    atl = sheet.crop((x0, y0, x1, y1)).load()
    frm = frame.crop((x0 - ox, y0 - oy, x1 - ox, y1 - oy)).load()
    pts = [(xx + x0 - ox, yy + y0 - oy)
           for yy in range(y1 - y0) for xx in range(x1 - x0)
           if atl[xx, yy] != frm[xx, yy]]
    xs, ys = [p[0] for p in pts], [p[1] for p in pts]
    print(f"the {len(pts)} differing pixels are the player, absent from the "
          f"atlas: x {min(xs)}..{max(xs)}  y {min(ys)}..{max(ys)}")
    print(f"tile box is x {ax}..{ax+T-1}  y {ay}..{ay+T-1}")
    print(f"\n=> a tile is {T} px; the sprite is lifted "
          f"{(ay - min(ys)) / T:.4f} of a tile above it (SPRITE_LIFT)")


if __name__ == "__main__":
    main()

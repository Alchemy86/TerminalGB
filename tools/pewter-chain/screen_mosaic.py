#!/usr/bin/env python3
"""The whole population's screens, tiled, playing at once.

    python3 tools/pewter-chain/screen_mosaic.py <corpus-dir> --out <dir>

`swarm_film.py` draws where every agent *is*, on one world. This draws what
every agent *sees*, side by side — forty-eight Game Boys in a rack, all playing
the same link from the same pool of entry states, at the same seeds. It is the
other half of the same question: a map says the swarm converged on a doorway, a
rack of screens says whether they are all in the same menu.

## It cannot be derived from a corpus that has already been collected

A `path-NNNNN.csv` carries `frame,map,x,y,facing,walking,battle` and no pixels
at all, so this needs a capture pass — `pewter_chain corpus --screens N`, which
writes a `screen-NNNNN.gbscr` beside each attempt's recording. See
`docs/agent/screen-mosaic.md` for the storage cost and the exact command; the
capture prints its own estimate before it emulates a frame.

## What the picture cannot tell you

Read this before drawing a conclusion off a still.

* **A tile is 80x72 at two bits.** Menu text is legible as shape, not as
  letters. Anything about *what* a box says belongs to `agent-NNNNN.ndjson`.
* **Agents are ordered by seed, not by progress**, so a row is not a cohort and
  two neighbouring tiles are related only by their seed.
* **The sample rate is coarse** — one frame in twelve by default — so a screen
  that flashes for four frames may not be in the capture at all.
* **A finished agent's tile is its last captured frame**, not a live machine.
  It is dimmed, bordered and labelled with its ending for exactly that reason:
  a frozen picture and a stuck one are otherwise the same picture.
* **The population shown is the one that was captured**, which is the low
  seeds. It is a sample of the corpus, and the corpus is the population.
"""
import argparse
import json
import math
import os
import struct
import sys

MAGIC = b"GBSCR1\0\0"
HEADER = 24
# Our own four DMG shades, darkest first — the ranks `screens.rs` quantises to.
SHADES = [(0x00, 0x00, 0x00), (0x60, 0x60, 0x60), (0xC0, 0xC0, 0xC0),
          (0xFF, 0xFF, 0xFF)]
FPS_DMG = 59.7275

INK = (232, 236, 242)
DIM = (128, 136, 150)
BG = (10, 12, 16)
PANEL = (18, 22, 28)
LIVE_EDGE = (60, 70, 86)
GOAL_EDGE = (86, 196, 122)
FAIL_EDGE = (214, 96, 96)

# `Ending::tag` (src/scenario/record.rs) rendered as a whole short word. A tag
# with no entry falls back to its first hyphen-separated part rather than being
# cut mid-word, because a clipped label reads as a complete one.
ENDING_WORD = {
    "goal": "GOAL",
    "failed": "FAILED",
    "timeout": "TIMEOUT",
    "out-of-calls": "NO CALLS",
    "stuck": "STUCK",
    "violated": "BROKE RULE",
    "fault": "FAULT",
}


# ---------------------------------------------------------------------------
# the capture
# ---------------------------------------------------------------------------

class Track:
    """One agent's screens, decoded on demand."""

    def __init__(self, path):
        self.path = path
        with open(path, "rb") as f:
            head = f.read(HEADER)
            if len(head) < HEADER or head[:8] != MAGIC:
                raise ValueError(f"{path}: not a gbscr/1 capture")
            (self.w, self.h, self.every, self.scale,
             self.frames) = struct.unpack("<HHHHI", head[8:20])
            self.data = f.read()
        self.stride = (self.w + 3) // 4
        self.plane = self.h * self.stride
        want = self.frames * self.plane
        if len(self.data) < want:
            # A capture whose file is short is a capture that was interrupted.
            # Say so and use what is there rather than decoding past the end.
            self.frames = len(self.data) // self.plane

    def bytes_on_disk(self):
        return HEADER + len(self.data)

    def last_frame(self):
        """The emulated frame the final sample was taken on."""
        return self.frames * self.every

    def at(self, emulated):
        """Sample index for an emulated frame, clamped to what was captured.

        Sample `i` is emulated frame `(i + 1) * every`, because a sample is
        taken *after* the frame it names. Returns `(index, live)` — `live` is
        false once the run has ended, which is what makes a finished tile
        distinguishable from a stuck one.
        """
        if self.frames == 0:
            return 0, False
        i = int(emulated) // self.every - 1
        if i < 0:
            return 0, True
        if i >= self.frames:
            return self.frames - 1, False
        return i, True

    def rgb(self, i):
        """One sample as a flat RGB bytes object, `w * h * 3`."""
        base = i * self.plane
        out = bytearray(self.w * self.h * 3)
        o = 0
        for y in range(self.h):
            row = self.data[base + y * self.stride:
                            base + (y + 1) * self.stride]
            for x in range(self.w):
                v = (row[x >> 2] >> (6 - 2 * (x & 3))) & 3
                r, g, b = SHADES[v]
                out[o] = r
                out[o + 1] = g
                out[o + 2] = b
                o += 3
        return bytes(out)


def link_name(d):
    """The scenario's own name, from a recording rather than from a directory.

    A directory can be called anything; a recording carries the name the
    scenario declared, which is what the banner should say.
    """
    for f in sorted(os.listdir(d)):
        if f.startswith("run-") and f.endswith(".json"):
            try:
                with open(os.path.join(d, f)) as fh:
                    n = json.load(fh).get("scenario")
                if n:
                    return str(n)
            except Exception:
                pass
            break
    return os.path.basename(os.path.abspath(d))


def read_index(d):
    """seed -> the corpus's own row for it."""
    out = {}
    p = os.path.join(d, "index.ndjson")
    if not os.path.exists(p):
        return out
    with open(p) as f:
        for line in f:
            if line.strip():
                r = json.loads(line)
                out[int(r["seed"])] = r
    return out


def find_captures(d):
    """`(seed, path)` for every capture in a corpus directory, seed order."""
    out = []
    if not os.path.isdir(d):
        return out
    for f in sorted(os.listdir(d)):
        if f.startswith("screen-") and f.endswith(".gbscr"):
            try:
                out.append((int(f[7:-6]), os.path.join(d, f)))
            except ValueError:
                pass
    return out


def scan(root, depth=3):
    """Every directory under `root` that holds a capture."""
    hits = []
    root = os.path.abspath(root)
    for cur, dirs, files in os.walk(root):
        rel = os.path.relpath(cur, root)
        if rel != "." and rel.count(os.sep) + 1 > depth:
            dirs[:] = []
            continue
        n = sum(1 for f in files
                if f.startswith("screen-") and f.endswith(".gbscr"))
        if n:
            size = sum(os.path.getsize(os.path.join(cur, f)) for f in files
                       if f.startswith("screen-") and f.endswith(".gbscr"))
            hits.append((cur, n, size))
    return hits


# ---------------------------------------------------------------------------
# output
# ---------------------------------------------------------------------------

def human(n):
    for unit, div in (("GiB", 1 << 30), ("MiB", 1 << 20), ("KiB", 1 << 10)):
        if n >= div:
            return f"{n / div:.1f} {unit}"
    return f"{n} B"


def toon_rows(name, cols, rows):
    print(f"{name}[{len(rows)}]{{{','.join(cols)}}}:")
    for r in rows:
        print("  " + ",".join(str(v) for v in r))


def die(msg, *helps):
    print(f"error: {msg}")
    for h in helps:
        print(f"help: {h}")
    sys.exit(2)


def grid_for(n, area_w, area_h, tile_w, tile_h, gap):
    """Columns and rows that leave the tiles biggest at a WHOLE zoom.

    Tried exhaustively rather than derived: `n` is at most a few hundred and
    the aspect ratio of a Game Boy screen (10:9) is not a video's, so the
    closed form for one is wrong for the other.

    **The integer zoom is the thing being maximised, and the fraction is only
    the tie-break.** A tile is drawn nearest-neighbour at a whole multiple, for
    the same reason every other picture in this repository is: a fractional
    resample invents shades the Game Boy never drew, and a four-shade picture
    is exactly where that is most visible. A grid that scores 2.9 fractionally
    and 2 as a whole number is no better than one that scores 2.1.
    """
    best = None
    for cols in range(1, n + 1):
        rows = math.ceil(n / cols)
        w = (area_w - gap * (cols + 1)) / cols
        h = (area_h - gap * (rows + 1)) / rows
        if w <= 4 or h <= 4:
            continue
        scale = min(w / tile_w, h / tile_h)
        key = (int(scale), scale)
        if best is None or key > best[0]:
            best = (key, cols, rows)
    if best is None:
        die(f"{n} tiles do not fit in {area_w}x{area_h}",
            f"Lower --tiles, or raise --width/--height (a {tile_w}x{tile_h} "
            f"tile needs about {tile_w * n // max(1, int(math.sqrt(n)))} px "
            f"across)")
    return best[1], best[2]


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


def home(root):
    """No arguments: what captures exist, not a manual."""
    here = os.path.abspath(__file__).replace(os.path.expanduser("~"), "~", 1)
    print(f"bin: {here}")
    print("description: Tile many agents' Game Boy screens into one video, "
          "from a `pewter_chain corpus --screens` capture.")
    hits = scan(root)
    if not hits:
        print(f"captures: 0 found under {root} — nothing has recorded its "
              f"screens yet")
        print("help:")
        print("  Run `./target/release/examples/pewter_chain corpus --rom <rom> "
              "--scenario scenarios/pewter/05-the-rival-battle.json "
              "--start-dir <pool> --runs 48 --screens 12 --out <dir>` to "
              "capture one")
        print("  Read docs/agent/screen-mosaic.md for the storage cost and the "
              "whole recipe")
        return
    rows = []
    for d, n, size in sorted(hits, key=lambda h: -h[1]):
        rows.append((os.path.relpath(d, root), n, human(size).replace(" ", "")))
    toon_rows("captures", ["dir", "agents", "size"], rows)
    print("help:")
    print(f"  Run `python3 {os.path.relpath(__file__, root)} <dir> --dry-run` "
          f"to see what a mosaic of one would show")
    print(f"  Run `python3 {os.path.relpath(__file__, root)} <dir> --out "
          f"<frames-dir>` to render it")


# ---------------------------------------------------------------------------

class Parser(argparse.ArgumentParser):
    """argparse, with its errors moved to stdout.

    An error an agent cannot read is an error it cannot act on, and argparse
    puts its own on stderr — along with a message naming argparse's internals
    (`invalid <lambda> value`) rather than anything this tool can do about it.
    """

    def error(self, message):
        die(message, f"Run `python3 {os.path.relpath(__file__)} --help` for "
                     f"every flag and its default")


def main():
    ap = Parser(
        add_help=True,
        formatter_class=argparse.RawDescriptionHelpFormatter,
        description="Tile many agents' Game Boy screens into one video, from a "
                    "`pewter_chain corpus --screens` capture.",
        epilog="""examples:
  screen_mosaic.py
      list every capture under the current directory

  screen_mosaic.py target/mosaic/05-the-rival-battle --dry-run
      the grid, the pace and the size, without writing a frame

  screen_mosaic.py target/mosaic/05-the-rival-battle --out target/mosaic-frames
      render 60 tiles at 1920x1080, 8x, 30 fps

  screen_mosaic.py <dir> --out <dir> --tiles 24 --speed 12 --cover 0.9
      a smaller, faster cut that stops once 90% of agents have finished

the capture comes first:
  pewter_chain corpus --rom <rom> --scenario <s.json> --start-dir <pool> \\
      --runs 60 --workers 8 --screens 12 --out <dir>
see docs/agent/screen-mosaic.md for the storage cost.""")
    ap.add_argument("corpus", nargs="?",
                    help="a corpus directory holding screen-NNNNN.gbscr")
    ap.add_argument("--out", default=None,
                    help="where the PNG sequence goes")
    ap.add_argument("--tiles", type=int, default=60,
                    help="how many agents to show (default 60, which is what "
                         "packs a 1920x1080 frame at a whole x2)")
    ap.add_argument("--first", type=int, default=0,
                    help="the first seed to show (default 0)")
    ap.add_argument("--cols", type=int, default=0,
                    help="columns; 0 picks the grid that leaves tiles biggest")
    ap.add_argument("--width", type=int, default=1920,
                    help="1920x1080 by default, because that is the smallest "
                         "frame that draws 60 tiles at a whole x2")
    ap.add_argument("--height", type=int, default=1080,
                    help="output height (default 1080)")
    ap.add_argument("--fps", type=int, default=30,
                    help="output frame rate (default 30)")
    ap.add_argument("--speed", type=float, default=8.0,
                    help="emulated seconds per second of film (default 8)")
    ap.add_argument("--seconds", type=float, default=0.0,
                    help="a runtime to aim for instead of a speed; the "
                         "resulting ratio is printed either way")
    ap.add_argument("--cover", type=float, default=1.0,
                    help="fraction of agents the film runs to the end of "
                         "(1.0 = the slowest)")
    ap.add_argument("--title", default=None,
                    help="the banner's first line (default: the link's name)")
    ap.add_argument("--dry-run", action="store_true",
                    help="report what would be drawn and draw nothing")
    ap.add_argument("--root", default=".",
                    help="where the no-argument view looks for captures")
    a = ap.parse_args()

    if not a.corpus:
        home(a.root)
        return

    if not os.path.isdir(a.corpus):
        die(f"{a.corpus} is not a directory",
            f"Run `python3 {os.path.relpath(__file__)}` to list the captures "
            f"that exist")
    caps = [(s, p) for s, p in find_captures(a.corpus) if s >= a.first]
    if not caps:
        die(f"{a.corpus} holds 0 screen-NNNNN.gbscr captures",
            "A corpus records its screens only when asked: add "
            "`--screens 12` to the `pewter_chain corpus` command",
            "Read docs/agent/screen-mosaic.md for the storage cost first")
    caps = caps[:max(1, a.tiles)]
    index = read_index(a.corpus)

    tracks, bad = [], []
    for seed, p in caps:
        try:
            tracks.append((seed, Track(p)))
        except ValueError as e:
            bad.append((seed, str(e)))
    if not tracks:
        die(f"every capture in {a.corpus} failed to decode",
            "The format is `gbscr/1`; a capture from a later build needs a "
            "later reader")

    every = tracks[0][1].every
    tw, th = tracks[0][1].w, tracks[0][1].h
    for seed, t in tracks:
        if (t.every, t.w, t.h) != (every, tw, th):
            die(f"seed {seed} was captured at {t.w}x{t.h}/{t.every} and seed "
                f"{tracks[0][0]} at {tw}x{th}/{every}",
                "A mosaic needs one capture pass; render them separately")

    # How long the film covers. `--cover` cuts stragglers, never a stage: the
    # emulated time is the material and a lower cover only drops the thinning
    # tail at the end.
    ends = sorted(t.last_frame() for _, t in tracks)
    k = min(len(ends) - 1, max(0, int(round(a.cover * len(ends))) - 1))
    last = ends[k]
    if a.seconds > 0:
        frames_out = max(1, int(round(a.seconds * a.fps)))
        stride = max(every, int(math.ceil(last / frames_out)))
    else:
        stride = max(every, int(round(a.speed * FPS_DMG / a.fps)))
    frames_out = max(1, int(math.ceil(last / stride)))
    ratio = stride * a.fps / FPS_DMG

    name = link_name(a.corpus)
    title = a.title or name.replace("-", " ").upper()
    goals = sum(1 for s, _ in tracks if index.get(s, {}).get("goal"))
    known = sum(1 for s, _ in tracks if s in index)

    banner_h = max(46, a.height // 14)
    gap = 4
    cols, rows = (a.cols, math.ceil(len(tracks) / a.cols)) if a.cols > 0 else \
        grid_for(len(tracks), a.width, a.height - banner_h, tw, th, gap)
    cw = (a.width - gap * (cols + 1)) // cols
    ch = (a.height - banner_h - gap * (rows + 1)) // rows
    zoom = max(1, min(cw // tw, ch // th))
    dw, dh = tw * zoom, th * zoom

    print(f"mosaic:")
    print(f"  corpus: {a.corpus}")
    print(f"  link: {name}")
    print(f"  agents: {len(tracks)} of {len(find_captures(a.corpus))} captured "
          f"(seeds {tracks[0][0]}..{tracks[-1][0]})")
    print(f"  outcomes: {goals} goal, {known - goals} not, "
          f"{len(tracks) - known} unrecorded")
    print(f"  tile: {tw}x{th} at 2 bpp, drawn {dw}x{dh} (x{zoom})")
    print(f"  grid: {cols}x{rows} in {a.width}x{a.height}")
    print(f"  sampled: every {every} emulated frames")
    print(f"  film: {frames_out} frames at {a.fps} fps = "
          f"{frames_out / a.fps:.1f} s, {ratio:.1f}x timelapse, "
          f"covering {last} emulated frames ({last / FPS_DMG:.1f} s)")
    print(f"  capture: {human(sum(t.bytes_on_disk() for _, t in tracks))} read")
    if bad:
        toon_rows("undecodable", ["seed", "why"], bad)

    # The smoothness ceiling, stated rather than assumed: a sample every
    # `every` frames cannot be shown twice, so a stride below it repeats.
    if stride < every:
        print(f"  note: a stride of {stride} is finer than the capture's "
              f"{every}, so frames repeat")

    if a.dry_run:
        print("help:")
        print(f"  Run the same command with `--out <dir>` to render it")
        print(f"  Add `--tiles <n>` for a different population, or "
              f"`--speed <x>` for a different pace")
        return

    if not a.out:
        die("--out is required to render",
            f"Run `python3 {os.path.relpath(__file__)} {a.corpus} --dry-run` "
            f"to see what it would draw without writing anything",
            f"Run `python3 {os.path.relpath(__file__)} {a.corpus} --out "
            f"target/mosaic-frames` to render")

    from PIL import Image, ImageDraw
    os.makedirs(a.out, exist_ok=True)
    f_big = load_font(max(13, banner_h // 3))
    f_small = load_font(max(9, banner_h // 5))
    f_tile = load_font(max(8, min(14, dw // 9)))

    # A tile's ending, decided once: colour AND a word, never colour alone.
    # **A truncated word is worse than a short one** — `OUT-OF-CA` is not a
    # reading, and the reader cannot tell it was cut. Every tag the runner emits
    # has a whole short form here (`src/scenario/record.rs`, `Ending::tag`).
    def ending_of(seed):
        r = index.get(seed)
        if not r:
            return None, "?"
        if r.get("goal"):
            return GOAL_EDGE, "GOAL"
        tag = str(r.get("ending", "end"))
        return FAIL_EDGE, ENDING_WORD.get(tag, tag.split("-")[0].upper()[:7])

    endings = {s: ending_of(s) for s, _ in tracks}
    ox = (a.width - (cols * dw + gap * (cols - 1))) // 2
    oy = banner_h + (a.height - banner_h - (rows * dh + gap * (rows - 1))) // 2

    for i in range(frames_out):
        emulated = (i + 1) * stride
        img = Image.new("RGB", (a.width, a.height), BG)
        d = ImageDraw.Draw(img)
        d.rectangle([0, 0, a.width, banner_h - 1], fill=PANEL)
        live_n = 0
        for n, (seed, t) in enumerate(tracks):
            idx, live = t.at(emulated)
            live_n += 1 if live else 0
            tile = Image.frombytes("RGB", (t.w, t.h), t.rgb(idx))
            if zoom > 1:
                tile = tile.resize((dw, dh), Image.NEAREST)
            edge, word = endings[seed]
            if not live:
                # **A finished agent must not look like a stuck one.** Its last
                # frame is dimmed towards the background so the live tiles read
                # as the live ones, and the ending is written on it — the
                # redundant coding `src/screen/panel.rs` insists on, because a
                # colour alone is not a reading.
                tile = Image.blend(tile, Image.new("RGB", tile.size, PANEL),
                                   0.55)
            x = ox + n % cols * (dw + gap)
            y = oy + n // cols * (dh + gap)
            img.paste(tile, (x, y))
            td = ImageDraw.Draw(img)
            td.rectangle([x - 1, y - 1, x + dw, y + dh],
                         outline=(edge or LIVE_EDGE) if not live else LIVE_EDGE)
            td.text((x + 2, y + 1), f"{seed}", font=f_tile, fill=DIM)
            if not live:
                w = td.textlength(word, font=f_tile)
                td.rectangle([x + dw - w - 5, y + dh - 13, x + dw - 1, y + dh - 1],
                             fill=PANEL)
                td.text((x + dw - w - 3, y + dh - 13), word, font=f_tile,
                        fill=edge or DIM)
        d.text((10, 5), title, font=f_big, fill=INK)
        d.text((10, 5 + banner_h // 2),
               f"{len(tracks)} agents, one link, same entry pool  ·  "
               f"live {live_n}  ·  goal {goals}  ·  "
               f"t+{emulated / FPS_DMG:6.1f}s emulated  ·  {ratio:.0f}x",
               font=f_small, fill=DIM)
        img.save(os.path.join(a.out, f"m{i:05d}.png"))

    print(f"frames: {frames_out} -> {a.out}")
    print("help:")
    print(f"  Run `ffmpeg -y -framerate {a.fps} -i {a.out}/m%05d.png "
          f"-c:v libopenh264 -b:v 6M <out.mp4>` to encode it")
    print(f"  Run the same command with `--tiles <n>` or `--speed <x>` for a "
          f"different population or pace")


if __name__ == "__main__":
    main()

"""Game Boy screen captures, and tiling them.

**Moved here verbatim from `screen_mosaic.py` so there is ONE of each.** The
swarm film needs the same two things the mosaic tool needs — decode a
`gbscr/1` capture, and lay `n` of them out at a whole zoom — the moment it can
cut from the map to the battle screens, and a second copy of either would
eventually disagree with the first about what a capture holds.

See `chainrender/__init__.py` for why this package exists at all; the extract
was checked by re-extracting each name from both files and comparing, which is
this package's own standing rule after a salvaged `palette.py` was found to
have lost six entries.
"""

import math
import struct


def die(message, *helps):
    """Local, so this module needs no AXI import; callers override by catching
    SystemExit or by checking their inputs first."""
    print(f"error: {message}")
    for h in helps:
        print(f"help: {h}")
    raise SystemExit(1)


MAGIC = b"GBSCR1\0\0"
HEADER = 24
# Our own four DMG shades, darkest first — the ranks `screens.rs` quantises to.
SHADES = [(0x00, 0x00, 0x00), (0x60, 0x60, 0x60), (0xC0, 0xC0, 0xC0),
          (0xFF, 0xFF, 0xFF)]


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


def best_grid(n_max, area_w, area_h, tile_w, tile_h, gap, min_zoom=2):
    """How many tiles to show, and the grid and WHOLE zoom to draw them at.

    **New here, not extracted.** `grid_for` answers "given `n` tiles, what
    grid?"; a film that cuts to the battle screens has a different question,
    because the tile count is its choice too — it is showing *some* of the
    battles, and which number it picks is really a choice of zoom.

    **As many battles as fit at a LEGIBLE zoom**, which is the mosaic's whole
    job: showing many at once, with each still readable. Two objectives that
    look reasonable and are not, both measured on a 1024x720 picture area with
    an 80x72 capture:

    * maximise the count → 48 tiles at 1x, covering 276,480 px of 737,280 and
      leaving a third of the picture black. That was the first attempt.
    * maximise the covered area → **12** tiles at 3x (622,080 px), because
      coverage always prefers fewer, bigger tiles. A dozen battles is not a
      screen full of battles.

    So the count is maximised subject to `zoom >= min_zoom`: 24 tiles at 2x
    here, 6x4, covering 552,960 px. If no layout reaches `min_zoom` — a small
    picture, or a capture taken at scale 1 — it falls back to the largest zoom
    available, so a caller always gets a picture.

    Returns `(n, cols, rows, zoom)`.
    """
    def layouts(want_z):
        for n in range(n_max, 0, -1):
            for cols in range(1, n + 1):
                rows = math.ceil(n / cols)
                z = min((area_w - gap * (cols + 1)) // cols // tile_w,
                        (area_h - gap * (rows + 1)) // rows // tile_h)
                if z >= want_z:
                    yield (n, int(z), cols, rows)

    for n, z, cols, rows in layouts(min_zoom):
        return n, cols, rows, z
    best = None
    for n, z, cols, rows in layouts(1):
        key = (z, n)
        if best is None or key > best[0]:
            best = (key, n, cols, rows, z)
    if best is None:
        die(f"no whole zoom fits a {tile_w}x{tile_h} tile in {area_w}x{area_h}",
            "raise --width, or capture at a smaller --screens-scale")
    return best[1], best[2], best[3], best[4]

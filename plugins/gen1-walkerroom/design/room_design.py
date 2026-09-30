#!/usr/bin/env python3
"""The walker room, designed: pixel art -> tiles -> blocks -> a room.

    room_design.py OUT_DIR          writes OUT_DIR/preview.png and ../src/art_data.rs

Everything the plugin draws comes from here; nothing is taken from a cartridge. Art
is drawn with the small helpers below onto 32x32 block canvases ('0' lightest ...
'3' darkest), each block is cut into 4x4 tiles of 8x8, and identical tiles share
one id, because the cartridge gives a room only 96 tile slots (`vTileset`,
$9000-$95FF).

Two Game Boy facts the design is built around:
  * a walk tile is 2x2 screen tiles, and a step is refused or allowed by looking at
    ONE of them: the bottom-left tile of the quadrant. So a tile id is walkable or it
    is not, everywhere; `Tiles` keeps a tile that must be walked on and one that must
    not apart even when they look alike.
  * the trade table and its two cushions are fixed by the cartridge's hidden events
    at walk tiles (4,4) and (5,4), seats at (3,4) and (6,4): block (2,2) is the table
    and blocks (1,2) and (3,2) are the cushions. Nothing may move them.
"""
import sys, os

# ------------------------------------------------------------------- drawing

class Cv:
    """A pixel canvas. `.` is never stored: painting '.' is a no-op."""
    def __init__(self, w, h, fill='0'):
        self.w, self.h = w, h
        self.p = [[fill] * w for _ in range(h)]
    def set(self, x, y, c):
        if c != '.' and 0 <= x < self.w and 0 <= y < self.h:
            self.p[y][x] = c
    def rect(self, x, y, w, h, c):
        for j in range(h):
            for i in range(w):
                self.set(x + i, y + j, c)
    def frame(self, x, y, w, h, c):
        for i in range(w):
            self.set(x + i, y, c); self.set(x + i, y + h - 1, c)
        for j in range(h):
            self.set(x, y + j, c); self.set(x + w - 1, y + j, c)
    def disc(self, cx, cy, r, c):
        for j in range(-r, r + 1):
            for i in range(-r, r + 1):
                if i * i + j * j <= r * r + r // 2:
                    self.set(cx + i, cy + j, c)
    def ring(self, cx, cy, r, c):
        for j in range(-r - 1, r + 2):
            for i in range(-r - 1, r + 2):
                d = i * i + j * j
                if r * r - r <= d <= r * r + r:
                    self.set(cx + i, cy + j, c)
    def line(self, x0, y0, x1, y1, c):
        n = max(abs(x1 - x0), abs(y1 - y0), 1)
        for k in range(n + 1):
            self.set(round(x0 + (x1 - x0) * k / n), round(y0 + (y1 - y0) * k / n), c)
    def art(self, rows, x, y):
        """Paint ASCII rows at (x, y); '.' is see-through."""
        for j, row in enumerate(rows):
            for i, ch in enumerate(row):
                self.set(x + i, y + j, ch)
    def paste(self, other, x, y):
        for j in range(other.h):
            for i in range(other.w):
                self.set(x + i, y + j, other.p[j][i])
    def rows(self):
        return [''.join(r) for r in self.p]

def pattern(rows, w, h):
    """Tile an 8-row pattern across a w x h canvas."""
    cv = Cv(w, h)
    for y in range(h):
        row = rows[y % len(rows)]
        for x in range(w):
            cv.set(x, y, row[x % len(row)])
    return cv

# A 3x5 pixel font, for the words painted on the floor.
FONT = {
    'S': ["111", "100", "111", "001", "111"], 'E': ["111", "100", "111", "100", "111"],
    'N': ["101", "111", "111", "111", "101"], 'D': ["110", "101", "101", "101", "110"],
    'T': ["111", "010", "010", "010", "010"], 'A': ["010", "101", "111", "101", "101"],
    'K': ["101", "101", "110", "101", "101"],
}
def word(cv, text, x, y, c='3'):
    for k, ch in enumerate(text):
        for j, row in enumerate(FONT[ch]):
            for i, bit in enumerate(row):
                if bit == '1':
                    cv.set(x + k * 4 + i, y + j, c)

# --------------------------------------------------------------------- floor & wall

FLOOR = ["11111111", "00000001", "00000001", "00000001",
         "11111111", "00010000", "00010000", "00010000"]

def floor(w=32, h=32):
    return pattern(FLOOR, w, h)

def wall_face():
    cv = Cv(32, 32)
    for y in range(32):
        row = "22212221" if y < 26 else ("11111111" if y < 30 else "33333333")
        if y < 2:
            row = "33333333"
        for x in range(32):
            cv.set(x, y, row[x % 8])
    return cv

def garland(cv):
    """A string of lights along the top of a wall block, sagging in the middle."""
    cv.line(0, 3, 31, 3, '3')
    for x in (3, 11, 19, 27):
        cv.set(x, 4, '3'); cv.set(x, 5, '0'); cv.set(x, 6, '0'); cv.set(x + 1, 5, '1'); cv.set(x - 1, 5, '1')
        cv.set(x, 7, '1')

# ----------------------------------------------------------------- objects
# Every object sits on the 8-pixel tile grid (a 16x16 object inside one quadrant of a
# block, or a run of them), so it costs whole tiles and shares them with its twins.

def poster(cv, x, y, style):
    cv.rect(x, y, 16, 16, '0'); cv.frame(x, y, 16, 16, '3'); cv.frame(x + 2, y + 2, 12, 12, '2')
    if style == 0:      # a Poke Ball
        cv.disc(x + 8, y + 8, 4, '1'); cv.rect(x + 4, y + 8, 9, 1, '3')
        cv.disc(x + 8, y + 8, 1, '0'); cv.ring(x + 8, y + 8, 4, '3')
    else:               # a lightning bolt
        cv.art(["..33..", ".33...", "3333..", "..33..", ".33...", "3....."], x + 5, y + 5)
        cv.rect(x + 3, y + 3, 2, 2, '1')

def window(cv, x, y):
    cv.rect(x, y, 16, 16, '0'); cv.frame(x, y, 16, 16, '3'); cv.frame(x + 1, y + 1, 14, 14, '2')
    cv.rect(x + 7, y + 1, 2, 14, '3'); cv.rect(x + 1, y + 7, 14, 2, '3')
    cv.line(x + 3, y + 5, x + 5, y + 3, '1'); cv.line(x + 10, y + 5, x + 12, y + 3, '1')

def rug_tile(cv, x0, y0, w, h):
    """A rug: black edge, dark band, woven middle."""
    for j in range(h):
        for i in range(w):
            edge = min(i, j, w - 1 - i, h - 1 - j)
            if edge == 0: c = '3'
            elif edge == 1: c = '2'
            elif edge == 2: c = '1'
            else: c = '1' if (i + j) % 2 == 0 else '0'
            cv.set(x0 + i, y0 + j, c)

def pokeball(cv, cx, cy, r):
    cv.disc(cx, cy, r, '0')
    for j in range(-r, 0):
        for i in range(-r, r + 1):
            if i * i + j * j <= r * r + r // 2:
                cv.set(cx + i, cy + j, '2')
    cv.rect(cx - r, cy, 2 * r + 1, 1, '3')
    cv.ring(cx, cy, r, '3')
    cv.disc(cx, cy, max(1, r // 3), '0'); cv.ring(cx, cy, max(1, r // 3), '3')

def cushion(cv, x, y):
    cv.disc(x + 8, y + 8, 6, '1'); cv.ring(x + 8, y + 8, 6, '3')
    cv.disc(x + 8, y + 8, 3, '0'); cv.ring(x + 8, y + 8, 3, '2')

PIKACHU_DOLL = [
    "..3......3......",
    ".33......33.....",
    ".33.....33......",
    "..3.3333.3......",
    "...311113.......",
    "..31011013......",
    "..31111113..33..",
    "..3211112..3223.",
    "...31111333.323.",
    "...3211123..33..",
    "..321111123.....",
    "..321111123.....",
    "...3222223......",
    "....33.33.......",
    ".....3.3........",
    "................",
]

def bookshelf(cv, x, y):
    """A 32x16 shelf: two rows of books, with the pattern repeating every 8 pixels
    so its middle tiles are all the same."""
    cv.rect(x, y, 32, 16, '2'); cv.frame(x, y, 32, 16, '3')
    for sy in (y + 1, y + 9):
        cv.rect(x + 1, sy, 30, 6, '3')
        for k in range(0, 30, 8):
            for (bx, bw, top, shade) in ((1, 2, 1, '0'), (4, 1, 0, '2'), (6, 1, 2, '1')):
                cv.rect(x + k + bx, sy + top, bw, 6 - top, shade)
    cv.rect(x, y + 7, 32, 1, '3')

def tv(cv, x, y):
    cv.rect(x, y, 16, 16, '2'); cv.frame(x, y, 16, 16, '3')
    cv.rect(x + 2, y + 3, 9, 9, '0'); cv.frame(x + 2, y + 3, 9, 9, '3')
    for k in range(5, 12, 2):
        cv.line(x + 3, y + k, x + 9, y + k, '1')
    cv.disc(x + 13, y + 6, 1, '0'); cv.disc(x + 13, y + 10, 1, '1')
    cv.line(x + 5, y + 3, x + 3, y, '3'); cv.line(x + 8, y + 3, x + 10, y, '3')

def walker_stand(cv, x, y):
    cv.rect(x + 1, y + 11, 14, 5, '2'); cv.frame(x + 1, y + 11, 14, 5, '3')
    pokeball(cv, x + 8, y + 6, 5)

def plant(cv, x, y):
    cv.art([
        "......3333......", "....3322233.....", "...322112223....", "..32211112223...",
        "..32111011223...", "...321100123....", "....3211123.....", ".....32123......",
        "......323.......", "....3333333.....", "...322222223....", "...322111123....",
        "...321111123....", "....3211123.....", "....3322233.....", ".....33333......"], x, y)

def pad(cv, label, arrow_up):
    """A floor plate, 32x16 on the tile grid, the word above and an arrow below."""
    cv.rect(0, 8, 32, 16, '0')
    cv.frame(0, 8, 32, 16, '3'); cv.frame(1, 9, 30, 14, '2')
    word(cv, label, 8, 11)
    a = ["..3..", ".333.", "33333", "..3..", "..3.."] if arrow_up else ["..3..", "..3..", "33333", ".333.", "..3.."]
    cv.art(a, 14, 17)

def table(cv):
    """The trade table, 32x32: two screens over one base; left and right identical."""
    for x in (2, 18):
        cv.rect(x, 4, 12, 10, '0'); cv.frame(x, 4, 12, 10, '3')
        for k in range(4):
            cv.rect(x + 2, 6 + k * 2, 8, 1, '1')
        cv.rect(x, 16, 12, 10, '2'); cv.frame(x, 16, 12, 10, '3'); cv.rect(x + 2, 19, 8, 2, '1')

# ---------------------------------------------------------------------- tiles

class Tiles:
    def __init__(self):
        self.tiles = []          # list of (pixel rows, walkable-or-None)
        self.index = {}
    def add(self, rows, walk_role=None):
        """An id for these 8 rows. `walk_role` is True/False when this is the tile a
        step is judged by (so its id must agree), None when it is not."""
        key = tuple(rows)
        if walk_role is None:
            for (pix, w), i in self.index.items():
                if pix == key:
                    return i
            self.index[(key, None)] = len(self.tiles)
            self.tiles.append((key, None))
            return len(self.tiles) - 1
        if (key, walk_role) in self.index:
            return self.index[(key, walk_role)]
        if (key, None) in self.index:
            i = self.index.pop((key, None))
            self.tiles[i] = (key, walk_role)
            self.index[(key, walk_role)] = i
            return i
        self.index[(key, walk_role)] = len(self.tiles)
        self.tiles.append((key, walk_role))
        return len(self.tiles) - 1

TILES = Tiles()
BLOCKS = {}

def block(name, cv, walk):
    """Cut a 32x32 canvas into 16 tiles. `walk` is the four flags for the walk-tile
    quadrants TL, TR, BL, BR: the bottom-left tile of each is what collision reads."""
    ids = []
    coll = {4: walk[0], 6: walk[1], 12: walk[2], 14: walk[3]}
    for t in range(16):
        ty, tx = divmod(t, 4)
        rows = [''.join(cv.p[ty * 8 + r][tx * 8:tx * 8 + 8]) for r in range(8)]
        ids.append(TILES.add(rows, coll.get(t)))
    BLOCKS[name] = ids

ALL, NONE = [True] * 4, [False] * 4

def on_floor(fn=None, rug=False):
    cv = floor()
    if fn:
        fn(cv)
    return cv

def make_blocks():
    block('floor', floor(), ALL)
    # walls: ring and top wall share the boards; the top wall alone has lights
    w = wall_face(); garland(w); block('wall', w, NONE)
    w = wall_face(); garland(w); poster(w, 8, 8, 0); block('wall_poster_ball', w, NONE)
    w = wall_face(); garland(w); poster(w, 8, 8, 1); block('wall_poster_bolt', w, NONE)
    w = wall_face(); garland(w); window(w, 8, 8); block('wall_window', w, NONE)
    w = wall_face(); block('wall_side', w, NONE)
    # the rug under the table: 3x3 blocks; the table is the middle, the cushions its sides
    big = Cv(96, 96); rug_tile(big, 0, 0, 96, 96)
    for by in range(3):
        for bx in range(3):
            sub = Cv(32, 32)
            for j in range(32):
                for i in range(32):
                    sub.set(i, j, big.p[by * 32 + j][bx * 32 + i])
            walk = ALL
            if (bx, by) == (1, 1):
                table(sub); walk = NONE
            elif (bx, by) in ((0, 1), (2, 1)):
                cushion(sub, 8, 8)
            block(f'rug{bx}{by}', sub, walk)
    # a runner rug under the lounge row, so the pads sit on something: the same rug
    # art, so its edge tiles are the big rug's own and cost nothing
    RUN_N = 9
    run = Cv(RUN_N * 32, 32); rug_tile(run, 0, 0, RUN_N * 32, 32)
    def runner(i):
        sub = Cv(32, 32)
        for j in range(32):
            for x in range(32):
                sub.set(x, j, run.p[j][i * 32 + x])
        return sub
    for i in range(RUN_N):
        block(f'run{i}', runner(i), ALL)
    # furniture, each in its own quadrants
    cv = floor(); bookshelf(cv, 0, 0); block('shelf', cv, [False, False, True, True])
    cv = floor(); tv(cv, 16, 0); block('tv', cv, [True, False, True, True])
    cv = floor(); plant(cv, 8, 8); block('plant', cv, [False, False, True, True])
    cv = runner(4); walker_stand(cv, 8, 8); block('stand', cv, NONE)
    cv = runner(1); pad(cv, "SEND", True); block('pad_send', cv, ALL)
    cv = runner(7); pad(cv, "TAKE", False); block('pad_take', cv, ALL)
    # dolls, each in one quadrant of an otherwise open block
    cv = runner(0); cv.art(PIKACHU_DOLL, 8, 6); block('doll_pikachu', cv, [False, True, True, True])
    cv = runner(8); pokeball(cv, 24, 22, 6); block('doll_ball', cv, [True, True, True, False])
    cv = floor(); cushion(cv, 0, 0); cushion(cv, 16, 16); block('cushions', cv, [False, True, True, False])

ROOM_W, ROOM_H = 12, 9

def layout():
    g = [['floor'] * ROOM_W for _ in range(ROOM_H)]
    for x in range(ROOM_W):
        g[0][x] = 'wall'; g[ROOM_H - 1][x] = 'wall_side'
    for y in range(1, ROOM_H):
        g[y][0] = 'wall_side'; g[y][ROOM_W - 1] = 'wall_side'
    g[0][1] = 'wall_poster_ball'; g[0][6] = 'wall_poster_bolt'
    g[0][4] = 'wall_window'; g[0][9] = 'wall_window'
    # the rug, with the table and its cushions on it: fixed by the cartridge
    for by in range(3):
        for bx in range(3):
            g[1 + by][1 + bx] = f'rug{bx}{by}'
    # the reading corner
    g[1][7] = 'shelf'; g[1][8] = 'shelf'; g[1][10] = 'tv'
    g[1][6] = 'plant'
    # the pads, the walker between them, and the dolls
    for i in range(9):
        g[6][1 + i] = f'run{i}'
    g[6][2] = 'pad_send'; g[6][5] = 'stand'; g[6][8] = 'pad_take'
    g[6][1] = 'doll_pikachu'; g[6][9] = 'doll_ball'
    g[4][9] = 'cushions'
    g[7][10] = 'plant'
    return g

# ---------------------------------------------------------------------- output

PAL = [(232, 240, 224), (168, 184, 152), (96, 112, 88), (24, 32, 24)]

def render(grid, scale=3):
    from PIL import Image
    im = Image.new('RGB', (ROOM_W * 32, ROOM_H * 32))
    px = im.load()
    for by, row in enumerate(grid):
        for bx, name in enumerate(row):
            ids = BLOCKS[name]
            for t in range(16):
                ty, tx = divmod(t, 4)
                pix, _ = TILES.tiles[ids[t]]
                for r in range(8):
                    for c in range(8):
                        px[bx * 32 + tx * 8 + c, by * 32 + ty * 8 + r] = PAL[int(pix[r][c])]
    return im.resize((ROOM_W * 32 * scale, ROOM_H * 32 * scale), Image.NEAREST)

def tile_bytes(pix):
    out = []
    for r in pix:
        lo = hi = 0
        for c, ch in enumerate(r):
            v = int(ch)
            lo |= (v & 1) << (7 - c)
            hi |= ((v >> 1) & 1) << (7 - c)
        out += [lo, hi]
    return out

def main():
    make_blocks()
    grid = layout()
    n = len(TILES.tiles)
    print(f"{n} tiles of 96, {len(BLOCKS)} blocks")
    if n > 96:
        sys.exit("too many tiles: the cartridge has room for 96")
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(out, exist_ok=True)
    render(grid).save(os.path.join(out, 'preview.png'))
    names = list(BLOCKS)
    bid = {nm: i for i, nm in enumerate(names)}
    rs = ["// generated by design/room_design.py; do not edit\n",
          f"pub const ROOM_W: usize = {ROOM_W};\npub const ROOM_H: usize = {ROOM_H};\n",
          f"pub const TILE_COUNT: usize = {n};\n",
          "pub const TILES: [[u8; 16]; %d] = [\n" % n]
    for pix, _ in TILES.tiles:
        rs.append("    [" + ", ".join("0x%02X" % b for b in tile_bytes(pix)) + "],\n")
    rs.append("];\n")
    walk = [i for i, (_, w) in enumerate(TILES.tiles) if w]
    rs.append("/// Tile ids a step may land on (the collision list).\npub const WALKABLE: [u8; %d] = [%s];\n" % (len(walk), ", ".join(str(i) for i in walk)))
    rs.append("pub const BLOCK_COUNT: usize = %d;\n" % len(names))
    rs.append("pub const BLOCKS: [[u8; 16]; %d] = [\n" % len(names))
    for nm in names:
        rs.append("    [" + ", ".join(str(t) for t in BLOCKS[nm]) + "], // %s\n" % nm)
    rs.append("];\n")
    rs.append("/// The room, row by row, as indexes into BLOCKS.\npub const ROOM: [u8; %d] = [\n" % (ROOM_W * ROOM_H))
    for row in grid:
        rs.append("    " + ", ".join(str(bid[b]) for b in row) + ",\n")
    rs.append("];\n")
    for kind in ("send", "take"):
        for by, row in enumerate(grid):
            for bx, nm in enumerate(row):
                if nm == "pad_" + kind:
                    rs.append("/// The %s pad, in walk tiles: x0, y0, x1, y1 (inclusive).\npub const PAD_%s: (u8, u8, u8, u8) = (%d, %d, %d, %d);\n" % (kind, kind.upper(), bx * 2, by * 2, bx * 2 + 1, by * 2 + 1))
    dest = os.path.join(os.path.dirname(os.path.abspath(__file__)), '..', 'src', 'art_data.rs')
    open(dest, 'w').write(''.join(rs))

main()

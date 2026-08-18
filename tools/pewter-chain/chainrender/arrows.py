"""Direction bookkeeping and the arrow polygon.

Moved here verbatim from path_arrows.py so there is ONE of it.
See chainrender/__init__.py for why this package exists.
"""

# --- the data ----------------------------------------------------------------
# `PathPoint::facing` — 0 down, 4 up, 8 left, 12 right, the game's own encoding
# (`src/scenario/run.rs`). Indexed 0..3 here so it fits an `array('B')`.
DIRS = ["down", "up", "left", "right"]

DELTA = {(0, 1): 0, (0, -1): 1, (-1, 0): 2, (1, 0): 3}

FACING_DIR = {0: 0, 4: 1, 8: 2, 12: 3}

OPPOSITE = {0: 1, 1: 0, 2: 3, 3: 2}

# Unit vector per direction, in screen axes (y grows down).
VEC = {0: (0, 1), 1: (0, -1), 2: (-1, 0), 3: (1, 0)}

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

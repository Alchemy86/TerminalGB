"""One renderer's worth of parts, shared by every film this repository makes.

## Why this exists

There were **four** things compositing these videos and three of them were
reimplementations of the first:

| renderer | camera | palette | atlas | tracks |
|---|---|---|---|---|
| `swarm_film.py` | the tuned one | its own | its own | its own |
| `examples/swarm_view.rs` | none | its own | measured seams | its own |
| `path_arrows.py` | n/a | its own | borrowed by `importlib` | its own |
| AgentGB `chain.py` | none | its own | its own | its own |

`CLAUDE.md` had already recorded the cause -- *"a worktree permission rule
caused a rewrite, the rewrite lost qualities of the original, and it then
happened again"* -- and recorded consolidating as deferred, not cancelled.

The cost was not tidiness. It was that **a fix landed on one video and not the
others**: the camera smoothing exists only in `swarm_film.py`, so every film
the other three make judders by construction; and an arrow palette fixed in
`path_arrows.py` reaches no other picture.

## The rule

Anything two renderers would both want lives here, and a renderer imports it.
A renderer keeps only what is genuinely its own -- what it draws, and how it
lays a frame out.

The directory above has a hyphen in its name, so it is not importable by name;
a tool adds its own directory to `sys.path` and imports `chainrender`. That is
also why `path_arrows.py` used to reach `Atlas` through an `importlib` hack,
which is now unnecessary.

## What is NOT here

`swarm_view.rs` is Rust and stays where it is for now. It holds three things
the Python side does not have -- map origins measured from seams on the running
cartridge, `--verify-determinism`, and the categorical no-new-colours guard --
so it is the thing to consolidate *onto* eventually, not away from. Retiring it
into this package would lose those.
"""

from . import arrows, atlas, axi, camera, paint, palette, tracks  # noqa: F401

from .atlas import Atlas  # noqa: F401
from .camera import Camera  # noqa: F401
from .arrows import DIRS, arrow_points  # noqa: F401
from .paint import load_font, on_screen, tint, wrap_hud  # noqa: F401
from .palette import (  # noqa: F401
    BATTLE_HUE, BATTLE_KIND, BG, DIM, DIRECTION_HUE, ENCODINGS, HEAT_HUE, INK,
    PACED_HUE, PANEL, TRAFFIC_RAMP, arrow_rgb, arrow_scale, band_of,
    legend_swatches, traffic_fraction,
)
from .tracks import (  # noqa: F401
    FACING, FPS_GB, STEP_PX, TILE, WALK_FRAMES, WALK_STEPS,
    flag_at, interpolate, link_at, load_links, track_files, walk_span,
)

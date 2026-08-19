# `chainrender` — one renderer's worth of parts

<!-- codemap:asof -->

[`tools/pewter-chain/chainrender/`](../../tools/pewter-chain/chainrender/) holds
the atlas, the track reader, the palettes, the arrow polygon, the drawing
primitives, the AXI helpers and **the camera**. Every Python renderer in this
repository imports it.

## Why it exists, and it is not tidiness

There were **four** things compositing these videos and three of them were
reimplementations of the first:

| renderer | camera | palette | atlas | tracks |
|---|---|---|---|---|
| [`swarm_film.py`](../../tools/pewter-chain/swarm_film.py) | the tuned one | its own | its own | its own |
| [`examples/swarm_view.rs`](../../examples/swarm_view.rs) | none | its own | measured seams | its own |
| [`path_arrows.py`](../../tools/pewter-chain/path_arrows.py) | n/a | its own | borrowed by `importlib` | its own |
| AgentGB's `chain.py` | none | its own | its own | its own |

`AGENTS.md` already recorded the cause — *a worktree permission rule caused a
rewrite, the rewrite lost qualities of the original, and it then happened
again* — and recorded consolidating as deferred, not cancelled.

**The cost was that a fix landed on one video and not the others.** The camera
smoothing existed only inside `swarm_film.main()`, two hundred lines that
nothing else could reach, so every film the other three made juddered by
construction. Landing [rule 7](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md#rule-7-smooth-the-box-because-a-bimodal-swarm-has-no-midpoint)
on that camera would have had exactly the same reach.

## The rule

Anything two renderers would both want lives here. A renderer keeps only what is
genuinely its own — what it draws, and how it lays a frame out.

The directory above has a hyphen in its name, so it is **not importable by
name**. Every tool puts its own directory on `sys.path` and imports
`chainrender`:

```python
HERE = os.path.dirname(os.path.abspath(__file__))
if HERE not in sys.path:
    sys.path.insert(0, HERE)
import chainrender as cr
```

That is also what replaced `path_arrows.py`'s `importlib` hack, which loaded
`swarm_film.py` as a module to reach one class.

## What is in it

| module | holds |
|---|---|
| `atlas.py` | `Atlas` — PixelGB's manifest, and where a map lands in it |
| `tracks.py` | the position-track reader, `TILE`/`FPS_GB`/`STEP_PX`, `walk_span`, `interpolate`, `walk_phase` |
| `camera.py` | `Camera` — the seven rules, and every constant they were tuned to |
| `palette.py` | every colour any renderer uses, and the two arrow encodings |
| `arrows.py` | direction bookkeeping and the arrow polygon |
| `paint.py` | `tint`, `wrap_hud`, `on_screen`, `load_font` |
| `axi.py` | TOON on stdout, errors on stdout, help lines |

## Extract by AST, never by line number

The proxied `grep`/`sed` in this environment renumber lines, and cutting on
their numbers split two docstrings on a first attempt. `ast.get_source_segment`
plus a walk back over the preceding comment block is what worked, and the same
walk is what proved every moved symbol is **verbatim**: re-extract each name
from the pre-move source and from the package and compare.

That check earned its place immediately. An in-flight copy of `palette.py` had
`HEAT_HUE` with **eight** entries where the live one has **fourteen** — a
palette a film indexes directly, so the first render after wiring it crashed
with `IndexError`. A file that looks right is not a file that is right.

## The equality gates

Refactoring a renderer is only safe if you can prove it changed nothing, and
both proofs are cheap enough that there is no excuse for skipping them.

```bash
cd /home/aaron/pokemon-run

# the camera: --no-split-aware --no-integer-zoom must reproduce the pre-fix
# trace byte for byte. **--no-integer-zoom is part of the gate now**: rules 8
# and 9 changed which rung the camera picks and how fast it travels between
# rungs, so the flag that reproduces the old shot has to be in the command or
# the gate is asserting the new one against the old one's md5.
python3 <repo>/tools/pewter-chain/swarm_film.py chain-film \
    --atlas atlas/atlas.json --out /tmp/x --dry-run \
    --camera-trace /tmp/cam.csv --speed 12 --fps 60 \
    --no-split-aware --no-integer-zoom
# 15,632 frames, md5 4291439921254f04bff653a4948fe000
# verified again after rules 8 and 9, at two invocations: the default
# --seconds 70 --fps 30 gives 2,100 frames / 74dd586a82d9af555beb6634865a052e
# and --speed 12 --fps 60 --cover 0.99 gives 69,640 / 81fd46e732ba1933df1dba422a04aaf1,
# identical to a build of the pre-change source in both cases.

# the arrows: --colour traffic must reproduce the published still
python3 <repo>/tools/pewter-chain/path_arrows.py still chain-film \
    --atlas atlas/atlas.json --colour traffic --out /tmp/arrows.png
# md5 75146a5f60b19beea973902c56b300cf, 551,205 bytes
```

`--dry-run` renders nothing, so the camera check is two seconds and the arrow
check is one. **Both still reproduce after `walk_phase` split `_segment` out of
`interpolate`** — the camera trace is derived from interpolated positions, so
that gate is a real check on the refactor and not only on the camera.

## `walk_phase` — the walk cycle, from the same segment as the position

`interpolate` and `walk_phase` are two questions about one thing: **where in
its step is this agent?** They share `_segment`, deliberately, so the legs and
the feet cannot disagree about it. `interpolate` turns the answer into a
fractional tile position; `walk_phase` returns the raw fraction, or `None` when
no step is under way — standing still, or a jump `interpolate` refuses to glide.

**It is a reconstruction, and the format asks for one.**
`terminalgb::scenario::track` measured that 83 % of all track rows changed
nothing but `wWalkCounter`, dropped it, and said in as many words that "the step
is expanded back at draw time". This is that expansion. `swarm_film.py` uses it
to pick a pose out of the walk cycle `pewter_chain sprites` captures: the
cartridge draws the standing pose for the first half of a step and a stepping
pose for the second, alternating which one between steps — all three of those
facts measured, and written up in
[the render comparison](swarm-render-comparison.md#72--the-walk-cycle).

A sprite directory with no `player-<dir>-w<n>.png` files in it renders exactly
as it did before the cycle existed, which is what keeps an already-published
film reproducible — checked, 700 frames, identical md5.

## Rules 8 and 9 — the zoom

Two rules landed in `chainrender/camera.py` after the package existed, and both
are in it precisely so that every renderer gets them rather than only the one
whose author happened to be looking.

* **Rule 8: every rung is an integer pixel scale.** Measured over the Mt Moon
  cut, not one frame of 7,100 sat at one — and a fractional nearest resample of
  Gen 1's one-pixel dither is moiré, which is both the softness in the picture
  and two thirds of the bitrate.
  [`film-encoding.md`](film-encoding.md) §1 is the measurement.
* **Rule 9: the zoom travels, and it never cuts.** Reported as *"it suddenly
  jerks OUT when more entertaining say mt moon, instead of either panning out
  and across nicely"*. Rule 7 fixed a swarm split between two clusters on ONE
  map; a **map transition** is a third shape neither rule covers — the atlas
  places Mt Moon 1F 1,400 px from Route 4, so the box multiplies the instant
  the first agents warp in and rule 3 assigned the new width in a single frame.
  The zoom now moves at most one rung per change, has to hold before the next,
  and **glides geometrically between rungs** so that the picture is pixel-exact
  whenever the camera is at rest and only the handful of frames of an actual
  zoom are off a rung.

Measured on the 600-agent Mt Moon cut, 8,290 frames:

| | before rule 9 | one rung at a time | rung + glide |
|---|---:|---:|---:|
| largest single-frame zoom step | up to 8x | 2.00x | **1.03x** |
| largest single-frame pan, x | 10 px | 5 px | **5 px** |
| frames off an integer rung | n/a | 0 | 78 (0.9 %) |
| camera coverage, mean | 99.5 % | 99.8 % | **99.8 %** |
| frames below 90 % covered | 0 | 0 | **0** |

`report_camera` prints the zoom step now. It did not before, which is why "it
jerks" had no number attached to it: the existing columns count how OFTEN the
zoom moved and say nothing about how far it went in one frame, and a cut reads
as a jerk however rarely it happens.

## The zoom target could latch on a rung wider than the world

**Found from a camera report and it was not the camera's framing.** Reported on
the gym cut: *"Everyone was in the gym for ages and yet we were still zoomed
out over a large area — we should have been right up in clear view on the
gym."*

The obvious reading is that the box is a symmetric percentile band, so it
cannot tighten onto a dense cluster with a tail. That was implemented (a
highest-density interval instead of a band), swept, and **rejected** — the
measurement and the numbers are in `camera.py` above `BAND`, and the short
version is that a swarm spread over Mt Moon's three floors has no single mass
to tighten onto, so it costs 99.8 % coverage for 83.2 %.

The real fault was rule 9's own: `zoom_to` was set to a **rung wider than the
world**, `cw` is clamped to the world, so the arrival test never fired — and
because a glide in flight suppresses the "should I start a zoom?" branch, the
camera latched at the world width for the rest of the film. The clamp is on the
target now, and completion is "we arrived **or** we cannot move further".

| gym cut, 600 agents | before | after |
|---|---:|---:|
| median crop width | 1696 px (the whole world) | **512 px** (the tightest rung) |
| frames at world width | 2938 of 2938 | **204 of 2938** |
| coverage, mean | 99.2 % | 98.7 % |

**Mt Moon is byte-identical either way** — the camera trace over its 8,290
frames has the same md5 before and after — because the latch only bites when a
swarm *concentrates after having been spread*, which is the gym's shape and not
Mt Moon's. That is why the fault survived a film that measured its camera on
every other axis: nothing was measuring the crop width against the world.

## Screen captures, and tiling them

`chainrender/mosaic.py` holds the `gbscr/1` reader and the tiling, moved
verbatim from `screen_mosaic.py` (checked by re-extracting each name from both
files and comparing — this package's own rule) plus one new function,
`best_grid`. The swarm film needs both the moment it can **cut to the battle
screens**, and a second copy of either would eventually disagree about what a
capture holds. See [`screen-mosaic.md`](screen-mosaic.md) §7.

## What is deliberately NOT here

**`examples/swarm_view.rs` stays where it is.** It is Rust, and it holds three
things the Python side does not: map origins measured from seams on the running
cartridge at single-step granularity, `--verify-determinism`, and the
categorical no-new-colours guard. It is the thing to consolidate *onto*
eventually, not away from; retiring it into this package would lose those.

**`screen_mosaic.py` shares only the font loader.** Its palette is deliberately
its own — its background is `(10, 12, 16)` against the films' `(16, 18, 22)` —
and unifying two palettes that were chosen separately would change a published
picture to no end.

## Related pages

- [the path-arrow map](path-arrows.md) — the movement map, and the two arrow
  encodings `palette.py` holds.
- [the Pewter chain](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) — §11 on the films, and rule 7.
- [the swarm view](../swarm-view.md) — the Rust compositor, and what it has
  that the Python side does not.

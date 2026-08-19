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
| `tracks.py` | the position-track reader, `TILE`/`FPS_GB`/`STEP_PX`, `walk_span`, `interpolate` |
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

# the camera: --no-split-aware must reproduce the pre-fix trace byte for byte
python3 <repo>/tools/pewter-chain/swarm_film.py chain-film \
    --atlas atlas/atlas.json --out /tmp/x --dry-run \
    --camera-trace /tmp/cam.csv --speed 12 --fps 60 --no-split-aware
# 15,632 frames, md5 4291439921254f04bff653a4948fe000

# the arrows: --colour traffic must reproduce the published still
python3 <repo>/tools/pewter-chain/path_arrows.py still chain-film \
    --atlas atlas/atlas.json --colour traffic --out /tmp/arrows.png
# md5 75146a5f60b19beea973902c56b300cf, 551,205 bytes
```

`--dry-run` renders nothing, so the camera check is two seconds and the arrow
check is one.

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

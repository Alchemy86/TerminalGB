# The swarm view — N emulators, one map, one picture

<!-- codemap:asof -->

Twenty-four Game Boys start from the same save state, walk off in twenty-four
different directions, and are drawn as twenty-four dots on one map of Route 1
and Pallet Town. The camera starts on the pile they all begin as and pulls back
until the whole corridor is in frame.

It is a **proof of concept**, deliberately small: one example
([`examples/swarm_view.rs`](../examples/swarm_view.rs)), no new dependency, no
change to any emulated behaviour, and a video that regenerates from two
commands.

## Credit

The picture is **Peter Whidden's**. *Training AI to Play Pokémon with
Reinforcement Learning* ([video](https://www.youtube.com/watch?v=DcYLT37ImBY),
[code](https://github.com/PWhiddy/PokemonRedExperiments)) opens with a swarm of
RL agents drawn on a stitched world map, zooming out until the whole crowd is
visible, and it is the best diagnostic image anyone has produced in this area:
**the moment a hundred agents pile into one corner you can see it, instead of
reading it out of a table.** He did it two ways — sprites on a map, and a
collage of live Game Boy screens placed at each agent's world position. This
reproduces the first.

His stack, for contrast: [PyBoy](https://github.com/Baekalfen/PyBoy) as the
emulator, [Stable-Baselines3](https://github.com/DLR-RM/stable-baselines3) with
`Monitor` / `DummyVecEnv` / `VecTransposeImage` wrappers, PPO, in a conda
environment. One detail is worth naming fairly, because it is the whole reason
the economics here are different: **PyBoy warns that its own timing degrades
above about 5x speed.** This core does not have that constraint — [the swarm
below](#the-run) ran hundreds of times faster than real time with every frame
cycle-accurate — which is what makes resets, replays and byte-identical reruns
affordable rather than a luxury.

## What is assembled, and what is new

**Not one of the pieces this depends on was built for it.** That is the
interesting fact about the picture, more than the picture itself: every row
below was written for its own unrelated reason, by whoever needed it, and the
demo fell out of the pile afterwards.

| Piece | Built for | What the swarm view uses it for |
|---|---|---|
| `HuntConfig::seed_for` + `Schedule` ([shiny hunting](shiny-hunt.md#4-decorrelation--the-interesting-problem)) | decorrelating parallel shiny hunts, so N instances do not all roll the same Pokémon | making twenty-four walkers genuinely diverge |
| `HuntMachine::load_state` and [save states](savestates.md) | letting a hunt reset an attempt to a known position | starting every instance from one identical world |
| `wCurMap` / `wXCoord` / `wYCoord` — `inv`-tier entries in [the memory atlas](gen1/atlas/README.md), [vendored here](../third_party/atlasgb/README.md) | documenting Gen 1 memory with evidence that the cartridge uses it | knowing where every agent is, every frame |
| `Gameboy::image()` and the [raw screenshot path](picture.md#4-screenshots--c-and-c) | an archival frame that is comparable against hardware, un-palette-mapped and un-upscaled | capturing the map backgrounds out of the running cartridge |
| `video::card::Canvas` and its 5x7 font ([video out](video-out.md#5b-configuring-a-destination-is-not-deciding-to-stream)) | so a virtual camera has something to show while the game is not streaming | every caption and map label on the composited frame |
| The categorical colour guard ([display scaling](display.md#the-regression-guard)) | proving the terminal's half-block picture is not being blurred | proving the zoom invents no shade between two agents' colours |
| The screen anchor at tile (8, 8) ([the World Trade Centre](worldlobby.md)) | calibrating a shared room against the running engine | placing a 160x144 framebuffer at the right world pixel |
| Headless speed, and frame-exact determinism | accuracy, and making the test suites affordable to run | twenty-four agents x ten emulated minutes in the wall time below, reproducibly |

**The compositor is the only genuinely new piece**: take N positions, stamp
markers on a canvas, emit a frame. The map backgrounds are captured by the same
example from the emulator itself (below), and ffmpeg turns the PNGs into a
video.

That is a better argument for how this project is built than any claim about
capability. Nobody planned a swarm view. The parts were sharp enough
individually that assembling one cost **one example file and no new
dependency** — and each of them is still doing its original job, unchanged, for
the feature it was actually written for.

Two small additions were made to the shiny hunter, both purely additive and
neither changing its behaviour: `Schedule::step` became `pub`, and
`Schedule::draw` was added so a walker can take a further value from the **same**
stream rather than starting a private PRNG beside it.

### Why the walk is not the hunter's walk

`Instance::attempt` paces a **±1 tile band of grass** on purpose — that walker
exists to roll encounters without leaving the patch, and it is the right walker
for a hunt. A swarm view wants agents that go somewhere, so the policy here is a
free four-direction explorer. Every *timing* decision it makes still comes out
of `hunt::Schedule`, so the two rigs diverge for exactly the same reason and a
divergence result on one is evidence about the other.

## Running it

```bash
# stage the cartridge and the save state out of the way of your real files
mkdir -p target/swarm
cp ~/roms/'Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb' target/swarm/blue.gb
cp ~/shiny/route1.gbss target/swarm/route1.gbss

cargo build --release --features plugin-gen1-shinyhunt,image --example swarm_view
cd target/swarm

# 1. capture the map backgrounds out of the emulator (25s)
../release/examples/swarm_view --rom blue.gb --state route1.gbss \
    --capture-maps maps --capture-frames 200000

# 2. run the swarm and composite (23s)
../release/examples/swarm_view --rom blue.gb --state route1.gbss --maps maps \
    --instances 24 --frames 36000 --sample 24 --zoom --out frames

# 3. the command it prints
ffmpeg -y -framerate 30 -i frames/frame_%06d.png \
    -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' -c:v libx264 -pix_fmt yuv420p \
    -crf 18 swarm.mp4
```

**Fedora, and anything else shipping a patent-clean ffmpeg, has no `libx264`** —
swap `-c:v libx264 -crf 18` for `-c:v libopenh264 -b:v 4M`. The tool prints that
note too, because `Unknown encoder 'libx264'` reads as a broken command rather
than a missing codec.

The starting state is the shiny hunter's:
`shiny_hunt --rom blue.gb --make-state route1.gbss` plays a fresh game to the
grass at the southern end of Route 1.

Nothing is written next to the ROM. Every instance is built with
`Gameboy::try_new_with_model(rom, None, ..)`, and `src/mbc/battery.rs` only
wires a battery when it is given a path — so a swarm of any size cannot touch a
real save.

## The run

24 instances, 36,000 emulated frames each (10.0 minutes of game time per agent,
241 minutes in total), a position sampled every 24 frames, on a 20-core box:

| | |
|---|---|
| wall time | **23.8 s** |
| emulated | 864,000 frames = **608x real time** |
| video | 1,500 frames, 352x926, **50 s at 30 fps = 12x game speed** |
| encounters fled | 203 |
| samples spent in battle | 19.05 % |
| blackouts | 0 |

## Proof they diverged

Not asserted — printed by the run, and reproducible:

```
divergence
  first disagreement at sample 1 (emulated frame 24)
  distinct paths   24 of 24
  distinct finishing positions  23
```

All twenty-four instances load byte-identical save-state bytes and then take
**twenty-four distinct paths**, differing within the first 24 emulated frames
(the schedules' `lead_in` values differ, so the first press lands at a different
moment). This is the same lever `shiny_hunt --probe` measures from the DV side;
here it is measured from the position side, which is a second, independent
witness for the same claim.

## The finding: Route 1's ledges are a one-way valve

This is what the picture is for, and the answer is not flattering to a random
walker.

```
where they went  (share of all agent-samples)
  PALLET TOWN     54.97%
  ROUTE 1         44.73%
  ELSEWHERE        0.30%   (two interiors, unplaced)

clustering  (the busiest tiles, share of all agent-samples)
  ROUTE 1        ( 17, 28)    5.03%
  ROUTE 1        (  4, 31)    3.67%
  PALLET TOWN    (  1,  2)    3.34%
  349 distinct tiles visited; the busiest 17 hold 37% of all samples
```

Three things are visible in that, and only the third needed the video:

1. **They cluster hard.** 5 % of the tiles they ever stand on hold 37 % of all
   the time they spend. The busiest single tile is the one they started on.
2. **They drain downhill.** After ten emulated minutes more than half the swarm
   is in Pallet Town, which is *south* of where they started. Route 1's ledges
   are one-way: an agent can hop down them and cannot walk back up, so a random
   walk is a ratchet. The top half of Route 1 is empty in every frame of the
   video after the first minute.
3. **Nobody ever reaches Viridian City.** Not in this run and not in a longer
   control (12 agents, 16 emulated minutes each: Route 1 70.06 %, Pallet Town
   29.87 %, Viridian 0 %). The corridor's exit is three screens north of the
   start and behind the ledges; unguided exploration does not find it.

That third point is the reason the video is worth having. A table of "percent
time per map" states it; the picture makes it obvious in two seconds, which is
precisely Whidden's argument.

It is also a fair reminder of what this swarm is and is not. **These are not
learning agents.** They are a decorrelated random walk — the null model. The
picture is the diagnostic; what an actual policy does to it is the experiment,
and the emulator already has an agent harness for that
([`docs/claude-plays.md`](claude-plays.md),
[`docs/longplay.md`](longplay.md)).

### It is published as the floor

**[AgentGB](https://github.com/Alchemy86/AgentGB) publishes a rendered still and
video of this run**, as `swarm-baseline.png` / `swarm-baseline.mp4` under its
`docs/media/`; that directory's own README carries the picture, the numbers
above and the same warning that the agents are not learning.

That is the useful thing to do with a null model: publish it as **the floor**.
Anything trained has to beat these figures on this corridor, and until now there
was nothing to beat — a paper reporting "our agent reached Viridian City" says
very little without knowing that undirected walking never does. The two commands
at the top regenerate it from a seed, so the baseline is checkable rather than
quoted.

## Determinism, tested

Same ROM, same save state, same `--seed`, `--instances`, `--frames` and
`--sample` gives a byte-identical video. Two independent claims, both measured:

* **In one process** — `--verify-determinism` runs the swarm twice and compares
  every sample of every instance: `IDENTICAL`.
* **Across processes** — two separate invocations writing to different
  directories produced 375 PNGs each with the same
  `sha256 bf4dbc2e4e3ab092…`. A third run with `--seed 99` produced
  `ad333b2a09e09ddc…`, so the check is not vacuous.

The mechanism is worth stating because it is the part that could easily have
been got wrong: **each instance is sampled on its own frame counter**, not on
wall time and not on a shared tick. Instance *i*'s emulated frame 1200 always
lands in video frame 50, whichever thread reached it first, so thread
scheduling cannot move a pixel.

## Capturing the maps from the emulator

There is no stitched world map in this repository yet — that is
`mapgb-render-t8` (PixelGB), a separate project. Rather than block on it, the
example captures the two maps it needs itself, and the mechanism is small:

* The player's walk tile occupies **screen tile (8, 8)**, i.e. screen pixel
  (64, 64) — measured against the running engine by the World Trade Centre work
  (`screen::calibrate` scores that row 360/360 and row 9 133/360; see
  [`docs/worldlobby.md`](worldlobby.md)). So the 160x144 framebuffer belongs at
  world pixel `(x*16 - 64, y*16 - 64)`.
* **Only when `wWalkCounter` is 0.** Mid-step the engine is scrolling and the
  screen is not on the tile grid; a frame captured then is a couple of pixels
  out and smears the whole sheet.
* The 16x24 box around the player is skipped, or the finished map is a trail of
  Reds. Every one of those pixels is covered from another viewpoint anyway.
* A pixel is written **once**: first view wins, and never-visited ground stays
  transparent so the compositor can show it as empty rather than as black.

**A random walk is a bad surveyor, and the reason is the same valve.** 120,000
frames of undirected walking covered 100 % of Pallet Town and only **44 %** of
Route 1 — it fell down the ledges and stayed there. The capture walker
therefore heads three times in four for the centroid of whatever is still
un-photographed on the current map. With that, 200,000 frames covers Route 1
100 % and Pallet Town 98 % in 25 seconds.

### Where the maps sit relative to each other

Also measured, not tabulated. A map *connection* preserves the player's world
position, so if an agent was standing at `(fx, fy)` on map A and one step later
is at `(tx, ty)` on map B having moved by `step`, then B's origin is
`A's + (fx + step.x - tx, fy + step.y - ty)` tiles. Exactly, with no averaging
needed.

**"One step later" is load-bearing, and getting it wrong is a silent two-tile
error.** The first version watched for the map change once per *sample* (every
24 frames), by which time the agent had walked one to three tiles into the new
map — so the offset came out short by however far it got, and Pallet Town was
placed 32 px under Route 1's southern edge. The picture still looked plausible;
what gave it away was Pallet Town's own label being invisible, overpainted by
Route 1's rectangle. The transition is watched **every frame** now, and only on
a frame where the walk animation has finished (`wWalkCounter == 0`), the machine
is out of battle and the map header has loaded (`wCurMapWidth != 0`) — during
`LoadMapData` the coordinates and the header disagree and an observation taken
there is nonsense. With that, Route 1 and Pallet Town abut to the pixel: the
canvas grew by exactly the 32 px that had been overlapping.

The remaining catch is that a **warp** — a door — also changes `wCurMap` while a
direction is held, and from outside it looks the same. The inference is
therefore restricted to an allow-list of outdoor maps in this corridor, between
which there is no warp, so every transition it sees is a connection. Anything
else is reported as unplaced and counted in the caption. Two Pallet Town
interiors (maps 37 and 39) turned up in the run and are listed exactly that way.

When PixelGB lands, the allow-list and the inference both go away and the real
world map takes their place.

## The zoom, and the guard on it

`--zoom` holds a 4x crop centred on the swarm's centroid and steps down one
integer scale at a time over the first third of the run. **Integer scales only,
nearest-neighbour, no filtering at all** — the whole value of this picture is
that every colour on it means something, so a smoothed zoom that invents a shade
between two agents' colours is a bug, not a softness.

That is checked rather than promised, by the same categorical rule
[`docs/display.md`](display.md) uses for the half-block path: after scaling,
**every colour in the output must already have existed in the input**. It is a
yes/no question with no threshold to tune, and the run fails loudly if a scaling
step ever invents one.

The caption is drawn *after* the zoom, so it stays the same size and in the same
place while the map moves underneath it.

## See also

* [the screen mosaic](agent/screen-mosaic.md) — the same population drawn as
  their own **screens** rather than as positions on a map. A position track
  carries no pixels, so it needs its own capture pass; it answers "are they all
  in the same menu?" where this answers "are they all in the same place?".
* [the swarm's paths](agent/path-arrows.md) — the same kind of position track
  as a *movement map*: one arrow per walked tile, coloured by how much of the
  swarm came that way. It reproduces the one-way-ledge finding above from the
  Pewter chain's own tracks — **22 of Route 1's tiles are northbound only and
  40 are southbound only** — so two instruments, built from different data for
  different reasons, agree about the shape of the route.

## Honest limits

* **Two maps, not a world.** By design for a proof of concept; see PixelGB
  above.
* **Markers, not screens.** Whidden's other view — a collage of live Game Boy
  framebuffers placed at each agent's world position, some showing a battle,
  some a menu — is not built. The pieces are all here (the capture path already
  places a framebuffer at a world position), but it is a bigger picture and a
  much bigger file.
* **The video is not committed.** It is game footage; it regenerates from the
  two commands above into `target/swarm/`.
* **The agents are not learning.** See the finding above.

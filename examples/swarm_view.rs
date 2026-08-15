//! **The swarm view** — N emulators from one save state, all drawn on one map.
//!
//! Peter Whidden's *Training AI to Play Pokémon with Reinforcement Learning*
//! (<https://github.com/PWhiddy/PokemonRedExperiments>, and the video of the
//! same name) opens with the picture this reproduces: a few dozen agents
//! running at once, each a dot at its own place in the world, the camera
//! pulling back until the whole swarm is in frame. It is the best diagnostic
//! image in the field, because **the moment a hundred agents pile into one
//! corner you can see it instead of reading it out of a table.**
//!
//! What is reused rather than reinvented:
//!
//! * **The divergence machinery** is the shiny hunter's.
//!   [`hunt::HuntConfig::seed_for`] derives one seed per instance and
//!   [`hunt::Schedule`] draws every press length independently from it — that
//!   is the lever proven (by `shiny_hunt --probe`) to make N machines started
//!   from one byte-identical save state produce N different runs. A second
//!   walker inventing its own randomness would be measuring a different rig.
//! * **The seam** is [`hunt::HuntMachine`], which the emulator already
//!   implements for `Gameboy`.
//! * **Positions** are `wCurMap` / `wXCoord` / `wYCoord`, all `inv`-tier
//!   entries in the vendored atlas (`third_party/atlasgb/pokemon-rb/atlas.tsv`).
//!
//! What is new is the **compositor**, and only the compositor: take N
//! positions, stamp markers on a canvas, emit a frame.
//!
//! The walk itself is deliberately *not* the hunter's. That walker exists to
//! stay inside a band of grass and is ±1 tile wide on purpose; a swarm view
//! wants agents that go somewhere. So the policy here is a free four-direction
//! explorer — but every timing decision it makes still comes out of
//! [`hunt::Schedule`], so the two rigs diverge for the same reason.
//!
//! ```text
//! # capture the backgrounds (one agent, walking, stitching its own frames)
//! cargo run --release --features plugin-gen1-shinyhunt,image --example swarm_view -- \
//!     --rom blue.gb --state route1.gbss --capture-maps target/swarm/maps
//!
//! # run the swarm and write the video frames
//! cargo run --release --features plugin-gen1-shinyhunt,image --example swarm_view -- \
//!     --rom blue.gb --state route1.gbss --maps target/swarm/maps \
//!     --instances 24 --frames 36000 --sample 12 --out target/swarm/frames
//!
//! # ...then the printed ffmpeg command turns them into an mp4.
//! ```
//!
//! **Determinism.** Same ROM, same save state, same `--seed`, same
//! `--instances`, same `--frames`, same `--sample` produces a byte-identical
//! video. Each instance is sampled on *its own* frame counter, so which thread
//! finishes first cannot change a pixel; `--verify-determinism` runs the swarm
//! twice in one process and compares.
//!
//! It never writes next to the ROM: every machine is built with
//! `Gameboy::try_new_with_model(rom, None, ..)`, and `src/mbc/battery.rs` only
//! wires a battery when it is given a path.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use terminalgb::gameboy::Gameboy;
use terminalgb::video::card::Canvas;
use terminalgb::KeypadKey;
use terminalgb_plugin_gen1_shinyhunt::hunt::{self, HuntConfig, HuntMachine, Schedule};
use terminalgb_plugin_gen1_shinyhunt::{ShinyHunt, Target};

// ---------------------------------------------------------------------------
// Cartridge facts. Every address is an `inv`-tier atlas entry.
// ---------------------------------------------------------------------------

/// `wCurMap` — the map the player is standing on.
const CUR_MAP: u16 = 0xD35E;
/// `wYCoord` — position in 16x16 tiles from the top of the map.
const Y_COORD: u16 = 0xD361;
/// `wXCoord` — position in 16x16 tiles from the left.
const X_COORD: u16 = 0xD362;
/// `wCurMapHeight` — in **32x32 blocks**, i.e. half the tile height.
const MAP_HEIGHT: u16 = 0xD368;
/// `wCurMapWidth` — in 32x32 blocks.
const MAP_WIDTH: u16 = 0xD369;
/// `wIsInBattle`. `$FF` is "blacked out", not a battle.
const IS_IN_BATTLE: u16 = 0xD057;
/// `wWalkCounter` — non-zero means the engine is mid-step and owns every
/// sprite's screen position. A framebuffer captured then is not tile-aligned.
const WALK_COUNTER: u16 = 0xCFC5;

/// A Gen 1 walk tile is 16x16 pixels; a block is 32x32.
const TILE_PX: i64 = 16;
/// The player's walk tile's top-left **screen** cell is (8, 8) in 8x8 GB tiles,
/// i.e. screen pixel (64, 64). Measured by the World Trade Centre crew against
/// the running engine (`screen::calibrate` scores row 8 at 360/360 cells and
/// row 9 at 133/360); see `docs/worldlobby.md`.
const PLAYER_SCREEN_PX: i64 = 64;

const GB_W: usize = 160;
const GB_H: usize = 144;

/// Outdoor maps of the Pallet Town corridor, and the **only** maps whose
/// relative position this tool will infer.
///
/// The inference watches an agent walk across a seam and reads the offset off
/// the coordinate jump (see [`Layout::build`]). That is sound for a map
/// *connection* and nonsense for a *warp*: a door also changes `wCurMap` while
/// the player is holding a direction, and there is no cheap way to tell the two
/// apart from outside. In this corridor no two allow-listed maps are joined by
/// a warp, so restricting the inference to the list makes every transition it
/// sees a connection. Anything else is drawn in the gutter and counted.
///
/// The real answer — every map in the game, placed — comes from PixelGB's
/// world-map extraction, which is a separate project and had not landed when
/// this was written.
const OUTDOOR: &[u8] = &[
    0,  // PALLET_TOWN
    1,  // VIRIDIAN_CITY
    12, // ROUTE_1
    13, // ROUTE_2
    40, // ROUTE_22
];

fn map_name(id: u8) -> &'static str {
    match id {
        0 => "PALLET TOWN",
        1 => "VIRIDIAN CITY",
        12 => "ROUTE 1",
        13 => "ROUTE 2",
        40 => "ROUTE 22",
        _ => "ELSEWHERE",
    }
}

// ---------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------

struct Args {
    rom: PathBuf,
    state: PathBuf,
    instances: usize,
    frames: u64,
    sample: u32,
    seed: u64,
    out: Option<PathBuf>,
    maps: Option<PathBuf>,
    capture_maps: Option<PathBuf>,
    capture_frames: u64,
    zoom: bool,
    scale: u32,
    verify: bool,
    quiet: bool,
}

fn usage() {
    println!(
        "\
The swarm view: N emulators from one save state, all drawn on one map.

  --rom <path>              Pokemon Red or Blue (required)
  --state <path>            the save state every instance starts from (required)
  --instances <n>           how many emulators (default {n}, cores - 1)
  --frames <n>              emulated frames each instance runs (default 36000,
                            about ten minutes of game time)
  --sample <n>              record a position every n emulated frames
                            (default 12, so five video frames per game second)
  --seed <n>                root seed; every instance's is derived from it
  --out <dir>               write the composited PNG sequence here
  --maps <dir>              backgrounds produced by --capture-maps
  --capture-maps <dir>      instead of a swarm: walk one agent around and stitch
                            its own framebuffers into a background per map
  --capture-frames <n>      how long the capture walk runs (default 240000)
  --zoom                    start close on the swarm and pull back, at integer
                            scales only
  --scale <n>               integer magnification of the finished frame
  --verify-determinism      run the swarm twice and compare, then exit
  --quiet                   no progress line

Exit status: 0 fine, 2 an error.",
        n = hunt::default_instances()
    );
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        rom: PathBuf::new(),
        state: PathBuf::new(),
        instances: hunt::default_instances(),
        frames: 36_000,
        sample: 12,
        seed: 0x5EED_C0FF_EE15_600D,
        out: None,
        maps: None,
        capture_maps: None,
        capture_frames: 240_000,
        zoom: false,
        scale: 1,
        verify: false,
        quiet: false,
    };
    let mut i = 0;
    while i < args.len() {
        let flag = args[i].clone();
        let mut used = 0usize;
        let mut next = |what: &str| -> Result<String, String> {
            used = 1;
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match flag.as_str() {
            "--rom" => a.rom = next("--rom")?.into(),
            "--state" => a.state = next("--state")?.into(),
            "--instances" => {
                a.instances = next("--instances")?
                    .parse()
                    .map_err(|_| "bad --instances")?
            }
            "--frames" => {
                a.frames = next("--frames")?.parse().map_err(|_| "bad --frames")?
            }
            "--sample" => {
                a.sample = next("--sample")?.parse().map_err(|_| "bad --sample")?
            }
            "--seed" => a.seed = next("--seed")?.parse().map_err(|_| "bad --seed")?,
            "--out" => a.out = Some(next("--out")?.into()),
            "--maps" => a.maps = Some(next("--maps")?.into()),
            "--capture-maps" => a.capture_maps = Some(next("--capture-maps")?.into()),
            "--capture-frames" => {
                a.capture_frames = next("--capture-frames")?
                    .parse()
                    .map_err(|_| "bad --capture-frames")?
            }
            "--zoom" => a.zoom = true,
            "--scale" => a.scale = next("--scale")?.parse().map_err(|_| "bad --scale")?,
            "--verify-determinism" => a.verify = true,
            "--quiet" => a.quiet = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
        i += 1 + used;
    }
    if a.rom.as_os_str().is_empty() {
        return Err("--rom is required".into());
    }
    if a.state.as_os_str().is_empty() {
        return Err("--state is required".into());
    }
    if a.sample == 0 {
        return Err("--sample must be at least 1".into());
    }
    a.scale = a.scale.clamp(1, 8);
    a.instances = a.instances.max(1);
    Ok(a)
}

// ---------------------------------------------------------------------------
// One agent
// ---------------------------------------------------------------------------

/// Where one agent was at one sampling instant.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct Sample {
    map: u8,
    x: u8,
    y: u8,
    battling: bool,
}

/// A map transition an agent walked through: it was settled at `(from, fx, fy)`
/// and the next settled frame found it at `(to, tx, ty)`, having taken one step
/// in direction `step`.
///
/// **`step` is what makes the offset exact.** The player's world position is
/// continuous across a connection, so `to`'s origin is
/// `from`'s + (fx + step.0 − tx, fy + step.1 − ty) tiles — but only if you know
/// the crossing was a *single* step. Watching for the transition once per
/// *sample* rather than once per *frame* let the agent walk one to three tiles
/// into the new map first, and the resulting offset was short by however far it
/// got: measured, Pallet Town landed **two tiles** under Route 1's southern
/// edge, which quietly hid its top two rows and its label.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Seam {
    from: u8,
    fx: u8,
    fy: u8,
    to: u8,
    tx: u8,
    ty: u8,
    step: (i8, i8),
}

/// Everything one instance produced.
struct Trace {
    samples: Vec<Sample>,
    /// Map id to `(width, height)` in **blocks**, as the running engine
    /// reported it. Read from the cartridge rather than from a table, so a
    /// wrong map id cannot produce a plausible rectangle.
    sizes: BTreeMap<u8, (u8, u8)>,
    seams: Vec<Seam>,
    encounters: u64,
    blackouts: u64,
}

/// How many times the walker will press RUN before giving up on a battle. The
/// shiny hunter uses the same number for the same reason.
const FLEE_TRIES: u32 = 60;

/// The four headings, as unit steps in tiles. Index matches [`DIR_KEYS`].
const DIR_STEPS: [(i8, i8); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];
const DIR_KEYS: [KeypadKey; 4] = [
    KeypadKey::Up,
    KeypadKey::Down,
    KeypadKey::Left,
    KeypadKey::Right,
];

struct Agent {
    gb: Gameboy,
    sched: Schedule,
    frames: u64,
    sample_every: u32,
    trace: Trace,
    /// The last frame on which the agent was standing still on a fully loaded
    /// map, as `(map, x, y)`. The other half of a seam observation.
    settled: Option<(u8, u8, u8)>,
    /// The direction the walker is currently pushing, so a crossing knows which
    /// way the single step went.
    step: (i8, i8),
}

impl Agent {
    fn new(gb: Gameboy, seed: u64, sample_every: u32) -> Agent {
        Agent {
            gb,
            // Attempt 0: one instance, one run, one stream. Everything the
            // walker decides comes out of it.
            sched: Schedule::for_attempt(seed, 0),
            frames: 0,
            sample_every,
            trace: Trace {
                samples: Vec::new(),
                sizes: BTreeMap::new(),
                seams: Vec::new(),
                encounters: 0,
                blackouts: 0,
            },
            settled: None,
            step: (0, 0),
        }
    }

    /// One emulated frame, sampled on the instance's **own** frame counter.
    ///
    /// That is what makes the finished video independent of thread scheduling:
    /// instance *i*'s frame 1200 always lands in video frame 100, whichever
    /// thread got there first.
    fn tick(&mut self) {
        self.gb.frame();
        self.frames += 1;
        self.watch_for_a_seam();
        if self.frames % u64::from(self.sample_every) != 0 {
            return;
        }
        let battle = self.gb.peek(IS_IN_BATTLE);
        self.trace.samples.push(Sample {
            map: self.gb.peek(CUR_MAP),
            x: self.gb.peek(X_COORD),
            y: self.gb.peek(Y_COORD),
            battling: battle != 0,
        });
    }

    /// Notice a map change the frame the engine finishes it.
    ///
    /// **Watched every frame, not every sample**, so exactly one step separates
    /// the two sides — see [`Seam`]. "Settled" means the walk animation has
    /// finished (`wWalkCounter == 0`), the machine is out of battle, and the map
    /// header has been loaded (`wCurMapWidth != 0`); during `LoadMapData` the
    /// coordinates and the header disagree and an observation taken there is
    /// nonsense.
    fn watch_for_a_seam(&mut self) {
        if self.gb.peek(WALK_COUNTER) != 0 || self.gb.peek(IS_IN_BATTLE) != 0 {
            return;
        }
        let (w, h) = (self.gb.peek(MAP_WIDTH), self.gb.peek(MAP_HEIGHT));
        if w == 0 || h == 0 {
            return;
        }
        let map = self.gb.peek(CUR_MAP);
        let (x, y) = (self.gb.peek(X_COORD), self.gb.peek(Y_COORD));
        self.trace.sizes.insert(map, (w, h));
        if let Some((pm, px, py)) = self.settled {
            if pm != map {
                self.trace.seams.push(Seam {
                    from: pm,
                    fx: px,
                    fy: py,
                    to: map,
                    tx: x,
                    ty: y,
                    step: self.step,
                });
            }
        }
        self.settled = Some((map, x, y));
    }

    fn hold(&mut self, key: KeypadKey, n: u32, budget: &mut u64) {
        self.gb.key(key, true);
        for _ in 0..n {
            if *budget == 0 {
                break;
            }
            self.tick();
            *budget -= 1;
        }
        self.gb.key(key, false);
    }

    fn idle(&mut self, n: u32, budget: &mut u64) {
        for _ in 0..n {
            if *budget == 0 {
                return;
            }
            self.tick();
            *budget -= 1;
        }
    }

    fn tap(&mut self, key: KeypadKey, gap: u32, budget: &mut u64) {
        self.hold(key, 4, budget);
        self.idle(gap, budget);
    }

    /// Press RUN until the battle ends. Identical in shape to the hunter's
    /// `flee`, and for the same reason: an agent that cannot leave a battle
    /// stops being an agent and becomes a stationary dot.
    fn flee(&mut self, budget: &mut u64) {
        self.trace.encounters += 1;
        for _ in 0..FLEE_TRIES {
            if *budget == 0 {
                return;
            }
            for key in [KeypadKey::Right, KeypadKey::Down, KeypadKey::A] {
                self.tap(key, 14, budget);
            }
            // `$FF` is "the player blacked out" (`home/overworld.asm`
            // `.allPokemonFainted`), not a battle state. The game is about to
            // teleport the agent to a Pokemon Center; nothing here can stop
            // that, and it is worth counting because a swarm that quietly
            // drains into one building is a finding.
            if self.gb.peek(IS_IN_BATTLE) == 0xFF {
                self.trace.blackouts += 1;
                return;
            }
            if self.gb.peek(IS_IN_BATTLE) == 0 {
                for _ in 0..6 {
                    self.tap(KeypadKey::B, 30, budget);
                }
                return;
            }
        }
    }

    /// Walk until the frame budget runs out.
    ///
    /// The policy is a free four-direction explorer. Two details are not
    /// arbitrary. **A press in a direction the player is not already facing
    /// only turns the sprite**, so a walker that changes direction every step
    /// spends half its presses standing still; keeping the same heading until
    /// it stops working is what makes an agent cover ground. And the heading is
    /// abandoned after two motionless presses, because that is what walking
    /// into a wall, a ledge or an NPC looks like from here.
    fn walk(&mut self, mut budget: u64) {
        let mut heading = (self.sched.draw() % 4) as usize;
        let mut motionless = 0u32;
        self.idle(self.sched.lead_in, &mut budget);
        while budget > 0 {
            if self.gb.peek(IS_IN_BATTLE) != 0 {
                self.flee(&mut budget);
                continue;
            }
            let before = (self.gb.peek(X_COORD), self.gb.peek(Y_COORD));
            let (hold, gap) = self.sched.step();
            self.step = DIR_STEPS[heading];
            self.hold(DIR_KEYS[heading], hold, &mut budget);
            self.idle(gap, &mut budget);
            let after = (self.gb.peek(X_COORD), self.gb.peek(Y_COORD));
            if after == before {
                motionless += 1;
                if motionless >= 2 {
                    heading = (self.sched.draw() % 4) as usize;
                    motionless = 0;
                }
            } else {
                motionless = 0;
                // A short chance of changing course even when the way is
                // clear, so an agent is not a straight line until it hits
                // something. Drawn from the same stream as everything else.
                if self.sched.draw() % 5 == 0 {
                    heading = (self.sched.draw() % 4) as usize;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Building machines
// ---------------------------------------------------------------------------

fn build(rom: &[u8]) -> Result<Gameboy, String> {
    // No file path: nothing on the filesystem is opened, created or written by
    // an instance, so a swarm cannot touch a real save however many of them run.
    let mut gb =
        Gameboy::try_new_with_model(rom.to_vec(), None, terminalgb::mode::Model::Auto)
            .map_err(|e| e.to_string())?;
    // The hunter is attached only for its cartridge check: it refuses anything
    // that is not a verified Red/Blue, which is what stops this tool decoding
    // another game's WRAM as if it were a position.
    gb.add_plugin(Box::new(ShinyHunt::new(Target::shiny())))
        .map_err(|e| e.to_string())?;
    Ok(gb)
}

fn run_swarm(rom: &[u8], base: &[u8], a: &Args) -> Result<Vec<Trace>, String> {
    let cfg = {
        let mut c = HuntConfig::new(base.to_vec(), Target::shiny());
        c.seed = a.seed;
        c.instances = a.instances;
        c
    };
    let mut handles = Vec::new();
    for index in 0..a.instances {
        let rom = rom.to_vec();
        let base = base.to_vec();
        let seed = cfg.seed_for(index);
        let frames = a.frames;
        let sample = a.sample;
        handles.push(std::thread::spawn(move || -> Result<Trace, String> {
            let mut gb = build(&rom)?;
            HuntMachine::load_state(&mut gb, &base)
                .map_err(|e| format!("save state: {e}"))?;
            // `on_attach` runs on the first dispatch, so give it a frame before
            // asking whether the cartridge was recognised.
            HuntMachine::frame(&mut gb);
            if let Some(why) = HuntMachine::refusal(&gb) {
                return Err(why);
            }
            let mut agent = Agent::new(gb, seed, sample);
            agent.walk(frames);
            Ok(agent.trace)
        }));
    }
    let mut out = Vec::new();
    for h in handles {
        out.push(h.join().map_err(|_| "an instance panicked".to_string())??);
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Where the maps sit relative to each other
// ---------------------------------------------------------------------------

/// Map offsets in world pixels, inferred from agents crossing the seams.
///
/// **Measured, not tabulated.** A connection crossing preserves the player's
/// world position: if an agent was at `(fx, fy)` on map A and the next sample
/// found it at `(tx, ty)` on map B having moved a tile or two, then B's origin
/// is at `A_origin + (fx - tx, fy - ty)` tiles, give or take the step. Averaging
/// several crossings of the same seam takes the step out.
struct Layout {
    /// Map id to origin in world pixels.
    origin: BTreeMap<u8, (i64, i64)>,
    sizes: BTreeMap<u8, (u8, u8)>,
    /// Maps seen that could not be placed.
    gutter: Vec<u8>,
}

impl Layout {
    /// Build from every trace: the root map (the one the save state starts on)
    /// goes at the origin, and the rest are placed by breadth-first walk over
    /// the observed seams.
    fn build(traces: &[Trace], root: u8) -> Layout {
        let mut sizes: BTreeMap<u8, (u8, u8)> = BTreeMap::new();
        for t in traces {
            for (k, v) in &t.sizes {
                sizes.insert(*k, *v);
            }
        }
        // Average every observation of each ordered seam. `edge[(a, b)]` is
        // b's origin minus a's, in tiles.
        let mut edge: BTreeMap<(u8, u8), (i64, i64, i64)> = BTreeMap::new();
        for t in traces {
            for s in &t.seams {
                if !OUTDOOR.contains(&s.from) || !OUTDOOR.contains(&s.to) {
                    continue;
                }
                // The single step the agent took across the seam is added
                // back: world position is continuous, so the origin of `to`
                // is where `from`'s coordinates *would have* continued to.
                let dx = i64::from(s.fx) + i64::from(s.step.0) - i64::from(s.tx);
                let dy = i64::from(s.fy) + i64::from(s.step.1) - i64::from(s.ty);
                let e = edge.entry((s.from, s.to)).or_insert((0, 0, 0));
                e.0 += dx;
                e.1 += dy;
                e.2 += 1;
            }
        }
        let mut origin: BTreeMap<u8, (i64, i64)> = BTreeMap::new();
        origin.insert(root, (0, 0));
        // Breadth-first, repeated until nothing new is placed. Tiny graphs; a
        // fixed point is cheaper than a queue.
        loop {
            let mut grew = false;
            for (&(from, to), &(sx, sy, n)) in &edge {
                if n == 0 || origin.contains_key(&to) {
                    continue;
                }
                let Some(&(ox, oy)) = origin.get(&from) else {
                    continue;
                };
                let (dx, dy) = (sx / n, sy / n);
                origin.insert(to, (ox + dx * TILE_PX, oy + dy * TILE_PX));
                grew = true;
            }
            if !grew {
                break;
            }
        }
        let mut gutter: Vec<u8> = sizes
            .keys()
            .copied()
            .filter(|m| !origin.contains_key(m))
            .collect();
        gutter.sort_unstable();
        Layout {
            origin,
            sizes,
            gutter,
        }
    }

    fn size_px(&self, map: u8) -> (i64, i64) {
        let (w, h) = self.sizes.get(&map).copied().unwrap_or((10, 9));
        (i64::from(w) * 32, i64::from(h) * 32)
    }

    /// Bounding box of everything placed, in world pixels.
    fn bounds(&self) -> (i64, i64, i64, i64) {
        let mut b = (i64::MAX, i64::MAX, i64::MIN, i64::MIN);
        for (&m, &(ox, oy)) in &self.origin {
            let (w, h) = self.size_px(m);
            b.0 = b.0.min(ox);
            b.1 = b.1.min(oy);
            b.2 = b.2.max(ox + w);
            b.3 = b.3.max(oy + h);
        }
        if b.0 == i64::MAX {
            (0, 0, 320, 288)
        } else {
            b
        }
    }
}

// ---------------------------------------------------------------------------
// Map capture: stitching an agent's own framebuffers into a background
// ---------------------------------------------------------------------------

/// One map's canvas, built out of 160x144 framebuffers.
struct MapSheet {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
    /// Whether any frame has written a pixel here yet — so a later, worse view
    /// cannot overwrite a good one, and unvisited ground stays visibly empty.
    seen: Vec<bool>,
}

impl MapSheet {
    fn new(w: usize, h: usize) -> MapSheet {
        MapSheet {
            w,
            h,
            rgba: vec![0; w * h * 4],
            seen: vec![false; w * h],
        }
    }
}

/// The 16x24 box around screen pixel (64, 64) that the player's own sprite
/// occupies. Blitting it would stamp a trail of Reds across the finished map;
/// every one of those pixels is covered from some other viewpoint anyway.
fn is_player_box(sx: i64, sy: i64) -> bool {
    (PLAYER_SCREEN_PX..PLAYER_SCREEN_PX + 16).contains(&sx)
        && (PLAYER_SCREEN_PX - 8..PLAYER_SCREEN_PX + 16).contains(&sy)
}

fn capture_maps(rom: &[u8], base: &[u8], a: &Args, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut gb = build(rom)?;
    HuntMachine::load_state(&mut gb, base).map_err(|e| format!("save state: {e}"))?;
    HuntMachine::frame(&mut gb);
    if let Some(why) = HuntMachine::refusal(&gb) {
        return Err(why);
    }
    let mut agent = Agent::new(gb, a.seed, 1);
    let mut sheets: BTreeMap<u8, MapSheet> = BTreeMap::new();
    let mut budget = a.capture_frames;
    let started = Instant::now();

    let mut heading = 0usize;
    let mut motionless = 0u32;
    while budget > 0 {
        if agent.gb.peek(IS_IN_BATTLE) != 0 {
            agent.flee(&mut budget);
            continue;
        }
        let before = (agent.gb.peek(X_COORD), agent.gb.peek(Y_COORD));
        let (hold, gap) = agent.sched.step();
        agent.step = DIR_STEPS[heading];
        agent.hold(DIR_KEYS[heading], hold, &mut budget);
        agent.idle(gap, &mut budget);
        // Stitch here: the walker is between steps, so `wWalkCounter` has
        // reached 0 and the screen is on the tile grid.
        stitch(&mut agent.gb, &mut sheets);
        let after = (agent.gb.peek(X_COORD), agent.gb.peek(Y_COORD));
        let mut repick = false;
        if after == before {
            motionless += 1;
            if motionless >= 2 {
                repick = true;
                motionless = 0;
            }
        } else {
            motionless = 0;
            repick = agent.sched.draw() % 4 == 0;
        }
        if repick {
            heading = next_heading(&mut agent, &sheets);
        }
        if !a.quiet && budget % 20_000 < 200 {
            let done: usize = sheets
                .values()
                .map(|s| s.seen.iter().filter(|b| **b).count())
                .sum();
            let total: usize = sheets.values().map(|s| s.seen.len()).sum();
            eprint!(
                "\rcapture  {:>7} frames left  {} map(s)  {:.0}% covered   ",
                budget,
                sheets.len(),
                if total > 0 {
                    done as f64 * 100.0 / total as f64
                } else {
                    0.0
                }
            );
        }
    }
    if !a.quiet {
        eprintln!();
    }

    let mut index = String::new();
    for (id, sheet) in &sheets {
        let covered = sheet.seen.iter().filter(|b| **b).count();
        let file = format!("map_{id:03}.png");
        image::save_buffer(
            dir.join(&file),
            &sheet.rgba,
            sheet.w as u32,
            sheet.h as u32,
            image::ColorType::Rgba8,
        )
        .map_err(|e| format!("writing {file}: {e}"))?;
        index.push_str(&format!("{id} {} {} {file}\n", sheet.w, sheet.h));
        println!(
            "map {id:>3} {:<14} {}x{} px  {:.0}% captured  -> {file}",
            map_name(*id),
            sheet.w,
            sheet.h,
            covered as f64 * 100.0 / sheet.seen.len() as f64
        );
    }
    std::fs::write(dir.join("maps.txt"), index)
        .map_err(|e| format!("writing maps.txt: {e}"))?;
    println!(
        "captured {} map(s) in {:.1}s -> {}",
        sheets.len(),
        started.elapsed().as_secs_f64(),
        dir.display()
    );
    Ok(())
}

/// A heading that heads for ground nobody has photographed yet.
///
/// A plain random walk is a bad surveyor and Route 1 makes it worse: its ledges
/// are one-way south, so a wanderer drains downhill into Pallet Town and the
/// northern half of the route is never seen. Measured — 120,000 frames of
/// random walking covered **44 %** of Route 1 and 100 % of Pallet Town, which
/// is exactly the shape of that bias.
///
/// So three times in four the walker heads for the centroid of what is still
/// black on the current map's sheet, along whichever axis is further away; the
/// fourth draw is left random so it can still escape a dead end. Both draws
/// come from the schedule, so a capture is as reproducible as a swarm.
fn next_heading(agent: &mut Agent, sheets: &BTreeMap<u8, MapSheet>) -> usize {
    let random = (agent.sched.draw() % 4) as usize;
    if agent.sched.draw() % 4 == 0 {
        return random;
    }
    let map = agent.gb.peek(CUR_MAP);
    let Some(sheet) = sheets.get(&map) else {
        return random;
    };
    let (mut sx, mut sy, mut n) = (0u64, 0u64, 0u64);
    // Every eighth pixel in each axis: a centroid does not need every sample
    // and this runs between every step.
    for y in (0..sheet.h).step_by(8) {
        for x in (0..sheet.w).step_by(8) {
            if !sheet.seen[y * sheet.w + x] {
                sx += x as u64;
                sy += y as u64;
                n += 1;
            }
        }
    }
    if n == 0 {
        return random;
    }
    let (tx, ty) = ((sx / n) as i64, (sy / n) as i64);
    let px = i64::from(agent.gb.peek(X_COORD)) * TILE_PX;
    let py = i64::from(agent.gb.peek(Y_COORD)) * TILE_PX;
    let (dx, dy) = (tx - px, ty - py);
    if dy.abs() >= dx.abs() {
        if dy < 0 {
            0 // Up
        } else {
            1 // Down
        }
    } else if dx < 0 {
        2 // Left
    } else {
        3 // Right
    }
}

/// Blit the current framebuffer into the sheet for whatever map the agent is
/// standing on.
fn stitch(gb: &mut Gameboy, sheets: &mut BTreeMap<u8, MapSheet>) {
    if gb.peek(WALK_COUNTER) != 0 || gb.peek(IS_IN_BATTLE) != 0 {
        return;
    }
    let map = gb.peek(CUR_MAP);
    let (bw, bh) = (gb.peek(MAP_WIDTH), gb.peek(MAP_HEIGHT));
    if bw == 0 || bh == 0 {
        return;
    }
    let (mw, mh) = (usize::from(bw) * 32, usize::from(bh) * 32);
    let px = i64::from(gb.peek(X_COORD)) * TILE_PX - PLAYER_SCREEN_PX;
    let py = i64::from(gb.peek(Y_COORD)) * TILE_PX - PLAYER_SCREEN_PX;
    let sheet = sheets.entry(map).or_insert_with(|| MapSheet::new(mw, mh));
    if sheet.w != mw || sheet.h != mh {
        return;
    }
    let frame = gb.image();
    for sy in 0..GB_H as i64 {
        let wy = py + sy;
        if wy < 0 || wy >= sheet.h as i64 {
            continue;
        }
        for sx in 0..GB_W as i64 {
            let wx = px + sx;
            if wx < 0 || wx >= sheet.w as i64 {
                continue;
            }
            if is_player_box(sx, sy) {
                continue;
            }
            let di = wy as usize * sheet.w + wx as usize;
            if sheet.seen[di] {
                continue;
            }
            let si = (sy as usize * GB_W + sx as usize) * 4;
            sheet.rgba[di * 4..di * 4 + 4].copy_from_slice(&frame[si..si + 4]);
            sheet.seen[di] = true;
        }
    }
}

/// A captured background: width, height and RGBA pixels.
type Sheets = BTreeMap<u8, (usize, usize, Vec<u8>)>;

fn load_maps(dir: &Path) -> Result<Sheets, String> {
    let index = dir.join("maps.txt");
    let text = std::fs::read_to_string(&index)
        .map_err(|e| format!("{}: {e} (run --capture-maps first)", index.display()))?;
    let mut out = BTreeMap::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split_whitespace().collect();
        if f.len() != 4 {
            return Err(format!("maps.txt: cannot read `{line}`"));
        }
        let id: u8 = f[0].parse().map_err(|_| "maps.txt: bad map id")?;
        let img = image::open(dir.join(f[3]))
            .map_err(|e| format!("{}: {e}", f[3]))?
            .to_rgba8();
        out.insert(
            id,
            (img.width() as usize, img.height() as usize, img.into_raw()),
        );
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// The compositor
// ---------------------------------------------------------------------------

const BG: [u8; 4] = [12, 14, 18, 255];
const MAP_EMPTY: [u8; 4] = [30, 36, 44, 255];
const MAP_EDGE: [u8; 4] = [58, 70, 84, 255];
const INK: [u8; 4] = [190, 205, 215, 255];
const DIM_INK: [u8; 4] = [96, 110, 124, 255];

/// One colour per agent, from a golden-angle hue rotation so that adjacent
/// indices are far apart and 40 of them are still distinguishable.
fn agent_colour(i: usize) -> [u8; 4] {
    let h = (i as f64 * 137.507_764_05) % 360.0;
    let (s, v) = (0.80_f64, 1.0_f64);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
        255,
    ]
}

/// What the caption says about one video frame.
#[derive(Default)]
struct Tally {
    live: usize,
    battling: usize,
    elsewhere: usize,
    here: BTreeMap<u8, usize>,
}

struct Composer {
    /// World pixel of the canvas's top-left corner.
    ox: i64,
    oy: i64,
    w: usize,
    h: usize,
    /// The background, already laid out. Copied into each frame.
    background: Vec<u8>,
    layout: Layout,
}

const HEADER_H: usize = 30;

impl Composer {
    fn new(layout: Layout, sheets: &Sheets) -> Composer {
        let (x0, y0, x1, y1) = layout.bounds();
        let pad = 16i64;
        let ox = x0 - pad;
        let oy = y0 - pad - HEADER_H as i64;
        let w = (x1 - x0 + pad * 2) as usize;
        let h = (y1 - y0 + pad * 2) as usize + HEADER_H;
        let mut background = vec![0u8; w * h * 4];
        {
            let mut c = Canvas::new(&mut background, w as u32, h as u32);
            c.fill(0, 0, w as u32, h as u32, BG);
        }
        let mut comp = Composer {
            ox,
            oy,
            w,
            h,
            background,
            layout,
        };
        comp.paint_background(sheets);
        comp
    }

    fn paint_background(&mut self, sheets: &Sheets) {
        let origins: Vec<(u8, i64, i64)> = self
            .layout
            .origin
            .iter()
            .map(|(&m, &(x, y))| (m, x, y))
            .collect();
        for (map, wx, wy) in origins {
            let (mw, mh) = self.layout.size_px(map);
            let (cx, cy) = (wx - self.ox, wy - self.oy);
            match sheets.get(&map) {
                Some((sw, sh, rgba)) if *sw as i64 == mw && *sh as i64 == mh => {
                    for y in 0..*sh {
                        for x in 0..*sw {
                            let si = (y * sw + x) * 4;
                            // A never-captured pixel is transparent black in
                            // the sheet; leave the empty colour showing rather
                            // than punching a hole in the canvas.
                            let opaque = rgba[si + 3] != 0;
                            let px = if opaque {
                                // Dim the map so the markers are the brightest
                                // thing on screen. A background that competes
                                // with the swarm defeats the picture.
                                [
                                    (u32::from(rgba[si]) * 42 / 100) as u8,
                                    (u32::from(rgba[si + 1]) * 42 / 100) as u8,
                                    (u32::from(rgba[si + 2]) * 46 / 100) as u8,
                                    255,
                                ]
                            } else {
                                MAP_EMPTY
                            };
                            self.put(cx + x as i64, cy + y as i64, px);
                        }
                    }
                }
                _ => {
                    let mut c =
                        Canvas::new(&mut self.background, self.w as u32, self.h as u32);
                    c.fill(cx, cy, mw as u32, mh as u32, MAP_EMPTY);
                }
            }
            let mut c = Canvas::new(&mut self.background, self.w as u32, self.h as u32);
            c.fill(cx, cy, mw as u32, 1, MAP_EDGE);
            c.fill(cx, cy + mh - 1, mw as u32, 1, MAP_EDGE);
            c.fill(cx, cy, 1, mh as u32, MAP_EDGE);
            c.fill(cx + mw - 1, cy, 1, mh as u32, MAP_EDGE);
            c.text(cx + 4, cy + 4, map_name(map), 1, DIM_INK);
        }
    }

    #[inline]
    fn put(&mut self, x: i64, y: i64, c: [u8; 4]) {
        if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 4;
        self.background[i..i + 4].copy_from_slice(&c);
    }

    /// World pixel of the centre of a walk tile.
    fn world_of(&self, s: &Sample) -> Option<(i64, i64)> {
        let &(ox, oy) = self.layout.origin.get(&s.map)?;
        Some((
            ox + i64::from(s.x) * TILE_PX + TILE_PX / 2,
            oy + i64::from(s.y) * TILE_PX + TILE_PX / 2,
        ))
    }

    /// One video frame's **map**, with no caption on it.
    ///
    /// The header is drawn separately and last, because it must survive the
    /// zoom: a caption that scrolls off the top of a close-up is a caption
    /// nobody can read at the moment the picture is most interesting.
    fn body(&self, traces: &[Trace], k: usize) -> (Vec<u8>, Tally) {
        let mut buf = self.background.clone();
        let mut tally = Tally::default();
        for (i, t) in traces.iter().enumerate() {
            let Some(s) = t.samples.get(k) else { continue };
            tally.live += 1;
            if s.battling {
                tally.battling += 1;
            }
            *tally.here.entry(s.map).or_default() += 1;
            let Some((wx, wy)) = self.world_of(s) else {
                tally.elsewhere += 1;
                continue;
            };
            self.marker(
                &mut buf,
                wx - self.ox,
                wy - self.oy,
                agent_colour(i),
                s.battling,
            );
        }
        (buf, tally)
    }

    /// A disc, blended so that a pile of agents on one tile reads as brighter
    /// than a lone one. **That saturation is the whole diagnostic**: you are
    /// meant to be able to see the corner they all walked into.
    fn marker(&self, buf: &mut [u8], cx: i64, cy: i64, c: [u8; 4], battling: bool) {
        let r: i64 = if battling { 6 } else { 5 };
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dy * dy;
                if d2 > r * r {
                    continue;
                }
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= self.w as i64 || y >= self.h as i64 {
                    continue;
                }
                // A battling agent is drawn as a ring, so a stopped dot is
                // distinguishable from a walking one at a glance.
                if battling && d2 < (r - 2) * (r - 2) {
                    continue;
                }
                let i = (y as usize * self.w + x as usize) * 4;
                for ch in 0..3 {
                    let old = u32::from(buf[i + ch]);
                    let new = u32::from(c[ch]);
                    buf[i + ch] = (new + old / 3).min(255) as u8;
                }
                buf[i + 3] = 255;
            }
        }
    }

    fn header(&self, buf: &mut [u8], k: usize, total: usize, sample: u32, t: &Tally) {
        let (live, battling, elsewhere, here) =
            (t.live, t.battling, t.elsewhere, &t.here);
        use terminalgb::video::card::{fit_scale, text_width};
        let mut c = Canvas::new(buf, self.w as u32, self.h as u32);
        c.fill(0, 0, self.w as u32, HEADER_H as u32, BG);
        c.fill(0, HEADER_H as i64 - 1, self.w as u32, 1, MAP_EDGE);
        let secs = (k as f64 * f64::from(sample)) / 59.7275;
        // The card font has letters, digits, `-`, `.`, `:` and `/` and draws
        // anything else as a blank — so no `+` in a caption, ever.
        let top = format!(
            "{live} AGENTS   {:02}:{:02}   {battling} IN BATTLE",
            (secs as u64) / 60,
            (secs as u64) % 60
        );
        // **Fit the caption, never truncate it.** A clipped count is
        // indistinguishable from a complete one, and the whole header exists to
        // be read off a video frame.
        c.text(6, 4, &top, fit_scale(&top, self.w as u32 - 12).min(2), INK);
        let mut tally: Vec<String> = here
            .iter()
            .map(|(m, n)| format!("{}:{n}", map_name(*m)))
            .collect();
        if elsewhere > 0 {
            tally.push(format!("UNPLACED:{elsewhere}"));
        }
        let mut line = tally.join("  ");
        while text_width(&line, 1) > self.w as u32 - 12 && line.len() > 3 {
            // Drop the least-populated map rather than cutting a word in half.
            tally.pop();
            line = format!("{}  ...", tally.join("  "));
        }
        c.text(6, HEADER_H as i64 - 12, &line, 1, DIM_INK);
        // Progress bar along the bottom of the header.
        let done = if total > 1 {
            (self.w - 2) * k / (total - 1)
        } else {
            0
        };
        c.fill(1, HEADER_H as i64 - 4, done as u32, 2, DIM_INK);
    }
}

/// Nearest-neighbour crop-and-magnify into a fixed output size.
///
/// **Integer scales only, and no filtering at all**: the whole value of this
/// picture is that every colour on it means something, so a smoothed zoom that
/// invents a shade between two agents' colours is a bug. Verified rather than
/// asserted — see [`assert_no_new_colours`].
fn zoom_into(
    src: &[u8],
    sw: usize,
    sh: usize,
    scale: usize,
    cx: i64,
    cy: i64,
) -> Vec<u8> {
    let mut out = vec![0u8; sw * sh * 4];
    let (vw, vh) = (sw / scale, sh / scale);
    let x0 = (cx - vw as i64 / 2).clamp(0, (sw as i64 - vw as i64).max(0));
    let y0 = (cy - vh as i64 / 2).clamp(0, (sh as i64 - vh as i64).max(0));
    for y in 0..sh {
        let sy = y0 + (y / scale) as i64;
        for x in 0..sw {
            let sx = x0 + (x / scale) as i64;
            let di = (y * sw + x) * 4;
            if sx < 0 || sy < 0 || sx >= sw as i64 || sy >= sh as i64 {
                out[di..di + 4].copy_from_slice(&BG);
                continue;
            }
            let si = (sy as usize * sw + sx as usize) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

/// Integer upscale of the finished frame. Same rule, same reason.
fn magnify(src: &[u8], w: usize, h: usize, scale: usize) -> Vec<u8> {
    if scale <= 1 {
        return src.to_vec();
    }
    let mut out = vec![0u8; w * scale * h * scale * 4];
    for y in 0..h * scale {
        for x in 0..w * scale {
            let si = ((y / scale) * w + x / scale) * 4;
            let di = (y * w * scale + x) * 4;
            out[di..di + 4].copy_from_slice(&src[si..si + 4]);
        }
    }
    out
}

/// **The categorical guard.** After any scaling step every colour in the output
/// must already have existed in the input. A count is the right test rather
/// than a pixel diff: an interpolating resize invents shades, and "a colour
/// that was not there" is a yes/no question with no threshold to tune.
fn assert_no_new_colours(src: &[u8], out: &[u8]) -> Result<(), String> {
    use std::collections::HashSet;
    let set: HashSet<[u8; 4]> = src
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect();
    for p in out.chunks_exact(4) {
        let c = [p[0], p[1], p[2], p[3]];
        if !set.contains(&c) {
            return Err(format!(
                "scaling invented the colour {c:?}, which is not in the source — \
                 the resize is smoothing when it must not"
            ));
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Reporting
// ---------------------------------------------------------------------------

/// The finding the picture is for. Where did they end up?
fn report(traces: &[Trace], layout: &Layout, sample: u32) {
    let n = traces.len();
    let frames: u64 =
        traces.iter().map(|t| t.samples.len() as u64).sum::<u64>() * u64::from(sample);
    println!();
    println!("swarm");
    println!("  instances        {n}");
    println!(
        "  emulated frames  {frames} ({:.1} minutes of game time in total)",
        frames as f64 / 59.7275 / 60.0
    );
    println!(
        "  encounters       {}   blackouts {}",
        traces.iter().map(|t| t.encounters).sum::<u64>(),
        traces.iter().map(|t| t.blackouts).sum::<u64>()
    );

    // Divergence, shown rather than asserted: the first sample at which any two
    // instances disagree, and how many distinct paths there are by the end.
    let len = traces.iter().map(|t| t.samples.len()).min().unwrap_or(0);
    let mut first_split = None;
    for k in 0..len {
        let a = traces[0].samples[k];
        if traces.iter().any(|t| t.samples[k] != a) {
            first_split = Some(k);
            break;
        }
    }
    let distinct_final: std::collections::HashSet<Sample> = traces
        .iter()
        .filter_map(|t| t.samples.last().copied())
        .collect();
    let distinct_paths: std::collections::HashSet<Vec<Sample>> =
        traces.iter().map(|t| t.samples.clone()).collect();
    println!();
    println!("divergence");
    match first_split {
        Some(k) => println!(
            "  first disagreement at sample {k} (emulated frame {})",
            k as u64 * u64::from(sample)
        ),
        None => println!("  NONE — every instance walked the identical path"),
    }
    println!("  distinct paths   {} of {n}", distinct_paths.len());
    println!("  distinct finishing positions  {}", distinct_final.len());

    // Occupancy: the point of the picture.
    let mut per_map: BTreeMap<u8, u64> = BTreeMap::new();
    let mut per_tile: BTreeMap<(u8, u8, u8), u64> = BTreeMap::new();
    let mut battling = 0u64;
    let mut total = 0u64;
    for t in traces {
        for s in &t.samples {
            *per_map.entry(s.map).or_default() += 1;
            *per_tile.entry((s.map, s.x, s.y)).or_default() += 1;
            total += 1;
            if s.battling {
                battling += 1;
            }
        }
    }
    println!();
    println!("where they went  (share of all agent-samples)");
    let mut maps: Vec<(u8, u64)> = per_map.into_iter().collect();
    maps.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    for (m, c) in maps.iter().take(8) {
        println!(
            "  {:<14} {:>6.2}%{}",
            map_name(*m),
            *c as f64 * 100.0 / total as f64,
            if layout.origin.contains_key(m) {
                ""
            } else {
                "   (unplaced — drawn in the gutter)"
            }
        );
    }
    if !layout.gutter.is_empty() {
        println!(
            "  {} map(s) could not be placed: {:?}",
            layout.gutter.len(),
            layout.gutter
        );
    }
    println!(
        "  in battle      {:>6.2}%",
        battling as f64 * 100.0 / total as f64
    );

    let mut tiles: Vec<((u8, u8, u8), u64)> = per_tile.into_iter().collect();
    tiles.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!();
    println!("clustering  (the busiest tiles, share of all agent-samples)");
    for ((m, x, y), c) in tiles.iter().take(6) {
        println!(
            "  {:<14} ({x:>3},{y:>3})  {:>6.2}%",
            map_name(*m),
            *c as f64 * 100.0 / total as f64
        );
    }
    let occupied = tiles.len();
    println!(
        "  {occupied} distinct tiles visited; the busiest {} hold {:.0}% of all samples",
        (occupied / 20).max(1),
        tiles
            .iter()
            .take((occupied / 20).max(1))
            .map(|(_, c)| *c)
            .sum::<u64>() as f64
            * 100.0
            / total as f64
    );
}

// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        usage();
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("swarm_view: {e}");
        std::process::exit(2);
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let a = parse(args)?;
    let rom = std::fs::read(&a.rom).map_err(|e| format!("{}: {e}", a.rom.display()))?;
    let base =
        std::fs::read(&a.state).map_err(|e| format!("{}: {e}", a.state.display()))?;

    if let Some(dir) = a.capture_maps.clone() {
        return capture_maps(&rom, &base, &a, &dir);
    }

    println!(
        "cartridge   {} ({} KiB)",
        a.rom.file_name().unwrap_or_default().to_string_lossy(),
        rom.len() / 1024
    );
    println!(
        "state       {}  ({} bytes)",
        a.state.file_name().unwrap_or_default().to_string_lossy(),
        base.len()
    );
    println!(
        "swarm       {} instances x {} frames, sampled every {}",
        a.instances, a.frames, a.sample
    );
    println!("seed        {:#018x}", a.seed);

    let started = Instant::now();
    let traces = run_swarm(&rom, &base, &a)?;
    let wall = started.elapsed();
    let emulated: u64 = a.frames * a.instances as u64;
    println!(
        "ran         {:.1}s wall, {} emulated frames, {:.0}x real time",
        wall.as_secs_f64(),
        emulated,
        emulated as f64 / 59.7275 / wall.as_secs_f64().max(1e-9)
    );

    if a.verify {
        println!("re-running for the determinism check...");
        let again = run_swarm(&rom, &base, &a)?;
        let same = traces.len() == again.len()
            && traces
                .iter()
                .zip(&again)
                .all(|(x, y)| x.samples == y.samples);
        println!(
            "determinism  {}",
            if same {
                "IDENTICAL — same seed, same save, same samples"
            } else {
                "DIFFERED — this is a bug"
            }
        );
        if !same {
            return Err("the swarm is not reproducible".into());
        }
    }

    let root = traces
        .first()
        .and_then(|t| t.samples.first())
        .map(|s| s.map)
        .unwrap_or(12);
    let layout = Layout::build(&traces, root);
    report(&traces, &layout, a.sample);

    let Some(out) = a.out.clone() else {
        println!();
        println!("no --out given, so nothing was drawn.");
        return Ok(());
    };

    let sheets = match &a.maps {
        Some(dir) => load_maps(dir)?,
        None => BTreeMap::new(),
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let comp = Composer::new(layout, &sheets);
    let total = traces.iter().map(|t| t.samples.len()).max().unwrap_or(0);
    println!();
    println!(
        "drawing     {total} frames at {}x{}{}",
        comp.w * a.scale as usize,
        comp.h * a.scale as usize,
        if a.zoom { ", zooming out" } else { "" }
    );

    // The zoom, if asked for: hold the tightest integer scale that still shows
    // the swarm, then step down one scale at a time over the first third of the
    // run until the whole canvas is in frame.
    let max_scale = 4usize;
    let zoom_until = total / 3;
    let mut checked = false;
    for k in 0..total {
        let (mut frame, tally) = comp.body(&traces, k);
        if a.zoom {
            let s = if k >= zoom_until || zoom_until == 0 {
                1
            } else {
                let t = k as f64 / zoom_until as f64;
                (max_scale as f64 - t * (max_scale as f64 - 1.0)).round() as usize
            }
            .clamp(1, max_scale);
            if s > 1 {
                // Centre on the swarm so the close-up is of the agents rather
                // than of an arbitrary corner.
                let (mut sx, mut sy, mut n) = (0i64, 0i64, 0i64);
                for t in &traces {
                    if let Some(p) = t.samples.get(k).and_then(|p| comp.world_of(p)) {
                        sx += p.0 - comp.ox;
                        sy += p.1 - comp.oy;
                        n += 1;
                    }
                }
                let (cx, cy) = if n > 0 {
                    (sx / n, sy / n)
                } else {
                    (comp.w as i64 / 2, comp.h as i64 / 2)
                };
                let zoomed = zoom_into(&frame, comp.w, comp.h, s, cx, cy);
                if !checked {
                    assert_no_new_colours(&frame, &zoomed)?;
                    checked = true;
                }
                frame = zoomed;
            }
        }
        // The caption goes on last, so it is the same size and in the same
        // place whatever the zoom is doing underneath it.
        comp.header(&mut frame, k, total, a.sample, &tally);
        let (fw, fh) = (comp.w, comp.h);
        let scaled = magnify(&frame, fw, fh, a.scale as usize);
        if a.scale > 1 && k == 0 {
            assert_no_new_colours(&frame, &scaled)?;
        }
        let path = out.join(format!("frame_{k:06}.png"));
        image::save_buffer(
            &path,
            &scaled,
            (fw * a.scale as usize) as u32,
            (fh * a.scale as usize) as u32,
            image::ColorType::Rgba8,
        )
        .map_err(|e| format!("{}: {e}", path.display()))?;
        if !a.quiet && k % 50 == 0 {
            eprint!("\rframe {k}/{total}   ");
        }
    }
    if !a.quiet {
        eprintln!();
    }

    // 59.7275 / sample is the true rate the samples were taken at, so playing
    // them back at that rate would be real time. 30 fps is what a video wants;
    // say out loud what that does to the clock rather than leaving the viewer
    // to guess how fast the swarm is really moving.
    let real_fps = 59.7275 / f64::from(a.sample);
    const PLAY_FPS: f64 = 30.0;
    println!();
    println!("wrote {total} frames to {}", out.display());
    println!("turn them into a video with:");
    println!(
        "  ffmpeg -y -framerate {PLAY_FPS} -i {}/frame_%06d.png \\\n\
         \x20   -vf 'scale=trunc(iw/2)*2:trunc(ih/2)*2' -c:v libx264 -pix_fmt yuv420p \\\n\
         \x20   -crf 18 swarm.mp4",
        out.display()
    );
    println!(
        "  {:.0}s of video, {:.0}x game speed ({:.1} emulated minutes per agent)",
        total as f64 / PLAY_FPS,
        PLAY_FPS / real_fps,
        a.frames as f64 / 59.7275 / 60.0
    );
    // Fedora (and anything else that ships a patent-clean ffmpeg) has no
    // libx264. Say so here rather than leaving a stranger with `Unknown
    // encoder`, which reads as a broken command rather than a missing codec.
    println!("  no libx264? swap `-c:v libx264 -crf 18` for `-c:v libopenh264 -b:v 4M`");
    Ok(())
}

//! The hunt driver: N headless emulators, one encounter loop each, stop on a
//! hit.
//!
//! # Why the driver is not the plugin
//!
//! A [`crate::ShinyHunt`] can *watch*. It cannot press a button on a schedule,
//! step a frame, load a save state or start a second emulator — the plugin API
//! has no verb for any of those, by design: a plugin is a passenger on a machine
//! somebody else is driving, and every callback it gets is a borrow that ends
//! when the callback does.
//!
//! Hunting needs a driver, so this module is one. It is written against
//! [`HuntMachine`], a seam of eight methods, which the
//! emulator implements for its `Gameboy` (behind the `plugin-gen1-shinyhunt`
//! feature). That is the same move [`terminalgb_plugin_api::MachineAccess`] makes
//! in the other direction, and it buys three things: the plugin crate never
//! names `gameboy` (so the dependency graph stays acyclic — the emulator depends
//! on its plugins, never the reverse); the whole loop is unit-testable against a
//! fake machine with no ROM and no emulator in CI; and a third-party host can
//! drive a hunt by implementing eight methods.
//!
//! The split is reported as a **finding about the plugin API**, not a complaint
//! about it: see `docs/shiny-hunt.md` §"What the plugin API could not do".
//!
//! # The loop
//!
//! Per instance, with no shared state at all except the stop flag and the
//! counters:
//!
//! 1. `load_state(base)` — the same starting position every attempt. This also
//!    resets the plugin latch, because the host resets plugin state on every
//!    load (`docs/plugins.md`: plugin state is not machine state).
//! 2. Drive input on a schedule derived from `(instance seed, attempt number)`.
//! 3. The plugin latches the encounter the moment the game finishes generating
//!    it; read the DVs off it.
//! 4. No match — go round again with the next attempt number.
//!
//! # Decorrelation: the problem that makes this interesting
//!
//! Emulation is deterministic and every instance starts from the same bytes, so
//! **identical input produces an identical Pokémon**: four instances doing the
//! same thing are one instance wearing four hats. That is the software version
//! of the desync problem the four-DS rig has, inverted — the splitter's consoles
//! drift *apart* until they stop agreeing; ours never drift at all.
//!
//! Gen 1's `Random_` (`pokered/home/random.asm`) folds `rDIV` — the
//! free-running divider — into its state on every call, and `rDIV` advances with
//! cycles. So the lever is **time**: change how many frames pass before each
//! press and every subsequent roll changes. Each attempt therefore gets its own
//! lead-in delay and its own per-step hold jitter, both drawn from a small
//! deterministic PRNG seeded by `(instance seed, attempt number)`.
//!
//! Two consequences worth stating plainly:
//!
//! * Divergence is **proved, not assumed**: [`probe`] runs the first few
//!   attempts of every instance and hands back the DV sequences, and the control
//!   case (every instance given the same seed) must come back *identical*. Both
//!   directions matter — a decorrelation that "worked" because the emulator was
//!   non-deterministic would be a far worse bug than no decorrelation at all.
//! * A hit is **reproducible from three numbers**: the base state, the instance
//!   seed and the attempt index. Nothing else feeds the schedule, so replaying
//!   it lands on the same Pokémon, and [`Hunt::run`] verifies exactly that in a
//!   fresh machine before reporting a hit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use terminalgb_plugin_api::KeypadKey;

use crate::{Sighting, Target, Watch, SETTLE_FRAMES};

/// `wCurMap` — `pokered` `ram/wram.asm`.
pub const CUR_MAP: u16 = 0xD35E;
/// `wYCoord`, the player's position within the current map.
pub const Y_COORD: u16 = 0xD361;
/// `wXCoord`.
pub const X_COORD: u16 = 0xD362;
/// `wIsInBattle` — 0 none, 1 wild, 2 trainer, `$FF` blacked out.
pub const IS_IN_BATTLE: u16 = 0xD057;

/// **What a host must provide for a hunt to run on it.**
///
/// Eight methods, all of which the emulator already had in public form; the
/// only thing this trait adds is a way to reach the attached
/// [`crate::ShinyHunt`]'s latch, because a plugin has no way to hand a value
/// back to whoever is driving.
///
/// `Send` because [`Hunt::run`] moves one machine onto each worker thread. It
/// is deliberately not `Sync`: a machine belongs to exactly one thread for its
/// whole life, and nothing here shares one.
pub trait HuntMachine: Send {
    /// Restore the machine — which also resets attached plugins, and therefore
    /// the latch.
    fn load_state(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// The machine's full state right now.
    fn save_state(&self) -> Vec<u8>;
    /// Run one emulated frame.
    fn frame(&mut self);
    /// Hold or release a button.
    fn key(&mut self, key: KeypadKey, down: bool);
    /// One byte of emulated address space, without ticking the bus.
    fn peek(&mut self, address: u16) -> u8;
    /// The attached hunter's latched encounter, if it has one.
    fn sighting(&self) -> Option<Sighting>;
    /// Clear the attached hunter's latch.
    fn arm(&mut self);
    /// `Some(reason)` when this machine cannot be hunted on — no hunter
    /// attached, or a cartridge whose RAM layout is not verified. Refusing is
    /// the whole point: decoding another game's memory as if it were Red/Blue
    /// produces confident nonsense.
    fn refusal(&self) -> Option<String>;
}

/// How the driver provokes an encounter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Shape {
    /// **Wild grass.** Pace back and forth across a band of tall grass until
    /// the step counter rolls an encounter. The base save state must be
    /// standing *in* grass on a map with an encounter table.
    #[default]
    Grass,
    /// **A fixed encounter reached by reset** — a static legendary, a gift, an
    /// in-game trade. The base save state must be positioned so that pressing A
    /// starts the sequence that generates the Pokémon.
    Reset,
}

impl Shape {
    pub fn parse(s: &str) -> Option<Shape> {
        Some(match s {
            "grass" | "wild" => Shape::Grass,
            "reset" | "static" | "gift" => Shape::Reset,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Shape::Grass => "wild grass",
            Shape::Reset => "soft reset",
        }
    }
}

/// Everything a hunt needs, apart from the machines themselves.
#[derive(Clone)]
pub struct HuntConfig {
    /// The save state every attempt restarts from.
    pub base_state: Vec<u8>,
    pub target: Target,
    pub shape: Shape,
    pub watch: Watch,
    /// Number of emulator instances. See [`default_instances`].
    pub instances: usize,
    /// Root seed. Every instance's seed is derived from it and the instance
    /// index, so `(base state, seed, instance, attempt)` names one Pokémon
    /// exactly and for ever.
    ///
    /// **It does not make a whole hunt deterministic, and claiming otherwise
    /// would be wrong**: which instance reaches a hit *first* is a race between
    /// threads, so two runs of the same hunt can stop on different Pokémon.
    /// What is reproducible is each attempt — which is the property a hit
    /// report needs, and it is checked before one is claimed.
    pub seed: u64,
    /// When false every instance uses the *same* schedule — the control case
    /// that proves decorrelation is doing the work. Never hunt with it off.
    pub decorrelate: bool,
    pub max_attempts: Option<u64>,
    pub time_limit: Option<Duration>,
    /// Emulated frames an attempt may burn before it is abandoned. Guards
    /// against a base state that never produces an encounter.
    pub max_frames_per_attempt: u32,
    /// Half-height of the grass band paced in [`Shape::Grass`], in tiles.
    pub grass_band: u8,
    /// How many encounters a single reload is run through, fleeing between
    /// them. **This is what lets a wild hunt reach more than a thin band of the
    /// DV space** — see [`Instance::attempt`]. One means "reload every time",
    /// which is the soft-reset method and is correct for [`Shape::Reset`].
    pub run_length: u32,
}

impl HuntConfig {
    pub fn new(base_state: Vec<u8>, target: Target) -> HuntConfig {
        HuntConfig {
            base_state,
            target,
            shape: Shape::Grass,
            watch: Watch::Encounter,
            instances: default_instances(),
            seed: 0x5EED_C0FF_EE15_600D,
            decorrelate: true,
            max_attempts: None,
            time_limit: None,
            max_frames_per_attempt: 2400,
            // One tile either way. Route 1's grass at the southern end is four
            // tiles deep (blocks `0B` in rows 16-17 of `maps/Route1.blk`), and
            // `examples/shiny_hunt.rs --make-state` parks the player in the
            // middle of it, so ±1 never leaves the grass. A wider band on a
            // narrow patch spends most of its steps outside it, rolling nothing.
            grass_band: 1,
            run_length: 32,
        }
    }

    /// The seed instance `index` actually runs on.
    pub fn seed_for(&self, index: usize) -> u64 {
        if self.decorrelate {
            mix(self.seed ^ (index as u64).wrapping_mul(0xA24B_AED4_963E_E407))
        } else {
            self.seed
        }
    }
}

/// One emulator per core, minus one for everything else on the machine.
///
/// The useful number is bounded by cores, not by imagination: every instance is
/// a full emulated Game Boy running flat out, so beyond one per core they take
/// turns and the aggregate encounter rate stops rising. Leaving one core free
/// keeps the progress display responsive and the box usable while a hunt runs
/// overnight.
pub fn default_instances() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// How many times the driver will press RUN before giving up on a battle.
const FLEE_TRIES: u32 = 60;

/// The narrowest press the walker will use, and how far above it the range
/// reaches.
///
/// **The floor is not "long enough for a step", it is "long enough for a turn
/// *and* a step".** A press in a direction the player is not already facing
/// spends itself turning the sprite, and a walk that paces a narrow band changes
/// direction constantly — so with a floor of 8 frames roughly half of all
/// presses moved nothing, the walk covered a fraction of the ground it should
/// have, and **71 of 72 runs ended by stalling** instead of meeting their quota
/// of Pokémon. Measured, and from the outside it looked exactly like a bad
/// encounter rate.
///
/// The ceiling deliberately reaches past a second step (a step is 16 frames and
/// the game commits to one once begun), so a long press moves two or three
/// tiles. That is what makes the *number* of steps vary as well as their timing,
/// which is half of what buys DV coverage — see [`Schedule`].
/// Frames one Gen 1 walking step takes.
const STEP_FRAMES: u32 = 16;

const HOLD_MIN: u64 = 20;
const HOLD_SPAN: u64 = 29; // 20..=48

/// A deterministic per-attempt input plan.
///
/// This is the decorrelation lever, and everything the driver does about timing
/// comes from here — so a hit is reproducible from the seed and the attempt
/// number alone.
///
/// # Why every step is drawn separately, and why that is the whole design
///
/// The first version drew one `hold` and one `gap` per attempt and jittered them
/// by 0-3 frames per step. Instances diverged from each other perfectly well…
/// and the rig as a whole could only ever produce **968 of the 65,536 DV
/// spreads**, measured (see [`Coverage`]). A 97,212-encounter hunt for a 1-in-
/// 8,192 target found nothing, because none of the eight spreads it wanted was
/// inside that set.
///
/// The reason is worth understanding before touching this again. Both halves of
/// Gen 1's `Random_` are functions of elapsed time: `rDIV` advances 18.3125
/// counts per frame, and the accumulator `hRandomAdd` grows by whatever `rDIV`
/// read at each call. When every knob is a *whole-attempt* constant, every step
/// time is `lead_in + i·cadence + Σ jitter`, so the state at the roll is a
/// function of two scalars — total frames and number of steps — and a
/// two-dimensional family cannot cover a two-byte space however many instances
/// draw from it.
///
/// Drawing `hold` and `gap` **independently for every step**, over ranges wide
/// enough to change the step count as well as its phase, makes the individual
/// step times matter rather than only their sum. That is what buys coverage, and
/// `--probe` measures it rather than assuming it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Frames to idle before touching anything, which shifts `rDIV` at every
    /// subsequent `Random_` call.
    pub lead_in: u32,
    state: u64,
}

impl Schedule {
    pub fn for_attempt(seed: u64, attempt: u64) -> Schedule {
        let mut state = mix(seed ^ attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lead_in = (next(&mut state) % 240) as u32;
        Schedule { lead_in, state }
    }

    /// The next step's `(hold, gap)`, drawn fresh. See the type's docs for why
    /// this is per step rather than per attempt.
    fn step(&mut self) -> (u32, u32) {
        let hold = (HOLD_MIN + next(&mut self.state) % HOLD_SPAN) as u32;
        let gap = (next(&mut self.state) % 25) as u32;
        (hold, gap)
    }
}

/// How much of the DV space a hunt can actually reach.
///
/// **The number that decides whether a hunt can ever succeed**, and it is not
/// the same question as "do the instances diverge". Instances can differ from
/// one another perfectly while the *union* of what they produce is a handful of
/// spreads — that is exactly the state this tool was in when it was first
/// measured, and a hunt in that state runs for ever and finds nothing, reporting
/// healthy encounter rates the whole time.
///
/// The estimate inverts `distinct = S · (1 − e^(−n/S))`, the expected number of
/// distinct values after `n` uniform draws from a set of size `S`. It is a
/// *coverage* estimate, not a uniformity test: it says how many spreads are
/// reachable, not whether they are equally likely.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coverage {
    /// Encounters sampled.
    pub samples: usize,
    /// Distinct `(wEnemyMonDVs, wEnemyMonDVs+1)` pairs among them.
    pub distinct: usize,
    /// Estimated size of the reachable set, out of 65,536.
    pub reachable: f64,
}

impl Coverage {
    /// Build from a sample count and a distinct count.
    pub fn from_counts(samples: usize, distinct: usize) -> Coverage {
        let reachable = estimate_reachable(samples, distinct);
        Coverage {
            samples,
            distinct,
            reachable,
        }
    }

    /// Expected number of the eight shiny-if-traded spreads inside a reachable
    /// set this size. **Below 1 means the hunt very probably cannot win at all**,
    /// however long it is left running.
    pub fn expected_shiny_spreads(&self) -> f64 {
        8.0 * self.reachable / 65536.0
    }

    /// A verdict a person can act on.
    pub fn verdict(&self) -> &'static str {
        if self.samples < 200 {
            "too few samples to judge — run --probe with more attempts"
        } else if self.reachable >= 30_000.0 {
            "good — most of the 65,536 spreads are reachable"
        } else if self.expected_shiny_spreads() >= 3.0 {
            "usable, but the reachable set is well short of the whole space"
        } else {
            "TOO NARROW — a shiny-if-traded spread may not be reachable at all \
             from this base state, so a hunt could run for ever and find nothing"
        }
    }
}

/// Invert `distinct = S · (1 − e^(−n/S))` for `S`, by bisection.
///
/// Monotone in `S`, so bisection is exact enough and needs no dependency.
fn estimate_reachable(samples: usize, distinct: usize) -> f64 {
    if samples == 0 || distinct == 0 {
        return 0.0;
    }
    if distinct >= samples {
        // Every draw was new: the sample says only "at least this many", and
        // pretending to a number would be inventing one.
        return f64::INFINITY;
    }
    let (n, d) = (samples as f64, distinct as f64);
    let (mut lo, mut hi) = (d, 1e9);
    for _ in 0..200 {
        let mid = (lo + hi) / 2.0;
        if mid * (1.0 - (-n / mid).exp()) < d {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    lo
}

/// Measure coverage over the sightings [`probe`] produced.
pub fn coverage(seqs: &[ProbedRuns]) -> Coverage {
    let mut samples = 0usize;
    let mut seen = std::collections::HashSet::new();
    for s in seqs.iter().flatten().flatten() {
        samples += 1;
        seen.insert((s.dvs.attack, s.dvs.defense, s.dvs.speed, s.dvs.special));
    }
    Coverage::from_counts(samples, seen.len())
}

fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One headless emulator running its own encounter loop.
pub struct Instance<M: HuntMachine> {
    gb: M,
    base: Vec<u8>,
    index: usize,
    seed: u64,
    shape: Shape,
    max_frames: u32,
    band: u8,
    run_length: u32,
    target: Target,
    /// The `wYCoord` the grass band is centred on, fixed for the whole run.
    ///
    /// Re-reading it after every battle would let the band *walk*: fleeing
    /// leaves the player wherever the encounter happened, and a band that
    /// follows drifts out of the grass over a long run and the hunt quietly
    /// stops rolling anything.
    band_centre: Option<u8>,
    /// Frames emulated over this instance's life — the honest denominator for a
    /// speed figure.
    frames: u64,
    /// Why runs ended early — `(stalled, could not flee, blacked out)`. A run
    /// that keeps stopping after two encounters is not a slow hunt, it is a
    /// broken one, and the three causes want three different fixes.
    blackouts: u64,
    flee_failures: u64,
    run_stalls: u64,
    runs: u64,
    run_encounters: u64,
}

impl<M: HuntMachine> Instance<M> {
    /// Wrap a machine the caller built.
    ///
    /// The caller is expected to have created it **without a save-file path**,
    /// which is what keeps a hunt off the filesystem entirely: no `.sav` is
    /// opened, created or written, so a thousand instances cannot touch
    /// anybody's real save (`src/mbc/battery.rs` only wires a battery when given
    /// a path). `examples/shiny_hunt.rs` does that and says so.
    pub fn new(mut gb: M, cfg: &HuntConfig, index: usize) -> Result<Instance<M>, String> {
        gb.load_state(&cfg.base_state)
            .map_err(|e| format!("base save state: {e}"))?;
        // `on_attach` runs on the first dispatch, so give it a frame before
        // asking whether the cartridge was recognised.
        gb.frame();
        if let Some(reason) = gb.refusal() {
            return Err(reason);
        }
        Ok(Instance {
            gb,
            base: cfg.base_state.clone(),
            index,
            seed: cfg.seed_for(index),
            shape: cfg.shape,
            max_frames: cfg.max_frames_per_attempt,
            band: cfg.grass_band,
            run_length: cfg.run_length,
            target: cfg.target.clone(),
            band_centre: None,
            frames: 0,
            blackouts: 0,
            flee_failures: 0,
            run_stalls: 0,
            runs: 0,
            run_encounters: 0,
        })
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Why runs ended early: `(stalled, could not flee, blacked out)`, plus the
    /// mean run length actually achieved. **A hunt whose runs keep stopping
    /// after two encounters is not slow, it is broken** — and each cause wants a
    /// different fix: a stall means the walk left the grass, a flee failure
    /// means the battle would not end, a blackout means the party is too weak
    /// for a run of battles.
    pub fn run_health(&self) -> RunHealth {
        RunHealth {
            runs: self.runs,
            encounters: self.run_encounters,
            stalls: self.run_stalls,
            flee_failures: self.flee_failures,
            blackouts: self.blackouts,
        }
    }

    /// Run attempt `n`: reload the base state, then meet up to
    /// `encounters_per_attempt` wild Pokémon in a row, running from each one
    /// that is not a hit.
    ///
    /// **The run of encounters is not an optimisation, it is the thing that
    /// makes a wild hunt possible at all**, and the reason is the sharpest fact
    /// this feature turned up. `Random_` (`pokered/home/random.asm`) reads
    /// `rDIV` and adds it to `hRandomAdd`, twice per call — and
    /// `LoadEnemyMonData` takes the DV bytes from two *consecutive* calls a few
    /// microseconds apart, so both see the **same** `rDIV` sample `R`:
    ///
    /// ```text
    /// hi = A + R          lo = hi + R          (A = hRandomAdd before the roll)
    /// ```
    ///
    /// The two bytes are therefore not independent — `lo - hi` *is* the `rDIV`
    /// sample — and from a fixed save state both `A` and `R` are functions of
    /// elapsed frames, so every roll lands on one thin one-dimensional band.
    /// Measured on Route 1: **~2.7 distinct `lo` values per `hi`**, and a
    /// reachable set of about **1,100 of the 65,536 spreads**. A 97,212-encounter
    /// hunt for a 1-in-8,192 target found nothing, and would never have,
    /// because none of the eight spreads it wanted was on the band. Reloading
    /// between every encounter is what pins the band in place: it puts the RNG
    /// back to the same state every time, so no amount of input jitter and no
    /// number of parallel instances can leave it.
    ///
    /// A battle is the way off it. Gen 1's battle engine calls `Random`
    /// constantly — accuracy, damage, the AI, the run-away roll — so the state
    /// after fleeing one is somewhere else entirely, and the next encounter
    /// starts from a different band. This is also, not coincidentally, what a
    /// human hunter's console is doing.
    ///
    /// Purity is kept: everything inside an attempt is deterministic, so the
    /// whole run still depends only on the base state, the instance seed and
    /// `n`, and a hit still replays.
    pub fn attempt(&mut self, n: u64) -> Result<Vec<Sighting>, String> {
        // `load_state` restores the machine byte for byte and resets plugin
        // state, so every attempt starts from the identical position with a
        // clear latch.
        let base = std::mem::take(&mut self.base);
        let r = self.gb.load_state(&base).map_err(|e| e.to_string());
        self.base = base;
        r?;
        // Belt and braces: `load_state` resets plugins, but a host that
        // implemented the seam without that would otherwise carry a stale latch
        // into the next attempt and report the same Pokémon for ever.
        self.gb.arm();
        // A base state taken *during* a battle is a whole hunt wasted: the
        // walker sees `wIsInBattle` already set, stops immediately, and every
        // attempt is reported as a stall with no clue why. It is an easy state
        // to make by accident — the last step of walking into grass is quite
        // likely to be the step that rolls an encounter — so name it.
        if self.shape == Shape::Grass && self.gb.peek(IS_IN_BATTLE) != 0 {
            return Err(
                "the base save state is already in a battle; a grass hunt needs one \
                 taken standing in grass, out of battle"
                    .into(),
            );
        }
        let mut sched = Schedule::for_attempt(self.seed, n);
        self.band_centre = None;
        let mut out = Vec::new();
        let runs = if self.shape == Shape::Grass {
            self.run_length.max(1)
        } else {
            // A soft-reset hunt has exactly one roll per reset by definition —
            // that *is* the method.
            1
        };
        for i in 0..runs {
            self.gb.arm();
            let mut budget = self.max_frames;
            if i == 0 {
                self.idle_bounded(sched.lead_in, &mut budget);
            }
            match self.shape {
                Shape::Grass => self.walk_the_grass(&mut sched, &mut budget),
                Shape::Reset => self.press_through(&mut sched, &mut budget),
            }
            match self.gb.sighting() {
                Some(s) => out.push(s),
                // Stalled. Stop the run rather than pressing on: whatever went
                // wrong (out of the grass, a text box, a wedged game) will go
                // wrong again, and the reload at the top of the next attempt is
                // the recovery.
                None => {
                    self.run_stalls += 1;
                    break;
                }
            }
            if self.target_matches(out.last()) || i + 1 == runs {
                break;
            }
            if !self.flee() {
                self.flee_failures += 1;
                break;
            }
        }
        self.runs += 1;
        self.run_encounters += out.len() as u64;
        Ok(out)
    }

    fn target_matches(&self, s: Option<&Sighting>) -> bool {
        s.is_some_and(|s| self.target.matches(s.dvs, s.species))
    }

    /// Run from the wild battle we are in, and get back to walking.
    ///
    /// `RUN` is the bottom-right entry of the 2x2 battle menu (`FIGHT ITEM` over
    /// `PKMN RUN`), so from the default `FIGHT` it is Right, Down, A. Running
    /// can fail, so this keeps trying; afterwards "Got away safely!" is still on
    /// screen with `wIsInBattle` already clear, and **a text box swallows every
    /// direction key** — so it is pressed through with **B**, never A (both
    /// advance a box, but B can only ever *close* a menu, while an A one frame
    /// late opens the START menu and wedges the walk for the rest of the run).
    ///
    /// Returns false if the battle would not end, which ends the run and lets
    /// the next attempt's reload recover.
    fn flee(&mut self) -> bool {
        for _ in 0..FLEE_TRIES {
            for key in [KeypadKey::Right, KeypadKey::Down, KeypadKey::A] {
                self.tap(key, 14);
            }
            // `$FF` is "the player blacked out" (`home/overworld.asm`
            // `.allPokemonFainted`), not a battle state — and it is the end of
            // the run whatever else happens, because the game is about to
            // teleport the player to a Pokémon Center, where there is no grass.
            // A party that cannot survive a run of battles is a setup problem,
            // and the count reported beside the stalls is how it becomes
            // visible instead of looking like a slow hunt.
            if self.gb.peek(IS_IN_BATTLE) == 0xFF {
                self.blackouts += 1;
                // Counted here rather than at the call site, and the caller's
                // `flee_failures` also ticks — a blackout is a flee failure with
                // a known cause, and the two lines together say so.
                return false;
            }
            if self.gb.peek(IS_IN_BATTLE) == 0 {
                for _ in 0..6 {
                    self.tap(KeypadKey::B, 30);
                }
                return true;
            }
        }
        false
    }

    fn tap(&mut self, key: KeypadKey, gap: u32) {
        self.gb.key(key, true);
        for _ in 0..4 {
            self.gb.frame();
            self.frames += 1;
        }
        self.gb.key(key, false);
        for _ in 0..gap {
            self.gb.frame();
            self.frames += 1;
        }
    }

    fn latched(&self) -> bool {
        self.gb.sighting().is_some()
    }

    /// Run frames with a key held, stopping early once the plugin latches.
    fn hold_key(&mut self, key: KeypadKey, frames: u32, budget: &mut u32) {
        self.gb.key(key, true);
        let mut spent = 0;
        while spent < frames && *budget > 0 {
            self.gb.frame();
            self.frames += 1;
            *budget -= 1;
            spent += 1;
            if self.latched() {
                break;
            }
        }
        self.gb.key(key, false);
    }

    /// Pace up and down a band of grass.
    ///
    /// Paced **by coordinate**, never by blind alternation: in Gen 1 a press in
    /// a direction the player is not already facing only turns the sprite, so it
    /// costs a press and moves nothing. Alternating up/down blindly therefore
    /// drifts, and a hunter that drifts out of the grass quietly stops hunting —
    /// it keeps reporting attempts and never rolls another encounter.
    fn walk_the_grass(&mut self, sched: &mut Schedule, budget: &mut u32) {
        let start_y = *self
            .band_centre
            .get_or_insert_with(|| self.gb.peek(Y_COORD));
        let lo = start_y.saturating_sub(self.band);
        let hi = start_y.saturating_add(self.band);
        let mut going_up = true;
        let mut motionless = 0u32;
        while *budget > 0 {
            if self.latched() || self.gb.peek(IS_IN_BATTLE) != 0 {
                break;
            }
            let before = self.gb.peek(Y_COORD);
            if going_up && before <= lo {
                going_up = false;
            } else if !going_up && before >= hi {
                going_up = true;
            }
            let key = if going_up {
                KeypadKey::Up
            } else {
                KeypadKey::Down
            };
            // **Cap the press at the room actually left in the band.** A press
            // is held for up to [`HOLD_MIN`] + [`HOLD_SPAN`] frames and a Gen 1
            // step is 16, so a long one walks two or three tiles — and Route 1's
            // grass is only four tiles deep. Uncapped, the walker paced
            // 30 → 36 through a band of 32 → 35, spent most of its steps on
            // ordinary ground where nothing is ever rolled, and every run ended
            // in a stall after about five encounters. Measured; from the outside
            // it looked like a bad encounter rate rather than a walk in the
            // wrong place.
            //
            // The cap costs no entropy where there is room for a long press,
            // which is the point: the range stays wide in the middle of a band
            // and narrows only at its edges.
            let room = u32::from(if going_up {
                before.saturating_sub(lo)
            } else {
                hi.saturating_sub(before)
            });
            let cap = HOLD_MIN as u32 + room.saturating_sub(1) * STEP_FRAMES;
            let (hold, gap) = sched.step();
            self.hold_key(key, hold.min(cap.max(HOLD_MIN as u32)), budget);
            self.idle_bounded(gap, budget);
            if self.gb.peek(Y_COORD) == before {
                motionless += 1;
                // Turning in place, or walking into a ledge. Flip after a couple
                // so the walk cannot lock against an obstacle.
                if motionless >= 2 {
                    going_up = !going_up;
                    motionless = 0;
                }
            } else {
                motionless = 0;
            }
        }
        // In a battle now (or out of budget): stop pressing anything and let the
        // game finish generating the encounter.
        self.settle();
    }

    /// Tap A until the encounter appears — the soft-reset shape.
    fn press_through(&mut self, sched: &mut Schedule, budget: &mut u32) {
        while *budget > 0 {
            if self.latched() {
                return;
            }
            let (hold, gap) = sched.step();
            self.hold_key(KeypadKey::A, hold, budget);
            self.idle_bounded(gap + 4, budget);
        }
        self.settle();
    }

    fn idle_bounded(&mut self, frames: u32, budget: &mut u32) {
        for _ in 0..frames {
            if *budget == 0 {
                return;
            }
            self.gb.frame();
            self.frames += 1;
            *budget -= 1;
            if self.latched() {
                return;
            }
        }
    }

    /// Hand off the controls and wait for `LoadEnemyMonData` to finish.
    ///
    /// Bounded by [`SETTLE_FRAMES`] and given its **own** budget rather than
    /// the attempt's: the settle is the one part that must not be cut short, or
    /// an attempt that used all its frames reaching the battle would be
    /// abandoned one frame before its answer arrived and counted as a stall.
    fn settle(&mut self) {
        let mut left = SETTLE_FRAMES;
        while left > 0 {
            if self.latched() {
                return;
            }
            if self.gb.peek(IS_IN_BATTLE) == 0 {
                return;
            }
            self.gb.frame();
            self.frames += 1;
            left -= 1;
        }
    }

    /// The machine's full state right now — the thing that makes a hit
    /// actionable rather than a boast.
    pub fn state(&self) -> Vec<u8> {
        self.gb.save_state()
    }

    /// The map and position the instance is standing on, for the hit report,
    /// as `(map, x, y)`.
    pub fn location(&mut self) -> (u8, u8, u8) {
        (
            self.gb.peek(CUR_MAP),
            self.gb.peek(X_COORD),
            self.gb.peek(Y_COORD),
        )
    }
}

/// What one instance met, attempt by attempt: `runs[attempt][encounter]`.
///
/// Nested because an attempt is a *run* of encounters (see [`Instance::attempt`])
/// and the shape of a run — where in it a hit landed, whether it was cut short —
/// is exactly what [`probe`] is for.
pub type ProbedRuns = Vec<Vec<Sighting>>;

/// Why an attempt's run of encounters ended early, and how long runs really are.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RunHealth {
    pub runs: u64,
    pub encounters: u64,
    pub stalls: u64,
    pub flee_failures: u64,
    pub blackouts: u64,
}

impl RunHealth {
    pub fn mean_run_length(&self) -> f64 {
        if self.runs == 0 {
            0.0
        } else {
            self.encounters as f64 / self.runs as f64
        }
    }

    pub fn merge(&mut self, other: RunHealth) {
        self.runs += other.runs;
        self.encounters += other.encounters;
        self.stalls += other.stalls;
        self.flee_failures += other.flee_failures;
        self.blackouts += other.blackouts;
    }
}

/// A hit, with everything needed to reproduce it and to catch the Pokémon.
#[derive(Clone)]
pub struct Hit {
    pub instance: usize,
    pub attempt: u64,
    /// Which encounter of the attempt's run it was (0-based). An attempt meets
    /// several Pokémon in a row, fleeing between them — see
    /// [`Instance::attempt`] — so the attempt number alone does not name one.
    pub encounter_in_run: usize,
    pub seed: u64,
    pub sighting: Sighting,
    /// A save state taken the instant the encounter was read. Load this and the
    /// Pokémon is standing there waiting for a Ball.
    pub state: Vec<u8>,
    /// `(wCurMap, wXCoord, wYCoord)` at the moment of the hit.
    pub location: (u8, u8, u8),
    /// Whether replaying `(seed, attempt)` from the base state in a *fresh*
    /// machine produced the same Pokémon. A hit that does not reproduce is
    /// reported as such rather than quietly trusted.
    pub reproduced: bool,
    pub elapsed: Duration,
}

/// A live view of a running hunt.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub attempts: u64,
    pub encounters: u64,
    pub stalls: u64,
    pub frames: u64,
    pub elapsed: Duration,
    pub per_instance: Vec<u64>,
    pub expected_encounters: f64,
    pub median_encounters: f64,
}

impl Snapshot {
    /// Encounters per second, measured — not assumed.
    pub fn encounters_per_second(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0.0 {
            0.0
        } else {
            self.encounters as f64 / s
        }
    }

    /// How many times faster than a Game Boy the whole rig is running.
    pub fn speed_multiple(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0.0 {
            0.0
        } else {
            self.frames as f64 / s / 59.7275
        }
    }

    /// Mean time to a hit at the measured rate, or `None` when nothing has been
    /// measured yet. This is the number that answers "should I leave it running
    /// overnight?".
    pub fn eta_mean(&self) -> Option<Duration> {
        self.eta(self.expected_encounters)
    }

    /// Time by which half of hunts would have finished.
    pub fn eta_median(&self) -> Option<Duration> {
        self.eta(self.median_encounters)
    }

    fn eta(&self, want: f64) -> Option<Duration> {
        let rate = self.encounters_per_second();
        if rate <= 0.0 || !want.is_finite() {
            return None;
        }
        let left = (want - self.encounters as f64).max(0.0);
        Some(Duration::from_secs_f64(left / rate))
    }
}

/// How a hunt ended.
#[derive(Clone)]
pub struct Outcome {
    pub hit: Option<Hit>,
    pub attempts: u64,
    pub encounters: u64,
    pub stalls: u64,
    pub frames: u64,
    pub elapsed: Duration,
    pub per_instance: Vec<u64>,
    pub errors: Vec<String>,
}

impl Outcome {
    pub fn encounters_per_second(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0.0 {
            0.0
        } else {
            self.encounters as f64 / s
        }
    }

    /// Emulated frames per second across every instance, expressed as a
    /// multiple of a real Game Boy's 59.7275 Hz.
    pub fn speed_multiple(&self) -> f64 {
        let s = self.elapsed.as_secs_f64();
        if s <= 0.0 {
            0.0
        } else {
            self.frames as f64 / s / 59.7275
        }
    }
}

/// The coordinator.
pub struct Hunt;

impl Hunt {
    /// Run until a hit, a limit, or an error.
    ///
    /// `build` is called once per instance, **on that instance's own thread**,
    /// and must hand back a machine with a [`crate::ShinyHunt`] already
    /// attached. `on_progress` is called on the calling thread roughly every
    /// `tick`.
    pub fn run<M, F>(
        cfg: &HuntConfig,
        build: F,
        tick: Duration,
        mut on_progress: impl FnMut(&Snapshot),
    ) -> Outcome
    where
        M: HuntMachine,
        F: Fn(usize) -> Result<M, String> + Sync,
    {
        let n = cfg.instances.max(1);
        let stop = AtomicBool::new(false);
        let attempts = AtomicU64::new(0);
        let encounters = AtomicU64::new(0);
        let stalls = AtomicU64::new(0);
        let frames = AtomicU64::new(0);
        let per: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
        let hit: Mutex<Option<Hit>> = Mutex::new(None);
        let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let started = Instant::now();
        let expected = cfg.target.expected_encounters();
        let median = cfg.target.median_encounters();
        let build = &build;

        let snapshot = |elapsed: Duration| Snapshot {
            attempts: attempts.load(Ordering::Relaxed),
            encounters: encounters.load(Ordering::Relaxed),
            stalls: stalls.load(Ordering::Relaxed),
            frames: frames.load(Ordering::Relaxed),
            elapsed,
            per_instance: per.iter().map(|c| c.load(Ordering::Relaxed)).collect(),
            expected_encounters: expected,
            median_encounters: median,
        };

        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(n);
            for i in 0..n {
                let (stop, attempts, encounters, stalls, frames, per, hit, errors) = (
                    &stop,
                    &attempts,
                    &encounters,
                    &stalls,
                    &frames,
                    &per,
                    &hit,
                    &errors,
                );
                handles.push(scope.spawn(move || {
                    let mut inst = match build(i).and_then(|gb| Instance::new(gb, cfg, i))
                    {
                        Ok(v) => v,
                        Err(e) => {
                            errors.lock().unwrap().push(format!("instance {i}: {e}"));
                            stop.store(true, Ordering::Relaxed);
                            return;
                        }
                    };
                    let mut n_attempt = 0u64;
                    loop {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        if let Some(max) = cfg.max_attempts {
                            if attempts.load(Ordering::Relaxed) >= max {
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        let before = inst.frames();
                        let result = inst.attempt(n_attempt);
                        frames.fetch_add(inst.frames() - before, Ordering::Relaxed);
                        attempts.fetch_add(1, Ordering::Relaxed);
                        per[i].fetch_add(1, Ordering::Relaxed);
                        match result {
                            Ok(run) => {
                                encounters.fetch_add(run.len() as u64, Ordering::Relaxed);
                                // An attempt that produced no encounter at all
                                // is a stall — counted and reported, never
                                // silently retried as if it had been one.
                                if run.is_empty() {
                                    stalls.fetch_add(1, Ordering::Relaxed);
                                }
                                let winner = run
                                    .iter()
                                    .position(|s| cfg.target.matches(s.dvs, s.species));
                                if let Some(at) = winner {
                                    let s = run[at];
                                    let state = inst.state();
                                    let location = inst.location();
                                    let elapsed = started.elapsed();
                                    // Prove it before claiming it: replay the
                                    // same (seed, attempt) from the base state
                                    // in a *fresh* machine and require the same
                                    // Pokémon at the same point in the run.
                                    let reproduced = build(i)
                                        .and_then(|gb| replay(gb, cfg, i, n_attempt))
                                        .map(|r| r.get(at) == Some(&s))
                                        .unwrap_or(false);
                                    *hit.lock().unwrap() = Some(Hit {
                                        instance: i,
                                        attempt: n_attempt,
                                        encounter_in_run: at,
                                        seed: inst.seed(),
                                        sighting: s,
                                        state,
                                        location,
                                        reproduced,
                                        elapsed,
                                    });
                                    stop.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                            Err(e) => {
                                errors.lock().unwrap().push(format!("instance {i}: {e}"));
                                stop.store(true, Ordering::Relaxed);
                                break;
                            }
                        }
                        n_attempt += 1;
                    }
                }));
            }

            loop {
                std::thread::sleep(tick);
                let elapsed = started.elapsed();
                if let Some(limit) = cfg.time_limit {
                    if elapsed >= limit {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
                let done = handles.iter().all(|h| h.is_finished());
                on_progress(&snapshot(elapsed));
                if done {
                    break;
                }
            }
        });

        let elapsed = started.elapsed();
        Outcome {
            hit: hit.into_inner().unwrap(),
            attempts: attempts.load(Ordering::Relaxed),
            encounters: encounters.load(Ordering::Relaxed),
            stalls: stalls.load(Ordering::Relaxed),
            frames: frames.load(Ordering::Relaxed),
            elapsed,
            per_instance: per.iter().map(|c| c.load(Ordering::Relaxed)).collect(),
            errors: errors.into_inner().unwrap(),
        }
    }
}

/// Replay one attempt in a fresh machine.
///
/// This is the reproducibility check, and it is also what a user runs to get
/// their Pokémon back if they lose the state file: the three numbers in a [`Hit`]
/// plus the base state are the whole recipe.
pub fn replay<M: HuntMachine>(
    gb: M,
    cfg: &HuntConfig,
    instance: usize,
    attempt: u64,
) -> Result<Vec<Sighting>, String> {
    let mut inst = Instance::new(gb, cfg, instance)?;
    inst.attempt(attempt)
}

/// Run the first `attempts` attempts of every instance and hand back what each
/// one found, in order.
///
/// **This is the divergence proof.** Run it with `decorrelate: true` and the
/// sequences must differ; run it with `decorrelate: false` and they must be
/// identical. Both halves matter — see the module docs.
pub fn probe<M, F>(
    cfg: &HuntConfig,
    build: F,
    attempts: u64,
) -> Result<(Vec<ProbedRuns>, RunHealth), String>
where
    M: HuntMachine,
    F: Fn(usize) -> Result<M, String> + Sync,
{
    let n = cfg.instances.max(1);
    let out: Mutex<Vec<ProbedRuns>> = Mutex::new(vec![Vec::new(); n]);
    let health: Mutex<RunHealth> = Mutex::new(RunHealth::default());
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let build = &build;
    std::thread::scope(|scope| {
        for i in 0..n {
            let (out, errors, health) = (&out, &errors, &health);
            scope.spawn(move || {
                match build(i).and_then(|gb| Instance::new(gb, cfg, i)) {
                    Ok(mut inst) => {
                        let mut seq = Vec::new();
                        for a in 0..attempts {
                            match inst.attempt(a) {
                                Ok(s) => seq.push(s),
                                Err(e) => {
                                    errors.lock().unwrap().push(e);
                                    break;
                                }
                            }
                        }
                        health.lock().unwrap().merge(inst.run_health());
                        out.lock().unwrap()[i] = seq;
                    }
                    Err(e) => errors.lock().unwrap().push(e),
                }
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok((out.into_inner().unwrap(), health.into_inner().unwrap()))
}

/// Whether every instance produced a *different* sequence of Pokémon.
///
/// The comparison is between whole sequences rather than single encounters: two
/// instances legitimately meeting the same species at the same level on the same
/// attempt is a coincidence a route with six encounter slots produces often, and
/// calling that a correlation failure would cry wolf. Two instances agreeing on
/// *every* attempt is the thing that means they are one instance wearing two
/// hats.
pub fn sequences_diverge(seqs: &[ProbedRuns]) -> bool {
    for (i, a) in seqs.iter().enumerate() {
        for b in seqs.iter().skip(i + 1) {
            if a == b {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use terminalgb_plugin_trainer::gen1::Dvs;

    #[test]
    fn different_seeds_give_different_schedules_and_the_same_seed_repeats() {
        let a = Schedule::for_attempt(1, 0);
        let b = Schedule::for_attempt(2, 0);
        let c = Schedule::for_attempt(1, 0);
        assert_eq!(
            a, c,
            "a schedule must be a pure function of (seed, attempt)"
        );
        assert_ne!(a, b, "two seeds must not give the same plan");
    }

    #[test]
    fn consecutive_attempts_do_not_repeat_a_schedule() {
        // A hunt that reused a schedule would re-roll the same Pokémon for ever.
        // The plan is the lead-in *and* the whole stream of per-step draws, so
        // compare the first several steps rather than three summary numbers.
        let mut seen = HashSet::new();
        for n in 0..512 {
            let mut s = Schedule::for_attempt(0xDEAD_BEEF, n);
            let plan: Vec<(u32, u32)> = (0..6).map(|_| s.step()).collect();
            seen.insert((s.lead_in, plan));
        }
        assert_eq!(seen.len(), 512, "schedules collapsed onto each other");
    }

    /// The per-step draw is the fix for the coverage collapse described on
    /// [`Schedule`]: a plan whose steps were all the same length would put every
    /// roll back on one thin band of the DV space.
    #[test]
    fn every_step_of_a_plan_is_drawn_separately() {
        let mut s = Schedule::for_attempt(0x1234_5678, 9);
        let steps: Vec<(u32, u32)> = (0..64).map(|_| s.step()).collect();
        let distinct: HashSet<_> = steps.iter().collect();
        assert!(
            distinct.len() > 40,
            "only {} distinct (hold, gap) pairs in 64 steps",
            distinct.len()
        );
        // …and every hold still buys at least one whole Gen 1 step (16 frames),
        // or the "walk" turns on the spot and the hunt rolls nothing.
        assert!(steps.iter().all(|(h, _)| *h >= HOLD_MIN as u32));
    }

    #[test]
    fn the_lead_in_spans_more_than_a_frame_so_rdiv_really_moves() {
        // `rDIV` advances 274 times per frame and wraps at 256, so a one-frame
        // shift moves it — but a schedule that only ever chose 0 or 1 would
        // still be weak. Check the spread.
        let (mut min, mut max) = (u32::MAX, 0);
        for n in 0..1000 {
            let s = Schedule::for_attempt(7, n);
            min = min.min(s.lead_in);
            max = max.max(s.lead_in);
        }
        assert!(min < 10 && max > 200, "lead-in range {min}..{max}");
    }

    #[test]
    fn default_instance_count_is_bounded_by_cores() {
        let n = default_instances();
        assert!(n >= 1);
        let cores = std::thread::available_parallelism()
            .map(|c| c.get())
            .unwrap_or(1);
        assert!(n <= cores);
    }

    #[test]
    fn shapes_parse_and_label() {
        assert_eq!(Shape::parse("wild"), Some(Shape::Grass));
        assert_eq!(Shape::parse("reset"), Some(Shape::Reset));
        assert_eq!(Shape::parse("nonsense"), None);
        assert_eq!(Shape::default(), Shape::Grass);
    }

    #[test]
    fn the_eta_is_computed_from_the_measured_rate() {
        let s = Snapshot {
            attempts: 100,
            encounters: 100,
            stalls: 0,
            frames: 60_000,
            elapsed: Duration::from_secs(10),
            per_instance: vec![50, 50],
            expected_encounters: 8192.0,
            median_encounters: 5678.0,
        };
        assert!((s.encounters_per_second() - 10.0).abs() < 1e-9);
        // 8092 encounters left at 10/s.
        let eta = s.eta_mean().unwrap();
        assert!((eta.as_secs_f64() - 809.2).abs() < 0.1);
        assert!(s.eta_median().unwrap() < eta);
    }

    // ---- The driver loop, against a fake machine ---------------------------
    //
    // No ROM, no emulator, no filesystem — so all of this runs in CI, where the
    // Pokémon cartridge this feature is for cannot go.

    /// A stand-in Game Boy that behaves like Gen 1 in the two ways the driver
    /// depends on: an encounter appears after a few steps, and **the Pokémon it
    /// rolls is a function of the input timing** — which is what `rDIV` makes
    /// true on the real thing (`pokered/home/random.asm` folds it in on every
    /// call).
    struct FakeGb {
        /// Hash of everything that has happened since the last `load_state`.
        /// Standing in for `rDIV` plus the RNG state.
        entropy: u64,
        frames_since_load: u32,
        steps: u32,
        battle: u8,
        /// Presses of A seen while in a battle, so the fake can model running
        /// away — without it the driver's run of encounters would end after one
        /// and the loop that fixes DV coverage would go untested.
        flee_presses: u32,
        latched: Option<Sighting>,
        /// Steps of walking before an encounter fires. Fixed, so that the
        /// *timing* is the only thing that can vary between instances.
        steps_per_encounter: u32,
        refuse: Option<String>,
    }

    impl FakeGb {
        fn new() -> FakeGb {
            FakeGb {
                entropy: 0,
                frames_since_load: 0,
                steps: 0,
                battle: 0,
                flee_presses: 0,
                latched: None,
                steps_per_encounter: 3,
                refuse: None,
            }
        }
    }

    impl HuntMachine for FakeGb {
        fn load_state(&mut self, _bytes: &[u8]) -> Result<(), String> {
            self.entropy = 0;
            self.frames_since_load = 0;
            self.steps = 0;
            self.battle = 0;
            self.flee_presses = 0;
            self.latched = None;
            Ok(())
        }
        fn save_state(&self) -> Vec<u8> {
            self.entropy.to_le_bytes().to_vec()
        }
        fn frame(&mut self) {
            self.frames_since_load += 1;
            if self.battle == 1 && self.latched.is_none() {
                // Stand in for the animation `InitBattle` plays before
                // `LoadEnemyMonData`: the answer is not available instantly.
                let e = mix(self.entropy);
                self.latched = Some(Sighting {
                    species: (e % 151) as u8 + 1,
                    level: 5,
                    dvs: Dvs::from_bytes((e >> 8) as u8, (e >> 16) as u8),
                    frames_into_attempt: self.frames_since_load as u64,
                });
            }
        }
        fn key(&mut self, key: KeypadKey, down: bool) {
            if !down {
                return;
            }
            // The frame the press lands on is the entropy — exactly the lever
            // the real decorrelation pulls.
            self.entropy = mix(self.entropy ^ u64::from(self.frames_since_load));
            if self.battle != 0 {
                // In a battle, A is the driver working the RUN menu.
                if key == KeypadKey::A {
                    self.flee_presses += 1;
                    if self.flee_presses >= 2 {
                        self.battle = 0;
                        self.flee_presses = 0;
                    }
                }
                return;
            }
            if matches!(key, KeypadKey::Up | KeypadKey::Down | KeypadKey::A) {
                self.steps += 1;
                if self.steps % self.steps_per_encounter == 0 {
                    self.battle = 1;
                }
            }
        }
        fn peek(&mut self, address: u16) -> u8 {
            match address {
                IS_IN_BATTLE => self.battle,
                // A band the walker can pace without ever hitting an edge.
                Y_COORD => 8u8.wrapping_add((self.steps % 3) as u8),
                X_COORD => 4,
                CUR_MAP => 0x0C,
                _ => 0,
            }
        }
        fn sighting(&self) -> Option<Sighting> {
            self.latched
        }
        fn arm(&mut self) {
            self.latched = None;
        }
        fn refusal(&self) -> Option<String> {
            self.refuse.clone()
        }
    }

    fn cfg(target: Target) -> HuntConfig {
        let mut c = HuntConfig::new(vec![0u8; 4], target);
        c.instances = 4;
        c.max_frames_per_attempt = 4000;
        c
    }

    /// Coverage is a different question from divergence, and this is the test
    /// that keeps them apart. The fake rolls from its input timing, so a run
    /// that reloaded between every encounter would keep landing on the same
    /// narrow set — exactly the failure the real cartridge showed.
    #[test]
    fn a_run_of_encounters_reaches_more_spreads_than_reloading_every_time() {
        let mut c = cfg(Target::shiny());
        c.instances = 2;
        c.run_length = 16;
        let wide = coverage(&probe(&c, |_| Ok(FakeGb::new()), 6).unwrap().0);

        let mut narrow_cfg = c.clone();
        narrow_cfg.run_length = 1;
        let narrow = coverage(&probe(&narrow_cfg, |_| Ok(FakeGb::new()), 6).unwrap().0);

        assert!(
            wide.samples > narrow.samples * 4,
            "a run of 16 should meet far more Pokémon per attempt: {} vs {}",
            wide.samples,
            narrow.samples
        );
        assert!(wide.distinct > narrow.distinct);
        assert!(
            narrow.verdict().starts_with("too few samples") || narrow.samples >= 200,
            "the verdict must refuse to judge a sample this small"
        );
    }

    #[test]
    fn coverage_says_too_narrow_when_the_reachable_set_cannot_hold_a_hit() {
        // 200 samples that only ever produced 12 spreads: an estimated
        // reachable set far too small to contain one of the eight shiny ones.
        let narrow = Coverage::from_counts(200, 12);
        assert!(narrow.reachable < 100.0, "{:?}", narrow);
        assert!(narrow.expected_shiny_spreads() < 0.05);
        assert!(narrow.verdict().starts_with("TOO NARROW"));
        // …and a healthy one says so.
        let wide = Coverage::from_counts(20_000, 17_000);
        assert!(wide.reachable > 30_000.0, "{:?}", wide);
        assert_eq!(
            wide.verdict(),
            "good — most of the 65,536 spreads are reachable"
        );
        // Every draw new: the sample bounds the set from below and the
        // estimator must not invent a number.
        assert!(Coverage::from_counts(50, 50).reachable.is_infinite());
    }

    #[test]
    fn instances_diverge_when_decorrelated_and_agree_when_they_are_not() {
        // The whole argument for the feature having more than one instance.
        let c = cfg(Target::shiny());
        let (seqs, health) = probe(&c, |_| Ok(FakeGb::new()), 4).unwrap();
        assert!(health.runs > 0 && health.mean_run_length() > 1.0);
        assert_eq!(seqs.len(), 4);
        assert!(seqs
            .iter()
            .all(|s| s.len() == 4 && s.iter().all(|run| !run.is_empty())));
        assert!(
            sequences_diverge(&seqs),
            "four decorrelated instances produced the same sequence: {seqs:?}"
        );

        // And the control: identical seeds must give identical sequences, or the
        // divergence above proves nothing (it could have been non-determinism).
        let mut control = c.clone();
        control.decorrelate = false;
        let (seqs, _) = probe(&control, |_| Ok(FakeGb::new()), 4).unwrap();
        for s in &seqs[1..] {
            assert_eq!(
                s, &seqs[0],
                "with decorrelation off the instances must agree exactly"
            );
        }
        assert!(!sequences_diverge(&seqs));
    }

    #[test]
    fn a_hunt_stops_on_a_hit_and_preserves_enough_to_reproduce_it() {
        // `atk&1` is roughly half of all spreads, so a hit arrives in a handful
        // of attempts — the contrived-target trick the docs describe.
        let mut c = cfg(Target::parse("atk&1", None).unwrap());
        c.max_attempts = Some(2000);
        let out = Hunt::run(&c, |_| Ok(FakeGb::new()), Duration::from_millis(5), |_| {});
        assert!(out.errors.is_empty(), "{:?}", out.errors);
        let hit = out
            .hit
            .expect("a 1-in-2 target must be hit within 2000 attempts");
        assert!(c.target.matches(hit.sighting.dvs, hit.sighting.species));
        assert!(hit.reproduced, "a hit must replay to the same Pokémon");
        assert!(!hit.state.is_empty(), "the state at the hit must be kept");
        assert_eq!(hit.seed, c.seed_for(hit.instance));
        assert_eq!(out.stalls, 0);
        // A run meets several Pokémon per attempt, so encounters outnumber
        // attempts — that is the point of the run.
        assert!(out.encounters >= out.attempts);
    }

    #[test]
    fn replaying_the_hits_three_numbers_lands_on_the_same_pokemon() {
        // The claim a `Hit` makes: base state + instance seed + attempt index is
        // the whole recipe. Checked directly rather than only through `run`.
        let c = cfg(Target::shiny());
        let a = replay(FakeGb::new(), &c, 2, 17).unwrap();
        let b = replay(FakeGb::new(), &c, 2, 17).unwrap();
        assert_eq!(a, b);
        assert!(!a.is_empty());
        // A different instance index is a different machine, and must differ.
        let other = replay(FakeGb::new(), &c, 3, 17).unwrap();
        assert_ne!(a, other);
    }

    #[test]
    fn an_attempt_that_never_produces_an_encounter_is_a_stall_not_a_hit() {
        let mut c = cfg(Target::shiny());
        c.max_attempts = Some(3);
        c.instances = 1;
        let out = Hunt::run(
            &c,
            |_| {
                let mut gb = FakeGb::new();
                // Never reaches an encounter within the budget.
                gb.steps_per_encounter = u32::MAX;
                Ok(gb)
            },
            Duration::from_millis(5),
            |_| {},
        );
        assert!(out.hit.is_none());
        assert_eq!(out.stalls, out.attempts);
        assert_eq!(out.encounters, 0);
        assert!(out.errors.is_empty());
    }

    #[test]
    fn an_unrecognised_cartridge_is_refused_rather_than_hunted_on() {
        let c = cfg(Target::shiny());
        let out = Hunt::run(
            &c,
            |_| {
                let mut gb = FakeGb::new();
                gb.refuse = Some("`TETRIS` is not Pokémon Red/Blue".into());
                Ok(gb)
            },
            Duration::from_millis(5),
            |_| {},
        );
        assert!(out.hit.is_none());
        assert!(!out.errors.is_empty());
        assert!(out.errors[0].contains("TETRIS"));
    }

    #[test]
    fn progress_is_reported_while_the_hunt_runs() {
        let mut c = cfg(Target::parse("atk&1", None).unwrap());
        c.max_attempts = Some(400);
        let mut ticks = 0;
        let out = Hunt::run(
            &c,
            |_| Ok(FakeGb::new()),
            Duration::from_millis(1),
            |s| {
                ticks += 1;
                // Encounters outnumber attempts: each attempt is a run.
                assert!(s.encounters >= s.attempts.saturating_sub(s.stalls));
            },
        );
        assert!(ticks > 0, "the progress callback never fired");
        assert!(out.speed_multiple() >= 0.0);
    }
}

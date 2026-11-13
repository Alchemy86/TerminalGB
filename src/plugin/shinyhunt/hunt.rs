//! The hunt driver: N headless emulators, one encounter loop each, stop on a hit.
//!
//! # Why this is not inside the plugin
//!
//! A [`super::ShinyHunt`] can *watch*. It cannot press a button, step a frame,
//! load a save state or start a second emulator — the plugin API
//! ([`crate::plugin`]) has no verb for any of those, by design: a plugin is a
//! passenger on a machine somebody else is driving.
//!
//! Hunting needs a driver. So this module is a **host-side companion** to the
//! plugin, written against exactly the public API a third party has
//! ([`Gameboy::load_state`], [`Gameboy::keydown`], [`Gameboy::frame`],
//! [`Gameboy::add_plugin`], [`Gameboy::plugin_mut`]) and reaching into no core
//! internals. The split is a finding about the plugin API rather than a
//! complaint about it; `docs/shiny-hunt.md` records it for the crew formalising
//! that API.
//!
//! # The loop
//!
//! Per instance, with no shared state at all:
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
//! **identical input produces identical Pokémon**: four instances doing the same
//! thing are one instance wearing four hats. That is the software version of the
//! desync problem the hardware rig has, inverted — the DS splitter's four
//! consoles drift *apart* and stop agreeing; ours never drift at all.
//!
//! Gen 1's `Random` (`pokered/home/random.asm`) folds `rDIV` — the free-running
//! divider register — into its state on every call, and `rDIV` advances with
//! cycles. So the lever is **time**: change how many frames pass before each
//! press and every subsequent roll changes. Each attempt therefore gets its own
//! lead-in delay and its own per-step hold jitter, both drawn from a small
//! deterministic PRNG seeded by `(instance seed, attempt number)`.
//!
//! Two consequences worth stating plainly:
//!
//! * Divergence is **proved, not assumed**: [`probe`] runs the first few
//!   attempts of every instance and hands back the DV sequences, and the
//!   control case (all instances given the same seed) must come back identical.
//!   Both directions are checked — a decorrelation that "works" because the
//!   emulator is non-deterministic would be a much worse bug.
//! * A hit is **reproducible from three numbers**: the base state, the instance
//!   seed and the attempt index. Nothing else feeds the schedule, so replaying
//!   it lands on the same Pokémon. [`Hunt::run`] verifies that before reporting.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{ShinyHunt, Sighting, Target, Watch};
use crate::gameboy::Gameboy;
use crate::KeypadKey;

/// `wCurMap` — `pokered` `ram/wram.asm`.
pub const CUR_MAP: u16 = 0xD35E;
/// `wYCoord`, the player's position within the current map.
pub const Y_COORD: u16 = 0xD361;
/// `wXCoord`.
pub const X_COORD: u16 = 0xD362;
/// `wIsInBattle` — 0 none, 1 wild, 2 trainer.
pub const IS_IN_BATTLE: u16 = 0xD057;

/// How the driver provokes an encounter.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Shape {
    /// **Wild grass.** Pace back and forth across a band of tall grass until the
    /// step counter rolls an encounter. The save state must be standing *in*
    /// grass on a map with an encounter table.
    #[default]
    Grass,
    /// **A fixed encounter reached by reset** — a static legendary, a gift, an
    /// in-game trade. The save state must be positioned so that pressing A
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

/// Everything a hunt needs. Cheap to clone except for the two byte vectors,
/// which every instance shares by reference through [`Hunt`].
#[derive(Clone)]
pub struct HuntConfig {
    pub rom: Vec<u8>,
    /// The save state every attempt restarts from.
    pub base_state: Vec<u8>,
    pub target: Target,
    pub shape: Shape,
    pub watch: Watch,
    /// Number of emulator instances. See [`default_instances`].
    pub instances: usize,
    /// Root seed. The whole hunt is reproducible from this plus the base state.
    pub seed: u64,
    /// When false every instance uses the *same* schedule — the control case
    /// that proves decorrelation is doing the work. Never use it to hunt.
    pub decorrelate: bool,
    pub max_attempts: Option<u64>,
    pub time_limit: Option<Duration>,
    /// Emulated frames an attempt may burn before it is abandoned. Guards
    /// against a state that never produces an encounter.
    pub max_frames_per_attempt: u32,
    /// Half-height of the grass band paced in [`Shape::Grass`], in tiles.
    pub grass_band: u8,
}

impl HuntConfig {
    pub fn new(rom: Vec<u8>, base_state: Vec<u8>, target: Target) -> HuntConfig {
        HuntConfig {
            rom,
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
            grass_band: 2,
        }
    }
}

/// One emulator per core, minus one for everything else on the machine.
///
/// The useful number is bounded by cores: every instance is a full emulated
/// Game Boy running flat out, so beyond one per core they take turns and the
/// aggregate encounter rate stops rising. Leaving one core free keeps the
/// progress display responsive and the box usable.
pub fn default_instances() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().saturating_sub(1).max(1))
        .unwrap_or(1)
}

/// A deterministic per-attempt input plan.
///
/// This is the decorrelation lever, and it is deliberately tiny: everything the
/// driver does about timing comes from here, so a hit is reproducible from the
/// seed and the attempt number alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// Frames to idle before touching anything. The dominant term: it shifts
    /// `rDIV` at every subsequent `Random` call.
    pub lead_in: u32,
    /// Frames a direction (or A) is held per step.
    pub hold: u32,
    /// Frames released between steps.
    pub gap: u32,
    state: u64,
}

impl Schedule {
    pub fn for_attempt(seed: u64, attempt: u64) -> Schedule {
        let mut state = mix(seed ^ attempt.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let lead_in = (next(&mut state) % 240) as u32;
        let hold = 14 + (next(&mut state) % 6) as u32;
        let gap = 2 + (next(&mut state) % 3) as u32;
        Schedule {
            lead_in,
            hold,
            gap,
            state,
        }
    }

    /// Small per-step jitter, so a long walk does not settle into a fixed
    /// cadence that two instances could still share.
    fn step_jitter(&mut self) -> u32 {
        (next(&mut self.state) % 4) as u32
    }
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
pub struct Instance {
    gb: Gameboy,
    /// The save state every attempt restarts from.
    base: Vec<u8>,
    index: usize,
    seed: u64,
    shape: Shape,
    max_frames: u32,
    band: u8,
    /// Frames emulated over this instance's life — the honest denominator for
    /// a speed figure.
    frames: u64,
}

impl Instance {
    /// Build an instance. The ROM is passed **without a file path**, which is
    /// what keeps a hunt off the filesystem entirely: no `.sav` is opened,
    /// created or written, so a thousand instances cannot touch anybody's real
    /// save (`src/mbc/battery.rs` only wires a battery when given a path).
    pub fn new(cfg: &HuntConfig, index: usize) -> Result<Instance, String> {
        let mut gb = Gameboy::new(cfg.rom.clone(), None);
        gb.add_plugin(Box::new(
            ShinyHunt::new(cfg.target.clone()).watching(cfg.watch),
        ))
        .map_err(|e| e.to_string())?;
        gb.load_state(&cfg.base_state)
            .map_err(|e| format!("base save state: {e}"))?;
        // `on_attach` runs on the first dispatch, so give it a frame before
        // asking whether the cartridge was recognised.
        gb.frame();
        let recognised = gb
            .plugin::<ShinyHunt>()
            .map(|p| (p.game().is_some(), p.detected_title().to_string()));
        match recognised {
            Some((true, _)) => {}
            Some((false, title)) => {
                return Err(format!(
                    "`{}` is not a verified Pokémon Red/Blue cartridge — refusing to \
                     read another game's memory as if it were",
                    title.trim()
                ))
            }
            None => return Err("the hunt plugin did not attach".into()),
        }
        let seed = if cfg.decorrelate {
            mix(cfg.seed ^ (index as u64).wrapping_mul(0xA24B_AED4_963E_E407))
        } else {
            cfg.seed
        };
        Ok(Instance {
            gb,
            base: cfg.base_state.clone(),
            index,
            seed,
            shape: cfg.shape,
            max_frames: cfg.max_frames_per_attempt,
            band: cfg.grass_band,
            frames: 0,
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

    /// Run attempt `n`. `Ok(None)` means the attempt burned its frame budget
    /// without producing an encounter — a stall, counted and reported, never
    /// silently retried as if it had been an encounter.
    ///
    /// Pure in the sense that matters: the result depends only on the base
    /// state, the instance seed and `n`.
    pub fn attempt(&mut self, n: u64) -> Result<Option<Sighting>, String> {
        // `load_state` restores the machine byte for byte and resets plugin
        // state, so every attempt starts from the identical position with a
        // clear latch. The clone is one 170 KB memcpy per attempt against ~500
        // emulated frames — not worth avoiding.
        let base = std::mem::take(&mut self.base);
        let r = self.gb.load_state(&base).map_err(|e| e.to_string());
        self.base = base;
        r?;
        if let Some(p) = self.gb.plugin_mut::<ShinyHunt>() {
            p.arm();
        }
        let mut sched = Schedule::for_attempt(self.seed, n);
        self.idle(sched.lead_in);
        match self.shape {
            Shape::Grass => self.walk_the_grass(&mut sched),
            Shape::Reset => self.press_through(&mut sched),
        }
        Ok(self.gb.plugin::<ShinyHunt>().and_then(|p| p.sighting()))
    }

    fn idle(&mut self, frames: u32) {
        for _ in 0..frames {
            self.gb.frame();
            self.frames += 1;
        }
    }

    fn latched(&self) -> bool {
        self.gb
            .plugin::<ShinyHunt>()
            .map(|p| p.sighting().is_some())
            .unwrap_or(false)
    }

    /// Run frames with a key held, stopping early once the plugin latches.
    /// Returns the frames actually spent.
    fn hold_key(&mut self, key: KeypadKey, frames: u32, budget: &mut u32) -> u32 {
        self.gb.keydown(key);
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
        self.gb.keyup(key);
        spent
    }

    /// Pace up and down a band of grass.
    ///
    /// Paced **by coordinate**, never by blind alternation: in Gen 1 a press in
    /// a direction the player is not already facing only turns the sprite, so
    /// it costs a press and moves nothing. Alternating up/down blindly therefore
    /// drifts, and a hunter that drifts out of the grass quietly stops hunting.
    fn walk_the_grass(&mut self, sched: &mut Schedule) {
        let start_y = self.gb.peek(Y_COORD);
        let lo = start_y.saturating_sub(self.band);
        let hi = start_y.saturating_add(self.band);
        let mut going_up = true;
        let mut budget = self.max_frames.saturating_sub(sched.lead_in);
        let mut motionless = 0u32;
        while budget > 0 {
            if self.latched() {
                return;
            }
            if self.gb.peek(IS_IN_BATTLE) != 0 {
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
            let hold = sched.hold + sched.step_jitter();
            self.hold_key(key, hold, &mut budget);
            self.idle_bounded(sched.gap, &mut budget);
            if self.gb.peek(Y_COORD) == before {
                motionless += 1;
                // Turning in place, or walking into a ledge. Flip after a
                // couple so the walk cannot lock against an obstacle.
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
        self.settle(&mut budget);
    }

    /// Tap A until the encounter appears — the soft-reset shape.
    fn press_through(&mut self, sched: &mut Schedule) {
        let mut budget = self.max_frames.saturating_sub(sched.lead_in);
        while budget > 0 {
            if self.latched() {
                return;
            }
            let hold = sched.hold + sched.step_jitter();
            self.hold_key(KeypadKey::A, hold, &mut budget);
            self.idle_bounded(sched.gap + 4, &mut budget);
        }
        self.settle(&mut budget);
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

    /// Hands off the controls and waits for `LoadEnemyMonData` to finish. Bounded
    /// by [`super::SETTLE_FRAMES`] so a wedged game is abandoned, not waited on.
    fn settle(&mut self, budget: &mut u32) {
        let mut left = super::SETTLE_FRAMES.min(u64::from(*budget) + super::SETTLE_FRAMES);
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

    /// The map and position the instance is standing on, for the hit report.
    pub fn location(&mut self) -> (u8, u8, u8) {
        (
            self.gb.peek(CUR_MAP),
            self.gb.peek(X_COORD),
            self.gb.peek(Y_COORD),
        )
    }
}

/// A hit, with everything needed to reproduce and to catch it.
#[derive(Clone)]
pub struct Hit {
    pub instance: usize,
    pub attempt: u64,
    pub seed: u64,
    pub sighting: Sighting,
    /// A save state taken the instant the encounter was read. Load this and the
    /// Pokémon is standing there waiting to be caught.
    pub state: Vec<u8>,
    /// Map, x, y at the moment of the hit.
    pub location: (u8, u8, u8),
    /// Whether replaying `(seed, attempt)` from the base state produced the same
    /// Pokémon. A hit that does not reproduce is reported as such rather than
    /// quietly trusted.
    pub reproduced: bool,
    pub elapsed: Duration,
}

/// A live view of a running hunt.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub attempts: u64,
    pub encounters: u64,
    pub stalls: u64,
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

    /// Emulated frames per second across every instance, i.e. how many times
    /// faster than a Game Boy the whole rig ran.
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
    /// Run until a hit, a limit, or an error. `on_progress` is called on the
    /// calling thread roughly every `tick`.
    pub fn run(
        cfg: &HuntConfig,
        tick: Duration,
        mut on_progress: impl FnMut(&Snapshot),
    ) -> Outcome {
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

        let snapshot = |elapsed: Duration| Snapshot {
            attempts: attempts.load(Ordering::Relaxed),
            encounters: encounters.load(Ordering::Relaxed),
            stalls: stalls.load(Ordering::Relaxed),
            elapsed,
            per_instance: per.iter().map(|c| c.load(Ordering::Relaxed)).collect(),
            expected_encounters: expected,
            median_encounters: median,
        };

        std::thread::scope(|scope| {
            let mut handles = Vec::with_capacity(n);
            for i in 0..n {
                let stop = &stop;
                let attempts = &attempts;
                let encounters = &encounters;
                let stalls = &stalls;
                let frames = &frames;
                let per = &per;
                let hit = &hit;
                let errors = &errors;
                handles.push(scope.spawn(move || {
                    let mut inst = match Instance::new(cfg, i) {
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
                            Ok(Some(s)) => {
                                encounters.fetch_add(1, Ordering::Relaxed);
                                if cfg.target.matches(s.dvs, s.species) {
                                    let state = inst.state();
                                    let location = inst.location();
                                    let elapsed = started.elapsed();
                                    // Prove it before claiming it: replay the
                                    // same (seed, attempt) from the base state
                                    // in a *fresh* machine and require the same
                                    // Pokémon.
                                    let reproduced = replay(cfg, i, n_attempt)
                                        .map(|r| r == Some(s))
                                        .unwrap_or(false);
                                    *hit.lock().unwrap() = Some(Hit {
                                        instance: i,
                                        attempt: n_attempt,
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
                            Ok(None) => {
                                stalls.fetch_add(1, Ordering::Relaxed);
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

/// Replay one attempt in a fresh machine. This is the reproducibility check and
/// also what a user runs to get their Pokémon back if they lose the state file.
pub fn replay(
    cfg: &HuntConfig,
    instance: usize,
    attempt: u64,
) -> Result<Option<Sighting>, String> {
    let mut inst = Instance::new(cfg, instance)?;
    inst.attempt(attempt)
}

/// Run the first `attempts` attempts of every instance and hand back what each
/// one found, in order.
///
/// This is the divergence proof. Run it with `decorrelate: true` and the
/// sequences must differ; run it with `decorrelate: false` and they must be
/// identical. Both halves matter — see the module docs.
pub fn probe(cfg: &HuntConfig, attempts: u64) -> Result<Vec<Vec<Option<Sighting>>>, String> {
    let n = cfg.instances.max(1);
    let out: Mutex<Vec<Vec<Option<Sighting>>>> = Mutex::new(vec![Vec::new(); n]);
    let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for i in 0..n {
            let out = &out;
            let errors = &errors;
            scope.spawn(move || match Instance::new(cfg, i) {
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
                    out.lock().unwrap()[i] = seq;
                }
                Err(e) => errors.lock().unwrap().push(e),
            });
        }
    });
    let errors = errors.into_inner().unwrap();
    if !errors.is_empty() {
        return Err(errors.join("; "));
    }
    Ok(out.into_inner().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn different_seeds_give_different_schedules_and_the_same_seed_repeats() {
        let a = Schedule::for_attempt(1, 0);
        let b = Schedule::for_attempt(2, 0);
        let c = Schedule::for_attempt(1, 0);
        assert_eq!(a, c, "a schedule must be a pure function of (seed, attempt)");
        assert_ne!((a.lead_in, a.hold, a.gap), (b.lead_in, b.hold, b.gap));
    }

    #[test]
    fn consecutive_attempts_do_not_repeat_a_schedule() {
        // A hunt that reused a schedule would re-roll the same Pokémon for ever.
        let mut seen = std::collections::HashSet::new();
        for n in 0..512 {
            let s = Schedule::for_attempt(0xDEAD_BEEF, n);
            seen.insert((s.lead_in, s.hold, s.gap));
        }
        // 240 * 6 * 3 = 4320 distinct plans; 512 draws should cover most of it
        // without collapsing. A generator that had collapsed would show far
        // fewer.
        assert!(seen.len() > 400, "only {} distinct schedules", seen.len());
    }

    #[test]
    fn the_lead_in_spans_more_than_a_frame_so_rdiv_really_moves() {
        // `rDIV` advances 274 times per frame and wraps at 256, so a one-frame
        // shift moves it by 18 — but a schedule that only ever chose 0 or 1
        // would still be weak. Check the spread.
        let mut min = u32::MAX;
        let mut max = 0;
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
}

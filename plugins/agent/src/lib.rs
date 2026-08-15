//! **Claude Plays Pokémon** — the substrate that lets an AI agent watch this
//! emulator and play it, with a commentary panel a Twitch audience can follow.
//!
//! Full write-up, OBS recipe and the honest gaps: [`docs/claude-plays.md`](../docs/claude-plays.md).
//!
//! # The problem this module exists to solve
//!
//! **The model is slow and the game is not.** A round trip takes seconds; the
//! Game Boy runs at 59.7 frames a second. Two obvious designs both fail:
//! consulting the agent every frame costs a fortune and achieves nothing, and
//! letting the emulator wait on the agent turns a stream into a slideshow.
//!
//! The shape that works is **plan, then execute locally**:
//!
//! ```text
//!   snapshot ──▶ driver ──▶ plan (≤12 actions) ──▶ executor ──▶ hundreds of
//!      ▲          (slow)                            (local,      frames
//!      │                                             per-frame)     │
//!      └──────────── outcomes + what changed ◀──────────────────────┘
//! ```
//!
//! The agent is asked again only when the plan **finishes**, **fails**, or the
//! world does something it did not plan for (a battle starts, the map changes).
//! One call therefore buys anywhere from a few hundred to fifteen hundred
//! emulated frames. On the opening of Pokémon Red that is roughly one call
//! every five to twenty seconds of game time instead of sixty a second.
//!
//! # The three non-negotiables, and where each one lives
//!
//! | rule | where |
//! |---|---|
//! | the emulator never blocks on the agent | [`driver::Driver`] is polled, never awaited; [`subprocess::CommandDriver`] runs the child on its own thread |
//! | a hard budget cap and a circuit breaker | [`governor::Governor`] — four independent ceilings and a breaker, in the host, where no driver can reach them |
//! | stuck detection with escalation | [`governor::Stuck`], escalating nudge → insist → local intervention |
//!
//! # Keeping the agent swappable
//!
//! Nothing here knows what Claude is. A driver is *"here is the state, give me
//! actions and commentary"* and nothing more — it never receives a
//! [`Machine`](terminalgb_plugin_api::Machine), so it cannot press a button, read
//! memory or write a save. The host parses its reply, clamps every number and
//! performs the plan. That is what makes
//! [`driver::ScriptedDriver`] a complete substitute for a model in tests: it
//! goes through the identical path.

use std::path::PathBuf;
use std::time::Instant;

pub mod action;
pub mod commentary;
pub mod driver;
pub mod governor;
pub mod json;
pub mod observe;
pub mod subprocess;

use action::{Action, Executor, Limits, Outcome};
use commentary::{Activity, Commentary};
use driver::{Driver, DriverError, Poll};
use governor::{Escalation, Gate, Governor, Stuck, Usage};
use observe::{classify, GameState, Gen1Observer, Screen};
use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};

/// The name this plugin registers under.
pub const NAME: &str = "agent";

/// The snapshot/reply schema version. Bump it when a field is renamed,
/// retyped or removed; additions are free.
pub const SCHEMA: &str = "gbagent/1";

/// How the host is configured. Everything with a cost or a risk is here rather
/// than in the driver.
#[derive(Debug, Clone)]
pub struct Config {
    pub budget: governor::Budget,
    pub limits: Limits,
    /// What the session is trying to achieve, in one sentence, handed to the
    /// driver every call. This is the *operator's* objective, not the agent's
    /// plan.
    pub objective: String,
    /// Frames to let the screen settle after a plan before taking the snapshot
    /// the next decision is made from. Without it the agent is shown a fade.
    pub settle_frames: u16,
    /// Write the current frame here as a PNG at each decision, and tell the
    /// driver where it is. Off by default: it is the expensive channel.
    pub frame_capture: Option<PathBuf>,
    /// NDJSON log of every decision — request, reply, outcome, cost. This is
    /// where errors, stderr and paths go; never the panel.
    pub log: Option<PathBuf>,
    /// How many past decisions' outcomes to include in a snapshot.
    pub history: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            budget: governor::Budget::default(),
            limits: Limits::default(),
            objective: String::new(),
            settle_frames: 20,
            frame_capture: None,
            log: None,
            history: 4,
        }
    }
}

/// Where the plugin is in its loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Letting the screen settle before the next snapshot.
    Settling(u16),
    /// A request is with the driver.
    Thinking,
    /// A plan is being carried out.
    Executing,
    /// The agent has stopped: budget, breaker, or switched off.
    Stopped,
}

/// What the world looked like when the current plan started, so an unexpected
/// change can interrupt it.
#[derive(Debug, Clone, Copy)]
struct Watch {
    map: u8,
    in_battle: bool,
}

/// A driver reply the host accepted, handed back to whatever is recording the
/// run.
///
/// This is deliberately the **raw text**, not the parsed plan: replaying a
/// recording means pushing this back through the same parse-clamp-execute path
/// the live run took, and anything already parsed would be a second
/// implementation of it. See `src/scenario/record.rs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedReply {
    pub decision: u64,
    pub frame: u64,
    pub reply: String,
}

/// One decision, as it goes into the log.
#[derive(Debug, Clone)]
struct Record {
    decision: u64,
    frame: u64,
    request: String,
    reply: String,
    error: Option<String>,
    stderr: String,
    usage: Usage,
    ms: u128,
}

/// The plugin.
pub struct AgentPlugin {
    driver: Box<dyn Driver>,
    config: Config,
    governor: Governor,
    stuck: Stuck,
    commentary: Commentary,
    exec: Executor,
    obs: Gen1Observer,
    supported: bool,
    detected_title: String,
    phase: Phase,
    frame: u64,
    decisions: u64,
    /// Outcomes of the last few plans, newest last.
    history: Vec<Outcome>,
    /// Why the last plan stopped early, if it did.
    interrupted: Option<String>,
    watch: Option<Watch>,
    /// The request currently with the driver, kept for the log.
    pending: Option<(String, Instant)>,
    log: Option<std::io::BufWriter<std::fs::File>>,
    /// Switched off by the operator — the human has the controller back.
    enabled: bool,
    /// Complaints about the last reply, fed back to the driver next time.
    complaints: Vec<String>,
    /// Standing instructions the *host* wants the agent to have, set from
    /// outside between decisions and emitted at the head of `notes`. See
    /// [`AgentPlugin::set_briefing`].
    briefing: Vec<String>,
    /// Replies accepted since the host last drained them, for a host that is
    /// recording the run. See [`AgentPlugin::drain_accepted`].
    accepted: Vec<AcceptedReply>,
    /// Set once the very first snapshot has been sent, so the action reference
    /// is only paid for once.
    sent_help: bool,
    /// Screen hash seen on the previous settling frame, and how many
    /// consecutive frames it has held. A real run spent a model call reasoning
    /// about `KU:N:` — half of two different screens — because the settle was
    /// a blind frame count; now the snapshot also waits for the tilemap to
    /// stop changing (bounded — see [`SETTLE_EXTRA_CAP`]).
    settle_hash: u64,
    settle_stable: u16,
}

/// Consecutive identical-tilemap frames required before a snapshot. Short,
/// because the things being waited out (a scroll, a fade, letter-by-letter
/// printing) touch the tilemap every frame or two; the blinking ▼ arrow
/// toggles far more slowly than this window, so it cannot starve it.
const SCREEN_STABLE_FRAMES: u16 = 4;
/// Extra settling frames allowed for stability before giving up and
/// snapshotting anyway — a hard bound, so a screen that animates for ever (or
/// one we misjudge) costs two emulated seconds, not the run.
const SETTLE_EXTRA_CAP: u16 = 120;

impl AgentPlugin {
    pub fn new(driver: Box<dyn Driver>, config: Config) -> AgentPlugin {
        let log = config
            .log
            .as_ref()
            .and_then(|p| std::fs::File::create(p).ok().map(std::io::BufWriter::new));
        AgentPlugin {
            governor: Governor::new(config.budget),
            exec: Executor::new(config.limits),
            driver,
            config,
            stuck: Stuck::default(),
            commentary: Commentary::default(),
            obs: Gen1Observer::new(false),
            supported: false,
            detected_title: String::new(),
            phase: Phase::Settling(0),
            frame: 0,
            decisions: 0,
            history: Vec::new(),
            interrupted: None,
            watch: None,
            pending: None,
            log,
            enabled: true,
            complaints: Vec::new(),
            briefing: Vec::new(),
            accepted: Vec::new(),
            sent_help: false,
            settle_hash: 0,
            settle_stable: 0,
        }
    }

    /// What the panel draws.
    pub fn commentary(&self) -> &Commentary {
        &self.commentary
    }

    pub fn governor(&self) -> &Governor {
        &self.governor
    }

    pub fn decisions(&self) -> u64 {
        self.decisions
    }

    pub fn driver_name(&self) -> &str {
        self.driver.name()
    }

    /// Outcomes of the plan that most recently ran — what the last decision
    /// actually *did*, action by action.
    ///
    /// Additive and read-only. It exists because a long unattended run doubles
    /// as a dataset (`src/longplay`): a record of one decision needs the
    /// observation, the stated intent, the **actions** and the **outcome**, and
    /// only the plugin knows the middle two. The executor clears these when the
    /// next plan starts, so this is exactly the last plan's and never a
    /// running total.
    pub fn last_plan(&self) -> &[Outcome] {
        self.exec.outcomes()
    }

    /// Highest number of consecutive decisions that changed nothing, this
    /// session — the agent's own repeat-detector high-water mark, for a run
    /// report. See [`governor::Stuck`], and note it answers a *different*
    /// question from the game-level wall detector in `src/longplay`.
    pub fn stuck_worst(&self) -> u32 {
        self.stuck.worst()
    }

    /// Standing instructions from the host, put at the **head** of the
    /// snapshot's `notes` — before the situational ones, because a rule the
    /// agent must follow outranks an observation about the current screen.
    ///
    /// Additive and host-driven: the plugin has no opinion about what goes in
    /// here. A scenario runner puts its rules and whatever its knowledge base
    /// worked out for the current position; a plain session leaves it empty and
    /// pays nothing. Replaced wholesale each time, so a caller that recomputes
    /// per decision cannot accumulate stale advice.
    pub fn set_briefing(&mut self, lines: Vec<String>) {
        self.briefing = lines;
    }

    /// Take the replies accepted since this was last called.
    ///
    /// Read-only and drain-on-read, so a host that never asks pays one empty
    /// `Vec` per session. It exists so a run can be *recorded*: the replies in
    /// order are all a replay needs, since everything downstream of them is
    /// deterministic.
    pub fn drain_accepted(&mut self) -> Vec<AcceptedReply> {
        std::mem::take(&mut self.accepted)
    }

    /// Whether this cartridge is one the observer understands.
    pub fn is_supported(&self) -> bool {
        self.supported
    }

    /// Why nothing is happening, when the cartridge is not supported.
    pub fn refusal(&self) -> Option<String> {
        if self.supported {
            return None;
        }
        Some(format!(
            "The agent reads Pokémon Red/Blue (English). This cartridge is \
             \"{}\", so it has no map, no party and no way to tell a menu from \
             a battle — it will not play rather than press buttons blindly.",
            self.detected_title
        ))
    }

    /// Hand the controller back to the human (or take it again). The emulator
    /// is unaffected either way.
    pub fn set_enabled(&mut self, on: bool) {
        if self.enabled == on {
            return;
        }
        self.enabled = on;
        if on {
            self.commentary.set_activity(Activity::Starting);
            self.commentary.system(self.frame, "Agent switched on.");
            self.phase = Phase::Settling(0);
        } else {
            self.driver.cancel();
            self.commentary.set_activity(Activity::Off);
            self.commentary.system(
                self.frame,
                "Agent switched off — a human has the controller.",
            );
            self.phase = Phase::Stopped;
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// One line for the plugin row in the status panel. Safe to broadcast.
    fn status_text(&self) -> String {
        if !self.supported {
            return "unsupported cartridge".to_string();
        }
        if !self.enabled {
            return "off".to_string();
        }
        format!(
            "{} · {} · {}",
            self.driver.name(),
            self.commentary.activity().label(),
            self.governor.status()
        )
    }

    // -- the loop ----------------------------------------------------------

    fn on_frame(&mut self, machine: &mut Machine) {
        if !self.supported || !self.enabled {
            return;
        }
        match self.phase {
            Phase::Stopped => {}
            Phase::Executing => self.tick_plan(machine),
            Phase::Thinking => {
                // Keep watching while a call is gated or in flight: a snapshot
                // built on the retry path must report the screen as it is
                // *then*, not the verdict the settle happened to end on.
                self.observe_stability(machine);
                self.tick_thinking(machine);
            }
            Phase::Settling(n) => {
                // Track whether the tilemap is still changing — a scroll, a
                // fade, a text box printing. The minimum settle is unchanged;
                // on top of it the snapshot waits (bounded) for the screen to
                // hold still, so the driver is not handed half of two screens.
                self.observe_stability(machine);
                let min_done = n + 1 >= self.config.settle_frames;
                let capped =
                    n + 1 >= self.config.settle_frames.saturating_add(SETTLE_EXTRA_CAP);
                if min_done && (self.screen_settled() || capped) {
                    self.phase = Phase::Thinking;
                    self.start_call(machine);
                } else {
                    self.phase = Phase::Settling(n + 1);
                }
            }
        }
    }

    /// One frame's worth of "is the screen still moving?". Hash-only: this runs
    /// every frame while a decision is pending, and decoding eighteen lines of
    /// text to answer it would be waste.
    fn observe_stability(&mut self, machine: &mut Machine) {
        let h = Screen::tilemap_hash(machine);
        if h == self.settle_hash {
            self.settle_stable = self.settle_stable.saturating_add(1);
        } else {
            self.settle_stable = 0;
            self.settle_hash = h;
        }
    }

    /// Has the tilemap held still long enough to believe what it says?
    ///
    /// Bounded by measurement, not by hope: on a real cartridge a dialogue box
    /// waiting on the blinking `▼` holds still for 42 frames at a stretch, and
    /// menus and the overworld do not change at all — so a 4-frame window
    /// cannot be starved by anything the game does routinely.
    fn screen_settled(&self) -> bool {
        self.settle_stable >= SCREEN_STABLE_FRAMES
    }

    fn tick_plan(&mut self, machine: &mut Machine) {
        // Interrupt on a surprise: something the plan did not account for is
        // exactly when the agent's judgement is worth paying for again.
        if let Some(w) = self.watch {
            let map = machine.peek(observe::W_CUR_MAP).unwrap_or(w.map);
            let battling = machine
                .peek(terminalgb_plugin_trainer::gen1::addr::IS_IN_BATTLE)
                .unwrap_or(0)
                != 0;
            if battling && !w.in_battle {
                self.exec.abort("a battle started", machine);
                self.commentary.event(self.frame, "A battle started!");
            } else if map != w.map {
                let name = observe::map_name(map);
                self.exec.abort("the map changed", machine);
                self.commentary.event(
                    self.frame,
                    &if name.is_empty() {
                        "Somewhere new.".to_string()
                    } else {
                        format!("Arrived: {name}.")
                    },
                );
            }
        }

        let running = self.exec.tick(machine, &self.obs);
        if let Some(a) = self.exec.current() {
            self.commentary.set_doing(&a.describe());
        }
        if running {
            return;
        }

        // The plan is over. Keep its outcomes for the next snapshot.
        self.interrupted = self.exec.abort_reason().map(|s| s.to_string());
        for o in self.exec.outcomes() {
            self.commentary
                .act(self.frame, &format!("{} — {}", o.action, o.result));
        }
        self.history.extend_from_slice(self.exec.outcomes());
        let keep = self.config.history * 4;
        if self.history.len() > keep {
            let drop = self.history.len() - keep;
            self.history.drain(0..drop);
        }
        self.watch = None;
        self.phase = Phase::Settling(0);
    }

    fn tick_thinking(&mut self, machine: &mut Machine) {
        let now = Instant::now();
        if self.governor.call_timed_out(now) {
            self.driver.cancel();
            self.fail(DriverError::Timeout, now);
            return;
        }
        match self.driver.poll() {
            Poll::Pending => {}
            Poll::Idle => {
                // Nothing outstanding: either the call was never started (the
                // gate said wait) or the driver is finished. Try again.
                self.start_call(machine);
            }
            Poll::Ready(text) => self.accept(text, machine, now),
            Poll::Failed(e) => self.fail(e, now),
        }
    }

    /// Ask the driver, if the governor allows it.
    fn start_call(&mut self, machine: &mut Machine) {
        let now = Instant::now();
        match self.governor.gate(now) {
            Gate::Stop(h) => {
                self.phase = Phase::Stopped;
                self.commentary.pause(self.frame, h.public_text());
                self.log_line(&format!("{{\"ev\":\"halt\",\"why\":\"{}\"}}", h.label()));
                self.driver.cancel();
                return;
            }
            Gate::Wait => {
                self.commentary.set_activity(Activity::Paused);
                return;
            }
            Gate::Go => {}
        }

        // Stuck? Escalate before spending a call on the same question.
        let game = self.obs.read(machine);
        let screen = Screen::read(machine);
        let fingerprint = fingerprint(&game, &screen);
        let escalation = self.stuck.observe(fingerprint);
        if escalation == Escalation::Intervene {
            self.intervene(machine, &game);
            return;
        }

        let request = self.snapshot(machine, &game, &screen, escalation);
        if !self.driver.submit(&request) {
            // The driver would not take it — a finished script, or busy. Not a
            // failure worth counting against the breaker; just idle.
            self.phase = Phase::Stopped;
            self.commentary.set_activity(Activity::Paused);
            self.commentary
                .system(self.frame, "The agent has nothing more to say.");
            return;
        }
        self.governor.call_started(now);
        self.pending = Some((request, now));
        self.phase = Phase::Thinking;
        self.commentary.set_activity(Activity::Thinking);
    }

    /// Break a loop locally rather than paying a model to repeat itself.
    fn intervene(&mut self, machine: &mut Machine, game: &GameState) {
        self.stuck.intervened();
        self.commentary.system(
            self.frame,
            "Going in circles — backing out and trying a different way.",
        );
        // B twice backs out of nested menus; then a step away from whatever
        // was being walked into.
        let away = match game.position.facing {
            Some(observe::Facing::Up) => terminalgb_plugin_api::KeypadKey::Down,
            Some(observe::Facing::Down) => terminalgb_plugin_api::KeypadKey::Up,
            Some(observe::Facing::Left) => terminalgb_plugin_api::KeypadKey::Right,
            _ => terminalgb_plugin_api::KeypadKey::Left,
        };
        self.exec.start(vec![
            Action::Press {
                button: terminalgb_plugin_api::KeypadKey::B,
                times: 2,
                hold: terminalgb_plugin_api::DEFAULT_HOLD,
            },
            Action::Wait { frames: 30 },
            Action::Walk {
                dir: away,
                steps: 2,
            },
        ]);
        self.begin_execution(machine);
    }

    fn accept(&mut self, text: String, machine: &mut Machine, now: Instant) {
        let stderr = self.take_driver_stderr();
        let (request, started) = self.pending.take().unwrap_or_default_pair();
        self.decisions += 1;

        let parsed = json::parse_embedded(&text);
        let reply = match parsed {
            Ok(v) => v,
            Err(e) => {
                self.log_record(Record {
                    decision: self.decisions,
                    frame: self.frame,
                    request,
                    reply: text,
                    error: Some(format!("parse: {e}")),
                    stderr,
                    usage: Usage::default(),
                    ms: now.saturating_duration_since(started).as_millis(),
                });
                self.fail(DriverError::Unreadable(e.0), now);
                return;
            }
        };

        // Usage is self-reported, so it is only ever a *floor* on the true
        // cost. The wall-clock ceiling in the governor is what makes that safe.
        let usage = reply
            .get("usage")
            .map(|u| Usage {
                input_tokens: u.u64_field("input_tokens").unwrap_or(0),
                output_tokens: u.u64_field("output_tokens").unwrap_or(0),
                usd: u.get("usd").and_then(|v| v.as_f64()).unwrap_or(0.0),
            })
            .unwrap_or_default();
        self.governor.call_succeeded(usage);

        if let Some(goal) = reply.str_field("goal") {
            self.commentary.set_goal(self.frame, goal);
        }
        if let Some(say) = reply
            .str_field("say")
            .or_else(|| reply.str_field("commentary"))
        {
            self.commentary.thought(self.frame, say);
        }

        let items = reply
            .get("plan")
            .or_else(|| reply.get("actions"))
            .and_then(|p| p.as_array())
            .unwrap_or(&[]);
        let (plan, errors) = action::plan_from_json(items, &self.config.limits);
        self.complaints = errors;

        // Recorded before the plan is judged usable: a reply that parsed but
        // said nothing to do is still what the driver said, and a replay that
        // skipped it would ask its script one question out of step.
        self.accepted.push(AcceptedReply {
            decision: self.decisions,
            frame: self.frame,
            reply: text.clone(),
        });

        self.log_record(Record {
            decision: self.decisions,
            frame: self.frame,
            request,
            reply: text,
            error: None,
            stderr,
            usage,
            ms: now.saturating_duration_since(started).as_millis(),
        });

        if plan.is_empty() {
            // Not a driver failure — it answered, it just did not say what to
            // do. Ask again after a settle rather than counting it against the
            // breaker, but tell it so next time.
            self.complaints
                .push("your last reply contained no usable actions".to_string());
            self.commentary
                .note_problem(self.frame, commentary::Problem::NoActions);
            self.phase = Phase::Settling(0);
            return;
        }

        self.exec.start(plan);
        self.begin_execution(machine);
    }

    fn begin_execution(&mut self, machine: &mut Machine) {
        self.watch = Some(Watch {
            map: machine.peek(observe::W_CUR_MAP).unwrap_or(0),
            in_battle: machine
                .peek(terminalgb_plugin_trainer::gen1::addr::IS_IN_BATTLE)
                .unwrap_or(0)
                != 0,
        });
        self.phase = Phase::Executing;
        self.commentary.set_activity(Activity::Playing);
        if let Some(a) = self.exec.current() {
            self.commentary.set_doing(&a.describe());
        }
    }

    fn fail(&mut self, e: DriverError, now: Instant) {
        let stderr = self.take_driver_stderr();
        let (request, started) = self.pending.take().unwrap_or_default_pair();
        self.log_record(Record {
            decision: self.decisions,
            frame: self.frame,
            request,
            reply: String::new(),
            error: Some(e.detail()),
            stderr,
            usage: Usage::default(),
            ms: now.saturating_duration_since(started).as_millis(),
        });
        // The panel is told the *category*, never the detail.
        self.commentary.note_problem(self.frame, e.category());
        let tripped = self.governor.call_failed(now);
        if tripped {
            self.commentary.pause(
                self.frame,
                "the agent keeps failing — waiting a moment before trying again",
            );
        }
        self.phase = Phase::Settling(0);
    }

    /// Drain the driver's diagnostic output into the log. **Never** into the
    /// commentary — a subprocess's stderr is exactly the kind of thing that
    /// carries a path, a stack trace or a key.
    fn take_driver_stderr(&mut self) -> String {
        self.driver.take_stderr()
    }

    // -- the snapshot ------------------------------------------------------

    /// Build the state the driver reasons about.
    fn snapshot(
        &mut self,
        machine: &mut Machine,
        game: &GameState,
        screen: &Screen,
        escalation: Escalation,
    ) -> String {
        let mode = classify(screen, game);
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", SCHEMA);
        w.num_field("frame", self.frame as f64);
        w.num_field("decision", self.decisions as f64 + 1.0);
        if !self.config.objective.is_empty() {
            w.str_field("objective", &self.config.objective);
        }
        w.str_field("mode", mode.name());

        w.obj_field("game");
        w.bool_field("in_game", game.in_game);
        if game.in_game {
            w.str_field("player", &game.player_name);
            w.obj_field("map");
            w.num_field("id", game.position.map as f64);
            if !game.map_name.is_empty() {
                w.str_field("name", game.map_name);
            }
            w.num_field("x", game.position.x as f64);
            w.num_field("y", game.position.y as f64);
            match game.position.facing {
                Some(f) => w.str_field("facing", f.name()),
                None => w.null_field("facing"),
            }
            match game.map_size {
                Some((mw, mh)) => {
                    w.num_field("width", mw as f64);
                    w.num_field("height", mh as f64);
                }
                // No header loaded: say so in the state rather than only in
                // `mode`, because this is the field that explains why walking
                // does nothing.
                None => w.bool_field("loaded", false),
            }
            w.end_obj();
            // Doors and stairs. Without these the agent has coordinates and no
            // idea a way out exists; with them "leave the house" is a `goto`.
            w.arr_field("exits");
            for e in &game.exits {
                w.begin_obj();
                w.num_field("x", e.x as f64);
                w.num_field("y", e.y as f64);
                w.num_field("to_map", e.to_map as f64);
                let n = observe::map_name(e.to_map);
                if !n.is_empty() {
                    w.str_field("to", n);
                }
                w.end_obj();
            }
            w.end_arr();
            w.num_field("badges", game.badges.count_ones() as f64);
            w.num_field("money", game.money as f64);
            w.arr_field("party");
            for m in &game.party {
                w.begin_obj();
                w.str_field("name", &m.nickname);
                w.num_field("level", m.level as f64);
                w.num_field("hp", m.hp as f64);
                w.num_field("max_hp", m.max_hp as f64);
                if let Some(s) = m.status {
                    w.str_field("status", s);
                }
                w.end_obj();
            }
            w.end_arr();
            if game.battle != terminalgb_plugin_trainer::gen1::BattleKind::None {
                w.obj_field("battle");
                w.str_field(
                    "kind",
                    match game.battle {
                        terminalgb_plugin_trainer::gen1::BattleKind::Wild => "wild",
                        _ => "trainer",
                    },
                );
                if let Some(o) = &game.opponent {
                    w.obj_field("opponent");
                    w.str_field("name", &o.nickname);
                    w.num_field("level", o.level as f64);
                    w.num_field("hp", o.hp as f64);
                    w.num_field("max_hp", o.max_hp as f64);
                    w.end_obj();
                }
                w.end_obj();
            }
        }
        w.end_obj();

        w.obj_field("screen");
        // Honesty before content: a decode of a half-drawn screen or of
        // graphics that happen to sit in the font range is nonsense, and
        // nonsense delivered in the same field as good data gets the same
        // trust. When the read cannot be trusted the text is *withheld* and
        // `trust` says why, so "screen not readable right now" is a fact the
        // driver can act on rather than a wall of letter salad.
        let trust = match screen.trust() {
            observe::Trust::Ok if !self.screen_settled() => observe::Trust::Settling,
            t => t,
        };
        w.str_field("trust", trust.label());
        // What shape of screen this is, so "what does A do here?" does not
        // have to be inferred from the text every time. Sent whatever the
        // trust verdict, because it is derived from the cursor and arrow
        // glyphs rather than from words.
        w.str_field("kind", screen.kind().name());
        if trust == observe::Trust::Ok {
            w.str_field("text", &screen.compact_text());
            // What the ▶ cursor points at — "is NEW GAME or OPTION selected?"
            // as a fact, not an inference over raw rows, and on the naming
            // keyboard the single letter A would type.
            if let Some(sel) = screen.selected() {
                w.str_field("selected", &sel);
            }
        } else {
            w.str_field("text", "");
        }
        w.bool_field("waiting_for_input", screen.waiting);
        match screen.cursor_row {
            Some(r) => w.num_field("menu_cursor_row", r as f64),
            None => w.null_field("menu_cursor_row"),
        }
        // On a grid (the naming keyboard) the row alone cannot say which
        // letter the cursor is on; the column completes it.
        if let Some(c) = screen.cursor_col {
            w.num_field("menu_cursor_col", c as f64);
        }
        w.end_obj();

        // What the last plan actually achieved. This is what makes an agent
        // that cannot see the map viable: it is told precisely what its last
        // intention did.
        w.arr_field("last_actions");
        let start = self.history.len().saturating_sub(self.config.history);
        for o in &self.history[start..] {
            w.begin_obj();
            w.str_field("action", &o.action);
            w.str_field("result", &o.result);
            w.bool_field("changed", o.changed);
            w.end_obj();
        }
        w.end_arr();
        if let Some(reason) = &self.interrupted {
            w.str_field("interrupted", reason);
        }

        w.arr_field("notes");
        // The host's standing instructions first: a rule the run is judged on
        // outranks a remark about the current screen, and a model that reads
        // only the first few notes must read those.
        for b in &self.briefing {
            w.str_item(b);
        }
        for c in &self.complaints {
            w.str_item(c);
        }
        // The single most valuable thing the host can tell an agent that cannot
        // see the picture. "Every direction is a wall" is almost never true; it
        // means the game is not accepting movement at all — a cutscene, a text
        // box, a menu — and an agent told only "blocked" will keep trying
        // directions for ever. Found by watching a real run do exactly that.
        // Standing on a door and wondering why nothing happened. Gen 1 fires a
        // warp when you step *through* it, so arriving sideways along the
        // bottom row of a house leaves you on the mat with nothing to show for
        // it — which a coordinate-only agent cannot possibly work out. Watched
        // a real run try to press A at the door six times in a row.
        if let Some(e) = game
            .exits
            .iter()
            .find(|e| e.x == game.position.x && e.y == game.position.y)
        {
            let dest = observe::map_name(e.to_map);
            w.str_item(&format!(
                // Written with `\` continuations like its neighbours: a
                // literal broken across lines *without* them keeps the source
                // indentation, and this note shipped for a while with two runs
                // of eighteen spaces inside it — visible in any captured
                // snapshot, paid for in tokens on every decision.
                "You are standing ON an exit{}. A door is not opened with A — \
                 walk one more step in the direction it faces (for a door at \
                 the bottom of a room, that is down) to go through it.",
                if dest.is_empty() {
                    String::new()
                } else {
                    format!(" to {dest}")
                }
            ));
        }
        if self.nothing_moved() {
            w.str_item(
                "You did not move at all last time. Walking in every direction \
                 failing usually does NOT mean you are surrounded by walls — it \
                 means the game is not accepting movement: a cutscene, a text \
                 box or a menu is up. Use advance_text, or press A or B, \
                 instead of walking.",
            );
        }
        // The naming keyboard, said by the host at the moment it is on screen
        // rather than only in the standing instructions: a fact recorded in a
        // document was still walked into on a live run, which is the test for
        // promoting it to an enforced, situational note (the same promotion
        // the standing-on-an-exit note earned).
        if screen.naming_keyboard() {
            w.str_item(
                "You are on the NAMING KEYBOARD. A types another letter and \
                 advance_text will refuse to run here. The name typed so far \
                 is fine — press START to finish it and move on.",
            );
        }
        // The same promotion for the screen's own trustworthiness: when the
        // text was withheld, say why in words rather than leaving an empty
        // field to be guessed about.
        match trust {
            observe::Trust::Graphics => w.str_item(
                "The screen is showing GRAPHICS, not text (a logo or artwork \
                 whose tiles overlap the font). screen.text was withheld \
                 because it would be letter salad. Judge by mode and position, \
                 not the screen.",
            ),
            observe::Trust::Settling => w.str_item(
                "The screen was still redrawing when it was read (a scroll, a \
                 fade or printing text), so screen.text was withheld rather \
                 than sent half-drawn. Wait a few frames and look again.",
            ),
            _ => {}
        }
        if screen.yes_no_prompt() {
            w.str_item(
                "A YES/NO box is up. This is a QUESTION, not flowing text — \
                 advance_text refuses to run here so the default answer is \
                 not chosen blindly. Read the dialogue above it, move the \
                 cursor if needed, and answer with A deliberately.",
            );
        }
        match escalation {
            Escalation::Nudge => w.str_item(
                "Nothing has changed for a few decisions — what you are trying is \
                 not working. Try something different.",
            ),
            Escalation::Insist => w.str_item(
                "You are stuck in a loop. Do NOT repeat your last plan. Back out \
                 of any menu with B, and move somewhere you have not been.",
            ),
            _ => {}
        }
        w.end_arr();

        w.obj_field("budget");
        w.num_field("calls_used", self.governor.calls() as f64);
        w.num_field("calls_max", self.config.budget.max_calls as f64);
        w.num_field(
            "fraction_used",
            (self.governor.used_fraction() * 1000.0).round() / 1000.0,
        );
        w.end_obj();

        if let Some(path) = self.capture_frame(machine) {
            w.str_field("frame_png", &path);
        }

        if !self.sent_help {
            self.sent_help = true;
            w.str_field("actions_help", ACTIONS_HELP);
            w.str_field("screen_help", SCREEN_HELP);
        }
        w.end_obj();
        w.finish()
    }

    /// Did the last plan contain movement, all of which achieved nothing?
    fn nothing_moved(&self) -> bool {
        let start = self.history.len().saturating_sub(self.config.history);
        let recent = &self.history[start..];
        let moves: Vec<&Outcome> = recent
            .iter()
            .filter(|o| o.action.starts_with("walk ") || o.action.starts_with("head for"))
            .collect();
        !moves.is_empty() && moves.iter().all(|o| !o.changed)
    }

    /// Write the current frame as a PNG for a driver that wants to look at it.
    ///
    /// The expensive channel, and off unless asked for: the structured fields
    /// above are a few hundred bytes, an image is orders of magnitude more for
    /// a model to read.
    fn capture_frame(&self, machine: &Machine) -> Option<String> {
        let dir = self.config.frame_capture.as_ref()?;
        // The emulator's own encoder, reached through the API: it compresses
        // (unlike the stored-deflate path the 60 Hz wire uses), and asking for
        // it here is what keeps this crate free of an image dependency.
        let png = machine.frame_png()?;
        let path = dir.join("frame.png");
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&path, &png).ok()?;
        Some(path.to_string_lossy().into_owned())
    }

    // -- logging -----------------------------------------------------------

    fn log_line(&mut self, line: &str) {
        if let Some(f) = self.log.as_mut() {
            use std::io::Write as _;
            let _ = writeln!(f, "{line}");
            let _ = f.flush();
        }
    }

    fn log_record(&mut self, r: Record) {
        if self.log.is_none() {
            return;
        }
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("ev", "decision");
        w.num_field("decision", r.decision as f64);
        w.num_field("frame", r.frame as f64);
        w.num_field("ms", r.ms as f64);
        w.str_field("request", &r.request);
        w.str_field("reply", &r.reply);
        if let Some(e) = &r.error {
            w.str_field("error", e);
        }
        if !r.stderr.is_empty() {
            w.str_field("stderr", &r.stderr);
        }
        w.obj_field("usage");
        w.num_field("input_tokens", r.usage.input_tokens as f64);
        w.num_field("output_tokens", r.usage.output_tokens as f64);
        w.num_field("usd", r.usage.usd);
        w.end_obj();
        w.end_obj();
        let line = w.finish();
        self.log_line(&line);
    }
}

/// The action reference, sent once per session.
pub const ACTIONS_HELP: &str = concat!(
    "Reply with one JSON object: ",
    r#"{"goal":"<one short sentence, the standing objective>",
"say":"<one or two sentences of commentary for viewers, plain English>",
"plan":[ up to 12 actions ]}"#,
    "\nActions:\n",
    r#"{"do":"press","button":"a|b|up|down|left|right|start|select","times":1}"#,
    "\n",
    r#"{"do":"walk","dir":"up|down|left|right","steps":1-24}"#,
    "\n",
    r#"{"do":"goto","x":N,"y":N}   - walk toward a coordinate on this map"#,
    "\n",
    r#"{"do":"advance_text"}       - press A through dialogue until it ends"#,
    "\n",
    r#"{"do":"wait","frames":N}    - let an animation finish"#,
    "\nThe plan runs to completion locally and you are asked again when it ",
    "finishes, fails, or the world changes unexpectedly. Prefer few, large ",
    "actions. 'say' is read aloud to strangers watching a stream: no JSON, no ",
    "file paths, no jargon."
);

/// What the `screen` object means. Sent once, beside [`ACTIONS_HELP`] — the
/// fields are useless if nobody says what they promise, and the promises are
/// narrow on purpose (see [`crate::observe::Screen`]).
pub const SCREEN_HELP: &str = concat!(
    "READING THE SCREEN. `screen` is the game's own on-screen text, decoded ",
    "from the tilemap — it is what a player sees, not a summary.\n",
    "  screen.kind      what shape of screen it is, and so what A does:\n",
    "    no-text         nothing on screen is text (the overworld, the title \
screen). An empty screen.text here is a fact, not a failed read.\n",
    "    text            text but nothing asking for a button.\n",
    "    dialogue        the game is waiting: press A (or advance_text).\n",
    "    menu            a cursor is up: A chooses screen.selected.\n",
    "    yes-no          a QUESTION. A answers it with whatever is selected.\n",
    "    naming-keyboard A types a letter; START finishes the name.\n",
    "  screen.text      the visible text, blank lines dropped, indentation \
kept so a two-column menu still reads as two columns.\n",
    "  screen.selected  the exact item the cursor points at (on the naming \
keyboard, the single letter A would type).\n",
    "  screen.trust     'ok' means believe screen.text. Anything else means it \
was WITHHELD and screen.text is empty: 'graphics-not-text' (artwork whose \
tiles overlap the font), 'still-redrawing' (read mid-transition), \
'screen-off'. Do not treat an empty text as an empty screen when trust is \
not ok — look again.\n",
    "  Sprites are NOT in screen.text. People, Poke Balls and other objects \
are drawn over the tilemap and do not appear here at all."
);

/// A stable hash of "the situation", for stuck detection: where the player is
/// and what is on screen. Deliberately **not** the frame number or the clock —
/// those always change, which would make nothing ever look stuck.
fn fingerprint(game: &GameState, screen: &Screen) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in [
        game.position.map,
        game.position.x,
        game.position.y,
        game.badges,
        game.party.len() as u8,
    ] {
        h ^= b as u64;
        h = h.wrapping_mul(0x1000_0000_01b3);
    }
    h ^= screen.hash;
    h.wrapping_mul(0x1000_0000_01b3)
}

/// Small helper so `self.pending.take()` reads cleanly at the two call sites.
trait PairOrDefault {
    fn unwrap_or_default_pair(self) -> (String, Instant);
}

impl PairOrDefault for Option<(String, Instant)> {
    fn unwrap_or_default_pair(self) -> (String, Instant) {
        self.unwrap_or_else(|| (String::new(), Instant::now()))
    }
}

impl Plugin for AgentPlugin {
    fn name(&self) -> &str {
        NAME
    }

    /// It reads memory and presses buttons — and **does not** declare
    /// `WRITE_MEMORY`. An agent that can poke the game is not playing it, and
    /// a stream where the player might be editing RAM is not the same show.
    /// The host enforces that, so it is not a promise in a comment.
    fn capabilities(&self) -> Capability {
        Capability::OBSERVE | Capability::READ_MEMORY | Capability::CONTROL_INPUT
    }

    fn status(&self) -> String {
        self.status_text()
    }

    fn on_attach(&mut self, machine: &mut Machine) {
        self.detected_title = machine.rom_title();
        self.supported = terminalgb_plugin_trainer::Game::detect(
            &self.detected_title,
            machine.global_checksum(),
        )
        .is_some();
        self.obs = Gen1Observer::new(self.supported);
        if self.supported {
            if !self.config.objective.is_empty() {
                self.commentary.set_goal(0, &self.config.objective);
            }
            self.commentary.set_activity(Activity::Starting);
        } else {
            self.commentary.set_activity(Activity::Off);
            self.phase = Phase::Stopped;
        }
    }

    fn on_event(&mut self, event: &Event, machine: &mut Machine) {
        let Event::Frame { number } = *event else {
            return;
        };
        self.frame = number;
        self.on_frame(machine);
    }

    fn reset(&mut self) {
        // A save state was loaded: the plan in flight refers to a world that no
        // longer exists.
        self.driver.cancel();
        self.exec = Executor::new(self.config.limits);
        self.watch = None;
        self.history.clear();
        self.interrupted = None;
        self.phase = Phase::Settling(0);
        self.stuck = Stuck::default();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// Where the plugin currently is, for a frontend that wants to show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseView {
    Settling,
    Thinking,
    Executing,
    Stopped,
}

impl AgentPlugin {
    pub fn phase(&self) -> PhaseView {
        match self.phase {
            Phase::Settling(_) => PhaseView::Settling,
            Phase::Thinking => PhaseView::Thinking,
            Phase::Executing => PhaseView::Executing,
            Phase::Stopped => PhaseView::Stopped,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_agent_never_asks_for_write_memory() {
        let p = AgentPlugin::new(
            Box::new(driver::ScriptedDriver::new(vec![])),
            Config::default(),
        );
        let caps = p.capabilities();
        assert!(caps.contains(Capability::CONTROL_INPUT));
        assert!(caps.contains(Capability::READ_MEMORY));
        assert!(
            !caps.contains(Capability::WRITE_MEMORY),
            "an agent that can poke RAM is not playing the game"
        );
        assert_eq!(caps.describe(), "observe+read+input");
    }

    #[test]
    fn an_unsupported_cartridge_is_refused_with_an_explanation() {
        let mut p = AgentPlugin::new(
            Box::new(driver::ScriptedDriver::new(vec![])),
            Config::default(),
        );
        p.detected_title = "TETRIS".to_string();
        let msg = p.refusal().expect("refused");
        assert!(msg.contains("TETRIS"));
        assert!(msg.contains("Red/Blue"));
    }

    #[test]
    fn the_fingerprint_ignores_time_and_notices_movement() {
        let screen = Screen {
            hash: 42,
            ..Default::default()
        };
        let a = GameState {
            position: observe::Position {
                map: 1,
                x: 2,
                y: 3,
                facing: None,
            },
            ..Default::default()
        };
        let mut b = a.clone();
        assert_eq!(fingerprint(&a, &screen), fingerprint(&b, &screen));
        // Turning on the spot is not progress...
        b.position.facing = Some(observe::Facing::Up);
        assert_eq!(fingerprint(&a, &screen), fingerprint(&b, &screen));
        // ...but moving is.
        b.position.x = 3;
        assert_ne!(fingerprint(&a, &screen), fingerprint(&b, &screen));
        // And so is the screen changing while standing still.
        let other = Screen {
            hash: 43,
            ..Default::default()
        };
        assert_ne!(fingerprint(&a, &screen), fingerprint(&a, &other));
    }

    #[test]
    fn the_status_line_is_safe_to_broadcast() {
        let mut p = AgentPlugin::new(
            Box::new(driver::ScriptedDriver::new(vec![]).with_name("scripted")),
            Config::default(),
        );
        p.supported = true;
        let s = p.status_text();
        assert!(commentary::sanitise(&s, 200).is_some(), "{s}");
        assert!(s.contains("scripted"), "{s}");
    }
}

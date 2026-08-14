//! How the agent drives: the action vocabulary and the local executor.
//!
//! # Why these primitives and not "press A"
//!
//! A round trip to a model takes seconds; the game runs at 59.7 frames a
//! second. An agent that reasons about individual frames spends its whole
//! budget on nothing. So the unit of agency here is not a button press, it is
//! an **intention that cheap local code can carry out over hundreds of frames**:
//!
//! * [`Action::AdvanceText`] — hold a conversation down. One decision instead
//!   of thirty. This is the single biggest saving in the whole design: the
//!   opening of Pokémon Red is mostly dialogue, and at one model call per `A`
//!   press it would cost more than the rest of the game.
//! * [`Action::Walk`] / [`Action::GoTo`] — move, and *know whether it worked*.
//!   Walking is not "press down": it is press, wait for the step animation,
//!   compare coordinates, and notice when a wall stopped you.
//! * [`Action::Press`] with a repeat count — menu navigation.
//! * [`Action::Wait`] — let an animation finish.
//!
//! Every one of them is **bounded**. There is no "repeat until it works": each
//! action carries its own cap, the plan carries a frame cap
//! ([`Limits::plan_frames`]), and an action that hits its cap reports that it
//! did rather than looping. A driver that is wrong about the world therefore
//! costs one plan, not the session.
//!
//! # What the executor reports back
//!
//! Each action produces one line of [`Outcome`] — "walked down 3, blocked after
//! 1 at (5,4)". That log is fed straight into the next snapshot, and it is what
//! makes a blind agent viable: it cannot see the map, but it is told precisely
//! what its last intention achieved.

use crate::input::KeypadKey;
use crate::plugin::Machine;

use super::json::Json;
use super::observe::{Facing, Gen1Observer, Position, Screen};

/// Frames a direction is held for one overworld step. A Gen 1 step animation
/// is 16 frames; holding for the whole of it is what makes one press mean one
/// tile rather than a turn-in-place.
pub const WALK_HOLD: u16 = 16;
/// Frames of quiet after a walk press, so the coordinate read afterwards is
/// the settled one. Measured, not guessed: at a 6-frame gap the coordinate is
/// still being written when it is read, and every step reports "blocked".
pub const WALK_GAP: u16 = 14;
/// Consecutive presses that move nothing before a direction counts as walled.
///
/// **Not one.** A press that finds the player facing another way only turns
/// them, and a press that lands while the game is still ignoring input (just
/// after a cutscene, a warp or a menu closing) does nothing at all. Declaring a
/// wall on the first unchanged reading is wrong roughly half the time, which
/// reads to the agent as "every direction is blocked" — the single most
/// misleading thing this layer could tell it.
pub const WALK_MISSES: u8 = 3;
/// The same for [`Action::GoTo`], which is also trying to route round things.
pub const GOTO_MISSES: u8 = 6;
/// Frames of quiet after an [`Action::Press`], which is what a driver uses to
/// work a menu.
///
/// Longer than [`crate::input::DEFAULT_GAP`] on purpose. Gen 1's
/// `HandleMenuInput` **delays several frames after accepting a press**, so two
/// presses eight frames apart look from outside exactly like the emulator
/// dropping input — measured and written up by the battle-control crew in
/// [`docs/gen1/sharp-edges.md`](../../docs/gen1/sharp-edges.md), whose advice is
/// to space menu presses by around half a second. 24 frames is 0.4 s, which
/// keeps a twenty-press repeat inside one plan's frame budget.
pub const MENU_GAP: u16 = 24;

/// Caps. Every one of them exists because the thing it bounds has a plausible
/// way to run forever.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Emulated frames one plan may take before it is abandoned and the driver
    /// is asked again. At 59.7 fps the default is about 25 seconds of game.
    pub plan_frames: u32,
    /// Most actions one plan may contain.
    pub plan_actions: usize,
    /// Most presses one [`Action::Press`] may repeat.
    pub max_repeat: u8,
    /// Most steps one [`Action::Walk`] or [`Action::GoTo`] may take.
    pub max_steps: u8,
    /// Most `A` presses one [`Action::AdvanceText`] may spend.
    pub max_text_presses: u8,
    /// Frames the screen must stay unchanged before a conversation counts as
    /// finished.
    pub text_settle: u16,
    /// Most frames one [`Action::Wait`] may ask for.
    pub max_wait: u16,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            plan_frames: 1500,
            plan_actions: 12,
            max_repeat: 20,
            max_steps: 24,
            max_text_presses: 40,
            text_settle: 24,
            max_wait: 300,
        }
    }
}

/// One thing the agent wants done.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Tap a button, optionally several times.
    Press {
        button: KeypadKey,
        times: u8,
        hold: u16,
    },
    /// Do nothing for a while — let an animation or a warp finish.
    Wait { frames: u16 },
    /// Take up to `steps` overworld steps in one direction, stopping early on
    /// a wall.
    Walk { dir: KeypadKey, steps: u8 },
    /// Head for a coordinate on the current map, greedily, going round what
    /// blocks it. Gives up rather than wandering.
    GoTo { x: u8, y: u8, max_steps: u8 },
    /// Press through a conversation until the game stops asking for input.
    AdvanceText { max_presses: u8 },
}

impl Action {
    /// Parse one action from a driver's reply, clamping every count to
    /// [`Limits`]. An unknown verb is an error the driver is told about, never
    /// a silent no-op — a driver whose actions vanish cannot learn anything.
    pub fn from_json(v: &Json, lim: &Limits) -> Result<Action, String> {
        let verb = v
            .str_field("do")
            .or_else(|| v.str_field("action"))
            .ok_or_else(|| "action has no \"do\" field".to_string())?
            .trim()
            .to_ascii_lowercase();
        let button = || -> Result<KeypadKey, String> {
            let name = v
                .str_field("button")
                .or_else(|| v.str_field("key"))
                .ok_or_else(|| format!("\"{verb}\" needs a \"button\""))?;
            KeypadKey::parse(name).ok_or_else(|| format!("unknown button \"{name}\""))
        };
        let dir = || -> Result<KeypadKey, String> {
            let name = v
                .str_field("dir")
                .or_else(|| v.str_field("direction"))
                .or_else(|| v.str_field("button"))
                .ok_or_else(|| format!("\"{verb}\" needs a \"dir\""))?;
            match KeypadKey::parse(name) {
                Some(
                    k @ (KeypadKey::Up
                    | KeypadKey::Down
                    | KeypadKey::Left
                    | KeypadKey::Right),
                ) => Ok(k),
                Some(_) => Err(format!("\"{name}\" is not a direction")),
                None => Err(format!("unknown direction \"{name}\"")),
            }
        };
        let num = |k: &str, dflt: u64| v.u64_field(k).unwrap_or(dflt);

        Ok(match verb.as_str() {
            "press" | "tap" | "button" => Action::Press {
                button: button()?,
                times: num("times", num("count", 1)).clamp(1, lim.max_repeat as u64)
                    as u8,
                hold: num("hold", crate::input::DEFAULT_HOLD as u64).clamp(1, 120) as u16,
            },
            "wait" => Action::Wait {
                frames: num("frames", 30).min(lim.max_wait as u64) as u16,
            },
            "walk" | "move" => Action::Walk {
                dir: dir()?,
                steps: num("steps", 1).clamp(1, lim.max_steps as u64) as u8,
            },
            "goto" | "go_to" | "walk_to" => Action::GoTo {
                x: v.u64_field("x")
                    .ok_or_else(|| "\"goto\" needs \"x\"".to_string())?
                    .min(255) as u8,
                y: v.u64_field("y")
                    .ok_or_else(|| "\"goto\" needs \"y\"".to_string())?
                    .min(255) as u8,
                max_steps: num("max_steps", lim.max_steps as u64)
                    .clamp(1, lim.max_steps as u64) as u8,
            },
            "advance_text" | "advance" | "talk" | "text" => Action::AdvanceText {
                max_presses: num("max_presses", lim.max_text_presses as u64)
                    .clamp(1, lim.max_text_presses as u64)
                    as u8,
            },
            other => return Err(format!("unknown action \"{other}\"")),
        })
    }

    /// Short human phrasing, for the commentary panel. Deliberately plain
    /// English: a viewer should read "walk down 3" and not `{"do":"walk",…}`.
    pub fn describe(&self) -> String {
        match self {
            Action::Press {
                button, times: 1, ..
            } => format!("press {}", button.name().to_uppercase()),
            Action::Press { button, times, .. } => {
                format!("press {} x{times}", button.name().to_uppercase())
            }
            Action::Wait { frames } => format!("wait {frames} frames"),
            Action::Walk { dir, steps } => format!("walk {} {steps}", dir.name()),
            Action::GoTo { x, y, .. } => format!("head for ({x},{y})"),
            Action::AdvanceText { .. } => "read through the text".to_string(),
        }
    }
}

/// What actually happened when an action ran.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub action: String,
    pub result: String,
    /// Whether anything observable changed. Used for stuck detection.
    pub changed: bool,
}

/// Per-action working state.
#[derive(Debug, Clone)]
enum Step {
    Idle,
    Press {
        left: u8,
        hold: u16,
    },
    Wait {
        left: u16,
    },
    Walk {
        dir: KeypadKey,
        want: u8,
        taken: u8,
        misses: u8,
        presses: u8,
        last: Option<Position>,
        start: Position,
    },
    GoTo {
        x: u8,
        y: u8,
        budget: u8,
        taken: u8,
        misses: u8,
        presses: u8,
        last: Option<Position>,
        start: Position,
    },
    Text {
        left: u8,
        pressed: u8,
        quiet: u16,
        last_hash: u64,
        /// Hash when the action started, so "did anything happen at all?" can
        /// be reported honestly.
        first_hash: u64,
        /// One opening press has been made. See the comment in `advance`.
        primed: bool,
    },
}

/// Runs a plan, one emulated frame at a time.
///
/// The executor only does work on frames where the input queue is **idle** —
/// while a press is in flight there is nothing to decide and nothing worth
/// reading, so the whole thing costs one boolean test per frame mid-press.
pub struct Executor {
    plan: Vec<Action>,
    idx: usize,
    step: Step,
    frames: u32,
    pub limits: Limits,
    outcomes: Vec<Outcome>,
    /// Set when the plan was cut short, with the reason for the driver.
    aborted: Option<String>,
}

impl Default for Executor {
    fn default() -> Self {
        Executor::new(Limits::default())
    }
}

impl Executor {
    pub fn new(limits: Limits) -> Executor {
        Executor {
            plan: Vec::new(),
            idx: 0,
            step: Step::Idle,
            frames: 0,
            limits,
            outcomes: Vec::new(),
            aborted: None,
        }
    }

    /// Load a plan, replacing anything in flight.
    pub fn start(&mut self, plan: Vec<Action>) {
        self.plan = plan;
        self.plan.truncate(self.limits.plan_actions);
        self.idx = 0;
        self.step = Step::Idle;
        self.frames = 0;
        self.outcomes.clear();
        self.aborted = None;
    }

    pub fn is_running(&self) -> bool {
        self.idx < self.plan.len()
    }

    /// Frames this plan has been running for.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// The action currently being carried out.
    pub fn current(&self) -> Option<&Action> {
        self.plan.get(self.idx)
    }

    pub fn outcomes(&self) -> &[Outcome] {
        &self.outcomes
    }

    pub fn abort_reason(&self) -> Option<&str> {
        self.aborted.as_deref()
    }

    /// Stop the plan now, releasing anything held.
    pub fn abort(&mut self, why: &str, m: &mut Machine) {
        if self.is_running() {
            self.aborted = Some(why.to_string());
        }
        self.idx = self.plan.len();
        self.step = Step::Idle;
        m.clear_input();
    }

    /// Advance one emulated frame. Returns `true` while the plan is still
    /// running.
    pub fn tick(&mut self, m: &mut Machine, obs: &Gen1Observer) -> bool {
        if !self.is_running() {
            return false;
        }
        self.frames += 1;
        if self.frames > self.limits.plan_frames {
            let n = self.frames;
            self.abort(&format!("the plan ran for {n} frames without finishing"), m);
            return false;
        }
        // Nothing to decide while a press is still being performed.
        if !m.input_idle() {
            return true;
        }

        let action = self.plan[self.idx].clone();
        if matches!(self.step, Step::Idle) {
            self.step = self.begin(&action, m, obs);
        }
        if let Some(outcome) = self.advance(&action, m, obs) {
            self.outcomes.push(outcome);
            self.idx += 1;
            self.step = Step::Idle;
        }
        self.is_running()
    }

    fn begin(&self, action: &Action, m: &mut Machine, obs: &Gen1Observer) -> Step {
        match *action {
            Action::Press { times, hold, .. } => Step::Press { left: times, hold },
            Action::Wait { frames } => Step::Wait { left: frames },
            Action::Walk { dir, steps } => Step::Walk {
                dir,
                want: steps,
                taken: 0,
                misses: 0,
                presses: 0,
                last: None,
                start: obs.read(m).position,
            },
            Action::GoTo { x, y, max_steps } => Step::GoTo {
                x,
                y,
                budget: max_steps,
                taken: 0,
                misses: 0,
                presses: 0,
                last: None,
                start: obs.read(m).position,
            },
            Action::AdvanceText { max_presses } => {
                let h = Screen::read(m).hash;
                Step::Text {
                    left: max_presses,
                    pressed: 0,
                    quiet: 0,
                    last_hash: h,
                    first_hash: h,
                    primed: false,
                }
            }
        }
    }

    /// One idle-frame step of the current action. `Some(outcome)` when it is
    /// finished.
    fn advance(
        &mut self,
        action: &Action,
        m: &mut Machine,
        obs: &Gen1Observer,
    ) -> Option<Outcome> {
        let label = action.describe();
        match &mut self.step {
            Step::Idle => None,

            Step::Press { left, hold } => {
                if *left == 0 {
                    return Some(Outcome {
                        action: label,
                        result: "done".to_string(),
                        changed: true,
                    });
                }
                if let Action::Press { button, .. } = action {
                    m.press(*button, *hold, MENU_GAP);
                }
                *left -= 1;
                None
            }

            Step::Wait { left } => {
                if *left == 0 {
                    return Some(Outcome {
                        action: label,
                        result: "done".to_string(),
                        changed: false,
                    });
                }
                *left -= 1;
                None
            }

            Step::Walk {
                dir,
                want,
                taken,
                misses,
                presses,
                last,
                start,
            } => {
                let now = obs.read(m).position;
                if let Some(prev) = *last {
                    if same_tile(prev, now) {
                        *misses += 1;
                    } else {
                        *taken += 1;
                        *misses = 0;
                    }
                }
                let done = *taken >= *want;
                let walled = *misses >= WALK_MISSES;
                // A hard cap on presses as well, so a game that moves the
                // player *without* the coordinate settling cannot loop.
                let spent = *presses >= want.saturating_add(WALK_MISSES + 3);
                if done || walled || spent {
                    let s = *start;
                    let result = if done {
                        format!("walked {taken} step(s) to ({},{})", now.x, now.y)
                    } else if walled {
                        format!(
                            "blocked after {taken} step(s) at ({},{}) — a wall that way",
                            now.x, now.y
                        )
                    } else {
                        format!(
                            "only managed {taken} step(s), now at ({},{})",
                            now.x, now.y
                        )
                    };
                    return Some(Outcome {
                        action: label,
                        result,
                        changed: !same_tile(s, now),
                    });
                }
                *last = Some(now);
                *presses += 1;
                m.press(*dir, WALK_HOLD, WALK_GAP);
                None
            }

            Step::GoTo {
                x,
                y,
                budget,
                taken,
                misses,
                presses,
                last,
                start,
            } => {
                let now = obs.read(m).position;
                if let Some(prev) = *last {
                    if same_tile(prev, now) {
                        *misses += 1;
                    } else {
                        *taken += 1;
                        *misses = 0;
                    }
                }
                let arrived = now.x == *x && now.y == *y;
                let walled = *misses >= GOTO_MISSES;
                let spent = *presses >= budget.saturating_add(GOTO_MISSES + 3);
                if arrived || walled || spent {
                    let s = *start;
                    let result = if arrived {
                        format!("arrived at ({},{}) after {taken} step(s)", now.x, now.y)
                    } else if walled {
                        format!(
                            "stuck at ({},{}) trying to reach ({x},{y}) — walls in the way",
                            now.x, now.y
                        )
                    } else {
                        format!(
                            "ran out of steps at ({},{}), wanted ({x},{y})",
                            now.x, now.y
                        )
                    };
                    return Some(Outcome {
                        action: label,
                        result,
                        changed: !same_tile(s, now),
                    });
                }
                // Greedy, alternating the axis it prefers whenever it is stuck,
                // which is enough to get round furniture with no collision map.
                let prefer_x = (*misses / 2) % 2 == 0;
                let dx = match now.x.cmp(x) {
                    std::cmp::Ordering::Less => Some(KeypadKey::Right),
                    std::cmp::Ordering::Greater => Some(KeypadKey::Left),
                    std::cmp::Ordering::Equal => None,
                };
                let dy = match now.y.cmp(y) {
                    std::cmp::Ordering::Less => Some(KeypadKey::Down),
                    std::cmp::Ordering::Greater => Some(KeypadKey::Up),
                    std::cmp::Ordering::Equal => None,
                };
                let key = if prefer_x { dx.or(dy) } else { dy.or(dx) };
                if let Some(k) = key {
                    *last = Some(now);
                    *presses += 1;
                    m.press(k, WALK_HOLD, WALK_GAP);
                }
                None
            }

            Step::Text {
                left,
                pressed,
                quiet,
                last_hash,
                first_hash,
                primed,
            } => {
                let screen = Screen::read(m);
                if screen.hash != *last_hash {
                    *last_hash = screen.hash;
                    *quiet = 0;
                } else {
                    *quiet += 1;
                }
                // **Always press A once.** Gen 1's opening speech spends long
                // stretches on a still frame with no ▼ arrow — between text
                // boxes, during a sprite animation — and an "advance text"
                // that returns "read 0 boxes" in those moments burns a whole
                // model call to achieve nothing. Measured on a real run: four
                // consecutive decisions wasted that way. One opening press
                // costs nothing anywhere it is wrong, and unblocks the intro.
                if !*primed && *left > 0 {
                    *primed = true;
                    *left -= 1;
                    *pressed += 1;
                    *quiet = 0;
                    m.press(KeypadKey::A, crate::input::DEFAULT_HOLD, 10);
                    return None;
                }
                if screen.waiting && *left > 0 {
                    *left -= 1;
                    *pressed += 1;
                    *quiet = 0;
                    m.press(KeypadKey::A, crate::input::DEFAULT_HOLD, 10);
                    return None;
                }
                // Finished when the game has stopped asking for input and the
                // screen has settled — or when the press budget is spent, which
                // is reported rather than hidden.
                if *quiet >= self.limits.text_settle || *left == 0 {
                    let p = *pressed;
                    let moved = screen.hash != *first_hash;
                    let spent = *left == 0 && screen.waiting;
                    return Some(Outcome {
                        action: label,
                        result: if spent {
                            format!("still more text after {p} presses — it did not end")
                        } else if !moved {
                            "nothing to read — the game was not showing text, and \
                             the screen did not change"
                                .to_string()
                        } else {
                            format!("read through {p} text box(es)")
                        },
                        changed: moved,
                    });
                }
                None
            }
        }
    }
}

/// Same map and same coordinates. Facing is deliberately excluded: turning on
/// the spot is not progress.
fn same_tile(a: Position, b: Position) -> bool {
    a.map == b.map && a.x == b.x && a.y == b.y
}

/// Parse a whole plan, collecting per-action errors so the driver can be told
/// exactly which one it got wrong.
pub fn plan_from_json(items: &[Json], lim: &Limits) -> (Vec<Action>, Vec<String>) {
    let mut plan = Vec::new();
    let mut errors = Vec::new();
    for (i, item) in items.iter().take(lim.plan_actions).enumerate() {
        match Action::from_json(item, lim) {
            Ok(a) => plan.push(a),
            Err(e) => errors.push(format!("action {}: {e}", i + 1)),
        }
    }
    if items.len() > lim.plan_actions {
        errors.push(format!(
            "only the first {} actions of {} were kept",
            lim.plan_actions,
            items.len()
        ));
    }
    (plan, errors)
}

/// Turn a facing into the button that produces it.
pub fn facing_key(f: Facing) -> KeypadKey {
    match f {
        Facing::Down => KeypadKey::Down,
        Facing::Up => KeypadKey::Up,
        Facing::Left => KeypadKey::Left,
        Facing::Right => KeypadKey::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::json;

    fn parse_one(src: &str) -> Result<Action, String> {
        let v = json::parse(src).expect("valid json");
        Action::from_json(&v, &Limits::default())
    }

    #[test]
    fn the_documented_action_shapes_parse() {
        assert_eq!(
            parse_one(r#"{"do":"press","button":"a"}"#).unwrap(),
            Action::Press {
                button: KeypadKey::A,
                times: 1,
                hold: crate::input::DEFAULT_HOLD
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"walk","dir":"down","steps":3}"#).unwrap(),
            Action::Walk {
                dir: KeypadKey::Down,
                steps: 3
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"goto","x":7,"y":1}"#).unwrap(),
            Action::GoTo {
                x: 7,
                y: 1,
                max_steps: Limits::default().max_steps
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"advance_text"}"#).unwrap(),
            Action::AdvanceText {
                max_presses: Limits::default().max_text_presses
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"wait","frames":45}"#).unwrap(),
            Action::Wait { frames: 45 }
        );
    }

    /// A driver asking for something unbounded gets it bounded, silently in the
    /// sense that the game is safe and loudly in the sense that the clamp is
    /// visible in what the action then reports doing.
    #[test]
    fn counts_are_clamped_to_the_limits() {
        let lim = Limits::default();
        assert_eq!(
            parse_one(r#"{"do":"press","button":"a","times":9999}"#).unwrap(),
            Action::Press {
                button: KeypadKey::A,
                times: lim.max_repeat,
                hold: crate::input::DEFAULT_HOLD
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"wait","frames":100000}"#).unwrap(),
            Action::Wait {
                frames: lim.max_wait
            }
        );
        assert_eq!(
            parse_one(r#"{"do":"walk","dir":"up","steps":250}"#).unwrap(),
            Action::Walk {
                dir: KeypadKey::Up,
                steps: lim.max_steps
            }
        );
    }

    /// A malformed action must be *reported*, not dropped: a driver whose
    /// mistakes vanish silently cannot correct them.
    #[test]
    fn bad_actions_are_reported_with_the_index() {
        let v = json::parse(
            r#"[{"do":"press","button":"a"},{"do":"fly"},{"do":"walk","dir":"a"}]"#,
        )
        .unwrap();
        let (plan, errs) = plan_from_json(v.as_array().unwrap(), &Limits::default());
        assert_eq!(plan.len(), 1);
        assert_eq!(errs.len(), 2);
        assert!(errs[0].contains("action 2"), "{errs:?}");
        assert!(errs[0].contains("fly"), "{errs:?}");
        assert!(errs[1].contains("not a direction"), "{errs:?}");
    }

    #[test]
    fn an_over_long_plan_is_truncated_and_says_so() {
        let items: Vec<Json> = (0..40)
            .map(|_| json::parse(r#"{"do":"press","button":"a"}"#).unwrap())
            .collect();
        let lim = Limits::default();
        let (plan, errs) = plan_from_json(&items, &lim);
        assert_eq!(plan.len(), lim.plan_actions);
        assert!(errs[0].contains("only the first"));
    }

    #[test]
    fn descriptions_are_plain_english_for_a_viewer() {
        assert_eq!(
            Action::Walk {
                dir: KeypadKey::Down,
                steps: 3
            }
            .describe(),
            "walk down 3"
        );
        assert_eq!(
            Action::AdvanceText { max_presses: 9 }.describe(),
            "read through the text"
        );
        assert_eq!(
            Action::Press {
                button: KeypadKey::A,
                times: 1,
                hold: 8
            }
            .describe(),
            "press A"
        );
    }
}

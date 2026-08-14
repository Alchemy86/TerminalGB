//! A cinematic script: the button presses that turn a battle into a clip.
//!
//! # Why the steps are what they are
//!
//! The obvious script language is a list of `(frame, button)` pairs, and it is
//! the wrong one. It records *this* run rather than describing the sequence, so
//! it breaks the moment anything upstream shifts by a frame — a different
//! starting save, a longer name to print, a Pokémon whose cry is a few frames
//! longer. It is also unreadable: nobody can tell what `A at 214` is for.
//!
//! So the language is written in terms of what is **on screen**. Gen 1 writes
//! character codes straight into `wTileMap` and loads its font so a tile id is
//! the code of the glyph it draws, which means the tilemap reads back as the
//! text a player can see (see [`super::wram::read_screen`]). `await FIGHT`
//! therefore means exactly what it says, and survives everything a frame count
//! does not.
//!
//! Determinism is not lost by doing this. The emulator is deterministic, so the
//! same starting state and the same script produce the same frames; `await`
//! only removes the *authoring* fragility, not the reproducibility. What it
//! buys is that a script written against one save still works against another.
//!
//! # Grammar
//!
//! One step per `do =` line in the preset file:
//!
//! ```text
//! do = wait 30                 # 30 emulated frames
//! do = press a                 # hold A for 4 frames, then 4 frames released
//! do = press a 8               # hold for 8, release for 8
//! do = mash b 6                # six presses of B
//! do = await FIGHT             # until the screen contains "FIGHT" (600 frames max)
//! do = await "Enemy PIDGEY" 300
//! do = press-until b FIGHT     # press B every half second until FIGHT shows
//! do = await-battle            # until the game is in a battle
//! do = await-overworld         # until the battle has ended
//! ```

use gameboy_plugin_api::KeypadKey;

/// How long a `press` holds a button, and how long it waits afterwards, when
/// the script does not say. The game samples the pad once per frame, so one
/// frame would be enough to register; four is enough for the *game* to treat it
/// as a deliberate press through its own auto-repeat and text-delay logic, and
/// short enough that a clip does not drag.
pub const DEFAULT_HOLD: u32 = 4;
/// Frames an `await` will wait before giving up. Ten seconds: longer than any
/// battle intro, shorter than a hang.
pub const DEFAULT_TIMEOUT: u32 = 600;
/// How often `press-until` presses, and how long it waits before the *first*
/// press. Half a second: Gen 1's menu code delays several frames after
/// accepting a button and does not see one that arrives inside that delay, and
/// a faster mash would also skip messages the viewer is meant to read.
pub const PRESS_UNTIL_PERIOD: u32 = 30;

/// One instruction of a cinematic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// Do nothing for `frames`.
    Wait { frames: u32 },
    /// Hold `key` for `hold` frames, then release it for `gap`.
    Press { key: KeypadKey, hold: u32, gap: u32 },
    /// Wait until the game's own screen contains `text` (compared with
    /// whitespace collapsed and case ignored), or `timeout` frames pass.
    Await { text: String, timeout: u32 },
    /// Wait until `wIsInBattle` is nonzero.
    AwaitBattle { timeout: u32 },
    /// Wait until `wIsInBattle` is zero again — the battle is over.
    AwaitOverworld { timeout: u32 },
    /// Press `key` every `period` frames until `text` appears on screen.
    ///
    /// This is the one step that is genuinely a loop, and it is here because
    /// Gen 1 needs it: several battle messages ("Wild PIDGEY appeared!") hold
    /// until the player presses a button, and *how many* presses it takes
    /// depends on the length of the names involved. Counting presses is exactly
    /// the brittleness `await` exists to avoid, so the press count is the thing
    /// that gets driven by the screen too.
    PressUntil {
        key: KeypadKey,
        text: String,
        period: u32,
        timeout: u32,
    },
}

impl Step {
    /// One line, for a log or the status bar.
    pub fn label(&self) -> String {
        match self {
            Step::Wait { frames } => format!("wait {frames}"),
            Step::Press { key, hold, .. } => {
                format!("press {} x{hold}", key_name(*key))
            }
            Step::Await { text, .. } => format!("await '{text}'"),
            Step::AwaitBattle { .. } => "await battle".into(),
            Step::AwaitOverworld { .. } => "await overworld".into(),
            Step::PressUntil { key, text, .. } => {
                format!("press {} until '{text}'", key_name(*key))
            }
        }
    }
}

pub fn key_name(k: KeypadKey) -> &'static str {
    match k {
        KeypadKey::A => "A",
        KeypadKey::B => "B",
        KeypadKey::Start => "START",
        KeypadKey::Select => "SELECT",
        KeypadKey::Up => "UP",
        KeypadKey::Down => "DOWN",
        KeypadKey::Left => "LEFT",
        KeypadKey::Right => "RIGHT",
    }
}

pub fn key_from_name(s: &str) -> Option<KeypadKey> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "a" => KeypadKey::A,
        "b" => KeypadKey::B,
        "start" => KeypadKey::Start,
        "select" => KeypadKey::Select,
        "up" => KeypadKey::Up,
        "down" => KeypadKey::Down,
        "left" => KeypadKey::Left,
        "right" => KeypadKey::Right,
        _ => return None,
    })
}

/// Parse one `do =` line. The error is the text a user sees, so it names the
/// verb that was not understood rather than "parse error".
pub fn parse_step(line: &str) -> Result<Step, String> {
    let line = line.trim();
    let (verb, rest) = match line.split_once(char::is_whitespace) {
        Some((v, r)) => (v.trim(), r.trim()),
        None => (line, ""),
    };
    let num = |s: &str, default: u32| -> Result<u32, String> {
        if s.is_empty() {
            return Ok(default);
        }
        s.parse::<u32>()
            .map_err(|_| format!("'{s}' is not a number of frames"))
    };
    match verb.to_ascii_lowercase().as_str() {
        "wait" => Ok(Step::Wait {
            frames: num(rest, 60)?,
        }),
        "press" | "mash" => {
            let (name, tail) = match rest.split_once(char::is_whitespace) {
                Some((a, b)) => (a, b.trim()),
                None => (rest, ""),
            };
            let key = key_from_name(name)
                .ok_or_else(|| format!("'{name}' is not a Game Boy button"))?;
            if verb.eq_ignore_ascii_case("mash") {
                // `mash b 6` is six presses; the runner expands it, but a
                // repeat count of zero is a typo worth catching.
                let n = num(tail, 4)?;
                if n == 0 {
                    return Err("'mash' with a count of 0 does nothing".into());
                }
                return Ok(Step::Press {
                    key,
                    hold: DEFAULT_HOLD,
                    // Encoded as a single press repeated by the expander; see
                    // `expand_mash`.
                    gap: n,
                });
            }
            let hold = num(tail, DEFAULT_HOLD)?;
            if hold == 0 {
                return Err("'press' with a hold of 0 frames is never seen".into());
            }
            Ok(Step::Press {
                key,
                hold,
                gap: hold,
            })
        }
        "await" => {
            let (text, timeout) = split_quoted(rest);
            if text.is_empty() {
                return Err("'await' needs some text to wait for".into());
            }
            Ok(Step::Await {
                text,
                timeout: num(&timeout, DEFAULT_TIMEOUT)?,
            })
        }
        "press-until" | "mash-until" => {
            let (name, tail) = match rest.split_once(char::is_whitespace) {
                Some((a, b)) => (a, b.trim()),
                None => (rest, ""),
            };
            let key = key_from_name(name)
                .ok_or_else(|| format!("'{name}' is not a Game Boy button"))?;
            let (text, timeout) = split_quoted(tail);
            if text.is_empty() {
                return Err("'press-until' needs some text to wait for".into());
            }
            Ok(Step::PressUntil {
                key,
                text,
                period: PRESS_UNTIL_PERIOD,
                timeout: num(&timeout, DEFAULT_TIMEOUT)?,
            })
        }
        "await-battle" | "await_battle" => Ok(Step::AwaitBattle {
            timeout: num(rest, DEFAULT_TIMEOUT)?,
        }),
        "await-overworld" | "await_overworld" => Ok(Step::AwaitOverworld {
            timeout: num(rest, DEFAULT_TIMEOUT)?,
        }),
        other => Err(format!(
            "'{other}' is not a script verb (wait, press, mash, await, \
             press-until, await-battle, await-overworld)"
        )),
    }
}

/// `"Enemy PIDGEY" 300` -> (`Enemy PIDGEY`, `300`); `FIGHT` -> (`FIGHT`, ``).
fn split_quoted(s: &str) -> (String, String) {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (rest[..end].to_string(), rest[end + 1..].trim().to_string());
        }
    }
    match s.rsplit_once(char::is_whitespace) {
        // Only treat a trailing token as a timeout if it really is a number,
        // so `await Enemy PIDGEY` still waits for the whole phrase.
        Some((head, tail)) if tail.parse::<u32>().is_ok() => {
            (head.trim().to_string(), tail.to_string())
        }
        _ => (s.to_string(), String::new()),
    }
}

/// Turn a `mash key n` step into `n` ordinary presses. Done at parse time so
/// the runner only ever sees one shape.
fn expand_mash(step: Step) -> Vec<Step> {
    match step {
        Step::Press { key, hold, gap } if gap != hold => (0..gap)
            .map(|_| Step::Press {
                key,
                hold,
                gap: hold,
            })
            .collect(),
        other => vec![other],
    }
}

/// A parsed cinematic.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Script {
    pub steps: Vec<Step>,
}

impl Script {
    pub fn push_line(&mut self, line: &str) -> Result<(), String> {
        self.steps.extend(expand_mash(parse_step(line)?));
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// The worst case, in frames, if every `await` times out. What a headless
    /// runner should budget.
    pub fn max_frames(&self) -> u32 {
        self.steps
            .iter()
            .map(|s| match s {
                Step::Wait { frames } => *frames,
                Step::Press { hold, gap, .. } => hold + gap,
                Step::Await { timeout, .. }
                | Step::AwaitBattle { timeout }
                | Step::AwaitOverworld { timeout }
                | Step::PressUntil { timeout, .. } => *timeout,
            })
            .sum()
    }
}

/// What the machine looks like this frame, as far as a script cares.
pub struct Observation<'a> {
    pub screen: &'a str,
    pub in_battle: bool,
}

/// What the runner wants done to the pad this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad {
    pub key: Option<KeypadKey>,
}

/// Executes a [`Script`] one frame at a time.
///
/// Kept free of any emulator type so it can be unit-tested over a synthetic
/// screen — which is how the timeout and the `await` matcher are proved without
/// a ROM.
#[derive(Debug, Default)]
pub struct Runner {
    script: Script,
    step: usize,
    /// Frames spent inside the current step.
    elapsed: u32,
    /// Set when a step gave up. A timed-out cinematic must say so rather than
    /// quietly producing a clip of the wrong thing.
    timed_out: Option<String>,
    finished: bool,
}

impl Runner {
    pub fn new(script: Script) -> Runner {
        Runner {
            finished: script.is_empty(),
            script,
            step: 0,
            elapsed: 0,
            timed_out: None,
        }
    }

    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// `Some(message)` if a step gave up waiting.
    pub fn timed_out(&self) -> Option<&str> {
        self.timed_out.as_deref()
    }

    /// Progress as `step/total`, for a status line.
    pub fn progress(&self) -> (usize, usize) {
        (
            self.step.min(self.script.steps.len()),
            self.script.steps.len(),
        )
    }

    pub fn current(&self) -> Option<&Step> {
        self.script.steps.get(self.step)
    }

    /// A step by index, so a caller can name the one that just finished.
    pub fn script_step(&self, i: usize) -> Option<&Step> {
        self.script.steps.get(i)
    }

    /// Advance one frame. Returns the button to hold this frame, if any.
    pub fn tick(&mut self, obs: &Observation) -> Pad {
        if self.finished {
            return Pad { key: None };
        }
        loop {
            let Some(step) = self.script.steps.get(self.step).cloned() else {
                self.finished = true;
                return Pad { key: None };
            };
            let (done, pad) = match &step {
                Step::Wait { frames } => (self.elapsed + 1 >= *frames, Pad { key: None }),
                Step::Press { key, hold, gap } => {
                    let total = hold + gap;
                    let pad = if self.elapsed < *hold {
                        Pad { key: Some(*key) }
                    } else {
                        Pad { key: None }
                    };
                    (self.elapsed + 1 >= total, pad)
                }
                Step::Await { text, timeout } => {
                    if screen_contains(obs.screen, text) {
                        (true, Pad { key: None })
                    } else if self.elapsed + 1 >= *timeout {
                        self.timed_out = Some(format!("gave up waiting for '{text}'"));
                        self.finished = true;
                        return Pad { key: None };
                    } else {
                        (false, Pad { key: None })
                    }
                }
                Step::AwaitBattle { timeout } => {
                    if obs.in_battle {
                        (true, Pad { key: None })
                    } else if self.elapsed + 1 >= *timeout {
                        self.timed_out = Some("gave up waiting for the battle".into());
                        self.finished = true;
                        return Pad { key: None };
                    } else {
                        (false, Pad { key: None })
                    }
                }
                Step::PressUntil {
                    key,
                    text,
                    period,
                    timeout,
                } => {
                    if screen_contains(obs.screen, text) {
                        (true, Pad { key: None })
                    } else if self.elapsed + 1 >= *timeout {
                        self.timed_out = Some(format!("gave up pressing for '{text}'"));
                        self.finished = true;
                        return Pad { key: None };
                    } else {
                        // The press lands at the *end* of each period, not the
                        // start. That is what gives the game time to settle
                        // after the previous step: Gen 1's `HandleMenuInput`
                        // delays several frames after accepting a button, and a
                        // press that arrives inside that delay is simply not
                        // seen — which looks exactly like the emulator dropping
                        // input. Waiting first also means a condition that is
                        // already true costs no press at all.
                        let period = (*period).max(DEFAULT_HOLD + 1);
                        let pad = if self.elapsed % period >= period - DEFAULT_HOLD {
                            Pad { key: Some(*key) }
                        } else {
                            Pad { key: None }
                        };
                        (false, pad)
                    }
                }
                Step::AwaitOverworld { timeout } => {
                    if !obs.in_battle {
                        (true, Pad { key: None })
                    } else if self.elapsed + 1 >= *timeout {
                        self.timed_out = Some("the battle did not end in time".into());
                        self.finished = true;
                        return Pad { key: None };
                    } else {
                        (false, Pad { key: None })
                    }
                }
            };
            if done {
                self.step += 1;
                self.elapsed = 0;
                if self.step >= self.script.steps.len() {
                    // Finish on the frame the last step completes, not on the
                    // frame after it: a runner that needs one extra tick to
                    // notice adds a frame to every clip.
                    self.finished = true;
                }
                // An `await` that is already satisfied costs no frame, so a
                // script does not stall one frame per condition. A step that
                // consumes time returns immediately.
                if matches!(
                    step,
                    Step::Await { .. }
                        | Step::AwaitBattle { .. }
                        | Step::AwaitOverworld { .. }
                ) {
                    continue;
                }
                return pad;
            }
            self.elapsed += 1;
            return pad;
        }
    }
}

/// Case- and whitespace-insensitive containment. The tilemap is padded with
/// spaces to twenty columns per row, so `"Enemy PIDGEY"` would otherwise never
/// match text that wraps or is column-aligned.
fn screen_contains(screen: &str, needle: &str) -> bool {
    fn squash(s: &str) -> String {
        let mut out = String::with_capacity(s.len());
        let mut space = false;
        for c in s.chars() {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.extend(c.to_lowercase());
        }
        out
    }
    squash(screen).contains(&squash(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        script: &[&str],
        mut screen_at: impl FnMut(u32) -> (String, bool),
    ) -> Vec<Option<KeypadKey>> {
        let mut s = Script::default();
        for l in script {
            s.push_line(l).unwrap();
        }
        let mut r = Runner::new(s);
        let mut out = Vec::new();
        let mut f = 0;
        while !r.is_finished() && f < 2000 {
            let (screen, in_battle) = screen_at(f);
            let pad = r.tick(&Observation {
                screen: &screen,
                in_battle,
            });
            out.push(pad.key);
            f += 1;
        }
        out
    }

    #[test]
    fn a_press_holds_then_releases_for_the_stated_frames() {
        let keys = run(&["press a 3"], |_| (String::new(), false));
        assert_eq!(
            keys,
            vec![
                Some(KeypadKey::A),
                Some(KeypadKey::A),
                Some(KeypadKey::A),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn mash_expands_to_separate_presses() {
        let mut s = Script::default();
        s.push_line("mash b 3").unwrap();
        assert_eq!(s.steps.len(), 3);
        assert!(s.steps.iter().all(|st| matches!(
            st,
            Step::Press {
                key: KeypadKey::B,
                ..
            }
        )));
    }

    /// The whole reason the language is written against the screen: a step
    /// completes when the text appears, however long that takes.
    #[test]
    fn await_waits_for_the_text_then_the_next_step_runs() {
        let keys = run(&["await FIGHT", "press a 2"], |f| {
            let screen = if f >= 10 {
                "  FIGHT  ITEM  "
            } else {
                "  ...  "
            };
            (screen.to_string(), false)
        });
        // Ten frames of waiting, then the press.
        assert_eq!(keys.len(), 10 + 4);
        assert_eq!(keys[9], None);
        assert_eq!(keys[10], Some(KeypadKey::A));
    }

    #[test]
    fn await_matches_across_padding_and_case() {
        assert!(screen_contains("   enemy    pidgey  ", "Enemy PIDGEY"));
        assert!(screen_contains("FIGHT   PKMN", "fight"));
        assert!(!screen_contains("FIGHT", "FLIGHT"));
    }

    /// A cinematic that never gets its cue must fail loudly. Producing a clip
    /// of the wrong thing silently is the failure mode worth designing out.
    #[test]
    fn a_timeout_is_reported_rather_than_hanging_or_passing() {
        let mut s = Script::default();
        s.push_line("await NEVER 5").unwrap();
        let mut r = Runner::new(s);
        for _ in 0..10 {
            r.tick(&Observation {
                screen: "",
                in_battle: false,
            });
        }
        assert!(r.is_finished());
        assert_eq!(r.timed_out(), Some("gave up waiting for 'NEVER'"));
    }

    #[test]
    fn await_battle_and_await_overworld_bracket_a_fight() {
        let keys = run(&["await-battle", "press a", "await-overworld"], |f| {
            (String::new(), (5..40).contains(&f))
        });
        // Five frames waiting for the battle, eight for the press, then the
        // wait for it to end — which completes on the frame it ends, 40.
        assert_eq!(keys.len(), 41);
        assert_eq!(keys[5], Some(KeypadKey::A));
    }

    #[test]
    fn bad_verbs_and_bad_buttons_are_named() {
        assert!(parse_step("jump 5").unwrap_err().contains("jump"));
        assert!(parse_step("press z").unwrap_err().contains("'z'"));
        assert!(parse_step("press a 0").unwrap_err().contains("never seen"));
        assert!(parse_step("await").unwrap_err().contains("text"));
    }

    #[test]
    fn a_trailing_number_is_a_timeout_but_a_word_is_not() {
        assert_eq!(
            parse_step("await FIGHT 120").unwrap(),
            Step::Await {
                text: "FIGHT".into(),
                timeout: 120
            }
        );
        assert_eq!(
            parse_step("await Enemy PIDGEY").unwrap(),
            Step::Await {
                text: "Enemy PIDGEY".into(),
                timeout: DEFAULT_TIMEOUT
            }
        );
        assert_eq!(
            parse_step("await \"got away\" 90").unwrap(),
            Step::Await {
                text: "got away".into(),
                timeout: 90
            }
        );
    }

    #[test]
    fn max_frames_budgets_the_worst_case() {
        let mut s = Script::default();
        s.push_line("await FIGHT 100").unwrap();
        s.push_line("press a 4").unwrap();
        assert_eq!(s.max_frames(), 108);
    }
}

//! The gesture layer: how a touch screen drives the machine.
//!
//! The target is a touch device, so gestures — swipe, press, hold — are the
//! input vocabulary and buttons are not. Everything above this module speaks
//! [`Gesture`]; everything below it speaks the pad state
//! [`crate::game::Input`] that the ported logic reads, because that is the
//! shape the original's own code samples and changing it would change the
//! game rather than the controls.
//!
//! ## What the original's input semantics actually are
//!
//! Worth stating plainly, because the summary this port was commissioned from
//! describes a different game. Read against the disassembly:
//!
//! | often said | what `surfing_pikachu.asm` does |
//! |---|---|
//! | "Up speeds up" | `SurfingMinigame_SpeedUpPikachu` is called unconditionally every riding frame. Nothing reads Up. Nothing reads Down either. |
//! | "Down triggers a jump" | `SurfingMinigame_TryStartJump` takes no input at all: cross a wave crest at 1.25 px/frame or better and you launch. |
//! | "A-timed airborne tricks" | `SurfingMinigame_DPadAction` reads **Left and Right**, held, and only while airborne. A is never read during play. |
//!
//! `grep -n 'PAD_' surfing_pikachu.asm` returns six lines: Select and Start
//! outside the run, A on the two end screens, and Left/Right in
//! `_DPadAction`. So there are exactly three things for a gesture to mean.
//!
//! ## The mapping
//!
//! | gesture | what it does | why |
//! |---|---|---|
//! | swipe left / right | one spin of the board | a spin is a held direction, and [`SWIPE_FRAMES`] is how long it has to be held to complete one |
//! | press and hold left / right | keep spinning | chains spins into the combo the scoring table rewards |
//! | tap | confirm — dismiss the results or the game-over screen | the original's A |
//! | long press | leave the game | the original's Select |
//!
//! There is deliberately **no accelerate gesture and no jump gesture**: the
//! original has neither, and adding one would change what the game is. If one
//! is ever wanted, this module is the only place that would change — which is
//! the point of it being a layer rather than a frontend detail.

use crate::game::Input;

/// Which way a gesture goes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Left,
    Right,
}

/// What the input device reports.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    /// A flick. The board spins for [`SWIPE_FRAMES`] unless the device
    /// overrides it with [`Gesture::SwipeFor`], which a longer or faster
    /// swipe should.
    Swipe(Dir),
    /// A flick whose length the device measured: hold the spin this many
    /// frames. Saturated at [`MAX_HOLD_FRAMES`] so a wild gesture cannot
    /// latch the controls.
    SwipeFor(Dir, u8),
    /// A finger down, dragged to one side. Holds until [`Gesture::Release`].
    Press(Dir),
    /// The finger came off.
    Release,
    /// A tap: confirm.
    Tap,
    /// A long press: leave.
    LongPress,
}

/// How long one swipe holds the spin for: **one complete rotation of the
/// board**.
///
/// Every frame `SurfingMinigame_DPadAction` sees a held direction it steps
/// `ANIM_OBJ_FRAME_SET` through its fourteen values, and it only sees one on
/// the frames `SurfingPikachu_GetJoypad_3FrameBuffer` refreshes — every other
/// one. Fourteen samples is twenty-eight frames, and it lands on two things at
/// once:
///
/// - the spin **registers**, because the trick counter passes `$b` going left
///   on the twelfth sample and `$d` going right on the fourteenth; and
/// - the board comes back to **exactly the angle it launched at**, because
///   fourteen steps through fourteen frame sets is a full turn.
///
/// So one swipe is worth one trick and costs nothing: measured over four
/// seeds, swiping once per jump scores an extra 50 points every landing and
/// does not change the wipeout count at all. Anything shorter is worth
/// nothing — at 26 frames a right-hand spin expired one sample early, silently
/// — and anything longer leaves the board part-way round, which is what makes
/// holding a direction a gamble.
pub const SWIPE_FRAMES: u8 = 28;

/// Ceiling on a measured swipe, so no gesture can pin the board spinning.
pub const MAX_HOLD_FRAMES: u8 = 120;

/// Turns gestures into the per-frame pad state the ported logic reads.
///
/// Feed it whatever the device produced since the last frame, then call
/// [`Controls::frame`] exactly once per frame.
#[derive(Default, Debug)]
pub struct Controls {
    /// A finger currently down and dragged: held until released.
    pressed: Option<Dir>,
    /// A swipe still playing out, and how many frames are left of it.
    coasting: Option<(Dir, u8)>,
    confirm: bool,
    leave: bool,
}

impl Controls {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one gesture. Safe to call any number of times between frames.
    pub fn feed(&mut self, gesture: Gesture) {
        match gesture {
            Gesture::Swipe(dir) => self.coasting = Some((dir, SWIPE_FRAMES)),
            Gesture::SwipeFor(dir, frames) => {
                self.coasting = Some((dir, frames.min(MAX_HOLD_FRAMES)))
            }
            Gesture::Press(dir) => {
                self.pressed = Some(dir);
                // A press supersedes a swipe still coasting, so reversing
                // direction mid-spin takes effect at once.
                self.coasting = None;
            }
            Gesture::Release => self.pressed = None,
            Gesture::Tap => self.confirm = true,
            Gesture::LongPress => self.leave = true,
        }
    }

    /// The pad state for this frame. Advances the swipe's decay, so call it
    /// once per frame and no more.
    pub fn frame(&mut self) -> Input {
        let dir = self.pressed.or_else(|| self.coasting.map(|(d, _)| d));
        let input = Input {
            left: dir == Some(Dir::Left),
            right: dir == Some(Dir::Right),
            a: std::mem::take(&mut self.confirm),
            select: std::mem::take(&mut self.leave),
        };
        self.coasting = match self.coasting {
            Some((d, n)) if n > 1 => Some((d, n - 1)),
            _ => None,
        };
        input
    }

    /// Whether a spin is currently being driven, for a frontend that wants to
    /// show it.
    pub fn spinning(&self) -> Option<Dir> {
        self.pressed.or_else(|| self.coasting.map(|(d, _)| d))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Game, PikachuState, Status};

    #[test]
    fn a_swipe_holds_then_lets_go() {
        let mut c = Controls::new();
        c.feed(Gesture::Swipe(Dir::Left));
        for f in 0..SWIPE_FRAMES {
            let i = c.frame();
            assert!(i.left, "frame {f} of the swipe should still be held");
            assert!(!i.right);
        }
        assert!(!c.frame().left, "the swipe should have let go by now");
    }

    #[test]
    fn a_press_holds_until_released() {
        let mut c = Controls::new();
        c.feed(Gesture::Press(Dir::Right));
        for _ in 0..500 {
            assert!(c.frame().right);
        }
        c.feed(Gesture::Release);
        assert!(!c.frame().right);
    }

    #[test]
    fn a_press_takes_over_from_a_swipe_immediately() {
        let mut c = Controls::new();
        c.feed(Gesture::Swipe(Dir::Left));
        c.feed(Gesture::Press(Dir::Right));
        let i = c.frame();
        assert!(i.right && !i.left);
    }

    #[test]
    fn tap_and_long_press_are_single_frame_edges() {
        let mut c = Controls::new();
        c.feed(Gesture::Tap);
        c.feed(Gesture::LongPress);
        let i = c.frame();
        assert!(i.a && i.select);
        let i = c.frame();
        assert!(!i.a && !i.select);
    }

    /// Pins the sampling period, which [`SWIPE_FRAMES`] is derived from: if
    /// held input were refreshed every frame, or every third, this count
    /// would move and every swipe length in the game would be wrong.
    #[test]
    fn held_input_reaches_the_trick_counter_every_other_frame() {
        let mut g = Game::new(5);
        // Find a jump, then hold right and count the frames to the first
        // spin. Fourteen samples two frames apart, from a standing start.
        let mut holding = 0u32;
        for _ in 0..40_000 {
            let airborne = g.state() == PikachuState::Jumping;
            g.step(Input {
                right: airborne,
                ..Default::default()
            });
            if airborne {
                holding += 1;
                if g.radness_meter() > 0 {
                    assert!(
                        (26..=SWIPE_FRAMES as u32).contains(&holding),
                        "a right spin registered after {holding} frames of holding; \
                         SWIPE_FRAMES is {SWIPE_FRAMES} and must cover it"
                    );
                    return;
                }
            } else {
                holding = 0;
            }
        }
        panic!("never completed a spin while holding right");
    }

    #[test]
    fn a_measured_swipe_is_capped() {
        let mut c = Controls::new();
        c.feed(Gesture::SwipeFor(Dir::Left, 255));
        let mut held = 0;
        while c.frame().left {
            held += 1;
            assert!(held <= MAX_HOLD_FRAMES + 1, "a swipe latched the controls");
        }
        assert_eq!(held, MAX_HOLD_FRAMES);
    }

    /// The point of [`SWIPE_FRAMES`]: one swipe must be worth exactly one
    /// spin, in the real machine, in both directions.
    fn one_swipe_per_jump(dir: Dir, seed: u32) -> Option<u8> {
        let mut g = Game::new(seed);
        let mut c = Controls::new();
        let mut swiped = false;
        for _ in 0..40_000 {
            let airborne = g.state() == PikachuState::Jumping;
            if airborne && !swiped {
                c.feed(Gesture::Swipe(dir));
                swiped = true;
            }
            let meter = g.radness_meter();
            if g.step(c.frame()) == Status::Finished {
                break;
            }
            // Read the meter on the frame the jump ends: it is zeroed when
            // the next one starts.
            if swiped && !matches!(g.state(), PikachuState::Jumping) {
                return Some(meter);
            }
            if !airborne {
                swiped = false;
            }
        }
        None
    }

    /// Swipe once during a jump and report (angle before, angle after the
    /// swipe finishes). `None` if no jump came round.
    fn angle_across_one_swipe(dir: Dir, seed: u32) -> Option<(u8, u8)> {
        let mut g = Game::new(seed);
        let mut c = Controls::new();
        let mut started: Option<u8> = None;
        for _ in 0..40_000 {
            let airborne = g.state() == PikachuState::Jumping;
            if airborne && started.is_none() {
                started = Some(g.board_frame_set());
                c.feed(Gesture::Swipe(dir));
            }
            if g.step(c.frame()) == Status::Finished {
                return None;
            }
            if let Some(before) = started {
                if c.spinning().is_none() {
                    return Some((before, g.board_frame_set()));
                }
            }
        }
        None
    }

    /// The other half of [`SWIPE_FRAMES`]: a full swipe must leave the board
    /// at the angle it started, or "one swipe per jump" would be a gamble
    /// rather than free points.
    #[test]
    fn one_swipe_returns_the_board_to_the_angle_it_started_at() {
        for dir in [Dir::Left, Dir::Right] {
            let got = angle_across_one_swipe(dir, 3);
            assert!(got.is_some(), "{dir:?}: never got a swipe in during a jump");
            let (before, after) = got.unwrap();
            assert_eq!(
                after, before,
                "{dir:?}: a full swipe left the board part-way round"
            );
        }
    }

    /// The whole point of [`SWIPE_FRAMES`]: one swipe is worth one spin, in
    /// both directions, in the real machine. One frame short and it is worth
    /// nothing at all — which is how the value was got wrong the first time.
    #[test]
    fn one_swipe_is_worth_exactly_one_spin() {
        for dir in [Dir::Left, Dir::Right] {
            for seed in [3u32, 11, 99] {
                let meter = one_swipe_per_jump(dir, seed);
                assert_eq!(
                    meter,
                    Some(1),
                    "{dir:?} seed {seed}: one swipe should be one spin"
                );
            }
        }
    }
}

pub struct Keypad {
    row0: u8,
    row1: u8,
    data: u8,
    pub interrupt: u8,
}

// `KeypadKey` is defined by the plugin API rather than here, because it is
// vocabulary both sides need: a plugin names a button, this emulator presses
// it. Re-exported so `terminalgb::KeypadKey` and `crate::input::KeypadKey` keep
// naming the one and only such type — including across the C FFI, whose
// `#[repr(C)]` discriminants live with the definition.
pub use terminalgb_plugin_api::KeypadKey;

/// One button action waiting to be performed: hold `key` down for `hold`
/// emulated frames, then leave every button up for `gap` frames.
///
/// Both counts are in **frames**, because a frame is the granularity the game
/// actually samples input at — Gen 1 reads the joypad once per VBlank — and it
/// is the only unit an outside driver can reason about without knowing the
/// game's internals.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct QueuedPress {
    pub key: KeypadKey,
    pub hold: u16,
    pub gap: u16,
}

// The tap timings are the API's too: an outside driver has to be able to name
// the same units the emulator schedules in.
pub use terminalgb_plugin_api::{DEFAULT_GAP, DEFAULT_HOLD};

/// The one mechanism for driving the buttons from outside the emulated
/// machine.
///
/// **Why a queue and not `keydown`/`keyup`.** A press is not instantaneous:
/// the game samples the joypad once a frame, a menu needs frames to settle, and
/// holding a direction walks further than tapping it. Anything driving the game
/// programmatically — an AI agent, a scripted test, a battle-control plugin —
/// therefore has to schedule presses *in frames* and wait for them to complete.
/// Every such caller writing its own timer is how two mechanisms diverge, so
/// there is exactly one: enqueue here, and the host advances the queue once per
/// emulated frame.
///
/// This is **not machine state.** It is external intent, in the same category
/// as plugin state (see [`crate::plugin`]), so it is deliberately not part of a
/// save state and is dropped when one is loaded.
#[derive(Default)]
pub struct InputQueue {
    pending: std::collections::VecDeque<QueuedPress>,
    /// Key currently held: the button, frames of hold left, and the gap owed
    /// once it is released.
    current: Option<(KeypadKey, u16, u16)>,
    /// Frames of enforced release left after the last press.
    gap: u16,
    /// Total presses actually started — the number the status panel shows so
    /// "something else is playing my game" is never invisible.
    presses: u64,
}

impl InputQueue {
    /// Schedule a press. `hold`/`gap` are frame counts; zero hold is clamped to
    /// one frame, because a press no frame observes never happened.
    pub fn push(&mut self, key: KeypadKey, hold: u16, gap: u16) {
        self.pending.push_back(QueuedPress {
            key,
            hold: hold.max(1),
            gap,
        });
    }

    /// Schedule a plain tap.
    pub fn tap(&mut self, key: KeypadKey) {
        self.push(key, DEFAULT_HOLD, DEFAULT_GAP);
    }

    /// Nothing queued, nothing held, and no enforced gap outstanding — i.e. the
    /// machine has caught up with everything asked of it. A driver waits for
    /// this before reading state, or it reads the game mid-press.
    pub fn is_idle(&self) -> bool {
        self.pending.is_empty() && self.current.is_none() && self.gap == 0
    }

    /// How many presses are still waiting (excluding one in progress).
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Total presses started since the machine booted.
    pub fn presses(&self) -> u64 {
        self.presses
    }

    /// Advance one emulated frame. Called by the host exactly once per frame,
    /// before the frame runs, so a press is held for the whole of the frame the
    /// game samples it in.
    pub fn advance(&mut self, pad: &mut Keypad) {
        if let Some((key, left, gap)) = self.current {
            if left <= 1 {
                pad.keyup(key);
                self.current = None;
                // The frame the key is released in *is* the first idle frame,
                // so one press costs exactly `hold + gap` frames. Without this
                // the release frame is free and every measured cadence is one
                // frame longer than the caller asked for.
                self.gap = gap.saturating_sub(1);
            } else {
                self.current = Some((key, left - 1, gap));
            }
            return;
        }
        if self.gap > 0 {
            self.gap -= 1;
            return;
        }
        if let Some(p) = self.pending.pop_front() {
            pad.keydown(p.key);
            self.current = Some((p.key, p.hold, p.gap));
            self.presses = self.presses.wrapping_add(1);
        }
    }

    /// Drop everything, releasing whatever is held. Used when a save state is
    /// loaded (the queue describes intent against a machine that no longer
    /// exists) and when a driver's plan is abandoned.
    pub fn reset(&mut self, pad: &mut Keypad) {
        if let Some((key, _, _)) = self.current.take() {
            pad.keyup(key);
        }
        self.pending.clear();
        self.gap = 0;
    }
}

impl Default for Keypad {
    fn default() -> Self {
        Keypad {
            row0: 0x0F,
            row1: 0x0F,
            data: 0xFF,
            interrupt: 0,
        }
    }
}

impl Keypad {
    pub fn rb(&self) -> u8 {
        self.data
    }

    pub fn wb(&mut self, value: u8) {
        self.data = (self.data & 0xCF) | (value & 0x30);
        self.update();
    }

    fn update(&mut self) {
        let old_values = self.data & 0xF;
        let mut new_values = 0xF;

        if self.data & 0x10 == 0x00 {
            new_values &= self.row0;
        }
        if self.data & 0x20 == 0x00 {
            new_values &= self.row1;
        }

        if old_values == 0xF && new_values != 0xF {
            self.interrupt |= 0x10;
        }

        self.data = (self.data & 0xF0) | new_values;
    }

    pub(crate) fn save_state(&self, w: &mut crate::savestate::Writer) {
        w.u8(self.row0);
        w.u8(self.row1);
        w.u8(self.data);
        w.u8(self.interrupt);
    }

    pub(crate) fn load_state(
        &mut self,
        r: &mut crate::savestate::Reader,
    ) -> Result<(), crate::savestate::StateError> {
        self.row0 = r.u8()?;
        self.row1 = r.u8()?;
        self.data = r.u8()?;
        self.interrupt = r.u8()?;
        Ok(())
    }

    pub fn keydown(&mut self, key: KeypadKey) {
        match key {
            KeypadKey::Right => self.row0 &= !(1 << 0),
            KeypadKey::Left => self.row0 &= !(1 << 1),
            KeypadKey::Up => self.row0 &= !(1 << 2),
            KeypadKey::Down => self.row0 &= !(1 << 3),
            KeypadKey::A => self.row1 &= !(1 << 0),
            KeypadKey::B => self.row1 &= !(1 << 1),
            KeypadKey::Select => self.row1 &= !(1 << 2),
            KeypadKey::Start => self.row1 &= !(1 << 3),
        }
        self.update();
    }

    pub fn keyup(&mut self, key: KeypadKey) {
        match key {
            KeypadKey::Right => self.row0 |= 1 << 0,
            KeypadKey::Left => self.row0 |= 1 << 1,
            KeypadKey::Up => self.row0 |= 1 << 2,
            KeypadKey::Down => self.row0 |= 1 << 3,
            KeypadKey::A => self.row1 |= 1 << 0,
            KeypadKey::B => self.row1 |= 1 << 1,
            KeypadKey::Select => self.row1 |= 1 << 2,
            KeypadKey::Start => self.row1 |= 1 << 3,
        }
        self.update();
    }
}

#[cfg(test)]
mod queue_test {
    use super::*;

    /// Row 1 bit 0 is A; it reads low while A is held.
    fn a_is_down(pad: &mut Keypad) -> bool {
        pad.wb(0x10);
        pad.rb() & 0x01 == 0
    }

    #[test]
    fn a_queued_tap_is_held_for_exactly_its_frames_then_released() {
        let mut pad = Keypad::default();
        let mut q = InputQueue::default();
        q.push(KeypadKey::A, 3, 2);
        assert!(!q.is_idle());

        let mut down = Vec::new();
        for _ in 0..8 {
            q.advance(&mut pad);
            down.push(a_is_down(&mut pad));
        }
        // Three frames down, then up; the queue reports idle once the gap has
        // also been served.
        assert_eq!(down, [true, true, true, false, false, false, false, false]);
        assert!(q.is_idle());
        assert_eq!(q.presses(), 1);
    }

    #[test]
    fn consecutive_taps_are_separated_by_the_gap() {
        let mut pad = Keypad::default();
        let mut q = InputQueue::default();
        q.push(KeypadKey::A, 2, 2);
        q.push(KeypadKey::A, 2, 0);
        let mut down = Vec::new();
        for _ in 0..8 {
            q.advance(&mut pad);
            down.push(a_is_down(&mut pad));
        }
        // Without the gap these two taps would look like one four-frame hold,
        // and a menu would advance once instead of twice.
        assert_eq!(down, [true, true, false, false, true, true, false, false]);
        assert!(q.is_idle());
        assert_eq!(q.presses(), 2);
    }

    #[test]
    fn a_zero_length_press_still_lasts_one_frame() {
        let mut pad = Keypad::default();
        let mut q = InputQueue::default();
        q.push(KeypadKey::Start, 0, 0);
        q.advance(&mut pad);
        pad.wb(0x10);
        assert_eq!(pad.rb() & 0x08, 0, "Start is held for the frame");
        q.advance(&mut pad);
        pad.wb(0x10);
        assert_eq!(pad.rb() & 0x08, 0x08, "and released on the next");
    }

    #[test]
    fn reset_releases_whatever_is_held() {
        let mut pad = Keypad::default();
        let mut q = InputQueue::default();
        q.push(KeypadKey::Down, 30, 0);
        q.push(KeypadKey::Down, 30, 0);
        q.advance(&mut pad);
        assert!(!q.is_idle());
        q.reset(&mut pad);
        pad.wb(0x20);
        assert_eq!(pad.rb() & 0x08, 0x08, "Down must not stay stuck down");
        assert!(q.is_idle());
        assert_eq!(q.pending(), 0);
    }

    #[test]
    fn button_names_round_trip() {
        for k in KeypadKey::ALL {
            assert_eq!(KeypadKey::parse(k.name()), Some(k));
        }
        assert_eq!(KeypadKey::parse("  START "), Some(KeypadKey::Start));
        assert_eq!(KeypadKey::parse("x"), None);
    }
}

#[cfg(test)]
mod test {
    use super::KeypadKey;

    #[test]
    fn keys_buttons() {
        let mut keypad = super::Keypad::default();
        let keys0: [KeypadKey; 4] = [
            KeypadKey::A,
            KeypadKey::B,
            KeypadKey::Select,
            KeypadKey::Start,
        ];

        for (i, _keys) in keys0.iter().enumerate() {
            keypad.keydown(keys0[i]);

            keypad.wb(0x00);
            assert_eq!(keypad.rb(), 0xCF & !(1 << i));

            keypad.wb(0x10);
            assert_eq!(keypad.rb(), 0xDF & !(1 << i));

            keypad.wb(0x20);
            assert_eq!(keypad.rb(), 0xEF);

            keypad.wb(0x30);
            assert_eq!(keypad.rb(), 0xFF);

            keypad.keyup(keys0[i]);
        }
    }

    #[test]
    fn keys_direction() {
        let mut keypad = super::Keypad::default();
        let keys1: [KeypadKey; 4] = [
            KeypadKey::Right,
            KeypadKey::Left,
            KeypadKey::Up,
            KeypadKey::Down,
        ];

        for (i, _keys) in keys1.iter().enumerate() {
            keypad.keydown(keys1[i]);

            keypad.wb(0x00);
            assert_eq!(keypad.rb(), 0xCF & !(1 << i));

            keypad.wb(0x10);
            assert_eq!(keypad.rb(), 0xDF);

            keypad.wb(0x20);
            assert_eq!(keypad.rb(), 0xEF & !(1 << i));

            keypad.wb(0x30);
            assert_eq!(keypad.rb(), 0xFF);

            keypad.keyup(keys1[i]);
        }
    }
}

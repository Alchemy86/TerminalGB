//! The console's buttons, and the units a scheduled press is measured in.
//!
//! These live in the plugin API rather than in the emulator because they are
//! part of the contract: a plugin holding [`Capability::CONTROL_INPUT`] names a
//! button, and it must mean the same thing on both sides. The emulator
//! re-exports [`KeypadKey`] as `gameboy::KeypadKey`, which is also its C ABI
//! type — hence `#[repr(C)]`.
//!
//! [`Capability::CONTROL_INPUT`]: crate::Capability::CONTROL_INPUT

/// One of the console's eight buttons.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(C)]
pub enum KeypadKey {
    Right,
    Left,
    Up,
    Down,
    A,
    B,
    Select,
    Start,
}

impl KeypadKey {
    /// Every button, in the order the hardware's two rows list them.
    pub const ALL: [KeypadKey; 8] = [
        KeypadKey::Right,
        KeypadKey::Left,
        KeypadKey::Up,
        KeypadKey::Down,
        KeypadKey::A,
        KeypadKey::B,
        KeypadKey::Select,
        KeypadKey::Start,
    ];

    /// Lower-case name, as an external driver spells it.
    pub fn name(self) -> &'static str {
        match self {
            KeypadKey::Right => "right",
            KeypadKey::Left => "left",
            KeypadKey::Up => "up",
            KeypadKey::Down => "down",
            KeypadKey::A => "a",
            KeypadKey::B => "b",
            KeypadKey::Select => "select",
            KeypadKey::Start => "start",
        }
    }

    /// Parse a button name, case-insensitively. `None` for anything else — an
    /// external driver's typo must be a reported error, never a silent no-op.
    pub fn parse(s: &str) -> Option<KeypadKey> {
        let s = s.trim();
        KeypadKey::ALL
            .into_iter()
            .find(|k| k.name().eq_ignore_ascii_case(s))
    }
}

/// A tap short enough to register as one press and long enough for a game
/// polling once per frame to see it. Eight frames is ~134 ms — a brisk human
/// press.
pub const DEFAULT_HOLD: u16 = 8;
/// Frames with nothing held after a tap, so consecutive presses are two
/// presses and not one long hold.
pub const DEFAULT_GAP: u16 = 8;

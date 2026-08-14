//! What a plugin is allowed to do.

/// What a plugin is allowed to do. Declared by
/// [`Plugin::capabilities`](crate::Plugin::capabilities) and enforced by
/// [`Machine`](crate::Machine) — an undeclared operation fails rather than
/// being trusted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Capability(u8);

impl Capability {
    /// Receive [`Event`](crate::Event)s. Costs one branch per event when unset.
    pub const OBSERVE: Capability = Capability(1 << 0);
    /// Read emulated memory through [`Machine::peek`](crate::Machine::peek).
    pub const READ_MEMORY: Capability = Capability(1 << 1);
    /// Write emulated memory through [`Machine::poke`](crate::Machine::poke).
    /// **Can corrupt a save.**
    pub const WRITE_MEMORY: Capability = Capability(1 << 2);
    /// Drive the serial port as the other end of the link cable.
    pub const SERIAL_LINK: Capability = Capability(1 << 3);
    /// Drive the console's buttons — [`Machine::set_button`](crate::Machine::set_button)
    /// (the raw latch) and [`Machine::press`](crate::Machine::press) (a press
    /// scheduled in frames). The *control* face applied to the joypad rather
    /// than to memory. A plugin with this is playing the game, so the frontend
    /// must show it.
    pub const CONTROL_INPUT: Capability = Capability(1 << 4);

    pub const NONE: Capability = Capability(0);

    pub const fn union(self, other: Capability) -> Capability {
        Capability(self.0 | other.0)
    }

    pub const fn contains(self, other: Capability) -> bool {
        self.0 & other.0 == other.0
    }

    /// The raw bitmask. Stable across the 1.x API: the bit each capability owns
    /// is part of the contract, so a trace or a manifest can record one.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Human-readable list, for the status bar and `--help`-style output.
    pub fn describe(self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if self.contains(Capability::OBSERVE) {
            parts.push("observe");
        }
        if self.contains(Capability::READ_MEMORY) {
            parts.push("read");
        }
        if self.contains(Capability::WRITE_MEMORY) {
            parts.push("write");
        }
        if self.contains(Capability::SERIAL_LINK) {
            parts.push("link");
        }
        if self.contains(Capability::CONTROL_INPUT) {
            parts.push("input");
        }
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join("+")
        }
    }
}

impl std::ops::BitOr for Capability {
    type Output = Capability;
    fn bitor(self, rhs: Capability) -> Capability {
        self.union(rhs)
    }
}

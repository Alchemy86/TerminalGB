//! The ground truth: a timed list of APU register writes.
//!
//! Everything in this crate compiles down to a [`Track`]. A score compiles to
//! one, a `.vgm` log parses into one, and a register script *is* one written
//! out longhand. That is deliberate — the register level is the only layer that
//! cannot be wrong, because it is exactly what a cartridge puts on the bus.

use core::fmt;

/// The DMG's master clock, in Hz. Every time in this crate is a count of these
/// T-cycles, because that is the unit the APU is clocked in.
pub const CLOCK_HZ: u64 = 4_194_304;

/// First APU register (`NR10`).
pub const APU_BASE: u16 = 0xFF10;
/// Last address the APU answers for (`WAVE15`).
pub const APU_TOP: u16 = 0xFF3F;

/// Which console's APU to render on. The two differ in real, audible ways —
/// see [`Model::describe`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Model {
    /// Original Game Boy / Game Boy Pocket.
    #[default]
    Dmg,
    /// Game Boy Color.
    Cgb,
}

impl Model {
    pub fn parse(s: &str) -> Option<Model> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dmg" | "gb" | "classic" => Some(Model::Dmg),
            "cgb" | "gbc" | "color" => Some(Model::Cgb),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::Dmg => "dmg",
            Model::Cgb => "cgb",
        }
    }

    /// The differences a composer can actually hear or measure. Kept short and
    /// honest: these are the ones this renderer models, not a wish list.
    pub fn describe(self) -> &'static str {
        match self {
            Model::Dmg => {
                "DMG: length counters survive an APU power-off and NRx1 length \
                 loads still land while the APU is off; wave RAM is only \
                 readable in a narrow window around channel 3's own fetch."
            }
            Model::Cgb => {
                "CGB: an APU power-off clears the length counters and NRx1 \
                 writes while off are ignored; wave RAM reads always return the \
                 byte channel 3 is on."
            }
        }
    }
}

/// One write to one APU register, at an exact time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RegisterWrite {
    /// T-cycles from the start of the render (4,194,304 per second).
    pub at: u64,
    /// Absolute address, `0xFF10..=0xFF3F`.
    pub reg: u16,
    pub value: u8,
}

impl RegisterWrite {
    pub fn new(at: u64, reg: u16, value: u8) -> RegisterWrite {
        RegisterWrite { at, reg, value }
    }
}

impl fmt::Display for RegisterWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:>10}  {:<7} ${:02X}",
            self.at,
            reg_name(self.reg).unwrap_or("?"),
            self.value
        )
    }
}

/// A complete piece: what to write, when, for how long, and on which machine.
#[derive(Clone, Debug)]
pub struct Track {
    /// Sorted by `at` (stably, so two writes at the same cycle keep the order
    /// they were given in — which matters: `NR13` then `NR14` triggers, the
    /// other way round does not).
    pub writes: Vec<RegisterWrite>,
    /// Total length to render, in T-cycles.
    pub duration: u64,
    pub model: Model,
    /// A human-readable name for the piece, if the source carried one.
    pub title: Option<String>,
}

impl Default for Track {
    fn default() -> Self {
        Track::new()
    }
}

impl Track {
    pub fn new() -> Track {
        Track {
            writes: Vec::new(),
            duration: 0,
            model: Model::Dmg,
            title: None,
        }
    }

    pub fn push(&mut self, at: u64, reg: u16, value: u8) {
        self.writes.push(RegisterWrite::new(at, reg, value));
    }

    /// Sort into render order. **Stable**, so same-cycle writes keep their
    /// source order — the register file is not commutative.
    pub fn sort(&mut self) {
        self.writes.sort_by_key(|w| w.at);
    }

    /// Seconds of audio this track renders to.
    pub fn seconds(&self) -> f64 {
        self.duration as f64 / CLOCK_HZ as f64
    }

    /// Reject anything the APU would not see, so a typo is an error rather than
    /// a silently missing note.
    pub fn validate(&self) -> Result<(), String> {
        for w in &self.writes {
            if !(APU_BASE..=APU_TOP).contains(&w.reg) {
                return Err(format!(
                    "register ${:04X} is outside the APU's ${APU_BASE:04X}-${APU_TOP:04X}",
                    w.reg
                ));
            }
            if w.at > self.duration {
                return Err(format!(
                    "a write at {} T-cycles falls past the track's {} T-cycle end",
                    w.at, self.duration
                ));
            }
        }
        Ok(())
    }

    /// The register writes, longhand, in the `.gbregs` text format this crate
    /// also reads back. This is the "show me exactly what the hardware gets"
    /// output, and it round-trips.
    pub fn to_register_script(&self) -> String {
        let mut s = String::new();
        s.push_str(
            "# Game Boy APU register script — times are T-cycles at 4194304 Hz.\n",
        );
        if let Some(t) = &self.title {
            s.push_str("# title: ");
            s.push_str(t);
            s.push('\n');
        }
        s.push_str("model ");
        s.push_str(self.model.name());
        s.push('\n');
        for w in &self.writes {
            let name = reg_name(w.reg)
                .map(|n| n.to_string())
                .unwrap_or_else(|| format!("${:04X}", w.reg));
            s.push_str(&format!("{}t {} ${:02X}\n", w.at, name, w.value));
        }
        s.push_str(&format!("end {}t\n", self.duration));
        s
    }
}

/// Canonical name of an APU register, for printing and for the script format.
pub fn reg_name(addr: u16) -> Option<&'static str> {
    const NAMES: [&str; 0x17] = [
        "NR10", "NR11", "NR12", "NR13", "NR14", "FF15", "NR21", "NR22", "NR23", "NR24",
        "NR30", "NR31", "NR32", "NR33", "NR34", "FF1F", "NR41", "NR42", "NR43", "NR44",
        "NR50", "NR51", "NR52",
    ];
    const WAVE: [&str; 16] = [
        "WAVE0", "WAVE1", "WAVE2", "WAVE3", "WAVE4", "WAVE5", "WAVE6", "WAVE7", "WAVE8",
        "WAVE9", "WAVE10", "WAVE11", "WAVE12", "WAVE13", "WAVE14", "WAVE15",
    ];
    match addr {
        0xFF10..=0xFF26 => Some(NAMES[(addr - 0xFF10) as usize]),
        0xFF30..=0xFF3F => Some(WAVE[(addr - 0xFF30) as usize]),
        _ => None,
    }
}

/// Look a register up by name, case-insensitively. Also accepts a raw address
/// as `$FF10`, `0xFF10` or `FF10`.
pub fn reg_addr(name: &str) -> Option<u16> {
    let up = name.trim().to_ascii_uppercase();
    for a in APU_BASE..=APU_TOP {
        if reg_name(a) == Some(up.as_str()) {
            return Some(a);
        }
    }
    // WAVE RAM is also commonly written WAVE_RAM+n / WAV0.
    if let Some(rest) = up.strip_prefix("WAV") {
        let rest = rest.strip_prefix('E').unwrap_or(rest);
        if let Ok(n) = rest.parse::<u16>() {
            if n < 16 {
                return Some(0xFF30 + n);
            }
        }
    }
    let hex = up
        .strip_prefix('$')
        .or_else(|| up.strip_prefix("0X"))
        .unwrap_or(&up);
    u16::from_str_radix(hex, 16).ok()
}

/// The registers a driver writes to bring the chip up: power on, both sides at
/// full master volume, every channel routed to both. A cartridge does this
/// itself; a score is not a cartridge, so the score compiler emits it.
///
/// Order matters and is hardware's: **nothing but `NR52` lands while the APU is
/// powered off**, so the power-on write has to be first.
pub const POWER_ON: [(u16, u8); 3] = [
    (0xFF26, 0x80), // NR52: APU on
    (0xFF24, 0x77), // NR50: master volume 7 both sides, no VIN
    (0xFF25, 0xFF), // NR51: all four channels to both sides
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_names_round_trip() {
        for a in APU_BASE..=APU_TOP {
            if let Some(n) = reg_name(a) {
                assert_eq!(reg_addr(n), Some(a), "{n}");
            }
        }
        assert_eq!(reg_addr("$FF10"), Some(0xFF10));
        assert_eq!(reg_addr("0xff26"), Some(0xFF26));
        assert_eq!(reg_addr("nr52"), Some(0xFF26));
        assert_eq!(reg_addr("wav3"), Some(0xFF33));
        assert_eq!(reg_addr("WAVE15"), Some(0xFF3F));
    }

    #[test]
    fn sorting_is_stable_so_a_trigger_stays_after_its_frequency() {
        let mut t = Track::new();
        t.push(100, 0xFF13, 0xC1);
        t.push(100, 0xFF14, 0x87);
        t.push(0, 0xFF26, 0x80);
        t.sort();
        assert_eq!(t.writes[0].reg, 0xFF26);
        assert_eq!(t.writes[1].reg, 0xFF13);
        assert_eq!(t.writes[2].reg, 0xFF14);
    }

    #[test]
    fn a_write_outside_the_apu_is_refused() {
        let mut t = Track::new();
        t.duration = 1000;
        t.push(0, 0xFF40, 0x91);
        assert!(t.validate().is_err());
    }
}

/// The **resolved** hardware mode the emulated machine is running in.
///
/// This is a consequence of two things: which console was booted (see
/// [`Model`]) and what the cartridge header asks for.
///
/// * `Classic` — an original Game Boy (DMG). No CGB registers, DMG boot
///   registers (`A=0x01`, `HL=0x014D`).
/// * `Color` — a Game Boy Color running a CGB-aware cartridge.
/// * `ColorAsClassic` — **CGB hardware** running a DMG-only cartridge, i.e. the
///   Color console's compatibility mode. It is *not* the same machine as
///   `Classic`: the boot registers differ (`A=0x11`, `HL=0x007C`) and the
///   undocumented CGB registers `FF72`/`FF73`/`FF75`-`FF77` still respond.
///   Mooneye's `acceptance/bits/reg_f` and friends print those boot registers
///   and so distinguish the two.
#[derive(PartialEq, Debug, Copy, Clone)]
pub enum GbMode {
    Classic,
    Color,
    ColorAsClassic,
}

/// Which **console** to boot. Chosen before the cartridge is examined, unlike
/// [`GbMode`] which is the resolved result.
///
/// The Game Boy library is not one machine: a DMG-only cartridge behaves
/// differently on a DMG than it does in a Color console's compatibility mode,
/// and test ROMs that print their boot registers can tell. Historically this
/// emulator always booted a Color console ([`GbMode::Color`] /
/// [`GbMode::ColorAsClassic`]) and had no way to be a DMG at all.
///
/// [`Model::Auto`] does what a person with both consoles would do: a cartridge
/// whose CGB flag (`0x0143` bit 7) is set goes in the Color, everything else
/// goes in the DMG.
#[derive(PartialEq, Eq, Debug, Copy, Clone, Default)]
pub enum Model {
    /// Pick from the cartridge header. This is the default.
    #[default]
    Auto,
    /// Force an original Game Boy (DMG), whatever the header says.
    Dmg,
    /// Force a Game Boy Color, whatever the header says.
    Cgb,
}

/// Environment override consulted by [`Model::resolve`] when the caller asked
/// for [`Model::Auto`]. Frontends therefore all honour it without each having
/// to grow a flag.
pub const MODEL_ENV: &str = "GB_MODEL";

impl Model {
    /// Parse a user-supplied model name. Accepts the names the community uses:
    /// `auto`, `dmg`/`gb`/`classic`, `cgb`/`gbc`/`color`. Case-insensitive.
    pub fn parse(s: &str) -> Option<Model> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Model::Auto),
            "dmg" | "gb" | "classic" => Some(Model::Dmg),
            "cgb" | "gbc" | "color" | "colour" => Some(Model::Cgb),
            _ => None,
        }
    }

    /// Read `$GB_MODEL`. An unset or unparseable value yields `None` (the
    /// caller's own choice stands) — a typo must never silently switch console.
    pub fn from_env() -> Option<Model> {
        std::env::var(MODEL_ENV).ok().and_then(|v| {
            let m = Model::parse(&v);
            if m.is_none() {
                eprintln!(
                    "warning: {MODEL_ENV}={v:?} is not one of auto/dmg/cgb — ignoring"
                );
            }
            m
        })
    }

    /// Resolve to a concrete [`GbMode`] for a cartridge whose header byte
    /// `0x0143` is `cgb_flag`.
    ///
    /// `Auto` consults `$GB_MODEL` first (so a user override beats detection),
    /// then the header: bit 7 set means the cartridge knows about the Color
    /// hardware, so boot a Color; otherwise boot a DMG.
    ///
    /// A forced `Dmg` is honoured even for a CGB-*only* cartridge (`0x143 ==
    /// 0xC0`). That is deliberate: forcing means forcing, and what such a
    /// cartridge does on a DMG is exactly what a test would want to observe.
    pub fn resolve(self, cgb_flag: u8) -> GbMode {
        self.resolve_with(cgb_flag, Model::from_env())
    }

    /// The whole of [`resolve`](Self::resolve) with the environment passed in,
    /// so it can be tested without mutating process-global state.
    fn resolve_with(self, cgb_flag: u8, env: Option<Model>) -> GbMode {
        let chosen = match self {
            Model::Auto => {
                env.filter(|m| *m != Model::Auto)
                    .unwrap_or(if cgb_flag & 0x80 != 0 {
                        Model::Cgb
                    } else {
                        Model::Dmg
                    })
            }
            explicit => explicit,
        };
        match chosen {
            Model::Dmg => GbMode::Classic,
            // A Color console running a cartridge that does not advertise CGB
            // support runs in compatibility mode.
            Model::Cgb | Model::Auto => {
                if cgb_flag & 0x80 != 0 {
                    GbMode::Color
                } else {
                    GbMode::ColorAsClassic
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The three headers that exist in the wild: DMG-only, CGB-enhanced (still
    /// runs on a DMG), CGB-only.
    const DMG_ONLY: u8 = 0x00;
    const CGB_ENHANCED: u8 = 0x80;
    const CGB_ONLY: u8 = 0xC0;

    #[test]
    fn auto_puts_the_cartridge_in_the_console_its_header_asks_for() {
        assert_eq!(Model::Auto.resolve_with(DMG_ONLY, None), GbMode::Classic);
        assert_eq!(Model::Auto.resolve_with(CGB_ENHANCED, None), GbMode::Color);
        assert_eq!(Model::Auto.resolve_with(CGB_ONLY, None), GbMode::Color);
    }

    #[test]
    fn a_dmg_only_cartridge_in_a_color_console_gets_compatibility_mode() {
        // Not the same machine as a real DMG: different boot registers, and the
        // undocumented CGB registers still respond. This is what the emulator
        // used to do for *every* cartridge.
        assert_eq!(
            Model::Cgb.resolve_with(DMG_ONLY, None),
            GbMode::ColorAsClassic
        );
    }

    #[test]
    fn forcing_a_model_beats_the_header() {
        assert_eq!(Model::Dmg.resolve_with(CGB_ENHANCED, None), GbMode::Classic);
        // Forcing means forcing, even for a cartridge that says it will not run
        // on a DMG — observing what it does there is the point of forcing.
        assert_eq!(Model::Dmg.resolve_with(CGB_ONLY, None), GbMode::Classic);
        assert_eq!(
            Model::Cgb.resolve_with(DMG_ONLY, None),
            GbMode::ColorAsClassic
        );
    }

    #[test]
    fn the_environment_overrides_detection_but_never_an_explicit_caller() {
        assert_eq!(
            Model::Auto.resolve_with(DMG_ONLY, Some(Model::Cgb)),
            GbMode::ColorAsClassic
        );
        assert_eq!(
            Model::Auto.resolve_with(CGB_ENHANCED, Some(Model::Dmg)),
            GbMode::Classic
        );
        // An explicit caller choice wins: a frontend that means CGB gets CGB.
        assert_eq!(
            Model::Cgb.resolve_with(CGB_ENHANCED, Some(Model::Dmg)),
            GbMode::Color
        );
        // `GB_MODEL=auto` is not an override.
        assert_eq!(
            Model::Auto.resolve_with(DMG_ONLY, Some(Model::Auto)),
            GbMode::Classic
        );
    }

    #[test]
    fn model_names_are_the_ones_the_community_uses() {
        for s in ["dmg", "DMG", " gb ", "classic"] {
            assert_eq!(Model::parse(s), Some(Model::Dmg), "{s:?}");
        }
        for s in ["cgb", "GBC", "color", "colour"] {
            assert_eq!(Model::parse(s), Some(Model::Cgb), "{s:?}");
        }
        assert_eq!(Model::parse("auto"), Some(Model::Auto));
        // A typo must be rejected, not silently treated as a console.
        assert_eq!(Model::parse("dgm"), None);
        assert_eq!(Model::parse("sgb"), None);
    }
}

#[derive(PartialEq, Copy, Clone)]
pub enum GbSpeed {
    Single,
    Double,
}

impl GbMode {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            GbMode::Classic => 0,
            GbMode::Color => 1,
            GbMode::ColorAsClassic => 2,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<GbMode, crate::savestate::StateError> {
        match v {
            0 => Ok(GbMode::Classic),
            1 => Ok(GbMode::Color),
            2 => Ok(GbMode::ColorAsClassic),
            _ => Err(crate::savestate::StateError::BadField("GbMode")),
        }
    }
}

impl GbSpeed {
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            GbSpeed::Single => 0,
            GbSpeed::Double => 1,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<GbSpeed, crate::savestate::StateError> {
        match v {
            0 => Ok(GbSpeed::Single),
            1 => Ok(GbSpeed::Double),
            _ => Err(crate::savestate::StateError::BadField("GbSpeed")),
        }
    }
}

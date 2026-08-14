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
/// * `Super` / `Super2` — a **Super Game Boy** (or the later SGB2). The Game
///   Boy side is DMG hardware, so the PPU, APU and I/O map behave as a DMG's;
///   what differs is the boot registers and the ICD2 chip listening on the
///   joypad port (see [`crate::sgb`]). The two SGB revisions differ only in the
///   boot value of `A`, which is precisely what Mooneye's `boot_regs-sgb` and
///   `boot_regs-sgb2` measure — so they are separate machines here.
#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum GbMode {
    Classic,
    Color,
    ColorAsClassic,
    Super,
    Super2,
}

impl GbMode {
    /// True for every machine that presents **DMG** hardware to the cartridge:
    /// an original Game Boy and both Super Game Boys.
    ///
    /// A CGB in compatibility mode is deliberately **not** one of these. It
    /// renders the same picture but it is Color silicon: the undocumented
    /// `FF72`/`FF73`/`FF75`-`FF77` registers still respond, it has no OAM
    /// corruption bug, and its boot ROM leaves a different machine behind.
    pub fn is_dmg(self) -> bool {
        matches!(self, GbMode::Classic | GbMode::Super | GbMode::Super2)
    }

    /// True on a Super Game Boy, where the joypad port is also a command
    /// channel to the SNES side ([`crate::sgb`]).
    pub fn is_sgb(self) -> bool {
        matches!(self, GbMode::Super | GbMode::Super2)
    }
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
    /// Force a Super Game Boy. Never chosen by [`Model::Auto`]: an SGB is a
    /// peripheral a player opts into, and this emulator shows the Game Boy
    /// screen rather than the SNES's border and palettes, so booting one by
    /// accident would give a game SGB boot registers and none of the display it
    /// asked for. Conformance runners select it explicitly, as the GB Emulator
    /// Shootout's own manifest does.
    Sgb,
    /// Force the later SGB2. Identical to [`Model::Sgb`] except for the boot
    /// value of `A`.
    Sgb2,
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
            "sgb" | "super" => Some(Model::Sgb),
            "sgb2" | "super2" => Some(Model::Sgb2),
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
                    "warning: {MODEL_ENV}={v:?} is not one of auto/dmg/cgb/sgb/sgb2 \
                     — ignoring"
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
            Model::Sgb => GbMode::Super,
            Model::Sgb2 => GbMode::Super2,
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
        for s in ["sgb", "SGB", "super"] {
            assert_eq!(Model::parse(s), Some(Model::Sgb), "{s:?}");
        }
        assert_eq!(Model::parse("sgb2"), Some(Model::Sgb2));
        assert_eq!(Model::parse("auto"), Some(Model::Auto));
        // A typo must be rejected, not silently treated as a console.
        assert_eq!(Model::parse("dgm"), None);
        assert_eq!(Model::parse("sgb3"), None);
    }

    /// An SGB is never inferred. A cartridge's SGB flag (`$0146 == 0x03`) says
    /// the game has a border and a palette to offer, not that the player owns
    /// the peripheral — and this emulator shows the Game Boy screen, so booting
    /// one by accident would change a game's boot registers and give it nothing
    /// in return.
    #[test]
    fn an_sgb_is_only_ever_asked_for_explicitly() {
        assert_eq!(Model::Auto.resolve_with(DMG_ONLY, None), GbMode::Classic);
        assert_eq!(
            Model::Auto.resolve_with(DMG_ONLY, Some(Model::Sgb)),
            GbMode::Super
        );
        assert_eq!(Model::Sgb.resolve_with(DMG_ONLY, None), GbMode::Super);
        assert_eq!(Model::Sgb2.resolve_with(DMG_ONLY, None), GbMode::Super2);
        // Forcing means forcing, here too.
        assert_eq!(Model::Sgb.resolve_with(CGB_ONLY, None), GbMode::Super);
    }

    #[test]
    fn both_super_game_boys_are_dmg_hardware_to_the_cartridge() {
        for m in [GbMode::Classic, GbMode::Super, GbMode::Super2] {
            assert!(m.is_dmg(), "{m:?}");
        }
        for m in [GbMode::Color, GbMode::ColorAsClassic] {
            assert!(!m.is_dmg(), "{m:?}");
        }
        assert!(GbMode::Super.is_sgb() && GbMode::Super2.is_sgb());
        assert!(!GbMode::Classic.is_sgb());
    }

    /// Save states carry the mode as a byte; every machine must survive the
    /// round trip or a state loads as the wrong console.
    #[test]
    fn every_mode_round_trips_through_a_save_state() {
        for m in [
            GbMode::Classic,
            GbMode::Color,
            GbMode::ColorAsClassic,
            GbMode::Super,
            GbMode::Super2,
        ] {
            assert_eq!(GbMode::from_u8(m.to_u8()).unwrap(), m, "{m:?}");
        }
        assert!(GbMode::from_u8(5).is_err());
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
            GbMode::Super => 3,
            GbMode::Super2 => 4,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Result<GbMode, crate::savestate::StateError> {
        match v {
            0 => Ok(GbMode::Classic),
            1 => Ok(GbMode::Color),
            2 => Ok(GbMode::ColorAsClassic),
            3 => Ok(GbMode::Super),
            4 => Ok(GbMode::Super2),
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

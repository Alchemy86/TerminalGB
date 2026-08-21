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
///
/// Every value except [`Model::Auto`] names one physical console **revision**.
/// The revisions matter because a boot ROM is part of the silicon: `Dmg0` and
/// `Dmg` are the same hardware running different boot code, so they present an
/// identical machine to a cartridge from `PC=$0100` onwards and differ only in
/// the state they arrive there with. That is why the revisions are *not* part
/// of [`GbMode`] and are not serialised into a save state — they are consumed
/// once, at construction, by [`crate::mmu::MemoryManagementUnit::set_initial`]
/// and [`crate::cpu::registers::Registers::boot`].
#[derive(PartialEq, Eq, Debug, Copy, Clone, Default)]
pub enum Model {
    /// Pick from the cartridge header. This is the default.
    #[default]
    Auto,
    /// Force an original Game Boy (DMG revisions A, B and C — the ones that
    /// shipped in quantity), whatever the header says.
    Dmg,
    /// The **first** DMG revision, whose boot ROM is a different program: it
    /// has no header-checksum lock-up, so it leaves different registers, a
    /// different system counter and the PPU at a different point in its frame.
    /// Mooneye's `boot_regs-dmg0`, `boot_div-dmg0` and `boot_hwio-dmg0` are the
    /// three ROMs that tell it apart from [`Model::Dmg`], and each of the three
    /// fails on the other machine.
    Dmg0,
    /// The Game Boy Pocket (MGB). Same boot ROM as [`Model::Dmg`] but for one
    /// byte: it hands over `A = $FF` instead of `$01`, which is how a cartridge
    /// detects the smaller console. `boot_regs-mgb`.
    Mgb,
    /// Force a Game Boy Color (CPU CGB revisions A-E), whatever the header
    /// says.
    Cgb,
    /// The **first** CGB revision. Identical to [`Model::Cgb`] except that its
    /// boot ROM is a few M-cycles shorter, which `misc/boot_div-cgb0` measures
    /// through the system counter. (The APU also differs between CGB
    /// revisions — that axis is [`crate::sound::CgbRevision`], selected
    /// separately, because SameSuite splits the revisions differently again.)
    Cgb0,
    /// A Game Boy Advance (or AGS) running a Game Boy cartridge. It is CGB
    /// hardware with a third boot ROM: `B` arrives holding `$01` rather than
    /// `$00` — the bit a cartridge tests to offer AGB-specific colours — and
    /// the flags register carries the result of the `inc b` that put it there.
    /// `misc/boot_regs-A`, `misc/boot_div-A`.
    Agb,
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
    /// `auto`, `dmg`/`gb`/`classic`, `cgb`/`gbc`/`color`, and the revision
    /// names the accuracy suites use (`dmg0`, `mgb`, `cgb0`, `agb`/`ags`).
    /// Case-insensitive.
    pub fn parse(s: &str) -> Option<Model> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => Some(Model::Auto),
            "dmg" | "gb" | "classic" => Some(Model::Dmg),
            "dmg0" => Some(Model::Dmg0),
            "mgb" | "pocket" => Some(Model::Mgb),
            "cgb" | "gbc" | "color" | "colour" => Some(Model::Cgb),
            "cgb0" => Some(Model::Cgb0),
            // AGS is the backlit Game Boy Advance SP; Mooneye verifies the same
            // boot state on both, so they are one machine here.
            "agb" | "ags" | "gba" => Some(Model::Agb),
            "sgb" | "super" => Some(Model::Sgb),
            "sgb2" | "super2" => Some(Model::Sgb2),
            _ => None,
        }
    }

    /// The names [`Model::parse`] accepts, for the warning a typo produces and
    /// for any frontend that wants to list them.
    pub const NAMES: &'static str = "auto/dmg/dmg0/mgb/cgb/cgb0/agb/sgb/sgb2";

    /// The canonical name of this console — the one [`Model::parse`] round
    /// trips. Test harnesses label their rows with it, so it lives here rather
    /// than in each of them, where a new console would silently be missed.
    pub fn name(self) -> &'static str {
        match self {
            Model::Auto => "auto",
            Model::Dmg => "dmg",
            Model::Dmg0 => "dmg0",
            Model::Mgb => "mgb",
            Model::Cgb => "cgb",
            Model::Cgb0 => "cgb0",
            Model::Agb => "agb",
            Model::Sgb => "sgb",
            Model::Sgb2 => "sgb2",
        }
    }

    /// Read `$GB_MODEL`. An unset or unparseable value yields `None` (the
    /// caller's own choice stands) — a typo must never silently switch console.
    pub fn from_env() -> Option<Model> {
        std::env::var(MODEL_ENV).ok().and_then(|v| {
            let m = Model::parse(&v);
            if m.is_none() {
                eprintln!(
                    "warning: {MODEL_ENV}={v:?} is not one of {} — ignoring",
                    Model::NAMES
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
        self.console(cgb_flag).mode(cgb_flag)
    }

    /// Which console this actually is, with [`Model::Auto`] resolved away.
    ///
    /// Boot state is a property of the *console revision*, not of the resolved
    /// [`GbMode`] — a DMG0 and a DMG are both `GbMode::Classic` and hand the
    /// cartridge different registers — so construction keeps this value and
    /// [`GbMode`] both.
    pub fn console(self, cgb_flag: u8) -> Model {
        self.console_with(cgb_flag, Model::from_env())
    }

    /// The whole of [`console`](Self::console) with the environment passed in,
    /// so it can be tested without mutating process-global state.
    fn console_with(self, cgb_flag: u8, env: Option<Model>) -> Model {
        match self {
            Model::Auto => {
                env.filter(|m| *m != Model::Auto)
                    .unwrap_or(if cgb_flag & 0x80 != 0 {
                        Model::Cgb
                    } else {
                        Model::Dmg
                    })
            }
            explicit => explicit,
        }
    }

    /// The hardware mode a *concrete* console presents to a cartridge whose
    /// header byte `0x0143` is `cgb_flag`. Panics-free on [`Model::Auto`], which
    /// is treated as a Color console for backwards compatibility; callers should
    /// resolve with [`Model::console`] first.
    pub fn mode(self, cgb_flag: u8) -> GbMode {
        match self {
            Model::Dmg | Model::Dmg0 | Model::Mgb => GbMode::Classic,
            Model::Sgb => GbMode::Super,
            Model::Sgb2 => GbMode::Super2,
            // A Color console running a cartridge that does not advertise CGB
            // support runs in compatibility mode. An AGB is Color hardware, so
            // it takes the same branch.
            Model::Cgb | Model::Cgb0 | Model::Agb | Model::Auto => {
                if cgb_flag & 0x80 != 0 {
                    GbMode::Color
                } else {
                    GbMode::ColorAsClassic
                }
            }
        }
    }

    /// The whole of [`resolve`](Self::resolve) with the environment passed in.
    #[cfg(test)]
    fn resolve_with(self, cgb_flag: u8, env: Option<Model>) -> GbMode {
        self.console_with(cgb_flag, env).mode(cgb_flag)
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

    /// Every console must survive `name` → `parse`, and every name `NAMES`
    /// advertises must be one a user can actually type — a console reachable
    /// only from Rust is a console nobody can select.
    #[test]
    fn every_console_has_a_name_that_round_trips() {
        for m in [
            Model::Auto,
            Model::Dmg,
            Model::Dmg0,
            Model::Mgb,
            Model::Cgb,
            Model::Cgb0,
            Model::Agb,
            Model::Sgb,
            Model::Sgb2,
        ] {
            assert_eq!(Model::parse(m.name()), Some(m), "{m:?}");
            assert!(
                Model::NAMES.split('/').any(|n| n == m.name()),
                "{m:?} is missing from Model::NAMES"
            );
        }
        for n in Model::NAMES.split('/') {
            assert!(Model::parse(n).is_some(), "{n} is advertised but unparsed");
        }
    }

    /// The revisions are boot state only — after `PC=$0100` they present the
    /// same machine as the console they are a revision of, which is why they
    /// share a [`GbMode`] and are not serialised into a save state.
    #[test]
    fn a_revision_presents_the_same_hardware_as_its_console() {
        for m in [Model::Dmg, Model::Dmg0, Model::Mgb] {
            assert_eq!(m.mode(DMG_ONLY), GbMode::Classic, "{m:?}");
        }
        for m in [Model::Cgb, Model::Cgb0, Model::Agb] {
            assert_eq!(m.mode(CGB_ENHANCED), GbMode::Color, "{m:?}");
            assert_eq!(m.mode(DMG_ONLY), GbMode::ColorAsClassic, "{m:?}");
        }
    }

    /// `Auto` picks only the two consoles a person is likely to own. A
    /// revision — a pre-release DMG, a Game Boy Pocket, an AGB — is something
    /// you ask for by name; inferring one would silently change every game's
    /// boot registers.
    #[test]
    fn auto_never_infers_a_revision() {
        for flag in [DMG_ONLY, CGB_ENHANCED, CGB_ONLY] {
            let picked = Model::Auto.console_with(flag, None);
            assert!(
                matches!(picked, Model::Dmg | Model::Cgb),
                "auto picked {picked:?} for flag {flag:#04x}"
            );
        }
        // But naming one explicitly, or through the environment, is honoured.
        assert_eq!(Model::Agb.console_with(DMG_ONLY, None), Model::Agb);
        assert_eq!(
            Model::Auto.console_with(DMG_ONLY, Some(Model::Mgb)),
            Model::Mgb
        );
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

/// Which CGB CPU stepping's **PPU** to model.
///
/// The Color console shipped in revisions and its silicon changed between
/// them. `sound::CgbRevision` already carries the *APU*'s split, and this is a
/// separate type on purpose: **the two subsystems' revision boundaries do not
/// coincide.** The APU's quirks divide 0/B from C-and-later; the PPU's divide
/// C-and-earlier from D-and-later. One enum spanning both would have to name
/// four steppings to express two facts, and `sound` is not even compiled on
/// `wasm32` while the PPU always is.
///
/// Two independent emulators state the same boundary, and it is the only PPU
/// difference either of them models:
///
/// * SameBoy, `Core/display.c` (`GB_FETCHER_GET_TILE_T1`):
///   `if (gb->model > GB_MODEL_CGB_C) { /* This value is cached on the CGB-D
///   and newer, so it cannot be used to mix tiles together */ gb->fetcher_y =
///   y; }`, and the bitplane fetch reads
///   `gb->model > GB_MODEL_CGB_C ? gb->fetcher_y : fetcher_y(gb)`.
/// * docboy, `src/docboy/docboy/ppu/ppu.cpp` (`bg_prefetcher_get_tile_0`):
///   "Differently from DMG, on CGB there's no way to create a bitplane desync
///   effect by changing SCY between the tile data low/high fetches … it's
///   computed exactly here. [mealybug/m3_scy_change]" — docboy models D+ only.
///
/// Both are read as hardware references and reimplemented here per
/// `CONTRIBUTING.md`; no code was taken. The second difference — a palette
/// write landing one dot later on a CGB C — is **measured**, not borrowed: the
/// Mealybug suite's own `CPU CGB C` and `CPU CGB D` reference photographs
/// differ by exactly one dot of palette-change position on every affected row
/// (`docs/mealybug.md` §10).
///
/// This is machine *configuration*, not cartridge state, so it is not
/// serialised — the same call `Sound::cgb_revision` makes.
#[derive(PartialEq, Eq, PartialOrd, Ord, Debug, Copy, Clone, Default)]
pub enum PpuRevision {
    /// CPU CGB 0, B and C. `SCY` is re-read at every bitplane fetch (so a
    /// mid-fetch write desyncs the two bitplanes, as on a DMG), and a palette
    /// write is seen one dot later than on a CGB D.
    CgbC,
    /// CPU CGB D, and **only** D. It latches `SCY + LY` at the tile-number
    /// fetch like every later stepping, and it is alone in how it answers a
    /// `TILE_SEL` reset landing on a bitplane read — see
    /// [`PpuRevision::tile_sel_glitch_reads_tile_index`].
    CgbD,
    /// CPU CGB E, the stepping this emulator targets elsewhere (see
    /// `sound::CgbRevision`). The default. Identical to `CgbD` except for the
    /// `TILE_SEL` glitch, where it behaves as every revision but D does.
    #[default]
    CgbE,
}

impl PpuRevision {
    /// The spellings `$GB_CGB_REVISION` and the suite runners use, matching
    /// `sound::CgbRevision::from_tag` where they overlap. A multi-letter tag
    /// means "these revisions agree here"; `cgb0BC` is C-and-earlier and
    /// `cgbDE` is D-and-later, which is exactly this type's boundary.
    pub fn from_tag(tag: &str) -> Option<PpuRevision> {
        match tag.to_ascii_lowercase().as_str() {
            "cgb0" | "0" | "cgb0b" | "cgbb" | "b" | "cgbc" | "c" | "cgb0bc" => {
                Some(PpuRevision::CgbC)
            }
            "cgbd" | "d" => Some(PpuRevision::CgbD),
            "cgbde" | "cgbe" | "e" => Some(PpuRevision::CgbE),
            _ => None,
        }
    }

    /// Read `$GB_CGB_REVISION` — the same knob the APU reads, so one spelling
    /// configures both subsystems and every tag one accepts the other accepts
    /// too. An unset or unparseable value is the default; the warning for a
    /// mis-spelled tag is `sound::CgbRevision::from_env`'s, which reads the
    /// same variable — printing a second one here would double up.
    pub fn from_env() -> PpuRevision {
        match std::env::var("GB_CGB_REVISION") {
            Ok(s) if !s.is_empty() => PpuRevision::from_tag(&s).unwrap_or_default(),
            _ => PpuRevision::default(),
        }
    }

    /// `true` where the fetcher latches `SCY + LY` at the tile-number fetch and
    /// reuses it for both bitplanes, so a mid-fetch `SCY` write cannot mix two
    /// rows into one drawn tile.
    pub fn caches_fetcher_y(self) -> bool {
        self != PpuRevision::CgbC
    }

    /// A palette write is seen one dot later on a CPU CGB D and later than on a
    /// CPU CGB C — measured against the suite's own two Color reference sets,
    /// which differ by exactly that one dot on every row a mid-line palette
    /// write reaches (`docs/mealybug.md` §10).
    pub fn late_palette_commit(self) -> bool {
        self != PpuRevision::CgbC
    }

    /// **Resetting `LCDC.4` on the same dot as a bitplane read makes the PPU
    /// read the tile *index* as that bitplane's data — on every Color stepping
    /// except CPU CGB D**, which instead reads from the bitplane-0 address.
    /// Matt Currie, *The Comprehensive Game Boy PPU Documentation*, `TILE_SEL`
    /// (bit 4) — he wrote both the rule and the reference photographs this
    /// project scores against. SameBoy models the same split
    /// (`Core/display.c`, `data_for_tile_sel_glitch`, citing the same page).
    ///
    /// This is the one place `CgbD` and `CgbE` differ, and it is why the two
    /// are separate values at all.
    pub fn tile_sel_glitch_reads_tile_index(self) -> bool {
        self != PpuRevision::CgbD
    }
}

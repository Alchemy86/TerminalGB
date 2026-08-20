//! **Speed flags** — the accuracy/speed trade as a documented set of switches
//! rather than one opaque mode.
//!
//! Every shortcut that changes anything a cartridge can observe is its own
//! named flag, described precisely enough that a reader can decide whether it
//! matters to *their* game. [`SpeedFlags::PRESET`] — "speed mode" — is a
//! preset that turns a subset of them on, and it is nothing more than that:
//! there is no separate fast code path to fall out of step with the accurate
//! one, and the preset's contents are auditable here in one place.
//!
//! **Explicit beats preset, and last wins.** `speed,-instruction-bus` is the
//! preset with one shortcut forced off; `no-oam-bug` on its own is that one
//! shortcut with no preset at all. Both directions work because a game we have
//! never run may need a behaviour our workload does not.
//!
//! **What is in the preset is decided by evidence, not by argument.** A flag
//! earns its place by the certified Pewter-chain links still certifying under
//! it — see `docs/modes.md` for the table and the measurements. A flag that
//! reaches wider than our own workload ships as a flag and stays *out* of the
//! preset, because we cannot vouch for what we do not run.
//!
//! Every flag is refused under [`RenderMode::Accurate`]: identical mode's whole
//! claim is that nothing is skipped.
//!
//! [`RenderMode::Accurate`]: crate::ppu::RenderMode::Accurate

/// One named shortcut each. `false` everywhere is the accurate machine.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpeedFlags {
    /// Advance the peripherals once per CPU **instruction** instead of once per
    /// bus **access**.
    ///
    /// *Affects:* which sub-instruction M-cycle a memory access lands on. The
    /// totals are unchanged — `DIV`, `TIMA`, the serial shift clock, the APU
    /// frame sequencer and OAM DMA all still advance exactly as many cycles per
    /// instruction — so nothing runs at the wrong rate. What breaks is any
    /// program that measures *where inside an instruction* an access happened:
    /// Blargg `mem_timing`, `mem_timing-2`, and `dmg_sound` 09/10/12, which
    /// time the DMG wave-RAM window to two T-cycles.
    ///
    /// *Does not affect:* the picture, the audio, interrupt dispatch (the PPU
    /// still steps through it, so a dispatch can still be redirected), OAM/VRAM
    /// access blocking, the OAM-corruption bug, or OAM DMA bus conflicts.
    pub instruction_bus: bool,

    /// Skip the DMG OAM-corruption bug (`Gpu::oam_bug` and the IDU hook).
    ///
    /// *Affects:* `blargg/oam_bug`, and any DMG-class program that moves a
    /// 16-bit register through `$FE00-$FEFF` while the PPU is scanning OAM —
    /// which corrupts object memory on real DMG silicon and does not here with
    /// this on. Colour consoles have no such defect, so on them the flag is a
    /// no-op by construction.
    ///
    /// *Does not affect:* anything on a CGB or AGB; anything that does not
    /// touch the OAM page with the increment/decrement unit.
    pub no_oam_bug: bool,

    /// Freeze the APU's state machines as well as its output.
    ///
    /// *Affects:* everything a program can read back from the sound chip —
    /// `NR52`'s channel-enable bits, the length counters, the sweep and
    /// envelope units, `PCM12`/`PCM34`. A game that waits on any of them does
    /// not merely go quiet, it **stops**.
    ///
    /// *Does not affect:* anything in a program that never reads the sound
    /// registers back. **Not in the preset**: Pokémon's Pokémon-Center healing
    /// sequence is exactly such a wait, and this is the flag that proves the
    /// preset is decided by evidence — see `docs/modes.md`.
    ///
    /// This is *not* [`crate::gameboy::Gameboy::set_render_audio`], which skips
    /// only the band-limited resample and leaves every one of those registers
    /// exact. Prefer that one; it costs nothing in accuracy.
    pub no_apu_state: bool,
}

impl SpeedFlags {
    /// The accurate machine: every shortcut off. Also [`Default`].
    pub const NONE: Self = Self {
        instruction_bus: false,
        no_oam_bug: false,
        no_apu_state: false,
    };

    /// **Speed mode.** What `speed` turns on, and the whole of it.
    ///
    /// Membership is earned by evidence: every flag here has had the certified
    /// Pewter-chain links re-run under it with the same seeds and still
    /// certified. `no_apu_state` is deliberately absent — it fails that test.
    pub const PRESET: Self = Self {
        instruction_bus: true,
        // Measured at **0.0 %** on Pokémon Blue: the OAM-bug hook only fires
        // when a 16-bit register points into `$FE00-$FEFF`, which this
        // workload never does, so the preset would be trading real accuracy
        // for nothing. Safe for our workload and still excluded — the bar for
        // the preset is safe *and* worth having.
        no_oam_bug: false,
        // Fails certification: see the module docs and `docs/modes.md`.
        no_apu_state: false,
    };

    /// Whether any shortcut is on.
    pub fn any(&self) -> bool {
        *self != Self::NONE
    }

    /// Every flag's name, in table order.
    pub const NAMES: &'static [&'static str] =
        &["instruction-bus", "no-oam-bug", "no-apu-state"];

    fn set_by_name(&mut self, name: &str, on: bool) -> bool {
        match name {
            "instruction-bus" => self.instruction_bus = on,
            "no-oam-bug" => self.no_oam_bug = on,
            "no-apu-state" => self.no_apu_state = on,
            _ => return false,
        }
        true
    }

    /// Parse a comma-separated list. `speed` (or `preset`, or `1`) applies
    /// [`Self::PRESET`]; a bare or `+`-prefixed name turns one flag on; a
    /// `-`-prefixed name turns one off. **Last wins**, so
    /// `speed,-instruction-bus` is the preset without that shortcut.
    ///
    /// Returns the flags and every token that was not understood, so a caller
    /// can warn rather than silently running a machine nobody asked for — the
    /// rule `config.toml`'s `protocol` key already follows.
    pub fn parse(spec: &str) -> (Self, Vec<String>) {
        let mut flags = Self::NONE;
        let mut bad = Vec::new();
        for raw in spec.split(',') {
            let token = raw.trim();
            if token.is_empty() {
                continue;
            }
            let (on, name) = match token.strip_prefix('-') {
                Some(rest) => (false, rest),
                None => (true, token.strip_prefix('+').unwrap_or(token)),
            };
            match name {
                "speed" | "preset" | "1" | "all" if on => flags = Self::PRESET,
                "speed" | "preset" | "1" | "all" => flags = Self::NONE,
                _ if flags.set_by_name(name, on) => {}
                _ => bad.push(token.to_string()),
            }
        }
        (flags, bad)
    }

    /// Read `$GB_SPEED`. Absent or empty is [`Self::NONE`] — every shortcut is
    /// opt-in and nothing turns one on by accident.
    pub fn from_env() -> (Self, Vec<String>) {
        match std::env::var("GB_SPEED") {
            Ok(spec) if !spec.trim().is_empty() => Self::parse(&spec),
            _ => (Self::NONE, Vec::new()),
        }
    }

    /// The flags that are on, as the spec string that reproduces them.
    pub fn spec(&self) -> String {
        let on: Vec<&str> = Self::NAMES
            .iter()
            .copied()
            .filter(|n| match *n {
                "instruction-bus" => self.instruction_bus,
                "no-oam-bug" => self.no_oam_bug,
                "no-apu-state" => self.no_apu_state,
                _ => false,
            })
            .collect();
        if on.is_empty() {
            "none".to_string()
        } else {
            on.join(",")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_on_by_default() {
        assert_eq!(SpeedFlags::default(), SpeedFlags::NONE);
        assert!(!SpeedFlags::NONE.any());
        assert_eq!(SpeedFlags::NONE.spec(), "none");
    }

    #[test]
    fn the_preset_is_what_it_says_it_is() {
        // If this changes, `docs/modes.md`'s table and the certification
        // evidence behind it change with it — deliberately a test, not a
        // comment, so the two cannot drift.
        assert_eq!(SpeedFlags::parse("speed").0, SpeedFlags::PRESET);
        assert_eq!(
            SpeedFlags::PRESET.spec(),
            "instruction-bus",
            "the preset must not quietly grow a flag"
        );
        // Through `parse` rather than the constant, so these read as tests
        // rather than as constant expressions the compiler folds away.
        let preset = SpeedFlags::parse("speed").0;
        assert!(
            !preset.no_apu_state,
            "no-apu-state fails certification and must stay out of the preset"
        );
        assert!(
            !preset.no_oam_bug,
            "no-oam-bug buys 0.0% measured — safe is not enough, it must also \
             be worth having"
        );
    }

    #[test]
    fn an_explicit_flag_overrides_the_preset_in_both_directions() {
        let (off, bad) = SpeedFlags::parse("speed,-instruction-bus");
        assert!(bad.is_empty());
        assert!(!off.instruction_bus);

        let (extra, _) = SpeedFlags::parse("speed,no-oam-bug");
        assert!(
            extra.instruction_bus && extra.no_oam_bug,
            "the preset can be added to"
        );

        let (on, _) = SpeedFlags::parse("no-apu-state");
        assert!(on.no_apu_state);
        assert!(!on.instruction_bus, "one flag is not the preset");
    }

    #[test]
    fn last_wins() {
        assert!(
            !SpeedFlags::parse("instruction-bus,-instruction-bus")
                .0
                .instruction_bus
        );
        assert!(
            SpeedFlags::parse("-instruction-bus,instruction-bus")
                .0
                .instruction_bus
        );
    }

    #[test]
    fn an_unknown_flag_is_reported_and_not_guessed_at() {
        let (flags, bad) = SpeedFlags::parse("speed,nonsense");
        assert_eq!(bad, vec!["nonsense".to_string()]);
        assert_eq!(flags, SpeedFlags::PRESET, "the rest still applies");
    }

    #[test]
    fn a_spec_round_trips() {
        for spec in ["speed", "instruction-bus", "no-oam-bug,no-apu-state"] {
            let (flags, _) = SpeedFlags::parse(spec);
            assert_eq!(SpeedFlags::parse(&flags.spec()).0, flags, "{spec}");
        }
    }
}

//! Shiny hunting for Gen 1 — the plugin half.
//!
//! # What "shiny hunting in Gen 1" is
//!
//! **Generation I has no shiny Pokémon.** Shininess arrives in Generation II,
//! which derives it from the DVs a Pokémon already carries. Those DVs cross the
//! Time Capsule unchanged, so a Red/Blue Pokémon with the right spread *becomes*
//! shiny the moment it is traded forward. Hunting one in Gen 1 therefore means
//! hunting a **DV pattern**, and never means looking at a sprite:
//!
//! > defence, speed and special all exactly **10**, and the attack DV one of
//! > 2, 3, 6, 7, 10, 11, 14, 15 (bit 1 set) — 8 spreads in 65,536, **1 in 8192**.
//!
//! The rule lives in [`crate::trainer::derived::shiny_if_traded`] and is used
//! from there; see [`target`] for why there is exactly one copy of it.
//!
//! That is what makes this tractable, and it is the whole difference from the
//! hardware rig this imitates — a hunter with four Nintendo DS consoles wired to
//! one controller through an input splitter, watching four screens for a colour.
//! We do not watch anything: the instant the game rolls the encounter we read
//! `wEnemyMonDVs` out of memory and know.
//!
//! # What this plugin does, and what it deliberately does not
//!
//! It **observes**. It declares [`Capability::OBSERVE`] and
//! [`Capability::READ_MEMORY`] and nothing else, so [`Machine::poke`] refuses it
//! at the host — a hunter that could write memory could fabricate its own hit,
//! which would make every result worthless. Once per frame it samples a handful
//! of bytes, and when the game finishes generating an encounter it latches a
//! [`Sighting`]: species, level, the DV spread, and whether it matches the
//! [`Target`].
//!
//! It does **not** drive the machine. Pressing buttons, stepping frames, loading
//! save states and running many instances at once are all things the plugin API
//! has no verb for, so they live beside the plugin in [`hunt`], written against
//! the same public [`crate::gameboy::Gameboy`] API any third party has. That
//! split is deliberate and is reported as a finding, not worked around: see
//! `docs/shiny-hunt.md` §"What the plugin API could not do".
//!
//! # Two hunt shapes
//!
//! * [`Watch::Encounter`] — a battle. Covers a wild grass encounter and a static
//!   legendary alike: both end in `LoadEnemyMonData` filling `wEnemyMon`.
//! * [`Watch::PartySlot`] — a gift or an in-game trade, which lands directly in
//!   the party without a battle.

use super::{Capability, Event, Machine, Plugin};
use crate::trainer::derived::{hidden_power, shiny_if_traded, HiddenPower};
use crate::trainer::gen1::{addr, battle, party, Dvs, PARTY_MON_SIZE};
use crate::trainer::Game;

/// The multi-instance driver. Native only: it runs one OS thread per emulator,
/// which `wasm32` has no equivalent of. The plugin itself builds everywhere.
#[cfg(not(target_arch = "wasm32"))]
pub mod hunt;
pub mod target;

#[cfg(not(target_arch = "wasm32"))]
pub use hunt::{Hit, Hunt, HuntConfig, Outcome, Shape, Snapshot};
pub use target::{dv_total, Target};

/// Where the plugin expects the newly generated Pokémon to appear.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Watch {
    /// `wEnemyMon`, filled by `LoadEnemyMonData` — a wild encounter or a static
    /// battle. The default.
    #[default]
    Encounter,
    /// A party slot (0-based), for a gift Pokémon or an in-game trade.
    PartySlot(u8),
}

/// One generated Pokémon, as read out of the running game.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Sighting {
    /// Internal index number (`MonsterNames` order, *not* the Pokédex number).
    pub species: u8,
    pub level: u8,
    pub dvs: Dvs,
    /// The frame on which the plugin latched it.
    pub frame: u64,
}

impl Sighting {
    /// Whether this Pokémon would be shiny once traded to Gen 2. Gen 1 itself
    /// has no such concept — see the module docs.
    pub fn shiny_if_traded(&self) -> bool {
        shiny_if_traded(self.dvs)
    }

    /// The five DVs summed, out of 75.
    pub fn dv_total(&self) -> u8 {
        dv_total(self.dvs)
    }

    pub fn hidden_power(&self) -> HiddenPower {
        hidden_power(self.dvs)
    }

    /// `atk/def/spd/spc/hp`, the order a DV calculator prints.
    pub fn dv_line(&self) -> String {
        format!(
            "atk {:2}  def {:2}  spd {:2}  spc {:2}  hp {:2}  (total {}/75)",
            self.dvs.attack,
            self.dvs.defense,
            self.dvs.speed,
            self.dvs.special,
            self.dvs.hp(),
            self.dv_total()
        )
    }
}

/// The plugin. One per emulator instance.
pub struct ShinyHunt {
    target: Target,
    watch: Watch,
    game: Option<Game>,
    detected_title: String,
    /// Latched once the current encounter's data is complete. Cleared by
    /// [`ShinyHunt::arm`] and by [`Plugin::reset`] (which the host calls on
    /// every save-state load, i.e. on every attempt).
    sighting: Option<Sighting>,
    /// Set when the latched sighting satisfies the target.
    matched: bool,
    /// Rising-edge detector for `wIsInBattle` / party growth.
    was_armed_condition: bool,
    /// Total sightings latched over this plugin's life.
    seen: u64,
    frame: u64,
}

/// Frames to allow between the battle flag going up and `wEnemyMon` being
/// complete. `InitBattle` runs the encounter animation before
/// `LoadEnemyMonData`; measured worst case on Route 1 is well under a second.
const SETTLE_LIMIT: u64 = 600;

impl ShinyHunt {
    pub fn new(target: Target) -> ShinyHunt {
        ShinyHunt {
            target,
            watch: Watch::Encounter,
            game: None,
            detected_title: String::new(),
            sighting: None,
            matched: false,
            was_armed_condition: false,
            seen: 0,
            frame: 0,
        }
    }

    pub fn watching(mut self, watch: Watch) -> ShinyHunt {
        self.watch = watch;
        self
    }

    /// Whether the cartridge is one whose RAM layout is verified. `None` means
    /// the plugin will never report anything — deliberately, rather than
    /// decoding another game's memory as if it were Red/Blue.
    pub fn game(&self) -> Option<Game> {
        self.game
    }

    pub fn detected_title(&self) -> &str {
        &self.detected_title
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    /// Clear the latch, ready for the next attempt. Idempotent.
    pub fn arm(&mut self) {
        self.sighting = None;
        self.matched = false;
        self.was_armed_condition = false;
    }

    /// The encounter latched since the last [`ShinyHunt::arm`], if the game has
    /// finished generating one.
    pub fn sighting(&self) -> Option<Sighting> {
        self.sighting
    }

    /// Whether the latched sighting is a hit.
    pub fn is_hit(&self) -> bool {
        self.matched
    }

    /// Sightings latched over this plugin's whole life, across arms.
    pub fn seen(&self) -> u64 {
        self.seen
    }

    fn sample(&mut self, machine: &mut Machine) {
        if self.sighting.is_some() {
            return;
        }
        match self.watch {
            Watch::Encounter => self.sample_encounter(machine),
            Watch::PartySlot(slot) => self.sample_party(machine, slot),
        }
    }

    /// `wIsInBattle` (0 none / 1 wild / 2 trainer) gates the read, and the enemy
    /// structure is only trusted once `LoadEnemyMonData` has plainly finished
    /// with it.
    ///
    /// The completeness test matters more than it looks: `wIsInBattle` is set
    /// before the enemy structure is filled, and the structure still holds the
    /// *previous* battle's bytes until it is. Reading one frame too early
    /// reports a stale spread that looks entirely plausible — the worst possible
    /// failure for a tool whose whole output is "this one is the one". So the
    /// latch waits for a non-zero species and level **and** for the species to
    /// agree with `wCurOpponent`, which the overworld sets before the battle
    /// starts and is therefore an independent witness of which Pokémon this is.
    fn sample_encounter(&mut self, machine: &mut Machine) {
        let Some(kind) = machine.peek(addr::IS_IN_BATTLE) else {
            return;
        };
        if kind == 0 {
            self.was_armed_condition = false;
            return;
        }
        if !self.was_armed_condition {
            self.was_armed_condition = true;
        }
        // A trainer battle generates its roster from fixed data, not from
        // `Random`, so it is not a hunt result. Skip it rather than report it.
        if kind != 1 {
            return;
        }
        let base = addr::ENEMY_MON;
        let species = machine.peek(base + battle::SPECIES).unwrap_or(0);
        let level = machine.peek(base + battle::LEVEL).unwrap_or(0);
        let opponent = machine.peek(addr::CUR_OPPONENT).unwrap_or(0);
        if species == 0 || level == 0 || species != opponent {
            return;
        }
        let hi = machine.peek(base + battle::DVS).unwrap_or(0);
        let lo = machine.peek(base + battle::DVS + 1).unwrap_or(0);
        self.latch(Sighting {
            species,
            level,
            dvs: Dvs::from_bytes(hi, lo),
            frame: self.frame,
        });
    }

    /// A gift or in-game trade arrives in the party with no battle: watch the
    /// slot fill in. `wPartyCount` growing past the slot is the edge.
    fn sample_party(&mut self, machine: &mut Machine, slot: u8) {
        let Some(count) = machine.peek(addr::PARTY_COUNT) else {
            return;
        };
        if count <= slot || count > 6 {
            self.was_armed_condition = false;
            return;
        }
        self.was_armed_condition = true;
        let base = addr::PARTY_MONS + u16::from(slot) * PARTY_MON_SIZE;
        let species = machine.peek(base + party::SPECIES).unwrap_or(0);
        let level = machine.peek(base + party::LEVEL).unwrap_or(0);
        if species == 0 || level == 0 {
            return;
        }
        let hi = machine.peek(base + party::DVS).unwrap_or(0);
        let lo = machine.peek(base + party::DVS + 1).unwrap_or(0);
        self.latch(Sighting {
            species,
            level,
            dvs: Dvs::from_bytes(hi, lo),
            frame: self.frame,
        });
    }

    fn latch(&mut self, s: Sighting) {
        self.matched = self.target.matches(s.dvs, s.species);
        self.sighting = Some(s);
        self.seen += 1;
    }
}

impl Plugin for ShinyHunt {
    fn name(&self) -> &str {
        "gen1-shinyhunt"
    }

    fn capabilities(&self) -> Capability {
        // Read-only, on purpose. A hunter that could write memory could
        // manufacture its own hit.
        Capability::OBSERVE | Capability::READ_MEMORY
    }

    fn status(&self) -> String {
        if self.game.is_none() {
            return format!("not Red/Blue ({})", self.detected_title.trim());
        }
        match self.sighting {
            Some(s) if self.matched => format!("HIT #{} dv {}", s.species, s.dv_total()),
            Some(s) => format!("seen #{} lv{}", s.species, s.level),
            None => format!("hunting {}", self.target.label()),
        }
    }

    fn on_attach(&mut self, machine: &mut Machine) {
        self.detected_title = machine.rom_title();
        self.game = Game::detect(&self.detected_title, machine.global_checksum());
    }

    fn on_event(&mut self, event: &Event, machine: &mut Machine) {
        let Event::Frame { number } = *event else {
            return;
        };
        self.frame = number;
        if self.game.is_none() {
            return;
        }
        // Once a sighting is latched there is nothing left to do until the
        // driver arms the next attempt, so the steady-state cost of a hunt in
        // progress is one branch per frame.
        if self.sighting.is_some() {
            return;
        }
        self.sample(machine);
    }

    fn reset(&mut self) {
        // The host calls this on every save-state load, which is once per hunt
        // attempt — exactly the moment the latch must be cleared.
        self.arm();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

/// The plugin never reports past this many frames of a battle without the enemy
/// structure completing; the driver uses it as its own timeout so a wedged
/// attempt is abandoned rather than hanging the hunt.
pub const SETTLE_FRAMES: u64 = SETTLE_LIMIT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hunter_cannot_write_memory() {
        let p = ShinyHunt::new(Target::shiny());
        assert!(p.capabilities().contains(Capability::READ_MEMORY));
        assert!(!p.capabilities().contains(Capability::WRITE_MEMORY));
        assert!(!p.capabilities().contains(Capability::SERIAL_LINK));
    }

    #[test]
    fn a_sighting_reports_the_derived_values_the_trainer_panel_does() {
        // The canonical shiny pair, ATKDEFDV_SHINY $EA / SPDSPCDV_SHINY $AA.
        let s = Sighting {
            species: 0x54,
            level: 5,
            dvs: Dvs::from_bytes(0xEA, 0xAA),
            frame: 0,
        };
        assert!(s.shiny_if_traded());
        assert_eq!(s.dvs.attack, 14);
        assert_eq!(s.dvs.defense, 10);
        assert_eq!(s.dvs.speed, 10);
        assert_eq!(s.dvs.special, 10);
        assert!(s.dv_line().contains("atk 14"));
    }

    #[test]
    fn arming_clears_the_latch_and_reset_arms() {
        let mut p = ShinyHunt::new(Target::parse("spc=10", None).unwrap());
        p.latch(Sighting {
            species: 1,
            level: 3,
            dvs: Dvs::from_bytes(0x00, 0x0A),
            frame: 7,
        });
        assert!(p.is_hit());
        assert_eq!(p.seen(), 1);
        p.reset();
        assert!(p.sighting().is_none());
        assert!(!p.is_hit());
        // `seen` is a lifetime counter and must survive arming, or the hunt's
        // own encounter count would reset every attempt.
        assert_eq!(p.seen(), 1);
    }
}

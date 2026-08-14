//! # Shiny hunting for Gen 1 — many emulators, one DV pattern
//!
//! ## What "shiny hunting in Gen 1" is, precisely
//!
//! **Generation I has no shiny Pokémon.** Shininess arrives in Generation II,
//! which derives it from the DVs a Pokémon already carries. Those DVs cross the
//! Time Capsule unchanged, so a Red/Blue Pokémon with the right spread *becomes*
//! shiny the moment it is traded forward. Hunting one in Gen 1 therefore means
//! hunting a **DV pattern**, and it never means looking at a sprite:
//!
//! > defence, speed and special all exactly **10**, and the attack DV one of
//! > 2, 3, 6, 7, 10, 11, 14, 15 (bit 1 set) — 8 spreads in 65,536, **1 in 8192**.
//!
//! The rule lives in [`terminalgb_plugin_trainer::derived::shiny_if_traded`],
//! cited to `CheckShininess` (`pokecrystal/engine/gfx/color.asm`), and is used
//! from there. There is one copy of it in the tree; see [`target`].
//!
//! That single fact is what makes this tractable, and it is the whole
//! difference from the hardware rig this imitates — a hunter with four Nintendo
//! DS consoles wired to one controller through an input splitter, watching four
//! screens for a colour. **We watch nothing.** The instant the game finishes
//! rolling the encounter we read `wEnemyMonDVs` out of memory and know.
//!
//! ## The two halves, and why they are two
//!
//! * **[`ShinyHunt`], the plugin.** It *observes*: [`Capability::OBSERVE`] and
//!   [`Capability::READ_MEMORY`], and nothing else — so the host refuses it
//!   [`Machine::poke`], and a hunter provably cannot fabricate its own hit.
//!   Once per frame it samples a handful of bytes and, when the game has
//!   finished generating an encounter, latches a [`Sighting`].
//! * **[`hunt`], the driver.** Pressing buttons on a schedule, stepping frames,
//!   loading a save state and running N machines at once are things the plugin
//!   API has no verb for — a plugin is a passenger on a machine somebody else is
//!   driving. So the driver sits beside the plugin, over the small
//!   [`hunt::HuntMachine`] seam, and the emulator implements that seam for its
//!   `Gameboy`. See `docs/shiny-hunt.md` §"What the plugin API could not do":
//!   this is reported as a finding for the API, not worked around behind its
//!   back.
//!
//! ## Two hunt shapes
//!
//! * [`Watch::Encounter`] — a battle. Covers a wild grass encounter and a
//!   static legendary alike: both end in `LoadEnemyMonData` filling
//!   `wEnemyMon`.
//! * [`Watch::PartySlot`] — a gift or an in-game trade, which lands directly in
//!   a party slot with no battle at all (`_AddPartyMon`'s `; Not wild.`
//!   branch rolls the DVs there).
//!
//! ## What a user must supply
//!
//! A **save state positioned for the hunt**: standing in grass on a map with an
//! encounter table for [`hunt::Shape::Grass`], or in front of the thing that
//! generates the Pokémon for [`hunt::Shape::Reset`]. Every attempt restarts
//! from those exact bytes, so where they were taken is the whole setup.

#![forbid(unsafe_code)]

pub mod hunt;
pub mod target;

use terminalgb_plugin_api::{Capability, Event, Machine, Plugin};
use terminalgb_plugin_trainer::derived::{hidden_power, shiny_if_traded, HiddenPower};
use terminalgb_plugin_trainer::gen1::{addr, battle, party, Dvs, PARTY_MON_SIZE};
use terminalgb_plugin_trainer::Game;

pub use target::{dv_total, Target};

/// The name this plugin is installed under (`plugins = gen1-shinyhunt`).
pub const NAME: &str = "gen1-shinyhunt";

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
    /// Internal index number (`MonsterNames` order, **not** the Pokédex
    /// number — `wEnemyMonSpecies` holds the internal index).
    pub species: u8,
    pub level: u8,
    pub dvs: Dvs,
    /// How many emulated frames into the current attempt the plugin latched
    /// it — counted from the last [`ShinyHunt::arm`], **not** from the start of
    /// the machine.
    ///
    /// It has to be relative, and the reason is worth keeping: the host's frame
    /// counter (`Event::Frame { number }`) is not machine state and
    /// `PluginHost::reset` does not rewind it, so an absolute number differs
    /// between an instance on its seventeenth attempt and a fresh machine
    /// replaying that same attempt. A [`Sighting`] must be a pure function of
    /// (base state, seed, attempt) or the driver's reproducibility check
    /// compares two identical Pokémon and calls them different — which is
    /// exactly what it did until this was fixed.
    pub frames_into_attempt: u64,
}

impl Sighting {
    /// Whether this Pokémon would be shiny once traded to Gen 2. Gen 1 itself
    /// has no such concept — see the crate docs.
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

/// Frames the driver allows between the battle flag going up and `wEnemyMon`
/// being complete.
///
/// `InitBattle` plays the encounter animation before `LoadEnemyMonData`, so the
/// gap is real and scene-dependent; measured worst case on Route 1 is well under
/// a second. The driver uses this as its own timeout so a wedged attempt is
/// abandoned and counted, never waited on for ever.
pub const SETTLE_FRAMES: u64 = 600;

/// Does this look like a Pokémon the game has finished generating, or like
/// whatever was in that RAM before?
///
/// The same shape as `terminalgb_plugin_trainer::encounter::looks_populated`, on
/// raw values rather than a Pokédex number — this plugin is deliberately not
/// given the cartridge, so it cannot resolve `PokedexOrder`. A full battle
/// structure is checked instead: HP is meaningless before `CalcStats` has run.
pub fn looks_populated(species: u8, level: u8, hp: u16, max_hp: u16) -> bool {
    species != 0 && (1..=100).contains(&level) && max_hp > 0 && hp <= max_hp
}

/// The plugin. One per emulator instance.
pub struct ShinyHunt {
    target: Target,
    watch: Watch,
    game: Option<Game>,
    detected_title: String,
    /// Latched once the current encounter's data is complete. Cleared by
    /// [`ShinyHunt::arm`] and by [`Plugin::reset`], which the host calls on
    /// every save-state load — i.e. once per hunt attempt.
    sighting: Option<Sighting>,
    /// Set when the latched sighting satisfies the target.
    matched: bool,
    /// The host frame counter as of the last [`ShinyHunt::arm`], so a sighting
    /// can report a frame number relative to its own attempt.
    frame_base: u64,
    /// Sightings latched over this plugin's whole life, across arms.
    seen: u64,
    frame: u64,
}

impl ShinyHunt {
    pub fn new(target: Target) -> ShinyHunt {
        ShinyHunt {
            target,
            watch: Watch::Encounter,
            game: None,
            detected_title: String::new(),
            sighting: None,
            matched: false,
            frame_base: 0,
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
        self.frame_base = self.frame;
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
        match self.watch {
            Watch::Encounter => self.sample_encounter(machine),
            Watch::PartySlot(slot) => self.sample_party(machine, slot),
        }
    }

    /// `wIsInBattle` gates the read, and the enemy structure is checked for
    /// self-consistency before it is believed.
    ///
    /// **There is no half-written window to guard against, and that is a fact
    /// worth writing down rather than a lucky guess.** `InitWildBattle`
    /// (`pokered/engine/battle/core.asm`) is straight-line code:
    ///
    /// ```text
    /// InitWildBattle:
    ///     ld a, 1
    ///     ld [wIsInBattle], a
    ///     call LoadEnemyMonData
    ///     call DoBattleTransitionAndInitBattleVariables
    /// ```
    ///
    /// so the flag and the DV roll land in the same frame, with the encounter
    /// animation only afterwards. A once-per-frame sampler therefore never sees
    /// the flag up and the structure stale. The consistency test below is a
    /// guard against a *different* thing: `wIsInBattle` taking a value the
    /// overworld wrote for some other reason, or a base save state whose
    /// `wEnemyMon` still holds a previous battle's bytes.
    ///
    /// **`wCurOpponent` is NOT usable as a second witness** — it reads back `0`
    /// once the battle has begun, measured on the cartridge (`--diagnose` in
    /// `examples/shiny_hunt.rs` prints it beside `wEnemyMonSpecies`). An earlier
    /// version of this function required the two to agree and consequently
    /// never latched anything at all: every attempt was reported as a stall.
    /// The lesson generalises — an "independent witness" has to be *observed*
    /// to still hold at the moment you read it.
    ///
    /// A **trainer** battle is skipped rather than reported: `LoadEnemyMonData`
    /// gives every trainer Pokémon the fixed pair `ATKDEFDV_TRAINER` /
    /// `SPDSPCDV_TRAINER` (`$98`/`$88`), so it is not a roll at all and could
    /// never be a hit.
    fn sample_encounter(&mut self, machine: &mut Machine) {
        let Some(kind) = machine.peek(addr::IS_IN_BATTLE) else {
            return;
        };
        // 1 is a wild battle. 0 is none, 2 is a trainer, and `$FF` means the
        // player blacked out (`home/overworld.asm` `.allPokemonFainted`) — the
        // sentinel a catch-all arm would swallow.
        if kind != 1 {
            return;
        }
        let base = addr::ENEMY_MON;
        let species = machine.peek(base + battle::SPECIES).unwrap_or(0);
        let level = machine.peek(base + battle::LEVEL).unwrap_or(0);
        let hp = machine.peek_be16(base + battle::HP).unwrap_or(0);
        let max_hp = machine.peek_be16(base + battle::MAX_HP).unwrap_or(0);
        if !looks_populated(species, level, hp, max_hp) {
            return;
        }
        let hi = machine.peek(base + battle::DVS).unwrap_or(0);
        let lo = machine.peek(base + battle::DVS + 1).unwrap_or(0);
        self.latch(Sighting {
            species,
            level,
            dvs: Dvs::from_bytes(hi, lo),
            frames_into_attempt: self.frame.saturating_sub(self.frame_base),
        });
    }

    /// A gift or in-game trade arrives in the party with no battle: watch the
    /// slot fill in. `wPartyCount` growing past the slot is the edge.
    fn sample_party(&mut self, machine: &mut Machine, slot: u8) {
        let Some(count) = machine.peek(addr::PARTY_COUNT) else {
            return;
        };
        if count <= slot || count > 6 {
            return;
        }
        let base = addr::PARTY_MONS + u16::from(slot) * PARTY_MON_SIZE;
        let species = machine.peek(base + party::SPECIES).unwrap_or(0);
        let level = machine.peek(base + party::LEVEL).unwrap_or(0);
        let hp = machine.peek_be16(base + party::HP).unwrap_or(0);
        let max_hp = machine.peek_be16(base + party::MAX_HP).unwrap_or(0);
        if !looks_populated(species, level, hp, max_hp) {
            return;
        }
        let hi = machine.peek(base + party::DVS).unwrap_or(0);
        let lo = machine.peek(base + party::DVS + 1).unwrap_or(0);
        self.latch(Sighting {
            species,
            level,
            dvs: Dvs::from_bytes(hi, lo),
            frames_into_attempt: self.frame.saturating_sub(self.frame_base),
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
        NAME
    }

    fn capabilities(&self) -> Capability {
        // Read-only, on purpose: a hunter that could write memory could
        // manufacture its own hit, and then no result it reported would mean
        // anything.
        Capability::OBSERVE | Capability::READ_MEMORY
    }

    /// The status-bar line. Attached to somebody's own game rather than to a
    /// hunt, this is the whole user interface, so it says whether the Pokémon on
    /// screen matches and shows the spread it judged that on.
    ///
    /// **It never says "shiny".** The word would be a claim about a Pokémon in a
    /// game that has none; `MATCH` is a claim about the target that was set, and
    /// that is the claim this plugin is entitled to make. The species is the
    /// internal index because the plugin is deliberately not given the
    /// cartridge — the trainer panel, which is, prints the name beside it.
    fn status(&self) -> String {
        if self.game.is_none() {
            return format!("not Red/Blue ({})", self.detected_title.trim());
        }
        match self.sighting {
            Some(s) if self.matched => format!(
                "MATCH  #{} lv{}  {}/{}/{}/{}  ({} seen)",
                s.species,
                s.level,
                s.dvs.attack,
                s.dvs.defense,
                s.dvs.speed,
                s.dvs.special,
                self.seen
            ),
            Some(s) => format!(
                "no match  #{} lv{}  {}/{}/{}/{}  ({} seen)",
                s.species,
                s.level,
                s.dvs.attack,
                s.dvs.defense,
                s.dvs.speed,
                s.dvs.special,
                self.seen
            ),
            None => format!("watching for {}", self.target.label()),
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
        // attempt — exactly the moment the latch must be cleared. `seen` is a
        // lifetime counter and deliberately survives.
        self.arm();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hunter_cannot_write_memory_or_press_buttons() {
        let p = ShinyHunt::new(Target::shiny());
        assert!(p.capabilities().contains(Capability::READ_MEMORY));
        assert!(p.capabilities().contains(Capability::OBSERVE));
        assert!(!p.capabilities().contains(Capability::WRITE_MEMORY));
        assert!(!p.capabilities().contains(Capability::CONTROL_INPUT));
        assert!(!p.capabilities().contains(Capability::SERIAL_LINK));
    }

    #[test]
    fn a_sighting_reports_the_derived_values_the_trainer_panel_does() {
        // The canonical shiny pair: `ATKDEFDV_SHINY` $EA / `SPDSPCDV_SHINY` $AA.
        let s = Sighting {
            species: 0x54,
            level: 5,
            dvs: Dvs::from_bytes(0xEA, 0xAA),
            frames_into_attempt: 0,
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
            frames_into_attempt: 7,
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

    /// A machine holding a hand-written Gen 1 wild battle, and nothing else.
    struct FakeRam {
        bytes: std::collections::HashMap<u16, u8>,
    }

    impl FakeRam {
        fn in_a_wild_battle(species: u8, level: u8, dv_hi: u8, dv_lo: u8) -> FakeRam {
            let mut bytes = std::collections::HashMap::new();
            bytes.insert(addr::IS_IN_BATTLE, 1);
            let b = addr::ENEMY_MON;
            bytes.insert(b + battle::SPECIES, species);
            bytes.insert(b + battle::LEVEL, level);
            bytes.insert(b + battle::HP, 0);
            bytes.insert(b + battle::HP + 1, 20);
            bytes.insert(b + battle::MAX_HP, 0);
            bytes.insert(b + battle::MAX_HP + 1, 20);
            bytes.insert(b + battle::DVS, dv_hi);
            bytes.insert(b + battle::DVS + 1, dv_lo);
            FakeRam { bytes }
        }
    }

    impl terminalgb_plugin_api::MachineAccess for FakeRam {
        fn peek(&mut self, address: u16) -> u8 {
            self.bytes.get(&address).copied().unwrap_or(0)
        }
        fn poke(&mut self, _address: u16, _value: u8) {}
        fn peek_bank(&mut self, _bank: usize, _address: u16) -> u8 {
            0xFF
        }
        fn rom_title(&self) -> String {
            "POKEMON BLUE".into()
        }
        fn global_checksum(&self) -> u16 {
            0x9D0A
        }
        fn rom_bank(&self) -> usize {
            1
        }
        fn set_button(&mut self, _key: terminalgb_plugin_api::KeypadKey, _down: bool) {}
        fn queue_press(
            &mut self,
            _key: terminalgb_plugin_api::KeypadKey,
            _h: u16,
            _g: u16,
        ) {
        }
        fn input_idle(&self) -> bool {
            true
        }
        fn clear_input(&mut self) {}
        fn frame_rgba(&self) -> &[u8] {
            &[]
        }
        fn frame_png(&self) -> Vec<u8> {
            Vec::new()
        }
        fn report_fault(&mut self, _reason: &str) {}
        fn fault(&self) -> Option<&str> {
            None
        }
    }

    fn deliver(p: &mut ShinyHunt, ram: &mut FakeRam, frame: u64) {
        let mut m = Machine::new(ram, p.capabilities());
        p.on_event(&Event::Frame { number: frame }, &mut m);
    }

    /// The bug that made the driver call every hit unreproducible.
    ///
    /// The host's frame counter is not machine state and `PluginHost::reset`
    /// does not rewind it, so the same attempt run on a fresh machine and on one
    /// that has already done sixteen attempts sees wildly different absolute
    /// frame numbers. If that number lands in the [`Sighting`], the two compare
    /// unequal and a perfectly good hit is reported as suspect.
    #[test]
    fn the_same_encounter_at_a_different_absolute_frame_is_the_same_sighting() {
        // Both machines meet the identical battle on the fourth frame of their
        // attempt. One is on its first attempt, the other on its five-hundredth.
        let battle = |ram: &mut FakeRam, on: bool| {
            ram.bytes.insert(addr::IS_IN_BATTLE, u8::from(on));
        };

        let mut ram = FakeRam::in_a_wild_battle(0xA5, 4, 0xEA, 0xAA);
        battle(&mut ram, false);
        let mut fresh = ShinyHunt::new(Target::shiny());
        {
            let mut m = Machine::new(&mut ram, fresh.capabilities());
            fresh.on_attach(&mut m);
        }
        for f in 1..=3 {
            deliver(&mut fresh, &mut ram, f);
        }
        battle(&mut ram, true);
        deliver(&mut fresh, &mut ram, 4);
        let a = fresh.sighting().expect("latched on a fresh machine");

        // A machine that has been running for a while. `reset` is what the host
        // calls on every `load_state`, i.e. at the top of every attempt.
        battle(&mut ram, false);
        let mut worn = ShinyHunt::new(Target::shiny());
        {
            let mut m = Machine::new(&mut ram, worn.capabilities());
            worn.on_attach(&mut m);
        }
        for f in 1..=500 {
            deliver(&mut worn, &mut ram, f);
        }
        worn.reset();
        for f in 501..=503 {
            deliver(&mut worn, &mut ram, f);
        }
        battle(&mut ram, true);
        deliver(&mut worn, &mut ram, 504);
        let b = worn.sighting().expect("latched on a worn machine");

        assert_eq!(a, b, "the same encounter must be the same Sighting");
        assert_eq!(a.frames_into_attempt, 4);
        assert!(a.shiny_if_traded());
    }

    /// The other half: a battle flag with an empty structure behind it must not
    /// be latched, and `$FF` (blacked out) must not read as a wild battle.
    #[test]
    fn an_incomplete_or_impossible_battle_is_not_latched() {
        assert!(!looks_populated(0, 5, 10, 20), "no species");
        assert!(!looks_populated(1, 0, 10, 20), "no level");
        assert!(!looks_populated(1, 5, 10, 0), "no max HP");
        assert!(!looks_populated(1, 5, 30, 20), "more HP than it can hold");
        assert!(looks_populated(1, 5, 20, 20));

        // `wIsInBattle` values the plugin must ignore: 0 (none), 2 (a trainer,
        // whose DVs are the fixed `$98`/`$88` pair and never a roll) and `$FF`
        // (the player blacked out — `home/overworld.asm` `.allPokemonFainted`).
        for flag in [0u8, 2, 3, 0xFF] {
            let mut ram = FakeRam::in_a_wild_battle(0xA5, 4, 0xEA, 0xAA);
            ram.bytes.insert(addr::IS_IN_BATTLE, flag);
            let mut p = ShinyHunt::new(Target::shiny());
            {
                let mut m = Machine::new(&mut ram, p.capabilities());
                p.on_attach(&mut m);
            }
            for f in 1..=5 {
                deliver(&mut p, &mut ram, f);
            }
            assert!(
                p.sighting().is_none(),
                "wIsInBattle = {flag:#04X} must not produce a sighting"
            );
        }
    }

    #[test]
    fn an_unrecognised_cartridge_reports_nothing_at_all() {
        let mut ram = FakeRam::in_a_wild_battle(0xA5, 4, 0xEA, 0xAA);
        struct Tetris(FakeRam);
        impl terminalgb_plugin_api::MachineAccess for Tetris {
            fn peek(&mut self, a: u16) -> u8 {
                self.0.peek(a)
            }
            fn poke(&mut self, _a: u16, _v: u8) {}
            fn peek_bank(&mut self, _b: usize, _a: u16) -> u8 {
                0xFF
            }
            fn rom_title(&self) -> String {
                "TETRIS".into()
            }
            fn global_checksum(&self) -> u16 {
                0x1234
            }
            fn rom_bank(&self) -> usize {
                1
            }
            fn set_button(&mut self, _k: terminalgb_plugin_api::KeypadKey, _d: bool) {}
            fn queue_press(
                &mut self,
                _k: terminalgb_plugin_api::KeypadKey,
                _h: u16,
                _g: u16,
            ) {
            }
            fn input_idle(&self) -> bool {
                true
            }
            fn clear_input(&mut self) {}
            fn frame_rgba(&self) -> &[u8] {
                &[]
            }
            fn frame_png(&self) -> Vec<u8> {
                Vec::new()
            }
            fn report_fault(&mut self, _r: &str) {}
            fn fault(&self) -> Option<&str> {
                None
            }
        }
        let mut tetris = Tetris(std::mem::replace(
            &mut ram,
            FakeRam {
                bytes: Default::default(),
            },
        ));
        let mut p = ShinyHunt::new(Target::shiny());
        {
            let mut m = Machine::new(&mut tetris, p.capabilities());
            p.on_attach(&mut m);
        }
        for f in 1..=5 {
            let mut m = Machine::new(&mut tetris, p.capabilities());
            p.on_event(&Event::Frame { number: f }, &mut m);
        }
        assert!(p.game().is_none());
        assert!(p.sighting().is_none());
        assert!(p.status().contains("not Red/Blue"));
    }

    /// The status row is the whole interface when the plugin is attached to
    /// somebody's own game rather than to a hunt, and the one word it must never
    /// contain is the one a reader would most expect.
    #[test]
    fn the_status_row_never_calls_a_gen_1_pokemon_shiny() {
        let mut p = ShinyHunt::new(Target::shiny());
        p.game = Some(Game::Blue);
        assert!(p.status().contains("watching for shiny-if-traded"));
        // A hit on the canonical shiny pair — still not the word "shiny" about
        // the Pokémon itself.
        p.latch(Sighting {
            species: 0x54,
            level: 5,
            dvs: Dvs::from_bytes(0xEA, 0xAA),
            frames_into_attempt: 3,
        });
        let s = p.status();
        assert!(s.starts_with("MATCH"), "{s}");
        assert!(s.contains("14/10/10/10"), "{s}");
        p.arm();
        p.latch(Sighting {
            species: 0x54,
            level: 5,
            dvs: Dvs::from_bytes(0x12, 0x34),
            frames_into_attempt: 3,
        });
        assert!(p.status().starts_with("no match"), "{}", p.status());
    }

    #[test]
    fn a_target_that_does_not_match_is_latched_but_is_not_a_hit() {
        let mut p = ShinyHunt::new(Target::shiny());
        p.latch(Sighting {
            species: 1,
            level: 3,
            dvs: Dvs::from_bytes(0x12, 0x34),
            frames_into_attempt: 1,
        });
        assert!(p.sighting().is_some(), "a miss is still an encounter");
        assert!(!p.is_hit());
        assert_eq!(p.seen(), 1);
    }
}

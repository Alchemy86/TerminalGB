//! A faithful model of Generation 1 Pokémon battle mechanics, and agents that
//! play it.
//!
//! # Why a battle
//!
//! A whole playthrough is a sprawling, badly-specified problem. A battle is a
//! **bounded game**: a small action space, a definite state, a definite outcome,
//! and — at seventy times real time — thousands of repetitions a minute. It is
//! the one part of the game where an agent can be *measured* rather than
//! watched.
//!
//! # The cartridge is the oracle
//!
//! Every rule in [`mechanics`] is a transcription of a named routine in
//! [`pret/pokered`](https://github.com/pret/pokered), cited in place, and every
//! one keeps the game's truncation, saturation and bugs. Where this model and a
//! real battle disagree, **the model is wrong** — the emulator in this
//! repository can run the same battle on the real cartridge and the difference
//! is a defect to fix, never a number to tune. That comparison is
//! `examples/gen1_battle_sim.rs`.
//!
//! Three consequences worth stating plainly:
//!
//! * **Bugs are behaviour.** The Focus Energy inversion, the 1/256 miss, the
//!   badge-boost stacking, the shared `wDamage` behind Counter — these are
//!   reproduced, not corrected. A model that fixes them is a model of a game
//!   nobody played.
//! * **Dice consumption is part of the model.** A turn that reaches the right
//!   answer while drawing a different number of random bytes diverges on the
//!   *next* turn. [`sim::TurnLog::rolls`] is what catches it.
//! * **What is not modelled says so.** [`effects::support`] classifies every
//!   move effect, and a battle that touched an unmodelled one carries that in
//!   [`sim::TurnLog::unmodelled`] and in the evaluation report. A win rate
//!   measured through a silently-dropped Disable is not a win rate.
//!
//! # No game data lives here
//!
//! Base stats, moves, the type chart and names are read out of the player's own
//! cartridge at run time by [`data::GameData::load_from_rom`]. The *formats* are
//! documented facts and are written down with their citations; the tables are
//! the game's and are not ours to ship. [`data::GameData::synthetic`] is how
//! every test in this crate runs with no ROM at all.
//!
//! # Layout
//!
//! | Module | What it is |
//! |---|---|
//! | [`types`] | Elemental types and the type chart |
//! | [`data`] | Cartridge tables, and the loader that reads them |
//! | [`state`] | The battle's state, laid out as the cartridge lays it out |
//! | [`rng`] | The dice, and the honesty rules around them |
//! | [`mechanics`] | The arithmetic, routine by routine |
//! | [`effects`] | Move-effect ids, and how completely each is modelled |
//! | [`sim`] | Turn resolution — `MainInBattleLoop` |
//! | [`ai`] | Agents, and the trait an LLM plugs into |
//! | [`eval`] | The evaluation protocol |
//!
//! # This crate is dependency-free, and does not depend on `terminalgb`
//!
//! The same arrangement, for the same reason, as `terminalgb-plugin-api` and
//! `terminalgb-chiptune`: the differential harness and the CLI live in
//! `terminalgb`, so `terminalgb` must be able to depend on *this*. Putting the
//! engine inside the emulator would make that edge a cycle.
//!
//! It is deliberately **not a plugin** either. A `Plugin` is handed a machine
//! during a frame the emulator has already decided to run; an engine that
//! searches thousands of hypothetical battles has no machine and no frame.
//! `plugins/battle-control` — which *starts* battles on the real cartridge — is
//! the plugin, and this is what reasons about them.

pub mod ai;
pub mod data;
pub mod effects;
pub mod eval;
pub mod mechanics;
pub mod rng;
pub mod sim;
pub mod state;
#[cfg(test)]
mod testsupport;
pub mod types;

pub use data::{BaseStats, GameData, MoveData};
pub use rng::{Provenance, Rng};
pub use sim::{Engine, Event, TurnLog};
pub use state::{
    Action, Battle, BattleKind, BattleMon, Dvs, MoveSlot, Outcome, Party, Side, Stages,
    Stat, Stats, Status,
};
pub use types::{Type, TypeChart};

/// The version of the engine's behaviour.
///
/// Bump it when a rule changes, so a stored evaluation result can be compared
/// only against results from the same engine. A win rate from two different
/// rule sets is two different measurements.
pub const ENGINE_VERSION: u32 = 1;

//! A tiny invented cartridge, shared by the crate's tests.
//!
//! **Nothing here is game data.** Every number is made up: two species with
//! round base stats, and nine moves chosen to exercise one mechanic each. That
//! is not merely licence hygiene — a fixture built from real base stats would
//! make a failing test say "Bulbasaur is wrong" when what is wrong is the
//! arithmetic, and it would need a ROM on the machine to run at all.
//!
//! The real cartridge's numbers are checked in `examples/gen1_battle_sim.rs`,
//! against the real cartridge, which is where they belong.

#![cfg(test)]

use crate::data::{BaseStats, GameData, MoveData, STRUGGLE};
use crate::effects::id as fx;
use crate::sim::{switch_in, Engine};
use crate::state::{Battle, BattleKind, BattleMon, Dvs, Party, Side};
use crate::types::{Type, TypeChart};

fn species(dex: u8, hp: u8, atk: u8, def: u8, spd: u8, spc: u8, t: Type) -> BaseStats {
    BaseStats {
        dex,
        hp,
        attack: atk,
        defense: def,
        speed: spd,
        special: spc,
        type1: t,
        type2: t,
        catch_rate: 45,
        base_exp: 64,
    }
}

fn mv(id: u8, effect: u8, power: u8, kind: Type, acc: u8, pp: u8) -> MoveData {
    MoveData {
        id,
        animation: id,
        effect,
        power,
        kind,
        accuracy: acc,
        pp,
    }
}

/// The fixture cartridge.
///
/// Species 1 is fast (base speed 100); species 2 is slow (50). Moves:
///
/// | id | what it exercises |
/// |---|---|
/// | 1 | ordinary 40-power damage |
/// | 2 | a move that does nothing at all |
/// | 3 | a stat-up effect |
/// | 4 | sleep |
/// | 5 | 100 power with recoil |
/// | 6 | Hyper Beam (the recharge, and the no-recharge-on-KO rule) |
/// | 7 | Leech Seed |
/// | 8 | a heal |
/// | 9 | Substitute |
pub fn engine() -> Engine {
    let mut moves = vec![
        mv(1, fx::NO_ADDITIONAL_EFFECT, 40, Type::NORMAL, 255, 35),
        mv(2, fx::NO_ADDITIONAL_EFFECT, 0, Type::NORMAL, 255, 30),
        mv(3, fx::ATTACK_UP1, 0, Type::NORMAL, 255, 30),
        mv(4, fx::SLEEP, 0, Type::NORMAL, 255, 20),
        mv(5, fx::RECOIL, 100, Type::NORMAL, 255, 15),
        mv(6, fx::HYPER_BEAM, 150, Type::NORMAL, 230, 5),
        mv(7, fx::LEECH_SEED, 0, Type::GRASS, 229, 10),
        mv(8, fx::HEAL, 0, Type::NORMAL, 255, 20),
        mv(9, fx::SUBSTITUTE, 0, Type::NORMAL, 255, 10),
    ];
    while moves.len() < STRUGGLE as usize {
        let id = moves.len() as u8 + 1;
        moves.push(mv(id, fx::NO_ADDITIONAL_EFFECT, 0, Type::NORMAL, 255, 10));
    }
    moves[STRUGGLE as usize - 1] = mv(STRUGGLE, fx::RECOIL, 50, Type::NORMAL, 255, 10);

    let data = GameData::synthetic(
        vec![
            species(1, 100, 100, 100, 100, 100, Type::NORMAL),
            species(2, 100, 100, 100, 50, 100, Type::NORMAL),
        ],
        moves,
    )
    .with_chart(TypeChart::neutral());
    Engine::new(data)
}

/// Build a fixture Pokémon with perfect DVs.
pub fn battler(e: &Engine, species: u8, level: u8, moves: [u8; 4]) -> BattleMon {
    BattleMon::build(e.data(), species, level, Dvs::PERFECT, moves)
        .expect("the fixture cartridge knows this species")
}

/// A one-on-one trainer battle.
pub fn duel(p: BattleMon, e: BattleMon) -> Battle {
    Battle::new(
        Party::new(vec![p]),
        Party::new(vec![e]),
        BattleKind::Trainer,
    )
}

/// Run both sides through the switch-in stat pipeline, as the cartridge does
/// before the first turn.
pub fn switch_both(b: &mut Battle) {
    let badges = b.badges;
    let apply = b.badges_apply();
    switch_in(b.active_mut(Side::Player), badges, apply);
    switch_in(b.active_mut(Side::Enemy), badges, apply);
}

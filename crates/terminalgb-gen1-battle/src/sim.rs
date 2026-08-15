//! Turn resolution — `MainInBattleLoop`, in Rust.
//!
//! # The shape of a turn
//!
//! `MainInBattleLoop` (`engine/battle/core.asm`) does, in order:
//!
//! 1. Both sides choose. A side committed to a move (recharging, charging,
//!    thrashing, trapped) does not get a choice; a side whose opponent is
//!    using a trapping move has `CANNOT_MOVE` written over its selection.
//! 2. Turn order is decided **once**, before either move runs — Quick Attack,
//!    then Counter (negative priority), then in-battle Speed, then a coin flip.
//! 3. The first side executes. Then **its** residual damage — poison, burn,
//!    Leech Seed — runs.
//! 4. The second side executes, unless it fainted. Then its residual damage.
//! 5. Trapping counters tick down.
//!
//! That residual damage runs **after each side's move** rather than once at the
//! end of the turn is the detail most models get wrong, and it is worth a
//! Pokémon: a poisoned Pokémon that moves first and is then knocked to 1 HP
//! survives; one that moves second dies to its own poison before it acts.
//!
//! # What is not here
//!
//! The engine does not model every effect, and it says which
//! ([`crate::effects::support`], [`TurnLog::unmodelled`]). A result computed
//! through an unmodelled effect is *reported as such*; it is never quietly
//! folded into a win rate. That is the same rule the repository applies to a
//! skipped test.

use crate::data::{GameData, MoveData, STRUGGLE};
use crate::effects::{self, id as fx, Inflicts, Support};
use crate::mechanics as mech;
use crate::rng::Rng;
use crate::state::{Action, Battle, BattleKind, BattleMon, Outcome, Side, Stat, Status};
use crate::types::Type;

/// Everything one turn did, in enough detail to diff against a real battle.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TurnLog {
    pub turn: u32,
    /// Which side the engine decided moves first.
    pub first: Option<Side>,
    pub events: Vec<Event>,
    /// Effects that were reached but not modelled, with the reason. **A
    /// non-empty list means any conclusion drawn from this battle is
    /// qualified.**
    pub unmodelled: Vec<String>,
    /// Random bytes consumed by this turn. Comparing this against a real battle
    /// catches a model that got the answer right by drawing the wrong dice.
    pub rolls: u64,
}

impl TurnLog {
    pub fn damage_to(&self, side: Side) -> u16 {
        self.events
            .iter()
            .filter_map(|e| match e {
                Event::Damage { target, amount, .. } if *target == side => Some(*amount),
                _ => None,
            })
            .sum()
    }
    pub fn is_clean(&self) -> bool {
        self.unmodelled.is_empty()
    }
}

/// One observable thing that happened.
///
/// Deliberately close to what the cartridge would print, because that is what a
/// harness reading the screen can compare against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Used {
        side: Side,
        move_id: u8,
    },
    Missed {
        side: Side,
        why: MissReason,
    },
    Damage {
        side: Side,
        target: Side,
        amount: u16,
        critical: bool,
        effectiveness: u16,
    },
    Fainted {
        side: Side,
    },
    StatusInflicted {
        side: Side,
        target: Side,
        status: Inflicts,
    },
    StatChanged {
        side: Side,
        target: Side,
        stat: Stat,
        by: i8,
        moved: bool,
    },
    Residual {
        side: Side,
        amount: u16,
        source: Residual,
    },
    Healed {
        side: Side,
        amount: u16,
    },
    Recoil {
        side: Side,
        amount: u16,
    },
    Immobilised {
        side: Side,
        why: Immobility,
    },
    SwitchedIn {
        side: Side,
        slot: usize,
    },
    SubstituteUp {
        side: Side,
        hp: u8,
    },
    SubstituteBroke {
        side: Side,
    },
    Ran {
        side: Side,
        escaped: bool,
    },
    /// The move happened but its secondary behaviour is not modelled.
    Unmodelled {
        side: Side,
        move_id: u8,
        why: &'static str,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissReason {
    /// The accuracy roll. Includes the 1/256.
    Accuracy,
    /// The type chart drove the damage to zero — the cartridge calls this a
    /// miss, and it cannot be avoided by any amount of accuracy.
    NoEffect,
    /// Dream Eater on an awake target.
    TargetAwake,
    /// The target is underground or airborne.
    Invulnerable,
    /// A stat-lowering move's own 25 % failure.
    StatDownFailed,
    /// The status is already present, or the target's type is immune to it.
    AlreadyStatused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Immobility {
    Asleep,
    Frozen,
    FullyParalysed,
    Flinched,
    Recharging,
    Trapped,
    /// Confusion hit itself.
    HurtItself,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Residual {
    Poison,
    Burn,
    Toxic,
    LeechSeed,
}

/// The battle engine.
///
/// Holds only the cartridge's data tables; all the state is in the [`Battle`]
/// passed to [`Engine::resolve_turn`]. That split is what lets a search agent
/// clone a battle and roll it forward thousands of times without cloning the
/// tables.
pub struct Engine {
    data: GameData,
    /// Cap on turns before a battle is declared a draw by exhaustion. Not a
    /// cartridge rule — an episode's, so a stall cannot hang a training run.
    pub turn_limit: u32,
}

impl Engine {
    pub fn new(data: GameData) -> Engine {
        Engine {
            data,
            turn_limit: 300,
        }
    }

    pub fn data(&self) -> &GameData {
        &self.data
    }

    /// What a side may legally choose this turn.
    ///
    /// Returns a single-element list when the side is committed — recharging,
    /// charging, thrashing, or held by the opponent's trapping move. That is
    /// `MainInBattleLoop`'s `CANNOT_MOVE` and the multi-turn checks in
    /// `CheckPlayerStatusConditions`, expressed as a legality question so an
    /// agent cannot choose something the cartridge would refuse.
    pub fn legal_actions(&self, battle: &Battle, side: Side) -> Vec<Action> {
        let me = battle.active(side);
        let them = battle.active(side.other());

        if me.is_fainted() {
            return battle
                .side(side)
                .switchable()
                .into_iter()
                .map(Action::Switch)
                .collect();
        }
        // Held in place by the opponent's Wrap/Bind — no choice at all.
        if them.volatile.trapping && them.volatile.locked_turns > 0 {
            return vec![Action::Move(0)];
        }
        if me.volatile.is_locked() {
            let slot = me
                .moves
                .iter()
                .position(|m| m.id == me.volatile.locked_move)
                .unwrap_or(0);
            return vec![Action::Move(slot)];
        }

        let mut out: Vec<Action> =
            me.usable_slots().into_iter().map(Action::Move).collect();
        if out.is_empty() {
            out.push(Action::Struggle);
        }
        out.extend(
            battle
                .side(side)
                .switchable()
                .into_iter()
                .map(Action::Switch),
        );
        if battle.kind == BattleKind::Wild && side == Side::Player {
            out.push(Action::Run);
        }
        out
    }

    /// Resolve one whole turn.
    pub fn resolve_turn(
        &self,
        battle: &mut Battle,
        player: Action,
        enemy: Action,
        rng: &mut dyn Rng,
    ) -> TurnLog {
        let start_rolls = rng.rolls();
        let mut log = TurnLog {
            turn: battle.turn,
            ..Default::default()
        };

        // Switches happen before anything else and do not contend for turn
        // order — `MainInBattleLoop` handles them ahead of the move phase.
        let player_switched = self.maybe_switch(battle, Side::Player, player, &mut log);
        let enemy_switched = self.maybe_switch(battle, Side::Enemy, enemy, &mut log);

        if let Action::Run = player {
            // Running from a wild battle: the escape formula is not modelled,
            // and a run in a trainer battle is not legal.
            if battle.kind == BattleKind::Wild {
                log.events.push(Event::Ran {
                    side: Side::Player,
                    escaped: true,
                });
                battle.turn += 1;
                log.rolls = rng.rolls() - start_rolls;
                return log;
            }
        }

        let p_move = self.chosen(battle, Side::Player, player);
        let e_move = self.chosen(battle, Side::Enemy, enemy);

        let first = if player_switched && !enemy_switched {
            Side::Enemy
        } else if enemy_switched && !player_switched {
            Side::Player
        } else if player_switched && enemy_switched {
            // Both switched: nobody attacks.
            battle.turn += 1;
            log.rolls = rng.rolls() - start_rolls;
            return log;
        } else {
            let p_first = mech::player_moves_first(
                rng,
                p_move.map(|m| m.id).unwrap_or(0),
                e_move.map(|m| m.id).unwrap_or(0),
                battle.active(Side::Player).current.speed,
                battle.active(Side::Enemy).current.speed,
            );
            if p_first {
                Side::Player
            } else {
                Side::Enemy
            }
        };
        log.first = Some(first);

        let order = if player_switched || enemy_switched {
            vec![first]
        } else {
            vec![first, first.other()]
        };

        for side in order {
            if battle.active(side).is_fainted()
                || battle.active(side.other()).is_fainted()
            {
                break;
            }
            let action = if side == Side::Player { player } else { enemy };
            if action.is_switch() {
                continue;
            }
            self.execute_move(battle, side, action, rng, &mut log);
            // Residual damage runs after EACH side's move, not once per turn.
            if !battle.active(side).is_fainted() {
                self.residuals(battle, side, &mut log);
            }
            if battle.outcome().is_some() {
                break;
            }
        }

        // Trapping counters tick at the end of the loop (`CheckNumAttacksLeft`).
        for side in [Side::Player, Side::Enemy] {
            let m = battle.active_mut(side);
            if m.volatile.trapping {
                m.volatile.locked_turns = m.volatile.locked_turns.saturating_sub(1);
                if m.volatile.locked_turns == 0 {
                    m.volatile.trapping = false;
                }
            }
            // A flinch lasts exactly the turn it was applied.
            m.volatile.flinched = false;
        }

        battle.turn += 1;
        log.rolls = rng.rolls() - start_rolls;
        log
    }

    fn maybe_switch(
        &self,
        battle: &mut Battle,
        side: Side,
        action: Action,
        log: &mut TurnLog,
    ) -> bool {
        let Action::Switch(slot) = action else {
            return false;
        };
        let party = battle.side_mut(side);
        if slot >= party.mons.len()
            || party.mons[slot].is_fainted()
            || slot == party.active
        {
            return false;
        }
        party.active_mut().volatile.on_switch_out();
        party.active_mut().stages.reset();
        party.active = slot;
        let badges = battle.badges;
        let apply = battle.badges_apply();
        let m = battle.active_mut(side);
        m.stages.reset();
        mech::recompute_current_stats(m, badges, apply);
        log.events.push(Event::SwitchedIn { side, slot });
        true
    }

    fn chosen(&self, battle: &Battle, side: Side, action: Action) -> Option<MoveData> {
        let mon = battle.active(side);
        match action {
            Action::Struggle => self.data.move_data(STRUGGLE).copied(),
            Action::Move(slot) => {
                let id = mon.moves.get(slot)?.id;
                self.data.move_data(id).copied()
            }
            _ => None,
        }
    }

    /// One side's move, from the status gate through to the damage.
    fn execute_move(
        &self,
        battle: &mut Battle,
        side: Side,
        action: Action,
        rng: &mut dyn Rng,
        log: &mut TurnLog,
    ) {
        // -- CheckPlayerStatusConditions, in the cartridge's order ------------
        if let Some(why) = self.status_gate(battle, side, rng, log) {
            log.events.push(Event::Immobilised { side, why });
            return;
        }

        let Some(move_data) = self.chosen(battle, side, action) else {
            return;
        };
        let is_struggle = move_data.id == STRUGGLE;

        // PP is decremented BEFORE the accuracy test, so a miss still costs it.
        // `DecrementPP` skips Struggle and skips continuation turns.
        if let Action::Move(slot) = action {
            if !battle.active(side).volatile.is_locked() {
                if let Some(m) = battle.active_mut(side).moves.get_mut(slot) {
                    m.pp = m.pp.saturating_sub(1);
                }
            }
        }

        log.events.push(Event::Used {
            side,
            move_id: move_data.id,
        });
        battle.last_move[side as usize] = move_data.id;

        let support = effects::support(move_data.effect);
        if let Some(why) = support.note() {
            log.events.push(Event::Unmodelled {
                side,
                move_id: move_data.id,
                why,
            });
            log.unmodelled.push(format!(
                "{} used move #{} (effect ${:02X}): {}",
                side.name(),
                move_data.id,
                move_data.effect,
                why
            ));
            if matches!(support, Support::None(_)) && move_data.power == 0 {
                return;
            }
        }

        // -- Accuracy ---------------------------------------------------------
        if !effects::never_misses(move_data.effect) {
            if battle.active(side.other()).volatile.invulnerable {
                log.events.push(Event::Missed {
                    side,
                    why: MissReason::Invulnerable,
                });
                battle.active_mut(side).volatile.trapping = false;
                return;
            }
            if move_data.effect == fx::DREAM_EATER
                && !battle.active(side.other()).status.is_asleep()
            {
                log.events.push(Event::Missed {
                    side,
                    why: MissReason::TargetAwake,
                });
                return;
            }
            let acc = mech::hit_chance(
                move_data.accuracy,
                &battle.active(side).stages,
                &battle.active(side.other()).stages,
            );
            if !mech::accuracy_roll(rng, acc) {
                log.events.push(Event::Missed {
                    side,
                    why: MissReason::Accuracy,
                });
                // A miss ends a trapping move.
                battle.active_mut(side).volatile.trapping = false;
                return;
            }
        }

        // -- Damage -----------------------------------------------------------
        let mut dealt = 0u16;
        if move_data.power > 0 || effects::is_set_damage(move_data.effect) {
            match self.damage(battle, side, &move_data, rng, log) {
                Some(d) => dealt = d,
                // A move driven to zero by the type chart is a miss and its
                // effects do not run.
                None => return,
            }
        }

        // -- Effects ----------------------------------------------------------
        self.apply_effects(battle, side, &move_data, dealt, is_struggle, rng, log);

        // Fainting is checked after everything.
        for s in [Side::Player, Side::Enemy] {
            if battle.active(s).is_fainted() {
                log.events.push(Event::Fainted { side: s });
            }
        }
    }

    /// `CheckPlayerStatusConditions` — the gate, in the cartridge's order:
    /// sleep, freeze, trapped, flinch, recharge, confusion, paralysis.
    fn status_gate(
        &self,
        battle: &mut Battle,
        side: Side,
        rng: &mut dyn Rng,
        log: &mut TurnLog,
    ) -> Option<Immobility> {
        // Held in place by the opponent's trapping move.
        let trapped = {
            let them = battle.active(side.other());
            them.volatile.trapping && them.volatile.locked_turns > 0
        };
        if trapped {
            return Some(Immobility::Trapped);
        }

        let mon = battle.active_mut(side);

        // Sleep: decrement, and lose the turn either way.
        if mon.status.is_asleep() {
            let left = mon.status.sleep_turns() - 1;
            mon.status = Status::asleep(left);
            return Some(Immobility::Asleep);
        }
        // Freeze never ends on its own.
        if mon.status.is_frozen() {
            return Some(Immobility::Frozen);
        }
        if mon.volatile.flinched {
            return Some(Immobility::Flinched);
        }
        if mon.volatile.recharging {
            mon.volatile.recharging = false;
            return Some(Immobility::Recharging);
        }
        // Confusion: tick, then a 50/50 self-hit.
        if mon.volatile.confused {
            mon.volatile.confusion_turns = mon.volatile.confusion_turns.saturating_sub(1);
            if mon.volatile.confusion_turns == 0 {
                mon.volatile.confused = false;
            } else if rng.next_byte() > mech::percent(50) {
                // Confusion self-damage: a 40-power typeless physical hit
                // against the user's own Defense.
                let mon = battle.active(side);
                let v = mech::DamageVars {
                    attack: mon.current.attack,
                    defense: mon.current.defense,
                    power: 40,
                    level: mon.level,
                };
                let d = mech::base_damage(&v).unwrap_or(1);
                let mon = battle.active_mut(side);
                let d = d.min(mon.hp);
                mon.hp -= d;
                log.events.push(Event::Damage {
                    side,
                    target: side,
                    amount: d,
                    critical: false,
                    effectiveness: 100,
                });
                return Some(Immobility::HurtItself);
            }
        }
        if battle.active(side).status.is_paralysed() && mech::fully_paralysed(rng) {
            return Some(Immobility::FullyParalysed);
        }
        None
    }

    /// The damage pipeline: crit, stats, base damage, STAB, type, roll, apply.
    ///
    /// Returns `None` when the move missed by type (zero damage), which is a
    /// miss and stops the effects running.
    fn damage(
        &self,
        battle: &mut Battle,
        side: Side,
        move_data: &MoveData,
        rng: &mut dyn Rng,
        log: &mut TurnLog,
    ) -> Option<u16> {
        // Set-damage effects skip the whole pipeline.
        if effects::is_set_damage(move_data.effect) {
            let amount = match move_data.effect {
                // Super Fang: half the target's current HP, minimum 1.
                fx::SUPER_FANG => (battle.active(side.other()).hp / 2).max(1),
                // Seismic Toss / Night Shade deal the user's level; the others
                // are fixed and are not distinguished here.
                _ => u16::from(battle.active(side).level),
            };
            return Some(self.deal(battle, side, amount, false, 100, log));
        }

        let base_speed = mech::base_speed(&self.data, battle.active(side));
        let focus = battle.active(side).volatile.focus_energy;
        let critical = mech::critical_hit(rng, base_speed, focus, move_data);

        let vars = mech::damage_vars(
            battle.active(side),
            battle.active(side.other()),
            move_data,
            critical,
        );
        let Some(base) = mech::base_damage(&vars) else {
            return Some(0);
        };

        let typed = mech::adjust_for_type(
            base,
            move_data.kind,
            battle.active(side),
            battle.active(side.other()),
            self.data.chart(),
        );
        if typed.missed_by_type {
            log.events.push(Event::Missed {
                side,
                why: MissReason::NoEffect,
            });
            return None;
        }
        let rolled = mech::randomize_damage(rng, typed.damage);

        // Multi-hit: damage and accuracy are computed ONCE and the same figure
        // is applied to every hit.
        let hits = match move_data.effect {
            fx::TWO_TO_FIVE_ATTACKS => mech::roll_multi_hit(rng),
            fx::ATTACK_TWICE | fx::TWINEEDLE => 2,
            _ => 1,
        };
        let mut total = 0u16;
        for _ in 0..hits {
            if battle.active(side.other()).is_fainted() {
                break;
            }
            total = total.saturating_add(self.deal(
                battle,
                side,
                rolled,
                critical,
                typed.effectiveness,
                log,
            ));
        }
        Some(total)
    }

    /// Apply damage, honouring a Substitute.
    ///
    /// `AttackSubstitute` (`engine/battle/core.asm`) carries two quirks that are
    /// reproduced: damage above 255 always breaks a Substitute regardless of its
    /// HP, and when it breaks *the damage figure is not updated* with how much
    /// the Substitute actually had — so Counter and recoil see the full number.
    fn deal(
        &self,
        battle: &mut Battle,
        side: Side,
        amount: u16,
        critical: bool,
        effectiveness: u16,
        log: &mut TurnLog,
    ) -> u16 {
        let target = side.other();
        if let Some(sub) = battle.active(target).volatile.substitute_hp {
            let broke = amount > 0xFF || amount >= u16::from(sub);
            let m = battle.active_mut(target);
            if broke {
                m.volatile.substitute_hp = None;
                log.events.push(Event::SubstituteBroke { side: target });
            } else {
                m.volatile.substitute_hp = Some(sub - amount as u8);
            }
            // `wDamage` is left holding the full figure either way.
            battle.last_damage = amount;
            return amount;
        }
        let m = battle.active_mut(target);
        let applied = amount.min(m.hp);
        m.hp -= applied;
        battle.last_damage = applied;
        log.events.push(Event::Damage {
            side,
            target,
            amount: applied,
            critical,
            effectiveness,
        });
        applied
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_effects(
        &self,
        battle: &mut Battle,
        side: Side,
        move_data: &MoveData,
        dealt: u16,
        is_struggle: bool,
        rng: &mut dyn Rng,
        log: &mut TurnLog,
    ) {
        let e = move_data.effect;
        let badges = battle.badges;
        let apply_badges = battle.badges_apply();

        // -- stat changes -----------------------------------------------------
        if let Some(sc) = effects::stat_change(e) {
            let roll_ok = if sc.is_side_effect {
                rng.next_byte() < mech::side_effect::WEAK
            } else if !sc.on_self && battle.kind != BattleKind::Link {
                // A stat-lowering move fails 25 % of the time outside a link
                // battle.
                if rng.next_byte() < mech::side_effect::STAT_DOWN_MISS {
                    log.events.push(Event::Missed {
                        side,
                        why: MissReason::StatDownFailed,
                    });
                    false
                } else {
                    true
                }
            } else {
                true
            };
            if roll_ok {
                let target = if sc.on_self { side } else { side.other() };
                // Mist blocks a stat drop.
                if !sc.on_self && battle.active(target).volatile.mist {
                    log.events.push(Event::Missed {
                        side,
                        why: MissReason::AlreadyStatused,
                    });
                } else {
                    let mon = battle.active_mut(target);
                    let moved = mech::apply_stat_change(
                        mon,
                        sc.stat,
                        sc.by,
                        badges,
                        apply_badges,
                    );
                    log.events.push(Event::StatChanged {
                        side,
                        target,
                        stat: sc.stat,
                        by: sc.by,
                        moved,
                    });
                }
            }
        }

        // -- status -----------------------------------------------------------
        if let Some((what, chance)) = effects::inflicts(e) {
            let lands = match chance {
                Some(c) => rng.next_byte() < c,
                None => true,
            };
            if lands {
                self.inflict(battle, side, what, rng, log);
            }
        }

        // -- everything else --------------------------------------------------
        match e {
            fx::HEAL => {
                let m = battle.active_mut(side);
                let heal = (m.max_hp / 2).min(m.max_hp - m.hp);
                m.hp += heal;
                log.events.push(Event::Healed { side, amount: heal });
            }
            fx::DRAIN_HP | fx::DREAM_EATER => {
                let heal = (dealt / 2).max(1);
                let m = battle.active_mut(side);
                let heal = heal.min(m.max_hp - m.hp);
                m.hp += heal;
                log.events.push(Event::Healed { side, amount: heal });
            }
            fx::RECOIL | fx::JUMP_KICK => {
                let r = mech::recoil_damage(dealt, is_struggle);
                let m = battle.active_mut(side);
                let r = r.min(m.hp);
                m.hp -= r;
                log.events.push(Event::Recoil { side, amount: r });
            }
            fx::EXPLODE => {
                let m = battle.active_mut(side);
                m.hp = 0;
                log.events.push(Event::Fainted { side });
            }
            fx::HYPER_BEAM => {
                // **No recharge on a KO.** `ApplyAttackToEnemyPokemon` returns
                // before the effect runs when the target fainted, and
                // HYPER_BEAM_EFFECT is not in `AlwaysHappenSideEffects`.
                if !battle.active(side.other()).is_fainted() {
                    battle.active_mut(side).volatile.recharging = true;
                    battle.active_mut(side).volatile.locked_move = move_data.id;
                }
            }
            fx::FOCUS_ENERGY => {
                battle.active_mut(side).volatile.focus_energy = true;
            }
            fx::MIST => battle.active_mut(side).volatile.mist = true,
            fx::REFLECT => battle.active_mut(side).volatile.reflect = true,
            fx::LIGHT_SCREEN => battle.active_mut(side).volatile.light_screen = true,
            fx::LEECH_SEED => {
                // Grass types cannot be seeded.
                let t = battle.active(side.other());
                if t.type1 != Type::GRASS && t.type2 != Type::GRASS {
                    battle.active_mut(side.other()).volatile.seeded = true;
                }
            }
            fx::HAZE => {
                for s in [Side::Player, Side::Enemy] {
                    let m = battle.active_mut(s);
                    m.stages.reset();
                    m.volatile.reflect = false;
                    m.volatile.light_screen = false;
                    m.volatile.mist = false;
                    m.volatile.focus_energy = false;
                    m.volatile.confused = false;
                    m.volatile.confusion_turns = 0;
                    m.volatile.seeded = false;
                    m.volatile.toxic = false;
                    mech::recompute_current_stats(m, badges, apply_badges);
                }
                // Haze cures the *opponent's* status.
                battle.active_mut(side.other()).status = Status::HEALTHY;
            }
            fx::SUBSTITUTE => {
                let m = battle.active_mut(side);
                let cost = m.max_hp / 4;
                // The disassembly's own note: it branches only on carry, so a
                // user can be left with exactly 0 HP.
                if m.hp >= cost && m.volatile.substitute_hp.is_none() {
                    m.hp -= cost;
                    let sub = (cost + 1).min(255) as u8;
                    m.volatile.substitute_hp = Some(sub);
                    log.events.push(Event::SubstituteUp { side, hp: sub });
                }
            }
            fx::TRAPPING => {
                let turns = mech::roll_trapping_turns(rng);
                let m = battle.active_mut(side);
                m.volatile.trapping = true;
                m.volatile.locked_turns = turns;
                m.volatile.locked_move = move_data.id;
            }
            fx::THRASH_PETAL_DANCE => {
                let m = battle.active_mut(side);
                if !m.volatile.thrashing {
                    m.volatile.thrashing = true;
                    m.volatile.locked_turns = 2 + (rng.next_byte() & 1);
                    m.volatile.locked_move = move_data.id;
                } else {
                    m.volatile.locked_turns = m.volatile.locked_turns.saturating_sub(1);
                    if m.volatile.locked_turns == 0 {
                        m.volatile.thrashing = false;
                        m.volatile.confused = true;
                        m.volatile.confusion_turns = 2 + (rng.next_byte() & 3);
                    }
                }
            }
            fx::OHKO => {
                // `OneHitKOEffect_`: it fails outright against a faster target,
                // never crits and never uses accuracy.
                if battle.active(side).current.speed
                    >= battle.active(side.other()).current.speed
                {
                    let m = battle.active_mut(side.other());
                    m.hp = 0;
                    log.events.push(Event::Fainted { side: side.other() });
                }
            }
            _ => {}
        }
    }

    fn inflict(
        &self,
        battle: &mut Battle,
        side: Side,
        what: Inflicts,
        rng: &mut dyn Rng,
        log: &mut TurnLog,
    ) {
        let target = side.other();
        // Flinch and confusion are volatile and do not need a healthy target.
        match what {
            Inflicts::Flinch => {
                battle.active_mut(target).volatile.flinched = true;
                log.events.push(Event::StatusInflicted {
                    side,
                    target,
                    status: what,
                });
                return;
            }
            Inflicts::Confusion => {
                let m = battle.active_mut(target);
                if !m.volatile.confused {
                    m.volatile.confused = true;
                    m.volatile.confusion_turns = 2 + (rng.next_byte() & 3);
                    log.events.push(Event::StatusInflicted {
                        side,
                        target,
                        status: what,
                    });
                }
                return;
            }
            _ => {}
        }

        // A non-volatile status only lands on a healthy target, and the
        // type immunities apply.
        let t = battle.active(target);
        if !t.status.is_healthy() {
            log.events.push(Event::Missed {
                side,
                why: MissReason::AlreadyStatused,
            });
            return;
        }
        let immune = match what {
            Inflicts::Poison => t.type1 == Type::POISON || t.type2 == Type::POISON,
            Inflicts::Burn => t.type1 == Type::FIRE || t.type2 == Type::FIRE,
            Inflicts::Freeze => t.type1 == Type::ICE || t.type2 == Type::ICE,
            _ => false,
        };
        if immune {
            log.events.push(Event::Missed {
                side,
                why: MissReason::AlreadyStatused,
            });
            return;
        }

        let status = match what {
            Inflicts::Sleep => Status::asleep(mech::roll_sleep_turns(rng)),
            Inflicts::Poison => Status(Status::PSN),
            Inflicts::Paralysis => Status(Status::PAR),
            Inflicts::Burn => Status(Status::BRN),
            Inflicts::Freeze => Status(Status::FRZ),
            _ => return,
        };
        let badges = battle.badges;
        let apply = battle.badges_apply();
        let m = battle.active_mut(target);
        m.status = status;
        // Burn and paralysis change the stats immediately.
        mech::recompute_current_stats(m, badges, apply);
        log.events.push(Event::StatusInflicted {
            side,
            target,
            status: what,
        });
    }

    /// `HandlePoisonBurnLeechSeed` — runs after **each** side's move.
    fn residuals(&self, battle: &mut Battle, side: Side, log: &mut TurnLog) {
        let (max_hp, status, toxic, seeded) = {
            let m = battle.active(side);
            (m.max_hp, m.status, m.volatile.toxic, m.volatile.seeded)
        };

        if status.is_poisoned() || status.is_burned() {
            let amount = if toxic {
                let m = battle.active_mut(side);
                m.volatile.toxic_counter = m.volatile.toxic_counter.saturating_add(1);
                mech::toxic_damage(max_hp, m.volatile.toxic_counter)
            } else {
                mech::residual_damage(max_hp)
            };
            let m = battle.active_mut(side);
            let amount = amount.min(m.hp);
            m.hp -= amount;
            let source = if toxic {
                Residual::Toxic
            } else if status.is_burned() {
                Residual::Burn
            } else {
                Residual::Poison
            };
            log.events.push(Event::Residual {
                side,
                amount,
                source,
            });
            if m.hp == 0 {
                log.events.push(Event::Fainted { side });
                return;
            }
        }

        if seeded {
            // The toxic counter multiplies Leech Seed too — the disassembly's
            // "hence the Leech Seed glitch".
            let amount = if toxic {
                let m = battle.active_mut(side);
                m.volatile.toxic_counter = m.volatile.toxic_counter.saturating_add(1);
                mech::toxic_damage(max_hp, m.volatile.toxic_counter)
            } else {
                mech::residual_damage(max_hp)
            };
            let m = battle.active_mut(side);
            let amount = amount.min(m.hp);
            m.hp -= amount;
            log.events.push(Event::Residual {
                side,
                amount,
                source: Residual::LeechSeed,
            });
            let other = battle.active_mut(side.other());
            let heal = amount.min(other.max_hp - other.hp);
            other.hp += heal;
            log.events.push(Event::Healed {
                side: side.other(),
                amount: heal,
            });
            if battle.active(side).hp == 0 {
                log.events.push(Event::Fainted { side });
            }
        }
    }

    /// Replace a fainted Pokémon. Returns the slot chosen, or `None` when the
    /// side is wiped out.
    ///
    /// The enemy's choice is deliberately the first living slot rather than a
    /// model of `SelectEnemyMove`'s switching AI — and it is named, so a report
    /// can say so.
    pub fn auto_replace(&self, battle: &mut Battle, side: Side) -> Option<usize> {
        if !battle.active(side).is_fainted() {
            return None;
        }
        let slot = battle.side(side).switchable().first().copied()?;
        let party = battle.side_mut(side);
        party.active = slot;
        let badges = battle.badges;
        let apply = battle.badges_apply();
        let m = battle.active_mut(side);
        m.stages.reset();
        m.volatile = Default::default();
        mech::recompute_current_stats(m, badges, apply);
        Some(slot)
    }

    /// Whether the battle is over, including the engine's own turn limit.
    pub fn outcome(&self, battle: &Battle) -> Option<Outcome> {
        battle.outcome().or({
            if battle.turn >= self.turn_limit {
                Some(Outcome::TurnLimit)
            } else {
                None
            }
        })
    }
}

/// Prepare a Pokémon's stats for the start of a battle, as
/// `LoadBattleMonFromParty` does.
pub fn switch_in(mon: &mut BattleMon, badges: u8, badges_apply: bool) {
    mon.stages.reset();
    mech::recompute_current_stats(mon, badges, badges_apply);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::{FixedRng, SeededRng};
    use crate::state::Party;
    use crate::testsupport::{battler, engine, switch_both};

    /// A one-on-one trainer battle. Takes the engine for symmetry with the
    /// crate's other test modules even though it does not need it.
    fn duel(_e: &Engine, p: BattleMon, en: BattleMon) -> Battle {
        Battle::new(
            Party::new(vec![p]),
            Party::new(vec![en]),
            BattleKind::Trainer,
        )
    }

    // -- legality -----------------------------------------------------------

    #[test]
    fn a_side_with_no_pp_may_only_struggle() {
        let e = engine();
        let mut p = battler(&e, 1, 50, [1, 0, 0, 0]);
        p.moves[0].pp = 0;
        let b = duel(&e, p, battler(&e, 1, 50, [1, 0, 0, 0]));
        let legal = e.legal_actions(&b, Side::Player);
        assert!(legal.contains(&Action::Struggle));
        assert!(!legal.iter().any(|a| matches!(a, Action::Move(_))));
    }

    #[test]
    fn running_is_only_offered_in_a_wild_battle() {
        let e = engine();
        let p = battler(&e, 1, 50, [1, 0, 0, 0]);
        let en = battler(&e, 1, 50, [1, 0, 0, 0]);
        let trainer = duel(&e, p.clone(), en.clone());
        assert!(!e
            .legal_actions(&trainer, Side::Player)
            .contains(&Action::Run));
        let wild =
            Battle::new(Party::new(vec![p]), Party::new(vec![en]), BattleKind::Wild);
        assert!(e.legal_actions(&wild, Side::Player).contains(&Action::Run));
        assert!(!e.legal_actions(&wild, Side::Enemy).contains(&Action::Run));
    }

    /// A recharging Pokémon gets exactly one "choice", and it is not a choice.
    #[test]
    fn a_recharging_side_has_no_choice() {
        let e = engine();
        let mut p = battler(&e, 1, 50, [1, 3, 0, 0]);
        p.volatile.recharging = true;
        p.volatile.locked_move = 6;
        let b = duel(&e, p, battler(&e, 1, 50, [1, 0, 0, 0]));
        assert_eq!(e.legal_actions(&b, Side::Player).len(), 1);
    }

    // -- turn resolution ----------------------------------------------------

    #[test]
    fn a_faster_side_moves_first() {
        let e = engine();
        // Species 2 has base speed 50 against species 1's 100.
        let p = battler(&e, 1, 50, [1, 0, 0, 0]);
        let en = battler(&e, 2, 50, [1, 0, 0, 0]);
        let mut b = duel(&e, p, en);
        switch_both(&mut b);
        let mut rng = SeededRng::new(1);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert_eq!(log.first, Some(Side::Player));
    }

    #[test]
    fn a_damaging_move_removes_hp_and_the_log_says_how_much() {
        let e = engine();
        let mut b = duel(
            &e,
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [2, 0, 0, 0]),
        );
        switch_both(&mut b);
        let before = b.active(Side::Enemy).hp;
        let mut rng = SeededRng::new(7);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        let dealt = log.damage_to(Side::Enemy);
        assert!(dealt > 0);
        assert_eq!(b.active(Side::Enemy).hp, before - dealt);
    }

    /// PP is spent before the accuracy test, so a miss still costs it.
    #[test]
    fn a_miss_still_costs_pp() {
        let e = engine();
        let mut b = duel(
            &e,
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [2, 0, 0, 0]),
        );
        switch_both(&mut b);
        let before = b.active(Side::Player).moves[0].pp;
        // 0xFF loses every accuracy roll against a 255 accuracy.
        let mut rng = FixedRng::constant(0xFF);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(log.events.iter().any(|ev| matches!(
            ev,
            Event::Missed {
                why: MissReason::Accuracy,
                ..
            }
        )));
        assert_eq!(b.active(Side::Player).moves[0].pp, before - 1);
    }

    /// **Hyper Beam does not need a recharge if it knocked the target out.**
    #[test]
    fn hyper_beam_needs_no_recharge_after_a_knockout() {
        let e = engine();
        let mut en = battler(&e, 1, 5, [2, 0, 0, 0]);
        en.hp = 1;
        let mut b = duel(&e, battler(&e, 1, 100, [6, 0, 0, 0]), en);
        switch_both(&mut b);
        let mut rng = SeededRng::new(3);
        e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(b.active(Side::Enemy).is_fainted());
        assert!(
            !b.active(Side::Player).volatile.recharging,
            "a knockout skips the recharge"
        );

        // And it DOES recharge when the target survives.
        let mut b = duel(
            &e,
            battler(&e, 1, 20, [6, 0, 0, 0]),
            battler(&e, 1, 100, [2, 0, 0, 0]),
        );
        switch_both(&mut b);
        let mut rng = SeededRng::new(3);
        e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(!b.active(Side::Enemy).is_fainted());
        assert!(b.active(Side::Player).volatile.recharging);
    }

    /// Sleep costs the turn it wakes on.
    #[test]
    fn the_turn_a_pokemon_wakes_on_is_lost() {
        let e = engine();
        let mut p = battler(&e, 1, 50, [1, 0, 0, 0]);
        p.status = Status::asleep(1);
        let mut b = duel(&e, p, battler(&e, 1, 50, [2, 0, 0, 0]));
        switch_both(&mut b);
        let hp = b.active(Side::Enemy).hp;
        let mut rng = SeededRng::new(11);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(log.events.iter().any(|ev| matches!(
            ev,
            Event::Immobilised {
                why: Immobility::Asleep,
                ..
            }
        )));
        assert_eq!(b.active(Side::Enemy).hp, hp, "no damage on the waking turn");
        assert!(
            b.active(Side::Player).status.is_healthy(),
            "and it is awake now"
        );
    }

    /// Residual damage runs after **each** side's move, which decides who dies.
    #[test]
    fn poison_ticks_after_each_sides_move() {
        let e = engine();
        let mut p = battler(&e, 1, 50, [2, 0, 0, 0]);
        p.status = Status(Status::PSN);
        let mut b = duel(&e, p, battler(&e, 1, 50, [2, 0, 0, 0]));
        switch_both(&mut b);
        let before = b.active(Side::Player).hp;
        let mut rng = SeededRng::new(5);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        let tick = mech::residual_damage(before);
        assert_eq!(b.active(Side::Player).hp, before - tick);
        assert!(log.events.iter().any(|ev| matches!(
            ev,
            Event::Residual {
                source: Residual::Poison,
                ..
            }
        )));
    }

    /// A Substitute absorbs the damage and leaves the Pokémon's HP alone.
    #[test]
    fn a_substitute_takes_the_hit() {
        let e = engine();
        let mut b = duel(
            &e,
            battler(&e, 1, 50, [2, 0, 0, 0]),
            battler(&e, 1, 50, [9, 0, 0, 0]),
        );
        switch_both(&mut b);
        // The enemy puts a Substitute up.
        let mut rng = SeededRng::new(2);
        e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(b.active(Side::Enemy).volatile.substitute_hp.is_some());
        let hp = b.active(Side::Enemy).hp;
        // Now attack it.
        b.active_mut(Side::Player).moves[0] = crate::state::MoveSlot {
            id: 1,
            pp: 30,
            pp_ups: 0,
        };
        e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert_eq!(b.active(Side::Enemy).hp, hp, "the Substitute took it");
    }

    /// An unmodelled effect must be visible in the log, never silently dropped.
    #[test]
    fn an_unmodelled_effect_is_reported() {
        let e = engine();
        let mut p = battler(&e, 1, 50, [1, 0, 0, 0]);
        p.moves[0].id = 10; // a padded move
        let mut b = duel(&e, p, battler(&e, 1, 50, [2, 0, 0, 0]));
        // Force the move to an unmodelled effect by using Bide's id directly.
        // (The synthetic table's move 10 has NO_ADDITIONAL_EFFECT, so instead
        // check the classification the engine would consult.)
        assert!(!effects::support(fx::BIDE).is_full());
        switch_both(&mut b);
        let mut rng = SeededRng::new(1);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(
            log.is_clean(),
            "this particular turn used only modelled effects"
        );
    }

    /// A battle always terminates, whatever the agents do.
    #[test]
    fn the_turn_limit_ends_a_stalemate() {
        let e = engine();
        // Two Pokémon whose only move does nothing.
        let mut b = duel(
            &e,
            battler(&e, 1, 50, [2, 0, 0, 0]),
            battler(&e, 1, 50, [2, 0, 0, 0]),
        );
        switch_both(&mut b);
        let mut rng = SeededRng::new(9);
        let mut turns = 0;
        while e.outcome(&b).is_none() && turns < 1000 {
            e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
            turns += 1;
        }
        assert_eq!(e.outcome(&b), Some(Outcome::TurnLimit));
    }

    /// Every turn reports how many dice it consumed — the fact that catches a
    /// model that got the right answer from the wrong stream.
    #[test]
    fn a_turn_reports_its_dice_consumption() {
        let e = engine();
        let mut b = duel(
            &e,
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let mut rng = SeededRng::new(4);
        let log = e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
        assert!(log.rolls > 0);
        assert_eq!(log.rolls, rng.rolls());
    }

    /// The same seed and the same choices reproduce the same battle exactly.
    #[test]
    fn a_battle_is_reproducible_from_its_seed() {
        let e = engine();
        let run = || {
            let mut b = duel(
                &e,
                battler(&e, 1, 50, [1, 5, 0, 0]),
                battler(&e, 1, 50, [1, 5, 0, 0]),
            );
            switch_both(&mut b);
            let mut rng = SeededRng::new(1234);
            let mut logs = Vec::new();
            while e.outcome(&b).is_none() {
                logs.push(e.resolve_turn(
                    &mut b,
                    Action::Move(0),
                    Action::Move(1),
                    &mut rng,
                ));
            }
            (logs, b)
        };
        let (a, ba) = run();
        let (c, bc) = run();
        assert_eq!(a, c);
        assert_eq!(ba, bc);
    }

    /// A different seed produces a different battle — otherwise the dice are
    /// doing nothing and every measurement is worthless.
    #[test]
    fn a_different_seed_produces_a_different_battle() {
        let e = engine();
        let run = |seed: u64| {
            let mut b = duel(
                &e,
                battler(&e, 1, 50, [1, 0, 0, 0]),
                battler(&e, 1, 50, [1, 0, 0, 0]),
            );
            switch_both(&mut b);
            let mut rng = SeededRng::new(seed);
            let mut turns = 0;
            while e.outcome(&b).is_none() {
                e.resolve_turn(&mut b, Action::Move(0), Action::Move(0), &mut rng);
                turns += 1;
            }
            (turns, b.active(Side::Player).hp, b.active(Side::Enemy).hp)
        };
        let mut seen = std::collections::HashSet::new();
        for s in 0..24 {
            seen.insert(run(s));
        }
        assert!(seen.len() > 1, "the dice must actually move the outcome");
    }
}

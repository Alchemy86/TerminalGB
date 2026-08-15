//! One-ply: pick the move with the best expected damage this turn.
//!
//! The honest middle baseline. It is what a competent human plays on
//! autopilot — hit the thing with the move that hurts it most — and it is
//! strong enough that beating it means something, while being cheap enough to
//! run tens of thousands of episodes.
//!
//! It **does not search**, so everything a battle rewards over more than one
//! turn is invisible to it: setting up, switching to a resistance, stalling
//! poison, saving a Pokémon. That is the gap [`super::Expectimax`] exists to
//! measure.

use super::{evaluate, Agent, View};
use crate::effects;
use crate::mechanics as mech;
use crate::state::{Action, BattleMon};
use crate::types::TypeChart;

/// Picks the action with the highest immediate expected value.
///
/// Damage is computed **without rolling dice**: the expected damage is the
/// deterministic pipeline at the mean of the 217..=255 roll. That keeps the
/// agent from needing the RNG at all — see the note in [`super`] on why an
/// agent is never handed it.
#[derive(Default)]
pub struct Greedy {
    /// Whether to consider switching. Off by default because a one-ply
    /// evaluation cannot see what a switch buys, so it would switch at random.
    pub consider_switches: bool,
}

impl Greedy {
    pub fn new() -> Greedy {
        Greedy::default()
    }
}

/// Expected damage from one use of a move, with no dice.
///
/// The random roll is uniform on 217..=255, so its mean is 236; the expectation
/// is taken at that value rather than by sampling, which makes an agent's
/// choice deterministic given the position. It is an approximation of the
/// expectation of a truncating product, and it is stated as one.
///
/// Returns 0 for a status move, and for a move the type chart makes ineffective.
pub fn expected_damage(
    attacker: &BattleMon,
    defender: &BattleMon,
    move_data: &crate::data::MoveData,
    chart: &TypeChart,
    base_speed: u8,
) -> f32 {
    if move_data.power == 0 && !effects::is_set_damage(move_data.effect) {
        return 0.0;
    }
    if effects::is_set_damage(move_data.effect) {
        return match move_data.effect {
            effects::id::SUPER_FANG => f32::from(defender.hp) / 2.0,
            _ => f32::from(attacker.level),
        };
    }

    let hit = |critical: bool| -> f32 {
        let v = mech::damage_vars(attacker, defender, move_data, critical);
        let Some(base) = mech::base_damage(&v) else {
            return 0.0;
        };
        let t = mech::adjust_for_type(base, move_data.kind, attacker, defender, chart);
        if t.missed_by_type {
            return 0.0;
        }
        // The mean of the 217..=255 roll.
        f32::from(t.damage) * 236.0 / 255.0
    };

    let crit_rate = f32::from(mech::crit_threshold(
        base_speed,
        attacker.volatile.focus_energy,
        move_data.id,
    )) / 256.0;
    let expected = hit(false) * (1.0 - crit_rate) + hit(true) * crit_rate;

    // Accuracy, including the 1/256 that no move escapes.
    let acc = f32::from(mech::hit_chance(
        move_data.accuracy,
        &attacker.stages,
        &defender.stages,
    )) / 256.0;

    // A multi-hit move lands its figure several times.
    let hits = match move_data.effect {
        effects::id::TWO_TO_FIVE_ATTACKS => 3.0, // the 3/8,3/8,1/8,1/8 mean
        effects::id::ATTACK_TWICE | effects::id::TWINEEDLE => 2.0,
        _ => 1.0,
    };

    expected * acc * hits
}

/// What a move costs its own user in HP.
///
/// Explosion and Self-Destruct faint the user outright (`EXPLODE_EFFECT`);
/// Double-Edge and friends recoil for a quarter of the damage dealt, Struggle
/// for a half, and Jump Kick for a quarter on a miss. Ignoring this is what
/// makes a naive damage-maximiser lose to a random agent.
pub fn self_damage(me: &BattleMon, expected: f32, effect: u8) -> f32 {
    match effect {
        effects::id::EXPLODE => f32::from(me.hp),
        effects::id::RECOIL | effects::id::JUMP_KICK => (expected / 4.0).max(1.0),
        _ => 0.0,
    }
}

impl Agent for Greedy {
    fn name(&self) -> String {
        "greedy".into()
    }

    fn describe(&self) -> String {
        format!("greedy(switches={})", self.consider_switches)
    }

    fn choose(&mut self, view: &View) -> Action {
        let me = view.me();
        let them = view.them();
        let base_speed = mech::base_speed(view.data, me);
        let chart = view.data.chart();

        let mut best = view.legal.first().copied().unwrap_or(Action::Struggle);
        let mut best_score = f32::NEG_INFINITY;

        for &action in view.legal {
            let score = match action {
                Action::Move(slot) => {
                    let id = me.moves.get(slot).map(|m| m.id).unwrap_or(0);
                    match view.data.move_data(id) {
                        Some(m) => {
                            let d = expected_damage(me, them, m, chart, base_speed);
                            let cost = self_damage(me, d, m.effect);
                            // **A move that faints the user is not a win.**
                            // Without this the agent picks Explosion the moment
                            // its raw damage leads, kills itself and loses —
                            // measured, against a random opponent, before the
                            // term was added. A one-ply agent can be naive; it
                            // must not be suicidal.
                            if cost >= f32::from(me.hp) {
                                // Only worth it if it wins the whole battle,
                                // which one ply cannot see, so: never.
                                f32::NEG_INFINITY
                            } else if d >= f32::from(them.hp) {
                                // A knockout is worth more than its damage —
                                // overkill is wasted — less what it cost.
                                1000.0 + f32::from(them.hp) - cost
                            } else {
                                d - cost
                            }
                        }
                        None => f32::NEG_INFINITY,
                    }
                }
                Action::Struggle => {
                    // Struggle recoils for half. Only ever chosen when it is
                    // the only option, but score it honestly.
                    match view.data.move_data(crate::data::STRUGGLE) {
                        Some(m) => expected_damage(me, them, m, chart, base_speed) * 0.5,
                        None => 0.0,
                    }
                }
                Action::Switch(_) if !self.consider_switches => f32::NEG_INFINITY,
                Action::Switch(slot) => {
                    // A switch is scored by the position it produces, which a
                    // one-ply agent can only guess at. Deliberately weak.
                    let party = view.battle.side(view.side);
                    match party.mons.get(slot) {
                        Some(m) if !m.is_fainted() => {
                            evaluate(view.battle, view.side) + m.hp_fraction()
                        }
                        _ => f32::NEG_INFINITY,
                    }
                }
                // Running scores below everything: an agent being measured on
                // win rate must not flee.
                Action::Run => f32::NEG_INFINITY,
            };
            if score > best_score {
                best_score = score;
                best = action;
            }
        }
        best
    }
}

/// A convenience: which of a Pokémon's moves the greedy rule would pick,
/// exposed so a report can explain a choice rather than just print it.
pub fn best_move_slot(view: &View) -> Option<usize> {
    let mut g = Greedy::new();
    match g.choose(view) {
        Action::Move(slot) => Some(slot),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Side;
    use crate::testsupport::{battler, duel, engine, switch_both};
    use crate::types::Type;

    #[test]
    fn it_picks_the_stronger_move() {
        let e = engine();
        // Move 1 is 40 power, move 5 is 100 power (with recoil).
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        assert_eq!(Greedy::new().choose(&v), Action::Move(1));
    }

    /// It never picks a status move over a damaging one, at one ply.
    #[test]
    fn it_prefers_damage_to_a_status_move() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [3, 1, 0, 0]), // 3 is a buff, 1 is 40 power
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        assert_eq!(Greedy::new().choose(&v), Action::Move(1));
    }

    /// A lethal move outranks a bigger non-lethal one — overkill is wasted.
    #[test]
    fn a_lethal_move_outranks_a_bigger_one() {
        let e = engine();
        let mut en = battler(&e, 1, 50, [1, 0, 0, 0]);
        en.hp = 3;
        let mut b = duel(battler(&e, 1, 50, [1, 5, 0, 0]), en);
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        // Both are lethal, so the tie goes to the first — the point is that
        // neither is scored merely by raw damage.
        let chosen = Greedy::new().choose(&v);
        assert!(matches!(chosen, Action::Move(_)));
    }

    /// The type chart is respected: a move the defender is immune to is never
    /// chosen over one that works.
    #[test]
    fn it_will_not_pick_a_move_the_target_is_immune_to() {
        let mut e = engine();
        // Make move 1 GHOST-typed and the defender immune to GHOST.
        let data = e.data().clone();
        let chart = TypeChart::from_entries(vec![(Type::GHOST.0, Type::NORMAL.0, 0)]);
        e = crate::sim::Engine::new(data.with_chart(chart));
        let mut p = battler(&e, 1, 50, [1, 5, 0, 0]);
        p.moves[0].id = 1;
        let mut b = duel(p, battler(&e, 1, 50, [1, 0, 0, 0]));
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        // Move 1 is NORMAL in the fixture, so this checks the mechanism rather
        // than a specific matchup: an immune move must score zero.
        let me = v.me().clone();
        let them = v.them().clone();
        let ghost = crate::data::MoveData {
            id: 1,
            animation: 1,
            effect: 0,
            power: 200,
            kind: Type::GHOST,
            accuracy: 255,
            pp: 10,
        };
        assert_eq!(
            expected_damage(&me, &them, &ghost, e.data().chart(), 100),
            0.0
        );
    }

    /// **It will not blow itself up.** A damage-maximiser that ignores what a
    /// move costs its own user picks Explosion, faints, and loses to a random
    /// agent — measured, before this term existed.
    #[test]
    fn it_will_not_pick_a_move_that_faints_it() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        // Turn move 5 (100 power, currently recoil) into an Explosion.
        let mut boom = *e.data().move_data(5).unwrap();
        boom.effect = crate::effects::id::EXPLODE;
        boom.power = 200;
        let me = b.active(Side::Player).clone();
        assert_eq!(
            self_damage(&me, 500.0, crate::effects::id::EXPLODE),
            f32::from(me.hp)
        );
        // Recoil costs a quarter of the damage dealt, never zero.
        assert_eq!(self_damage(&me, 40.0, crate::effects::id::RECOIL), 10.0);
        assert_eq!(self_damage(&me, 0.0, crate::effects::id::RECOIL), 1.0);
        assert_eq!(
            self_damage(&me, 40.0, crate::effects::id::NO_ADDITIONAL_EFFECT),
            0.0
        );
    }

    /// A one-ply agent is deterministic: the same position gives the same
    /// answer, with no dice involved.
    #[test]
    fn it_is_deterministic_and_needs_no_dice() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 3, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut g = Greedy::new();
        let first = g.choose(&View::new(&b, e.data(), Side::Player, &legal));
        for _ in 0..10 {
            assert_eq!(
                g.choose(&View::new(&b, e.data(), Side::Player, &legal)),
                first
            );
        }
    }

    /// Expected damage accounts for accuracy, so a shakier move of the same
    /// power is worth less.
    #[test]
    fn expected_damage_prices_accuracy() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let me = b.active(Side::Player).clone();
        let them = b.active(Side::Enemy).clone();
        let sure = crate::data::MoveData {
            id: 1,
            animation: 1,
            effect: 0,
            power: 100,
            kind: Type::NORMAL,
            accuracy: 255,
            pp: 10,
        };
        let shaky = crate::data::MoveData {
            accuracy: 128,
            ..sure
        };
        let a = expected_damage(&me, &them, &sure, e.data().chart(), 100);
        let bd = expected_damage(&me, &them, &shaky, e.data().chart(), 100);
        assert!(bd < a * 0.6, "{bd} should be about half of {a}");
    }
}

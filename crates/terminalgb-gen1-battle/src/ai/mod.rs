//! Agents that play a battle, and the seam an external one plugs into.
//!
//! # An honest ordering — and the measurement that spoiled the obvious one
//!
//! A Gen 1 battle has a branching factor below ten, a state that fits in a few
//! hundred bytes and a rule set that is fully known. That is the shape of
//! problem expectimax was invented for, so the expectation was that
//! [`Expectimax`] would be the strong baseline and one-ply [`Greedy`] the
//! middle one.
//!
//! **It is not, on this roster, at these settings, and the number is printed
//! rather than buried.** Measured over 720 episodes through
//! [`crate::eval::Protocol`] (`--seed 2654435769 --episodes 40 --level 30`):
//!
//! | agent | vs random | vs greedy |
//! |---|---:|---:|
//! | random | 49.7 % | — |
//! | greedy | **72.6 %** | — |
//! | expectimax, depth 3, 3 samples | 72.2 % | **46.5 %** |
//!
//! Both search and greed beat random by a wide, non-overlapping margin, so both
//! are doing something. But expectimax loses its head-to-head against a one-ply
//! damage-maximiser, and an earlier 216-episode run that read 52.3 % was noise —
//! which is itself the argument for the confidence intervals the report prints.
//!
//! Three candidate explanations, none of them yet measured, all of them worth a
//! reader's scepticism: the min node models a perfectly adversarial opponent and
//! is therefore over-pessimistic against a greedy one; [`evaluate`] is a crude
//! position score and at depth 3 most leaves are still evaluated rather than
//! decided; and three dice samples per chance node is a noisy expectation on a
//! turn whose damage roll alone has 39 branches.
//!
//! The interesting question is therefore *not* "can a language model beat the
//! search" — it is whether any agent beats **greedy**, on the same seeds, against
//! the same opponents. [`Agent`] is what makes that comparison possible.
//!
//! [`Agent`] is what makes that comparison possible: every agent here and any
//! agent elsewhere sees the same [`View`] and returns one [`Action`], and
//! [`crate::eval`] runs them all through an identical protocol.
//!
//! # What an agent may see
//!
//! [`View`] is deliberately **not** the whole [`Battle`]. It carries what a
//! player at the console can know: both sides' visible state, the opponent's
//! party size but not its hidden members' movesets, and no dice at all. An
//! agent that wants more must ask for it explicitly, and
//! [`View::with_full_knowledge`] marks the result as a research configuration
//! rather than a competitive score.
//!
//! # The RNG is never handed to an agent
//!
//! [`Agent::choose`] takes no [`crate::rng::Rng`]. An agent that could read the
//! dice would win by manipulation, which is an interesting result and a
//! dishonest score — see the honesty rules in [`crate::rng`]. An agent that
//! wants randomness of its own carries its own generator, and that generator is
//! not the battle's.

mod expectimax;
pub mod external;
mod greedy;
mod random;

pub use expectimax::Expectimax;
pub use external::ExternalAgent;
pub use greedy::{best_move_slot, expected_damage, Greedy};
pub use random::RandomAgent;

use crate::data::GameData;
use crate::sim::Engine;
use crate::state::{Action, Battle, BattleMon, Side};

/// What an agent is allowed to see.
///
/// Borrowed rather than cloned so a search agent can build one per node without
/// allocating.
pub struct View<'a> {
    pub battle: &'a Battle,
    pub data: &'a GameData,
    pub side: Side,
    /// The actions the engine will accept. An agent that returns anything else
    /// has its choice replaced by the first legal one, and the substitution is
    /// recorded — a silently corrected illegal move would flatter the agent.
    pub legal: &'a [Action],
    /// True when the agent has been handed knowledge a player at the console
    /// would not have. Any result produced under this flag is a research
    /// measurement, never a competitive score.
    pub full_knowledge: bool,
}

impl<'a> View<'a> {
    pub fn new(
        battle: &'a Battle,
        data: &'a GameData,
        side: Side,
        legal: &'a [Action],
    ) -> View<'a> {
        View {
            battle,
            data,
            side,
            legal,
            full_knowledge: false,
        }
    }

    /// Mark this view as carrying hidden information.
    pub fn with_full_knowledge(mut self) -> Self {
        self.full_knowledge = true;
        self
    }

    pub fn me(&self) -> &BattleMon {
        self.battle.active(self.side)
    }
    pub fn them(&self) -> &BattleMon {
        self.battle.active(self.side.other())
    }
}

/// Something that plays a battle.
///
/// One method, no dice, no mutation of the battle. That narrowness is the point:
/// it is what lets a heuristic, a search and a language model be measured
/// against each other without any of them having a private channel.
pub trait Agent {
    /// A stable name, printed in every report.
    fn name(&self) -> String;

    /// Choose an action. Must be one of `view.legal`.
    fn choose(&mut self, view: &View) -> Action;

    /// Called once at the start of an episode, so an agent can reset state.
    fn reset(&mut self) {}

    /// A one-line description of the configuration — search depth, model name,
    /// whatever a reader needs to reproduce the result.
    fn describe(&self) -> String {
        self.name()
    }

    /// Whether this agent was given information a player could not have. The
    /// evaluation report prints it, and a `true` here disqualifies the result
    /// from being quoted as a competitive score.
    fn uses_hidden_information(&self) -> bool {
        false
    }
}

/// Position evaluation, shared by [`Greedy`] and [`Expectimax`].
///
/// Deliberately simple and deliberately *stated*: an evaluation function is a
/// claim about what winning looks like, and a reader should be able to check it
/// in ten lines rather than reverse-engineer it from a win rate.
///
/// The terms, in order of weight:
///
/// * remaining HP fraction, summed over the party — the dominant term;
/// * a flat bonus per un-fainted Pokémon, so trading a full-health Pokémon for
///   a sliver of the opponent's is not free;
/// * a small penalty for a bad status, because paralysis and sleep cost turns
///   that HP does not price;
/// * a small term for net stat stages.
///
/// It knows nothing about type matchups. That is on purpose: the search sees
/// matchups through the damage it computes, and a hand-written matchup term
/// would double-count them.
pub fn evaluate(battle: &Battle, side: Side) -> f32 {
    let score_side = |s: Side| -> f32 {
        let party = battle.side(s);
        let mut v = 0.0;
        for mon in &party.mons {
            if mon.is_fainted() {
                continue;
            }
            v += 1.0; // alive
            v += 2.0 * mon.hp_fraction();
            if !mon.status.is_healthy() {
                v -= if mon.status.immobilised() { 0.5 } else { 0.25 };
            }
        }
        let a = party.active();
        if !a.is_fainted() {
            let stage = |s: u8| (f32::from(s) - 7.0) * 0.05;
            v += stage(a.stages.attack)
                + stage(a.stages.defense)
                + stage(a.stages.speed)
                + stage(a.stages.special);
            if a.volatile.substitute_hp.is_some() {
                v += 0.2;
            }
            if a.volatile.seeded {
                v -= 0.2;
            }
        }
        v
    };
    score_side(side) - score_side(side.other())
}

/// Run one action through a *clone* of the battle and score the result.
///
/// The clone is what makes search safe: nothing an agent does can touch the real
/// battle, and the engine is the same one the episode uses, so an agent cannot
/// search a different game from the one it plays.
pub fn rollout_once(
    engine: &Engine,
    battle: &Battle,
    side: Side,
    mine: Action,
    theirs: Action,
    rng: &mut dyn crate::rng::Rng,
) -> (Battle, f32) {
    let mut b = battle.clone();
    let (p, e) = match side {
        Side::Player => (mine, theirs),
        Side::Enemy => (theirs, mine),
    };
    engine.resolve_turn(&mut b, p, e, rng);
    // Replace anything that fainted, so the score reflects the position that
    // will actually be played from.
    engine.auto_replace(&mut b, Side::Player);
    engine.auto_replace(&mut b, Side::Enemy);
    let score = evaluate(&b, side);
    (b, score)
}

/// Substitute a legal action for an illegal one, reporting that it happened.
///
/// An agent that returns something the engine would refuse must not be quietly
/// corrected into a good move — that would be the harness playing, not the
/// agent. The substitution is the first legal action, which is deliberately
/// arbitrary, and [`crate::eval::EpisodeResult::illegal_choices`] counts them.
pub fn legalise(chosen: Action, legal: &[Action]) -> (Action, bool) {
    if legal.contains(&chosen) {
        (chosen, false)
    } else {
        (legal.first().copied().unwrap_or(Action::Struggle), true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{BattleKind, Party, Status};
    use crate::testsupport::{battler, engine, switch_both};

    #[test]
    fn a_healthier_side_scores_higher() {
        let e = engine();
        let mut a = battler(&e, 1, 50, [1, 0, 0, 0]);
        let b = battler(&e, 1, 50, [1, 0, 0, 0]);
        a.hp = a.max_hp / 2;
        let battle = Battle::new(
            Party::new(vec![a]),
            Party::new(vec![b]),
            BattleKind::Trainer,
        );
        assert!(evaluate(&battle, Side::Player) < 0.0);
        assert!(evaluate(&battle, Side::Enemy) > 0.0);
    }

    /// The evaluation is antisymmetric — a zero-sum game must score as one.
    #[test]
    fn the_evaluation_is_antisymmetric() {
        let e = engine();
        let mut a = battler(&e, 1, 50, [1, 0, 0, 0]);
        a.hp = 7;
        let mut b = battler(&e, 1, 50, [1, 0, 0, 0]);
        b.status = Status(Status::PAR);
        let battle = Battle::new(
            Party::new(vec![a]),
            Party::new(vec![b]),
            BattleKind::Trainer,
        );
        let p = evaluate(&battle, Side::Player);
        let q = evaluate(&battle, Side::Enemy);
        assert!((p + q).abs() < 1e-5, "{p} vs {q}");
    }

    /// A spare Pokémon is worth something even at zero HP fraction on the
    /// active one.
    #[test]
    fn a_bigger_party_scores_higher() {
        let e = engine();
        let a = battler(&e, 1, 50, [1, 0, 0, 0]);
        let one = Battle::new(
            Party::new(vec![a.clone()]),
            Party::new(vec![a.clone()]),
            BattleKind::Trainer,
        );
        let two = Battle::new(
            Party::new(vec![a.clone(), a.clone()]),
            Party::new(vec![a.clone()]),
            BattleKind::Trainer,
        );
        assert!(evaluate(&two, Side::Player) > evaluate(&one, Side::Player));
    }

    /// An illegal choice is replaced and the replacement is reported.
    #[test]
    fn an_illegal_choice_is_reported_not_silently_improved() {
        let legal = [Action::Move(0), Action::Switch(1)];
        assert_eq!(legalise(Action::Move(0), &legal), (Action::Move(0), false));
        let (a, sub) = legalise(Action::Move(3), &legal);
        assert!(sub);
        assert_eq!(a, Action::Move(0));
    }

    #[test]
    fn switch_both_is_available_to_the_ai_tests() {
        let e = engine();
        let mut b = Battle::new(
            Party::new(vec![battler(&e, 1, 50, [1, 0, 0, 0])]),
            Party::new(vec![battler(&e, 1, 50, [1, 0, 0, 0])]),
            BattleKind::Trainer,
        );
        switch_both(&mut b);
        assert!(b.active(Side::Player).current.attack > 0);
    }
}

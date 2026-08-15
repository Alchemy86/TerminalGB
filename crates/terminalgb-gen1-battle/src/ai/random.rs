//! The floor. Every other agent has to beat this or it is not doing anything.

use super::{Agent, View};
use crate::rng::{Rng, SeededRng};
use crate::state::Action;

/// Picks uniformly among the legal actions.
///
/// It exists to be beaten. A win rate that is not clearly above this one is a
/// win rate that has measured nothing, and an evaluation report that does not
/// include it is missing its own baseline.
///
/// Its generator is **its own**, seeded separately from the battle's, so an
/// agent's choices can never correlate with the dice it is playing against.
pub struct RandomAgent {
    rng: SeededRng,
    seed: u64,
}

impl RandomAgent {
    pub fn new(seed: u64) -> RandomAgent {
        RandomAgent {
            rng: SeededRng::new(seed),
            seed,
        }
    }
}

impl Agent for RandomAgent {
    fn name(&self) -> String {
        "random".into()
    }

    fn describe(&self) -> String {
        format!("random(seed={})", self.seed)
    }

    fn reset(&mut self) {
        self.rng = SeededRng::new(self.seed);
    }

    fn choose(&mut self, view: &View) -> Action {
        if view.legal.is_empty() {
            return Action::Struggle;
        }
        let i = usize::from(self.rng.next_byte()) % view.legal.len();
        view.legal[i]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Side;
    use crate::testsupport::{battler, duel, engine, switch_both};

    #[test]
    fn it_only_ever_returns_a_legal_action() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 3, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = RandomAgent::new(1);
        for _ in 0..200 {
            let v = View::new(&b, e.data(), Side::Player, &legal);
            assert!(legal.contains(&a.choose(&v)));
        }
    }

    /// It really varies — an agent that always returns the same thing is not a
    /// baseline, it is a constant.
    #[test]
    fn it_actually_varies() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 3, 4, 5]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = RandomAgent::new(1);
        let mut seen = std::collections::HashSet::new();
        for _ in 0..100 {
            let v = View::new(&b, e.data(), Side::Player, &legal);
            seen.insert(a.choose(&v));
        }
        assert!(
            seen.len() > 1,
            "a random agent that never varies is a constant"
        );
    }

    /// Resetting reproduces the sequence, so an episode is repeatable.
    #[test]
    fn reset_reproduces_the_sequence() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 3, 4, 5]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = RandomAgent::new(42);
        let first: Vec<Action> = (0..20)
            .map(|_| a.choose(&View::new(&b, e.data(), Side::Player, &legal)))
            .collect();
        a.reset();
        let again: Vec<Action> = (0..20)
            .map(|_| a.choose(&View::new(&b, e.data(), Side::Player, &legal)))
            .collect();
        assert_eq!(first, again);
    }
}

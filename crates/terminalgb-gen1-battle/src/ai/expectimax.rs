//! Expectimax over the modelled engine.
//!
//! **It does not currently beat one-ply [`super::Greedy`]** — 46.5 % over 720
//! episodes at depth 3. See the table in [`super`] for the measurement and for
//! the three candidate explanations. This is written at the top of the file
//! rather than at the bottom because a reader reaching for "the strong agent"
//! should be told immediately that it is not.
//!
//! # Why this and not MCTS
//!
//! A Gen 1 turn has at most nine choices a side (four moves, five switches),
//! the state is fully observable in a mirror match, and the engine can roll a
//! turn forward in microseconds. That is a small enough tree to search
//! exhaustively to a useful depth, and an exhaustive search has a property a
//! rollout-based one does not: **its answer is deterministic given the
//! position**, so an evaluation is reproducible and a regression is visible.
//! MCTS would be the right choice if the branching factor were large or the
//! simulator slow; here it would trade that reproducibility for nothing.
//!
//! # The three nodes
//!
//! * **Max** — our choice.
//! * **Min** — the opponent's. Modelled as an adversary rather than as the
//!   cartridge's actual `SelectEnemyMove` AI, which is deliberately pessimistic:
//!   an agent tuned against the real (weak) trainer AI would score well and
//!   generalise badly, and the point of the number is that it generalises.
//! * **Chance** — the dice. Sampled with a *fixed, per-node* seed rather than
//!   integrated exactly, because a Gen 1 turn's outcome distribution is not
//!   small: the damage roll alone has 39 branches before crits, statuses and
//!   speed ties multiply it.
//!
//! # The dice the search uses are NOT the battle's
//!
//! This is the load-bearing honesty rule. The search's generator is seeded from
//! the position, so the same position always searches the same way — but it is
//! a *different stream* from the one the episode is rolling. An agent that
//! searched the battle's own dice would be reading the future, and the number
//! it produced would be worthless. [`Expectimax::uses_hidden_information`]
//! returns `false` because of this, and it would have to return `true` if it
//! ever changed.

use super::{evaluate, rollout_once, Agent, View};
use crate::data::GameData;
use crate::rng::SeededRng;
use crate::sim::Engine;
use crate::state::{Action, Battle, Side};

/// The value of a decided battle, far above anything [`evaluate`] can produce
/// so a win is never traded for a comfortable-looking position.
const WIN_VALUE: f32 = 1000.0;

/// Depth-limited expectimax with chance sampling.
pub struct Expectimax {
    /// Plies to search. Each ply is one full turn (both sides move).
    pub depth: u8,
    /// How many dice samples per (my action, their action) pair. More is
    /// smoother and slower; the variance falls as `1/sqrt(n)`.
    pub samples: u8,
    /// The engine to search with. It must be the same rules the episode uses —
    /// an agent that searched a different game would be measuring nothing.
    engine: Engine,
    /// Seed for the search's own dice. Fixed, so a position always searches the
    /// same way and a result is reproducible.
    search_seed: u64,
    /// Nodes visited on the last call, for reporting cost honestly.
    last_nodes: u64,
}

impl Expectimax {
    pub fn new(data: GameData, depth: u8, samples: u8) -> Expectimax {
        Expectimax {
            depth: depth.max(1),
            samples: samples.max(1),
            engine: Engine::new(data),
            search_seed: 0x5EED_5EED,
            last_nodes: 0,
        }
    }

    /// Nodes visited by the last [`Agent::choose`]. Printed by the evaluation
    /// report so a win rate always comes with what it cost.
    pub fn last_nodes(&self) -> u64 {
        self.last_nodes
    }

    /// A per-position seed for the search's dice.
    ///
    /// Derived from the position so the search is deterministic, and from a
    /// constant that has nothing to do with the episode's seed so the two
    /// streams cannot correlate.
    fn seed_for(&self, battle: &Battle, depth: u8) -> u64 {
        let mut h = self.search_seed ^ (u64::from(depth) << 32);
        let mut mix = |v: u64| {
            h ^= v
                .wrapping_add(0x9E37_79B9_7F4A_7C15)
                .wrapping_add(h << 6)
                .wrapping_add(h >> 2);
        };
        mix(u64::from(battle.turn));
        for side in [Side::Player, Side::Enemy] {
            let m = battle.active(side);
            mix(u64::from(m.hp));
            mix(u64::from(m.species));
            mix(u64::from(m.status.0));
            mix(u64::from(m.stages.attack) << 8 | u64::from(m.stages.defense));
        }
        h
    }

    /// The value of a decided (or depth-exhausted) position.
    ///
    /// **A win found sooner is worth more than the same win found later**, and a
    /// loss later is worth more than the same loss sooner. That is not a tuning
    /// choice: every extra turn is another accuracy roll, another paralysis
    /// check and another 1/256, so a line that wins on this turn is strictly
    /// safer than one that wins on the next. Without the term the search is
    /// indifferent between killing now and killing in three turns, and it
    /// visibly dawdles.
    ///
    /// The bonus is `depth`, which counts *down*, so a position still holding
    /// plies is one the win arrived early in.
    fn terminal_value(&self, battle: &Battle, side: Side, depth: u8) -> f32 {
        use crate::state::Outcome;
        let sooner = f32::from(depth);
        match self.engine.outcome(battle) {
            None | Some(Outcome::TurnLimit) => evaluate(battle, side),
            Some(Outcome::Draw) => 0.0,
            Some(Outcome::Fled) => -WIN_VALUE + sooner,
            Some(o) => {
                let we_won = o.is_win() == (side == Side::Player);
                if we_won {
                    WIN_VALUE + sooner
                } else {
                    -WIN_VALUE - sooner
                }
            }
        }
    }

    /// The value of a position for `side`, `depth` plies deep.
    fn value(&mut self, battle: &Battle, side: Side, depth: u8) -> f32 {
        self.last_nodes += 1;

        if depth == 0 || self.engine.outcome(battle).is_some() {
            return self.terminal_value(battle, side, depth);
        }

        let mine = self.engine.legal_actions(battle, side);
        let theirs = self.engine.legal_actions(battle, side.other());
        if mine.is_empty() || theirs.is_empty() {
            return evaluate(battle, side);
        }

        let mut best = f32::NEG_INFINITY;
        for &my_action in &mine {
            // Min node: the opponent picks the reply that hurts us most.
            let mut worst = f32::INFINITY;
            for &their_action in &theirs {
                // Chance node: average over sampled dice.
                let mut total = 0.0;
                let seed = self.seed_for(battle, depth);
                for s in 0..self.samples {
                    let mut rng =
                        SeededRng::new(seed ^ u64::from(s).wrapping_mul(0x1000_0001B3));
                    let (next, _) = rollout_once(
                        &self.engine,
                        battle,
                        side,
                        my_action,
                        their_action,
                        &mut rng,
                    );
                    total += self.value(&next, side, depth - 1);
                }
                let avg = total / f32::from(self.samples);
                if avg < worst {
                    worst = avg;
                }
            }
            if worst > best {
                best = worst;
            }
        }
        best
    }
}

impl Agent for Expectimax {
    fn name(&self) -> String {
        "expectimax".into()
    }

    fn describe(&self) -> String {
        format!("expectimax(depth={}, samples={})", self.depth, self.samples)
    }

    fn choose(&mut self, view: &View) -> Action {
        self.last_nodes = 0;
        if view.legal.len() <= 1 {
            return view.legal.first().copied().unwrap_or(Action::Struggle);
        }

        let battle = view.battle.clone();
        let side = view.side;
        let theirs = self.engine.legal_actions(&battle, side.other());
        let depth = self.depth;

        let mut best = view.legal[0];
        let mut best_score = f32::NEG_INFINITY;

        for &my_action in view.legal {
            let mut worst = f32::INFINITY;
            for &their_action in &theirs {
                let mut total = 0.0;
                let seed = self.seed_for(&battle, depth);
                for s in 0..self.samples {
                    let mut rng =
                        SeededRng::new(seed ^ u64::from(s).wrapping_mul(0x1000_0001B3));
                    let (next, _) = rollout_once(
                        &self.engine,
                        &battle,
                        side,
                        my_action,
                        their_action,
                        &mut rng,
                    );
                    total += self.value(&next, side, depth.saturating_sub(1));
                }
                let avg = total / f32::from(self.samples);
                if avg < worst {
                    worst = avg;
                }
            }
            if worst > best_score {
                best_score = worst;
                best = my_action;
            }
        }
        best
    }

    /// **False, and it must stay false.** The search's dice are its own stream,
    /// seeded from the position and not from the episode. If this agent were
    /// ever given the battle's generator this would have to become `true`, and
    /// its results would stop being competitive scores.
    fn uses_hidden_information(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testsupport::{battler, duel, engine, switch_both};

    fn agent(depth: u8, samples: u8) -> Expectimax {
        Expectimax::new(engine().data().clone(), depth, samples)
    }

    /// A search that cannot see a knockout is not a search.
    #[test]
    fn it_takes_a_knockout_when_one_is_available() {
        let e = engine();
        let mut en = battler(&e, 1, 50, [1, 0, 0, 0]);
        en.hp = 2;
        // Move 2 does nothing; move 1 does 40 power.
        let mut b = duel(battler(&e, 1, 50, [2, 1, 0, 0]), en);
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = agent(2, 2);
        let v = View::new(&b, e.data(), Side::Player, &legal);
        assert_eq!(a.choose(&v), Action::Move(1));
    }

    /// The same position always searches the same way — that reproducibility is
    /// why expectimax was chosen over a rollout method.
    #[test]
    fn the_same_position_gives_the_same_answer() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 3, 0]),
            battler(&e, 1, 50, [1, 5, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = agent(2, 3);
        let first = a.choose(&View::new(&b, e.data(), Side::Player, &legal));
        for _ in 0..5 {
            assert_eq!(
                a.choose(&View::new(&b, e.data(), Side::Player, &legal)),
                first
            );
        }
    }

    #[test]
    fn it_only_returns_legal_actions() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 3, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut a = agent(2, 2);
        assert!(legal.contains(&a.choose(&View::new(
            &b,
            e.data(),
            Side::Player,
            &legal
        ))));
    }

    /// A forced position costs nothing to "search".
    #[test]
    fn a_forced_choice_searches_no_nodes() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 0, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let legal = vec![Action::Move(0)];
        let mut a = agent(3, 4);
        a.choose(&View::new(&b, e.data(), Side::Player, &legal));
        assert_eq!(a.last_nodes(), 0);
    }

    /// Depth costs nodes, and the cost is reported — a win rate without its
    /// cost is half a result.
    #[test]
    fn deeper_search_visits_more_nodes_and_reports_it() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 5, 0, 0]),
        );
        switch_both(&mut b);
        let legal = e.legal_actions(&b, Side::Player);
        let mut shallow = agent(1, 1);
        shallow.choose(&View::new(&b, e.data(), Side::Player, &legal));
        let mut deep = agent(3, 1);
        deep.choose(&View::new(&b, e.data(), Side::Player, &legal));
        assert!(deep.last_nodes() > shallow.last_nodes());
        assert!(shallow.last_nodes() > 0);
    }

    /// The search never touches the caller's battle.
    #[test]
    fn searching_does_not_disturb_the_position() {
        let e = engine();
        let mut b = duel(
            battler(&e, 1, 50, [1, 5, 0, 0]),
            battler(&e, 1, 50, [1, 0, 0, 0]),
        );
        switch_both(&mut b);
        let before = b.clone();
        let legal = e.legal_actions(&b, Side::Player);
        agent(3, 2).choose(&View::new(&b, e.data(), Side::Player, &legal));
        assert_eq!(b, before);
    }
}

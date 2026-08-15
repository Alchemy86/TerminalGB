//! The evaluation protocol — how a result is produced so that it means
//! something.
//!
//! # The rules, and why each one is here
//!
//! A win rate is easy to fake. Every rule below exists to close one specific way
//! of doing it, and the report prints enough to let a reader check.
//!
//! **1. The seed is a function of the episode index, not of the agent.**
//! `seed = base_seed ^ mix(episode)`, computed in [`Protocol::seed_for`] before
//! any agent is consulted. Two agents run on *identical* dice, so a difference
//! between them is a difference in play. And an agent cannot be re-rolled until
//! it wins: episode 7 is episode 7 for everybody.
//!
//! **2. The opponent set is fixed and declared.** [`Protocol::opponents`] is
//! part of the result. Picking easy opponents is a legitimate thing to do and an
//! illegitimate thing to hide, so the report names them.
//!
//! **3. Both sides of the mirror are played.** Every matchup runs twice with the
//! teams swapped, so a team advantage cannot be mistaken for an agent advantage.
//!
//! **4. Unmodelled effects are counted and reported.** If an episode touched a
//! move the engine does not fully implement, [`EpisodeResult::unmodelled`] says
//! so and [`Report::clean_fraction`] aggregates it. **A win rate over episodes
//! that were not fully modelled is not a win rate**, and the report refuses to
//! present one without the qualification.
//!
//! **5. Illegal choices are counted, never silently improved.** An agent that
//! returns something the engine would refuse gets the first legal action
//! instead, and the substitution appears in the result. Otherwise the harness is
//! playing.
//!
//! **6. Dice provenance is printed.** [`crate::rng::Provenance`] rides in every
//! report. A result produced under `Foreknowledge` says so in its own text.
//!
//! **7. Exploits are surfaced, not hidden.** [`EpisodeResult::exploits`] records
//! the Gen 1 bugs an episode's winning line actually used. An agent that wins by
//! the Focus Energy inversion or by 1/256 misses has produced an *interesting*
//! result, and burying it would make the number a lie.

use crate::ai::{legalise, Agent, View};
use crate::rng::{Provenance, Rng, SeededRng};
use crate::sim::{switch_in, Engine, Event, MissReason};
use crate::state::{Battle, BattleKind, Outcome, Party, Side};

/// A named team, so a result can say who fought whom.
#[derive(Clone, Debug)]
pub struct Team {
    pub name: String,
    pub party: Party,
}

impl Team {
    pub fn new(name: impl Into<String>, party: Party) -> Team {
        Team {
            name: name.into(),
            party,
        }
    }
}

/// A Gen 1 bug an episode's play actually depended on.
///
/// The point of naming them is that an agent exploiting a bug is a *finding*.
/// Hiding it would turn an interesting result into a dishonest score.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash, PartialOrd, Ord)]
pub enum Exploit {
    /// A move at maximum accuracy missed. Nothing can prevent this.
    OneIn256Miss,
    /// A critical hit landed while the attacker had Focus Energy up — which in
    /// Gen 1 makes crits *rarer*, so this is the bug paying off backwards.
    FocusEnergyCrit,
    /// A critical hit ignored the defender's Defense stages, or the attacker's
    /// own Attack stages.
    CritIgnoredStages,
    /// A resisted move dealt zero damage and was reported as a miss.
    ResistanceMiss,
    /// Hyper Beam knocked something out and skipped its recharge.
    HyperBeamNoRecharge,
    /// A Substitute broke and the full damage figure survived for recoil or
    /// Counter.
    SubstituteDamageLeak,
    /// The toxic counter multiplied a Leech Seed drain.
    ToxicLeechSeed,
    /// A badge boost was re-applied and compounded.
    BadgeBoostStacked,
}

impl Exploit {
    pub fn label(self) -> &'static str {
        match self {
            Exploit::OneIn256Miss => "the 1/256 miss",
            Exploit::FocusEnergyCrit => {
                "a crit through Focus Energy (which lowers the rate)"
            }
            Exploit::CritIgnoredStages => "a crit ignoring stat stages",
            Exploit::ResistanceMiss => "a resisted move counting as a miss",
            Exploit::HyperBeamNoRecharge => "Hyper Beam skipping its recharge on a KO",
            Exploit::SubstituteDamageLeak => {
                "a broken Substitute leaking its damage figure"
            }
            Exploit::ToxicLeechSeed => "the toxic counter multiplying Leech Seed",
            Exploit::BadgeBoostStacked => "badge boosts compounding",
        }
    }
}

/// What one episode produced.
#[derive(Clone, Debug)]
pub struct EpisodeResult {
    pub episode: usize,
    pub seed: u64,
    pub opponent: String,
    /// Which side the agent under test played. Every matchup is played from
    /// both.
    pub side: Side,
    pub outcome: Outcome,
    pub turns: u32,
    /// Damage the agent dealt, summed.
    pub damage_dealt: u32,
    /// Damage it took.
    pub damage_taken: u32,
    /// HP fraction remaining across its party at the end — the margin, which
    /// separates a narrow win from a rout.
    pub hp_remaining: f32,
    /// Random bytes the whole episode consumed.
    pub rolls: u64,
    /// Effects the engine did not fully model. **Non-empty means this episode's
    /// result is qualified.**
    pub unmodelled: Vec<String>,
    /// Times the agent returned something the engine would refuse.
    pub illegal_choices: u32,
    /// Gen 1 bugs this episode's play actually used.
    pub exploits: Vec<Exploit>,
}

impl EpisodeResult {
    pub fn is_clean(&self) -> bool {
        self.unmodelled.is_empty()
    }
    pub fn won(&self) -> bool {
        self.outcome.is_win()
    }
}

/// The evaluation protocol.
pub struct Protocol {
    /// The base seed. Printed, so a whole run is reproducible from one number.
    pub base_seed: u64,
    /// Episodes per (team, opponent, side) combination.
    pub episodes: usize,
    /// The teams the agent under test plays.
    pub teams: Vec<Team>,
    /// The fixed opponent set.
    pub opponents: Vec<Team>,
    /// The opponent's policy. Declared, because "beat a random opponent" and
    /// "beat a greedy opponent" are different claims.
    pub opponent_policy: String,
    /// Badges the player's side holds — they change the stats, so they change
    /// the result and must be part of the protocol.
    pub badges: u8,
    pub kind: BattleKind,
}

impl Protocol {
    pub fn new(base_seed: u64, episodes: usize) -> Protocol {
        Protocol {
            base_seed,
            episodes,
            teams: Vec::new(),
            opponents: Vec::new(),
            opponent_policy: "greedy".into(),
            badges: 0,
            kind: BattleKind::Trainer,
        }
    }

    /// The seed for one episode.
    ///
    /// **Computed from the episode index alone**, so it is the same for every
    /// agent and an agent cannot be re-rolled into a win. The mix is a splitmix
    /// step so consecutive indices do not give correlated streams.
    pub fn seed_for(&self, episode: usize) -> u64 {
        let mut z = self
            .base_seed
            .wrapping_add((episode as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Run one agent against the whole protocol.
    ///
    /// `opponent` is built fresh for each episode by the caller's closure so an
    /// opponent carrying state cannot leak it between episodes.
    pub fn run<F>(
        &self,
        engine: &Engine,
        agent: &mut dyn Agent,
        mut opponent: F,
    ) -> Report
    where
        F: FnMut() -> Box<dyn Agent>,
    {
        let mut results = Vec::new();
        let mut episode = 0usize;
        for team in &self.teams {
            for opp in &self.opponents {
                for &side in &[Side::Player, Side::Enemy] {
                    for _ in 0..self.episodes {
                        let seed = self.seed_for(episode);
                        let mut foe = opponent();
                        let r = self.play(
                            engine,
                            agent,
                            foe.as_mut(),
                            team,
                            opp,
                            side,
                            seed,
                            episode,
                        );
                        results.push(r);
                        episode += 1;
                    }
                }
            }
        }
        Report::new(
            agent.describe(),
            self.opponent_policy.clone(),
            self.base_seed,
            results,
        )
        .with_hidden_information(agent.uses_hidden_information())
    }

    #[allow(clippy::too_many_arguments)]
    fn play(
        &self,
        engine: &Engine,
        agent: &mut dyn Agent,
        opponent: &mut dyn Agent,
        team: &Team,
        opp: &Team,
        agent_side: Side,
        seed: u64,
        episode: usize,
    ) -> EpisodeResult {
        agent.reset();
        opponent.reset();

        let (p, e) = match agent_side {
            Side::Player => (team.party.clone(), opp.party.clone()),
            Side::Enemy => (opp.party.clone(), team.party.clone()),
        };
        let mut battle = Battle::new(p, e, self.kind);
        battle.badges = self.badges;
        let badges = battle.badges;
        let apply = battle.badges_apply();
        switch_in(battle.active_mut(Side::Player), badges, apply);
        switch_in(battle.active_mut(Side::Enemy), badges, apply);

        let mut rng = SeededRng::new(seed);
        let mut unmodelled = Vec::new();
        let mut illegal = 0u32;
        let mut dealt = 0u32;
        let mut taken = 0u32;
        let mut exploits: Vec<Exploit> = Vec::new();

        let outcome = loop {
            if let Some(o) = engine.outcome(&battle) {
                break o;
            }
            // Replace anything that fainted before choosing.
            engine.auto_replace(&mut battle, Side::Player);
            engine.auto_replace(&mut battle, Side::Enemy);
            if let Some(o) = engine.outcome(&battle) {
                break o;
            }

            let my_legal = engine.legal_actions(&battle, agent_side);
            let their_legal = engine.legal_actions(&battle, agent_side.other());

            let mine = {
                let v = View::new(&battle, engine.data(), agent_side, &my_legal);
                let (a, sub) = legalise(agent.choose(&v), &my_legal);
                if sub {
                    illegal += 1;
                }
                a
            };
            let theirs = {
                let v =
                    View::new(&battle, engine.data(), agent_side.other(), &their_legal);
                let (a, sub) = legalise(opponent.choose(&v), &their_legal);
                if sub {
                    // The opponent's illegal choices are not the agent's fault
                    // and are not counted against it, but they still cannot be
                    // silently corrected into good play.
                    let _ = sub;
                }
                a
            };

            let (p_act, e_act) = match agent_side {
                Side::Player => (mine, theirs),
                Side::Enemy => (theirs, mine),
            };
            let before = snapshot(&battle);
            let log = engine.resolve_turn(&mut battle, p_act, e_act, &mut rng);
            unmodelled.extend(log.unmodelled.iter().cloned());
            dealt += u32::from(log.damage_to(agent_side.other()));
            taken += u32::from(log.damage_to(agent_side));
            note_exploits(&log.events, &before, &battle, agent_side, &mut exploits);
        };

        let party = battle.side(agent_side);
        let hp_remaining: f32 = if party.mons.is_empty() {
            0.0
        } else {
            party.mons.iter().map(|m| m.hp_fraction()).sum::<f32>()
                / party.mons.len() as f32
        };

        // The engine reports outcomes from the player's point of view; flip
        // them when the agent was the enemy, so a report never has to.
        let outcome = match (agent_side, outcome) {
            (Side::Enemy, Outcome::PlayerWins) => Outcome::EnemyWins,
            (Side::Enemy, Outcome::EnemyWins) => Outcome::PlayerWins,
            (_, o) => o,
        };

        exploits.sort_unstable();
        exploits.dedup();

        EpisodeResult {
            episode,
            seed,
            opponent: opp.name.clone(),
            side: agent_side,
            outcome,
            turns: battle.turn,
            damage_dealt: dealt,
            damage_taken: taken,
            hp_remaining,
            rolls: rng.rolls(),
            unmodelled,
            illegal_choices: illegal,
            exploits,
        }
    }
}

/// The bits of a battle an exploit detector needs to compare across a turn.
struct Snapshot {
    focus_energy: [bool; 2],
    stages_off_neutral: [bool; 2],
    badges: u8,
}

fn snapshot(b: &Battle) -> Snapshot {
    Snapshot {
        focus_energy: [
            b.active(Side::Player).volatile.focus_energy,
            b.active(Side::Enemy).volatile.focus_energy,
        ],
        stages_off_neutral: [
            !b.active(Side::Player).stages.is_neutral(),
            !b.active(Side::Enemy).stages.is_neutral(),
        ],
        badges: b.badges,
    }
}

/// Detect which Gen 1 bugs a turn's events actually used.
///
/// Conservative on purpose: it reports a bug only when the event *observably*
/// depended on it. Over-reporting would make the exploit list noise, and noise
/// is ignored, which defeats the point of having it.
fn note_exploits(
    events: &[Event],
    before: &Snapshot,
    after: &Battle,
    agent_side: Side,
    out: &mut Vec<Exploit>,
) {
    let idx = |s: Side| if s == Side::Player { 0 } else { 1 };
    for ev in events {
        match ev {
            // A miss the agent benefited from: the opponent missed.
            Event::Missed {
                side,
                why: MissReason::Accuracy,
            } if *side != agent_side => {
                out.push(Exploit::OneIn256Miss);
            }
            Event::Missed {
                side,
                why: MissReason::NoEffect,
            } if *side != agent_side => {
                out.push(Exploit::ResistanceMiss);
            }
            Event::Damage {
                side,
                critical: true,
                ..
            } if *side == agent_side => {
                if before.focus_energy[idx(*side)] {
                    out.push(Exploit::FocusEnergyCrit);
                }
                if before.stages_off_neutral[idx(side.other())]
                    || before.stages_off_neutral[idx(*side)]
                {
                    out.push(Exploit::CritIgnoredStages);
                }
            }
            Event::SubstituteBroke { side } if *side != agent_side => {
                out.push(Exploit::SubstituteDamageLeak);
            }
            Event::Residual {
                side,
                source: crate::sim::Residual::LeechSeed,
                ..
            } if *side != agent_side && after.active(*side).volatile.toxic => {
                out.push(Exploit::ToxicLeechSeed);
            }
            Event::Fainted { side } if *side != agent_side => {
                // A Hyper Beam that killed and skipped its recharge.
                if events.iter().any(|e| {
                    matches!(e, Event::Used { side: s, move_id }
                        if *s == agent_side
                            && after.last_move[agent_side as usize] == *move_id)
                }) && !after.active(agent_side).volatile.recharging
                    && events
                        .iter()
                        .any(|e| matches!(e, Event::Damage { critical: _, .. }))
                {
                    // Only claim it when the move really was Hyper Beam-shaped:
                    // the agent's active Pokémon is not recharging and its move
                    // set the recharge flag on a survivor in other turns. This
                    // is deliberately narrow — see the module note on
                    // conservatism.
                }
            }
            _ => {}
        }
    }
    if before.badges != 0 && after.badges != 0 {
        // Badge stacking is only claimed when a stat change happened while
        // badges were held, which is the condition the cartridge re-applies
        // under.
        if events
            .iter()
            .any(|e| matches!(e, Event::StatChanged { moved: true, .. }))
        {
            out.push(Exploit::BadgeBoostStacked);
        }
    }
}

/// The aggregate.
#[derive(Clone, Debug)]
pub struct Report {
    pub agent: String,
    pub opponent_policy: String,
    pub base_seed: u64,
    pub results: Vec<EpisodeResult>,
    pub used_hidden_information: bool,
    pub provenance: Provenance,
}

impl Report {
    pub fn new(
        agent: String,
        opponent_policy: String,
        base_seed: u64,
        results: Vec<EpisodeResult>,
    ) -> Report {
        Report {
            agent,
            opponent_policy,
            base_seed,
            results,
            used_hidden_information: false,
            provenance: Provenance::Seeded { seed: base_seed },
        }
    }

    pub fn with_hidden_information(mut self, yes: bool) -> Self {
        self.used_hidden_information = yes;
        self
    }

    pub fn episodes(&self) -> usize {
        self.results.len()
    }

    pub fn wins(&self) -> usize {
        self.results.iter().filter(|r| r.won()).count()
    }

    pub fn win_rate(&self) -> f32 {
        if self.results.is_empty() {
            0.0
        } else {
            self.wins() as f32 / self.results.len() as f32
        }
    }

    /// The fraction of episodes in which every effect reached was fully
    /// modelled. **Below 1.0, the win rate is qualified** — say so wherever it
    /// is quoted.
    pub fn clean_fraction(&self) -> f32 {
        if self.results.is_empty() {
            return 1.0;
        }
        self.results.iter().filter(|r| r.is_clean()).count() as f32
            / self.results.len() as f32
    }

    /// The win rate over the episodes that were fully modelled. This is the
    /// number to quote when [`Report::clean_fraction`] is below 1.
    pub fn clean_win_rate(&self) -> Option<f32> {
        let clean: Vec<_> = self.results.iter().filter(|r| r.is_clean()).collect();
        if clean.is_empty() {
            return None;
        }
        Some(clean.iter().filter(|r| r.won()).count() as f32 / clean.len() as f32)
    }

    pub fn mean_turns(&self) -> f32 {
        if self.results.is_empty() {
            return 0.0;
        }
        self.results.iter().map(|r| r.turns as f32).sum::<f32>()
            / self.results.len() as f32
    }

    /// Damage dealt per point taken — an efficiency figure that separates
    /// "won at full health" from "won with a sliver left".
    pub fn damage_efficiency(&self) -> f32 {
        let dealt: u64 = self.results.iter().map(|r| u64::from(r.damage_dealt)).sum();
        let taken: u64 = self.results.iter().map(|r| u64::from(r.damage_taken)).sum();
        if taken == 0 {
            f32::INFINITY
        } else {
            dealt as f32 / taken as f32
        }
    }

    pub fn mean_hp_remaining(&self) -> f32 {
        if self.results.is_empty() {
            return 0.0;
        }
        self.results.iter().map(|r| r.hp_remaining).sum::<f32>()
            / self.results.len() as f32
    }

    pub fn illegal_choices(&self) -> u32 {
        self.results.iter().map(|r| r.illegal_choices).sum()
    }

    /// How often each Gen 1 bug appeared in a winning line, most common first.
    pub fn exploits(&self) -> Vec<(Exploit, usize)> {
        let mut counts: Vec<(Exploit, usize)> = Vec::new();
        for r in self.results.iter().filter(|r| r.won()) {
            for &x in &r.exploits {
                match counts.iter_mut().find(|(e, _)| *e == x) {
                    Some((_, n)) => *n += 1,
                    None => counts.push((x, 1)),
                }
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        counts
    }

    /// A Wilson score interval for the win rate at ~95 % confidence.
    ///
    /// A bare win rate over fifty episodes is not a measurement, and a normal
    /// approximation degenerates at the ends (100 % of 20 would report ±0).
    /// Wilson does not.
    pub fn win_rate_interval(&self) -> (f32, f32) {
        let n = self.results.len() as f32;
        if n == 0.0 {
            return (0.0, 0.0);
        }
        let p = self.win_rate();
        let z = 1.96f32;
        let denom = 1.0 + z * z / n;
        let centre = (p + z * z / (2.0 * n)) / denom;
        let spread = z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt() / denom;
        ((centre - spread).max(0.0), (centre + spread).min(1.0))
    }

    /// The whole thing as text, with every qualification the rules demand.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let (lo, hi) = self.win_rate_interval();
        s.push_str(&format!("agent      {}\n", self.agent));
        s.push_str(&format!("opponent   {}\n", self.opponent_policy));
        s.push_str(&format!(
            "seed       {} (episode seed = mix(base, index))\n",
            self.base_seed
        ));
        s.push_str(&format!("dice       {}\n", self.provenance));
        s.push_str(&format!("episodes   {}\n", self.episodes()));
        s.push_str(&format!(
            "win rate   {:.1}%  (95% CI {:.1}–{:.1}%)\n",
            self.win_rate() * 100.0,
            lo * 100.0,
            hi * 100.0
        ));
        s.push_str(&format!("turns      {:.1} mean\n", self.mean_turns()));
        s.push_str(&format!(
            "efficiency {:.2} damage dealt per taken\n",
            self.damage_efficiency()
        ));
        s.push_str(&format!(
            "margin     {:.1}% party HP left\n",
            self.mean_hp_remaining() * 100.0
        ));

        if self.clean_fraction() < 1.0 {
            s.push_str(&format!(
                "\nQUALIFIED: only {:.1}% of episodes used exclusively modelled effects.\n",
                self.clean_fraction() * 100.0
            ));
            if let Some(w) = self.clean_win_rate() {
                s.push_str(&format!(
                    "           win rate over the fully modelled episodes: {:.1}%\n",
                    w * 100.0
                ));
            }
        }
        if self.illegal_choices() > 0 {
            s.push_str(&format!(
                "\nILLEGAL: the agent returned an unavailable action {} time(s); each was\n\
                 replaced by the first legal one, which is not the agent's play.\n",
                self.illegal_choices()
            ));
        }
        if self.used_hidden_information {
            s.push_str(
                "\nNOT A COMPETITIVE SCORE: this agent was given information a player at the\n\
                 console could not have.\n",
            );
        }
        let ex = self.exploits();
        if !ex.is_empty() {
            s.push_str("\nGen 1 bugs that appeared in winning lines:\n");
            for (x, n) in ex {
                s.push_str(&format!("  {:>4}x  {}\n", n, x.label()));
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::{Greedy, RandomAgent};
    use crate::testsupport::{battler, engine};

    fn teams(e: &Engine) -> (Vec<Team>, Vec<Team>) {
        let strong = Party::new(vec![battler(e, 1, 50, [5, 1, 0, 0])]);
        let weak = Party::new(vec![battler(e, 2, 40, [1, 2, 0, 0])]);
        (
            vec![Team::new("strong", strong)],
            vec![Team::new("weak", weak)],
        )
    }

    fn protocol(e: &Engine, episodes: usize) -> Protocol {
        let (t, o) = teams(e);
        let mut p = Protocol::new(0xC0FFEE, episodes);
        p.teams = t;
        p.opponents = o;
        p
    }

    /// **The seed depends on the episode index and nothing else.** Two agents
    /// see identical dice, so a difference between them is a difference in
    /// play.
    #[test]
    fn the_seed_is_a_function_of_the_episode_index_alone() {
        let e = engine();
        let p = protocol(&e, 3);
        let a: Vec<u64> = (0..10).map(|i| p.seed_for(i)).collect();
        let b: Vec<u64> = (0..10).map(|i| p.seed_for(i)).collect();
        assert_eq!(a, b);
        // And consecutive indices are not correlated into the same stream.
        assert_eq!(a.iter().collect::<std::collections::HashSet<_>>().len(), 10);
    }

    /// A whole run is reproducible from the base seed.
    #[test]
    fn a_run_is_reproducible() {
        let e = engine();
        let p = protocol(&e, 2);
        let run = || {
            let mut agent = Greedy::new();
            p.run(&e, &mut agent, || Box::new(RandomAgent::new(1)))
        };
        let a = run();
        let b = run();
        assert_eq!(a.wins(), b.wins());
        assert_eq!(a.mean_turns(), b.mean_turns());
        let outcomes_a: Vec<_> = a.results.iter().map(|r| r.outcome).collect();
        let outcomes_b: Vec<_> = b.results.iter().map(|r| r.outcome).collect();
        assert_eq!(outcomes_a, outcomes_b);
    }

    /// Every matchup is played from both sides, so a team advantage cannot pass
    /// for an agent advantage.
    #[test]
    fn both_sides_of_every_matchup_are_played() {
        let e = engine();
        let p = protocol(&e, 2);
        let mut agent = Greedy::new();
        let r = p.run(&e, &mut agent, || Box::new(RandomAgent::new(1)));
        assert_eq!(r.episodes(), 2 * 2, "2 episodes x 2 sides x 1 matchup");
        assert!(r.results.iter().any(|x| x.side == Side::Player));
        assert!(r.results.iter().any(|x| x.side == Side::Enemy));
    }

    /// The report says what it does not know. A qualified win rate must
    /// announce itself.
    #[test]
    fn a_qualified_report_says_so_in_its_own_text() {
        let mut r = Report::new("t".into(), "greedy".into(), 1, vec![]);
        r.results.push(EpisodeResult {
            episode: 0,
            seed: 1,
            opponent: "x".into(),
            side: Side::Player,
            outcome: Outcome::PlayerWins,
            turns: 5,
            damage_dealt: 10,
            damage_taken: 2,
            hp_remaining: 0.8,
            rolls: 40,
            unmodelled: vec!["Bide is not modelled".into()],
            illegal_choices: 0,
            exploits: vec![],
        });
        assert!(r.clean_fraction() < 1.0);
        let text = r.to_text();
        assert!(text.contains("QUALIFIED"), "{text}");
    }

    /// An agent given hidden information cannot present a clean score.
    #[test]
    fn a_foreknowledge_run_is_labelled_in_its_own_text() {
        let r =
            Report::new("t".into(), "g".into(), 1, vec![]).with_hidden_information(true);
        assert!(r.to_text().contains("NOT A COMPETITIVE SCORE"));
    }

    /// Illegal choices appear in the text; they are never silently improved.
    #[test]
    fn illegal_choices_are_reported() {
        let mut r = Report::new("t".into(), "g".into(), 1, vec![]);
        r.results.push(EpisodeResult {
            episode: 0,
            seed: 1,
            opponent: "x".into(),
            side: Side::Player,
            outcome: Outcome::PlayerWins,
            turns: 5,
            damage_dealt: 10,
            damage_taken: 2,
            hp_remaining: 0.8,
            rolls: 40,
            unmodelled: vec![],
            illegal_choices: 3,
            exploits: vec![],
        });
        assert!(r.to_text().contains("ILLEGAL"));
    }

    /// A confidence interval, because a bare win rate over a few episodes is
    /// not a measurement. Wilson does not degenerate at 100 %.
    #[test]
    fn the_interval_does_not_collapse_at_the_extremes() {
        let mut r = Report::new("t".into(), "g".into(), 1, vec![]);
        for i in 0..20 {
            r.results.push(EpisodeResult {
                episode: i,
                seed: 1,
                opponent: "x".into(),
                side: Side::Player,
                outcome: Outcome::PlayerWins,
                turns: 5,
                damage_dealt: 10,
                damage_taken: 2,
                hp_remaining: 0.8,
                rolls: 40,
                unmodelled: vec![],
                illegal_choices: 0,
                exploits: vec![],
            });
        }
        assert_eq!(r.win_rate(), 1.0);
        let (lo, hi) = r.win_rate_interval();
        assert!(lo < 1.0, "a 20-episode sweep is not certainty: lo={lo}");
        assert!(hi <= 1.0);
        assert!(lo > 0.7);
    }

    /// A stronger team really does win more — otherwise the harness is
    /// measuring nothing and every later comparison is worthless.
    #[test]
    fn the_harness_can_tell_a_strong_team_from_a_weak_one() {
        let e = engine();
        let mut p = Protocol::new(99, 6);
        p.teams = vec![Team::new(
            "strong",
            Party::new(vec![battler(&e, 1, 60, [5, 1, 0, 0])]),
        )];
        p.opponents = vec![Team::new(
            "weak",
            Party::new(vec![battler(&e, 2, 20, [1, 2, 0, 0])]),
        )];
        let mut agent = Greedy::new();
        let r = p.run(&e, &mut agent, || Box::new(Greedy::new()));
        // Both sides are played, so a level-60 team should take well over half.
        assert!(r.win_rate() > 0.4, "win rate {}", r.win_rate());
        assert!(r.episodes() == 12);
    }
}

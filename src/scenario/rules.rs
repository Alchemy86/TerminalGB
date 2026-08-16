//! **Rules that are enforced, not suggested** — and the referee that enforces
//! them.
//!
//! # Why a rule has to be checkable by the host
//!
//! A rule written into a prompt is a *request*. The model may honour it, ignore
//! it, or honour it for two hours and then not. That is not a foundation for
//! any claim about a run, and the claim is the whole product: a save state that
//! came out of a legitimate playthrough is worth something and a forged one is
//! worth nothing, and from the outside the two files are indistinguishable.
//!
//! So every rule here is decided by the **host**, from evidence the host owns:
//!
//! | rule | evidence |
//! |---|---|
//! | [`Rule::NoMemoryWrites`] | `Gameboy::host_writes()` and every plugin's `has_written` |
//! | [`Rule::NoStateLoads`] | the runner counts its own rewinds |
//! | [`Rule::Require`] | a condition on observed state, tested at success |
//! | [`Rule::Forbid`] | a condition on observed state, tested every sample |
//! | [`Rule::MaxCalls`] etc. | the governor's own counters |
//!
//! Nothing on that right-hand column can be talked out of.
//!
//! # A violation ends the run, and names itself
//!
//! [`Verdict::Violated`] carries the rule, the frame and a sentence. A run that
//! broke a rule is not a run that scored badly — it is not a run at all, and
//! the report says which rule and when rather than quietly reporting a failure.
//!
//! # Allowances are declared, never hidden
//!
//! Some things are legitimate and change what a number means: a rewind after a
//! crash, a battle recommendation computed from the opponent's true stats, a
//! start state somebody built by hand. [`Allowances`] is the list, it is
//! printed in every report, and [`Allowances::is_clean`] is what "this run had
//! no help" means. An undeclared allowance makes every number worthless — the
//! same principle the agent's memory-write ban already encodes.

use super::knowledge::Knowledge;
use crate::longplay::episode::Cond;
use crate::longplay::progress::Progress;
use crate::longplay::watch::Sample;
use terminalgb_plugin_agent::json;

/// What a run is and is not allowed to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// **The load-bearing one.** Nothing but button presses may reach the
    /// emulated machine. Broken by a host `debug_write` or by any plugin that
    /// actually wrote memory.
    NoMemoryWrites,
    /// No save state may be loaded once the attempt has begun. The attempt's
    /// own reset is not a load under this rule — it is what starts the attempt —
    /// but a rewind inside one is.
    NoStateLoads,
    /// Must hold **when the goal is reached**. This is how "must actually speak
    /// to Oak" and "must legitimately obtain a Pokémon" are written: as an
    /// observable the game's own code sets, not as a script of presses.
    Require(Cond),
    /// Must **never** hold. Checked at every sample; the first time it does the
    /// run is over.
    Forbid(Cond),
    /// Model calls.
    MaxCalls(u32),
    /// Emulated frames.
    MaxFrames(u64),
    /// Wall-clock seconds.
    MaxWallSeconds(u64),
    /// US dollars, as the driver reports them — so a **floor**, never a
    /// ceiling, and the wall clock is the ceiling that cannot be misreported.
    MaxUsd(f64),
    /// What to do in a battle. Enforced as far as it is observable: `Flee` is
    /// broken by winning a fight, `Fight` by running from one.
    Battle(BattlePolicy),
}

/// What a scenario says about fighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BattlePolicy {
    /// Run from everything. What the shiny-hunt walker does, and it is a policy
    /// rather than an omission.
    Flee,
    /// Never run. The scenario is about the fight.
    Fight,
    /// The agent's call. The default, and the honest one for a scenario that is
    /// about getting somewhere.
    Decide,
}

impl BattlePolicy {
    pub fn name(self) -> &'static str {
        match self {
            BattlePolicy::Flee => "flee",
            BattlePolicy::Fight => "fight",
            BattlePolicy::Decide => "decide",
        }
    }

    pub fn parse(s: &str) -> Option<BattlePolicy> {
        match s.trim().to_ascii_lowercase().as_str() {
            "flee" | "run" => Some(BattlePolicy::Flee),
            "fight" => Some(BattlePolicy::Fight),
            "decide" | "agent" | "any" => Some(BattlePolicy::Decide),
            _ => None,
        }
    }

    /// What the agent is told, in the words it acts on.
    pub fn instruction(self) -> Option<&'static str> {
        match self {
            BattlePolicy::Flee => Some(
                "BATTLE POLICY: flee. Every battle is to be run from — choose RUN \
                 from the battle menu. Do not fight, do not throw a ball.",
            ),
            BattlePolicy::Fight => Some(
                "BATTLE POLICY: fight. You may not run from a battle. Choose FIGHT \
                 and pick a move.",
            ),
            BattlePolicy::Decide => None,
        }
    }
}

impl Rule {
    /// The short stable name a report and a violation use. Stable because a
    /// recorded run months old must still resolve.
    pub fn name(&self) -> &'static str {
        match self {
            Rule::NoMemoryWrites => "no_memory_writes",
            Rule::NoStateLoads => "no_state_loads",
            Rule::Require(_) => "require",
            Rule::Forbid(_) => "forbid",
            Rule::MaxCalls(_) => "max_calls",
            Rule::MaxFrames(_) => "max_frames",
            Rule::MaxWallSeconds(_) => "max_wall_seconds",
            Rule::MaxUsd(_) => "max_usd",
            Rule::Battle(_) => "battle",
        }
    }

    /// How it was written, so a report can quote the scenario back.
    pub fn source(&self) -> String {
        match self {
            Rule::Require(c) => format!("require {}", c.source),
            Rule::Forbid(c) => format!("forbid {}", c.source),
            Rule::MaxCalls(n) => format!("max_calls {n}"),
            Rule::MaxFrames(n) => format!("max_frames {n}"),
            Rule::MaxWallSeconds(n) => format!("max_wall_seconds {n}"),
            Rule::MaxUsd(v) => format!("max_usd {v}"),
            Rule::Battle(p) => format!("battle {}", p.name()),
            other => other.name().to_string(),
        }
    }

    /// Parse one line of a scenario's `rules` list.
    ///
    /// A tiny grammar on purpose, the same argument as [`Cond::parse`]: a rule
    /// nobody can read at a glance is a rule that gets misused, and this one is
    /// load-bearing.
    pub fn parse(text: &str) -> Result<Rule, String> {
        let t = text.trim();
        let (head, rest) = t.split_once(char::is_whitespace).unwrap_or((t, ""));
        let rest = rest.trim();
        let num = |what: &str| -> Result<f64, String> {
            rest.parse::<f64>()
                .map_err(|_| format!("{what} needs a number, got {rest:?}"))
        };
        Ok(match head {
            "no_memory_writes" => Rule::NoMemoryWrites,
            "no_state_loads" => Rule::NoStateLoads,
            "require" => Rule::Require(Cond::parse(rest)?),
            "forbid" => Rule::Forbid(Cond::parse(rest)?),
            "max_calls" => Rule::MaxCalls(num("max_calls")? as u32),
            "max_frames" => Rule::MaxFrames(num("max_frames")? as u64),
            "max_wall_seconds" => Rule::MaxWallSeconds(num("max_wall_seconds")? as u64),
            "max_usd" => Rule::MaxUsd(num("max_usd")?),
            "battle" => Rule::Battle(BattlePolicy::parse(rest).ok_or_else(|| {
                format!("battle policy {rest:?} is not flee, fight or decide")
            })?),
            other => {
                return Err(format!(
                    "{other:?} is not a rule — try no_memory_writes, no_state_loads, \
                     require <cond>, forbid <cond>, max_calls <n>, max_frames <n>, \
                     max_wall_seconds <n>, max_usd <n>, battle <flee|fight|decide>"
                ))
            }
        })
    }

    /// The watch this rule's condition needs, if any.
    pub fn needs_watch(&self) -> Option<&str> {
        match self {
            Rule::Require(c) | Rule::Forbid(c) => c.needs_watch(),
            _ => None,
        }
    }
}

/// What the referee saw.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Nothing to say. Carry on.
    Ok,
    /// A rule was broken. The run is over and this is why.
    Violated(Violation),
}

/// A broken rule, with everything a reader needs and nothing they do not.
#[derive(Debug, Clone, PartialEq)]
pub struct Violation {
    pub rule: String,
    pub frame: u64,
    pub detail: String,
}

impl Violation {
    pub fn line(&self) -> String {
        format!(
            "rule {:?} broken at frame {}: {}",
            self.rule, self.frame, self.detail
        )
    }

    pub fn to_json(&self, w: &mut json::Writer) {
        w.begin_obj();
        w.str_field("rule", &self.rule);
        w.num_field("frame", self.frame as f64);
        w.str_field("detail", &self.detail);
        w.end_obj();
    }
}

/// Everything the run was measured on that was not the agent's own doing.
///
/// Printed in every report. `state_resets` is expected — an episode is resets —
/// but a reader must be able to see it rather than deduce it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Allowances {
    /// Attempt resets. Expected, still declared.
    pub state_resets: u32,
    /// Rewinds *inside* an attempt. Not expected.
    pub rewinds: u32,
    /// Bytes the host wrote into the emulated machine.
    pub memory_writes: u64,
    /// Decisions taken with a battle recommendation in the snapshot, computed
    /// from the opponent's true stats — which a player cannot see.
    pub battle_advice: u64,
    /// Decisions taken with learned guidance in the snapshot.
    pub guided_decisions: u64,
    /// Knowledge sources consulted, by name.
    pub sources: Vec<String>,
    /// The start state was supplied rather than played to.
    pub start_state_supplied: bool,
}

impl Allowances {
    /// Did the run have no help beyond the resets an episode is made of?
    pub fn is_clean(&self) -> bool {
        self.rewinds == 0 && self.memory_writes == 0 && self.battle_advice == 0
    }

    pub fn to_json(&self, w: &mut json::Writer) {
        w.obj_field("assistance");
        w.num_field("state_resets", self.state_resets as f64);
        w.num_field("rewinds", self.rewinds as f64);
        w.num_field("memory_writes", self.memory_writes as f64);
        w.num_field("battle_advice_decisions", self.battle_advice as f64);
        w.num_field("guided_decisions", self.guided_decisions as f64);
        w.bool_field("start_state_supplied", self.start_state_supplied);
        w.bool_field("clean", self.is_clean());
        w.arr_field("knowledge_sources");
        for s in &self.sources {
            w.str_item(s);
        }
        w.end_arr();
        w.end_obj();
    }

    /// The human-readable block, for a report a person reads.
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!("state resets: {}", self.state_resets),
            format!("rewinds inside an attempt: {}", self.rewinds),
            format!("host memory writes: {}", self.memory_writes),
        ];
        if self.battle_advice > 0 {
            v.push(format!(
                "battle advice given on {} decisions — computed from the \
                 opponent's true stats, which a player cannot see",
                self.battle_advice
            ));
        }
        if self.guided_decisions > 0 {
            v.push(format!(
                "learned guidance given on {} decisions",
                self.guided_decisions
            ));
        }
        if self.start_state_supplied {
            v.push("start state supplied rather than played to".into());
        }
        if !self.sources.is_empty() {
            v.push(format!("knowledge consulted: {}", self.sources.join(", ")));
        }
        v
    }
}

/// What the host can see, handed to the referee each sample.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    pub frame: u64,
    pub wall_seconds: u64,
    pub calls: u32,
    pub usd: f64,
    /// Host writes plus any plugin that has written.
    pub memory_writes: u64,
    pub rewinds: u32,
    /// Battles seen to end in a win, and in a successful escape. Both are
    /// derived from watching the battle flag fall with the party still standing
    /// — see [`super::run`].
    pub battles_won: u32,
    pub battles_fled: u32,
    /// A **trainer** battle is in progress. `wIsInBattle` reads 2 for one and
    /// 1 for a wild encounter, and `TryRunningFromBattle` branches on exactly
    /// that: `ld a,[wIsInBattle] / dec a / jr nz, .trainerBattle`, which
    /// prints "No! There's no running from a trainer battle!" and gives the
    /// turn away. See [`Referee::check`].
    pub in_trainer_battle: bool,
}

/// Holds the rules and decides.
#[derive(Debug, Clone, Default)]
pub struct Referee {
    rules: Vec<Rule>,
    /// Latched, so a violation reported once is not reported again as the run
    /// unwinds.
    seen: Option<Violation>,
}

impl Referee {
    pub fn new(rules: Vec<Rule>) -> Referee {
        Referee { rules, seen: None }
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn violation(&self) -> Option<&Violation> {
        self.seen.as_ref()
    }

    /// The one rule that changes what the agent is told rather than only what
    /// the host checks.
    pub fn battle_policy(&self) -> BattlePolicy {
        self.rules
            .iter()
            .find_map(|r| match r {
                Rule::Battle(p) => Some(*p),
                _ => None,
            })
            .unwrap_or(BattlePolicy::Decide)
    }

    /// Instructions the agent must be given for the rules to be followable.
    ///
    /// A rule the host enforces but never states is a trap: the agent fails it
    /// without ever being told it existed. So every rule that constrains
    /// *behaviour* (rather than merely bounding cost) produces a line here.
    pub fn instructions(&self) -> Vec<String> {
        let mut v = Vec::new();
        for r in &self.rules {
            match r {
                Rule::Battle(p) => {
                    if let Some(t) = p.instruction() {
                        v.push(t.to_string());
                    }
                }
                Rule::Forbid(c) => v.push(format!(
                    "RULE: the run ends in failure the moment `{}` becomes true. \
                     Avoid it.",
                    c.source
                )),
                Rule::Require(c) => v.push(format!(
                    "RULE: reaching the goal only counts if `{}` is also true.",
                    c.source
                )),
                _ => {}
            }
        }
        v
    }

    /// Refuse a rule set that cannot be judged, **before** anything is spent.
    pub fn validate(
        &self,
        watches: &crate::longplay::watch::Watches,
    ) -> Result<(), String> {
        for r in &self.rules {
            if let Some(n) = r.needs_watch() {
                if watches.get(n).is_none() {
                    return Err(format!(
                        "rule `{}` needs a watch called {n:?}, which this scenario \
                         does not declare",
                        r.source()
                    ));
                }
            }
        }
        Ok(())
    }

    /// Judge a sample. Returns the first violation and latches it.
    pub fn check(&mut self, o: &Observed, p: &Progress, w: &Sample) -> Verdict {
        if let Some(v) = &self.seen {
            return Verdict::Violated(v.clone());
        }
        for r in &self.rules {
            let broken = match r {
                Rule::NoMemoryWrites if o.memory_writes > 0 => Some(format!(
                    "{} byte(s) were written into the emulated machine by \
                     something other than the game itself",
                    o.memory_writes
                )),
                Rule::NoStateLoads if o.rewinds > 0 => Some(format!(
                    "{} save state(s) were loaded during the attempt",
                    o.rewinds
                )),
                Rule::Forbid(c) if c.holds(p, w) => {
                    Some(format!("`{}` became true", c.source))
                }
                Rule::MaxCalls(n) if o.calls > *n => {
                    Some(format!("{} model calls, the limit is {n}", o.calls))
                }
                Rule::MaxFrames(n) if o.frame > *n => {
                    Some(format!("{} frames, the limit is {n}", o.frame))
                }
                Rule::MaxWallSeconds(n) if o.wall_seconds > *n => Some(format!(
                    "{}s of wall clock, the limit is {n}s",
                    o.wall_seconds
                )),
                Rule::MaxUsd(v) if o.usd > *v => Some(format!(
                    "${:.4} reported spent, the limit is ${v:.4}",
                    o.usd
                )),
                Rule::Battle(BattlePolicy::Flee) if o.battles_won > 0 => Some(format!(
                    "{} battle(s) were won, and the policy is to flee",
                    o.battles_won
                )),
                // **A trainer battle cannot be fled, so a `battle flee` leg
                // that has walked into one is already lost.** Naming it here
                // is the whole point: otherwise it shows up as a timeout, and
                // a timeout says nothing about why. This is the failure the
                // community reports as a policy that "learned to insta run
                // from battles and consequently get stuck in trainer battles"
                // — a navigation reward plus permission to flee teaches
                // fleeing, and then the game declines.
                Rule::Battle(BattlePolicy::Flee) if o.in_trainer_battle => Some(
                    "a trainer battle is in progress and the policy is to flee, \
                     but a trainer battle cannot be fled — TryRunningFromBattle \
                     branches on wIsInBattle and prints \"No! There's no running \
                     from a trainer battle!\""
                        .to_string(),
                ),
                Rule::Battle(BattlePolicy::Fight) if o.battles_fled > 0 => Some(format!(
                    "{} battle(s) were run from, and the policy is to fight",
                    o.battles_fled
                )),
                _ => None,
            };
            if let Some(detail) = broken {
                let v = Violation {
                    rule: r.source(),
                    frame: o.frame,
                    detail,
                };
                self.seen = Some(v.clone());
                return Verdict::Violated(v);
            }
        }
        Verdict::Ok
    }

    /// The `require` rules, tested only when the goal has been reached.
    ///
    /// Separate from [`Referee::check`] because a requirement is not a thing
    /// that must be true *throughout* — "must have a Pokémon" is false for the
    /// first ten minutes of every legitimate playthrough. It is a condition on
    /// the *finish*.
    pub fn check_requirements(
        &mut self,
        frame: u64,
        p: &Progress,
        w: &Sample,
    ) -> Verdict {
        if let Some(v) = &self.seen {
            return Verdict::Violated(v.clone());
        }
        for r in &self.rules {
            if let Rule::Require(c) = r {
                if !c.holds(p, w) {
                    let v = Violation {
                        rule: r.source(),
                        frame,
                        detail: format!(
                            "the goal was reached but `{}` is not true, so it does \
                             not count",
                            c.source
                        ),
                    };
                    self.seen = Some(v.clone());
                    return Verdict::Violated(v);
                }
            }
        }
        Verdict::Ok
    }
}

/// Assemble the standing instructions a run gives its agent: the rules it must
/// follow, then whatever the knowledge base has to say.
pub fn standing_notes(
    referee: &Referee,
    knowledge: &Knowledge,
    map: u8,
    goal_map: Option<u8>,
) -> Vec<String> {
    let mut v = referee.instructions();
    v.extend(knowledge.notes_for(map, goal_map));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::longplay::watch::Watches;

    fn obs() -> Observed {
        Observed::default()
    }

    #[test]
    fn rules_parse_from_the_words_a_person_writes() {
        assert_eq!(
            Rule::parse("no_memory_writes").unwrap(),
            Rule::NoMemoryWrites
        );
        assert_eq!(Rule::parse("max_calls 12").unwrap(), Rule::MaxCalls(12));
        assert_eq!(
            Rule::parse("battle flee").unwrap(),
            Rule::Battle(BattlePolicy::Flee)
        );
        assert!(matches!(
            Rule::parse("require party>=1").unwrap(),
            Rule::Require(_)
        ));
        assert!(Rule::parse("be_nice").is_err());
        assert!(Rule::parse("battle sometimes").is_err());
        assert!(Rule::parse("max_calls lots").is_err());
    }

    /// **The load-bearing test.** One byte written by anything but the game and
    /// the run is over, whatever it achieved.
    #[test]
    fn one_memory_write_ends_the_run() {
        let mut r = Referee::new(vec![Rule::NoMemoryWrites]);
        let p = Progress::default();
        let w = Sample::default();
        assert_eq!(r.check(&obs(), &p, &w), Verdict::Ok);
        let o = Observed {
            frame: 900,
            memory_writes: 1,
            ..obs()
        };
        match r.check(&o, &p, &w) {
            Verdict::Violated(v) => {
                assert_eq!(v.rule, "no_memory_writes");
                assert_eq!(v.frame, 900);
                assert!(v.line().contains("900"));
            }
            other => panic!("{other:?}"),
        }
    }

    /// A violation latches: the run is over, and the report must name the first
    /// thing that went wrong rather than whatever was checked last.
    #[test]
    fn the_first_violation_is_the_one_that_is_reported() {
        let mut r = Referee::new(vec![Rule::NoMemoryWrites, Rule::MaxCalls(1)]);
        let o = Observed {
            frame: 10,
            memory_writes: 1,
            calls: 99,
            ..obs()
        };
        let first = r.check(&o, &Progress::default(), &Sample::default());
        let again = r.check(
            &Observed { frame: 20, ..o },
            &Progress::default(),
            &Sample::default(),
        );
        assert_eq!(first, again);
        assert_eq!(r.violation().unwrap().frame, 10);
    }

    /// A requirement is a condition on the *finish*, not on every frame — a
    /// legitimate run has no Pokémon for its first ten minutes.
    #[test]
    fn a_requirement_is_judged_at_the_goal_and_not_before() {
        let mut r = Referee::new(vec![Rule::Require(Cond::parse("party>=1").unwrap())]);
        let empty = Progress::default();
        assert_eq!(
            r.check(&obs(), &empty, &Sample::default()),
            Verdict::Ok,
            "an empty party mid-run is not a violation"
        );
        match r.check_requirements(500, &empty, &Sample::default()) {
            Verdict::Violated(v) => assert!(v.detail.contains("does not count"), "{v:?}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_satisfied_requirement_passes() {
        let mut r = Referee::new(vec![Rule::Require(Cond::parse("badges>=1").unwrap())]);
        let p = Progress {
            badges: 0b0000_0001,
            ..Default::default()
        };
        assert_eq!(r.check_requirements(1, &p, &Sample::default()), Verdict::Ok);
    }

    /// A rule the host enforces but never states is a trap. Every behavioural
    /// rule must reach the agent in words.
    #[test]
    fn every_behavioural_rule_is_told_to_the_agent() {
        let r = Referee::new(vec![
            Rule::Battle(BattlePolicy::Flee),
            Rule::Forbid(Cond::parse("party_hp=0").unwrap()),
            Rule::Require(Cond::parse("party>=1").unwrap()),
            Rule::MaxCalls(4),
        ]);
        let notes = r.instructions().join(" | ");
        assert!(notes.contains("flee"), "{notes}");
        assert!(notes.contains("party_hp=0"), "{notes}");
        assert!(notes.contains("party>=1"), "{notes}");
    }

    #[test]
    fn a_flee_policy_is_broken_by_winning_and_a_fight_policy_by_running() {
        let mut flee = Referee::new(vec![Rule::Battle(BattlePolicy::Flee)]);
        let won = Observed {
            battles_won: 1,
            ..obs()
        };
        assert!(matches!(
            flee.check(&won, &Progress::default(), &Sample::default()),
            Verdict::Violated(_)
        ));
        let mut fight = Referee::new(vec![Rule::Battle(BattlePolicy::Fight)]);
        let fled = Observed {
            battles_fled: 1,
            ..obs()
        };
        assert!(matches!(
            fight.check(&fled, &Progress::default(), &Sample::default()),
            Verdict::Violated(_)
        ));
        // …and neither is broken by the other's evidence.
        let mut flee2 = Referee::new(vec![Rule::Battle(BattlePolicy::Flee)]);
        assert_eq!(
            flee2.check(&fled, &Progress::default(), &Sample::default()),
            Verdict::Ok
        );
    }

    /// **A `battle flee` leg standing in a trainer battle is already lost**,
    /// and it has to be *named* rather than left to expire as a timeout — the
    /// exact failure the community reports as a policy that learned to run
    /// from everything and then met somebody it could not run from.
    #[test]
    fn a_trainer_battle_forfeits_a_flee_policy_and_a_wild_one_does_not() {
        let mut flee = Referee::new(vec![Rule::Battle(BattlePolicy::Flee)]);
        let trainer = Observed {
            in_trainer_battle: true,
            ..obs()
        };
        let v = flee.check(&trainer, &Progress::default(), &Sample::default());
        match v {
            Verdict::Violated(x) => {
                assert!(x.detail.contains("cannot be fled"), "{}", x.detail)
            }
            _ => panic!("a trainer battle must forfeit a flee policy"),
        }
        // A wild encounter is fleeable, so it is not a violation.
        let mut flee2 = Referee::new(vec![Rule::Battle(BattlePolicy::Flee)]);
        assert_eq!(
            flee2.check(&obs(), &Progress::default(), &Sample::default()),
            Verdict::Ok
        );
        // And a leg that allows fighting is untouched by it.
        let mut decide = Referee::new(vec![Rule::Battle(BattlePolicy::Decide)]);
        assert_eq!(
            decide.check(&trainer, &Progress::default(), &Sample::default()),
            Verdict::Ok
        );
    }

    /// The expensive mistake: a rule on a watch nobody declared can never be
    /// judged, so it must be refused before a run spends anything.
    #[test]
    fn a_rule_on_an_undeclared_watch_is_refused_before_it_costs_anything() {
        let r = Referee::new(vec![Rule::Forbid(
            Cond::parse("watch:enemy_hp<=0").unwrap(),
        )]);
        assert!(r.validate(&Watches::default()).is_err());
    }

    #[test]
    fn a_clean_run_is_one_with_no_writes_no_rewinds_and_no_advice() {
        let clean = Allowances {
            state_resets: 10,
            ..Default::default()
        };
        assert!(clean.is_clean(), "resets are the episode, not assistance");
        assert!(!Allowances {
            memory_writes: 1,
            ..clean.clone()
        }
        .is_clean());
        assert!(!Allowances {
            battle_advice: 1,
            ..clean.clone()
        }
        .is_clean());
        assert!(!Allowances {
            rewinds: 1,
            ..clean
        }
        .is_clean());
    }

    /// Advice taken from the opponent's true stats must appear in the words a
    /// person reads, not only in a JSON field.
    #[test]
    fn battle_advice_is_declared_in_prose() {
        let a = Allowances {
            battle_advice: 3,
            ..Default::default()
        };
        let lines = a.lines().join(" ");
        assert!(lines.contains("a player cannot see"), "{lines}");
    }
}

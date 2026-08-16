//! **Scenarios** — a goal, a set of enforced rules, and the knowledge an agent
//! is allowed to plan with.
//!
//! Reader-facing page: [`docs/scenarios.md`](../docs/scenarios.md).
//!
//! # The three things stacked
//!
//! ```text
//!   scenario.json ──▶ Referee   rules the HOST decides, not the model
//!                 ──▶ Knowledge atlas + what earlier runs worked out
//!                 ──▶ Episode   the goal machinery already in src/longplay
//!                              │
//!                              ▼
//!                          run.rs ──▶ recording.json ──▶ replay, no model calls
//! ```
//!
//! Everything below the first line already existed. [`crate::longplay::episode`]
//! is the reset-and-repeat primitive with success and failure conditions; this
//! module adds the two things the captain asked for on top of it — **rules that
//! are enforced rather than suggested**, and a run that is a **reusable
//! artefact** rather than a log.
//!
//! # What a scenario is
//!
//! A small JSON file a person writes without touching Rust:
//!
//! ```json
//! {
//!   "schema": "gbscenario/1",
//!   "name": "leave-the-house",
//!   "objective": "Get out of Red's house and into Pallet Town.",
//!   "start_state": "states/bedroom.gbss",
//!   "goal": ["map=0x00"],
//!   "failure": ["play_seconds>600"],
//!   "rules": [
//!     "no_memory_writes",
//!     "battle flee",
//!     "max_calls 20"
//!   ],
//!   "knowledge": ["atlas", "guidance"],
//!   "timeout_frames": 40000
//! }
//! ```
//!
//! `goal` is an **AND** and `failure` is an **OR**, the same as an episode's,
//! for the same reasons — see [`crate::longplay::episode`].
//!
//! # Why the rules and the goal are different lists
//!
//! A goal is what the run is *for*. A rule is what makes reaching it mean
//! something. Reaching Route 1 by writing an event flag and a party into RAM
//! satisfies `map=0x0C` perfectly, and it is not a playthrough — which is
//! precisely the difference between [`examples/shiny_hunt.rs`'s
//! `make_state`](../examples/shiny_hunt.rs) and what this exists to build.
//!
//! That distinction is not rhetorical here. The same scenario run over the
//! walker's own route **fails**, at the frame of the first poke, naming the
//! rule; run over button presses it does not. See `testharness/scenario.rs`.
//!
//! # Honest about the hard part
//!
//! Nothing here has trained anything. What it produces is a **harness and an
//! honest report** — *"this scenario is solved in N decisions, replays
//! deterministically in M seconds, and here is the file"*. The recorded run is
//! shaped as training data (one row per decision, observation in, action out,
//! outcome) because that is cheap and obviously useful; whether it trains
//! anything is a claim nobody here has earned.

pub mod knowledge;
pub mod record;
pub mod rules;
pub mod run;

use std::path::{Path, PathBuf};

use crate::longplay::episode::{Cond, Episode};
use crate::longplay::watch::{Watch, Watches};
use knowledge::{Atlas, Guidance, Knowledge};
use record::fnv1a;
use rules::{Referee, Rule};
use terminalgb_plugin_agent::json;

/// Bump when a field is renamed, retyped or removed. Additions are free.
pub const SCENARIO_SCHEMA: &str = "gbscenario/1";

/// A knowledge source a scenario may switch on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// AtlasGB's evidenced Gen 1 addresses. Static reference; costs nothing to
    /// consult and is not privileged information.
    Atlas,
    /// What earlier runs walked. Emitted by a run, read by the next.
    Guidance,
    /// **Privileged.** A recommended move computed from the opponent's true
    /// stats, which a player at the console cannot see. Off unless asked for,
    /// and asking puts a line in the report — see [`rules::Allowances`].
    BattleAdvice,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Source::Atlas => "atlas",
            Source::Guidance => "guidance",
            Source::BattleAdvice => "battle_advice",
        }
    }

    pub fn parse(s: &str) -> Option<Source> {
        match s.trim().to_ascii_lowercase().as_str() {
            "atlas" => Some(Source::Atlas),
            "guidance" | "learned" => Some(Source::Guidance),
            "battle_advice" | "battle-advice" => Some(Source::BattleAdvice),
            _ => None,
        }
    }

    /// Does consulting this give the agent something a player could not have?
    pub fn is_privileged(self) -> bool {
        matches!(self, Source::BattleAdvice)
    }
}

/// A goal, the rules it is judged under, and the knowledge it may use.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: String,
    pub objective: String,
    /// The save state every attempt starts from, relative to the scenario file.
    pub start_state: Option<PathBuf>,
    /// **All** must hold for the goal to be reached.
    pub goal: Vec<Cond>,
    /// **Any** ends the attempt as a failure.
    pub failure: Vec<Cond>,
    pub rules: Vec<Rule>,
    pub knowledge: Vec<Source>,
    /// Where learned guidance is read from and written back to, relative to the
    /// scenario file. `None` means a run learns nothing durable.
    pub guidance_file: Option<PathBuf>,
    pub timeout_frames: u64,
    pub watches: Watches,
    /// One line per goal condition saying **which address or flag defines
    /// success and where it is defined in the cartridge**, so the reward is
    /// auditable by a reader rather than taken on trust.
    ///
    /// Optional, and additive to the schema — a scenario without it hashes
    /// exactly as it did before this field existed. But
    /// [`Scenario::briefing`] says so loudly when it is missing, because the
    /// whole claim this project makes over guide-derived work is that its
    /// goals come from the cartridge. **A goal nobody can check is back to
    /// being a proxy** — and one link in the Pewter chain was authored from a
    /// plausible assumption (walk to the laboratory) rather than from the flag
    /// the escort sets, which is exactly the class of error this field exists
    /// to make visible.
    pub evidence: Vec<String>,
    /// A map id the agent is told it is heading for, so the knowledge base can
    /// hand it a route. Derived from a `map=` goal when there is exactly one.
    goal_map: Option<u8>,
}

impl Default for Scenario {
    fn default() -> Self {
        Scenario {
            name: "scenario".into(),
            objective: String::new(),
            start_state: None,
            goal: Vec::new(),
            failure: Vec::new(),
            rules: Vec::new(),
            knowledge: Vec::new(),
            guidance_file: None,
            timeout_frames: 60_000,
            watches: Watches::default(),
            evidence: Vec::new(),
            goal_map: None,
        }
    }
}

impl Scenario {
    pub fn from_json(text: &str) -> Result<Scenario, String> {
        let v =
            json::parse(text).map_err(|e| format!("scenario is not JSON: {}", e.0))?;
        let schema = v.str_field("schema").unwrap_or("");
        if schema != SCENARIO_SCHEMA {
            return Err(format!(
                "scenario schema is {schema:?}, this build reads {SCENARIO_SCHEMA:?}"
            ));
        }
        let conds = |key: &str| -> Result<Vec<Cond>, String> {
            v.get(key)
                .and_then(|a| a.as_array())
                .unwrap_or(&[])
                .iter()
                .map(|c| {
                    c.as_str()
                        .ok_or_else(|| format!("{key} must be a list of strings"))
                        .and_then(Cond::parse)
                })
                .collect()
        };
        let rules = v
            .get("rules")
            .and_then(|a| a.as_array())
            .unwrap_or(&[])
            .iter()
            .map(|r| {
                r.as_str()
                    .ok_or_else(|| "rules must be a list of strings".to_string())
                    .and_then(Rule::parse)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let knowledge = v
            .get("knowledge")
            .and_then(|a| a.as_array())
            .unwrap_or(&[])
            .iter()
            .map(|k| {
                k.as_str().and_then(Source::parse).ok_or_else(|| {
                    format!(
                        "{:?} is not a knowledge source — try atlas, guidance, \
                             battle_advice",
                        k.as_str().unwrap_or("")
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let watches = Watches(
            v.get("watch")
                .and_then(|a| a.as_array())
                .unwrap_or(&[])
                .iter()
                .map(Watch::from_json)
                .collect::<Result<Vec<_>, _>>()?,
        );
        let mut s = Scenario {
            name: v.str_field("name").unwrap_or("scenario").to_string(),
            objective: v.str_field("objective").unwrap_or("").to_string(),
            start_state: v.str_field("start_state").map(PathBuf::from),
            goal: conds("goal")?,
            failure: conds("failure")?,
            rules,
            knowledge,
            guidance_file: v.str_field("guidance").map(PathBuf::from),
            timeout_frames: v.u64_field("timeout_frames").unwrap_or(60_000),
            watches,
            evidence: v
                .get("evidence")
                .and_then(|a| a.as_array())
                .unwrap_or(&[])
                .iter()
                .filter_map(|e| e.as_str().map(str::to_string))
                .collect(),
            goal_map: None,
        };
        s.goal_map = s.derive_goal_map();
        s.validate()?;
        Ok(s)
    }

    /// The canonical JSON, which is also what [`Scenario::hash`] hashes.
    pub fn to_json(&self) -> String {
        let mut w = json::Writer::new();
        w.begin_obj();
        w.str_field("schema", SCENARIO_SCHEMA);
        w.str_field("name", &self.name);
        w.str_field("objective", &self.objective);
        if let Some(s) = &self.start_state {
            w.str_field("start_state", &s.to_string_lossy());
        }
        w.arr_field("goal");
        for c in &self.goal {
            w.str_item(&c.source);
        }
        w.end_arr();
        w.arr_field("failure");
        for c in &self.failure {
            w.str_item(&c.source);
        }
        w.end_arr();
        w.arr_field("rules");
        for r in &self.rules {
            w.str_item(&r.source());
        }
        w.end_arr();
        w.arr_field("knowledge");
        for k in &self.knowledge {
            w.str_item(k.name());
        }
        w.end_arr();
        if let Some(g) = &self.guidance_file {
            w.str_field("guidance", &g.to_string_lossy());
        }
        w.num_field("timeout_frames", self.timeout_frames as f64);
        w.arr_field("watch");
        for watch in &self.watches.0 {
            watch.to_json(&mut w);
        }
        w.end_arr();
        // Written only when present, so a scenario that predates the field
        // hashes exactly as it always did and its recordings still replay.
        if !self.evidence.is_empty() {
            w.arr_field("evidence");
            for e in &self.evidence {
                w.str_item(e);
            }
            w.end_arr();
        }
        w.end_obj();
        w.finish()
    }

    /// Identity of the rules in force, carried in every recording so a replay
    /// can say "these rules have been edited since".
    pub fn hash(&self) -> u64 {
        fnv1a(self.to_json().as_bytes())
    }

    /// Refuse a scenario that cannot work, **before** anything is spent.
    ///
    /// The expensive mistakes: a goal nothing could satisfy (a thousand
    /// attempts is a thousand timeouts and a real bill), and a rule or
    /// condition on a watch nobody declared.
    pub fn validate(&self) -> Result<(), String> {
        if self.goal.is_empty() {
            return Err(format!(
                "scenario {:?} has no goal, so no run could ever reach one",
                self.name
            ));
        }
        if self.timeout_frames == 0 {
            return Err(format!("scenario {:?} has a zero timeout", self.name));
        }
        for c in self.goal.iter().chain(&self.failure) {
            if let Some(n) = c.needs_watch() {
                if self.watches.get(n).is_none() {
                    return Err(format!(
                        "condition {:?} needs a watch called {n:?}, which this \
                         scenario does not declare",
                        c.source
                    ));
                }
            }
        }
        Referee::new(self.rules.clone()).validate(&self.watches)?;
        // A privileged source that nothing asked for is a mistake worth naming;
        // a privileged source that *was* asked for is fine and is declared.
        if self.wants(Source::BattleAdvice) && self.battle_policy_is_flee() {
            return Err(format!(
                "scenario {:?} asks for battle advice and also forbids fighting — \
                 the advice could never be acted on",
                self.name
            ));
        }
        Ok(())
    }

    fn battle_policy_is_flee(&self) -> bool {
        Referee::new(self.rules.clone()).battle_policy() == rules::BattlePolicy::Flee
    }

    pub fn wants(&self, s: Source) -> bool {
        self.knowledge.contains(&s)
    }

    /// The map the goal names, when it names exactly one. This is what turns
    /// "reach Route 1" into a route the knowledge base can hand over.
    fn derive_goal_map(&self) -> Option<u8> {
        let mut found = None;
        for c in &self.goal {
            if let crate::longplay::episode::Lhs::Map = c.lhs {
                if c.op != crate::longplay::episode::Op::Eq {
                    continue;
                }
                if found.is_some() {
                    return None; // two of them: no single destination
                }
                found = Some(c.rhs as u8);
            }
        }
        found
    }

    pub fn goal_map(&self) -> Option<u8> {
        self.goal_map
    }

    /// Resolve a path in the scenario against the directory the file came from.
    pub fn resolve(&self, scenario_file: &Path, p: &Path) -> PathBuf {
        if p.is_absolute() {
            return p.to_path_buf();
        }
        scenario_file.parent().unwrap_or(Path::new(".")).join(p)
    }

    pub fn start_state_from(&self, scenario_file: &Path) -> Option<PathBuf> {
        Some(self.resolve(scenario_file, self.start_state.as_ref()?))
    }

    pub fn guidance_from(&self, scenario_file: &Path) -> Option<PathBuf> {
        Some(self.resolve(scenario_file, self.guidance_file.as_ref()?))
    }

    /// The [`Episode`] this scenario's goal machinery runs on.
    ///
    /// Deliberately a *conversion* rather than a parallel implementation: the
    /// reset-and-repeat loop, the condition grammar and the attempt report
    /// already exist and are tested, and a second copy of them would drift.
    /// `max_calls` comes from the `max_calls` rule when there is one, because a
    /// budget stated twice is a budget that disagrees with itself.
    pub fn episode(&self) -> Episode {
        let max_calls = self
            .rules
            .iter()
            .find_map(|r| match r {
                Rule::MaxCalls(n) => Some(*n),
                _ => None,
            })
            .unwrap_or(40);
        Episode {
            name: self.name.clone(),
            objective: self.objective.clone(),
            start_state: self.start_state.clone(),
            success: self.goal.clone(),
            failure: self.failure.clone(),
            timeout_frames: self.timeout_frames,
            max_calls,
            watches: self.watches.clone(),
        }
    }

    pub fn referee(&self) -> Referee {
        Referee::new(self.rules.clone())
    }

    /// Assemble the knowledge this scenario asked for, saying what it did and
    /// did not get. A source that was wanted and is missing is **reported**,
    /// never silently dropped — a run that planned without the atlas it asked
    /// for is a different run.
    pub fn knowledge_from(&self, scenario_file: &Path) -> Knowledge {
        let mut k = Knowledge::default();
        for s in &self.knowledge {
            match s {
                Source::Atlas => match Atlas::load() {
                    Some(a) => {
                        k.sources.push(format!("atlas ({} addresses)", a.len()));
                        k.atlas = Some(a);
                    }
                    None => k.missing.push(
                        "atlas — no atlas.tsv found; set $GB_ATLAS or run from the \
                         repository"
                            .into(),
                    ),
                },
                Source::Guidance => match self.guidance_from(scenario_file) {
                    Some(p) if p.is_file() => match Guidance::load(&p) {
                        Ok(g) => {
                            k.sources.push(format!(
                                "guidance ({} links, {} notes)",
                                g.links.len(),
                                g.notes.values().map(|v| v.len()).sum::<usize>()
                            ));
                            k.guidance = g;
                        }
                        Err(e) => k.missing.push(format!("guidance — {e}")),
                    },
                    Some(p) => {
                        // Not an error: the first run of a scenario has nothing
                        // to be guided by, and that is the normal beginning.
                        k.sources.push(format!(
                            "guidance (new, will be written to {})",
                            p.display()
                        ));
                    }
                    None => k.missing.push(
                        "guidance — the scenario asked for it but names no guidance \
                         file"
                            .into(),
                    ),
                },
                Source::BattleAdvice => {
                    k.sources.push("battle_advice (PRIVILEGED)".into());
                }
            }
        }
        k
    }

    pub fn load(path: &Path) -> Result<Scenario, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Scenario::from_json(&text)
    }

    /// What the run is about to do, in the words a person needs **before** it
    /// starts spending. Printed by every front end.
    pub fn briefing(&self, attempts: u32) -> Vec<String> {
        let r = self.referee();
        let mut v = vec![
            format!("scenario {:?}: {}", self.name, self.objective),
            format!(
                "goal: {}",
                self.goal
                    .iter()
                    .map(|c| c.source.as_str())
                    .collect::<Vec<_>>()
                    .join(" AND ")
            ),
        ];
        if !self.failure.is_empty() {
            v.push(format!(
                "fails on: {}",
                self.failure
                    .iter()
                    .map(|c| c.source.as_str())
                    .collect::<Vec<_>>()
                    .join(" OR ")
            ));
        }
        v.push(format!(
            "rules: {}",
            if self.rules.is_empty() {
                "none — nothing about this run is enforced".to_string()
            } else {
                self.rules
                    .iter()
                    .map(|r| r.source())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        let max_calls = self.episode().max_calls;
        v.push(format!(
            "cost ceiling: {attempts} attempt(s) × {max_calls} calls = {} model \
             calls worst case, {} emulated frames worst case",
            attempts as u64 * max_calls as u64,
            attempts as u64 * self.timeout_frames
        ));
        if self.knowledge.is_empty() {
            v.push("knowledge: none".into());
        } else {
            v.push(format!(
                "knowledge: {}{}",
                self.knowledge
                    .iter()
                    .map(|k| k.name())
                    .collect::<Vec<_>>()
                    .join(", "),
                if self.knowledge.iter().any(|k| k.is_privileged()) {
                    " — includes a PRIVILEGED source, so any result is a research \
                     measurement rather than a score"
                } else {
                    ""
                }
            ));
        }
        if r.battle_policy() != rules::BattlePolicy::Decide {
            v.push(format!("battles: {}", r.battle_policy().name()));
        }
        if self.evidence.is_empty() {
            v.push(
                "evidence: NONE STATED — nothing in this file says which address \
                 defines success, so a reader cannot check the goal is the game's \
                 own record of the milestone rather than somebody's idea of it"
                    .into(),
            );
        } else {
            for e in &self.evidence {
                v.push(format!("evidence: {e}"));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEAVE: &str = r#"{
      "schema": "gbscenario/1",
      "name": "leave-the-house",
      "objective": "Get out of Red's house.",
      "goal": ["map=0x00"],
      "failure": ["play_seconds>600"],
      "rules": ["no_memory_writes", "battle flee", "max_calls 20"],
      "knowledge": ["atlas", "guidance"],
      "guidance": "guidance/pallet.json",
      "timeout_frames": 40000
    }"#;

    #[test]
    fn a_scenario_round_trips_through_its_own_json() {
        let s = Scenario::from_json(LEAVE).unwrap();
        let back = Scenario::from_json(&s.to_json()).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.hash(), s.hash());
    }

    /// The hash is the identity of the *rules*, so editing one must move it —
    /// that is what lets a replay say "this is a different measurement now".
    #[test]
    fn editing_a_rule_moves_the_scenario_hash() {
        let a = Scenario::from_json(LEAVE).unwrap();
        let relaxed = LEAVE.replace("\"no_memory_writes\", ", "");
        let b = Scenario::from_json(&relaxed).unwrap();
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn a_wrong_schema_is_refused_rather_than_half_read() {
        assert!(Scenario::from_json("{\"schema\":\"gbscenario/9\"}").is_err());
    }

    /// The expensive mistake: a goal nothing satisfies means every attempt
    /// times out, and a thousand attempts is a thousand timeouts and a bill.
    #[test]
    fn a_scenario_with_no_goal_is_refused_before_it_costs_anything() {
        let s = LEAVE.replace("\"map=0x00\"", "");
        assert!(Scenario::from_json(&s).unwrap_err().contains("no goal"));
    }

    #[test]
    fn an_unknown_rule_or_source_is_named_rather_than_ignored() {
        let bad = LEAVE.replace("no_memory_writes", "no_cheating_honest");
        assert!(Scenario::from_json(&bad)
            .unwrap_err()
            .contains("no_cheating_honest"));
        let bad = LEAVE.replace("\"atlas\"", "\"telepathy\"");
        assert!(Scenario::from_json(&bad).unwrap_err().contains("telepathy"));
    }

    /// A budget stated twice is a budget that disagrees with itself, so the
    /// episode takes its call ceiling from the rule.
    #[test]
    fn the_call_ceiling_comes_from_the_rule_and_not_from_a_default() {
        let s = Scenario::from_json(LEAVE).unwrap();
        assert_eq!(s.episode().max_calls, 20);
        let no_rule = LEAVE.replace(", \"max_calls 20\"", "");
        assert_eq!(
            Scenario::from_json(&no_rule).unwrap().episode().max_calls,
            40
        );
    }

    /// "Reach map 0" is a destination the knowledge base can route to; "reach
    /// map 0 and map 1" is not, and guessing one would be worse than none.
    #[test]
    fn a_single_map_goal_becomes_a_destination_and_two_do_not() {
        assert_eq!(Scenario::from_json(LEAVE).unwrap().goal_map(), Some(0x00));
        let two = LEAVE.replace("[\"map=0x00\"]", "[\"map=0x00\",\"map=0x01\"]");
        assert_eq!(Scenario::from_json(&two).unwrap().goal_map(), None);
        let ranged = LEAVE.replace("map=0x00", "map>=1");
        assert_eq!(Scenario::from_json(&ranged).unwrap().goal_map(), None);
    }

    /// A scenario that asks for battle advice and forbids fighting is asking
    /// for something it could never act on. Caught before it spends anything.
    #[test]
    fn advice_that_could_never_be_acted_on_is_refused() {
        let bad = LEAVE
            .replace("\"atlas\", \"guidance\"", "\"battle_advice\"")
            .to_string();
        let e = Scenario::from_json(&bad).unwrap_err();
        assert!(e.contains("could never be acted on"), "{e}");
    }

    /// The captain's money. Every ceiling must be on screen before the first
    /// call, not discoverable afterwards from a bill.
    #[test]
    fn the_briefing_states_the_cost_ceiling_before_anything_is_spent() {
        let s = Scenario::from_json(LEAVE).unwrap();
        let b = s.briefing(50).join("\n");
        assert!(b.contains("1000 model calls"), "{b}");
        assert!(b.contains("no_memory_writes"), "{b}");
        assert!(b.contains("map=0x00"), "{b}");
    }

    /// A scenario with no rules is legal and must say so loudly — a reader
    /// skimming a briefing must not mistake "no rules" for "the usual rules".
    #[test]
    fn a_scenario_with_no_rules_says_nothing_is_enforced() {
        let s = LEAVE.replace(
            "\"rules\": [\"no_memory_writes\", \"battle flee\", \"max_calls 20\"],",
            "\"rules\": [],",
        );
        let b = Scenario::from_json(&s).unwrap().briefing(1).join("\n");
        assert!(b.contains("nothing about this run is enforced"), "{b}");
    }

    /// A privileged source changes what any number means, so the briefing has
    /// to say so before the run rather than the report saying so afterwards.
    #[test]
    fn a_privileged_source_is_announced_in_the_briefing() {
        let s = LEAVE
            .replace("\"atlas\", \"guidance\"", "\"atlas\", \"battle_advice\"")
            .replace("\"battle flee\", ", "");
        let b = Scenario::from_json(&s).unwrap().briefing(1).join("\n");
        assert!(b.contains("PRIVILEGED"), "{b}");
    }
}

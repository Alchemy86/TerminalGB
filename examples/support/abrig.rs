//! **The test rig's reusable half** — the plan, the statistics and the report.
//!
//! Reader-facing page: [`docs/agent/test-rig.md`](../../docs/agent/test-rig.md).
//!
//! Everything in here is pure: a plan is parsed from JSON, cells are filled in
//! by whoever is running the attempts, and a report is rendered. It knows
//! nothing about a Game Boy, a driver or a scenario, which is what lets it be
//! `#[path]`-included by any front end — the same sharing pattern
//! `examples/support/suite_baseline.rs` and `examples/support/gen1_cable_club.rs`
//! already use — and what lets its arithmetic be unit-tested with no cartridge.
//!
//! # Why an instrument rather than another script
//!
//! Every measurement this project has published — 300/300 on four emulators,
//! the 81-84 % damage agreement, the ablations that came back null — came from
//! code written once for that question and then left behind. The consequence is
//! not that the numbers are wrong; it is that **no two of them are strictly
//! comparable**, none is re-runnable by somebody who was not there, and every
//! new question costs a new harness. This is the harness, once, with the
//! question as its input.
//!
//! # The three things it is careful about
//!
//! 1. **Arms are paired.** Every arm plays the same entry states, the same
//!    seeds and the same phase offsets, so the comparison is within-subject and
//!    the right test is McNemar's on the discordant pairs, not two independent
//!    proportions. A rig that resampled per arm would need several times the
//!    attempts for the same resolution.
//! 2. **Subjects are never averaged.** A "student is 9 points behind the
//!    teacher" that pools two fights with different ceilings is a number about
//!    the mixture, not about either fight. Each subject gets its own row and
//!    its own gap.
//! 3. **The seen/unseen split is stated, loudly, on every report.** A
//!    distillation score quoted as evidence of generalisation would be the
//!    worst outcome this rig could produce, so it refuses to print a report
//!    that does not say which it is.

// The report renderer is in `pewter_chain.rs`, so a handful of accessors here
// are read only from there and one or two are the shape of the type rather than
// today's callers. Kept deliberately: this module is the reusable half.
#![allow(dead_code)]

use std::path::PathBuf;

use terminalgb_plugin_agent::json;

/// Which question a run is answering. **Required, and printed at the top of
/// every report.**
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Split {
    /// Trained on this fight, tested on this fight. Measures **how well the
    /// distillation worked** — nothing more.
    Seen,
    /// Trained on other movesets, other starters or other opponents; tested on
    /// this one. Measures whether the policy learned to *battle* or memorised a
    /// fight.
    ///
    /// Nothing in the published literature runs this control, because the
    /// competition's own benchmark is a fixed route: a memorising agent and a
    /// battling agent score identically there, so the question never arises.
    Unseen,
    /// No player was trained at all — the arms are the expert and its
    /// baselines. Reported as such rather than silently as "seen".
    NotApplicable,
}

impl Split {
    pub fn parse(s: &str) -> Option<Split> {
        match s {
            "seen" => Some(Split::Seen),
            "unseen" => Some(Split::Unseen),
            "n/a" | "na" | "none" => Some(Split::NotApplicable),
            _ => None,
        }
    }

    pub fn word(&self) -> &'static str {
        match self {
            Split::Seen => "SEEN",
            Split::Unseen => "UNSEEN",
            Split::NotApplicable => "NO TRAINED PLAYER",
        }
    }

    /// The sentence that goes under the banner. A reader who takes one line
    /// away from the report should take this one.
    pub fn meaning(&self) -> &'static str {
        match self {
            Split::Seen => {
                "the player under test was trained on this fight. This measures how \
                 well the distillation worked and is NOT evidence of generalisation."
            }
            Split::Unseen => {
                "the player under test was trained on a different fight. This measures \
                 whether it learned to battle or memorised one."
            }
            Split::NotApplicable => {
                "no trained player took part; these arms are the expert and its \
                 baselines, and the numbers are a ceiling and a floor."
            }
        }
    }
}

/// Who is pressing the buttons in one arm.
#[derive(Clone, Debug)]
pub enum Player {
    /// The scripted, memory-derived expert. The ceiling: a student distilled
    /// from it cannot exceed it, so the gap is the figure of merit rather than
    /// the absolute.
    Expert,
    /// Six buttons, uniformly. The floor. Without it a rate has no scale — a
    /// scenario a random walk solves half the time is not evidence of anything.
    Random { seed_base: u64 },
    /// A subprocess speaking the agent's JSON protocol
    /// (`plugins/agent/src/subprocess.rs`). **This is the seam a trained player
    /// arrives through, and it is deliberately the only one**: the student
    /// lives in another repository and must not be copied into this one.
    Command { program: String, args: Vec<String> },
}

impl Player {
    pub fn kind(&self) -> &'static str {
        match self {
            Player::Expert => "expert",
            Player::Random { .. } => "random",
            Player::Command { .. } => "command",
        }
    }
}

/// One column of the report.
#[derive(Clone, Debug)]
pub struct ArmSpec {
    pub name: String,
    pub player: Player,
}

/// One row of the report: a fight held constant while the arms vary.
///
/// Today a subject is a lead species, because that is the axis the first
/// question needed. It is a struct rather than a `u8` so that "a different
/// opponent" or "a different moveset" is a field rather than a rewrite.
#[derive(Clone, Debug)]
pub struct Subject {
    pub name: String,
    /// Keep only entry states whose lead is this **internal index number**.
    /// `None` keeps every state in the pool.
    pub lead_species: Option<u8>,
}

/// A test, as a file somebody else can re-run.
#[derive(Clone, Debug)]
pub struct Plan {
    pub name: String,
    /// What this test is for, in one sentence, printed at the top. A rig that
    /// does not make you write this produces tables nobody can interpret six
    /// months later.
    pub question: String,
    pub split: Split,
    pub scenario: PathBuf,
    pub pool: PathBuf,
    pub subjects: Vec<Subject>,
    pub arms: Vec<ArmSpec>,
    pub runs: u32,
    pub seed0: u64,
    pub idle_spread: u32,
    pub workers: u32,
}

impl Plan {
    /// Parse a `gbtestrig/1` plan.
    pub fn parse(text: &str) -> Result<Plan, String> {
        let v = json::parse(text).map_err(|e| format!("{e:?}"))?;
        let schema = v.get("schema").and_then(|s| s.as_str()).unwrap_or("");
        if schema != "gbtestrig/1" {
            return Err(format!(
                "expected \"schema\": \"gbtestrig/1\", found {schema:?}"
            ));
        }
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|x| x.to_string());
        let n = |k: &str, d: u64| v.get(k).and_then(|x| x.as_u64()).unwrap_or(d);
        let split_word = s("split").ok_or(
            "a plan must state \"split\": \"seen\" | \"unseen\" | \"n/a\" — a \
             distillation score quoted as generalisation is the worst thing this \
             rig could produce, so it is not optional",
        )?;
        let split = Split::parse(&split_word)
            .ok_or_else(|| format!("unknown split {split_word:?}"))?;
        let mut subjects = Vec::new();
        if let Some(arr) = v.get("subjects").and_then(|x| x.as_array()) {
            for it in arr {
                let name = it
                    .get("name")
                    .and_then(|x| x.as_str())
                    .ok_or("every subject needs a \"name\"")?;
                let lead = it.get("lead_species").and_then(|x| x.as_u64());
                subjects.push(Subject {
                    name: name.to_string(),
                    lead_species: lead.map(|x| x as u8),
                });
            }
        }
        let mut arms = Vec::new();
        for it in v
            .get("arms")
            .and_then(|x| x.as_array())
            .ok_or("a plan needs an \"arms\" array")?
        {
            let name = it
                .get("name")
                .and_then(|x| x.as_str())
                .ok_or("every arm needs a \"name\"")?
                .to_string();
            let kind = it
                .get("player")
                .and_then(|x| x.as_str())
                .unwrap_or("expert");
            let player = match kind {
                "expert" => Player::Expert,
                "random" => Player::Random {
                    seed_base: it
                        .get("seed_base")
                        .and_then(|x| x.as_u64())
                        .unwrap_or(0x9E37_79B9),
                },
                "command" => {
                    let program = it
                        .get("program")
                        .and_then(|x| x.as_str())
                        .ok_or("a \"command\" arm needs a \"program\"")?
                        .to_string();
                    let args = it
                        .get("args")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default();
                    Player::Command { program, args }
                }
                other => return Err(format!("unknown player {other:?}")),
            };
            arms.push(ArmSpec { name, player });
        }
        Ok(Plan {
            name: s("name").unwrap_or_else(|| "unnamed".into()),
            question: s("question").unwrap_or_default(),
            split,
            scenario: PathBuf::from(s("scenario").ok_or("a plan needs a \"scenario\"")?),
            pool: PathBuf::from(s("entry_pool").ok_or("a plan needs an \"entry_pool\"")?),
            subjects,
            arms,
            runs: n("runs", 200) as u32,
            seed0: n("seed0", 0),
            idle_spread: n("idle_spread", 3001) as u32,
            workers: n("workers", 0) as u32,
        })
    }
}

/// One attempt's result, as the report needs it.
#[derive(Clone, Debug, Default)]
pub struct Attempt {
    pub seed: u64,
    pub goal: bool,
    pub ending: String,
    pub frames: u64,
    pub decisions: u32,
    pub battles: u32,
    pub won: u32,
    pub fled: u32,
    pub lost: u32,
    pub frames_in_battle: u64,
    pub hp_permille: u64,
    pub hp_samples: u32,
    /// The four moves the lead held when the attempt ended, in slot order.
    /// Zero is an empty slot, which is a fact rather than a missing value.
    pub moves: [u8; 4],
    /// Whether [`Attempt::moves`] matches the entry's stated expectation.
    /// `None` when the entry carried none.
    pub correct: Option<bool>,
}

/// Which of an attempt's outcomes a rate or a gap is about.
///
/// **A scenario's pass rate and the battling underneath it are different
/// questions, and the first can be blind to the second.** Measured on the very
/// first subject this rig was pointed at: link 05's goal is that the rival
/// battle *happened* — the cartridge sets its flag whoever won and refuses to
/// black the player out — so the expert passes 100 % of attempts with all three
/// starters while losing that battle 0 %, 50 % and 17 % of the time. A rig that
/// reported only the goal would have said the three starters were identical.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// The scenario's own goal condition.
    Goal,
    /// Every battle the attempt had, it won. `None` — and so excluded from the
    /// paired comparison — for an attempt that had no battle, because "did not
    /// fight" is not "lost".
    BattleWon,
    /// The four moves the lead was holding at the end are the ones the entry
    /// state's expectation says it should be holding.
    ///
    /// **This is the unit for a move-learning test.** A level-up prompt has one
    /// right answer, stated by whoever built the pool, and the only thing that
    /// can be observed from outside is what the party looks like afterwards —
    /// which is exactly what a player's choice writes. `None` for an entry with
    /// no expectation beside it, so a pool that is half annotated measures only
    /// the annotated half rather than scoring the rest as wrong.
    PromptCorrect,
}

impl Outcome {
    pub fn label(&self) -> &'static str {
        match self {
            Outcome::Goal => "goal",
            Outcome::BattleWon => "battle won",
            Outcome::PromptCorrect => "prompt answered correctly",
        }
    }

    pub fn of(&self, a: &Attempt) -> Option<bool> {
        match self {
            Outcome::Goal => Some(a.goal),
            Outcome::BattleWon => {
                if a.battles == 0 {
                    None
                } else {
                    Some(a.won == a.battles)
                }
            }
            Outcome::PromptCorrect => a.correct,
        }
    }
}

/// Where one cell's wall clock went.
///
/// **The captain's standing condition on this rig is that it is only worth
/// having if it is fast**, so it says which part to attack rather than leaving
/// anyone to guess. Three buckets that add up to the wall clock of the attempts
/// themselves:
///
/// * `emulate_ms` — inside [`terminalgb::gameboy::Gameboy::frame`], measured by
///   the run loop itself.
/// * `setup_ms` — building the machine, burning the phase offset and loading
///   the start state, per attempt. This is the tax an *episode* pays, and it is
///   the number that decides whether short episodes are affordable.
/// * the remainder — the run loop, the referee and the plugin dispatch.
///
/// **The remainder is NOT the player's think time, and reading it as such is
/// the trap.** The emulator never blocks on a driver (that is the whole of
/// `plugins/agent`'s design), so a subprocess arm's spawn-and-decide happens
/// while the machine keeps running and is charged to `emulate_ms`. A slow
/// player shows up as **more frames per attempt** — the world runs on while it
/// thinks — not as a bigger share here. `frames` per attempt is therefore the
/// figure to watch for a student, and the shares say whether the rig itself is
/// in the way.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timing {
    pub attempts: u32,
    pub wall_ms: u64,
    pub emulate_ms: u64,
    pub setup_ms: u64,
    pub frames: u64,
}

impl Timing {
    /// Wall clock this cell's attempts spent neither emulating nor setting up.
    pub fn other_ms(&self) -> u64 {
        self.wall_ms
            .saturating_sub(self.emulate_ms)
            .saturating_sub(self.setup_ms)
    }

    /// Share of the attempts' wall clock, as a percentage. Zero attempts is
    /// zero rather than a division by zero.
    pub fn share(&self, part: u64) -> f64 {
        if self.wall_ms == 0 {
            return 0.0;
        }
        100.0 * part as f64 / self.wall_ms as f64
    }
}

/// Everything one (subject, arm) pair produced.
#[derive(Clone, Debug, Default)]
pub struct Cell {
    pub attempts: Vec<Attempt>,
    pub timing: Timing,
}

impl Cell {
    pub fn n(&self) -> u32 {
        self.attempts.len() as u32
    }
    pub fn goals(&self) -> u32 {
        self.attempts.iter().filter(|a| a.goal).count() as u32
    }
    pub fn rate(&self) -> f64 {
        if self.attempts.is_empty() {
            return 0.0;
        }
        self.goals() as f64 / self.n() as f64
    }
    /// Successes and the attempts that could have one, for an outcome. An
    /// attempt the outcome does not apply to is in neither.
    pub fn tally(&self, o: Outcome) -> (u32, u32) {
        let mut k = 0;
        let mut n = 0;
        for a in &self.attempts {
            if let Some(v) = o.of(a) {
                n += 1;
                if v {
                    k += 1;
                }
            }
        }
        (k, n)
    }
    fn sum(&self, f: impl Fn(&Attempt) -> u64) -> u64 {
        self.attempts.iter().map(f).sum()
    }
    pub fn battles(&self) -> u64 {
        self.sum(|a| a.battles as u64)
    }
    pub fn won(&self) -> u64 {
        self.sum(|a| a.won as u64)
    }
    pub fn fled(&self) -> u64 {
        self.sum(|a| a.fled as u64)
    }
    pub fn lost(&self) -> u64 {
        self.sum(|a| a.lost as u64)
    }
    /// Mean HP the lead had left when a battle ended, as a percentage.
    pub fn hp_left(&self) -> f64 {
        let s = self.sum(|a| a.hp_samples as u64);
        if s == 0 {
            return 0.0;
        }
        self.sum(|a| a.hp_permille) as f64 / s as f64 / 10.0
    }
    pub fn mean_decisions(&self) -> f64 {
        if self.attempts.is_empty() {
            return 0.0;
        }
        self.sum(|a| a.decisions as u64) as f64 / self.n() as f64
    }
    pub fn mean_frames(&self) -> f64 {
        if self.attempts.is_empty() {
            return 0.0;
        }
        self.sum(|a| a.frames) as f64 / self.n() as f64
    }
    /// Prompts this cell answered, and how many were right.
    pub fn prompts(&self) -> (u32, u32) {
        self.tally(Outcome::PromptCorrect)
    }

    /// On how many prompts did this cell and `other` leave the party holding
    /// **the same four moves**, and over how many pairs.
    ///
    /// **This is the question a distillation is really asking**, and it is not
    /// the same as both being right: two players can each be wrong and wrong in
    /// the same way, which is a fact about the student having learnt the
    /// teacher's mistakes and is worth seeing. Paired on the seed, and only
    /// over attempts that carried an expectation, so it counts prompts rather
    /// than whatever else the pool contains.
    pub fn agreement(&self, other: &Cell) -> (u32, u32) {
        let mine: std::collections::HashMap<u64, [u8; 4]> = self
            .attempts
            .iter()
            .filter(|a| a.correct.is_some())
            .map(|a| (a.seed, a.moves))
            .collect();
        let (mut same, mut pairs) = (0, 0);
        for a in &other.attempts {
            if a.correct.is_none() {
                continue;
            }
            if let Some(m) = mine.get(&a.seed) {
                pairs += 1;
                if *m == a.moves {
                    same += 1;
                }
            }
        }
        (same, pairs)
    }

    /// Endings by name, commonest first — the histogram that says *how* an arm
    /// failed, which a rate cannot.
    pub fn endings(&self) -> Vec<(String, u32)> {
        let mut m: std::collections::HashMap<String, u32> = Default::default();
        for a in &self.attempts {
            *m.entry(a.ending.clone()).or_default() += 1;
        }
        let mut v: Vec<(String, u32)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }
}

/// The Wilson score interval, at 95 %.
///
/// **Not the normal approximation**, which is the one everybody reaches for and
/// which is indefensible exactly where this rig lives: at 200 attempts and a
/// rate near 1.0 it produces an upper bound above 100 %, and at a rate of 0 it
/// produces the interval `[0, 0]`, i.e. certainty from no evidence. Wilson is
/// bounded, never empty, and behaves at the ends.
pub fn wilson(k: u32, n: u32) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let z = 1.959_963_984_540_054_f64;
    let n = n as f64;
    let p = k as f64 / n;
    let d = 1.0 + z * z / n;
    let c = p + z * z / (2.0 * n);
    let half = z * ((p * (1.0 - p) / n) + (z * z / (4.0 * n * n))).sqrt();
    (((c - half) / d).max(0.0), ((c + half) / d).min(1.0))
}

/// Two-sided exact binomial p for McNemar's test on `b` and `c` discordant
/// pairs.
///
/// **The arms play the same entries, seeds and phases, so the pairing is
/// real** and throwing it away for two independent proportions would waste most
/// of the sample. Only the pairs where the two arms disagreed carry any
/// information; a pair both won or both lost says nothing about which is
/// better. The exact form is used rather than the chi-square because `b + c`
/// here is routinely under twenty-five.
pub fn mcnemar_p(b: u32, c: u32) -> f64 {
    let n = b + c;
    if n == 0 {
        return 1.0;
    }
    // P(X <= min(b,c)) * 2 under Binomial(n, 1/2), clamped at 1.
    let k = b.min(c);
    let mut acc = 0.0f64;
    // log-space so a 3,000-pair run does not overflow the factorials.
    let lg = |x: u32| -> f64 { ln_gamma(x as f64 + 1.0) };
    for i in 0..=k {
        let log_c = lg(n) - lg(i) - lg(n - i);
        acc += (log_c - (n as f64) * std::f64::consts::LN_2).exp();
    }
    (2.0 * acc).min(1.0)
}

/// Lanczos approximation, good to ~1e-13 over the range a binomial coefficient
/// here needs. Hand-rolled like every other numeric in this repository: the
/// dependency count is a contract `make embed` measures.
fn ln_gamma(x: f64) -> f64 {
    const G: [f64; 9] = [
        0.999_999_999_999_809_9,
        676.520_368_121_885_1,
        -1_259.139_216_722_402_8,
        771.323_428_777_653_1,
        -176.615_029_162_140_6,
        12.507_343_278_686_905,
        -0.138_571_095_265_720_12,
        9.984_369_578_019_572e-6,
        1.505_632_735_149_311_6e-7,
    ];
    if x < 0.5 {
        return (std::f64::consts::PI / (std::f64::consts::PI * x).sin()).ln()
            - ln_gamma(1.0 - x);
    }
    let x = x - 1.0;
    let mut a = G[0];
    let t = x + 7.5;
    for (i, g) in G.iter().enumerate().skip(1) {
        a += g / (x + i as f64);
    }
    0.5 * (2.0 * std::f64::consts::PI).ln() + (x + 0.5) * t.ln() - t + a.ln()
}

/// A paired difference between two arms on one subject.
#[derive(Clone, Debug)]
pub struct Gap {
    /// Points, `reference - arm`, so a positive gap is the arm falling short.
    pub points: f64,
    /// Pairs the reference won and the arm lost.
    pub only_reference: u32,
    /// Pairs the arm won and the reference lost.
    pub only_arm: u32,
    pub p: f64,
    /// Standard error of the paired difference, from the discordant counts.
    pub se: f64,
    /// Pairs the comparison actually used. Smaller than the attempt count when
    /// an outcome did not apply to every attempt — a battle gap over a link
    /// where some attempts never fought, for instance.
    pub pairs: u32,
}

impl Gap {
    /// 95 % interval on the gap, in points. Normal approximation on the paired
    /// difference — reported with its `n` so a reader can see when it is thin.
    pub fn interval(&self) -> (f64, f64) {
        let h = 1.959_963_984_540_054 * self.se;
        (self.points - h, self.points + h)
    }
}

/// Compare two cells that played the same seeds.
///
/// Returns `None` when the two did not in fact play the same seeds — which is
/// a bug in the runner, not a result, so it is refused rather than approximated
/// by an unpaired comparison.
pub fn gap(reference: &Cell, arm: &Cell) -> Option<Gap> {
    gap_on(reference, arm, Outcome::Goal)
}

/// The same, for any outcome. Attempts the outcome does not apply to on either
/// side are dropped from the pairing rather than counted as failures.
pub fn gap_on(reference: &Cell, arm: &Cell, o: Outcome) -> Option<Gap> {
    if reference.n() != arm.n() || reference.n() == 0 {
        return None;
    }
    let mut by_seed: std::collections::HashMap<u64, Option<bool>> = Default::default();
    for a in &reference.attempts {
        by_seed.insert(a.seed, o.of(a));
    }
    let (mut b, mut c) = (0u32, 0u32);
    let (mut rk, mut ak, mut pairs) = (0u32, 0u32, 0u32);
    for a in &arm.attempts {
        let Some(Some(r)) = by_seed.get(&a.seed).copied() else {
            // Either the seed is missing — a runner bug, refused below — or
            // the reference attempt had no outcome of this kind.
            if !by_seed.contains_key(&a.seed) {
                return None;
            }
            continue;
        };
        let Some(v) = o.of(a) else { continue };
        pairs += 1;
        if r {
            rk += 1;
        }
        if v {
            ak += 1;
        }
        match (r, v) {
            (true, false) => b += 1,
            (false, true) => c += 1,
            _ => {}
        }
    }
    if pairs == 0 {
        return None;
    }
    let n = pairs as f64;
    let points = (rk as f64 - ak as f64) / n * 100.0;
    // **`sqrt(b + c) / n`, not the corrected `(b + c - (b-c)^2/n) / n^2`.** The
    // corrected form is the maximum-likelihood variance and it *degenerates*
    // exactly where this rig is most likely to be read carelessly: when every
    // discordant pair falls the same way it evaluates to zero, so a run of two
    // attempts prints a hundred-point gap with an interval of zero width, which
    // looks like certainty and is an artefact of the estimator. The
    // conventional McNemar standard error is never zero while a discordant pair
    // exists, is slightly conservative, and cannot manufacture confidence.
    let se = ((b + c) as f64).sqrt() / n;
    Some(Gap {
        points,
        only_reference: b,
        only_arm: c,
        p: mcnemar_p(b, c),
        se: se * 100.0,
        pairs,
    })
}

/// Do two subjects' gaps differ, or is the difference inside the noise?
///
/// **This is the captain's question about the species pair and it deserves its
/// own answer rather than an eyeball on two intervals.** If the gap is the same
/// on two fights whose absolute rates differ, the distillation is
/// species-independent and the fights merely have different ceilings. If the
/// gap itself moves, something species-specific is happening in the player
/// under test — a different and more interesting finding. The two subjects are
/// independent samples, so the difference of the two paired gaps has variance
/// equal to the sum of theirs.
pub fn gaps_differ(a: &Gap, b: &Gap) -> (f64, f64, f64) {
    let d = a.points - b.points;
    let se = (a.se * a.se + b.se * b.se).sqrt();
    let z = if se > 0.0 { d / se } else { 0.0 };
    (d, se, two_sided_normal_p(z))
}

/// Two-sided p from a z score, via an Abramowitz-and-Stegun erf.
pub fn two_sided_normal_p(z: f64) -> f64 {
    let x = z.abs() / std::f64::consts::SQRT_2;
    // 7.1.26, |error| < 1.5e-7.
    let t = 1.0 / (1.0 + 0.327_591_1 * x);
    let y = 1.0
        - (((((1.061_405_429 * t - 1.453_152_027) * t) + 1.421_413_741) * t
            - 0.284_496_736)
            * t
            + 0.254_829_592)
            * t
            * (-x * x).exp();
    (1.0 - y).clamp(0.0, 1.0)
}

/// What the pool says the right answer to one prompt is.
///
/// **The corpus contract between this rig and whoever produces the prompts**,
/// and it is deliberately tiny: a save state, and beside it a file naming the
/// four moves the lead should be holding once the prompt has been answered.
/// Everything else — the drill that triggered the level-up, the criterion that
/// decided the answer, the state-and-action pairs the student was distilled
/// from — belongs to the producer and never touches this rig.
///
/// `<pool>/00042.gbss` and `<pool>/00042.expect.json`:
///
/// ```json
/// {
///   "schema": "gbtestprompt/1",
///   "species": 176,
///   "level": 13,
///   "held": [10, 45, 52, 0],
///   "offered": 43,
///   "correct_moves": [10, 45, 52, 43],
///   "why": "the offered move goes into the empty slot; nothing is forgotten"
/// }
/// ```
///
/// Only `correct_moves` is load-bearing; the rest is provenance, carried so a
/// row of the report can be read without opening the producer's own notes.
/// **An entry with no file beside it is not scored on this outcome** — a pool
/// that is half annotated measures its annotated half rather than marking the
/// rest wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub species: u8,
    pub level: u8,
    pub held: [u8; 4],
    pub offered: u8,
    pub correct_moves: [u8; 4],
    pub why: String,
}

impl Expectation {
    pub fn parse(text: &str) -> Result<Expectation, String> {
        let v = json::parse(text).map_err(|e| format!("{e:?}"))?;
        let schema = v.get("schema").and_then(|x| x.as_str()).unwrap_or("");
        if schema != "gbtestprompt/1" {
            return Err(format!(
                "expected \"schema\": \"gbtestprompt/1\", found {schema:?}"
            ));
        }
        let four = |key: &str| -> Result<[u8; 4], String> {
            let a = v
                .get(key)
                .and_then(|x| x.as_array())
                .ok_or_else(|| format!("{key} must be an array of four move ids"))?;
            if a.len() != 4 {
                return Err(format!(
                    "{key} has {} entries; a Gen 1 Pokémon holds exactly four \
                     move slots and an empty one is 0",
                    a.len()
                ));
            }
            let mut out = [0u8; 4];
            for (i, x) in a.iter().enumerate() {
                out[i] = x
                    .as_u64()
                    .ok_or_else(|| format!("{key}[{i}] is not a number"))?
                    as u8;
            }
            Ok(out)
        };
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0) as u8;
        Ok(Expectation {
            species: n("species"),
            level: n("level"),
            held: four("held").unwrap_or_default(),
            offered: n("offered"),
            correct_moves: four("correct_moves")?,
            why: v
                .get("why")
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string(),
        })
    }

    /// **Slot order matters and that is deliberate.** Gen 1 replaces a
    /// forgotten move in place, so "which slot the new move went into" is part
    /// of the answer and a set comparison would score a wrong choice as right
    /// whenever the multiset happened to match.
    pub fn satisfied_by(&self, moves: &[u8; 4]) -> bool {
        &self.correct_moves == moves
    }
}

/// A live progress reporter, shared by the workers and read by a ticker.
///
/// **On by default, and that is the point.** A 2,700-attempt run that prints
/// nothing until it finishes is indistinguishable from a wedged one — a real
/// hour went on checking process tables to find out whether a silent run was
/// alive. It reports to **stderr** so stdout stays the report, and it appends
/// the identical tick to `<out>/progress.ndjson` so that something other than a
/// person can follow it, which is what this rig is for.
#[derive(Default)]
pub struct Live {
    pub total: u32,
    pub done: std::sync::atomic::AtomicU32,
    pub cell_total: std::sync::atomic::AtomicU32,
    pub cell_done: std::sync::atomic::AtomicU32,
    pub label: std::sync::Mutex<String>,
    pub stop: std::sync::atomic::AtomicBool,
}

impl Live {
    pub fn new(total: u32) -> Live {
        Live {
            total,
            ..Default::default()
        }
    }

    pub fn begin(&self, label: &str, cell_total: u32) {
        use std::sync::atomic::Ordering::Relaxed;
        *self.label.lock().unwrap() = label.to_string();
        self.cell_total.store(cell_total, Relaxed);
        self.cell_done.store(0, Relaxed);
    }

    pub fn tick(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.done.fetch_add(1, Relaxed);
        self.cell_done.fetch_add(1, Relaxed);
    }

    /// The tick, in both shapes: **TOON for stdout** and JSON for the file.
    /// `elapsed` is seconds since the run began.
    ///
    /// The TOON block is a one-row table rather than the object form, because a
    /// tick is a *sample* and a stream of samples is a collection: an agent
    /// tailing stdout gets the same `progress[1]{…}` header every time and can
    /// parse any one of them on its own. Two lines a tick is the price of the
    /// run being followable at all — see `docs/agent/test-rig.md` §4a.
    ///
    /// The estimate is a plain linear extrapolation of attempts completed, and
    /// it is **labelled an estimate** because the cells are not equally
    /// expensive: an expert attempt is ~65 decisions and a subprocess arm's is
    /// its whole budget, so the figure moves as the run crosses a cell
    /// boundary. Better would be per-cell rates, which is a refinement nobody
    /// has needed yet.
    pub fn lines(&self, elapsed: f64) -> (String, String) {
        use std::sync::atomic::Ordering::Relaxed;
        let done = self.done.load(Relaxed);
        let (cd, ct) = (self.cell_done.load(Relaxed), self.cell_total.load(Relaxed));
        let label = self.label.lock().unwrap().clone();
        let pct = if self.total > 0 {
            100.0 * done as f64 / self.total as f64
        } else {
            0.0
        };
        let eta = if done > 0 {
            elapsed * (self.total.saturating_sub(done)) as f64 / done as f64
        } else {
            f64::NAN
        };
        let eta_s = if eta.is_finite() {
            hms(eta)
        } else {
            "?".to_string()
        };
        let cell = if label.is_empty() { "-" } else { &label };
        let toon = format!(
            "progress[1]{{done,total,pct,cell,cell_done,cell_total,elapsed,eta}}:\n\
             \x20 {done},{},{pct:.1},{cell},{cd},{ct},{},{eta_s}",
            self.total,
            hms(elapsed),
        );
        let json = format!(
            "{{\"elapsed_s\":{elapsed:.1},\"done\":{done},\"total\":{},\
             \"cell\":{label:?},\"cell_done\":{cd},\"cell_total\":{ct},\
             \"eta_s\":{}}}",
            self.total,
            if eta.is_finite() {
                format!("{eta:.1}")
            } else {
                "null".into()
            }
        );
        (toon, json)
    }
}

/// `1h02m03s`, `4m07s`, `9s` — short enough to sit in a progress line.
pub fn hms(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}h{:02}m{:02}s", s / 3600, (s % 3600) / 60, s % 60)
    } else if s >= 60 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Format a proportion and its Wilson interval the same way everywhere.
pub fn pct_ci(k: u32, n: u32) -> String {
    if n == 0 {
        return "     —".into();
    }
    let (lo, hi) = wilson(k, n);
    format!(
        "{:5.1}% [{:.1}-{:.1}]",
        100.0 * k as f64 / n as f64,
        lo * 100.0,
        hi * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The interval that made Wilson worth hand-rolling: a perfect score must
    /// not claim an upper bound above 100 %, and it must not claim certainty.
    #[test]
    fn a_perfect_score_still_has_a_lower_bound_below_one() {
        let (lo, hi) = wilson(200, 200);
        assert!(hi >= 0.999_9, "upper bound should reach 1.0, got {hi}");
        assert!(
            lo > 0.97 && lo < 1.0,
            "lower bound should be near but under 1: {lo}"
        );
        let (lo, hi) = wilson(0, 200);
        assert!(lo <= 0.000_1 && hi > 0.0 && hi < 0.05, "[{lo},{hi}]");
    }

    /// Sanity against a figure this repository already publishes: 2,999 of
    /// 3,000 is the certified link-11 rate.
    #[test]
    fn wilson_matches_a_published_rate() {
        let (lo, hi) = wilson(2999, 3000);
        assert!((lo * 100.0 - 99.81).abs() < 0.05, "{lo}");
        assert!((hi * 100.0 - 99.99).abs() < 0.05, "{hi}");
    }

    /// McNemar against values that can be checked by hand: with one discordant
    /// pair the two-sided exact p is exactly 1, and with ten against zero it is
    /// `2 * 2^-10`.
    #[test]
    fn mcnemar_agrees_with_arithmetic_that_can_be_done_on_paper() {
        assert!((mcnemar_p(1, 0) - 1.0).abs() < 1e-12);
        assert!((mcnemar_p(0, 0) - 1.0).abs() < 1e-12);
        let expect = 2.0 * 0.5_f64.powi(10);
        assert!(
            (mcnemar_p(10, 0) - expect).abs() < 1e-12,
            "{}",
            mcnemar_p(10, 0)
        );
        assert!((mcnemar_p(5, 5) - 1.0).abs() < 1e-9);
        // The case that actually exercises the binomial coefficients rather
        // than their cancellation: n = 10, k = 2, so
        // 2 * (C(10,0)+C(10,1)+C(10,2)) / 2^10 = 2 * 56 / 1024.
        let expect = 2.0 * 56.0 / 1024.0;
        assert!(
            (mcnemar_p(8, 2) - expect).abs() < 1e-9,
            "{} against {expect}",
            mcnemar_p(8, 2)
        );
        // And one large enough that the factorials would overflow if they were
        // not computed in log space.
        assert!(mcnemar_p(300, 0) < 1e-80, "{}", mcnemar_p(300, 0));
        assert!(mcnemar_p(300, 0) > 0.0, "underflowed to zero");
    }

    /// **A gap between arms that did not play the same seeds is refused.**
    /// Teeth: the whole design rests on pairing, and quietly falling back to an
    /// unpaired comparison would produce a number that looks the same and means
    /// something else.
    #[test]
    fn a_gap_is_refused_when_the_arms_did_not_play_the_same_seeds() {
        let cell = |seeds: &[(u64, bool)]| Cell {
            attempts: seeds
                .iter()
                .map(|&(seed, goal)| Attempt {
                    seed,
                    goal,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let a = cell(&[(1, true), (2, true), (3, false)]);
        let b = cell(&[(1, true), (2, false), (3, false)]);
        let g = gap(&a, &b).expect("same seeds");
        assert_eq!((g.only_reference, g.only_arm), (1, 0));
        assert!((g.points - 33.333).abs() < 0.01, "{}", g.points);

        let elsewhere = cell(&[(9, true), (8, true), (7, false)]);
        assert!(
            gap(&a, &elsewhere).is_none(),
            "different seeds must be refused, not compared unpaired"
        );
    }

    /// **The pass rate can be blind to the fight underneath it, and that is
    /// what a second outcome is for.** Teeth: both arms below pass every
    /// attempt, so the goal gap is exactly zero and says the two are identical;
    /// on the battle they are forty points apart. An attempt with no battle is
    /// in neither column — "did not fight" is not "lost".
    #[test]
    fn a_pass_rate_can_be_blind_to_the_battle_under_it() {
        let cell = |won: &[bool]| Cell {
            attempts: won
                .iter()
                .enumerate()
                .map(|(i, &w)| Attempt {
                    seed: i as u64,
                    goal: true,
                    battles: 1,
                    won: w as u32,
                    lost: !w as u32,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let teacher = cell(&[true, true, true, true, true]);
        let pupil = cell(&[true, true, true, false, false]);
        let g = gap(&teacher, &pupil).expect("paired");
        assert_eq!(g.points, 0.0, "the goal cannot see it");
        let b = gap_on(&teacher, &pupil, Outcome::BattleWon).expect("paired");
        assert!((b.points - 40.0).abs() < 0.01, "{}", b.points);
        assert_eq!((b.only_reference, b.only_arm, b.pairs), (2, 0, 5));

        // An attempt that never fought is excluded from the battle pairing.
        let mut quiet = teacher.clone();
        quiet.attempts[0].battles = 0;
        quiet.attempts[0].won = 0;
        let mut quiet_pupil = pupil.clone();
        quiet_pupil.attempts[0].battles = 0;
        quiet_pupil.attempts[0].won = 0;
        let b = gap_on(&quiet, &quiet_pupil, Outcome::BattleWon).expect("paired");
        assert_eq!(
            b.pairs, 4,
            "the attempt with no battle is in neither column"
        );
    }

    /// **A prompt is scored on the four moves the party is left holding, in
    /// slot order.** Gen 1 replaces a forgotten move in place, so which slot the
    /// new move went into is part of the answer — teeth: the "wrong slot" case
    /// below holds exactly the right multiset and must still be wrong.
    #[test]
    fn a_prompt_is_answered_in_slot_order_not_as_a_set() {
        let e = Expectation::parse(
            r#"{"schema":"gbtestprompt/1","species":176,"level":13,
                "held":[10,45,52,0],"offered":43,
                "correct_moves":[10,45,52,43],
                "why":"the offered move goes into the empty slot"}"#,
        )
        .expect("parses");
        assert!(e.satisfied_by(&[10, 45, 52, 43]));
        assert!(
            !e.satisfied_by(&[43, 45, 52, 10]),
            "the same four moves in the wrong slots is a different answer"
        );
        assert!(
            !e.satisfied_by(&[10, 45, 52, 0]),
            "declining is not the answer here"
        );
        assert_eq!((e.species, e.level, e.offered), (176, 13, 43));
    }

    /// A pool that is only half annotated measures its annotated half. Scoring
    /// the rest as wrong would report a rate about the pool's bookkeeping.
    #[test]
    fn an_entry_with_no_expected_answer_is_in_neither_column() {
        let c = Cell {
            attempts: vec![
                Attempt {
                    seed: 0,
                    correct: Some(true),
                    ..Default::default()
                },
                Attempt {
                    seed: 1,
                    correct: Some(false),
                    ..Default::default()
                },
                Attempt {
                    seed: 2,
                    correct: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        assert_eq!(c.prompts(), (1, 2));
    }

    /// **Agreeing with the teacher and being right are different questions.**
    /// Both players below answer the second prompt the same wrong way, so they
    /// agree on two of two while each scores one of two — which is exactly what
    /// a student that learnt its teacher's mistake looks like, and a report
    /// that only carried correctness could not show it.
    #[test]
    fn agreeing_with_the_teacher_is_not_the_same_as_being_right() {
        let mk = |moves: [[u8; 4]; 2], right: [bool; 2]| Cell {
            attempts: (0..2)
                .map(|i| Attempt {
                    seed: i as u64,
                    moves: moves[i],
                    correct: Some(right[i]),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let teacher = mk([[1, 2, 3, 4], [9, 9, 9, 9]], [true, false]);
        let student = mk([[1, 2, 3, 4], [9, 9, 9, 9]], [true, false]);
        assert_eq!(teacher.agreement(&student), (2, 2));
        assert_eq!(teacher.prompts(), (1, 2));
        assert_eq!(student.prompts(), (1, 2));

        let other = mk([[1, 2, 3, 4], [5, 5, 5, 5]], [true, false]);
        assert_eq!(
            teacher.agreement(&other),
            (1, 2),
            "same score, different answers"
        );
    }

    /// A prompt file of the wrong schema, or with the wrong number of slots, is
    /// an error rather than a silently-defaulted zero.
    #[test]
    fn a_malformed_prompt_expectation_is_refused() {
        let e = Expectation::parse(r#"{"schema":"gbtestprompt/2"}"#).unwrap_err();
        assert!(e.contains("gbtestprompt/1"), "{e}");
        let e =
            Expectation::parse(r#"{"schema":"gbtestprompt/1","correct_moves":[1,2,3]}"#)
                .unwrap_err();
        assert!(e.contains("four move slots"), "{e}");
    }

    /// Two gaps that are plainly different, and two that plainly are not.
    #[test]
    fn a_species_difference_is_separated_only_when_the_evidence_separates_it() {
        let wide = Gap {
            points: 40.0,
            only_reference: 80,
            only_arm: 0,
            p: 0.0,
            se: 4.0,
            pairs: 200,
        };
        let narrow = Gap {
            points: 5.0,
            only_reference: 10,
            only_arm: 0,
            p: 0.0,
            se: 4.0,
            pairs: 200,
        };
        let (_, _, p) = gaps_differ(&wide, &narrow);
        assert!(p < 0.01, "35 points apart at se 5.7 should separate: p={p}");

        let a = Gap {
            points: 10.0,
            only_reference: 20,
            only_arm: 0,
            p: 0.0,
            se: 6.0,
            pairs: 200,
        };
        let b = Gap {
            points: 13.0,
            only_reference: 26,
            only_arm: 0,
            p: 0.0,
            se: 6.0,
            pairs: 200,
        };
        let (_, _, p) = gaps_differ(&a, &b);
        assert!(p > 0.4, "3 points apart at se 8.5 cannot separate: p={p}");
    }

    /// **A plan without a split is refused.** The one thing this rig must never
    /// do is let a "trained on this fight" number be read as generalisation.
    #[test]
    fn a_plan_must_declare_its_split() {
        let without = r#"{"schema":"gbtestrig/1","scenario":"s.json",
            "entry_pool":"p","arms":[{"name":"teacher","player":"expert"}]}"#;
        let e = Plan::parse(without).unwrap_err();
        assert!(e.contains("split"), "{e}");

        let with = r#"{"schema":"gbtestrig/1","split":"unseen","scenario":"s.json",
            "entry_pool":"p","arms":[{"name":"teacher","player":"expert"}]}"#;
        let p = Plan::parse(with).expect("parses");
        assert_eq!(p.split, Split::Unseen);
        assert_eq!(p.arms.len(), 1);
    }

    /// The schema is checked, so a plan for some future version fails loudly
    /// rather than being read with today's defaults.
    #[test]
    fn a_plan_of_the_wrong_schema_is_refused() {
        let e = Plan::parse(r#"{"schema":"gbtestrig/2","split":"seen"}"#).unwrap_err();
        assert!(e.contains("gbtestrig/1"), "{e}");
    }

    #[test]
    fn a_command_arm_carries_its_program_and_arguments() {
        let text = r#"{"schema":"gbtestrig/1","split":"unseen","scenario":"s.json",
            "entry_pool":"p","subjects":[{"name":"CHARMANDER","lead_species":176}],
            "arms":[{"name":"student","player":"command","program":"python3",
                     "args":["-m","agentgb.adapter","--policy","x.npz"]}]}"#;
        let p = Plan::parse(text).expect("parses");
        assert_eq!(p.subjects[0].lead_species, Some(176));
        match &p.arms[0].player {
            Player::Command { program, args } => {
                assert_eq!(program, "python3");
                assert_eq!(args.len(), 4);
            }
            other => panic!("expected a command arm, got {other:?}"),
        }
    }
}

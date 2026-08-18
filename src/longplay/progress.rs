//! What "progress" means to a person who walked away, and how the runner
//! notices it.
//!
//! # Why this is not the agent's stuck detector
//!
//! [`terminalgb_plugin_agent::governor::Stuck`] asks *"is the agent repeating
//! itself?"* by fingerprinting the world across consecutive **decisions**. That
//! is the right question at the scale of one call and it is already answered.
//!
//! It cannot answer the question this module exists for. An agent that walks
//! ten steps north, ten steps south, and ten steps north again is producing a
//! different fingerprint every decision — it is not repeating itself at all,
//! and the escalation ladder never fires. At 70x real time it can do that for
//! two hours of wall clock, which is **six game days**, and every individual
//! decision looks reasonable. That is the main failure mode of an unattended
//! run and it is invisible at decision scale.
//!
//! So this asks a different question, at a different scale: *has the game got
//! anywhere?* — measured in the handful of quantities that only move when
//! something real happened, over a window of **game time**, not decisions.
//!
//! # The score, and why each term is in it
//!
//! [`Progress::score`] is a single monotonically-non-decreasing number. Every
//! term is something a Gen 1 player cannot undo by walking about:
//!
//! | term | weight | why |
//! |---|---:|---|
//! | badges | 4000 | the game's own definition of progress |
//! | species owned | 60 | catching something is real progress |
//! | distinct maps visited | 25 | exploration, and it is what "getting somewhere" means |
//! | sum of party levels | 4 | grinding is progress even when nothing else moves |
//! | party size | 30 | a sixth Pokémon is an event |
//! | bag items | 2 | picking things up |
//!
//! The weights are not measured against anything and do not claim to be: the
//! score is only ever compared with *itself*, so all that matters is that each
//! term is non-zero and that a badge outranks a level. What is load-bearing is
//! that **no term goes down when the player merely moves**, because a score
//! that wobbles cannot detect a wall.
//!
//! # Two kinds of wall, and they need different advice
//!
//! Watching a real run, "stuck" turned out to be two different things that a
//! single counter cannot tell apart, and the useful report names which:
//!
//! * **pinned** — the player is in a handful of tiles, usually walking into
//!   the same wall or reopening the same menu. Low position churn.
//! * **wandering** — the player is covering plenty of ground and achieving
//!   nothing: the classic maze failure (Mt. Moon, Rock Tunnel, Viridian
//!   Forest). High churn, flat score.
//!
//! [`Wall::churn`] is what separates them, and it is counted as *distinct
//! (map, x, y) triples seen in the window* rather than as distance, so a
//! two-tile shuffle repeated a thousand times still reads as pinned.

use std::collections::BTreeSet;

/// Everything the runner watches, sampled from the running machine.
///
/// Deliberately small and all-integer: it is hashed, compared, journalled and
/// resumed, and anything derived (names, prose) is looked up at report time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Progress {
    /// False until a save is really loaded. Before that every number below is
    /// uninitialised WRAM and reporting on it is worse than reporting nothing.
    pub in_game: bool,
    pub map: u8,
    pub x: u8,
    pub y: u8,
    /// `wObtainedBadges` as a bitmask.
    pub badges: u8,
    pub money: u32,
    /// One `(species, level)` per party member, in party order.
    pub party: Vec<(u8, u8)>,
    /// Total current HP across the party — zero with a non-empty party means
    /// the run has just blacked out.
    pub party_hp: u32,
    /// How much HP the party is **short** of whole: the sum of every member's
    /// maximum less its current.
    ///
    /// **Zero is "the party is healed", and that is a claim `party_hp` alone
    /// cannot make.** A goal condition compares one number against a constant,
    /// so "current equals maximum" is not expressible as `party_hp = <n>` —
    /// the maximum is not a constant, it moves with the party and with every
    /// level. Carrying the shortfall makes the whole question a single
    /// comparison, `party_missing_hp=0`, which is what a link whose job is to
    /// heal is actually judged on.
    pub party_missing_hp: u32,
    /// Species owned, counted out of the Pokédex flags.
    pub owned: u16,
    /// Species seen.
    pub seen: u16,
    /// Items in the bag.
    pub items: u8,
    /// The game's own play-time clock, in seconds. This is the honest measure
    /// of "game time elapsed" — it is what the cartridge itself believes, and
    /// unlike a frame count it does not tick while the game is paused at a
    /// text box the runner is waiting on.
    pub play_seconds: u32,
}

impl Progress {
    /// The single number a wall is measured against. See the module docs for
    /// what each term is doing here.
    pub fn score(&self, maps_visited: usize) -> u64 {
        if !self.in_game {
            return 0;
        }
        let levels: u64 = self.party.iter().map(|&(_, lv)| lv as u64).sum();
        self.badges.count_ones() as u64 * 4000
            + self.owned as u64 * 60
            + maps_visited as u64 * 25
            + levels * 4
            + self.party.len() as u64 * 30
            + self.items as u64 * 2
    }

    /// Has the party wiped? A non-empty party with no HP left at all.
    ///
    /// Deliberately *not* read from `wIsInBattle == $FF`: that sentinel is set
    /// for a single moment inside `.allPokemonFainted` and a sampler running at
    /// 10 Hz will miss it most times. Total HP is a state, not an edge, so it
    /// is still true when the sample lands.
    pub fn wiped(&self) -> bool {
        self.in_game && !self.party.is_empty() && self.party_hp == 0
    }
}

/// How much a person who walked away wants to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Importance {
    /// Worth a line in the log and the summary, not worth a phone buzzing.
    Minor,
    /// A phone buzzing at 3am is justified. Badges, a first Pokémon, the run
    /// ending.
    Major,
    /// Something is wrong: a wall, a wipe, the agent halting.
    Trouble,
}

impl Importance {
    pub fn label(self) -> &'static str {
        match self {
            Importance::Minor => "minor",
            Importance::Major => "major",
            Importance::Trouble => "trouble",
        }
    }
}

/// Something worth telling somebody about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MilestoneKind {
    /// A save is loaded and the player is in control — the first honest
    /// moment, and the one that makes every number below meaningful.
    InControl,
    Badge {
        index: u8,
        name: &'static str,
    },
    /// First visit to a map. `named` is false for interiors the map table has
    /// no name for; those are logged but not pushed.
    NewArea {
        map: u8,
        name: &'static str,
        named: bool,
    },
    FirstPokemon {
        species: u8,
    },
    PartyGrew {
        size: usize,
        species: u8,
    },
    LevelUp {
        slot: usize,
        species: u8,
        from: u8,
        to: u8,
    },
    /// A species was added to the Pokédex as owned.
    Caught {
        owned: u16,
    },
    /// The party fainted.
    Wiped,
    /// No progress for a long time. `pinned` distinguishes walking into a wall
    /// from wandering a maze — see the module docs.
    Wall {
        /// Game-seconds of no progress at the moment it was declared.
        stalled_seconds: u32,
        /// How many times this wall has now been reported.
        times: u32,
        pinned: bool,
        churn: usize,
        map: u8,
        name: &'static str,
    },
    /// The wall broke on its own.
    Unstuck {
        after_seconds: u32,
    },
    /// The agent stopped for good — budget, breaker, or the operator.
    AgentHalted {
        why: String,
    },
    /// The run resumed from a checkpoint. **Always reported**, because a
    /// resume reloads a save state and that must never be silent.
    Resumed {
        checkpoint_frame: u64,
        resumes: u32,
    },
}

impl MilestoneKind {
    pub fn importance(&self) -> Importance {
        match self {
            MilestoneKind::Badge { .. }
            | MilestoneKind::FirstPokemon { .. }
            | MilestoneKind::InControl
            | MilestoneKind::Resumed { .. } => Importance::Major,
            // A town or a route is worth a buzz; the eleventh house in Celadon
            // is not.
            MilestoneKind::NewArea { named, .. } => {
                if *named {
                    Importance::Major
                } else {
                    Importance::Minor
                }
            }
            MilestoneKind::Wiped
            | MilestoneKind::Wall { .. }
            | MilestoneKind::AgentHalted { .. } => Importance::Trouble,
            MilestoneKind::PartyGrew { .. }
            | MilestoneKind::LevelUp { .. }
            | MilestoneKind::Caught { .. }
            | MilestoneKind::Unstuck { .. } => Importance::Minor,
        }
    }

    /// Short stable machine tag, for the journal and for grouping.
    pub fn tag(&self) -> &'static str {
        match self {
            MilestoneKind::InControl => "in-control",
            MilestoneKind::Badge { .. } => "badge",
            MilestoneKind::NewArea { .. } => "new-area",
            MilestoneKind::FirstPokemon { .. } => "first-pokemon",
            MilestoneKind::PartyGrew { .. } => "party-grew",
            MilestoneKind::LevelUp { .. } => "level-up",
            MilestoneKind::Caught { .. } => "caught",
            MilestoneKind::Wiped => "wiped",
            MilestoneKind::Wall { .. } => "wall",
            MilestoneKind::Unstuck { .. } => "unstuck",
            MilestoneKind::AgentHalted { .. } => "agent-halted",
            MilestoneKind::Resumed { .. } => "resumed",
        }
    }

    /// One line of plain English. No jargon, no frame numbers, no paths — this
    /// is what lands on a phone.
    pub fn text(&self) -> String {
        match self {
            MilestoneKind::InControl => {
                "The save is loaded and the game is in play.".into()
            }
            MilestoneKind::Badge { index, name } => format!(
                "Won the {name} Badge from {} — badge {} of 8.",
                terminalgb_plugin_trainer::gen1::BADGE_LEADERS
                    .get(*index as usize)
                    .copied()
                    .unwrap_or("a gym leader"),
                index + 1
            ),
            MilestoneKind::NewArea { name, map, named } => {
                if *named {
                    format!("Reached {name}.")
                } else {
                    format!("Went somewhere new (map {map:#04x}).")
                }
            }
            MilestoneKind::FirstPokemon { .. } => "Got a first Pokémon.".into(),
            MilestoneKind::PartyGrew { size, .. } => {
                format!("The party is now {size} strong.")
            }
            MilestoneKind::LevelUp { slot, from, to, .. } => {
                format!("Party slot {} reached level {to} (was {from}).", slot + 1)
            }
            MilestoneKind::Caught { owned } => {
                format!("A new species joined the Pokédex — {owned} owned.")
            }
            MilestoneKind::Wiped => "The whole party fainted.".into(),
            MilestoneKind::Wall {
                stalled_seconds,
                times,
                pinned,
                churn,
                name,
                map,
            } => {
                let where_ = if name.is_empty() {
                    format!("map {map:#04x}")
                } else {
                    (*name).to_string()
                };
                let how = if *pinned {
                    format!("going round in {churn} tiles")
                } else {
                    format!("wandering over {churn} tiles")
                };
                format!(
                    "Stuck in {where_}: {how} with nothing to show for {} of game time. \
                     (Reported {times}x.)",
                    human_seconds(*stalled_seconds)
                )
            }
            MilestoneKind::Unstuck { after_seconds } => format!(
                "Moving again after {} stuck.",
                human_seconds(*after_seconds)
            ),
            MilestoneKind::AgentHalted { why } => format!("The agent stopped: {why}."),
            MilestoneKind::Resumed {
                checkpoint_frame,
                resumes,
            } => format!(
                "Resumed from a checkpoint taken at frame {checkpoint_frame} \
                 (resume #{resumes}). The game was rewound to that point."
            ),
        }
    }
}

/// A milestone with the context that makes it readable later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Milestone {
    pub kind: MilestoneKind,
    /// Emulated frame it was noticed on.
    pub frame: u64,
    /// The game's own clock at that moment, in seconds.
    pub play_seconds: u32,
    /// Wall-clock seconds since the run started.
    pub wall_seconds: u64,
    /// Where the player was.
    pub map: u8,
}

impl Milestone {
    pub fn importance(&self) -> Importance {
        self.kind.importance()
    }
}

/// `h m` / `m s`, for a person rather than a log parser.
pub fn human_seconds(s: u32) -> String {
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// The game-level stuck detector. See the module docs for why it exists beside
/// the agent's own.
#[derive(Debug, Clone)]
pub struct Wall {
    /// Game-seconds of flat score before a wall is declared.
    pub after_seconds: u32,
    /// Best score seen, and the game-clock reading when it was reached.
    best: u64,
    since: u32,
    /// Distinct tiles seen since the score last moved. Bounded so a genuinely
    /// long wander cannot grow this without limit; the cap is far above the
    /// "pinned" threshold, so the distinction it feeds is unaffected.
    tiles: BTreeSet<(u8, u8, u8)>,
    /// How many times the current wall has been reported.
    times: u32,
    /// Game-seconds of stall at which the next report is due. It **doubles**
    /// after every report: a wall that is going to break usually breaks early,
    /// and a run that really is dead must not produce a report a minute for
    /// six hours. See [`Wall::observe`].
    next_due: u32,
    declared: bool,
}

/// Distinct tiles at or below which a wall is "pinned" rather than
/// "wandering". Chosen, not measured: a room is a few dozen tiles, so a run
/// that has genuinely explored one is already above this.
pub const PINNED_TILES: usize = 12;

/// Cap on the tile set, so an hours-long wander cannot grow it without bound.
const TILE_CAP: usize = 4096;

impl Wall {
    pub fn new(after_seconds: u32) -> Wall {
        Wall {
            after_seconds,
            best: 0,
            since: 0,
            tiles: BTreeSet::new(),
            times: 0,
            next_due: after_seconds,
            declared: false,
        }
    }

    /// Feed one sample. Returns a milestone when there is something to say.
    ///
    /// `now` is the **game** clock in seconds, not the wall clock: a run
    /// paused at a breakpoint or a slow model must not accumulate stuck-ness
    /// it did not earn, and a run at 70x must not have to wait 70x as long to
    /// be told about a wall.
    pub fn observe(
        &mut self,
        p: &Progress,
        maps_visited: usize,
        now: u32,
    ) -> Option<MilestoneKind> {
        if !p.in_game {
            return None;
        }
        let score = p.score(maps_visited);
        if score > self.best {
            let was_declared = self.declared;
            let stuck_for = now.saturating_sub(self.since);
            self.best = score;
            self.since = now;
            self.tiles.clear();
            self.times = 0;
            self.next_due = self.after_seconds;
            self.declared = false;
            return was_declared.then_some(MilestoneKind::Unstuck {
                after_seconds: stuck_for,
            });
        }
        if self.tiles.len() < TILE_CAP {
            self.tiles.insert((p.map, p.x, p.y));
        }
        // Escalate on a DOUBLING ladder, not every sample and not linearly:
        // the first report at the threshold, then at 2x, 4x, 8x… A linear
        // ladder at a 20-minute threshold still produces a report every twenty
        // minutes for ever, which is the spam this is here to avoid.
        let stalled = now.saturating_sub(self.since);
        if stalled < self.next_due {
            return None;
        }
        self.next_due = self.next_due.saturating_mul(2);
        self.declared = true;
        self.times += 1;
        Some(MilestoneKind::Wall {
            stalled_seconds: stalled,
            times: self.times,
            pinned: self.tiles.len() <= PINNED_TILES,
            churn: self.tiles.len(),
            map: p.map,
            name: super::map_name(p.map),
        })
    }

    /// Is a wall currently declared?
    pub fn stuck(&self) -> bool {
        self.declared
    }

    /// How many separate walls have been reported this run.
    pub fn reports(&self) -> u32 {
        self.times
    }

    /// Distinct tiles seen since the score last moved.
    pub fn churn(&self) -> usize {
        self.tiles.len()
    }

    /// Best score reached. Serialised into the checkpoint so a resumed run
    /// does not immediately declare a wall it had already cleared.
    pub fn best(&self) -> u64 {
        self.best
    }

    pub fn restore(&mut self, best: u64, now: u32) {
        self.best = best;
        self.since = now;
        self.tiles.clear();
        self.times = 0;
        self.next_due = self.after_seconds;
        self.declared = false;
    }
}

/// Turns a stream of [`Progress`] samples into [`Milestone`]s.
///
/// Pure and deterministic: given the same samples it emits the same
/// milestones, which is what makes the whole reporting layer testable without
/// a cartridge.
#[derive(Debug, Clone)]
pub struct Detector {
    last: Option<Progress>,
    visited: BTreeSet<u8>,
    reported_badges: u8,
    best_levels: Vec<u8>,
    best_owned: u16,
    seen_wipe: bool,
    in_control: bool,
    pub wall: Wall,
}

impl Detector {
    pub fn new(wall_after_seconds: u32) -> Detector {
        Detector {
            last: None,
            visited: BTreeSet::new(),
            reported_badges: 0,
            best_levels: Vec::new(),
            best_owned: 0,
            seen_wipe: false,
            in_control: false,
            wall: Wall::new(wall_after_seconds),
        }
    }

    pub fn visited(&self) -> &BTreeSet<u8> {
        &self.visited
    }

    pub fn maps_visited(&self) -> usize {
        self.visited.len()
    }

    /// Feed one sample; get back everything newly worth saying.
    pub fn observe(&mut self, p: &Progress) -> Vec<MilestoneKind> {
        let mut out = Vec::new();
        if !p.in_game {
            self.last = Some(p.clone());
            return out;
        }
        if !self.in_control {
            self.in_control = true;
            out.push(MilestoneKind::InControl);
        }

        // Areas. First visit only, and the *first* map is not an arrival —
        // it is where the save happened to be.
        if self.visited.insert(p.map) && self.last.is_some() {
            let name = super::map_name(p.map);
            out.push(MilestoneKind::NewArea {
                map: p.map,
                name,
                named: !name.is_empty(),
            });
        }

        // Badges. Bit by bit, so winning two between samples reports two.
        let fresh = p.badges & !self.reported_badges;
        for i in 0..8u8 {
            if fresh & (1 << i) != 0 {
                out.push(MilestoneKind::Badge {
                    index: i,
                    name: badge_name(i),
                });
            }
        }
        self.reported_badges |= p.badges;

        // Party.
        let had = self.last.as_ref().map(|l| l.party.len()).unwrap_or(0);
        if p.party.len() > had {
            if had == 0 && !p.party.is_empty() {
                out.push(MilestoneKind::FirstPokemon {
                    species: p.party[0].0,
                });
            } else {
                out.push(MilestoneKind::PartyGrew {
                    size: p.party.len(),
                    species: p.party.last().map(|m| m.0).unwrap_or(0),
                });
            }
        }

        // Levels. Against the best ever seen per slot, not against the last
        // sample: switching a Pokémon out of the party and back in would
        // otherwise read as a run of level-ups.
        for (slot, &(species, level)) in p.party.iter().enumerate() {
            if self.best_levels.len() <= slot {
                self.best_levels.resize(slot + 1, 0);
            }
            let best = self.best_levels[slot];
            if level > best && best > 0 {
                out.push(MilestoneKind::LevelUp {
                    slot,
                    species,
                    from: best,
                    to: level,
                });
            }
            self.best_levels[slot] = self.best_levels[slot].max(level);
        }

        // Pokédex. One milestone per new species, capped at one per sample so
        // a trade or an evolution cannot produce a burst.
        if p.owned > self.best_owned {
            self.best_owned = p.owned;
            out.push(MilestoneKind::Caught { owned: p.owned });
        }

        // Wipes. Edge-triggered on the *recovery*, so the trip to the Pokémon
        // Centre does not report a second one.
        if p.wiped() {
            if !self.seen_wipe {
                self.seen_wipe = true;
                out.push(MilestoneKind::Wiped);
            }
        } else {
            self.seen_wipe = false;
        }

        if let Some(m) = self.wall.observe(p, self.visited.len(), p.play_seconds) {
            out.push(m);
        }

        self.last = Some(p.clone());
        out
    }

    /// Restore counters from a checkpoint, so a resumed run does not replay
    /// every milestone it already reported.
    pub fn restore(
        &mut self,
        visited: BTreeSet<u8>,
        badges: u8,
        levels: Vec<u8>,
        owned: u16,
        wall_best: u64,
        play_seconds: u32,
    ) {
        self.visited = visited;
        self.reported_badges = badges;
        self.best_levels = levels;
        self.best_owned = owned;
        self.in_control = false;
        self.wall.restore(wall_best, play_seconds);
    }

    pub fn badges_reported(&self) -> u8 {
        self.reported_badges
    }
    pub fn best_levels(&self) -> &[u8] {
        &self.best_levels
    }
    pub fn best_owned(&self) -> u16 {
        self.best_owned
    }
}

/// The eight badges, in `wObtainedBadges` bit order.
///
/// Duplicated from nothing: `terminalgb_plugin_trainer::gen1::BADGE_NAMES` is the
/// same list and is used where that crate is in scope; this module is compiled
/// with it, so it defers to it rather than keeping a second copy.
fn badge_name(i: u8) -> &'static str {
    terminalgb_plugin_trainer::gen1::BADGE_NAMES
        .get(i as usize)
        .copied()
        .unwrap_or("Badge")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(map: u8, x: u8, y: u8) -> Progress {
        Progress {
            in_game: true,
            map,
            x,
            y,
            party: vec![(1, 5)],
            party_hp: 20,
            ..Default::default()
        }
    }

    #[test]
    fn the_score_never_falls_when_the_player_merely_walks() {
        // The load-bearing property: a score that wobbles as the player moves
        // cannot detect a wall, because every step would look like progress.
        let a = p(0, 1, 1);
        let mut b = a.clone();
        b.x = 9;
        b.y = 4;
        b.map = 12;
        assert_eq!(a.score(1), b.score(1));
    }

    #[test]
    fn every_term_of_the_score_moves_it() {
        let base = p(0, 0, 0);
        let bump = |f: &dyn Fn(&mut Progress)| {
            let mut q = base.clone();
            f(&mut q);
            assert!(
                q.score(1) > base.score(1),
                "a term of the score did nothing"
            );
        };
        bump(&|q| q.badges = 1);
        bump(&|q| q.owned = 1);
        bump(&|q| q.party.push((2, 5)));
        bump(&|q| q.party[0].1 = 6);
        bump(&|q| q.items = 1);
        assert!(base.score(2) > base.score(1), "maps visited");
    }

    #[test]
    fn a_new_area_is_reported_once_and_a_town_outranks_an_interior() {
        let mut d = Detector::new(600);
        d.observe(&p(0, 1, 1)); // first sample: not an arrival
        let m = d.observe(&p(0x0C, 1, 1)); // Route 1
        let area = m
            .iter()
            .find(|k| matches!(k, MilestoneKind::NewArea { .. }))
            .expect("arrival");
        assert_eq!(area.importance(), Importance::Major);
        assert!(area.text().contains("Route 1"), "{}", area.text());
        // Going back is not an arrival.
        let again = d.observe(&p(0x0C, 2, 1));
        assert!(!again
            .iter()
            .any(|k| matches!(k, MilestoneKind::NewArea { .. })));
        // An unnamed interior is logged but is not phone-worthy.
        let inside = d.observe(&p(0xFE, 1, 1));
        let a = inside
            .iter()
            .find(|k| matches!(k, MilestoneKind::NewArea { .. }))
            .expect("arrival");
        assert_eq!(a.importance(), Importance::Minor);
    }

    #[test]
    fn each_badge_is_reported_exactly_once_even_if_two_arrive_together() {
        let mut d = Detector::new(600);
        d.observe(&p(0, 1, 1));
        let mut q = p(0, 1, 1);
        q.badges = 0b11;
        let m = d.observe(&q);
        let badges: Vec<_> = m
            .iter()
            .filter(|k| matches!(k, MilestoneKind::Badge { .. }))
            .collect();
        assert_eq!(badges.len(), 2);
        assert!(
            badges[0].text().contains("Boulder Badge from Brock"),
            "{}",
            badges[0].text()
        );
        assert!(d.observe(&q).is_empty(), "reported twice");
    }

    #[test]
    fn a_level_up_is_measured_against_the_best_ever_not_the_last_sample() {
        // Swapping a Pokémon out and back in must not read as a level-up.
        let mut d = Detector::new(600);
        let mut q = p(0, 1, 1);
        q.party = vec![(1, 5)];
        d.observe(&q);
        q.party = vec![(1, 9)];
        assert!(d
            .observe(&q)
            .iter()
            .any(|k| matches!(k, MilestoneKind::LevelUp { from: 5, to: 9, .. })));
        q.party = vec![(2, 5)]; // a different, lower-level Pokémon in slot 1
        assert!(
            !d.observe(&q)
                .iter()
                .any(|k| matches!(k, MilestoneKind::LevelUp { .. })),
            "a swap is not a level-up"
        );
        q.party = vec![(1, 9)]; // back again
        assert!(
            !d.observe(&q)
                .iter()
                .any(|k| matches!(k, MilestoneKind::LevelUp { .. })),
            "returning to a level already reached is not a level-up"
        );
    }

    #[test]
    fn a_wipe_is_reported_once_per_wipe() {
        let mut d = Detector::new(600);
        let mut q = p(0, 1, 1);
        d.observe(&q);
        q.party_hp = 0;
        assert!(d.observe(&q).contains(&MilestoneKind::Wiped));
        assert!(!d.observe(&q).contains(&MilestoneKind::Wiped), "twice");
        q.party_hp = 20; // healed
        d.observe(&q);
        q.party_hp = 0;
        assert!(
            d.observe(&q).contains(&MilestoneKind::Wiped),
            "a second wipe"
        );
    }

    #[test]
    fn walking_into_the_same_wall_reads_as_pinned_and_a_maze_reads_as_wandering() {
        let mut pin = Wall::new(600);
        let mut wander = Wall::new(600);
        let (mut ma, mut mb) = (None, None);
        for t in 0..700u32 {
            let mut a = p(0, (t % 2) as u8, 3);
            a.play_seconds = t;
            ma = pin.observe(&a, 1, t).or(ma);
            let mut b = p(0, (t % 40) as u8, (t / 40 % 40) as u8);
            b.play_seconds = t;
            mb = wander.observe(&b, 1, t).or(mb);
        }
        // Both are walls; only one is pinned.
        assert!(
            matches!(ma, Some(MilestoneKind::Wall { pinned: true, .. })),
            "{ma:?}"
        );
        assert!(
            matches!(mb, Some(MilestoneKind::Wall { pinned: false, .. })),
            "{mb:?}"
        );
        assert!(ma.unwrap().text().contains("going round in"));
        assert!(mb.unwrap().text().contains("wandering over"));
    }

    #[test]
    fn progress_clears_a_wall_and_says_so() {
        let mut w = Wall::new(100);
        for t in 0..120u32 {
            let mut q = p(0, 1, 1);
            q.play_seconds = t;
            w.observe(&q, 1, t);
        }
        assert!(w.stuck(), "should have declared a wall");
        let mut q = p(0, 1, 1);
        q.badges = 1;
        let m = w.observe(&q, 1, 130);
        assert!(matches!(m, Some(MilestoneKind::Unstuck { .. })), "{m:?}");
        assert!(!w.stuck());
    }

    #[test]
    fn a_wall_does_not_report_once_a_second_for_ever() {
        let mut w = Wall::new(60);
        let mut reports = 0;
        for t in 0..3600u32 {
            let mut q = p(0, 1, 1);
            q.play_seconds = t;
            if w.observe(&q, 1, t).is_some() {
                reports += 1;
            }
        }
        // An hour of total deadlock at a 60 s threshold: a handful of
        // escalating reports, not sixty.
        assert!(
            (1..=12).contains(&reports),
            "{reports} reports in an hour of deadlock"
        );
    }

    #[test]
    fn nothing_is_reported_before_a_save_is_loaded() {
        let mut d = Detector::new(600);
        let q = Progress::default();
        assert!(d.observe(&q).is_empty());
        // …and the first in-game sample announces itself.
        assert!(d.observe(&p(0, 1, 1)).contains(&MilestoneKind::InControl));
    }

    #[test]
    fn a_resume_is_always_major_because_it_rewinds_the_game() {
        let k = MilestoneKind::Resumed {
            checkpoint_frame: 10,
            resumes: 1,
        };
        assert_eq!(k.importance(), Importance::Major);
        assert!(k.text().contains("rewound"), "{}", k.text());
    }

    #[test]
    fn every_milestone_reads_as_plain_english() {
        // No jargon, no paths, no braces: this text lands on a phone.
        let all = [
            MilestoneKind::InControl,
            MilestoneKind::Badge {
                index: 0,
                name: "Boulder",
            },
            MilestoneKind::NewArea {
                map: 1,
                name: "Viridian City",
                named: true,
            },
            MilestoneKind::FirstPokemon { species: 1 },
            MilestoneKind::PartyGrew {
                size: 2,
                species: 4,
            },
            MilestoneKind::LevelUp {
                slot: 0,
                species: 1,
                from: 5,
                to: 6,
            },
            MilestoneKind::Caught { owned: 3 },
            MilestoneKind::Wiped,
            MilestoneKind::Wall {
                stalled_seconds: 900,
                times: 1,
                pinned: true,
                churn: 3,
                map: 0,
                name: "Pallet Town",
            },
            MilestoneKind::Unstuck { after_seconds: 60 },
            MilestoneKind::AgentHalted {
                why: "call-cap".into(),
            },
            MilestoneKind::Resumed {
                checkpoint_frame: 1,
                resumes: 1,
            },
        ];
        for k in all {
            let t = k.text();
            assert!(!t.is_empty());
            assert!(!t.contains('{') && !t.contains('/'), "jargon in {t:?}");
            assert!(
                t.ends_with('.') || t.ends_with(')'),
                "not a sentence: {t:?}"
            );
            assert!(!k.tag().is_empty());
        }
    }

    #[test]
    fn human_seconds_reads_like_a_person_wrote_it() {
        assert_eq!(human_seconds(45), "45s");
        assert_eq!(human_seconds(125), "2m 5s");
        assert_eq!(human_seconds(7325), "2h 2m");
    }
}

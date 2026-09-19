//! Is this encounter worth a ball?
//!
//! # The shape of the answer, and why it is not simply yes or no
//!
//! **Catching is a capability that starts at zero.** A Pokémon Red or Blue
//! player begins with an empty bag and no way to catch anything at all; the
//! first ball arrives either as Oak's scripted gift of five (`07:52D9` on this
//! cartridge — `ld hl,$D74B / bit 4,[hl] / set 4,[hl] / jr nz / ld bc,POKE_BALL:5
//! / call GiveItem`) or across a shop counter. Measured on the Pewter chain's
//! own carried-forward states, **the bag is empty at every one of links 01 to
//! 10** and that gift flag is still clear — so for the whole chain as it stands
//! today there is no ball to throw.
//!
//! That is why [`Verdict::Unavailable`] exists and is not spelled `No`. A
//! policy that answers "no" has weighed something; a policy with no ball has
//! nothing to weigh, and the difference matters twice over — it keeps the
//! reports honest, and it means a student trained on these runs never sees a
//! catch attempt before the capability exists, so it cannot learn to try one.
//!
//! # What it reads, and where each comes from
//!
//! | input | source |
//! |---|---|
//! | the balls held | `wNumBagItems`/`wBagItems`, which [`crate::battle::View`] already carries as `items` |
//! | the odds | [`terminalgb_plugin_trainer::capture`], the disassembled routine — **not** a second formula |
//! | the opponent's HP, max HP and status | `wEnemyMon` `$CFE5` |
//! | its catch rate | `wEnemyMonActualCatchRate` `$D007`, the byte the routine itself compares — not the species' base rate, because bait and rocks move it |
//! | which battle this is | [`crate::battlekind`] from the screen, or `wIsInBattle` from memory |
//! | what the party already holds | `wPartySpecies` |
//!
//! # The safety net, measured
//!
//! A ball thrown in a trainer battle is **refused and kept**: `ItemUseBall`
//! branches to `ThrowBallAtTrainerMon`, which prints "The trainer blocked the
//! BALL! / Don't be a thief!" and returns without touching the bag. Measured
//! over 64 throws in synthesised trainer battles: 64 blocked, 64 balls kept,
//! zero spent. So mistaking a trainer battle for a wild one costs **a turn**,
//! never a ball — which is why [`Verdict::No`] rather than a refusal to run at
//! all is the right response to an uncertain discriminator, and why the
//! uncertain case is still declined.

use terminalgb_plugin_trainer::capture::{self, Ball, Odds, Target};
use terminalgb_plugin_trainer::gen1::BattleType;

use crate::battle::Item;
use crate::battlekind::ScreenVerdict;

/// A species worth a ball, and what it is for.
///
/// The list is **derived** — `tools/gen1-tables/gen1_tables.py --plan` reads
/// the encounter tables, the type chart, the base stats and the gym leaders'
/// own parties out of the cartridge and says what is worth catching and where.
/// Nothing here names a Pokémon; the caller supplies the answer that query
/// produced. See `docs/gen1/catching.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    /// The **internal** species index — `wEnemyMonSpecies`' own numbering, not
    /// the Pokédex number. The two differ for almost every species and mixing
    /// them is the classic Gen 1 mistake.
    pub species: u8,
    /// How many of it are worth holding. One, nearly always: a second Pikachu
    /// answers nothing the first did not.
    pub cap: u8,
    /// Why the run wants it, in words a report can print.
    pub why: &'static str,
}

/// Everything the decision reads.
#[derive(Debug, Clone)]
pub struct Situation<'a> {
    /// Which kind of battle this is, however the caller established it — the
    /// memory-derived expert converts `wIsInBattle`, a screen-only player uses
    /// [`crate::battlekind::Watcher`]. **`Unknown` declines**: see the module
    /// docs for what a wrong answer costs in each direction.
    pub kind: ScreenVerdict,
    /// `wBattleType`, when the caller can read it. `None` from a screen alone.
    /// A `Some(OldMan)` declines — the demonstration Pokémon never becomes the
    /// player's, so a ball there is thrown away.
    pub battle_type: Option<BattleType>,
    /// The opponent's internal species index.
    pub species: u8,
    /// What [`capture::odds`] needs, read straight off the battle struct.
    pub target: Target,
    /// The bag, exactly as [`crate::battle::View::items`] carries it.
    pub items: &'a [Item],
    /// Internal species indices the party already holds.
    pub party: &'a [u8],
    /// Our own remaining HP, and the worst hit — including a critical — we
    /// expect to take. A throw costs a turn, and a turn we do not survive is
    /// not worth any Pokémon.
    pub our_hp: u16,
    pub incoming_crit: u16,
}

impl<'a> Situation<'a> {
    /// Build a situation from the battle policy's own view, so a caller that
    /// already has one needs a single line rather than a second set of memory
    /// reads.
    ///
    /// `catch_rate` is the one field a [`crate::battle::View`] does not carry —
    /// it is `wEnemyMonActualCatchRate` `$D007`, deliberately not the species'
    /// base rate — so it is passed in beside the view. Everything else comes
    /// off the view: the opponent's species, HP, max HP and status, our HP, the
    /// worst incoming hit including a critical, and the bag.
    ///
    /// `kind` is the caller's, not the view's `trainer` flag, because a
    /// screen-only player has a different and weaker source for it and the
    /// whole point of the seam is that both can use this.
    pub fn from_view(
        v: &'a crate::battle::View,
        kind: ScreenVerdict,
        battle_type: Option<BattleType>,
        catch_rate: u8,
        party: &'a [u8],
    ) -> Situation<'a> {
        Situation {
            kind,
            battle_type,
            species: v.theirs.species,
            target: Target {
                catch_rate,
                hp: v.theirs.hp,
                max_hp: v.theirs.max_hp,
                status: terminalgb_plugin_trainer::gen1::Status(v.theirs.status),
            },
            items: &v.items,
            party,
            our_hp: v.ours.hp,
            incoming_crit: v.incoming_crit,
        }
    }
}

/// The chosen ball, and the number behind the choice.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Throw {
    /// The bag item id to select.
    pub item: u8,
    pub ball: Ball,
    /// What [`capture::odds`] says this throw does, right now.
    pub chance: f64,
    pub why: &'static str,
}

/// The four answers, and the first one is deliberately not "no".
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// **No ball is held, so nothing was decided.** The capability does not
    /// exist yet. Distinct from [`Verdict::No`] on purpose — see the module
    /// docs.
    Unavailable,
    Throw(Throw),
    /// Worth catching, but the odds are poor and the fight can improve them
    /// without risking the kill. Hand the turn back to the battle policy.
    Weaken {
        chance: f64,
        why: &'static str,
    },
    No {
        why: &'static str,
    },
}

impl Verdict {
    /// Did the policy actually weigh anything? `false` for
    /// [`Verdict::Unavailable`], which is what keeps "we chose not to" and "we
    /// could not" separate in a report.
    pub fn is_decision(&self) -> bool {
        !matches!(self, Verdict::Unavailable)
    }

    pub fn why(&self) -> &'static str {
        match self {
            Verdict::Unavailable => why::NO_BALLS,
            Verdict::Throw(t) => t.why,
            Verdict::Weaken { why, .. } | Verdict::No { why } => why,
        }
    }
}

/// The reasons, named, for the same reason [`crate::battle::why`] names its
/// four: a caller that acts on a reason must match a name, never a substring
/// of a sentence.
pub mod why {
    pub const NO_BALLS: &str = "no ball is held, so there is nothing to decide";
    pub const NOT_WILD: &str = "a trainer's Pokémon cannot be caught";
    pub const UNSURE_WHICH: &str =
        "the screen has not said whether this is a wild battle";
    pub const NOT_KEPT: &str = "this battle's Pokémon never becomes ours";
    pub const NOT_WANTED: &str = "nothing on the target list";
    pub const ALREADY_HELD: &str = "the party already holds one";
    pub const WOULD_FAINT: &str = "a thrown ball costs a turn we would not survive";
    pub const WEAKEN_FIRST: &str = "wanted, but weaken it before spending a ball";
    pub const HOPELESS: &str = "wanted, but no ball held is worth throwing here";
    pub const WORTH_IT: &str = "wanted, and this ball is the best value here";
}

/// Below this chance a throw is worth less than the turn it costs — **if** the
/// fight can still improve it. `Weaken` rather than `No`, so the run keeps the
/// encounter rather than walking away from it.
///
/// Not a tuned constant: it is the point where one throw is worth less than
/// one more turn of chipping, and one turn of chipping roughly doubles the HP
/// term for a Pokémon near full health (`03:5754` divides by `hp/4`). A third
/// is the honest statement of "the next turn is worth more than this ball".
pub const WEAKEN_BELOW: f64 = 1.0 / 3.0;

/// A throw is never worth attempting below this, even with nothing left to
/// weaken with. One in fifty is a ball spent on a coin nobody would flip.
pub const HOPELESS_BELOW: f64 = 0.02;

/// What the balls cost, in the shop's own money.
///
/// `ItemPrices` — three BCD bytes per item, located on this cartridge at file
/// offset `0x4608` with exactly one match for its opening four entries. A
/// Master Ball is free because it is never sold.
///
/// # Why the cheap ball nearly always wins, and it is a derivation
///
/// A dearer ball's whole advantage is bounded, and the bound is small. The
/// resampling loop narrows the first random byte to `0..=200` (Great) or
/// `0..=150` (Ultra) instead of `0..=255`, which is at most **1.27x** and
/// **1.70x**; and the Great Ball's smaller HP divisor (8 against 12,
/// `03:5742`) is worth at most a further **1.5x**. So a Great Ball is at most
/// 1.91x a Poké Ball and an Ultra Ball at most 1.70x — against price ratios of
/// **3x and 6x**.
///
/// **A Poké Ball therefore always wins on chance per ¥**, at every catch rate
/// and every HP. That is not a preference, it is arithmetic over the routine,
/// and `a_poke_ball_is_always_the_better_value` sweeps it rather than asserting
/// it. The dearer balls exist here for the one case where money is not the
/// scarce thing — see [`WEAKEN_BELOW`] and rule 6 of [`decide`].
pub const PRICES: [(Ball, u32); 4] = [
    (Ball::Poke, 200),
    (Ball::Great, 600),
    (Ball::Ultra, 1200),
    (Ball::Safari, 0),
];

fn price(b: Ball) -> u32 {
    PRICES
        .iter()
        .find(|(x, _)| *x == b)
        .map(|(_, p)| *p)
        .unwrap_or(0)
}

/// The Master Ball is never spent by this policy.
///
/// Not squeamishness: it is the one item in the game with no second copy and
/// no shop, `03:5702` makes it a certainty against anything, and every target
/// a *route* wants is a common wild Pokémon a ¥200 ball catches. A policy that
/// could reach for it would eventually reach for it on a Rattata.
pub const NEVER_SPEND_MASTER: bool = true;

/// The balls in the bag, in the order they should be preferred when their
/// chances are close: cheapest first. Prices from `ItemPrices`, cited above.
fn cheapest_first(items: &[Item]) -> Vec<(u8, Ball)> {
    let order = [Ball::Poke, Ball::Great, Ball::Ultra, Ball::Safari];
    let mut out = Vec::new();
    for want in order {
        for it in items {
            if it.count == 0 {
                continue;
            }
            if let Some(b) = capture::ball_for_item(it.id) {
                if b == want {
                    out.push((it.id, b));
                }
            }
        }
    }
    out
}

/// Does the bag hold anything that can be thrown at all — Master Ball
/// included? This is the **gate**, and it is asked before anything else so a
/// run with an empty bag never reaches a decision.
pub fn holds_a_ball(items: &[Item]) -> bool {
    items
        .iter()
        .any(|i| i.count > 0 && capture::ball_for_item(i.id).is_some())
}

/// Decide.
///
/// The order is by how much a wrong answer costs, exactly as
/// [`crate::battle::decide`]'s is:
///
/// 1. **No ball** — not a decision at all.
/// 2. **Not a wild battle, or not sure** — a trainer's Pokémon cannot be
///    caught, and the game refuses the throw without taking the ball.
/// 3. **Not something we keep** — the old man's demonstration Pokémon is never
///    added to the party.
/// 4. **Not wanted, or already held** — a second Pikachu answers nothing.
/// 5. **We would not survive the turn** — the ball is the cheapest thing in
///    that exchange.
/// 6. Then, and only then, which ball and whether now.
pub fn decide(s: &Situation, wants: &[Want]) -> Verdict {
    // 1. The gate.
    if !holds_a_ball(s.items) {
        return Verdict::Unavailable;
    }
    // 2. Which battle is this?
    match s.kind {
        ScreenVerdict::Trainer => return Verdict::No { why: why::NOT_WILD },
        ScreenVerdict::Unknown => {
            return Verdict::No {
                why: why::UNSURE_WHICH,
            }
        }
        ScreenVerdict::Wild => {}
    }
    // 3. Will it become ours if it works?
    if let Some(bt) = s.battle_type {
        if !bt.is_catchable() {
            return Verdict::No { why: why::NOT_KEPT };
        }
    }
    // 4. Is it wanted, and do we already have it?
    let Some(want) = wants.iter().find(|w| w.species == s.species) else {
        return Verdict::No {
            why: why::NOT_WANTED,
        };
    };
    let held = s.party.iter().filter(|p| **p == s.species).count();
    if held >= want.cap as usize {
        return Verdict::No {
            why: why::ALREADY_HELD,
        };
    }
    // 5. A throw is a turn. A turn we do not survive is not worth any Pokémon —
    //    a fainted lead loses the encounter *and* the walk back to a Pokémon
    //    Center. The critical hit is the worst case, the same reading
    //    `battle::View::incoming_crit` makes and for the same reason.
    if s.incoming_crit >= s.our_hp {
        return Verdict::No {
            why: why::WOULD_FAINT,
        };
    }

    // 6. Which ball. **Cheapest, unless money has stopped being the scarce
    //    thing.** See [`PRICES`]: a dearer ball's advantage is bounded by
    //    1.91x and its price by 3x, so on chance per ¥ the cheap one always
    //    wins — but a *turn* is not always replaceable the way ¥200 is. When
    //    even the cheap ball's chance is under [`WEAKEN_BELOW`] the encounter
    //    is likely to be lost before it is won, and then the best chance is
    //    worth its price.
    let held: Vec<(u8, Ball, Odds)> = cheapest_first(s.items)
        .into_iter()
        .filter(|(_, b)| !(NEVER_SPEND_MASTER && *b == Ball::Master))
        .map(|(i, b)| (i, b, capture::odds(s.target, b)))
        .collect();
    let Some(cheapest) = held.first().copied() else {
        // Only a Master Ball is held, and this policy does not spend it.
        return Verdict::No {
            why: why::NOT_WANTED,
        };
    };
    let (item, ball, odds) = if cheapest.2.chance >= WEAKEN_BELOW {
        cheapest
    } else {
        held.iter()
            .copied()
            .max_by(|a, b| {
                a.2.chance
                    .partial_cmp(&b.2.chance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    // A tie goes to the cheaper ball, which is what stops a
                    // saturated HP term spending an Ultra Ball for nothing.
                    .then(price(b.1).cmp(&price(a.1)))
            })
            .unwrap_or(cheapest)
    };

    if odds.chance < HOPELESS_BELOW {
        return Verdict::No { why: why::HOPELESS };
    }
    // Weakening is only worth suggesting while there is something left to
    // weaken. `hp_maxed` means the HP term has already saturated — the target
    // is low enough that chipping it further buys nothing at all, and the only
    // thing left is the catch-rate comparison. Suggesting "weaken it" there
    // would be advice that cannot be taken.
    if odds.chance < WEAKEN_BELOW && !odds.hp_maxed && s.target.status.0 == 0 {
        return Verdict::Weaken {
            chance: odds.chance,
            why: why::WEAKEN_FIRST,
        };
    }
    Verdict::Throw(Throw {
        item,
        ball,
        chance: odds.chance,
        why: if want.why.is_empty() {
            why::WORTH_IT
        } else {
            want.why
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use terminalgb_plugin_trainer::gen1::Status;

    const PIKACHU: u8 = 0x54; // internal index, from the encounter tables
    const RATTATA: u8 = 0xA5;

    fn wants() -> Vec<Want> {
        vec![Want {
            species: PIKACHU,
            cap: 1,
            why: "electric, and Misty is water",
        }]
    }

    fn target(catch_rate: u8, hp: u16, max_hp: u16, status: u8) -> Target {
        Target {
            catch_rate,
            hp,
            max_hp,
            status: Status(status),
        }
    }

    fn sit<'a>(items: &'a [Item], party: &'a [u8]) -> Situation<'a> {
        Situation {
            kind: ScreenVerdict::Wild,
            battle_type: Some(BattleType::Normal),
            species: PIKACHU,
            target: target(190, 8, 20, 0),
            items,
            party,
            our_hp: 40,
            incoming_crit: 6,
        }
    }

    /// **The gate.** With an empty bag the policy has not decided anything, and
    /// says so with a different word. This is the maintainer's constraint and the
    /// whole reason `Unavailable` is not `No`.
    #[test]
    fn an_empty_bag_is_not_a_decision() {
        let v = decide(&sit(&[], &[]), &wants());
        assert_eq!(v, Verdict::Unavailable);
        assert!(!v.is_decision());
        // A bag with things in it but no ball is the same answer.
        let potions = [Item { id: 0x14, count: 5 }];
        assert_eq!(decide(&sit(&potions, &[]), &wants()), Verdict::Unavailable);
        // A ball whose count has run to zero, likewise.
        let spent = [Item { id: 4, count: 0 }];
        assert_eq!(decide(&sit(&spent, &[]), &wants()), Verdict::Unavailable);
    }

    /// …and the moment one ball exists, the same situation becomes a decision.
    #[test]
    fn one_ball_turns_the_gate_into_a_decision() {
        let balls = [Item { id: 4, count: 1 }];
        let v = decide(&sit(&balls, &[]), &wants());
        assert!(v.is_decision());
        assert!(matches!(v, Verdict::Throw(_)), "{v:?}");
    }

    /// A trainer's Pokémon cannot be caught, and an *uncertain* screen is
    /// treated the same way. Both are `No`, and both are decisions.
    #[test]
    fn a_trainer_battle_and_an_unsure_screen_both_decline() {
        let balls = [Item { id: 4, count: 5 }];
        let mut s = sit(&balls, &[]);
        s.kind = ScreenVerdict::Trainer;
        assert_eq!(decide(&s, &wants()), Verdict::No { why: why::NOT_WILD });
        s.kind = ScreenVerdict::Unknown;
        assert_eq!(
            decide(&s, &wants()),
            Verdict::No {
                why: why::UNSURE_WHICH
            }
        );
    }

    /// The old man's demonstration Pokémon is never added to the party
    /// (`_AddPartyMon` is not reached for it), so a ball there is thrown away.
    #[test]
    fn the_demonstration_battle_is_declined() {
        let balls = [Item { id: 4, count: 5 }];
        let mut s = sit(&balls, &[]);
        s.battle_type = Some(BattleType::OldMan);
        assert_eq!(decide(&s, &wants()), Verdict::No { why: why::NOT_KEPT });
    }

    #[test]
    fn nothing_off_the_list_and_nothing_twice() {
        let balls = [Item { id: 4, count: 5 }];
        let mut s = sit(&balls, &[]);
        s.species = RATTATA;
        assert_eq!(
            decide(&s, &wants()),
            Verdict::No {
                why: why::NOT_WANTED
            }
        );
        // Already in the party: a second one answers nothing.
        let party = [PIKACHU];
        let s = sit(&balls, &party);
        assert_eq!(
            decide(&s, &wants()),
            Verdict::No {
                why: why::ALREADY_HELD
            }
        );
    }

    /// A throw is a turn, and a turn that kills the lead costs the encounter
    /// twice over. Same reading of the critical hit as the battle policy.
    #[test]
    fn a_throw_we_would_not_survive_is_declined() {
        let balls = [Item { id: 4, count: 5 }];
        let mut s = sit(&balls, &[]);
        s.our_hp = 5;
        s.incoming_crit = 6;
        assert_eq!(
            decide(&s, &wants()),
            Verdict::No {
                why: why::WOULD_FAINT
            }
        );
    }

    /// A healthy target with a poor rate is worth chipping first — and the
    /// suggestion is only made while chipping can still do something.
    #[test]
    fn a_full_health_target_is_weakened_before_a_ball_is_spent() {
        let balls = [Item { id: 4, count: 5 }];
        let mut s = sit(&balls, &[]);
        s.target = target(45, 300, 300, 0);
        match decide(&s, &wants()) {
            Verdict::Weaken { chance, .. } => assert!(chance < WEAKEN_BELOW),
            other => panic!("{other:?}"),
        }
        // Once the HP term has saturated there is nothing left to chip, so the
        // advice would be untakeable and is not given.
        s.target = target(45, 1, 300, 0);
        assert!(matches!(decide(&s, &wants()), Verdict::Throw(_)));
    }

    /// **The derivation behind the ball choice, swept rather than asserted.**
    /// A Great Ball costs three Poké Balls and an Ultra Ball six; their
    /// advantage is bounded by the resampling loop and, for the Great Ball, the
    /// smaller HP divisor. If a dearer ball ever beat those price ratios the
    /// cheap-first rule would be wrong, so the claim is checked over the whole
    /// space rather than at the two points that inspired it.
    #[test]
    fn a_poke_ball_is_always_the_better_value() {
        for rate in [0u8, 3, 45, 90, 150, 190, 255] {
            for max_hp in [12u16, 41, 100, 300, 700] {
                for num in 1..=8u16 {
                    let hp = (max_hp * num / 8).max(1);
                    for st in [0u8, 0x04, 0x08] {
                        let t = target(rate, hp, max_hp, st);
                        let p = capture::odds(t, Ball::Poke).chance;
                        let g = capture::odds(t, Ball::Great).chance;
                        let u = capture::odds(t, Ball::Ultra).chance;
                        assert!(g <= p * 3.0 + 1e-12, "great {g} vs poke {p} at {t:?}");
                        assert!(u <= p * 6.0 + 1e-12, "ultra {u} vs poke {p} at {t:?}");
                    }
                }
            }
        }
    }

    /// So the cheap ball is the one thrown while the odds are decent — and the
    /// dear one appears only where a *turn* has become the scarce thing.
    #[test]
    fn the_cheapest_ball_is_thrown_until_a_turn_costs_more_than_the_money() {
        let bag = [
            Item { id: 4, count: 9 },
            Item { id: 3, count: 9 },
            Item { id: 2, count: 9 },
        ];
        // Comfortable odds: the ¥200 ball, every time.
        let mut s = sit(&bag, &[]);
        s.target = target(255, 1, 40, 0);
        match decide(&s, &wants()) {
            Verdict::Throw(t) => assert_eq!(t.ball, Ball::Poke, "{t:?}"),
            other => panic!("{other:?}"),
        }
        // A hard target already on one hit point and asleep: nothing is left to
        // weaken, the cheap ball is under the threshold, and the encounter is
        // likelier to be lost than won — so the best chance is worth its price.
        s.target = target(45, 1, 300, 0x04);
        match decide(&s, &wants()) {
            Verdict::Throw(t) => {
                assert_eq!(t.ball, Ball::Ultra, "{t:?}");
                assert!(t.chance > capture::odds(s.target, Ball::Poke).chance);
            }
            other => panic!("{other:?}"),
        }
        // …and with only Poké Balls held it throws one rather than declining.
        let only = [Item { id: 4, count: 9 }];
        let mut s = sit(&only, &[]);
        s.target = target(45, 1, 300, 0x04);
        match decide(&s, &wants()) {
            Verdict::Throw(t) => assert_eq!(t.ball, Ball::Poke),
            other => panic!("{other:?}"),
        }
    }

    /// The Master Ball is never spent here, even when it is the only thing
    /// that would work.
    #[test]
    fn the_master_ball_is_never_spent() {
        let bag = [Item { id: 1, count: 1 }, Item { id: 4, count: 9 }];
        let mut s = sit(&bag, &[]);
        s.target = target(3, 300, 300, 0);
        // Hopeless with a Poké Ball, certain with the Master Ball — and the
        // answer is still not the Master Ball.
        match decide(&s, &wants()) {
            Verdict::Throw(t) => assert_ne!(t.ball, Ball::Master),
            Verdict::Weaken { .. } | Verdict::No { .. } => {}
            other => panic!("{other:?}"),
        }
        // With nothing else at all it declines rather than reaching for it.
        let only = [Item { id: 1, count: 1 }];
        let s = sit(&only, &[]);
        assert!(matches!(decide(&s, &wants()), Verdict::No { .. }));
        // …and the gate still says the capability exists, because it does.
        assert!(holds_a_ball(&only));
    }

    /// The seam a caller uses: everything but the catch rate comes off the
    /// battle policy's own view, so the two cannot be reading different states
    /// of the same machine.
    #[test]
    fn a_situation_built_from_a_view_reads_the_same_battle() {
        let v = crate::battle::View {
            trainer: false,
            ours: crate::battle::Side {
                hp: 30,
                max_hp: 30,
                ..Default::default()
            },
            theirs: crate::battle::Side {
                species: PIKACHU,
                hp: 7,
                max_hp: 20,
                status: 0x04,
                ..Default::default()
            },
            items: vec![Item { id: 4, count: 3 }],
            incoming_crit: 4,
            ..Default::default()
        };
        let party = [];
        let s = Situation::from_view(&v, ScreenVerdict::Wild, None, 190, &party);
        assert_eq!(s.species, PIKACHU);
        assert_eq!(s.target.hp, 7);
        assert_eq!(s.target.max_hp, 20);
        assert_eq!(s.target.status.0, 0x04);
        assert_eq!(s.target.catch_rate, 190);
        assert_eq!(s.our_hp, 30);
        assert_eq!(s.incoming_crit, 4);
        assert!(matches!(decide(&s, &wants()), Verdict::Throw(_)));
    }

    /// The odds a throw reports must be the ones `capture` computes. Two
    /// implementations of a probability is exactly the drift this policy exists
    /// to avoid.
    #[test]
    fn the_chance_reported_is_captures_own() {
        let balls = [Item { id: 4, count: 5 }];
        let s = sit(&balls, &[]);
        match decide(&s, &wants()) {
            Verdict::Throw(t) => {
                assert_eq!(t.chance, capture::odds(s.target, Ball::Poke).chance)
            }
            other => panic!("{other:?}"),
        }
    }
}

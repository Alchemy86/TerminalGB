//! Gen 1's capture routine, and the exact probability of a ball working right
//! now.
//!
//! # Where this came from
//!
//! **Disassembled from the player's own cartridge**, not from a wiki. Gen 1's
//! capture code is unlike every later generation's in ways that a modern
//! formula gets confidently wrong, so the routine below was read out of the ROM
//! instruction by instruction and is annotated with the bank-3 addresses it was
//! read from. Those addresses are `ItemUsePokeBall` in
//! [`pret/pokered`](https://github.com/pret/pokered)'s
//! `engine/items/item_effects.asm`; the addresses are this cartridge's, the
//! names are the disassembly's, and the two were checked against each other.
//!
//! ```text
//! 03:56FA  call Random           ; .loop — b = rnd1
//! 03:5701  ld a,[wcf91]          ; the ball
//! 03:5702  cp MASTER_BALL(1)     ; -> captured
//! 03:5707  cp POKE_BALL(4)       ; -> skip the threshold entirely
//! 03:570B  ld a,200 / cp b       ; jr c,.loop  — GREAT and better
//! 03:5711  cp GREAT_BALL(3)      ; -> skip the second threshold
//! 03:5715  ld a,150 / cp b       ; jr c,.loop  — ULTRA / SAFARI
//! 03:571A  ld a,[wEnemyMonStatus]
//! 03:5720  and (1<<FRZ)|SLP_MASK ; $27
//! 03:5722  ld c,12 / ld c,25     ; the status bonus
//! 03:5729  sub c / jp c,.captured; b < bonus -> caught outright
//! 03:572F  hMultiplicand = maxHP, hMultiplier = 255, Multiply
//! 03:5742  cp GREAT_BALL / ld a,12 / ld a,8   ; the ball factor, Divide
//! 03:5754  hp>>2, minimum 1, Divide
//! 03:576D  if the result exceeds 255, clamp the low byte to 255
//! 03:5777  ld a,[wEnemyMonActualCatchRate] / cp b / jr c,.shakes
//! 03:577D  if the divide overflowed 255 -> captured
//! 03:5782  call Random / cp b / jr c,.shakes ; d >= rnd3 -> captured
//! ```
//!
//! # The three things that make Gen 1 different
//!
//! 1. **The ball check is a resampling loop, not a rejection.** `jr c, .loop`
//!    at `03:570E` jumps *backwards to the `call Random`*. A Great Ball does
//!    not fail when the first random byte exceeds 200 — it draws again. So the
//!    ball's effect at this step is to make `b` uniform on `0..=200` (Great) or
//!    `0..=150` (Ultra and Safari) instead of `0..=255`, which makes a low `b`
//!    more likely and is *help*, not a hurdle. Reading it as a failure — the
//!    natural reading of a modern formula — makes every Great and Ultra Ball
//!    number too low.
//! 2. **The status bonus is subtracted, and can win outright.** `sub c` then
//!    `jp c` at `03:5729`: if the random byte is *below* the bonus the Pokémon
//!    is caught with no further checks at all. Sleep and freeze are worth 25,
//!    every other status 12.
//! 3. **The shake count is cosmetic.** Everything from `03:578D` runs only
//!    after the outcome is already decided, and computes what
//!    `wPokeBallAnimData` shows. "Aww! It appeared to be caught!" is three
//!    shakes, not a near miss — the miss happened earlier.
//!
//! # What "the chance" means here
//!
//! The routine draws up to three bytes from `Random` (`03:56FA`, `03:5782`).
//! Those are treated as **independent and uniform over `0..=255`**, which is
//! the standard assumption for Gen 1's `Random` (it mixes `rDIV` into a running
//! pair each call) and the only one under which a single number can be quoted
//! at all. Under that assumption the probability here is **exact, not
//! sampled** — [`Odds::chance`] is a closed form over the routine's own integer
//! arithmetic, and `the_closed_form_matches_the_routine_it_describes`
//! enumerates every reachable random triple and checks the two agree.

use super::gen1::{Mon, Status};

/// The balls a wild encounter can be judged against, with the item ids the
/// routine compares (`wcf91`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ball {
    Master,
    Ultra,
    Great,
    Poke,
    /// The Safari Zone's ball. It takes the same path as an Ultra Ball
    /// (`03:5715` is reached for any id that is not 1, 3 or 4).
    Safari,
}

/// The bag item id of each ball, **taken from the routine's own comparisons**
/// rather than from a transcribed constant list.
///
/// `ItemUsePokeBall` compares `wcf91` against `MASTER_BALL` at `03:5702`,
/// `POKE_BALL` at `03:5707` and `GREAT_BALL` at `03:5711`, and the immediates
/// in those three `cp`s are 1, 4 and 3 — the same bytes the module transcript
/// above records. `ULTRA_BALL` is 2, the remaining id below `GREAT_BALL` that
/// falls through to `03:5715`; `SAFARI_BALL` is 8 and reaches `03:5715` too,
/// which is why it and the Ultra Ball share a threshold here.
pub const ITEM_IDS: [(u8, Ball); 5] = [
    (1, Ball::Master),
    (2, Ball::Ultra),
    (3, Ball::Great),
    (4, Ball::Poke),
    (8, Ball::Safari),
];

/// Which ball a bag item id is, or `None` if it is not a ball at all.
///
/// **One mapping, in one place.** A second copy in a caller is how a policy and
/// the odds it quotes end up talking about different items.
pub fn ball_for_item(item: u8) -> Option<Ball> {
    ITEM_IDS.iter().find(|(i, _)| *i == item).map(|(_, b)| *b)
}

impl Ball {
    /// The bag item id to select for this ball.
    pub fn item_id(self) -> u8 {
        ITEM_IDS
            .iter()
            .find(|(_, b)| *b == self)
            .map(|(i, _)| *i)
            .expect("every Ball has an item id")
    }

    pub fn short(self) -> &'static str {
        match self {
            Ball::Master => "MSTR",
            Ball::Ultra => "ULTRA",
            Ball::Great => "GREAT",
            Ball::Poke => "POKé",
            Ball::Safari => "SAFARI",
        }
    }

    /// The largest first random byte the resampling loop accepts. A Poké Ball
    /// never loops, so it keeps the whole range.
    fn accept_max(self) -> u16 {
        match self {
            Ball::Master | Ball::Poke => 255,
            Ball::Great => 200,
            Ball::Ultra | Ball::Safari => 150,
        }
    }

    /// The divisor applied to `maxHP * 255` at `03:5742`. Only the Great Ball
    /// gets the smaller one, which is what makes it better at high HP.
    fn hp_factor(self) -> u32 {
        match self {
            Ball::Great => 8,
            _ => 12,
        }
    }
}

/// Everything the routine reads, gathered so the calculation can be unit-tested
/// without a running game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Target {
    /// `wEnemyMonActualCatchRate` (`$D007`) — the byte the routine itself
    /// compares against, *not* the species' base catch rate. In the Safari Zone
    /// bait and rocks move this, so reading it rather than the ROM table is
    /// what makes the number right there too.
    pub catch_rate: u8,
    pub hp: u16,
    pub max_hp: u16,
    pub status: Status,
}

impl Target {
    pub fn of(mon: &Mon, catch_rate: u8) -> Target {
        Target {
            catch_rate,
            hp: mon.hp,
            max_hp: mon.max_hp,
            status: mon.status,
        }
    }

    /// The bonus subtracted from the first random byte (`03:5722`): 25 for
    /// asleep or frozen, 12 for any other status, 0 for a healthy Pokémon.
    ///
    /// `and (1 << FRZ) | SLP_MASK` is `$27`; the low three bits are the sleep
    /// counter and bit 5 is freeze.
    pub fn status_bonus(self) -> u16 {
        if self.status.0 == 0 {
            0
        } else if self.status.0 & 0x27 != 0 {
            25
        } else {
            12
        }
    }

    /// `d` from `03:572F`–`03:5776`: `maxHP * 255 / ballFactor`, divided by
    /// `max(1, hp / 4)`.
    ///
    /// Returns `None` when the result exceeds 255, which the routine treats as
    /// a **guaranteed** capture (`03:577D`: the divide's third byte is still
    /// non-zero, so it jumps straight to `.captured`). That is the low-HP case
    /// every player already knows by feel.
    pub fn hp_term(self, ball: Ball) -> Option<u8> {
        let w = self.max_hp as u32 * 255 / ball.hp_factor();
        // `srl b / rr a` twice, then `inc a` if the low byte came out zero.
        let x = ((self.hp / 4) & 0xFF).max(1) as u32;
        let d = w / x;
        if d > 255 {
            None
        } else {
            Some(d as u8)
        }
    }
}

/// The chance one ball has of working, right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Odds {
    pub ball: Ball,
    /// Probability in `0.0..=1.0`.
    pub chance: f64,
    /// True when the HP term saturated, i.e. the third random byte cannot save
    /// the Pokémon and only the catch-rate comparison is left.
    pub hp_maxed: bool,
}

/// The exact probability that `ball` catches `t`.
///
/// A closed form over the routine's own integer arithmetic — see the module
/// docs for the independence assumption, which is the only approximation in it.
pub fn odds(t: Target, ball: Ball) -> Odds {
    // `03:5702`: the Master Ball never reaches any of the arithmetic.
    if ball == Ball::Master {
        return Odds {
            ball,
            chance: 1.0,
            hp_maxed: true,
        };
    }
    let n = ball.accept_max() + 1; // the values `b` can take, after the loop
    let s = t.status_bonus();

    // `03:5729`: b < bonus is a capture with nothing else checked.
    let outright = s.min(n);

    // Otherwise b is in `s..=min(catchRate + s, accept_max)`; anything above
    // that fails the `cp b` at `03:577A`.
    let upper = (t.catch_rate as u16 + s).min(ball.accept_max());
    let surviving = if upper >= s { upper - s + 1 } else { 0 };

    let (hp_pass, hp_maxed) = match t.hp_term(ball) {
        // `03:577D`: the divide overflowed — captured without a third roll.
        None => (1.0, true),
        // `03:5782`: captured when `d >= rnd3`, and rnd3 is a whole byte.
        Some(d) => ((d as f64 + 1.0) / 256.0, false),
    };

    let chance = (outright as f64 + surviving as f64 * hp_pass) / n as f64;
    Odds {
        ball,
        chance: chance.clamp(0.0, 1.0),
        hp_maxed,
    }
}

/// The balls worth comparing in this encounter: the Safari Zone only has one,
/// everywhere else the three a player actually chooses between.
pub fn ball_set(safari: bool) -> &'static [Ball] {
    if safari {
        &[Ball::Safari]
    } else {
        &[Ball::Poke, Ball::Great, Ball::Ultra]
    }
}

/// Run the routine itself for one triple of random bytes, so the closed form
/// can be checked against the thing it claims to describe.
///
/// This is the disassembly transcribed literally, resampling loop and all;
/// `rnd1` is already assumed to have passed the loop.
#[cfg(test)]
fn caught(t: Target, ball: Ball, rnd1: u8, rnd3: u8) -> bool {
    if ball == Ball::Master {
        return true;
    }
    if rnd1 as u16 > ball.accept_max() {
        unreachable!("the loop only exits with b <= accept_max");
    }
    let s = t.status_bonus();
    let b = rnd1 as u16;
    if b < s {
        return true; // 03:572A  jp c,.captured
    }
    let b = b - s;
    if (t.catch_rate as u16) < b {
        return false; // 03:577B  jr c,.shakes
    }
    match t.hp_term(ball) {
        None => true,         // 03:5780  jr nz,.captured
        Some(d) => d >= rnd3, // 03:5789  jr c,.shakes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(catch_rate: u8, hp: u16, max_hp: u16, status: u8) -> Target {
        Target {
            catch_rate,
            hp,
            max_hp,
            status: Status(status),
        }
    }

    /// The closed form and the transcribed routine must agree **exactly**, for
    /// every random byte the loop can produce, across a spread of states. If
    /// they ever disagree the closed form is wrong, because the routine is the
    /// thing being described.
    #[test]
    fn the_closed_form_matches_the_routine_it_describes() {
        let states = [
            target(45, 41, 41, 0),
            target(45, 1, 41, 0),
            target(45, 20, 41, 0x04), // asleep (sleep counter 4)
            target(45, 20, 41, 0x08), // poisoned
            target(3, 300, 300, 0),   // a legendary at full health
            target(3, 1, 300, 0x20),  // …frozen and on one HP
            target(255, 100, 100, 0),
            target(0, 50, 100, 0),
            target(190, 7, 19, 0x40), // paralysed
        ];
        for t in states {
            for ball in [
                Ball::Poke,
                Ball::Great,
                Ball::Ultra,
                Ball::Safari,
                Ball::Master,
            ] {
                let n = ball.accept_max() + 1;
                let mut hits = 0u32;
                for b in 0..n {
                    for r in 0..=255u16 {
                        if caught(t, ball, b as u8, r as u8) {
                            hits += 1;
                        }
                    }
                }
                let enumerated = hits as f64 / (n as f64 * 256.0);
                let closed = odds(t, ball).chance;
                assert!(
                    (enumerated - closed).abs() < 1e-12,
                    "{ball:?} on {t:?}: enumerated {enumerated} vs closed {closed}"
                );
            }
        }
    }

    /// Worked by hand from the disassembly, so the test is not the
    /// implementation restated. A level-15 Pokémon (41 max HP) with the common
    /// catch rate of 45, untouched, and a Poké Ball:
    ///
    /// * `b` is uniform on `0..=255`; it must be `<= 45`, so 46 of 256.
    /// * `w = 41 * 255 / 12 = 871`; `x = max(1, 41/4) = 10`; `d = 87`.
    /// * the third byte must be `<= 87`, so 88 of 256.
    #[test]
    fn a_full_health_poke_ball_throw_is_worked_by_hand() {
        let t = target(45, 41, 41, 0);
        let o = odds(t, Ball::Poke);
        assert!(!o.hp_maxed);
        let want = (46.0 / 256.0) * (88.0 / 256.0);
        assert!((o.chance - want).abs() < 1e-12, "{}", o.chance);
        assert!((o.chance - 0.0618).abs() < 0.001);
    }

    /// The same Pokémon on one hit point: `x` is 1, so `w = 871` overflows the
    /// byte and the third roll never happens — the whole chance is the
    /// catch-rate comparison.
    #[test]
    fn one_hit_point_removes_the_hp_roll_entirely() {
        let o = odds(target(45, 1, 41, 0), Ball::Poke);
        assert!(o.hp_maxed);
        assert!((o.chance - 46.0 / 256.0).abs() < 1e-12, "{}", o.chance);
    }

    /// Sleep is worth 25 and is *subtracted*, so it both catches outright and
    /// widens the surviving range. On one HP with catch rate 45: 25 outright
    /// plus `b` in `25..=70`, all of 256.
    #[test]
    fn sleep_is_a_subtraction_not_a_multiplier() {
        let o = odds(target(45, 1, 41, 0x04), Ball::Poke);
        assert!((o.chance - 71.0 / 256.0).abs() < 1e-12, "{}", o.chance);
        // Any other status is worth 12 rather than 25.
        assert_eq!(target(45, 1, 41, 0x08).status_bonus(), 12);
        assert_eq!(target(45, 1, 41, 0x40).status_bonus(), 12);
        assert_eq!(target(45, 1, 41, 0x20).status_bonus(), 25, "freeze");
        assert_eq!(target(45, 1, 41, 0x00).status_bonus(), 0);
    }

    /// The Master Ball is the one unconditional path.
    #[test]
    fn the_master_ball_always_works() {
        for hp in [1u16, 100, 999] {
            assert_eq!(odds(target(0, hp, 999, 0), Ball::Master).chance, 1.0);
        }
    }

    /// A catch rate of 3 — Mewtwo, the birds — is brutal, and the numbers must
    /// say so rather than rounding up into something encouraging.
    #[test]
    fn a_legendary_is_as_bad_as_it_feels() {
        let full = odds(target(3, 300, 300, 0), Ball::Ultra);
        assert!(full.chance < 0.01, "{}", full.chance);
        let asleep_and_hurt = odds(target(3, 1, 300, 0x04), Ball::Ultra);
        assert!(asleep_and_hurt.chance > full.chance);
        // Even then it is a long way from certain.
        assert!(asleep_and_hurt.chance < 0.25, "{}", asleep_and_hurt.chance);
    }

    /// The resampling loop is *help*, not a hurdle: narrowing `b` to `0..=150`
    /// makes a low `b` likelier. Reading `jr c, .loop` as a failure — which is
    /// what a modern formula implies — would make the Ultra Ball look worse
    /// than the Poké Ball, and on one HP it is plainly better.
    #[test]
    fn the_ball_threshold_helps_rather_than_hurts() {
        let t = target(45, 1, 41, 0);
        let poke = odds(t, Ball::Poke).chance;
        let great = odds(t, Ball::Great).chance;
        let ultra = odds(t, Ball::Ultra).chance;
        assert!(poke < great && great < ultra, "{poke} {great} {ultra}");
        assert!((poke - 46.0 / 256.0).abs() < 1e-12);
        assert!((great - 46.0 / 201.0).abs() < 1e-12);
        assert!((ultra - 46.0 / 151.0).abs() < 1e-12);
    }

    /// …and the Great Ball's smaller HP divisor is what wins it back at full
    /// health. Both facts are Gen 1 oddities and both fall out of the routine.
    #[test]
    fn the_great_balls_hp_divisor_wins_it_back_at_full_health() {
        let t = target(45, 300, 300, 0);
        assert_eq!(t.hp_term(Ball::Great), Some(127));
        assert_eq!(t.hp_term(Ball::Poke), Some(85));
        assert!(odds(t, Ball::Great).chance > odds(t, Ball::Ultra).chance);
    }

    /// The panel's whole point: the number must move the right way as a battle
    /// goes on. Chipping HP never makes it worse, and neither does a status.
    #[test]
    fn the_chance_only_improves_as_the_battle_goes_your_way() {
        for ball in [Ball::Poke, Ball::Great, Ball::Ultra] {
            let mut prev = 0.0;
            for hp in (1..=200u16).rev() {
                let c = odds(target(45, hp, 200, 0), ball).chance;
                assert!(c >= prev - 1e-12, "{ball:?} at {hp} HP: {c} < {prev}");
                prev = c;
            }
            for st in [0x08u8, 0x40, 0x04, 0x20] {
                assert!(
                    odds(target(45, 50, 200, st), ball).chance
                        >= odds(target(45, 50, 200, 0), ball).chance
                );
            }
        }
    }

    /// The item ids and the balls are one mapping, both ways round. A caller
    /// that kept its own copy would eventually disagree with the routine the
    /// odds are computed from.
    #[test]
    fn every_ball_round_trips_through_its_item_id() {
        for (id, ball) in ITEM_IDS {
            assert_eq!(ball_for_item(id), Some(ball));
            assert_eq!(ball.item_id(), id);
        }
        // The three the routine names by hand, spelled out so a renumbering
        // would be caught here rather than in a run.
        assert_eq!(ball_for_item(1), Some(Ball::Master));
        assert_eq!(ball_for_item(3), Some(Ball::Great));
        assert_eq!(ball_for_item(4), Some(Ball::Poke));
        // A potion is not a ball, and nor is anything else.
        assert_eq!(ball_for_item(0x14), None);
        assert_eq!(ball_for_item(0), None);
    }

    /// A catch rate of zero can still be caught, but only by the status bonus —
    /// which is exactly the sort of edge a "rate/255" formula gets wrong.
    #[test]
    fn a_zero_catch_rate_is_not_always_zero() {
        assert_eq!(odds(target(0, 1, 41, 0), Ball::Poke).chance, 1.0 / 256.0);
        let asleep = odds(target(0, 1, 41, 0x04), Ball::Poke).chance;
        assert!((asleep - 26.0 / 256.0).abs() < 1e-12, "{asleep}");
    }
}

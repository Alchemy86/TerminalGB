//! The arithmetic, reproduced instruction for instruction.
//!
//! Every function here is a transcription of a named routine in
//! `pret/pokered`, and every one keeps the cartridge's truncation, its
//! saturation and its off-by-ones. Where a cleaner formula gives the same answer
//! for most inputs, the cartridge's form is used anyway — the differences are
//! exactly the cases the differential harness exists to find.
//!
//! Three rules were followed throughout:
//!
//! 1. **Integers only.** The cartridge has no floating point. A rate expressed
//!    as `x/256` stays a comparison against a byte.
//! 2. **The dice are drawn in the cartridge's order and count.** A model that
//!    gets a turn right but draws a different number of random bytes diverges on
//!    the *next* turn. [`crate::rng::Rng::rolls`] is how that is caught.
//! 3. **Bugs are behaviour.** The Focus Energy inversion, the 1/256 miss and the
//!    badge-boost stacking are not corrected here. A model that "fixes" them is
//!    a model of a game nobody played.

use crate::data::{GameData, MoveData};
use crate::rng::Rng;
use crate::state::{BattleMon, Stages, Stat, Stats, Status, MAX_STAT_VALUE};
use crate::types::{Type, TypeChart};

/// `StatModifierRatios` — `data/battle/stat_modifiers.asm`, numerator then
/// denominator, indexed by `stage - 1`.
pub const STAT_MODIFIER_RATIOS: [(u16, u16); 13] = [
    (25, 100), // stage 1  (−6)
    (28, 100),
    (33, 100),
    (40, 100),
    (50, 100),
    (66, 100),
    (1, 1), // stage 7  (neutral)
    (15, 10),
    (2, 1),
    (25, 10),
    (3, 1),
    (35, 10),
    (4, 1), // stage 13 (+6)
];

/// `HighCriticalMoves` — `data/battle/critical_hit_moves.asm`:
/// KARATE CHOP, RAZOR LEAF, CRABHAMMER, SLASH.
pub const HIGH_CRITICAL_MOVES: [u8; 4] = [2, 75, 152, 163];

/// `MIN_NEUTRAL_DAMAGE` / `MAX_NEUTRAL_DAMAGE` — `constants/battle_constants.asm`.
pub const MIN_NEUTRAL_DAMAGE: u16 = 2;
pub const MAX_NEUTRAL_DAMAGE: u16 = 999;

/// The `percent` macro — `macros/data.asm`: `DEF percent EQUS "* $ff / 100"`.
///
/// This is why 100 % accuracy is stored as 255 and why "25 % paralysis" is
/// really 63/256. Never write a percentage as a fraction of 100 in this crate.
pub const fn percent(n: u16) -> u8 {
    ((n * 0xFF) / 100) as u8
}

// ---------------------------------------------------------------------------
// Critical hits
// ---------------------------------------------------------------------------

/// The critical-hit threshold, as `CriticalHitTest` computes it into `b`.
///
/// `engine/battle/core.asm`, `CriticalHitTest`. Reading it top to bottom:
///
/// ```text
/// ld a, [wMonHBaseSpeed]   ; BASE Speed, from the base-stat table
/// srl b                    ; T = base_speed / 2
/// bit GETTING_PUMPED, a
/// jr nz, .focusEnergyUsed  ; bug: ... a shift to the RIGHT instead of left,
///                          ;      resulting in 1/4 the usual crit chance
/// sla b                    ; no Focus Energy: T *= 2 (saturating)
/// .focusEnergyUsed: srl b  ; Focus Energy:    T /= 2
/// ; then, per move:
/// srl b                    ; ordinary move:   T /= 2
/// sla b / sla b            ; high-crit move:  T *= 4 (saturating)
/// ```
///
/// So an ordinary move with no Focus Energy nets `base_speed / 2`; **Focus
/// Energy quarters that** instead of doubling it, which is the single most
/// famous bug in the game and is reproduced here rather than corrected.
///
/// The threshold saturates at `$FF`, and the comparison is `cp b / ret nc`, so
/// the ceiling is 255/256 — never certain.
pub fn crit_threshold(base_speed: u8, focus_energy: bool, move_id: u8) -> u8 {
    // `sla b / jr nc / ld b, $ff` — a saturating double.
    let sat_double = |v: u8| -> u8 { v.saturating_mul(2) };
    let mut t = base_speed >> 1;
    if focus_energy {
        t >>= 1;
    } else {
        t = sat_double(t);
    }
    if HIGH_CRITICAL_MOVES.contains(&move_id) {
        t = sat_double(t);
        t = sat_double(t);
    } else {
        t >>= 1;
    }
    t
}

/// Roll for a critical hit.
///
/// The roll is `rlc a` three times — a **rotate**, not a shift — and then
/// `cp b / ret nc`. A rotation is a bijection on 0..=255 so the roll is still
/// uniform, but reproducing it matters when the dice come from a recording.
///
/// A move with zero base power never even reaches the roll (`ld a, [hld] /
/// and a / ret z`), so no die is drawn for it. That is a *consumption* fact and
/// it is why this returns early rather than rolling and discarding.
pub fn critical_hit(
    rng: &mut dyn Rng,
    base_speed: u8,
    focus_energy: bool,
    move_data: &MoveData,
) -> bool {
    if move_data.power == 0 {
        return false;
    }
    let threshold = crit_threshold(base_speed, focus_energy, move_data.id);
    let r = rng.next_byte().rotate_left(3);
    r < threshold
}

// ---------------------------------------------------------------------------
// Damage
// ---------------------------------------------------------------------------

/// The offensive and defensive stats and the level that go into
/// [`base_damage`], as `GetDamageVarsForPlayerAttack` sets them up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DamageVars {
    pub attack: u16,
    pub defense: u16,
    pub power: u8,
    /// Doubled already if this is a critical hit.
    pub level: u8,
}

/// Set up the damage variables.
///
/// `GetDamageVarsForPlayerAttack` / `GetDamageVarsForEnemyAttack`
/// (`engine/battle/core.asm`). Four things happen in order and all four matter:
///
/// 1. **The physical/special split is the move's type**, `cp SPECIAL`.
/// 2. **Reflect doubles the defender's Defense, Light Screen its Special** —
///    and the cartridge's own comment says these *"do not cap the stat at
///    `MAX_STAT_VALUE`, so weird things will happen during stats scaling"*.
/// 3. **A critical hit re-reads both stats unmodified** — the attacker's
///    offense and the defender's defense — so a crit discards stat stages in
///    *both* directions, and discards the screens with them. A crit into a
///    Swords-Danced attacker is weaker than a normal hit.
/// 4. **The 10-bit scaling.** If either stat exceeds a byte, both are divided by
///    four. The offense is floored at 1; the *defense is not*, and the
///    disassembly notes it "can actually end up as 0, leading to a division by 0
///    freeze". Here that becomes a defense of 1, because a model that hangs is
///    useless — [`DamageVars`] callers can detect it via
///    [`would_divide_by_zero`].
#[allow(clippy::too_many_arguments)]
pub fn damage_vars(
    attacker: &BattleMon,
    defender: &BattleMon,
    move_data: &MoveData,
    critical: bool,
) -> DamageVars {
    let special = move_data.is_special();

    let (mut atk, mut def) = if critical {
        // Unmodified on both sides — but "unmodified" is a different thing for
        // the enemy, which has its stat *recomputed* from base stats and DVs
        // rather than read back. See [`BattleMon::crit_stats`].
        let a = attacker.stats_for_crit();
        let d = defender.stats_for_crit();
        if special {
            (a.special, d.special)
        } else {
            (a.attack, d.defense)
        }
    } else {
        let mut d = if special {
            defender.current.special
        } else {
            defender.current.defense
        };
        // The screens double the defensive stat, uncapped.
        let screened = if special {
            defender.volatile.light_screen
        } else {
            defender.volatile.reflect
        };
        if screened {
            d = d.wrapping_mul(2);
        }
        let a = if special {
            attacker.current.special
        } else {
            attacker.current.attack
        };
        (a, d)
    };

    // .scaleStats — only `or b` on the *defensive* high byte and the offensive
    // 16-bit value decide whether to scale, and only the low byte survives.
    if atk > 0xFF || def > 0xFF {
        def /= 4;
        atk /= 4;
        if atk == 0 {
            atk = 1;
        }
        // The cartridge keeps only `l`, the low byte, of each.
        atk &= 0xFF;
        def &= 0xFF;
    }

    let level = if critical {
        // `sla e` — a byte, so a level above 127 would wrap. Unreachable in the
        // released game, kept faithful anyway.
        attacker.level.wrapping_mul(2)
    } else {
        attacker.level
    };

    DamageVars {
        attack: atk,
        defense: def,
        power: move_data.power,
        level,
    }
}

/// Whether these variables would hang the cartridge.
///
/// The disassembly's own comment on `.scaleStats`: *"defensive stat can actually
/// end up as 0, leading to a division by 0 freeze during damage calculation"*.
/// The model substitutes 1 so an episode can finish; this predicate is how a
/// report can say the real machine would have locked up instead.
pub fn would_divide_by_zero(v: &DamageVars) -> bool {
    v.defense == 0
}

/// `CalculateDamage` — the base damage, before STAB, type and the random roll.
///
/// ```text
/// level * 2      ; 16-bit
/// / 5            ; truncating
/// + 2
/// * base power
/// * attack
/// / defense      ; truncating
/// / 50           ; truncating
/// cap at 997     ; MAX_NEUTRAL_DAMAGE - MIN_NEUTRAL_DAMAGE
/// + 2            ; MIN_NEUTRAL_DAMAGE, added AFTER the cap
/// ```
///
/// Two details that a "cleaner" formula loses. The `+2` is applied **after** the
/// cap, so the true ceiling is 999 and not 997. And every division truncates,
/// including the `/5` — writing `2*L/5 + 2` as a single expression in a wider
/// type gives a different answer than doing it in the cartridge's order.
///
/// Returns `None` for zero power, which is `ret z` and means the whole damage
/// pipeline including the accuracy check is skipped.
pub fn base_damage(v: &DamageVars) -> Option<u16> {
    if v.power == 0 {
        return None;
    }
    let defense = if v.defense == 0 { 1 } else { v.defense };
    let mut d: u32 = u32::from(v.level) * 2;
    d /= 5;
    d += 2;
    d *= u32::from(v.power);
    d *= u32::from(v.attack);
    d /= u32::from(defense);
    d /= 50;
    let capped = d.min(u32::from(MAX_NEUTRAL_DAMAGE - MIN_NEUTRAL_DAMAGE)) as u16;
    Some(capped + MIN_NEUTRAL_DAMAGE)
}

/// The result of applying STAB and the type chart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TypedDamage {
    pub damage: u16,
    pub stab: bool,
    /// Effectiveness as a numerator over 100 — 200 for ×2, 25 for ×0.25.
    pub effectiveness: u16,
    /// The type chart drove the damage to zero, which the cartridge turns into
    /// a **miss**, not a zero-damage hit.
    pub missed_by_type: bool,
}

/// `AdjustDamageForMoveType` — STAB first, then the chart, truncating at every
/// step.
///
/// STAB is written as `damage + (damage >> 1)`, which is what the cartridge does
/// (`srl b / rr c / add hl, bc`), not `damage * 15 / 10`. The two agree for
/// every value, and the shift form is the one a reader can check against the
/// assembly.
///
/// The last line is the surprise. If the chart drives the damage to zero the
/// cartridge sets `wMoveMissed` — the disassembly notes this *"only occurs if a
/// move that would do 2 or 3 damage is 0.25x effective"*. A resisted move can
/// therefore **miss**, which no amount of accuracy will prevent.
pub fn adjust_for_type(
    damage: u16,
    move_kind: Type,
    attacker: &BattleMon,
    defender: &BattleMon,
    chart: &TypeChart,
) -> TypedDamage {
    let stab = move_kind == attacker.type1 || move_kind == attacker.type2;
    let mut d = damage;
    if stab {
        d = d.saturating_add(d >> 1);
    }
    let types = defender.types();
    let effectiveness = chart.effectiveness_percent(move_kind, types);
    d = chart.apply(d, move_kind, types);
    TypedDamage {
        damage: d,
        stab,
        effectiveness,
        missed_by_type: d == 0,
    }
}

/// `RandomizeDamage` — the 217..=255 roll.
///
/// ```text
/// ret c        ; damage 0 or 1 is not randomised at all
/// .loop
///   call BattleRandom
///   rrca                  ; ROTATE right, not shift
///   cp 85 percent + 1     ; 217
///   jr c, .loop           ; reject below 217
/// Multiply / Divide by 255
/// ```
///
/// Two things this must not be simplified into. It is a **rejection loop**, so a
/// single turn can consume any number of random bytes — which is exactly why
/// [`crate::rng::Rng::rolls`] is part of the comparison against a real battle.
/// And damage of 0 or 1 skips it entirely, which is how the minimum of 1 damage
/// survives: it is never multiplied down, it is never *reached*.
pub fn randomize_damage(rng: &mut dyn Rng, damage: u16) -> u16 {
    if damage <= 1 {
        return damage;
    }
    let r = loop {
        let b = rng.next_byte().rotate_right(1);
        // `cp 85 percent + 1 / jr c, .loop` — reject anything below 217.
        if b > percent(85) {
            break b;
        }
    };
    ((u32::from(damage) * u32::from(r)) / 255) as u16
}

// ---------------------------------------------------------------------------
// Accuracy
// ---------------------------------------------------------------------------

/// `CalcHitChance` — scale a move's accuracy by the attacker's accuracy stage
/// and the defender's evasion stage.
///
/// The evasion stage is *reflected over neutral* first (`ld a, $0e / sub c`), so
/// a defender at +2 evasion (stored 9) indexes the ratio table at `14 - 9 = 5`,
/// which is the 0.50 row. Accuracy is applied first, evasion second, each
/// truncating and each floored at 1; the final value is capped at 255.
pub fn hit_chance(
    accuracy: u8,
    attacker_stages: &Stages,
    defender_stages: &Stages,
) -> u8 {
    let acc_mod = attacker_stages.accuracy;
    let eva_mod = 14u8.saturating_sub(defender_stages.evasion);
    let step = |v: u32, stage: u8| -> u32 {
        let idx = (stage.max(1) as usize - 1).min(STAT_MODIFIER_RATIOS.len() - 1);
        let (num, den) = STAT_MODIFIER_RATIOS[idx];
        let r = v * u32::from(num) / u32::from(den);
        if r == 0 {
            1
        } else {
            r
        }
    };
    let v = step(step(u32::from(accuracy), acc_mod), eva_mod);
    v.min(255) as u8
}

/// `MoveHitTest`'s final comparison.
///
/// ```text
/// ; if the random number generated is greater than or equal to the scaled
/// ; accuracy, the move misses
/// ; note that this means that even the highest accuracy is still just a
/// ; 255/256 chance, not 100%
/// call BattleRandom
/// cp b
/// jr nc, .moveMissed
/// ```
///
/// **This is the 1/256 miss**, and it is a property of the comparison, not a
/// special case: a random byte is uniform on 0..=255, a 100 % move stores 255,
/// and `255 >= 255` misses. Nothing in the game can raise a move above 255/256.
pub fn accuracy_roll(rng: &mut dyn Rng, scaled_accuracy: u8) -> bool {
    rng.next_byte() < scaled_accuracy
}

// Move-effect ids live in [`crate::effects`]; `SWIFT_EFFECT` ($11) returns from
// `MoveHitTest` before *everything*, including the substitute and
// Fly/Dig-invulnerability checks — *"Swift never misses (this was fixed from
// the Japanese versions)"*. Its side effect is the drain-versus-Substitute bug
// two lines below it, modelled in [`crate::sim`].

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

/// `CalculateModifiedStat` — recompute one stat from its **unmodified** value
/// and its stage.
///
/// Capped at 999 and floored at 1. The load-bearing word is *unmodified*: the
/// recomputation always starts from `wPlayerMonUnmodifiedAttack`, which is why a
/// stat change silently discards a burn's halved attack and a paralysis's
/// quartered speed until they are re-applied.
pub fn modified_stat(unmodified: u16, stage: u8) -> u16 {
    let idx = (stage.max(1) as usize - 1).min(STAT_MODIFIER_RATIOS.len() - 1);
    let (num, den) = STAT_MODIFIER_RATIOS[idx];
    let v = u32::from(unmodified) * u32::from(num) / u32::from(den);
    let capped = v.min(u32::from(MAX_STAT_VALUE)) as u16;
    if capped == 0 {
        1
    } else {
        capped
    }
}

/// `CalculateModifiedStats` — all four, from the unmodified block.
pub fn modified_stats(unmodified: &Stats, stages: &Stages) -> Stats {
    Stats {
        attack: modified_stat(unmodified.attack, stages.attack),
        defense: modified_stat(unmodified.defense, stages.defense),
        speed: modified_stat(unmodified.speed, stages.speed),
        special: modified_stat(unmodified.special, stages.special),
    }
}

/// `ApplyBadgeStatBoosts` — ×1.125 per badge, on the **current** stat.
///
/// ```text
/// ; the boost is applied for badges whose bit position is even
/// ; Boulder (bit 0) - attack
/// ; Thunder (bit 2) - defense
/// ; Soul    (bit 4) - speed
/// ; Volcano (bit 6) - special
/// ; multiply stat at hl by 1.125
/// ```
///
/// The odd-numbered badges — Cascade, Rainbow, Marsh, Earth — boost nothing.
///
/// **This stacks, and the stacking is the point.** The boost multiplies the stat
/// already in the battle struct, and the routine is re-run on switch-in, every
/// time *you* use a stat-up move, and every time the *enemy* uses a stat-down
/// move. The disassembly says so in as many words: *"whenever the player uses a
/// stat-up move, badge boosts get reapplied again to every stat, even to those
/// not affected by the stat-up move (will be boosted further)"*. Modelling it as
/// a one-off multiplier on the unmodified stat would be a different game.
pub fn apply_badge_boosts(stats: &mut Stats, badges: u8) {
    let boost = |s: u16| -> u16 { (s + (s >> 3)).min(MAX_STAT_VALUE) };
    if badges & (1 << 0) != 0 {
        stats.attack = boost(stats.attack);
    }
    if badges & (1 << 2) != 0 {
        stats.defense = boost(stats.defense);
    }
    if badges & (1 << 4) != 0 {
        stats.speed = boost(stats.speed);
    }
    if badges & (1 << 6) != 0 {
        stats.special = boost(stats.special);
    }
}

/// `HalveAttackDueToBurn` — Attack /= 2, minimum 1.
pub fn apply_burn_penalty(stats: &mut Stats, status: Status) {
    if status.is_burned() {
        stats.attack = (stats.attack / 2).max(1);
    }
}

/// `QuarterSpeedDueToParalysis` — Speed /= 4, minimum 1.
pub fn apply_paralysis_penalty(stats: &mut Stats, status: Status) {
    if status.is_paralysed() {
        stats.speed = (stats.speed / 4).max(1);
    }
}

/// The whole switch-in stat pipeline, in `LoadBattleMonFromParty`'s order:
/// stages, then the burn and paralysis penalties, then the badge boosts.
///
/// The order is not arbitrary. Badges multiply what the penalties left, so a
/// burned Pokémon with the Boulder Badge has `(atk/2) * 1.125`, not
/// `(atk * 1.125) / 2` — the two differ by a point on most values.
pub fn recompute_current_stats(mon: &mut BattleMon, badges: u8, badges_apply: bool) {
    let mut s = modified_stats(&mon.unmodified, &mon.stages);
    apply_paralysis_penalty(&mut s, mon.status);
    apply_burn_penalty(&mut s, mon.status);
    if badges_apply {
        apply_badge_boosts(&mut s, badges);
    }
    mon.current = s;
}

/// What a stat-change move does, faithfully including the two things a clean
/// implementation would leave out.
///
/// After a stat change the cartridge recomputes from the unmodified stat —
/// *"paralysis and burn penalties, as well as badge boosts are ignored"* — and
/// then re-applies badge boosts to **all four** stats and the burn/paralysis
/// penalties to the **other** side only. So using Swords Dance while burned
/// removes your own burn penalty until something else re-applies it, and
/// re-boosts every badge-boosted stat again.
pub fn apply_stat_change(
    mon: &mut BattleMon,
    stat: Stat,
    by: i8,
    badges: u8,
    badges_apply: bool,
) -> bool {
    let moved = mon.stages.shift(stat, by);
    let mut s = modified_stats(&mon.unmodified, &mon.stages);
    if badges_apply {
        apply_badge_boosts(&mut s, badges);
    }
    mon.current = s;
    moved
}

// ---------------------------------------------------------------------------
// Turn order
// ---------------------------------------------------------------------------

/// Move priority. Gen 1 has exactly two priority moves and they are hard-coded
/// in `MainInBattleLoop`, not read from a table.
pub const QUICK_ATTACK: u8 = 98;
pub const COUNTER: u8 = 68;

/// Who moves first.
///
/// `MainInBattleLoop` (`engine/battle/core.asm`) checks, in this order:
/// Quick Attack (moves first), then Counter (moves **last** — it has *negative*
/// priority, which is why a Counter user is struck before it strikes back),
/// then current Speed, then a coin flip.
///
/// The speed comparison uses the **in-battle** Speed, so stages, paralysis and
/// badge boosts all count — unlike the critical-hit test, which uses base Speed.
/// Conflating the two is the classic Gen 1 modelling error.
///
/// The tie is `BattleRandom() < 128`, and it draws a die **only on a tie** —
/// another consumption fact.
pub fn player_moves_first(
    rng: &mut dyn Rng,
    player_move: u8,
    enemy_move: u8,
    player_speed: u16,
    enemy_speed: u16,
) -> bool {
    let pq = player_move == QUICK_ATTACK;
    let eq = enemy_move == QUICK_ATTACK;
    if pq && !eq {
        return true;
    }
    if eq && !pq {
        return false;
    }
    if !pq {
        let pc = player_move == COUNTER;
        let ec = enemy_move == COUNTER;
        if pc && !ec {
            return false;
        }
        if ec && !pc {
            return true;
        }
    }
    match player_speed.cmp(&enemy_speed) {
        core::cmp::Ordering::Greater => true,
        core::cmp::Ordering::Less => false,
        core::cmp::Ordering::Equal => rng.next_byte() < percent(50) + 1,
    }
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// `SleepEffect`'s duration roll: rejection-sample `BattleRandom() & 7` until it
/// is non-zero, giving a uniform 1..=7.
///
/// Sleep costs the full count *plus nothing* — but the turn a Pokémon wakes on
/// is still lost. `CheckPlayerStatusConditions` decrements, and both the
/// still-asleep and the just-woken branches fall into `.sleepDone`, which
/// returns `ExecutePlayerMoveDone`. So a 1-turn sleep costs one turn, not zero.
pub fn roll_sleep_turns(rng: &mut dyn Rng) -> u8 {
    loop {
        let v = rng.next_byte() & Status::SLP_MASK;
        if v != 0 {
            return v;
        }
    }
}

/// `CheckPlayerStatusConditions .ParalysisCheck` — `cp 25 percent`, with no
/// `+ 1`, so the real rate is **63/256 ≈ 24.6 %**, not a quarter.
pub fn fully_paralysed(rng: &mut dyn Rng) -> bool {
    rng.next_byte() < percent(25)
}

/// `HandlePoisonBurnLeechSeed_DecreaseOwnHP` — one sixteenth of maximum HP,
/// minimum 1.
///
/// The cartridge computes it by shifting the 16-bit max HP right four times,
/// then `inc c` if the result is zero. The assumption baked in — that max HP is
/// below 1024 — holds for every legitimate Pokémon.
pub fn residual_damage(max_hp: u16) -> u16 {
    (max_hp / 16).max(1)
}

/// Toxic damage: the same sixteenth, multiplied by the counter *after* it is
/// incremented, so the first tick is 1×.
///
/// The counter is shared with Leech Seed's drain — the disassembly's own note is
/// *"the toxic ticks are considered even if the damage is not poison (hence the
/// Leech Seed glitch)"* — so a badly poisoned, seeded Pokémon has both its
/// drains multiplied and the counter bumped twice a turn.
pub fn toxic_damage(max_hp: u16, counter: u8) -> u16 {
    residual_damage(max_hp).saturating_mul(u16::from(counter))
}

/// `RecoilEffect_` — half the damage dealt for Struggle, a quarter otherwise,
/// minimum 1.
///
/// The input is damage **dealt**, already clamped to what the target had left,
/// not the damage rolled.
pub fn recoil_damage(damage_dealt: u16, is_struggle: bool) -> u16 {
    let d = if is_struggle {
        damage_dealt / 2
    } else {
        damage_dealt / 4
    };
    d.max(1)
}

/// `TwoToFiveAttacksEffect` — the multi-hit count.
///
/// `BattleRandom() & 3`: 0 and 1 give two and three hits; 2 and 3 re-roll into
/// the four/five band, so the distribution is 3/8, 3/8, 1/8, 1/8.
pub fn roll_multi_hit(rng: &mut dyn Rng) -> u8 {
    let v = rng.next_byte() & 3;
    if v < 2 {
        v + 2
    } else {
        (rng.next_byte() & 3) / 2 + 4
    }
}

/// `TrappingEffect` — how many further turns a Wrap/Bind/Fire Spin/Clamp holds.
///
/// Same 3/8, 3/8, 1/8, 1/8 shape, then `inc a`, giving 1..=4 further hits.
pub fn roll_trapping_turns(rng: &mut dyn Rng) -> u8 {
    let v = rng.next_byte() & 3;
    let base = if v < 2 {
        v
    } else {
        (rng.next_byte() & 3) / 2 + 2
    };
    base + 1
}

/// The chance a secondary effect lands, as the byte the cartridge compares
/// against. `FreezeBurnParalyzeEffect` and friends.
pub mod side_effect {
    use super::percent;
    /// `PARALYZE_SIDE_EFFECT1`, `BURN_SIDE_EFFECT1`, `FREEZE_SIDE_EFFECT`,
    /// `FLINCH_SIDE_EFFECT1` — `10 percent + 1` = 26/256.
    pub const WEAK: u8 = percent(10) + 1;
    /// The `2` variants — `30 percent + 1` = 77/256.
    pub const STRONG: u8 = percent(30) + 1;
    /// `POISON_SIDE_EFFECT1` — `20 percent + 1` = 52/256.
    pub const POISON_WEAK: u8 = percent(20) + 1;
    /// `POISON_SIDE_EFFECT2` — `40 percent + 1` = 103/256.
    pub const POISON_STRONG: u8 = percent(40) + 1;
    /// The confusion side effect — `10 percent`, with **no** `+ 1`, so 25/256.
    pub const CONFUSION: u8 = percent(10);
    /// A stat-lowering move's own 25 % miss outside a link battle —
    /// `25 percent + 1`.
    pub const STAT_DOWN_MISS: u8 = percent(25) + 1;
}

/// Look up the base Speed the critical-hit test uses. Returns 0 when the species
/// is unknown, which makes a crit impossible — the honest answer for a Pokémon
/// the data cannot describe.
pub fn base_speed(data: &GameData, mon: &BattleMon) -> u8 {
    data.base_stats_for_index(mon.species)
        .map(|b| b.speed)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::FixedRng;
    use crate::state::{Dvs, MoveSlot, Volatile};

    fn mon(speed: u16, attack: u16, level: u8) -> BattleMon {
        let stats = Stats {
            attack,
            defense: 100,
            speed,
            special: 100,
        };
        BattleMon {
            species: 1,
            level,
            hp: 100,
            max_hp: 100,
            status: Status::HEALTHY,
            type1: Type::NORMAL,
            type2: Type::NORMAL,
            moves: [MoveSlot::empty(); 4],
            unmodified: stats,
            current: stats,
            crit_stats: None,
            stages: Stages::default(),
            volatile: Volatile::default(),
        }
    }

    fn tackle() -> MoveData {
        MoveData {
            id: 33,
            animation: 33,
            effect: 0,
            power: 35,
            kind: Type::NORMAL,
            accuracy: 255,
            pp: 35,
        }
    }

    // -- percent -----------------------------------------------------------

    /// The `percent` macro is `* $ff / 100`, which is why "100 %" is 255 and
    /// "25 %" paralysis is really 63/256.
    #[test]
    fn percent_is_the_cartridges_macro_not_a_hundredth() {
        assert_eq!(percent(100), 255);
        assert_eq!(percent(25), 63);
        assert_eq!(percent(50), 127);
        assert_eq!(percent(85), 216);
        assert_eq!(percent(10), 25);
    }

    // -- critical hits -----------------------------------------------------

    /// An ordinary move with no Focus Energy nets base Speed / 2 — the double
    /// and the halve cancel.
    #[test]
    fn an_ordinary_crit_threshold_is_half_base_speed() {
        assert_eq!(crit_threshold(100, false, 33), 50);
        assert_eq!(crit_threshold(1, false, 33), 0);
    }

    /// **The Focus Energy bug.** It shifts right where it should shift left, so
    /// it *quarters* the crit rate. This must never be "fixed".
    #[test]
    fn focus_energy_quarters_the_crit_rate_instead_of_doubling_it() {
        let without = crit_threshold(100, false, 33);
        let with = crit_threshold(100, true, 33);
        assert_eq!(without, 50);
        assert_eq!(with, 12); // 50/4, truncating through two shifts
        assert!(
            with < without,
            "Focus Energy must make crits rarer in Gen 1"
        );
    }

    /// A high-crit move is eight times the ordinary rate, saturating at 255.
    #[test]
    fn a_high_crit_move_is_eight_times_and_saturates() {
        // Slash, id 163. Base speed 100 → 50 ordinary → 400, saturated to 255.
        assert_eq!(crit_threshold(100, false, 163), 255);
        // Base speed 20 → 10 ordinary → 80.
        assert_eq!(crit_threshold(20, false, 163), 80);
        assert_eq!(crit_threshold(20, false, 33), 10);
    }

    /// The comparison is `cp b / ret nc`, so even a saturated threshold leaves
    /// one losing roll in 256.
    #[test]
    fn a_crit_is_never_certain() {
        // rotate_left(byte, 3) == 255 requires byte == 255.
        let mut rng = FixedRng::constant(0xFF);
        let mut m = tackle();
        m.id = 163; // Slash
        assert!(!critical_hit(&mut rng, 200, false, &m));
    }

    /// A zero-power move never reaches the roll, so it draws no die at all.
    #[test]
    fn a_zero_power_move_draws_no_crit_die() {
        let mut rng = FixedRng::constant(0x00);
        let mut m = tackle();
        m.power = 0;
        assert!(!critical_hit(&mut rng, 200, false, &m));
        assert_eq!(rng.rolls(), 0);
    }

    // -- damage ------------------------------------------------------------

    /// The textbook check: level 100, 100 attack, 100 defense, 100 power gives
    /// 2*100/5 + 2 = 42; 42*100*100/100/50 = 84; +2 = 86.
    #[test]
    fn base_damage_follows_the_cartridges_order_of_operations() {
        let v = DamageVars {
            attack: 100,
            defense: 100,
            power: 100,
            level: 100,
        };
        assert_eq!(base_damage(&v), Some(86));
    }

    /// The `/5` truncates on its own, before the `+2`. Level 3 gives
    /// `6/5 = 1`, not `1.2`, so the chain starts at 3 and not 3.2.
    #[test]
    fn the_level_divide_truncates_before_the_plus_two() {
        let v = DamageVars {
            attack: 10,
            defense: 10,
            power: 40,
            level: 3,
        };
        // (6/5 + 2) = 3; 3*40*10/10/50 = 2; +2 = 4.
        assert_eq!(base_damage(&v), Some(4));
    }

    /// The `+2` is added **after** the 997 cap, so the ceiling is 999.
    #[test]
    fn the_minimum_is_added_after_the_cap_so_the_ceiling_is_999() {
        let v = DamageVars {
            attack: 999,
            defense: 1,
            power: 250,
            level: 100,
        };
        assert_eq!(base_damage(&v), Some(MAX_NEUTRAL_DAMAGE));
    }

    #[test]
    fn a_zero_power_move_calculates_no_damage() {
        let v = DamageVars {
            attack: 100,
            defense: 100,
            power: 0,
            level: 100,
        };
        assert_eq!(base_damage(&v), None);
    }

    /// **A critical hit reads the unmodified stats on BOTH sides**, and doubles
    /// the level.
    ///
    /// From neutral that is roughly double damage, as expected. But the two
    /// halves do not cancel: because the *stat* reverts while only the *level*
    /// doubles, a crit through your own Swords Dance is a **downgrade** — one
    /// of the most consequential Gen 1 quirks, and the reason a model that
    /// applies a flat ×2 for crits mis-scores a whole class of positions.
    #[test]
    fn a_crit_discards_stat_stages_in_both_directions() {
        let def = mon(50, 100, 50);

        // Neutral: the crit is worth roughly double.
        let neutral = mon(50, 100, 50);
        let ord = base_damage(&damage_vars(&neutral, &def, &tackle(), false)).unwrap();
        let cr = base_damage(&damage_vars(&neutral, &def, &tackle(), true)).unwrap();
        assert!(cr > ord, "a crit from neutral is stronger: {cr} vs {ord}");

        // +2 attack: the crit reverts the stat and only doubles the level, so
        // it deals LESS.
        let mut buffed = mon(50, 100, 50);
        buffed.current.attack = 200;
        buffed.stages.attack = 9;
        let vars_ord = damage_vars(&buffed, &def, &tackle(), false);
        let vars_crit = damage_vars(&buffed, &def, &tackle(), true);
        assert_eq!(vars_ord.attack, 200);
        assert_eq!(
            vars_crit.attack, 100,
            "a crit must read the unmodified attack"
        );
        assert_eq!(vars_crit.level, 100, "a crit doubles the level");
        let ord = base_damage(&vars_ord).unwrap();
        let cr = base_damage(&vars_crit).unwrap();
        assert!(
            cr < ord,
            "a crit is a downgrade at +2 attack: {cr} vs {ord}"
        );

        // And the defender's stages are discarded too, which is the other half
        // of the same rule: a crit punches through a Barrier.
        let mut walled = mon(50, 100, 50);
        walled.current.defense = 400;
        walled.stages.defense = 13;
        let through =
            base_damage(&damage_vars(&neutral, &walled, &tackle(), true)).unwrap();
        let blocked =
            base_damage(&damage_vars(&neutral, &walled, &tackle(), false)).unwrap();
        assert!(
            through > blocked * 4,
            "a crit ignores the defender's stages too"
        );
    }

    /// Reflect doubles the defender's Defense, and a crit ignores it.
    #[test]
    fn reflect_doubles_defense_and_a_crit_ignores_it() {
        let atk = mon(50, 100, 50);
        let mut def = mon(50, 100, 50);
        def.volatile.reflect = true;
        assert_eq!(damage_vars(&atk, &def, &tackle(), false).defense, 200);
        assert_eq!(damage_vars(&atk, &def, &tackle(), true).defense, 100);
    }

    /// Light Screen is the special-side twin, and it is selected by the move's
    /// *type*, not by any flag on the move.
    #[test]
    fn light_screen_is_selected_by_the_moves_type() {
        let atk = mon(50, 100, 50);
        let mut def = mon(50, 100, 50);
        def.volatile.light_screen = true;
        def.current.special = 120;
        let mut ember = tackle();
        ember.kind = Type::FIRE; // >= $14, so special
        assert!(ember.is_special());
        assert_eq!(damage_vars(&atk, &def, &ember, false).defense, 240);
        // A physical move ignores Light Screen entirely.
        assert_eq!(damage_vars(&atk, &def, &tackle(), false).defense, 100);
    }

    /// The 10-bit scaling: over 255, both stats are divided by four.
    #[test]
    fn stats_over_a_byte_are_both_scaled_by_four() {
        let mut atk = mon(50, 400, 50);
        atk.current.attack = 400;
        let mut def = mon(50, 100, 50);
        def.current.defense = 300;
        let v = damage_vars(&atk, &def, &tackle(), false);
        assert_eq!(v.attack, 100);
        assert_eq!(v.defense, 75);
    }

    /// The offense is floored at 1 by the scaling; the defense is not, and the
    /// cartridge would divide by zero. The model substitutes 1 and says so.
    #[test]
    fn a_scaled_defense_can_reach_zero_and_is_reportable() {
        let mut atk = mon(50, 100, 50);
        atk.current.attack = 260;
        let mut def = mon(50, 100, 50);
        def.current.defense = 3;
        let v = damage_vars(&atk, &def, &tackle(), false);
        assert_eq!(v.defense, 0);
        assert!(would_divide_by_zero(&v));
        // The model still produces a number rather than hanging.
        assert!(base_damage(&v).is_some());
    }

    // -- STAB and type ------------------------------------------------------

    #[test]
    fn stab_is_the_shift_form_and_is_applied_before_the_chart() {
        let mut atk = mon(50, 100, 50);
        atk.type1 = Type::NORMAL;
        let def = mon(50, 100, 50);
        let t = adjust_for_type(7, Type::NORMAL, &atk, &def, &TypeChart::neutral());
        assert!(t.stab);
        assert_eq!(t.damage, 10); // 7 + (7 >> 1)
    }

    /// A resisted move can be driven to zero damage, and the cartridge calls
    /// that a **miss** — the disassembly notes it "only occurs if a move that
    /// would do 2 or 3 damage is 0.25x effective".
    #[test]
    fn a_quarter_effective_move_on_two_damage_misses() {
        let atk = mon(50, 100, 50);
        let mut def = mon(50, 100, 50);
        def.type1 = Type::BUG;
        def.type2 = Type::POISON;
        let chart = TypeChart::from_entries(vec![
            (Type::GRASS.0, Type::BUG.0, 5),
            (Type::GRASS.0, Type::POISON.0, 5),
        ]);
        let t = adjust_for_type(2, Type::GRASS, &atk, &def, &chart);
        assert_eq!(t.damage, 0);
        assert!(t.missed_by_type);
        assert_eq!(t.effectiveness, 25);
    }

    /// An immunity is zero damage and a miss too.
    #[test]
    fn an_immunity_is_reported_as_a_miss() {
        let atk = mon(50, 100, 50);
        let mut def = mon(50, 100, 50);
        def.type1 = Type::GHOST;
        def.type2 = Type::GHOST;
        let chart = TypeChart::from_entries(vec![(Type::NORMAL.0, Type::GHOST.0, 0)]);
        let t = adjust_for_type(100, Type::NORMAL, &atk, &def, &chart);
        assert_eq!(t.damage, 0);
        assert!(t.missed_by_type);
    }

    // -- the damage roll ----------------------------------------------------

    /// Damage of 0 or 1 skips the roll entirely — which is how the minimum of
    /// one damage survives, and it draws no die.
    #[test]
    fn one_damage_is_never_randomised_and_draws_no_die() {
        let mut rng = FixedRng::constant(0x00);
        assert_eq!(randomize_damage(&mut rng, 1), 1);
        assert_eq!(randomize_damage(&mut rng, 0), 0);
        assert_eq!(rng.rolls(), 0);
    }

    /// The roll is a rejection loop, so one turn can eat several dice.
    #[test]
    fn the_damage_roll_rejects_until_it_lands_in_range() {
        // rotate_right(b, 1) >= 217 requires the rotated value in 217..=255.
        // 0x00 rotates to 0x00 — rejected, every time.
        let mut rng = FixedRng::new(vec![0x00, 0x00, 0xFF]);
        // 0xFF rotates right to 0xFF = 255, accepted.
        assert_eq!(randomize_damage(&mut rng, 100), 100);
        assert_eq!(rng.rolls(), 3, "two rejections then an accept");
    }

    #[test]
    fn the_lowest_accepted_roll_is_about_85_percent() {
        // 217 as the rotated value: rotate_left(217, 1) = 179.
        let mut rng = FixedRng::constant(217u8.rotate_left(1));
        assert_eq!(randomize_damage(&mut rng, 100), 100 * 217 / 255);
    }

    // -- accuracy -----------------------------------------------------------

    /// **The 1/256 miss.** A 100 % move stores 255 and a random byte reaches
    /// 255, and `255 >= 255` misses.
    #[test]
    fn even_a_hundred_percent_move_misses_one_time_in_256() {
        let mut always = FixedRng::constant(0xFF);
        assert!(!accuracy_roll(&mut always, 255));
        let mut never = FixedRng::constant(0xFE);
        assert!(accuracy_roll(&mut never, 255));
    }

    /// Evasion is reflected over neutral before it indexes the ratio table.
    #[test]
    fn evasion_is_reflected_over_neutral() {
        let neutral = Stages::default();
        let mut evasive = Stages::default();
        evasive.evasion = 9; // +2 evasion → index 14-9 = 5 → the 0.50 row
        assert_eq!(hit_chance(255, &neutral, &neutral), 255);
        assert_eq!(hit_chance(255, &neutral, &evasive), 127);
    }

    #[test]
    fn accuracy_and_evasion_stack_and_the_result_is_floored_at_one() {
        let mut blind = Stages::default();
        blind.accuracy = 1; // −6
        let mut evasive = Stages::default();
        evasive.evasion = 13; // +6 → index 1 → the 0.25 row
        assert_eq!(hit_chance(255, &blind, &evasive), 15);
        // A tiny accuracy cannot be scaled to zero.
        assert!(hit_chance(1, &blind, &evasive) >= 1);
    }

    #[test]
    fn hit_chance_is_capped_at_255() {
        let mut sharp = Stages::default();
        sharp.accuracy = 13; // +6 → ×4
        assert_eq!(hit_chance(255, &sharp, &Stages::default()), 255);
    }

    // -- stats --------------------------------------------------------------

    #[test]
    fn a_modified_stat_is_capped_at_999_and_floored_at_1() {
        assert_eq!(modified_stat(100, 7), 100);
        assert_eq!(modified_stat(100, 9), 200);
        assert_eq!(modified_stat(100, 1), 25);
        assert_eq!(modified_stat(400, 13), 999);
        assert_eq!(modified_stat(1, 1), 1);
    }

    /// Only the even-numbered badges boost anything, and the boost is
    /// `s + s/8`.
    #[test]
    fn only_the_even_badges_boost_and_the_boost_is_one_eighth() {
        let base = Stats {
            attack: 100,
            defense: 100,
            speed: 100,
            special: 100,
        };
        let mut s = base;
        apply_badge_boosts(&mut s, 0b0000_0001); // Boulder
        assert_eq!(
            s,
            Stats {
                attack: 112,
                ..base
            }
        );
        let mut s = base;
        apply_badge_boosts(&mut s, 0b0000_0010); // Cascade — boosts nothing
        assert_eq!(s, base);
        let mut s = base;
        apply_badge_boosts(&mut s, 0b0101_0101); // all four boosting badges
        assert_eq!(
            s,
            Stats {
                attack: 112,
                defense: 112,
                speed: 112,
                special: 112
            }
        );
    }

    /// **Badge boosts stack.** Applying them twice is what the cartridge does on
    /// every stat-change event, and it compounds.
    #[test]
    fn badge_boosts_stack_when_reapplied() {
        let mut s = Stats {
            attack: 100,
            defense: 100,
            speed: 100,
            special: 100,
        };
        apply_badge_boosts(&mut s, 1);
        apply_badge_boosts(&mut s, 1);
        assert_eq!(s.attack, 126, "112 + 112/8");
        assert!(
            s.attack > 112,
            "the boost must compound, not idempotently cap"
        );
    }

    /// A stat change discards the burn penalty, because the recomputation starts
    /// from the unmodified stat.
    #[test]
    fn a_stat_change_wipes_the_burn_penalty() {
        let mut m = mon(50, 100, 50);
        m.status = Status(Status::BRN);
        recompute_current_stats(&mut m, 0, true);
        assert_eq!(m.current.attack, 50, "burn halves attack");
        apply_stat_change(&mut m, Stat::Defense, 1, 0, true);
        assert_eq!(
            m.current.attack, 100,
            "the recomputation starts from the unmodified stat, so the burn is gone"
        );
    }

    /// The switch-in order is penalties then badges, so a burned Boulder-Badge
    /// holder is `(atk/2)*1.125`.
    #[test]
    fn the_switch_in_order_is_penalties_then_badges() {
        let mut m = mon(50, 100, 50);
        m.status = Status(Status::BRN);
        recompute_current_stats(&mut m, 0b0000_0001, true);
        assert_eq!(m.current.attack, 56, "(100/2) + (50/8)");
        // The other order would give 112/2 = 56 too here; pick a value where
        // they differ.
        let mut m = mon(50, 7, 50);
        m.status = Status(Status::BRN);
        recompute_current_stats(&mut m, 0b0000_0001, true);
        assert_eq!(
            m.current.attack, 3,
            "(7/2)=3, +0 = 3; the other order gives 7/2=3 from 7+0=7"
        );
    }

    /// Badge boosts do not apply in a link battle.
    #[test]
    fn a_link_battle_gets_no_badge_boost() {
        let mut m = mon(50, 100, 50);
        recompute_current_stats(&mut m, 0xFF, false);
        assert_eq!(m.current.attack, 100);
        recompute_current_stats(&mut m, 0xFF, true);
        assert_eq!(m.current.attack, 112);
    }

    #[test]
    fn burn_and_paralysis_are_floored_at_one() {
        let mut s = Stats {
            attack: 1,
            defense: 1,
            speed: 1,
            special: 1,
        };
        apply_burn_penalty(&mut s, Status(Status::BRN));
        apply_paralysis_penalty(&mut s, Status(Status::PAR));
        assert_eq!(s.attack, 1);
        assert_eq!(s.speed, 1);
    }

    // -- turn order ---------------------------------------------------------

    #[test]
    fn quick_attack_moves_first_regardless_of_speed() {
        let mut rng = FixedRng::constant(0);
        assert!(player_moves_first(&mut rng, QUICK_ATTACK, 33, 1, 999));
        assert!(!player_moves_first(&mut rng, 33, QUICK_ATTACK, 999, 1));
    }

    /// **Counter has negative priority** — its user goes second.
    #[test]
    fn counter_moves_last() {
        let mut rng = FixedRng::constant(0);
        assert!(!player_moves_first(&mut rng, COUNTER, 33, 999, 1));
        assert!(player_moves_first(&mut rng, 33, COUNTER, 1, 999));
    }

    /// Quick Attack outranks Counter: the Quick Attack check comes first.
    #[test]
    fn quick_attack_outranks_counter() {
        let mut rng = FixedRng::constant(0);
        assert!(player_moves_first(&mut rng, QUICK_ATTACK, COUNTER, 1, 999));
    }

    /// A speed tie draws a die; a decided race does not.
    #[test]
    fn only_a_speed_tie_draws_a_die() {
        let mut rng = FixedRng::constant(0);
        assert!(player_moves_first(&mut rng, 33, 33, 100, 50));
        assert_eq!(rng.rolls(), 0);
        assert!(player_moves_first(&mut rng, 33, 33, 100, 100));
        assert_eq!(rng.rolls(), 1);
        let mut hi = FixedRng::constant(0xFF);
        assert!(!player_moves_first(&mut hi, 33, 33, 100, 100));
    }

    // -- status -------------------------------------------------------------

    #[test]
    fn sleep_is_one_to_seven_turns_and_rejects_zero() {
        let mut rng = FixedRng::new(vec![0x00, 0x08, 0x05]);
        // 0x00 & 7 == 0 → rejected. 0x08 & 7 == 0 → rejected. 0x05 & 7 == 5.
        assert_eq!(roll_sleep_turns(&mut rng), 5);
        assert_eq!(rng.rolls(), 3);
    }

    /// Paralysis is 63/256, not a quarter — there is no `+ 1` on the compare.
    #[test]
    fn full_paralysis_is_63_in_256() {
        let mut low = FixedRng::constant(62);
        assert!(fully_paralysed(&mut low));
        let mut edge = FixedRng::constant(63);
        assert!(!fully_paralysed(&mut edge));
        assert_eq!(percent(25), 63);
    }

    #[test]
    fn residual_damage_is_a_sixteenth_with_a_floor_of_one() {
        assert_eq!(residual_damage(160), 10);
        assert_eq!(residual_damage(15), 1);
        assert_eq!(residual_damage(1), 1);
    }

    /// The toxic counter multiplies, and the first tick is 1× because it is
    /// incremented before use.
    #[test]
    fn toxic_multiplies_by_the_counter() {
        assert_eq!(toxic_damage(160, 1), 10);
        assert_eq!(toxic_damage(160, 3), 30);
    }

    /// Struggle recoils for half; everything else for a quarter.
    #[test]
    fn struggle_recoils_for_half_and_other_moves_for_a_quarter() {
        assert_eq!(recoil_damage(100, true), 50);
        assert_eq!(recoil_damage(100, false), 25);
        assert_eq!(recoil_damage(1, false), 1, "minimum recoil is 1");
    }

    /// Multi-hit is 3/8, 3/8, 1/8, 1/8 over 2..=5.
    #[test]
    fn multi_hit_is_two_to_five_with_the_three_eighths_shape() {
        let mut two = FixedRng::constant(0b0000_0000);
        assert_eq!(roll_multi_hit(&mut two), 2);
        let mut three = FixedRng::constant(0b0000_0001);
        assert_eq!(roll_multi_hit(&mut three), 3);
        let mut four = FixedRng::new(vec![0b10, 0b00]);
        assert_eq!(roll_multi_hit(&mut four), 4);
        let mut five = FixedRng::new(vec![0b11, 0b10]);
        assert_eq!(roll_multi_hit(&mut five), 5);
    }

    #[test]
    fn trapping_holds_for_one_to_four_further_turns() {
        let mut one = FixedRng::constant(0b0000_0000);
        assert_eq!(roll_trapping_turns(&mut one), 1);
        let mut four = FixedRng::new(vec![0b11, 0b10]);
        assert_eq!(roll_trapping_turns(&mut four), 4);
    }

    #[test]
    fn the_side_effect_thresholds_are_the_cartridges_bytes() {
        assert_eq!(side_effect::WEAK, 26);
        assert_eq!(side_effect::STRONG, 77);
        assert_eq!(side_effect::POISON_WEAK, 52);
        assert_eq!(side_effect::POISON_STRONG, 103);
        assert_eq!(side_effect::CONFUSION, 25, "no `+ 1` on this one");
    }

    #[test]
    fn a_dv_spread_builds_the_expected_hp_dv() {
        assert_eq!(Dvs::PERFECT.hp(), 15);
    }
}

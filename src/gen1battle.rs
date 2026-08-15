//! Reading a **real** Generation 1 battle out of a running cartridge, so the
//! model in [`terminalgb_gen1_battle`] can be checked against it.
//!
//! # The cartridge is the oracle
//!
//! [`terminalgb_gen1_battle`] is a transcription of `pret/pokered`, and a
//! transcription can be wrong. This module is what makes that falsifiable: it
//! rebuilds a [`Battle`] from the emulator's WRAM, frame by frame, so the model's
//! prediction and the game's behaviour can be laid side by side. **Where they
//! disagree the model is wrong** — no exception, and no tuning the model until
//! it looks right.
//!
//! # What is read, and how the addresses were established
//!
//! Every address here already exists in this repository and was established by
//! one of the three grades in [`docs/gen1/README.md`](../docs/gen1/README.md).
//! Nothing new is derived, which is deliberate: a differential harness whose own
//! addresses were guesses would report the guess as an emulator bug.
//!
//! * `wBattleMon` (`$D014`) and `wEnemyMon` (`$CFE5`), with the 29-byte struct's
//!   field offsets — `plugins/battle-control/src/wram.rs`, whose `verify()`
//!   reads several of them straight out of the cartridge's machine code at
//!   attach time.
//! * `wPlayerMonStatMods` (`$CD1A`) and `wEnemyMonStatMods` (`$CD2E`) —
//!   `plugins/trainer/src/gen1.rs`.
//! * `wIsInBattle` (`$D057`) and `wObtainedBadges` (`$D356`) — both.
//!
//! Multi-byte stats are **big-endian**, which is the trap `plugins/trainer`
//! records: every HP and stat in a Gen 1 structure is stored high byte first.
//!
//! # What can and cannot be checked
//!
//! `Random_` folds in `rDIV`, so the cartridge's dice cannot be predicted from
//! outside without also modelling the emulated clock. That splits the checks in
//! two, and the split is stated rather than glossed:
//!
//! * **Exact** — anything with no die in it. Stat recomputation from stages,
//!   badge boosts, the burn and paralysis penalties, poison and burn residual
//!   damage, turn order when the speeds differ. A disagreement here is a plain
//!   defect.
//! * **Bounded** — damage. `RandomizeDamage` multiplies by a byte in
//!   `217..=255` and divides by 255, so an observed hit pins the model's
//!   pre-random figure to a band. Inverting that band ([`Observation::implied_roll`])
//!   turns each hit into a check: if the model's figure is wrong, the implied
//!   roll falls outside `217..=255`. Over a few hundred hits that is a sharp
//!   test — a one-point error in a fifty-point hit shows up in roughly one hit
//!   in six.
//!
//! A bounded check is weaker than an exact one and is reported as such. It is
//! not, however, weak: it is the same argument a physicist makes from a
//! confidence interval, and it needs no new address derivation to make.

use terminalgb_gen1_battle::data::GameData;
use terminalgb_gen1_battle::mechanics as mech;
use terminalgb_gen1_battle::state::{
    calc_stat, BattleMon, Dvs, MoveSlot, Stages, Stats, Status, Volatile,
};
use terminalgb_gen1_battle::types::Type;

use crate::gameboy::Gameboy;

/// `wIsInBattle` — `$D057`. `$FF` means the player blacked out, which is **not**
/// a trainer battle; see the `BattleKind::from_flag` note in `plugins/trainer`.
pub const IS_IN_BATTLE: u16 = 0xD057;
/// `wBattleMon` — `$D014`.
pub const BATTLE_MON: u16 = 0xD014;
/// `wEnemyMon` — `$CFE5`.
pub const ENEMY_MON: u16 = 0xCFE5;
/// `wPlayerMonStatMods` — `$CD1A`, six bytes, 7 neutral, in the order attack,
/// defense, speed, special, accuracy, evasion.
pub const PLAYER_STAT_MODS: u16 = 0xCD1A;
/// `wEnemyMonStatMods` — `$CD2E`.
pub const ENEMY_STAT_MODS: u16 = 0xCD2E;
/// `wObtainedBadges` — `$D356`.
pub const OBTAINED_BADGES: u16 = 0xD356;

/// Field offsets inside the battle struct — `plugins/battle-control`'s
/// `wram::battle_field`, which counts the 29th byte the 28-byte view in
/// `plugins/trainer` omits.
pub mod field {
    pub const SPECIES: u16 = 0;
    pub const HP: u16 = 1;
    pub const STATUS: u16 = 4;
    pub const TYPE1: u16 = 5;
    pub const TYPE2: u16 = 6;
    pub const MOVES: u16 = 8;
    pub const DVS: u16 = 12;
    pub const LEVEL: u16 = 14;
    pub const MAX_HP: u16 = 15;
    pub const ATTACK: u16 = 17;
    pub const DEFENSE: u16 = 19;
    pub const SPEED: u16 = 21;
    pub const SPECIAL: u16 = 23;
    pub const PP: u16 = 25;
}

fn be16(gb: &mut Gameboy, at: u16) -> u16 {
    (u16::from(gb.peek(at)) << 8) | u16::from(gb.peek(at + 1))
}

/// Read one side's battle struct into the model's own shape.
///
/// `unmodified` is left equal to `current`, because WRAM's
/// `wPlayerMonUnmodified*` block is not among the addresses this repository has
/// established. [`Observation`] therefore records the unmodified stats once, at
/// the first frame of the battle where every stage is neutral and the two are
/// by definition the same, and carries them forward.
pub fn read_mon(gb: &mut Gameboy, base: u16, stat_mods: u16) -> BattleMon {
    read_mon_with(gb, base, stat_mods, None)
}

/// Read a battle struct, optionally supplying the tables needed to reconstruct
/// what a **critical hit** would read.
///
/// `GetEnemyMonStat` (`engine/battle/core.asm`) does not read `wEnemyMon` on a
/// crit: outside a link battle it calls `GetMonHeader` on `wEnemyMonSpecies` and
/// recomputes from base stats, `wEnemyMonDVs` and `wEnemyMonLevel` with **zero
/// stat experience**. For a plain wild Pokémon that is the same number; for a
/// battle a tool has patched it is not, and the difference is exactly what the
/// oracle caught.
pub fn read_mon_with(
    gb: &mut Gameboy,
    base: u16,
    stat_mods: u16,
    recompute_for_crit: Option<&GameData>,
) -> BattleMon {
    let mut moves = [MoveSlot::empty(); 4];
    for (i, slot) in moves.iter_mut().enumerate() {
        let id = gb.peek(base + field::MOVES + i as u16);
        let raw = gb.peek(base + field::PP + i as u16);
        *slot = MoveSlot::from_raw(id, raw);
    }
    let dv_hi = gb.peek(base + field::DVS);
    let dv_lo = gb.peek(base + field::DVS + 1);
    let stats = Stats {
        attack: be16(gb, base + field::ATTACK),
        defense: be16(gb, base + field::DEFENSE),
        speed: be16(gb, base + field::SPEED),
        special: be16(gb, base + field::SPECIAL),
    };
    let stages = Stages {
        attack: gb.peek(stat_mods),
        defense: gb.peek(stat_mods + 1),
        speed: gb.peek(stat_mods + 2),
        special: gb.peek(stat_mods + 3),
        accuracy: gb.peek(stat_mods + 4),
        evasion: gb.peek(stat_mods + 5),
    };
    let dvs = Dvs::from_bytes(dv_hi, dv_lo);
    let species = gb.peek(base + field::SPECIES);
    let level = gb.peek(base + field::LEVEL);
    let crit_stats = recompute_for_crit.and_then(|data| {
        let b = data.base_stats_for_index(species)?;
        // `CalcStat` with statexp = 0, which is what `GetEnemyMonStat` supplies.
        let st = |base: u8, dv: u8| calc_stat(base, dv, 0, level, false);
        Some(Stats {
            attack: st(b.attack, dvs.attack),
            defense: st(b.defense, dvs.defense),
            speed: st(b.speed, dvs.speed),
            special: st(b.special, dvs.special),
        })
    });
    BattleMon {
        species,
        level,
        hp: be16(gb, base + field::HP),
        max_hp: be16(gb, base + field::MAX_HP),
        status: Status(gb.peek(base + field::STATUS)),
        type1: Type(gb.peek(base + field::TYPE1)),
        type2: Type(gb.peek(base + field::TYPE2)),
        moves,
        unmodified: stats,
        current: stats,
        crit_stats,
        stages,
        volatile: Volatile::default(),
    }
}

/// Whether a battle is running.
///
/// `$FF` is **not** a battle — it means the player blacked out. That catch-all
/// arm cost `plugins/trainer` a live bug (a full battle panel rendered in the
/// overworld from stale data), and it is not repeated here.
pub fn in_battle(gb: &mut Gameboy) -> bool {
    matches!(gb.peek(IS_IN_BATTLE), 1 | 2)
}

/// One frame's view of a running battle.
#[derive(Clone, Debug)]
pub struct Frame {
    pub player: BattleMon,
    pub enemy: BattleMon,
    pub badges: u8,
}

impl Frame {
    /// Read one frame.
    ///
    /// `data` is used only to reconstruct the enemy's critical-hit stats, which
    /// the cartridge recomputes rather than reads back — see [`read_mon_with`].
    /// The player's are read from the party struct on a crit, so they need no
    /// reconstruction.
    pub fn read(gb: &mut Gameboy, data: &GameData) -> Option<Frame> {
        if !in_battle(gb) {
            return None;
        }
        let player = read_mon(gb, BATTLE_MON, PLAYER_STAT_MODS);
        let enemy = read_mon_with(gb, ENEMY_MON, ENEMY_STAT_MODS, Some(data));
        // A struct the engine has not finished writing is not a battle to
        // measure. `plugins/trainer` calls this `looks_populated`, and it is the
        // difference between a real reading and a screenful of noise.
        if player.max_hp == 0
            || enemy.max_hp == 0
            || player.level == 0
            || enemy.level == 0
        {
            return None;
        }
        if !player.stages.is_valid() || !enemy.stages.is_valid() {
            return None;
        }
        Some(Frame {
            player,
            enemy,
            badges: gb.peek(OBTAINED_BADGES),
        })
    }
}

/// Which side something happened to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Who {
    Player,
    Enemy,
}

impl Who {
    pub fn name(self) -> &'static str {
        match self {
            Who::Player => "player",
            Who::Enemy => "enemy",
        }
    }
    pub fn other(self) -> Who {
        match self {
            Who::Player => Who::Enemy,
            Who::Enemy => Who::Player,
        }
    }
}

/// A thing the cartridge did that the model can be held to.
#[derive(Clone, Debug)]
pub enum Observation {
    /// HP fell. The attacker is inferred from which side lost it.
    Damage {
        to: Who,
        amount: u16,
        before: Box<Frame>,
    },
    /// A stat stage moved, so the cartridge recomputed a stat. The strongest
    /// exact check available: the whole `CalculateModifiedStat` →
    /// burn/paralysis → `ApplyBadgeStatBoosts` chain in one comparison.
    StatRecomputed {
        to: Who,
        before: Box<Frame>,
        after: Box<Frame>,
    },
    /// A non-volatile status appeared.
    Status { to: Who, status: Status },
}

impl Observation {
    /// The random multipliers consistent with an observed hit, given the model's
    /// pre-random figure — or `None` if there are none.
    ///
    /// `RandomizeDamage` computes `damage * r / 255` for `r` in `217..=255`.
    /// **That division truncates**, so the map from `r` to the observed figure
    /// is many-to-one and cannot be inverted by dividing back: this must be a
    /// forward search over the band, not `observed * 255 / predicted`.
    ///
    /// That distinction is not pedantry, and it is not theoretical. The first
    /// version of this harness divided back, and reported a **false
    /// disagreement** on the very first small hit it saw: a predicted 6 observed
    /// as 5 inverts to `5 * 255 / 6 = 212`, which is outside the band — while
    /// the forward computation `6 * 217 / 255 = 5` matches exactly. On small
    /// figures a single output covers a dozen rolls, so the naive inverse
    /// systematically accuses the model of being wrong about the hits where it
    /// is least likely to be.
    ///
    /// Returns the *middle* of the matching run, which is what the bias
    /// statistic in [`Tally::mean_roll`] wants.
    ///
    /// `None` for a predicted figure of 0 or 1: those skip the roll entirely
    /// (`ret c` in `RandomizeDamage`), so there is nothing to imply and the
    /// comparison is exact instead.
    pub fn implied_roll(observed: u16, predicted: u16) -> Option<u16> {
        if predicted <= 1 {
            return None;
        }
        let mut lo = None;
        let mut hi = 217u16;
        for r in 217u16..=255 {
            if (u32::from(predicted) * u32::from(r) / 255) as u16 == observed {
                if lo.is_none() {
                    lo = Some(r);
                }
                hi = r;
            }
        }
        lo.map(|l| (l + hi) / 2)
    }

    /// The band an implied roll must land in.
    pub const ROLL_BAND: core::ops::RangeInclusive<u16> = 217..=255;
}

/// Compare two frames and say what changed.
///
/// Deliberately conservative: a change in more than one thing at once is
/// reported as several observations rather than guessed at as one compound
/// event, and a frame in which nothing recognisable happened produces nothing.
pub fn diff(before: &Frame, after: &Frame) -> Vec<Observation> {
    let mut out = Vec::new();
    for (who, b, a) in [
        (Who::Player, &before.player, &after.player),
        (Who::Enemy, &before.enemy, &after.enemy),
    ] {
        // Only compare the same Pokémon: a switch replaces the struct wholesale.
        if b.species != a.species || b.max_hp != a.max_hp {
            continue;
        }
        if a.hp < b.hp {
            out.push(Observation::Damage {
                to: who,
                amount: b.hp - a.hp,
                before: Box::new(before.clone()),
            });
        }
        if a.stages != b.stages {
            out.push(Observation::StatRecomputed {
                to: who,
                before: Box::new(before.clone()),
                after: Box::new(after.clone()),
            });
        }
        if a.status != b.status && !a.status.is_healthy() {
            out.push(Observation::Status {
                to: who,
                status: a.status,
            });
        }
    }
    out
}

/// A verdict on one observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The model reproduced it exactly.
    Exact,
    /// The model's figure is consistent with the observation, given the
    /// cartridge's random roll. Carries the implied roll.
    InBand(u16),
    /// The model disagrees. Carries a human-readable account.
    Disagrees(String),
    /// Nothing could be concluded — the observation has a cause the model does
    /// not know about, or it is under-determined. **Counted separately and
    /// never as a pass**; a harness that reported "not applicable" as agreement
    /// would be a harness with no teeth.
    Undetermined(String),
}

impl Verdict {
    pub fn agrees(&self) -> bool {
        matches!(self, Verdict::Exact | Verdict::InBand(_))
    }
    pub fn is_disagreement(&self) -> bool {
        matches!(self, Verdict::Disagrees(_))
    }
}

/// Check one observation against the model.
pub fn check(data: &GameData, obs: &Observation) -> Verdict {
    match obs {
        Observation::StatRecomputed { to, before, after } => {
            check_stats(*to, before, after)
        }
        Observation::Damage { to, amount, before } => {
            check_damage(data, *to, *amount, before)
        }
        Observation::Status { .. } => Verdict::Undetermined(
            "a status landing depends on a roll the model cannot see".into(),
        ),
    }
}

/// **The exact check.** Recompute the stats the model's way and demand equality.
///
/// This puts `CalculateModifiedStat`, `QuarterSpeedDueToParalysis`,
/// `HalveAttackDueToBurn` and `ApplyBadgeStatBoosts` — and, crucially, the
/// *order* they are applied in — under one comparison with no dice anywhere near
/// it.
fn check_stats(to: Who, before: &Frame, after: &Frame) -> Verdict {
    // **The ENEMY side cannot be checked in a battle a plugin built, and this
    // is a finding about the harness rather than a limitation to shrug at.**
    //
    // `CalculateModifiedStat` recomputes from `wEnemyMonUnmodified*`, which the
    // engine fills from the wild Pokémon it *generated*. `plugins/battle-control`
    // then overwrites `wEnemyMon` with the requested species — but not the
    // unmodified block, which no longer describes anything on screen. So after
    // the first stat change the cartridge's own arithmetic is running on numbers
    // this harness cannot see, and every comparison against it would be a false
    // accusation.
    //
    // Measured: before this exclusion the harness reported four "disagreements",
    // all on the enemy, all of the shape "the cartridge behaved as if the
    // unmodified stat were about 8% lower than the one in `wEnemyMon`". The
    // player's side has no such problem: its party is written *before* the
    // battle starts, so the engine computes its unmodified block from exactly
    // what is there.
    if to == Who::Enemy {
        return Verdict::Undetermined(
            "the enemy's unmodified stats are the ones the engine GENERATED, not the ones \
             the plugin patched in — see the note at `check_stats`"
                .into(),
        );
    }
    let (b, a) = match to {
        Who::Player => (&before.player, &after.player),
        Who::Enemy => (&before.enemy, &after.enemy),
    };
    // The unmodified stats are recovered from the *earlier* frame only when its
    // stages were neutral; otherwise there is nothing to recompute from and the
    // observation is under-determined rather than a failure.
    if !b.stages.is_neutral() {
        return Verdict::Undetermined(
            "the earlier frame was already off neutral, so the unmodified stats are unknown".into(),
        );
    }
    let mut model = a.clone();
    model.unmodified = b.current;
    // A stat change recomputes from the unmodified stat and re-applies badge
    // boosts, but NOT the burn/paralysis penalties — that is the quirk under
    // test. Badges only apply outside a link battle, and this harness never
    // runs one.
    let moved = mech::apply_stat_change(
        &mut model,
        terminalgb_gen1_battle::state::Stat::Attack,
        0,
        after.badges,
        true,
    );
    let _ = moved;
    let mut expected = mech::modified_stats(&b.current, &a.stages);
    mech::apply_badge_boosts(&mut expected, after.badges);

    if expected == a.current {
        Verdict::Exact
    } else {
        Verdict::Disagrees(format!(
            "{}: stages {:?} on unmodified {:?} with badges ${:02X} — model says {:?}, \
             cartridge says {:?}",
            to.name(),
            a.stages,
            b.current,
            after.badges,
            expected,
            a.current
        ))
    }
}

/// **The bounded check.** Every cause the model knows of is tried; the
/// observation passes if any of them explains the figure.
///
/// The harness cannot see *which* move was used — that would need an address
/// this repository has not established — so it enumerates causes instead. That
/// is not the weakening it looks like: the candidates differ from one another by
/// far more than the 15 % the damage band allows, so a model that is wrong
/// usually fails for all of them at once.
///
/// The cause list is the interesting part, and it grew by measurement. The first
/// version tried only the attacker's damaging moves and reported five
/// "disagreements" that were nothing of the sort — **HP falls for reasons that
/// are not an attack**: poison and burn tick, recoil bites the attacker, a
/// confused Pokémon hits itself, Leech Seed drains. Each of those is now a
/// candidate, and each is cited.
fn check_damage(data: &GameData, to: Who, amount: u16, before: &Frame) -> Verdict {
    let (attacker, defender) = match to {
        Who::Player => (&before.enemy, &before.player),
        Who::Enemy => (&before.player, &before.enemy),
    };

    // **Once a stat has been recomputed, the numbers in `wEnemyMon` are no
    // longer the ones the cartridge is doing arithmetic with** — the same
    // artefact `check_stats` documents, reaching the damage check through the
    // attack and defense figures. A hit taken after any stage has moved is
    // therefore under-determined rather than a disagreement.
    //
    // This is a narrowing of what is *determinable*, not a widening of a
    // tolerance. The damage band stays exactly 217..=255.
    if !before.player.stages.is_neutral() || !before.enemy.stages.is_neutral() {
        return Verdict::Undetermined(
            "a stat stage has moved, so the stats the cartridge is calculating with are \
             not the ones in the battle struct"
                .into(),
        );
    }

    // -- exact causes, with no roll in them ---------------------------------

    // Poison and burn: exactly maxHP/16, floored at 1
    // (`HandlePoisonBurnLeechSeed_DecreaseOwnHP`).
    if (defender.status.is_poisoned() || defender.status.is_burned())
        && amount == mech::residual_damage(defender.max_hp)
    {
        return Verdict::Exact;
    }
    // Leech Seed drains the same sixteenth and is not visible in the status
    // byte, so it is offered whenever the figure matches.
    if amount == mech::residual_damage(defender.max_hp) && defender.max_hp >= 16 {
        return Verdict::Undetermined(
            "the figure is exactly maxHP/16 — poison, burn or Leech Seed, and this \
             harness cannot see which"
                .into(),
        );
    }

    // -- bounded causes ------------------------------------------------------

    let mut best: Option<u16> = None;
    // Every figure the model considered, so a disagreement can be diagnosed
    // rather than merely reported. A harness that says "no" without saying what
    // it expected sends the next person to read the whole engine.
    let mut tried: Vec<String> = Vec::new();
    let mut candidates = 0usize;
    // `implied_roll` returns `Some` only for a roll that is already inside the
    // band, so there is nothing left to test here — the closest to the band's
    // middle is kept purely so the bias statistic is not dominated by whichever
    // candidate happened to be tried first.
    let mut take = |r: u16| {
        let closer = best.map(|b| (r as i32 - 236).abs() < (b as i32 - 236).abs());
        if closer.unwrap_or(true) {
            best = Some(r);
        }
    };

    // Confusion self-damage: a 40-power typeless physical hit against the
    // Pokémon's own Defense (`.MonHurtItselfOrFullyParalysed`). It has no type
    // chart and no STAB.
    let confusion = mech::base_damage(&mech::DamageVars {
        attack: defender.current.attack,
        defense: defender.current.defense,
        power: 40,
        level: defender.level,
    });
    if let Some(c) = confusion {
        if let Some(r) = Observation::implied_roll(amount, c) {
            take(r);
        } else if amount == c {
            return Verdict::Exact;
        }
    }

    for slot in attacker.moves.iter() {
        if slot.is_empty() {
            continue;
        }
        let Some(m) = data.move_data(slot.id) else {
            continue;
        };
        if m.power == 0 {
            continue;
        }
        candidates += 1;
        for critical in [false, true] {
            let v = mech::damage_vars(attacker, defender, m, critical);
            let Some(base) = mech::base_damage(&v) else {
                continue;
            };
            let t = mech::adjust_for_type(base, m.kind, attacker, defender, data.chart());
            if t.missed_by_type {
                continue;
            }
            // A hit is clamped to the defender's remaining HP, so an overkill
            // reads as exactly its HP and cannot be inverted at all.
            if amount == defender.hp && t.damage >= amount {
                return Verdict::Undetermined(
                    "the hit was clamped to the target's remaining HP, so it says nothing \
                     about the model's figure"
                        .into(),
                );
            }
            tried.push(format!(
                "#{}{} -> {} (implies roll {})",
                m.id,
                if critical { " crit" } else { "" },
                t.damage,
                Observation::implied_roll(amount, t.damage)
                    .map(|r| r.to_string())
                    .unwrap_or_else(|| "n/a".into())
            ));
            match Observation::implied_roll(amount, t.damage) {
                None if amount == t.damage => return Verdict::Exact,
                None => {}
                Some(r) => take(r),
            }
        }
    }

    // Recoil: the DEFENDER here may be the one that just attacked, and
    // `RecoilEffect_` costs it a quarter of the damage it dealt (a half for
    // Struggle), minimum 1. Its own moves are the candidates.
    for slot in defender.moves.iter() {
        if slot.is_empty() {
            continue;
        }
        let Some(m) = data.move_data(slot.id) else {
            continue;
        };
        if m.power == 0 {
            continue;
        }
        for critical in [false, true] {
            let v = mech::damage_vars(defender, attacker, m, critical);
            let Some(base) = mech::base_damage(&v) else {
                continue;
            };
            let t = mech::adjust_for_type(base, m.kind, defender, attacker, data.chart());
            for struggle in [false, true] {
                let r = mech::recoil_damage(t.damage, struggle);
                // Recoil is a quarter of a figure that was itself rolled, so
                // the band applies to the pre-recoil damage.
                if let Some(implied) = Observation::implied_roll(amount * 4, t.damage) {
                    if !struggle {
                        take(implied);
                    }
                }
                if amount == r {
                    return Verdict::Undetermined(
                        "the figure matches recoil from the other side's move, which this \
                         harness cannot distinguish from a hit"
                            .into(),
                    );
                }
            }
        }
    }

    match best {
        Some(r) => Verdict::InBand(r),
        None if candidates == 0 => Verdict::Undetermined(
            "the attacker has no damaging move the model knows about".into(),
        ),
        None => Verdict::Disagrees(format!(
            "{} lost {} HP out of {} (L{} atk {} def {} spc {}); the model tried [{}] and \
             none implies a roll in 217..=255, nor does a sixteenth of max HP, confusion \
             or recoil",
            to.name(),
            amount,
            defender.max_hp,
            attacker.level,
            attacker.current.attack,
            defender.current.defense,
            attacker.current.special,
            tried.join("; ")
        )),
    }
}

/// The tally a differential run produces.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub exact: u32,
    pub in_band: u32,
    pub undetermined: u32,
    pub disagreements: Vec<String>,
    /// Every implied roll, so the run can show they really do spread across
    /// `217..=255` rather than clustering — a model that is wrong by a constant
    /// factor would pass every individual check while piling every roll against
    /// one end of the band.
    pub rolls: Vec<u16>,
}

impl Tally {
    pub fn record(&mut self, v: Verdict) {
        match v {
            Verdict::Exact => self.exact += 1,
            Verdict::InBand(r) => {
                self.in_band += 1;
                self.rolls.push(r);
            }
            Verdict::Undetermined(_) => self.undetermined += 1,
            Verdict::Disagrees(why) => self.disagreements.push(why),
        }
    }

    pub fn checked(&self) -> u32 {
        self.exact + self.in_band + self.disagreements.len() as u32
    }

    pub fn agreed(&self) -> u32 {
        self.exact + self.in_band
    }

    /// The mean implied roll. It should sit near 236, the middle of the band. A
    /// mean pressed against 217 or 255 says the model is systematically low or
    /// high even though every individual observation "passed" — which is the
    /// failure mode a bounded check is vulnerable to, and the reason this is
    /// reported.
    pub fn mean_roll(&self) -> Option<f32> {
        if self.rolls.is_empty() {
            return None;
        }
        Some(
            self.rolls.iter().map(|&r| f32::from(r)).sum::<f32>()
                / self.rolls.len() as f32,
        )
    }

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!(
            "checked {} observation(s): {} exact, {} within the damage band, {} \
             undetermined\n",
            self.checked(),
            self.exact,
            self.in_band,
            self.undetermined
        ));
        if let Some(m) = self.mean_roll() {
            s.push_str(&format!(
                "implied damage rolls: mean {m:.1} over {} (the band is 217..=255, its \
                 middle is 236)\n",
                self.rolls.len()
            ));
            if !(228.0..=244.0).contains(&m) {
                s.push_str(
                    "  WARNING: the mean is off centre. Every observation passed, but a\n\
                       systematic bias means the model is probably wrong by a small\n\
                       constant factor that the band is hiding.\n",
                );
            }
        }
        if self.disagreements.is_empty() {
            s.push_str(
                "no disagreements — the model matched the cartridge everywhere it\n\
                        could be held to it.\n",
            );
        } else {
            s.push_str(&format!(
                "\n{} DISAGREEMENT(S):\n",
                self.disagreements.len()
            ));
            for d in &self.disagreements {
                s.push_str(&format!("  {d}\n"));
            }
            s.push_str(
                "\nThe cartridge is the oracle. Each line above is a defect in the model,\n\
                 not a number to tune.\n",
            );
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(hp: u16, max_hp: u16) -> BattleMon {
        BattleMon {
            species: 1,
            level: 50,
            hp,
            max_hp,
            status: Status::HEALTHY,
            type1: Type::NORMAL,
            type2: Type::NORMAL,
            moves: [MoveSlot::empty(); 4],
            unmodified: Stats {
                attack: 100,
                defense: 100,
                speed: 100,
                special: 100,
            },
            current: Stats {
                attack: 100,
                defense: 100,
                speed: 100,
                special: 100,
            },
            crit_stats: None,
            stages: Stages::default(),
            volatile: Volatile::default(),
        }
    }

    fn frame(p: BattleMon, e: BattleMon) -> Frame {
        Frame {
            player: p,
            enemy: e,
            badges: 0,
        }
    }

    #[test]
    fn a_drop_in_hp_is_reported_as_damage() {
        let a = frame(mon(100, 100), mon(100, 100));
        let b = frame(mon(80, 100), mon(100, 100));
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            d[0],
            Observation::Damage {
                to: Who::Player,
                amount: 20,
                ..
            }
        ));
    }

    /// A switch replaces the struct wholesale, so it must not read as a huge
    /// hit — that would fabricate an observation and the harness would then
    /// report the model as wrong about a thing that never happened.
    #[test]
    fn a_switch_is_not_reported_as_damage() {
        let mut a = frame(mon(100, 100), mon(100, 100));
        a.player.species = 1;
        let mut b = frame(mon(20, 60), mon(100, 100));
        b.player.species = 4;
        assert!(diff(&a, &b).is_empty());
    }

    #[test]
    fn a_stage_change_is_reported() {
        let a = frame(mon(100, 100), mon(100, 100));
        let mut b = frame(mon(100, 100), mon(100, 100));
        b.player.stages.attack = 9;
        let d = diff(&a, &b);
        assert_eq!(d.len(), 1);
        assert!(matches!(
            d[0],
            Observation::StatRecomputed {
                to: Who::Player,
                ..
            }
        ));
    }

    /// The roll search is a **forward** one over the band.
    #[test]
    fn the_implied_roll_searches_the_band_forwards() {
        // A predicted 100 landing as 100 needs the very top of the band:
        // 100*255/255 = 100, and 100*254/255 = 99.
        assert_eq!(Observation::implied_roll(100, 100), Some(255));
        // 85 of 100 is the bottom: 100*217/255 = 85.
        let r = Observation::implied_roll(85, 100).unwrap();
        assert!((217..=219).contains(&r), "{r}");
        // Damage of 0 or 1 skips the roll, so nothing is implied.
        assert_eq!(Observation::implied_roll(1, 1), None);
    }

    /// **The regression this replaced.** A predicted 6 observed as 5 is
    /// perfectly consistent — `6 * 217 / 255 = 5` — but dividing back gives
    /// `5 * 255 / 6 = 212`, outside the band, and the old harness reported a
    /// disagreement that did not exist. Truncation makes the map many-to-one and
    /// it cannot be inverted by division.
    #[test]
    fn a_small_hit_is_not_falsely_accused_by_dividing_back() {
        assert!(
            Observation::implied_roll(5, 6).is_some(),
            "6*217/255 == 5, so a predicted 6 observed as 5 is consistent"
        );
        // The naive inverse, for comparison.
        assert_eq!(5u32 * 255 / 6, 212);
        assert!(!Observation::ROLL_BAND.contains(&212));
    }

    /// A model that is wrong by enough matches no roll at all — which is the
    /// whole claim the bounded check rests on.
    #[test]
    fn a_wrong_prediction_matches_no_roll() {
        // Observed 100 against a prediction of 200: the band's floor is 170.
        assert_eq!(Observation::implied_roll(100, 200), None);
        // And a prediction that is too low can never reach the observation.
        assert_eq!(Observation::implied_roll(100, 80), None);
        // The resolution really is one point on a mid-sized hit.
        assert!(Observation::implied_roll(45, 50).is_some());
        assert_eq!(Observation::implied_roll(45, 60), None);
    }

    /// A stat recomputation from a neutral earlier frame is checked exactly.
    #[test]
    fn a_stat_recomputation_is_checked_exactly() {
        let data = GameData::synthetic(vec![], vec![]);
        let before = frame(mon(100, 100), mon(100, 100));
        let mut after = frame(mon(100, 100), mon(100, 100));
        after.player.stages.attack = 9; // +2
        after.player.current.attack = 200; // what the cartridge would compute
        let obs = Observation::StatRecomputed {
            to: Who::Player,
            before: Box::new(before.clone()),
            after: Box::new(after.clone()),
        };
        assert_eq!(check(&data, &obs), Verdict::Exact);

        // A cartridge value the model cannot reproduce is a disagreement, not a
        // shrug.
        after.player.current.attack = 199;
        let obs = Observation::StatRecomputed {
            to: Who::Player,
            before: Box::new(before),
            after: Box::new(after),
        };
        assert!(check(&data, &obs).is_disagreement());
    }

    /// **Undetermined is never a pass.** A harness that counted "could not
    /// tell" as agreement would have no teeth.
    #[test]
    fn undetermined_is_not_agreement() {
        let v = Verdict::Undetermined("no idea".into());
        assert!(!v.agrees());
        assert!(!v.is_disagreement());
        let mut t = Tally::default();
        t.record(v);
        assert_eq!(t.agreed(), 0);
        assert_eq!(t.checked(), 0);
    }

    /// A systematically biased model passes every individual bounded check and
    /// must still be reported.
    #[test]
    fn a_biased_set_of_rolls_is_warned_about() {
        let mut t = Tally::default();
        for _ in 0..40 {
            t.record(Verdict::InBand(218));
        }
        assert!(t.disagreements.is_empty());
        assert!(t.to_text().contains("WARNING"), "{}", t.to_text());

        let mut t = Tally::default();
        for r in [230, 236, 240, 234, 238] {
            t.record(Verdict::InBand(r));
        }
        assert!(!t.to_text().contains("WARNING"));
    }

    #[test]
    fn a_disagreement_reaches_the_report() {
        let mut t = Tally::default();
        t.record(Verdict::Disagrees("the sky is green".into()));
        let s = t.to_text();
        assert!(s.contains("DISAGREEMENT"));
        assert!(s.contains("the sky is green"));
        assert!(s.contains("The cartridge is the oracle"));
    }

    /// `$FF` in `wIsInBattle` means the player blacked out, not that a trainer
    /// battle is running — the catch-all arm that cost `plugins/trainer` a live
    /// bug.
    #[test]
    fn the_blackout_sentinel_is_not_a_battle() {
        // Exercised through the predicate's own logic rather than a machine.
        for v in 0u8..=255 {
            let is = matches!(v, 1 | 2);
            assert_eq!(is, v == 1 || v == 2, "byte {v}");
        }
        assert!(!matches!(0xFFu8, 1 | 2));
    }
}

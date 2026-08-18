//! The battle policy — one implementation, used by every link that meets a
//! battle rather than re-solved per scenario.
//!
//! # What it decides, and from what
//!
//! Fight (and with which move), use an item, switch, or run. **Every input is a
//! real read of the cartridge**, and none of it is a table typed into Rust and
//! hoped to match:
//!
//! | input | where it comes from |
//! |---|---|
//! | both sides' HP, level, types, Attack/Defense/Speed/Special | `wBattleMon` `$D014` and `wEnemyMon` `$CFE5`, whose 28-byte layout is [`terminalgb_plugin_trainer::gen1::battle`] |
//! | our moves and their PP | `wBattleMonMoves` `$D01C` / `wBattleMonPP` `$D02D` |
//! | each move's power, type, accuracy | the cartridge's own `Moves` table, `0E:4000` |
//! | type effectiveness | the cartridge's own `TypeEffects`, `0F:6474`, every matching row applied in turn |
//! | stat stages | `wPlayerMonStatMods` `$CD1A` / `wEnemyMonStatMods` `$CD2E` |
//! | the bag | `wNumBagItems` `$D31D` / `wBagItems` `$D31E` |
//!
//! The two tables are pulled through [`terminalgb_plugin_trainer::rom::BattleTables`],
//! which re-checks the type chart's opening rows against the cartridge before
//! any of it is believed.
//!
//! **Three things cannot be read out of a data table because the cartridge does
//! not store them as one**, and each is transcribed from the code that holds
//! them, cited, exactly as `rom::sprite_bank` is:
//!
//! * the damage formula (`engine/battle/core.asm`, `CalculateDamage`),
//! * which move effects change a stat (`constants/move_effect_constants.asm`),
//! * how much a potion heals (`engine/items/item_effects.asm`, `ld b, 20` for
//!   `POTION`, `50` for `SUPER_POTION`, `200` for `HYPER_POTION`).
//!
//! # What a screen-only student would not have
//!
//! Said plainly, because [the no-crutches
//! rule](../../../docs/agent/pewter-chain.md) turns on it: a player looking at
//! the battle screen sees both names and levels, both HP *bars*, our exact HP,
//! and our move list with each move's type and PP. It does **not** see either
//! side's stats, the opponent's exact HP, or the opponent's move list. This
//! policy uses all of those, in the same way the scripted expert's route uses
//! `wCurMap`. It is the memory-derived expert, not the student.

use terminalgb_plugin_trainer::rom::BattleTables;

/// A neutral stat stage. `wPlayerMonStatMods` stores 1..13 with 7 the middle
/// (`constants/battle_constants.asm`).
pub const NEUTRAL_STAGE: u8 = 7;
/// The lowest a stage can go, so a further "lower it" move does nothing.
pub const MIN_STAGE: u8 = 1;
/// What one stage down does, as a fraction: `StatModifierRatios`
/// (`data/battle/stat_modifiers.asm`) lists `db 66, 100` for -1. Two numbers
/// rather than a table read, because this is the only entry the policy
/// projects with and the projection is only used to answer "would spending a
/// turn flip the race?".
pub const ONE_STAGE_DOWN: (u32, u32) = (66, 100);
/// At most this many turns of a battle may go on lowering a stat. Each further
/// one buys less (the ratios flatten) and costs a whole turn, and the estimate
/// it is spent on is noisy — measured, an ungated rule spent six turns of a
/// twelve-turn fight on GROWL and still lost.
pub const MAX_STAT_TURNS: u8 = 2;

/// Move effects that change a stat, transcribed from
/// `constants/move_effect_constants.asm`. **There is no table in the ROM to
/// read this from** — the effect byte is an index into a jump table of code —
/// so the ids are written here and cited, the same call `rom::sprite_bank`
/// makes for the picture banks.
mod effect {
    pub const ATTACK_DOWN1: u8 = 18;
    pub const DEFENSE_DOWN1: u8 = 19;
    pub const SPEED_DOWN1: u8 = 20;
    pub const SPECIAL_DOWN1: u8 = 21;
    pub const ATTACK_DOWN2: u8 = 58;
    pub const DEFENSE_DOWN2: u8 = 59;
    pub const SPEED_DOWN2: u8 = 60;
    pub const SPECIAL_DOWN2: u8 = 61;

    /// Which of the opponent's stats a 0-power move lowers, and by how many
    /// stages. `None` for everything else, including the "up" moves — raising
    /// our own stats is a real tactic and is deliberately not modelled yet,
    /// because nothing in the Pewter chain carries one.
    pub fn lowers(effect: u8) -> Option<(Stat, u8)> {
        Some(match effect {
            ATTACK_DOWN1 => (Stat::Attack, 1),
            DEFENSE_DOWN1 => (Stat::Defense, 1),
            SPEED_DOWN1 => (Stat::Speed, 1),
            SPECIAL_DOWN1 => (Stat::Special, 1),
            ATTACK_DOWN2 => (Stat::Attack, 2),
            DEFENSE_DOWN2 => (Stat::Defense, 2),
            SPEED_DOWN2 => (Stat::Speed, 2),
            SPECIAL_DOWN2 => (Stat::Special, 2),
            _ => return None,
        })
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Stat {
        Attack,
        Defense,
        Speed,
        Special,
    }
}

pub use effect::Stat;

/// How much a healing item restores. Immediates in `ItemUseMedicine`
/// (`engine/items/item_effects.asm`): `ld b, 20 ; Potion heal amount`,
/// `ld b, 50 ; Super Potion`, `ld b, 200 ; Hyper Potion`. `FULL_RESTORE` and
/// `MAX_POTION` refill outright.
pub fn heals(item: u8) -> u16 {
    match item {
        0x14 => 20,              // POTION
        0x13 => 50,              // SUPER_POTION
        0x12 => 200,             // HYPER_POTION
        0x11 | 0x10 => u16::MAX, // MAX_POTION, FULL_RESTORE
        _ => 0,
    }
}

/// One side of the battle, as the cartridge has it right now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Side {
    pub name: String,
    pub species: u8,
    pub level: u8,
    pub hp: u16,
    pub max_hp: u16,
    /// `wBattleMonStatus`; non-zero is asleep/poisoned/burned/frozen/paralysed.
    pub status: u8,
    pub types: (u8, u8),
    /// The **modified** stats the cartridge is fighting with — Gen 1 keeps the
    /// stage-adjusted value here and the unmodified one elsewhere, so nothing
    /// has to re-apply the stage table to work out damage.
    pub attack: u16,
    pub defense: u16,
    pub speed: u16,
    pub special: u16,
    /// Attack, Defense, Speed, Special, **accuracy and evasion** stages,
    /// 1..13 with 7 neutral. The last two are why SAND-ATTACK is dangerous and
    /// were the last thing the policy could not see.
    pub stages: [u8; 6],
}

impl Side {
    pub fn fainted(&self) -> bool {
        self.hp == 0
    }
    fn stage(&self, s: Stat) -> u8 {
        self.stages[match s {
            Stat::Attack => 0,
            Stat::Defense => 1,
            Stat::Speed => 2,
            Stat::Special => 3,
        }]
    }
    /// The accuracy stage, index 4 of `wPlayerMonStatMods`.
    pub fn accuracy_stage(&self) -> u8 {
        self.stages[4]
    }
    /// The evasion stage, index 5.
    pub fn evasion_stage(&self) -> u8 {
        self.stages[5]
    }
}

/// One of our four move slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Move {
    /// 0-based slot, which is also how far down the move list the cursor goes.
    pub slot: usize,
    pub id: u8,
    pub name: String,
    pub power: u8,
    /// The move's own type, which is often not its user's.
    pub kind: u8,
    pub effect: u8,
    /// Percent — the move's own accuracy byte **with the accuracy and evasion
    /// stages already applied**, exactly as `MoveHitTest` applies them: the
    /// attacker's accuracy ratio, then the inverse of the defender's evasion
    /// ratio, both out of `StatModifierRatios`.
    ///
    /// **Not the ROM value.** A Pidgey's SAND-ATTACK takes a 95% TACKLE to 62%
    /// and then 47%, and a policy reading 95 off the move table walks into a
    /// fight it is no longer winning — measured, that is exactly how three of
    /// 3,000 attempts at link 10 died.
    pub accuracy: u8,
    pub pp: u8,
    /// **DISABLE has taken this move away.** Read from `wPlayerDisabledMove`
    /// ($D06D), whose **high** nibble is the disabled slot plus one — measured
    /// on eight stuck states rather than read off a macro, and the low nibble
    /// is the turns left. See `terminalgb_plugin_trainer::gen1::PLAYER_DISABLED_MOVE`.
    ///
    /// This is a *separate* thing from having no PP and it has to be, because
    /// the game answers them differently: a move with no PP is greyed out and
    /// selecting it is refused before the turn starts, while a disabled move is
    /// selected happily and then answered with "The move is disabled!" and no
    /// turn passes at all. A policy that re-picks it does not lose the fight —
    /// it never finishes it. See [`Move::usable`].
    pub disabled: bool,
    /// Type effectiveness against the Pokémon in front of us, 100 = neutral.
    pub effect_pct: u32,
    pub stab: bool,
    /// Damage at the lowest and highest of Gen 1's 217..255 rolls.
    pub min: u16,
    pub max: u16,
}

impl Move {
    /// May the turn actually be spent on this move?
    ///
    /// **PP and DISABLE both make a move unusable, and only one of them is
    /// visible in the move list.** Selecting a disabled move costs no turn and
    /// changes nothing, so a policy that keeps choosing it spins for ever
    /// rather than losing — which is exactly what it did: 11 of 60 attempts at
    /// link 20 ran out of model calls standing on one tile, every one of them
    /// re-selecting a TACKLE a JIGGLYPUFF had disabled. The other three moves
    /// were a GROWL, a LEECH SEED and a VINE WHIP with no PP, so the fallback
    /// at the end of [`decide`] had something legal to reach for the whole
    /// time and was never asked.
    pub fn usable(&self) -> bool {
        self.pp > 0 && !self.disabled
    }
    pub fn damaging(&self) -> bool {
        self.power > 0 && self.effect_pct > 0
    }
    /// Damage per turn allowing for the accuracy roll.
    pub fn expected(&self) -> u32 {
        (self.min as u32 + self.max as u32) * self.accuracy as u32 / 200
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Item {
    pub id: u8,
    pub count: u8,
}

/// What the leg wants out of a battle it did not necessarily choose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Intent {
    /// May a wild battle be won? A `battle flee` leg forfeits on a win, so on
    /// those legs the answer is always RUN whatever the party looks like.
    pub may_fight_wild: bool,
    /// Stop picking fights once the lead is this level; below it, experience is
    /// the point of the encounter.
    pub train_to: u8,
}

/// The four reasons the policy declines a fight, named.
///
/// **A caller that acts on a reason must match a name, not a substring of a
/// sentence.** The reasons started as prose and were read back with
/// `why.contains("too hurt")`, which is fine for a report and not fine for a
/// decision — and one of them steers a decision now: the solver's walk to the
/// Pokémon Center is triggered by the policy having refused a fight, because
/// *the heal threshold has to be the fight threshold* and the fight threshold
/// is no longer a number anybody outside a battle can compute. Measured with
/// the two derived separately: a lead at 12 of 23 HP was too hurt to pick a
/// fight and not hurt enough to go and get healed, and ran from 94 of 100
/// encounters while the level it was sent to earn stood still.
pub mod why {
    /// The leg's scenario forbids winning a wild battle.
    pub const FORBIDDEN: &str = "this leg must not win a wild battle";
    /// One more hit — a critical one — and the lead faints.
    pub const FAINTING: &str =
        "one more hit and we faint, and this fight is not worth it";
    /// The lead has reached the level the leg was sent to earn.
    pub const TRAINED: &str = "the lead is trained; this encounter is only a cost";
    /// Hurt enough that picking this fight risks the lead, with no healing to hand.
    pub const TOO_HURT: &str = "too hurt to pick a fight, and nothing here to heal with";
}

/// The decision, and the reason, because a policy nobody can audit is a policy
/// nobody can debug.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Fight {
        slot: usize,
        why: &'static str,
    },
    Item {
        id: u8,
        why: &'static str,
    },
    Run {
        why: &'static str,
    },
    /// Party index, 0-based.
    Switch {
        to: usize,
        why: &'static str,
    },
}

impl Action {
    pub fn why(&self) -> &'static str {
        match self {
            Action::Fight { why, .. }
            | Action::Item { why, .. }
            | Action::Run { why, .. }
            | Action::Switch { why, .. } => why,
        }
    }
}

/// Everything the policy sees.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// `wIsInBattle == 2`. **A trainer battle cannot be fled** — `TryRunningFromBattle`
    /// prints "No! There's no running from a trainer battle!" and returns — so
    /// running is not an option the policy may reach for here, and treating it
    /// as one is the flee trap.
    pub trainer: bool,
    pub ours: Side,
    pub theirs: Side,
    pub moves: Vec<Move>,
    pub items: Vec<Item>,
    /// How many party members still have HP, including the active one.
    pub party_alive: usize,
    /// The worst single hit we expect to take, and the expected one.
    pub incoming_max: u16,
    pub incoming_expected: u16,
    /// The worst hit **including a critical**, which is the number that decides
    /// whether we can survive the turn.
    ///
    /// Gen 1's critical hit substitutes `2 * Level` for `Level` in the damage
    /// formula (`CriticalHitTest` sets the flag, `CalculateDamage` doubles the
    /// level), so it is close to twice an ordinary hit — and it happens on
    /// base-speed/2 out of 256 turns, which for the Pokémon on Route 1 is
    /// around one turn in eleven. **Deciding "am I about to faint?" from the
    /// ordinary maximum loses that fight roughly once in sixty**, measured: 10
    /// of 600 attempts at link 10 ended with the party wiped, every one of them
    /// above the ordinary threshold when the killing turn came.
    pub incoming_crit: u16,
    /// How many turns of this battle have already gone on a stat move.
    pub stat_turns: u8,
    /// `wCurrentMenuItem` — which row the cursor is on in whatever battle menu
    /// is open. The move list is 0..3 in slot order, so this is what makes
    /// "use move 2" a keypress rather than a guess, and it cannot go out of
    /// phase with a swallowed press the way counting `down`s can.
    pub menu_item: u8,
}

impl View {
    pub fn can_run(&self) -> bool {
        !self.trainer
    }

    /// Do we move first? A tie goes to whoever the speed check favours, which
    /// on this console is a coin flip, so a tie is treated as "not first".
    pub fn faster(&self) -> bool {
        self.ours.speed > self.theirs.speed
    }

    /// Turns for `hit` damage a turn to take `hp` down, at least one.
    fn turns(hp: u16, hit: u32) -> u32 {
        if hit == 0 {
            return u32::MAX;
        }
        (hp as u32).div_ceil(hit).max(1)
    }

    fn best_damaging(&self) -> Option<&Move> {
        self.moves
            .iter()
            .filter(|m| m.usable() && m.damaging())
            .max_by_key(|m| m.expected())
    }

    /// A 0-power move worth a whole turn.
    ///
    /// **Three conditions, and the third is the one that matters.** The move
    /// has to lower something that reduces what we take; the stat has to have
    /// room left to fall; and spending the turn has to *flip the race* — after
    /// the drop, they must need more turns to knock us out than we need after
    /// paying one turn for the privilege. Without that last test the policy
    /// keeps lowering a stat it is already losing behind, which was measured:
    /// six GROWLs in a twelve-turn fight, and it still lost.
    fn useful_stat_move(&self, ours_turns: u32) -> Option<&Move> {
        if self.stat_turns >= MAX_STAT_TURNS {
            return None;
        }
        self.moves.iter().find(|m| {
            if !m.usable() || m.power > 0 {
                return false;
            }
            // Only the stats that change what we take. Lowering their Defense
            // raises our damage instead, which the damage estimate already
            // rewards through a plain attack, so it is not worth a turn here.
            let Some((s @ (Stat::Attack | Stat::Special), _)) = effect::lowers(m.effect)
            else {
                return false;
            };
            if self.theirs.stage(s) <= MIN_STAGE {
                return false;
            }
            let after =
                self.incoming_expected as u32 * ONE_STAGE_DOWN.0 / ONE_STAGE_DOWN.1;
            View::turns(self.ours.hp, after) > ours_turns + 1
        })
    }
}

/// Choose an action.
///
/// The order is deliberate and each step is a fact about the state rather than
/// a preference:
///
/// 1. **A guaranteed knock-out ends the fight now** — no roll of the damage
///    range leaves them standing, so nothing else can be better.
/// 2. **About to faint**: heal if the bag can, swing if the swing might still
///    win, run if running is allowed, switch if there is a bench.
/// 3. **Losing the race**: if their expected damage kills us before ours kills
///    them, and a stat move flips that, spend the turn on it. This is the whole
///    of the rival battle — see `docs/agent/pewter-chain.md` §8.11.
/// 4. Otherwise the move with the highest expected damage.
pub fn decide(v: &View, intent: &Intent) -> Action {
    let wild = !v.trainer;
    // A `battle flee` leg forfeits the moment a battle is won, so on those legs
    // the answer is RUN whatever the state looks like. Running is still a
    // decision here — it is the leg's decision, made once, rather than a
    // fallback taken because nothing else was tried.
    if wild && !intent.may_fight_wild {
        return Action::Run {
            why: why::FORBIDDEN,
        };
    }
    let best = v.best_damaging();

    // 1. A knock-out that cannot fail.
    if let Some(m) = best {
        if m.min >= v.theirs.hp && m.accuracy >= 100 {
            return Action::Fight {
                slot: m.slot,
                why: "this knocks it out on the worst roll",
            };
        }
    }

    // **A critical hit is the worst case, not the top of the roll range.** It
    // is not rare — one turn in eleven for a Route 1 Pokémon — and it is the
    // difference between a policy that survives and one that dies once in
    // sixty. See [`View::incoming_crit`].
    let doomed = v.incoming_crit.max(v.incoming_max) >= v.ours.hp;
    let ours_turns = best
        .map(|m| View::turns(v.theirs.hp, m.expected()))
        .unwrap_or(u32::MAX);
    let theirs_turns = View::turns(v.ours.hp, v.incoming_expected as u32);

    // 2. One hit from fainting.
    if doomed {
        // Heal, if something in the bag restores more than the hit coming.
        if let Some(it) = v
            .items
            .iter()
            .filter(|i| i.count > 0 && heals(i.id) > 0)
            .filter(|i| {
                let short = v.ours.max_hp.saturating_sub(v.ours.hp);
                heals(i.id).min(short) > v.incoming_max
            })
            .min_by_key(|i| heals(i.id))
        {
            return Action::Item {
                id: it.id,
                why: "one more hit and we faint, and the bag can outrun it",
            };
        }
        // A swing that might still win outright — **and that is likely to
        // land**. Requiring only that a good roll *could* end it is what kept
        // the policy swinging a 47%-accuracy TACKLE at a Pidgey on 1 HP while
        // it was itself on 2, which is a coin flip taken in place of an escape
        // that also removes the incoming hit. More likely than not is the
        // threshold, and it is a statement rather than a tuned constant.
        if let Some(m) = best {
            if m.max >= v.theirs.hp && v.faster() && m.accuracy >= 50 {
                return Action::Fight {
                    slot: m.slot,
                    why: "faster, and a good roll ends it first",
                };
            }
        }
        if wild && v.can_run() {
            return Action::Run { why: why::FAINTING };
        }
        if v.party_alive > 1 {
            return Action::Switch {
                to: 1,
                why: "one more hit and we faint, and the bench is not empty",
            };
        }
    }

    // A wild fight we are not being paid for: the lead is already trained, so
    // the encounter is a cost. Run rather than spend the turns.
    if wild && v.ours.level >= intent.train_to && v.can_run() {
        return Action::Run { why: why::TRAINED };
    }
    // **Too hurt to pick a fight.** Experience is only worth having if the
    // Pokémon collecting it survives, and on a leg with no Pokémon Center on
    // the way there is nothing to repair the damage with. Half is the same
    // threshold the heal rule uses where a healer exists — `docs/agent/
    // pewter-chain.md` §8.4 is about what happens when the two differ.
    //
    // Measured: without this the lead grinds itself down fight by fight until
    // one starts too low to survive and the escape fails — 6 of 600 attempts at
    // link 10, after the critical-hit rule had already removed the obvious
    // half.
    // **Too hurt is relative to what is hitting us, not a flat fraction.**
    // Half of maximum is the floor — it protects the *next* encounter as much
    // as this one — but a lead that cannot survive two worst-case hits is in
    // trouble whatever fraction that is, and the worst case includes a
    // critical. Measured: at 11 of 20 against a Rattata hitting for 7 on a
    // crit, the flat rule says fight, the exchange leaves 6, the escape then
    // fails twice and the lead is dead. Two of 3,000, and the only two left.
    if wild
        && v.can_run()
        && (v.ours.hp * 2 <= v.ours.max_hp || v.ours.hp <= v.incoming_max * 2)
    {
        // **Unless the thing in front of us is one hit from death.** Running
        // does not dodge the incoming hit — it dodges it only if it *works*,
        // and Gen 1's run check against a faster opponent often does not:
        // measured, 255 of 3,000 link 11 attempts had an escape fail. Both
        // choices therefore cost the same hit; only one of them removes the
        // opponent. So a move that ends the battle and is likely to land beats
        // an escape that is not.
        //
        // Measured, and it is the only failure in 3,000: a level 5 Bulbasaur on
        // 10 of 20 ran from a level 3 Rattata on **2 of 14** that its own
        // TACKLE kills outright. The escape failed, the Rattata hit for 5; at
        // 5 HP the faint rule ran again, that escape failed too, and the lead
        // was dead — beaten by something it could have removed twice over.
        //
        // **The WORST roll has to kill, not the best — and the first version of
        // this rule used the best, which made link 11 worse rather than
        // better: 2,999 of 3,000 became 2,995, with three faints where there
        // had been none.** Measured: at 10 of 20 against a Rattata on 6 of 16
        // with a 5-6 damage TACKLE, `max >= hp` said "one hit from finishing
        // it", the Rattata (faster) hit first for 5, our roll came up 5, and it
        // survived on 1 — leaving us at 5 HP in the fight we had tried to end.
        // A maybe-kill bought nothing and cost the hit.
        //
        // With the worst roll killing, swinging *ends* the battle, so it
        // dominates: both choices cost at most one hit and only one of them is
        // certain to be the last. This is rule 1 with its `accuracy == 100`
        // relaxed to "more likely than not", which is the whole difference —
        // a 95% TACKLE is not a certainty and was falling through to the run.
        //
        // **And only when the escape is the unreliable half, which is when
        // they are at least as fast as us.** `TryRunningFromBattle` scales the
        // escape odds by our speed against theirs — steeply, and it always
        // succeeds once ours divided by four overflows a byte — so against a
        // slower opponent running is very nearly free and the argument above
        // does not apply. Measured, and it is the difference between a rule
        // that helps and one that does not: ungated, this fixed the lead that
        // ran from a *faster* Rattata on 2 HP and broke two runs that had been
        // walking away from *slower* Pidgeys perfectly safely, one of which
        // then missed three 95% TACKLEs in a row and died. Net one failure for
        // two. Gated on speed, both cases come out right.
        //
        // Reaching here means `doomed` was false, so the worst incoming hit
        // leaves us standing; the test is written out anyway rather than left
        // as a consequence of a branch fifty lines up.
        if let Some(m) = best {
            if !v.faster()
                && m.min >= v.theirs.hp
                && m.accuracy >= 50
                && v.ours.hp > v.incoming_crit
            {
                return Action::Fight {
                    slot: m.slot,
                    why: "the worst roll finishes it, and running does not dodge the hit",
                };
            }
        }
        return Action::Run { why: why::TOO_HURT };
    }

    // 3. Losing the race, with a turn to spare and a move that changes it.
    if theirs_turns <= ours_turns && theirs_turns >= 2 {
        if let Some(m) = v.useful_stat_move(ours_turns) {
            return Action::Fight {
                slot: m.slot,
                why: "losing the race on damage; lower what it hits with",
            };
        }
    }

    // 4. The best damage available.
    match best {
        Some(m) => Action::Fight {
            slot: m.slot,
            why: "the most damage on offer",
        },
        // Everything is out of PP or does nothing here: use whatever is left,
        // because Struggle is worse than a status move.
        None => match v.moves.iter().find(|m| m.usable()) {
            Some(m) => Action::Fight {
                slot: m.slot,
                why: "nothing damaging is left with PP",
            },
            None => Action::Fight {
                slot: 0,
                why: "out of PP entirely",
            },
        },
    }
}

/// Gen 1's damage range for one move, at the lowest and highest random roll.
///
/// Transcribed from `CalculateDamage` (`engine/battle/core.asm`):
/// `((2 * Level / 5 + 2) * Power * Attack / Defense) / 50 + 2`, then the
/// same-type bonus (x1.5), then each type row in turn, then a random
/// multiplier of 217..255 over 255. Critical hits are not folded in — they are
/// an upside, and a policy that counted on one would be gambling.
///
/// **Gen 1 splits physical from special by the move's TYPE**, not per move:
/// `SPECIAL EQU $14`, so a move at or above that type uses Special on both
/// sides. That is [`terminalgb_plugin_trainer::rom::BattleTables::is_special`].
pub fn damage(
    tables: &BattleTables,
    m: &Move,
    attacker: &Side,
    defender: &Side,
) -> (u16, u16) {
    if m.power == 0 || m.effect_pct == 0 {
        return (0, 0);
    }
    let special = tables.is_special(m.id);
    let (a, d) = if special {
        (attacker.special, defender.special)
    } else {
        (attacker.attack, defender.defense)
    };
    let d = d.max(1) as u32;
    let base =
        (2 * attacker.level as u32 / 5 + 2) * m.power as u32 * a as u32 / d / 50 + 2;
    let base = if m.stab { base * 3 / 2 } else { base };
    let base = base * m.effect_pct / 100;
    // A move that connects always does at least 1.
    let lo = (base * 217 / 255).max(1);
    let hi = (base * 255 / 255).max(1);
    (
        lo.min(u16::MAX as u32) as u16,
        hi.min(u16::MAX as u32) as u16,
    )
}

// ---------------------------------------------------------------------------
// the wire form
// ---------------------------------------------------------------------------
//
// **Written and parsed in one file on purpose.** The observer produces this and
// a driver consumes it across a process boundary in the general case, so the
// two halves drifting is a silent, total failure — the policy would decide from
// zeros. `a_view_survives_the_wire` is the guard.

impl Side {
    fn write(&self, w: &mut crate::json::Writer) {
        w.str_field("name", &self.name);
        w.num_field("species", self.species as f64);
        w.num_field("level", self.level as f64);
        w.num_field("hp", self.hp as f64);
        w.num_field("max_hp", self.max_hp as f64);
        w.num_field("status", self.status as f64);
        w.arr_field("types");
        w.num_item(self.types.0 as f64);
        w.num_item(self.types.1 as f64);
        w.end_arr();
        w.num_field("attack", self.attack as f64);
        w.num_field("defense", self.defense as f64);
        w.num_field("speed", self.speed as f64);
        w.num_field("special", self.special as f64);
        w.arr_field("stages");
        for s in self.stages {
            w.num_item(s as f64);
        }
        w.end_arr();
    }

    fn parse(v: &crate::json::Json) -> Side {
        let n = |k: &str| v.u64_field(k).unwrap_or(0);
        let arr = |k: &str, i: usize, dflt: u64| {
            v.get(k)
                .and_then(|a| a.as_array())
                .and_then(|a| a.get(i))
                .and_then(|x| x.as_u64())
                .unwrap_or(dflt)
        };
        let mut stages = [NEUTRAL_STAGE; 6];
        for (i, s) in stages.iter_mut().enumerate() {
            *s = arr("stages", i, NEUTRAL_STAGE as u64) as u8;
        }
        Side {
            name: v.str_field("name").unwrap_or_default().to_string(),
            species: n("species") as u8,
            level: n("level") as u8,
            hp: n("hp") as u16,
            max_hp: n("max_hp") as u16,
            status: n("status") as u8,
            types: (arr("types", 0, 0) as u8, arr("types", 1, 0) as u8),
            attack: n("attack") as u16,
            defense: n("defense") as u16,
            speed: n("speed") as u16,
            special: n("special") as u16,
            stages,
        }
    }
}

impl View {
    /// Write the battle facts into an open `battle` object.
    pub fn write(&self, w: &mut crate::json::Writer) {
        w.bool_field("can_run", self.can_run());
        w.num_field("party_alive", self.party_alive as f64);
        w.num_field("incoming_max", self.incoming_max as f64);
        w.num_field("incoming_expected", self.incoming_expected as f64);
        w.num_field("incoming_crit", self.incoming_crit as f64);
        w.num_field("menu_item", self.menu_item as f64);
        w.obj_field("ours");
        self.ours.write(w);
        w.end_obj();
        w.obj_field("theirs");
        self.theirs.write(w);
        w.end_obj();
        w.arr_field("moves");
        for m in &self.moves {
            w.begin_obj();
            w.num_field("slot", m.slot as f64);
            w.num_field("id", m.id as f64);
            w.num_field("power", m.power as f64);
            w.num_field("type", m.kind as f64);
            w.num_field("effect", m.effect as f64);
            w.num_field("accuracy", m.accuracy as f64);
            w.num_field("pp", m.pp as f64);
            w.bool_field("disabled", m.disabled);
            w.num_field("effect_pct", m.effect_pct as f64);
            w.bool_field("stab", m.stab);
            w.num_field("min", m.min as f64);
            w.num_field("max", m.max as f64);
            w.end_obj();
        }
        w.end_arr();
        w.arr_field("items");
        for it in &self.items {
            w.begin_obj();
            w.num_field("id", it.id as f64);
            w.num_field("count", it.count as f64);
            w.end_obj();
        }
        w.end_arr();
    }

    /// Read them back out of a snapshot's `battle` object.
    ///
    /// `None` when the object carries no `ours`, which is how the observer says
    /// the battle structs were not populated yet — acting on a half-written
    /// battle is acting on noise.
    pub fn parse(v: &crate::json::Json) -> Option<View> {
        let ours = Side::parse(v.get("ours")?);
        if ours.level == 0 {
            return None;
        }
        let theirs = Side::parse(v.get("theirs")?);
        let moves = v
            .get("moves")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|m| {
                        let n = |k: &str| m.u64_field(k).unwrap_or(0);
                        Move {
                            slot: n("slot") as usize,
                            id: n("id") as u8,
                            name: String::new(),
                            power: n("power") as u8,
                            kind: n("type") as u8,
                            effect: n("effect") as u8,
                            accuracy: n("accuracy") as u8,
                            pp: n("pp") as u8,
                            disabled: m
                                .get("disabled")
                                .and_then(|b| b.as_bool())
                                .unwrap_or(false),
                            effect_pct: n("effect_pct") as u32,
                            stab: m
                                .get("stab")
                                .and_then(|b| b.as_bool())
                                .unwrap_or(false),
                            min: n("min") as u16,
                            max: n("max") as u16,
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();
        let items = v
            .get("items")
            .and_then(|a| a.as_array())
            .map(|a| {
                a.iter()
                    .map(|i| Item {
                        id: i.u64_field("id").unwrap_or(0) as u8,
                        count: i.u64_field("count").unwrap_or(0) as u8,
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(View {
            trainer: v.str_field("kind") == Some("trainer"),
            ours,
            theirs,
            moves,
            items,
            party_alive: v.u64_field("party_alive").unwrap_or(1) as usize,
            incoming_max: v.u64_field("incoming_max").unwrap_or(0) as u16,
            incoming_expected: v.u64_field("incoming_expected").unwrap_or(0) as u16,
            incoming_crit: v.u64_field("incoming_crit").unwrap_or(0) as u16,
            // Not on the wire: the driver counts this, because the cartridge
            // does not. `View::parse` therefore never fills it and the caller
            // sets it from its own tally before deciding.
            stat_turns: 0,
            menu_item: v.u64_field("menu_item").unwrap_or(0) as u8,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> BattleTables {
        // POUND 40 NORMAL, GROWL 0 NORMAL ATTACK_DOWN1, EMBER 40 FIRE.
        const MOVES: [(u8, [u8; 6]); 3] = [
            (1, [1, 0, 40, 0x00, 255, 35]),
            (45, [45, effect::ATTACK_DOWN1, 0, 0x00, 255, 40]),
            (52, [52, 4, 40, 0x14, 255, 25]),
        ];
        BattleTables::read(|bank, addr| {
            let off = (addr as usize) & 0x3FFF;
            if bank == 0x0E {
                let (i, f) = (off / 6 + 1, off % 6);
                return MOVES
                    .iter()
                    .find(|(id, _)| *id as usize == i)
                    .map(|(_, m)| m[f])
                    .unwrap_or(0);
            }
            0xFF
        })
    }

    fn side(hp: u16, max: u16, atk: u16, def: u16, spd: u16) -> Side {
        Side {
            level: 5,
            hp,
            max_hp: max,
            attack: atk,
            defense: def,
            speed: spd,
            special: 10,
            stages: [NEUTRAL_STAGE; 6],
            ..Default::default()
        }
    }

    fn mv(slot: usize, id: u8, power: u8, effect: u8, min: u16, max: u16) -> Move {
        Move {
            slot,
            id,
            power,
            effect,
            accuracy: 100,
            pp: 10,
            effect_pct: 100,
            min,
            max,
            ..Default::default()
        }
    }

    fn view(trainer: bool) -> View {
        View {
            trainer,
            ours: side(19, 19, 10, 10, 10),
            theirs: side(19, 19, 11, 10, 12),
            moves: vec![
                mv(0, 1, 40, 0, 3, 4),
                mv(1, 45, 0, effect::ATTACK_DOWN1, 0, 0),
            ],
            party_alive: 1,
            incoming_max: 5,
            incoming_expected: 4,
            incoming_crit: 5,
            ..Default::default()
        }
    }

    /// **A disabled move is not offered, and the policy finds something else.**
    ///
    /// The shape that broke link 20: the only damaging move with PP left is
    /// the one DISABLE took away, so the policy must fall through to a move
    /// that does nothing rather than re-selecting a move the cartridge will
    /// refuse. Selecting a disabled move costs no turn, so the failure is a
    /// deadlock rather than a loss and no HP figure ever shows it.
    #[test]
    fn a_disabled_move_is_never_chosen() {
        let mut v = view(true);
        // Slot 0 is the damage, slot 1 is a GROWL. Make the damage clearly
        // worth having — otherwise rule 3 reaches for the stat move anyway and
        // the test passes for a reason that has nothing to do with DISABLE.
        v.moves[0].min = 8;
        v.moves[0].max = 10;
        // Disable the damage.
        v.moves[0].disabled = true;
        match decide(&v, &TRAIN) {
            Action::Fight { slot, .. } => assert_eq!(
                slot, 1,
                "picked a disabled move; the cartridge answers \"The move is \
                 disabled!\" and no turn passes"
            ),
            other => panic!("a trainer battle cannot be run from: {other:?}"),
        }
        // Teeth: with the flag clear the same view picks the damage, so the
        // test is measuring the flag and not the move order.
        v.moves[0].disabled = false;
        assert!(matches!(decide(&v, &TRAIN), Action::Fight { slot: 0, .. }));
    }

    /// A disabled move is a *different* thing from one with no PP, and both
    /// have to reach [`Move::usable`] or the policy is right by accident.
    #[test]
    fn disable_and_empty_pp_are_both_unusable() {
        let mut m = mv(0, 1, 40, 0, 3, 4);
        assert!(m.usable());
        m.disabled = true;
        assert!(!m.usable());
        m.disabled = false;
        m.pp = 0;
        assert!(!m.usable());
    }

    const TRAIN: Intent = Intent {
        may_fight_wild: true,
        train_to: 13,
    };
    const FLEE: Intent = Intent {
        may_fight_wild: false,
        train_to: 13,
    };

    #[test]
    fn a_certain_knock_out_is_taken_over_anything_else() {
        let mut v = view(true);
        v.theirs.hp = 3; // our worst roll is 3
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "this knocks it out on the worst roll"
            }
        );
    }

    #[test]
    fn a_trainer_battle_is_never_run_from() {
        let mut v = view(true);
        v.ours.hp = 2; // one hit from fainting
        v.party_alive = 1;
        assert!(!v.can_run(), "wIsInBattle == 2 means there is no running");
        assert!(
            !matches!(decide(&v, &TRAIN), Action::Run { .. }),
            "the flee trap: a trainer battle cannot be fled, so a policy that \
             reaches for RUN here deadlocks"
        );
    }

    #[test]
    fn a_leg_that_must_not_win_a_wild_battle_runs_at_once() {
        let v = view(false);
        assert_eq!(
            decide(&v, &FLEE),
            Action::Run {
                why: why::FORBIDDEN
            }
        );
    }

    #[test]
    fn losing_the_damage_race_spends_a_turn_lowering_their_attack() {
        // 19 HP each. We do 3-4 a turn, so five turns; they do 4, so five
        // turns, and they are faster. Growl changes that and the policy takes
        // it.
        let v = view(true);
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 1,
                why: "losing the race on damage; lower what it hits with"
            }
        );
    }

    #[test]
    fn winning_the_race_just_hits() {
        let mut v = view(true);
        v.moves[0].min = 9;
        v.moves[0].max = 10;
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "the most damage on offer"
            }
        );
    }

    #[test]
    fn a_stat_move_that_would_not_flip_the_race_is_not_worth_the_turn() {
        // Almost dead: they need two turns, we need five, and one GROWL takes
        // that to three against six. Spending the turn loses either way, so
        // the turn goes on damage.
        let mut v = view(true);
        v.ours.hp = 8;
        v.incoming_expected = 4;
        v.incoming_max = 4;
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "the most damage on offer"
            }
        );
    }

    #[test]
    fn no_more_than_two_turns_of_a_battle_go_on_lowering_a_stat() {
        let mut v = view(true);
        v.stat_turns = MAX_STAT_TURNS;
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "the most damage on offer"
            },
            "the cap has to bind, or a losing fight becomes all GROWL"
        );
    }

    #[test]
    fn a_stat_already_at_the_floor_is_not_lowered_again() {
        let mut v = view(true);
        v.theirs.stages[0] = MIN_STAGE;
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "the most damage on offer"
            }
        );
    }

    #[test]
    fn a_swing_that_probably_misses_is_not_taken_over_an_escape() {
        // 2 HP against a 1 HP opponent, faster, and one hit ends either of us
        // — but SAND-ATTACK has taken the move to 47%. That is the shape that
        // killed three of 3,000 attempts at link 10.
        let mut v = view(false);
        v.ours.hp = 2;
        v.theirs.hp = 1;
        v.ours.speed = 20;
        v.theirs.speed = 10;
        v.incoming_max = 6;
        v.incoming_crit = 6;
        v.moves[0].accuracy = 47;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::FAINTING });
        // With the move still connecting, the swing is right.
        v.moves[0].accuracy = 94;
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Fight {
                slot: 0,
                why: "faster, and a good roll ends it first"
            }
        );
    }

    #[test]
    fn a_critical_hit_is_what_about_to_faint_means() {
        // Comfortably above the ordinary maximum and inside a critical, which
        // is the case that was quietly losing one attempt in sixty.
        // Above half health, so the too-hurt rule is not what is being tested.
        let mut v = view(false);
        v.ours.hp = 12;
        v.incoming_max = 5;
        v.incoming_crit = 13;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::FAINTING });
        // ... and with no critical in range it fights on. The crit has to
        // drop below half our HP too, or the two-worst-hits rule takes over —
        // which is a different rule with its own test.
        v.incoming_crit = 5;
        assert!(matches!(decide(&v, &TRAIN), Action::Fight { .. }));
    }

    #[test]
    fn a_potion_is_used_when_it_outruns_the_hit_coming() {
        let mut v = view(true);
        v.ours.hp = 4; // incoming_max 5 would faint us
        v.items = vec![Item { id: 0x14, count: 1 }];
        assert_eq!(
            decide(&v, &TRAIN),
            Action::Item {
                id: 0x14,
                why: "one more hit and we faint, and the bag can outrun it"
            }
        );
    }

    #[test]
    fn a_fight_two_worst_hits_would_lose_is_not_picked_at_any_fraction() {
        // Comfortably over half, and still two ordinary hits from dead.
        let mut v = view(false);
        v.ours.hp = 12;
        v.ours.max_hp = 20;
        v.theirs.hp = 19; // out of reach, so the finisher rule stays out of it
        v.incoming_max = 7;
        v.incoming_expected = 7;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
        // The same fraction against something that cannot hurt us is a fight.
        v.incoming_max = 2;
        v.incoming_expected = 2;
        assert!(matches!(decide(&v, &TRAIN), Action::Fight { .. }));
    }

    /// **Two ORDINARY hits, not two critical ones — and the difference is the
    /// whole character of the leg.** A critical is about one turn in eleven, so
    /// requiring the lead to survive two of them retreats after roughly a
    /// single hit: measured on link 11, that is 5.2 walks to the Pokémon Center
    /// per attempt, 16,000 fled battles and 162 failed escapes in 3,000 runs —
    /// and it is on those walks, fleeing through tall grass, that the lead
    /// dies. Reading it as two ordinary hits leaves the critical to the faint
    /// rule, which already tests exactly that, and measures better on every
    /// secondary figure: failed escapes 162 -> 111, battles won 76.3% ->
    /// 79.0%, worst-case decisions 1,952 -> 1,615.
    ///
    /// The `2 * incoming_crit` form came from link 10, which no longer fights
    /// wild battles at all, so nothing certified depends on it.
    #[test]
    fn the_retreat_threshold_is_two_ordinary_hits_and_the_crit_is_the_faint_rules() {
        let mut v = view(false);
        v.ours.max_hp = 20;
        v.theirs.hp = 19; // nothing to finish
        v.incoming_max = 5;
        v.incoming_expected = 5;
        v.incoming_crit = 9;
        // 12 survives two ordinary hits, so it fights — where two *crits*
        // would have sent it home.
        v.ours.hp = 12;
        assert!(matches!(decide(&v, &TRAIN), Action::Fight { .. }));
        // 10 does not, so it retreats.
        v.ours.hp = 10;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
        // And a single critical still ends it, which is the faint rule's job.
        v.ours.hp = 9;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::FAINTING });
    }

    #[test]
    fn a_hurt_lead_does_not_pick_a_wild_fight() {
        let mut v = view(false);
        v.ours.hp = 9; // of 19, so under half
        v.incoming_crit = 1;
        // Well out of reach, so there is nothing to finish.
        v.theirs.hp = 19;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
    }

    /// **The only failure in 3,000 attempts of link 11** — and then the three
    /// faints the first repair caused, which is why the finisher is the *worst*
    /// roll and not the best. A hurt lead ran from something one hit from
    /// death, the escape failed, and the hit it did not dodge killed it, twice
    /// over. Running costs the same hit as swinging whenever it fails, so a
    /// finisher beats an unlikely escape — but only a real finisher does.
    #[test]
    fn a_hurt_lead_finishes_something_that_is_one_hit_from_death() {
        let mut v = view(false);
        v.ours.hp = 9; // under half, so "too hurt" would otherwise run
        v.incoming_crit = 5; // ... and we survive the worst of it
                             // 95, so rule 1 (which demands 100) declines and this rule decides.
        v.moves[0].accuracy = 95;
        // **They are faster**, which is what makes the escape the unreliable
        // half of the choice. Against something slower, running is nearly free
        // and this rule must keep its hands off — the case below.
        v.ours.speed = 9;
        v.theirs.speed = 10;
        v.theirs.hp = 3; // our move does 3-4, so the WORST roll kills
        assert!(
            matches!(decide(&v, &TRAIN), Action::Fight { .. }),
            "a 3-4 damage move against 3 HP ends the battle whatever it rolls; \
             an escape might not"
        );
        // **A maybe-kill is not a finisher.** At 4 the worst roll leaves it
        // standing, the hit has been taken and nothing has been bought — this
        // exact case cost link 11 three faints.
        v.theirs.hp = 4;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
        // Out of reach altogether, and it runs.
        v.theirs.hp = 9;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
        // In reach, but the worst incoming hit would kill us first — that is
        // the faint rule's business and it must still get there.
        v.theirs.hp = 3;
        v.incoming_crit = 9;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::FAINTING });
        // In reach, survivable, but the move will probably miss.
        v.incoming_crit = 5;
        v.moves[0].accuracy = 30;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
        // **And against something SLOWER it runs, because the escape is
        // reliable there.** Measured: overriding the run here cost two runs
        // that had been walking away from Pidgeys safely.
        v.moves[0].accuracy = 95;
        v.ours.speed = 12;
        v.theirs.speed = 8;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TOO_HURT });
    }

    #[test]
    fn a_hurt_lead_still_fights_a_trainer_it_cannot_flee() {
        let mut v = view(true);
        v.ours.hp = 9;
        v.incoming_crit = 1;
        assert!(
            matches!(decide(&v, &TRAIN), Action::Fight { .. }),
            "there is no running from a trainer battle, so the rule must not \
             leave the policy with nothing to do"
        );
    }

    #[test]
    fn a_wild_fight_is_left_once_the_lead_is_trained() {
        let mut v = view(false);
        v.ours.level = 13;
        assert_eq!(decide(&v, &TRAIN), Action::Run { why: why::TRAINED });
    }

    #[test]
    fn the_damage_formula_reproduces_a_level_5_tackle() {
        let t = tables();
        // Gen 1: ((2*5/5 + 2) * 40 * 10 / 10) / 50 + 2 = (4*40)/50 + 2 = 5.
        let a = side(19, 19, 10, 10, 10);
        let d = side(19, 19, 10, 10, 10);
        let m = mv(0, 1, 40, 0, 0, 0);
        let (lo, hi) = damage(&t, &m, &a, &d);
        assert_eq!((lo, hi), (4, 5), "217/255 of 5 is 4");
    }

    #[test]
    fn a_special_type_move_is_resolved_on_special() {
        let t = tables();
        assert!(
            t.is_special(52),
            "EMBER is FIRE, and FIRE is a special type"
        );
        assert!(!t.is_special(1), "POUND is NORMAL, which is physical");
        // Attack 100 into Defense 10 is a ratio of ten; Special 10 into
        // Special 100 is a tenth of one. A move resolved on the wrong pair
        // could not tell them apart.
        let mut a = side(19, 19, 100, 10, 10);
        a.special = 10;
        let mut d = side(19, 19, 10, 10, 10);
        d.defense = 10;
        d.special = 100;
        let m = mv(0, 52, 40, 4, 0, 0);
        let (_, hi) = damage(&t, &m, &a, &d);
        let phys = damage(&t, &mv(0, 1, 40, 0, 0, 0), &a, &d);
        assert!(
            hi < phys.1,
            "special {hi} should be well under physical {}",
            phys.1
        );
    }

    #[test]
    fn a_view_survives_the_wire() {
        let mut v = view(true);
        v.items = vec![Item { id: 0x14, count: 2 }];
        v.ours.name = "BULBASAUR".into();
        v.theirs.types = (0x14, 0x14);
        v.moves[0].stab = true;
        let mut w = crate::json::Writer::new();
        w.begin_obj();
        w.str_field("kind", "trainer");
        v.write(&mut w);
        w.end_obj();
        let text = w.finish();
        let json = crate::json::parse(&text).expect("the writer emits valid JSON");
        let back = View::parse(&json).expect("and the reader gets it all back");
        assert_eq!(back, v, "the wire form lost something:\n{text}");
    }

    #[test]
    fn a_battle_struct_that_is_not_populated_yet_is_refused() {
        let mut v = view(true);
        v.ours.level = 0;
        let mut w = crate::json::Writer::new();
        w.begin_obj();
        v.write(&mut w);
        w.end_obj();
        let json = crate::json::parse(&w.finish()).unwrap();
        assert!(View::parse(&json).is_none());
    }

    #[test]
    fn no_effect_means_no_damage_at_all() {
        let t = tables();
        let mut m = mv(0, 1, 40, 0, 0, 0);
        m.effect_pct = 0; // NORMAL into GHOST
        assert_eq!(
            damage(&t, &m, &side(19, 19, 10, 10, 10), &side(19, 19, 10, 10, 10)),
            (0, 0)
        );
        assert!(!m.damaging());
    }
}

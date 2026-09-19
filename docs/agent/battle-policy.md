# The battle policy — one capability, not a per-link workaround

Up to link 05 a battle was something the [Pewter chain](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md)
*survived*: the driver pressed FIGHT, then A on whatever move happened to sit in
the first slot. This is the thing that replaces it — fight, switch, use an item
or run, chosen from the cartridge's own state, by one implementation every link
uses.

> **Measuring it against anything else:** the battle numbers below are
> *driver-side*, so only this policy can produce them. For a comparison
> between two players — the same fight, the same start states, the same seeds,
> host-side battle outcomes and a paired gap — see
> [the test rig](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/test-rig.md).

- **The policy**: [`plugins/agent/src/battle.rs`](../../plugins/agent/src/battle.rs)
  — pure, unit-tested, no machine and no emulator.
- **The facts it decides from**: [`plugins/agent/src/observe.rs`](../../plugins/agent/src/observe.rs)
  reads them and puts them in the snapshot; the driver parses them back.
- **The cartridge tables**: [`plugins/trainer/src/rom.rs`](../../plugins/trainer/src/rom.rs)'s
  `BattleTables`.
- **The menus**: `ChainDriver::fight` in
  [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) — and *only* the
  menus. No link carries a battle decision of its own.

---

## 1. Every input is a read of the cartridge

| input | where it comes from |
|---|---|
| both sides' HP, level, types, Attack/Defense/Speed/Special, status | `wBattleMon` `$D014`, `wEnemyMon` `$CFE5` — the 28-byte battle structs |
| our moves and their PP | `wBattleMonMoves` `$D01C`, `wBattleMonPP` `$D02D` |
| each move's power, type, accuracy, effect | the cartridge's own `Moves` table, `0E:4000` |
| type effectiveness | the cartridge's own `TypeEffects`, `0F:6474` |
| stat stages, **including accuracy and evasion** | `wPlayerMonStatMods` `$CD1A`, `wEnemyMonStatMods` `$CD2E` |
| what a stat stage multiplies by | the cartridge's own `StatModifierRatios`, `0F:76CB` |
| the bag | `wNumBagItems` `$D31D`, `wBagItems` `$D31E` |
| which menu row the cursor is on | `wCurrentMenuItem` `$CC26` |

**The stats come out of the battle structs and not the party**, which matters:
Gen 1 copies the active Pokémon into `wBattleMon` and applies stat stages
*there*, so these are the numbers the cartridge is fighting with and nothing has
to re-apply a stage table to reproduce them.

**The type chart is checked before it is believed.** `BattleTables::verified`
compares the first six rows against the ones `data/types/type_matchups.asm`
opens with — the same rows `tools/gen1-tables/gen1_tables.py` finds the table by
searching for. The rows are how the *location* is confirmed; the data is
whatever the cartridge holds there. Read live off the retail Blue cartridge:
82 rows, `WATER → FIRE` 200%, `GROUND → FLYING` **0%**, `ELECTRIC` into a
`WATER/FLYING` 400%, `NORMAL → GHOST` **0%** — Gen 1's own quirks, from Gen 1.

### Three things are transcribed, and each says so

The cartridge does not store them as a table, so they cannot be read as one.
Each is written down with its citation, the same call
`rom::sprite_bank` already makes for the picture banks:

- **the damage formula** — `CalculateDamage`, `engine/battle/core.asm`;
- **which move effects lower a stat** — `constants/move_effect_constants.asm`,
  because the effect byte is an index into a jump table of *code*;
- **how much a potion heals** — `ld b, 20 ; Potion heal amount` and its two
  siblings in `ItemUseMedicine`, `engine/items/item_effects.asm`.

`StatModifierRatios` is **not** transcribed — it is read, from `0F:76CB`, an
address found by searching the cartridge for its own opening bytes
(`25,100,28,100,…`, which occur exactly once) and re-checked on every read: the
first pair must be 0.25 and the middle one exactly 1:1, because a table read
from the wrong place almost never has a neutral stage in its middle.

---

## 2. What it decides, in order

Each step is a fact about the state, not a preference.

1. **A knock-out that cannot fail ends the fight.** If the worst roll of a move
   still takes them to zero and it cannot miss, nothing else can be better.
2. **One hit from fainting** — and *one hit* means **including a critical**,
   not the top of the ordinary roll range. heal if the bag holds something that
   outruns it; swing if we are faster and a good roll ends it first; run if this
   is a wild battle; switch if the bench is not empty.
3. **A wild fight we are not being paid for** — the lead is already at
   `train_to`, so the encounter is only a cost. Run.
4. **Too hurt to pick a fight** — under half health on a leg with no Pokémon
   Center on the way, experience is not worth the risk of collecting it. Run.
5. **Losing the race**: if their expected damage takes us down before ours takes
   them, and a stat move would *flip* that, spend the turn on it.
6. Otherwise the move with the highest expected damage, accuracy folded in.

### Three rules that link 10 found, and it took 3,000 attempts to find the third

*(A fourth, from link 11, is below.)*

Link 10 is the first leg where the policy *fights* wild Pokémon at scale:

- **A critical hit is the worst case, not the top of the roll range.** Gen 1
  substitutes `2 * Level` for `Level` in the damage formula for a crit
  (`CriticalHitTest` sets the flag, `CalculateDamage` doubles the level), so it
  is close to twice an ordinary hit — and at base-speed/2 out of 256 it happens
  about one turn in eleven for a Route 1 Pokémon. Deciding "am I about to
  faint?" without it lost 10 of 600 attempts, every one of them comfortably
  above the ordinary threshold when the killing turn came. `View::incoming_crit`
  prices the same move with the attacker at twice its level, and the faint test
  uses it.
- **Too hurt to pick a fight, and "too hurt" is relative to what is hitting
  us.** The old `ChainDriver::fight` had a flat half-health rule and the first
  version of the policy dropped it, which is a regression this page should name
  rather than quietly fix: without it the lead grinds itself down fight by fight
  until one starts too low to survive — the remaining 6 of 600. But a flat
  fraction is not enough either: at 11 of 20 against a Rattata hitting for 7 on
  a critical, half says fight, the exchange leaves 6, and the escape then fails
  twice. **A lead that cannot survive two worst-case hits is in trouble whatever
  fraction that is**, so the rule is `hp*2 <= max_hp || hp <= 2*incoming_crit`,
  and half remains as the floor because it protects the *next* encounter as much
  as this one. §8.4 of [the chain](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) is about what happens when
  the fight and heal thresholds differ. **It is gated on the battle being
  wild**, because there is no running from a trainer and a rule that leaves the
  policy with nothing to do is worse than no rule.

- **A move's accuracy is not the number in the move table.** `MoveHitTest`
  multiplies it by the attacker's accuracy ratio and by the inverse of the
  defender's evasion ratio, both out of `StatModifierRatios` — so a Pidgey's
  SAND-ATTACK takes a 95% TACKLE to 62% and then 47%. The policy was reading 95
  off the ROM and, at 2 HP against a Pokémon on 1 HP, choosing to swing rather
  than escape because *"a good roll ends it first"*. It was a coin flip taken in
  place of an escape that also removes the incoming hit. Effective accuracy is
  what the snapshot carries now, and the swing has to be **more likely than not**
  to land.

  **This one only appeared at 3,000 attempts** — a 600-attempt run was clean
  twice over. That is the bar earning its keep rather than being a formality.

Together: link 10 went 98.3% → 99.0% → 99.9% → **100.0%**, and its battle report
from **no faints at all** and 62.6% of the lead's HP left, against 6 faints and
56.9% before.

### The fourth rule: an escape is not free, so a finisher beats it

**The only failure in 3,000 attempts of link 11**, and the sharpest illustration
on this page of why "run" has to be a decision:

```text
lv5 BULBASAUR 10/20  vs  lv3 RATTATA 2/14 (faster)
  -> RUN  "too hurt to pick a fight"     -> Can't escape!  -> took 5, now 5/20
  -> RUN  "one more hit and we faint"    -> Can't escape!  -> took 5, dead
```

The Rattata was on **2 of 14** and our TACKLE does 4-5. It was one press from
over. The policy ran instead, twice, and the hits it did not dodge killed it.
**Running does not dodge the incoming hit — it dodges it only if it works**, and
162 of 3,000 attempts had an escape fail. A failed escape and a swing cost the
same hit; only one of them removes the opponent.

It took three goes to state that correctly, and both wrong versions are worth
keeping because each was plausible:

| the rule | link 11, 3,000 attempts |
|---|---|
| no rule at all | 2,999, and the fatal battle above |
| `max >= their HP` — *a good roll ends it* | **2,995**, three faints |
| `min >= their HP`, any speed | 2,999, but two runs that had walked away safely now died |
| `min >= their HP` **and they are at least as fast as us** | 2,999, **zero faints** |

- **The worst roll has to kill, not the best.** At 10 of 20 against a Rattata on
  6 of 16 with a 5-6 TACKLE, `max >= hp` said "one hit from finishing it"; the
  faster Rattata hit first, our roll came up 5, and it survived on 1. A
  maybe-kill bought nothing and cost the hit. With the worst roll killing, the
  swing *ends* the battle — this is rule 1 with its `accuracy == 100` relaxed to
  "more likely than not", and that relaxation is the whole difference, because
  a 95% TACKLE is not a certainty and was falling through to the run.
- **Only when the escape is the unreliable half, which is when they are at
  least as fast as us.** `TryRunningFromBattle` scales the odds by our speed
  against theirs, steeply, and always succeeds once ours over four overflows a
  byte. Against something slower, running is very nearly free and the argument
  does not apply — ungated, the rule broke two runs that had been walking away
  from Pidgeys perfectly safely.

> **When two choices cost the same, prefer the one that ends the situation** —
> and check that it really does end it.

### The retreat threshold is two ORDINARY hits, not two critical ones

The rule above changed *which* fights are declined. This changed how many.

`hp <= 2 * incoming_crit` reads as "a lead that cannot survive two worst-case
hits is in trouble", and for a level 5 starter on Route 1 that evaluates to
14-18 of a 19 HP bar — so **the lead refused a fight after roughly one hit**.
Measured: 5.2 walks to the Pokémon Center per attempt, 16,000 fled battles and
162 failed escapes in 3,000 runs. And it is on those walks, fleeing through tall
grass, that the lead dies.

A critical is about one turn in eleven; two in a row is one in 121. Reading
"two worst-case hits" as two *ordinary* ones leaves the critical to the faint
rule, which already tests exactly that — and measures better on every secondary
figure:

| | `2 * crit` | `2 * max` |
|---|---:|---:|
| reached the goal | 2,999 / 3,000 | 2,999 / 3,000 |
| battles won | 76.3% | **79.0%** |
| escapes that failed | 162 | **111** |
| battles fled | 15,999 | **13,601** |
| decisions, p99 / worst | 1,489 / 1,952 | **1,164 / 1,615** |

The `2 * incoming_crit` form came from link 10's last two failures, and link 10
no longer fights wild battles at all — so nothing certified depended on it.
**Check whether a constant's justification still applies before treating it as
settled.**

### And then link 11 found the cost of the second one

A threat-relative "too hurt" refuses more fights than a flat half does, and on a
link whose *whole job* is to earn experience that has a consequence a flee leg
could never show. **The lead settled at 12 of 23 HP — above the flat half, so no
walk to the nurse — and the policy, weighing a level 3 Rattata's critical hit,
declined 94 of 100 encounters.** It paced the grass for 2,500 decisions and
gained nothing.

That is [§8.4's dead band](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) exactly, re-opened by the policy
getting cleverer, and the fix is the one that closes it for good rather than for
now: **the heal decision no longer computes a threshold at all.** The policy's
own refusal is the trigger — when `decide` returns
`Action::Run { why: why::TOO_HURT }`, the solver's next overworld decision walks
to the Pokémon Center. The flat half stays only as a floor, for a lead that is
hurt and has not been attacked yet.

**The general rule, and it is worth more than the fix: when one component
decides whether an action is safe, no other component may decide the same thing
from its own arithmetic.** Two thresholds that agree today are two thresholds
that will disagree the next time either is improved, and the gap between them is
silent — nothing fails, the run simply stops making progress. Link 11 went
99.3% → **100.0%** on it, and won 74.4% of its battles against 59.4%.

### Running is a decision

The flee trap (`~/pokemon-run/FLEE-TRAP.md`, outside the repository) is what happens
when it is a fallback: a policy rewarded for arriving learns to flee every
encounter, and then meets a trainer battle it cannot flee and deadlocks.

So `View::can_run` is `wIsInBattle != 2` and **the policy never reaches for RUN
in a trainer battle** —
`a_trainer_battle_is_never_run_from` is the test, and it asserts the absence
rather than the presence, because the failure is a deadlock and not a wrong
move. Every run is counted by the reason the policy gave, and escapes that
*failed* are counted separately, because a failed escape costs a turn and is the
real price of the decision.

### The stat move is capped, and the cap was earned

An earlier version used a stat move whenever it was losing the race and the
stat had room to fall. Measured: **six GROWLs in a twelve-turn fight, and it
still lost** — each one buys less and costs a whole turn. The rule now has to
*flip the race*: after the drop, they must need more turns to knock us out than
we need after paying one turn for the privilege. Plus a hard cap of two turns a
battle, because the projection is an estimate and a bad one should not spiral.

### And the estimate it reads is a KNOWN DEFECT, measured and deliberately not fixed

**`View::incoming_expected` is not an expectation.** It is the maximum over the
opponent's moves of the top damage roll — a near-worst case, which is what
`incoming_max` and `incoming_crit` already carry, so the view holds three worst
cases where one of them is meant to be a mean. Both of its readers ask an
expectation question: `theirs_turns` is how many turns until we faint at the
ordinary rate, and the rule above asks whether one stage down buys more turns than
the turn it costs. `SelectEnemyMove`'s `.chooseRandomMove` picks a wild Pokémon's
move uniformly, so an honest figure is a mean over its moves as well as over the
217..255 roll — roughly **half** what the field reports. A Caterpie with TACKLE
4-5 at 95% and STRING SHOT is priced at **4**; the truth is **2.14**.

It is left alone because it is **not** a link-11-local change. This rule is
reached by trainer battles too — link 05 spends 2,450 turns on stat moves in its
3,000 certified attempts — so correcting it re-opens the certification of links
05, 11 and 13, and link 11 is signed off. The full record, with both candidate
arithmetics, the 3,000-attempt measurements and the trap that the obvious
correction trades one factor-of-two bias for another, is
[`pewter-chain.md` §7c](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md).

**So read the numbers in §3 knowing that**: the stat-move rule fires more often
than an honest expectation would justify, and the 2,450 turns below are that
figure, not a tuned one.

---

## 3. Measured

The certification runs of `05-the-rival-battle` and `06-cross-route-1` — 3,000
attempts each from 3,000 held-out entry states, on the maintainer's Pokémon Blue.
This is the report the standing order asks for, beside the pass rate rather than
instead of it, and the two links exercise opposite halves of the policy: 05 is
one trainer battle that cannot be fled, 06 is a route full of wild ones that
must be.

```text
05  3000 of 3000 reached the goal (100.0%)
    battles 3000: won 2294 (76.5%), lost 706 (23.5%), fled 0 (0.0%); 8.9 turns each
      HP left when it ended: 32.4% of max (3000 samples); faints 706; items used 0
      ran because: 0 the leg forbids a win, 0 one hit from fainting,
                   0 the lead is already trained; 0 escapes failed
      turns spent lowering a stat: 2450
      damage predictions: 22680 checked, 81.3% inside the roll range,
                          2482 under, 1767 over (a critical hit lands over)

06  3000 of 3000 reached the goal (100.0%)
    battles 5288: won 0, lost 0, fled 5288 (100.0%); 1.0 turns each
      HP left when it ended: 99.7% of max (5288 samples); faints 0; items used 0
      ran because: 5372 the leg forbids a win, 0 one hit from fainting,
                   0 the lead is already trained; 84 escapes failed
      turns spent lowering a stat: 0
```

**Running is cheap and it is not free**, which is the point of measuring it: a
Route 1 encounter costs one turn and 0.3% of the lead's HP, and **84 of 5,372
escape attempts failed** — Gen 1's own run check, not a defect, and a cost that
would be invisible without the counter.

**Against the old always-press-the-first-move policy: 30.6% of these battles
were lost, and now 23.5% are.** That is the honest size of it — a real gain,
measured the same way on the same entry pool, and not a transformation. At level
5 the player has one damaging move and one status move, so there is very little
for a policy to be clever with; the gain is one well-timed GROWL.

**The damage model is right, and the residual is explained rather than
excused.** 81.3% of 22,680 predictions land inside Gen 1's 217..255 roll range.
The 8.0% *over* are critical hits, which the estimate deliberately excludes —
a level-5 starter's crit rate is base-speed/2 out of 256, 8.8% for Bulbasaur and
12.7% for Charmander, so that column landing where it does is itself a check.
The 11.5% *under* are mostly the rival's own GROWL:
the prediction is made from `wBattleMon`'s Attack at the moment the move is
chosen, and a GROWL landing before ours resolves lowers it.

### The counters had to be fixed four times before they could be believed

All four are worth knowing, because a battle metric is exactly the sort of
number nobody checks:

- **Fed only from the menu branch, the tally never saw a party at 0 HP** — the
  knock-out happens during the text a turn produces — and reported **300 of 300
  battles won** where the exit states said 218. It watches every decision now.
- **Counting an action on every decision counts the keypresses that walked to
  it.** 1,048 wild battles reported *3,158* runs and 3,158 failed escapes: one
  battle's `down`, `right`, `a`. Worse, that count feeds the policy's own cap on
  stat moves, so `MAX_STAT_TURNS` of two was really about one. An action is
  counted where it is *committed* — the press of A — and nowhere else.

- **A knockout censors the damage observation, and comparing it anyway scored
  every overkill as the model being wrong.** HP stops at zero, so a hit that was
  going to do 12 to a Pokémon with 5 left is only ever *seen* as 5. Link 11 wins
  three fights in four and read 73.9% inside the roll range where link 05, whose
  battles are trainer battles with the same overkills spread over far more
  turns, read 81.3%. The true statement about a knockout is "at least the HP
  they had", so it agrees with the model exactly when the model's own maximum
  reached that far — and it is now counted and reported **apart**, not folded in
  to flatter the percentage.

- **A battle inherited from the previous link is counted twice unless it is
  disowned.** Link 11's goal is `lead_level>=10`, which fires the instant the
  level ticks over — and that happens *inside* the battle that earned it. So
  every link 12 run opens on "CHARMANDER grew to level 10!" with the opponent
  already at 0 HP, and the tally scored it: **60 wins on a leg that runs from
  everything**, 47.6% "won". The referee already makes exactly this call
  (`scenario::run::Battles` primes on its first sample and disowns an inherited
  battle); the report has to make it too. Disowned, link 12 reads 66 battles,
  all fled, 93.9% of the lead's HP left.

The second one is the reason to prefer a metric that can be cross-checked: the
independent read of the exit states (a lead at level 6 won, at level 5 lost) is
what caught the first, and the arithmetic `3158 ≈ 3 × 1048` is what caught the
second. The third was caught by two links disagreeing about the same model,
which is the same shape: *when a harness and a model disagree, suspect the
harness first.*

### The reasons are named, because one of them steers a decision

`Action::Run` carries a sentence saying why, and callers read it back. That
started as `why.contains("too hurt")`, which is tolerable for a report and stops
being tolerable the moment a reason drives behaviour — which it now does, in
[link 11](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md). The four are `battle::why::{FORBIDDEN, FAINTING,
TRAINED, TOO_HURT}`; the policy names them, callers match the name, and the
compiler guarantees the two texts are one string rather than two that agree
today.

### What is not exercised

**Item use and switching are implemented and never fire in this chain**, and
saying so is more useful than a green tick: the party is one Pokémon from link
04 to link 14, and the chain never buys a potion, so `Action::Item` and
`Action::Switch` are decisions the policy can reach and the state never offers.
They have unit tests; they have no run behind them.

---

## 4. What a screen-only student would not have

Stated plainly, because the no-crutches rule
(`~/pokemon-run/NO-CRUTCHES.md`) turns on it. A player looking at the battle screen sees both names and levels,
both HP *bars*, our exact HP, and our move list with each move's type and PP.
It does **not** see either side's stats, the opponent's exact HP, or the
opponent's move list. This policy uses all three.

That is the same bargain the rest of the scripted expert makes — its route is
`wCurMap` and a waypoint table — and it is the reason nothing here is evidence
about a student. What the policy *is* good for is producing trajectories in
which the battles were played competently, which is what a learner needs to
learn from.

---

## 5. Two things the cartridge settled

**The battle move menu's cursor is 1-based and the main menu's is not.**
`MoveSelectionMenu` opens with `ld a, [wPlayerMoveListIndex] / inc a / ld [hli],
a ; wCurrentMenuItem`, so `wCurrentMenuItem` reads 1 with `▶` drawn on the first
move. Measured before it was believed, and it is why the driver reads
`menu_item - 1` for the move list and `menu_item` for everything else. Getting
it wrong is silent: the cursor is already where you wanted, so A fires on the
wrong move and the policy's choice is thrown away.

**`TACKLE`'s accuracy byte is 242, which is 94.9% — the community's 95% is
right.** This section used to claim the opposite (*"the byte is 240, not 242 …
that is 94%"*) and it was wrong: 94% of 255 is 239.7, so 240 is the byte you get
by *computing it from a percentage you already believed* rather than reading one.
The code was never affected — `rom::move_accuracy_percent` rounds to nearest and
has always reported 95 — only the prose was. Re-read off the retail cartridge at
`Moves` `0E:4000`, six bytes a move, with POUND, SCRATCH, VINE WHIP, HYPER BEAM
and HYDRO PUMP checked in the same pass so the offset and stride are not in
doubt. Full working:
[discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md#tackle-is-95-per-cent-and-we-said-94).

> **When you write down a number the cartridge holds, write down the command that
> reads it.** A confident, cited-sounding sentence with no command behind it is a
> guess wearing a citation's clothes.

---

## 6. Related pages

- [the Pewter chain](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) — the links this is used by, and the
  certification gate.
- [Gen 1 battle control](../gen1/battle-control.md) — starting a battle from
  outside, which is a different job.
- [the Gen 1 battle engine](../gen1/battle-engine.md) — a model of the rules,
  held to the cartridge by a differential harness. The policy here reads a live
  battle; that one simulates hypothetical ones.
- [AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md) — including the soft-lock next door to
  where this fight happens.
- [AtlasGB: discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) — every battle finding on this page, and
  the rest of the campaign's, written as *how it was found* with the wrong turns
  kept: the critical hit, the accuracy scaling, the 1-based cursor, the escape
  that is not free, and the two numbers of ours that turned out to be wrong.

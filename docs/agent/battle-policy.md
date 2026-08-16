# The battle policy — one capability, not a per-link workaround

Up to link 05 a battle was something the [Pewter chain](pewter-chain.md)
*survived*: the driver pressed FIGHT, then A on whatever move happened to sit in
the first slot. This is the thing that replaces it — fight, switch, use an item
or run, chosen from the cartridge's own state, by one implementation every link
uses.

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
| stat stages | `wPlayerMonStatMods` `$CD1A`, `wEnemyMonStatMods` `$CD2E` |
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

Also transcribed: `StatModifierRatios`' `db 66, 100` for one stage down, used
only to answer "would spending a turn flip the race?".

---

## 2. What it decides, in order

Each step is a fact about the state, not a preference.

1. **A knock-out that cannot fail ends the fight.** If the worst roll of a move
   still takes them to zero and it cannot miss, nothing else can be better.
2. **One hit from fainting** (their worst hit ≥ our HP): heal if the bag holds
   something that outruns it; swing if we are faster and a good roll ends it
   first; run if this is a wild battle; switch if the bench is not empty.
3. **A wild fight we are not being paid for** — the lead is already at
   `train_to`, so the encounter is only a cost. Run.
4. **Losing the race**: if their expected damage takes us down before ours takes
   them, and a stat move would *flip* that, spend the turn on it.
5. Otherwise the move with the highest expected damage, accuracy folded in.

### Running is a decision

The flee trap (`/home/aaron/pokemon-run/FLEE-TRAP.md`, outside the repository) is what happens
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

---

## 3. Measured

The certification runs of `05-the-rival-battle` and `06-cross-route-1` — 3,000
attempts each from 3,000 held-out entry states, on the captain's Pokémon Blue.
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

### The counters had to be fixed twice before they could be believed

Both are worth knowing, because a battle metric is exactly the sort of number
nobody checks:

- **Fed only from the menu branch, the tally never saw a party at 0 HP** — the
  knock-out happens during the text a turn produces — and reported **300 of 300
  battles won** where the exit states said 218. It watches every decision now.
- **Counting an action on every decision counts the keypresses that walked to
  it.** 1,048 wild battles reported *3,158* runs and 3,158 failed escapes: one
  battle's `down`, `right`, `a`. Worse, that count feeds the policy's own cap on
  stat moves, so `MAX_STAT_TURNS` of two was really about one. An action is
  counted where it is *committed* — the press of A — and nowhere else.

The second one is the reason to prefer a metric that can be cross-checked: the
independent read of the exit states (a lead at level 6 won, at level 5 lost) is
what caught the first, and the arithmetic `3158 ≈ 3 × 1048` is what caught the
second.

### What is not exercised

**Item use and switching are implemented and never fire in this chain**, and
saying so is more useful than a green tick: the party is one Pokémon from link
04 to link 14, and the chain never buys a potion, so `Action::Item` and
`Action::Switch` are decisions the policy can reach and the state never offers.
They have unit tests; they have no run behind them.

---

## 4. What a screen-only student would not have

Stated plainly, because the no-crutches rule
(`/home/aaron/pokemon-run/NO-CRUTCHES.md`) turns on it. A player looking at the battle screen sees both names and levels,
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

**`TACKLE`'s accuracy byte is 240, not 242.** That is 94%, where the community
usually quotes 95%. The cartridge is the authority and the reader converts
`byte * 100 / 255`; nothing here rounds it up to a nicer number.

---

## 6. Related pages

- [the Pewter chain](pewter-chain.md) — the links this is used by, and the
  certification gate.
- [Gen 1 battle control](../gen1/battle-control.md) — starting a battle from
  outside, which is a different job.
- [the Gen 1 battle engine](../gen1/battle-engine.md) — a model of the rules,
  held to the cartridge by a differential harness. The policy here reads a live
  battle; that one simulates hypothetical ones.
- [sharp edges](../gen1/sharp-edges.md) — including the soft-lock next door to
  where this fight happens.

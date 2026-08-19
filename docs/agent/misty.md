# Cerulean, and the Cascade Badge

Five links from arriving in Cerulean City to standing in its Pokémon Center with
the second badge. Everything here comes out of the cartridge — Misty's party and
her movesets, the gym's layout, the badge bits, where the grass is — and
everything is measured against it rather than argued from it.

Read [`pewter-chain.md`](pewter-chain.md) for the chain and its certification
gate, [`brock.md`](brock.md) for the first gym, and
[`cerulean.md`](cerulean.md) for the road that gets here. This page assumes all
three.

```text
   Cerulean ─26─▶ the nurse ─27─▶ Route 4's grass ─28─▶ the nurse ─29─▶ MISTY ─30─▶ the nurse
```

- **Scenarios**: [`26-heal-in-cerulean.json`](../../scenarios/pewter/26-heal-in-cerulean.json),
  [`27-train-for-misty.json`](../../scenarios/pewter/27-train-for-misty.json),
  [`28-heal-before-misty.json`](../../scenarios/pewter/28-heal-before-misty.json),
  [`29-fight-through-cerulean-gym.json`](../../scenarios/pewter/29-fight-through-cerulean-gym.json),
  [`30-heal-after-misty.json`](../../scenarios/pewter/30-heal-after-misty.json).
- **Solver**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) —
  `CERULEAN_TO_POKECENTER`, `ROUTE_4_TO_GRASS`, `GYM_TO_MISTY`,
  `MISTY_TRAIN_TO`, `ChainDriver::learn_move`, `worst_held`.

---

## 0. Certified

The chain's standing gate, run in order from a **frozen** link-25 exit pool,
each link entering from the previous one's exits, `--idle-spread 3001`, every
attempt a distinct trajectory, **one binary throughout**:

| link | reached the goal | 95 % CI | entry states |
|---|---:|---|---:|
| **26-heal-in-cerulean** | **2998/3000** | 99.9 [99.8-100.0] | 500 |
| **27-train-for-misty** | **3000/3000** | 100.0 [99.9-100.0] | 2998 |
| **28-heal-before-misty** | **3000/3000** | 100.0 [99.9-100.0] | 3000 |
| **29-fight-through-cerulean-gym** | **2211/3000** | **73.7 [72.1-75.2]** | 3000 |
| **30-heal-after-misty** | **3000/3000** | 100.0 [99.9-100.0] | 2211 |

**Three of the five clear the bar** — zero failures in 3,000 bounds the failure
rate at about 0.10 % by the rule of three, which is the 99.9 % claim the rest of
the chain carries — and the two that do not are named below rather than rounded
up. **Link 27 fought 153,485 wild battles across its 3,000 attempts and lost
none of them**; every one of its 3,599 runs was the policy declining a fight it
judged unsafe.

**Replayed and identical.** Sixty seeds of link 26 and sixty of link 29, played
again from the same pools with the same binary, reproduce **60/60 and 60/60
hash for hash** — trajectory and final state both.

### The gym, split by the line the entry state carries

| line | beat Misty at level 24 | 95 % CI | failures |
|---|---:|---|---:|
| **IVYSAUR** | **1107/1110 = 99.7 %** | 99.2-99.9 | 3 |
| **WARTORTLE** | **1104/1115 = 99.0 %** | 98.2-99.4 | 11 |
| **CHARMELEON** | **0/775 = 0.0 %** | 0.0-0.5 | **775** |

**Every one of the 789 failures is a loss to Misty and 775 of them are one
line.** Neither surviving line quite clears 99.9 % either — three and eleven
losses in about eleven hundred each — so link 29 is **accepted rather than
certified**, labelled that way permanently as link 11 is, and what is left there
is the fight's own variance rather than anything the route can move.

### And link 26's two

Both are **timeouts on Route 4 after about 1,070 decisions**, and both come from
entry states that arrive in Cerulean **poisoned, at 2-8 HP, with an empty bag** —
Route 3 and Mt Moon spend every antidote and potion link 18b bought, so there is
nothing to drink and nothing to cure with. The 21-step walk to the counter kills
them one tile short of the door; they wake at *Mt Moon's* Pokémon Center,
because this link runs before the anchor has moved (§5); and the crossing they
are sent back across kills them again. **The named fix is a shop in Cerulean** —
mart 2 in the cartridge's own list sells POKE_BALL, POTION, REPEL, ANTIDOTE,
BURN_HEAL, AWAKENING and PARLYZ_HEAL, and the chain walks past its door with
about ¥5,800 — which is the same repair `cerulean.md` §1 predicted for the same
cause one arc earlier. It is not taken here.

**The whole chain runs cold cartridge to the Cascade Badge in one command**:

```sh
./target/release/examples/pewter_chain chain --rom <blue.gb> --seeds 3 --to 30
...
=== link 29: 29-fight-through-cerulean-gym
seed 1: GOAL — 18162 frames, 167 decisions, ended on map 0x41 at (5, 2)
```

three seeds, thirty links, **4 m 57 s**.

---

## 1. Misty, read out of the cartridge

`TrainerDataPointers` (`0E:5D3B`, class 35 — `TrainerNames`' own 35th record is
`MISTY`), then the stats and the movesets built the way `WriteMonMoves` builds
them, from the base-stats moves plus the learnset up to the level:

| | level | HP | ATK | DEF | SPD | SPC | moves |
|---|---:|---:|---:|---:|---:|---:|---|
| STARYU | 18 | 41 | 24 | 27 | 38 | 33 | TACKLE, WATER GUN |
| STARMIE | 21 | 59 | 40 | 44 | 56 | 50 | TACKLE, WATER GUN, HARDEN |

**Her whole offence is one 40-power special WATER move.** That single fact
decides the arc, because WATER is a type two of the three starter lines have an
opinion about — and they have opposite ones.

STARYU is WATER; STARMIE is PSYCHIC/WATER, so it takes 2× from BUG as well.
Neither carries a PSYCHIC move: STARMIE learns nothing at all by level, so its
four are its base-stats four.

## 2. The starter decides it again, and the line that cannot pass is the other one

Brock's gym is ROCK/GROUND and Charmander's line has no answer to it
([`brock.md`](brock.md) §10 measured 670 of 1000). Misty is WATER, so the
prediction going in was that the ranking would invert. It does — and the line
that fails is **still Charmander's**, for the opposite reason.

Put each line through `CalculateDamage` against the stats in §1, at the lowest
of Gen 1's 217..255 rolls, with no critical hit and no item:

| line | its answer | first held at | kills STARMIE in | dies in |
|---|---|---:|---:|---:|
| **BULBASAUR → IVYSAUR** | VINE WHIP, GRASS, **2× on both** | level 13 | 4 turns at L18 | 5 |
| **SQUIRTLE → WARTORTLE** | BITE, 60 power — and it **resists WATER** | level 24 | 6 turns at L24 | 8 |
| **CHARMANDER → CHARMELEON** | nothing, ever | — | 8-12 turns at L20-30 | **2-3** |

Charmeleon is **2× weak to WATER GUN**: measured through the same formula, a
level 20 Charmeleon takes 38 of its 56 HP from one and a level 30 one takes 30
of 79, so STARMIE kills it in two or three turns whatever the level, while its
best move against STARMIE is a 40-power SCRATCH at 1× needing nine. Sweeping the
level to 30 does not close it, exactly as sweeping Brock's to 18 did not.

The Squirtle line's number is not a level, it is a **move**:
`EvosMovesPointerTable` (`0E:B05C`) gives WARTORTLE `BITE` at **level 24**, 60
power against the 40 of its best other attack. That is where the race flips, and
it is why `MISTY_TRAIN_TO` is 24.

### Measured, at the level the chain arrives with

Twenty attempts per line through [the rig](test-rig.md), paired on the seed,
from the certified link-26 exit pool — that is the gym fought at **level 20**,
i.e. with link 27 skipped:

| line | beat the gym at level 20 | 95% CI | what happened |
|---|---:|---|---|
| **IVYSAUR** | **100.0%** | 83.9-100.0 | 151 decisions, 75.4% HP left |
| **WARTORTLE** | **45.0%** | 25.8-65.8 | 284 decisions, and the eleven failures are Misty |
| **CHARMELEON** | **0.0%** | 0.0-16.1 | wins both trainer battles on the way in — 36 of them across 20 attempts — and loses to Misty every time |

That is [`brock.md`](brock.md) §2's table with the lines rearranged, which is
exactly what a WATER gym should do to it. It is also the argument for link 27:
Wartortle at 20 is a coin flip and at 24 it is not.

### And no reachable level fixes the Charmander line

Sweeping the threshold on a Charmeleon-only pool — 60 of the certified link-26
pool's own Charmeleon states, trained on Route 4's grass through
`scenarios/experiments/x-train-for-misty-30.json` and then put through links 28
and 29 unchanged:

| level | beat Misty | n | 95% CI | what it holds |
|---:|---:|---:|---|---|
| 20 | **0.0%** | 20 | 0.0-16.1 | SCRATCH, GROWL, EMBER, LEER |
| **24** — the committed threshold | **0.0%** | 12 | 0.0-24.2 | SCRATCH, GROWL, EMBER, RAGE |
| 30 | **8.3%** | 12 | 1.5-35.4 | SCRATCH, GROWL, EMBER, RAGE |

**A plateau rather than a curve**, which is the same shape
[`brock.md`](brock.md) §10 measured for the same line at the other gym — and ten
levels there bought 32 points where ten levels here buy eight. The reason is the
same both times and it is not a number: WATER GUN is 2× on FIRE, so a level 30
Charmeleon with 88 HP still loses three of them a turn while its own best answer
is a 40-power SCRATCH at 1×.

**The level 24 and level 30 rows also hold RAGE**, which the move-learn handler
took at level 24 over LEER. That is the criterion applied faithfully — clause 2
puts a status move ahead of any attack — and it is a lateral move at best: RAGE
is 20 power against SCRATCH's 40, and `battle::decide` never chooses it. Worth
saying rather than presenting the criterion as having improved something.

**The pool itself carries a finding.** Its 3,000 states are IVYSAUR 1112,
WARTORTLE 1119 and **CHARMELEON 769** — the Charmander line is under-represented
arriving in Cerulean because Brock's gym has already cost it a third of its runs
([`brock.md`](brock.md) §10). A line that cannot pass one gym is thinned before
it reaches the next.

## 3. What that costs: the move-learn prompt, which nothing in this chain had ever seen

**The chain has never levelled a Pokémon into a full move list before, and the
first run of link 27 stalled on it.** WARTORTLE reached level 24, the cartridge
offered BITE, and the driver spent every one of its remaining 600 decisions
alternating between two YES/NO boxes at (70,14):

```
move to make room     ▶YES        Abandon learning      ▶YES
for BITE?               NO        BITE?                   NO
```

`ChainDriver::fight` reads a YES/NO box as battle text and presses A into it,
and the two boxes hand each other back and forth for ever. So
`ChainDriver::learn_move` answers the prompt **before** `fight` sees the screen,
and it takes the move rather than declining it: link 27's threshold exists
*because* of BITE, and a solver that grinds to the level and then refuses the
move has walked the leg for nothing.

Three things about that handler are load-bearing, and every one is a rule this
repository had already paid for once in
[`examples/gen1_learn_drill.rs`](../../examples/gen1_learn_drill.rs):

* **The phrase to match is `move to make room`, not `Delete an older`.** Gen 1
  holds two lines of text and the question is three, so by the frame the box is
  answerable the opening words have scrolled off. Matching them makes the box
  invisible — and A on that box means YES, so the answer gets given by accident
  instead of chosen.
* **A press while the question is still typing advances the text; it does not
  answer.** The cursor is what says the box is answerable.
* **The cursor is read from `wCurrentMenuItem`, never from the glyph**, because
  `Screen::selected` truncates a list of aligned two-word move names: `TAIL
  WHIP` under `WATER GUN` reads back as `TAIL`. And **the forget list's cursor
  is 0-based where the battle move list's is 1-based.**

### Which move is given up

`worst_held` is [`learn_table.py`](../../tools/gen1-tables/learn_table.py)'s
criterion, clause for clause — the one
[`docs/gen1/learning-moves.md`](../gen1/learning-moves.md) states and enumerates:
never an unpriced move (`OHKO`, `SUPER FANG`, the fixed-damage effects), never
the last damaging move, a status move before any attack, and among attacks the
lowest `power × accuracy × STAB`.

Clause 4 is the one that needed a decision here. The criterion deliberately does
**not** rank status moves against each other, and a Wartortle at 24 holds two
things it could give up. The tie-break is the *policy's* own:
`battle::effect::lowers` says whether `battle::decide` can ever spend a turn on
a move, and one it cannot is worth less than one it can. Losing GROWL costs the
stat-lowering rule; losing LEECH SEED costs nothing, because nothing in this
solver can choose it. `effect` was made public for exactly that — there must be
one answer to "is this a status move anything can use" in the codebase, not two.

Measured on a real run: WARTORTLE arrives at level 24 holding TACKLE, TAIL WHIP,
BUBBLE, WATER GUN and leaves holding **TACKLE, BITE, BUBBLE, WATER GUN**.

### The heal link had a hole, and HP was hiding it

**A heal link's goal fires the instant the party is whole and the player is
inside the building — so a party that arrives already whole never reaches the
counter.** [`brock.md`](brock.md) §7 states that limit honestly and calls it a
limit of the cartridge, which it is; what nobody had seen is that it has a
*consequence*, because **HP is not the only thing `HealParty` restores**.

Measured on the first certified link-28 pool: **41 of 219 IVYSAUR states arrived
at the gym with VINE WHIP — its only move that answers WATER, and 10 PP — partly
or wholly spent on Route 4's grass.** Nineteen per cent of link-27 exits happen
to be at full HP, and every one of those walked past the nurse.

| VINE WHIP PP on arriving at the gym | beat Misty |
|---|---:|
| **10, full** | **887/890 = 99.7 %** [99.0-99.9] |
| short | 170/205 = 82.9 % [77.2-87.5] |
| 0 | 17/35 = 48.6 % |

So Ivysaur's first certified figure of **96.5 %** was a defect in link 28, not a
weakness of the line — and the fix is one line: **link 29 talks to the nurse on
its way out**, which link 28 cannot do because its own goal ends the run at the
door. The previously failing seed 121 went from `party_hp=0` at 203 decisions to
the goal at 136, and the line's certified rate went **96.5 % → 99.7 %**.

**The same hole is in Brock's arc**, links 16 and 17, with the same shape: a
Wartortle or an Ivysaur that arrives in Pewter whole enters that gym with
whatever PP the Route 2 grind left it. Nothing here measures how much of
`brock.md` §9's 88.6 % that is worth; re-certifying nine links is that arc's
call, not this one's. **The general lesson is the one to carry: a goal that
fires on arrival cannot force an interaction, and "whole" means more than HP.**

## 4. Where the training happens, and why not somewhere richer

Route 4's own grass, east of the mountain: **sixty walkable tiles at x=64-73,
y=10-15**, found by reading the tileset's grass tile (`$52`) out of the block
map. It is a closed room with one door — walled at row 9 and row 16 and at x=62
and x=75, entered along row 14 through x=75-77 — with no trainer in it and no
ledge inside it, so a patrol between (64,10) and (73,15) is on grass at every
step. It is also on the road the chain already walks.

Experience per step, computed from `WildDataPointers` and `BaseStats`' own
base-experience byte:

| grass | map | mean exp/encounter | rate | exp per step |
|---|---|---:|---:|---:|
| **Route 4**, x=64-73 | `$0F` | 96.6 | 20/256 | **7.55** |
| Route 5 | `$10` | 138.3 | 15/256 | 8.10 |
| **Route 24** | `$23` | 111.1 | 25/256 | **10.85** |
| Route 25 | `$24` | 108.1 | 15/256 | 6.33 |
| Route 3 | `$0E` | 54.0 | 20/256 | 4.22 |
| Mt Moon B2F | `$3D` | 98.7 | 10/256 | 3.86 |

Route 4 is 7% behind Route 5 and 30% behind Route 24 — and both of those cost
something Route 4 does not. **Route 5's grass is behind a one-way ledge
descent**: its three patches sit at rows 4-5, 8-9 and 12-13 with `LedgeTiles`
entries at rows 3, 7, 11 and 15, so each can be dropped into and never climbed
back out of, and the walk to a nurse is the length of the map. **Route 24's is
on the far side of Nugget Bridge** — five trainers in a column at x=10-11 plus a
ROCKET at the top, every one of them unavoidable on a two-tile bridge.

## 5. The heal comes first, and that is geometry rather than tidiness

`wLastBlackoutMap` (`$D719`) reads **`0x0F`** on the state link 25 hands over —
measured, not assumed. That is Route 4, i.e. the Mt Moon Pokémon Center, whose
door is at (11,5); and Route 4's three doors are all on row 5 with x=19-23 solid
on that row and x=20-23 solid on rows 6, 7 and 8, so **the mountain's eastern
mouth cannot be walked to from its western one**
([`cerulean.md`](cerulean.md) §3). A blackout while training east of the
mountain would therefore cost the whole crossing again.

One conversation moves the anchor: after link 26 the same byte reads **`0x03`**,
Cerulean City, and every blackout in this arc costs the walk across one city.
That is why link 26 is first and link 27 second, where Brock's arc trains first
and heals second.

## 6. The gym, from the cartridge

Map `$41`, 10×14 tiles, two warps out at (4,13) and (5,13). Three objects, and
the pools are water:

| at | facing | what |
|---|---|---|
| (4,2) | down | opponent 235 = 200 + class 35 = **MISTY** |
| (2,3) | right | opponent 206 = 200 + class 6 = **JR.TRAINER♀**, roster 1: GOLDEEN Lv19 |
| (8,7) | left | opponent 215 = 200 + class 15 = **SWIMMER**, roster 1: HORSEA Lv16, SHELLDER Lv16 |

```
   0 ##########
   1 ....##....
   2 .###M.###.     MISTY, facing down
   3 .#J.....#.     the junior trainer, facing right
   4 .######.#.
   5 .#......#.
   6 .#.##.###.
   7 .#.##.##S.     the swimmer, facing LEFT — into a wall
   8 .#....###.
   9 .###..###.
  10 .###..#G#.     the gym guide
  11 .###..#.#.
  12 ..........
  13 ....WW....     the way in
```

**Both trainers have to be fought, and only one of the two reasons is
geometry.** Misty's free neighbours are (4,3) and (5,2); (5,2) is reachable only
from (5,3) because row 1 is walled at x=4 and x=5; so every route to her crosses
row 3, which is the junior trainer's own line, and (1,3) is a wall so there is no
tile on that row to her left to enter from. A breadth-first search with row 3
blocked from x=3 to x=7 finds no path at all.

### A GEN 1 TRAINER SEES THROUGH A WALL, and this is where that was measured

The swimmer faces **left**, and (7,7) and (6,7) are both solid. A sight line that
terrain could block would never reach the route — the only walkable ground beside
him is column 9, to his right. He engages anyway, at **(5,7)**, three tiles away
with two walls in between: traced on the first attempt at this link, `Splash! …
I'm first up! Let's do it!`.

That is worth knowing beyond this gym, because this repository's severance
sweeps — the ones that establish which trainers on Route 3, in Viridian Forest
and on Mt Moon are unavoidable — block a cone's *tiles* and ask whether the map
still connects. Blocking is the right model for "may the player stand here"; it
is **not** a model of occlusion, and nothing in those sweeps assumed it was. The
correction is to any future reading that a wall between a trainer and the route
buys anything.

### The junior trainer parks on the tile the walk needed — the fifth time

`TrainerWalkUpToPlayer` brings a beaten trainer to the tile beside the player.
She engages from the west while the player is at (5,3), so her body lands on
**(4,3)**, which was the route's last waypoint. Measured: 24 decisions
oscillating between (5,2) and (5,3) with the goal never reached. Misty is talked
to from **(5,2)** facing left instead — her other neighbour, and on nobody's
line. Links 04, 05, 08 and 20 each paid for the same rule.

## 7. The badge bits, verified the way this repository earns a flag

Snapshot before, play the gym, snapshot after, diff **every one of the 320 bytes**
of `wEventFlags` (`$D747`) plus the badge bytes. **Exactly three move, and
nothing is cleared:**

| address | before | after | what |
|---|---:|---:|---|
| `$D356` `wObtainedBadges` | `$01` | `$03` | **bit 1 — the Cascade Badge** |
| `$D72A` `wBeatGymFlags` | `$01` | `$03` | bit 1 |
| `$D75E` | `$00` | `$CC` | bits 2, 3, 6, 7 |

`$D75E`'s four bits decode against `constants/event_constants.asm`'s own `const`
list as `EVENT_BEAT_CERULEAN_GYM_TRAINER_0` (2),
`EVENT_BEAT_CERULEAN_GYM_TRAINER_1` (3), `EVENT_GOT_TM11` (6) and
**`EVENT_BEAT_MISTY`** (7). The same counting of that list puts
`EVENT_BEAT_PEWTER_GYM_TRAINER_0`, `EVENT_GOT_TM34` and `EVENT_BEAT_BROCK` on
`$D755` bits 2, 6 and 7, `EVENT_FOLLOWED_OAK_INTO_LAB` on `$D747` bit 0 and
`EVENT_GOT_POKEDEX` on `$D74B` bit 5 — three flags other links in this chain
derived independently, and agreeing with them is what says the counting is right.

Pewter's gym also **cleared** two bits of `$D7EB` (it rearms the Route 22 rival
encounter). Cerulean's clears nothing.

The goal is `badges>=2`, which counts the bits of `wObtainedBadges`. At this
point in the chain no third badge is reachable, so the count is the badge.

## 8. The links

| link | goal | what it is judged on |
|---|---|---|
| **26-heal-in-cerulean** | `watch:last_blackout_map=0x03`, `party_missing_hp=0`, `map=0x40` | the nurse, the whole party, inside the building |
| **27-train-for-misty** | `watch:lead_level>=24` | the level, in Route 4's grass |
| **28-heal-before-misty** | `party_missing_hp=0`, `map=0x40` | the whole party, inside the building |
| **29-fight-through-cerulean-gym** | `badges>=2` | `wObtainedBadges` bit 1 |
| **30-heal-after-misty** | `party_missing_hp=0`, `map=0x40`, `require badges>=2` | the whole party, inside the building |

Link 27 is the **third** remedial link the chain has needed, and it is named so
in its own objective: link 11 exists because Viridian Forest's bug catchers beat
a level 6 lead, link 15 because Brock beats a level 10 one, and this because
Misty's STARMIE is level 21 and the chain arrives below it.

Neither heal link can prove a *repeat* heal, and both say so: `$D719` is already
`0x03` by link 28, so `party_missing_hp=0` plus being inside the building is the
strongest claim the cartridge supports. It forces anyone who is hurt through the
nurse and cannot be satisfied at a door.

## 8b. The test rig, on all five links

Every new link handed to [the rig](test-rig.md) beside the expert, from its own
certified entry pool, paired on the seed, `--idle-spread 3001`:

| plan | link | teacher | `mash-a` | `random` | gap [goal] |
|---|---|---:|---:|---:|---|
| [`heal-in-cerulean`](../../scenarios/testrig/heal-in-cerulean.json) | 26 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]** |
| [`train-for-misty`](../../scenarios/testrig/train-for-misty.json) | 27 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]** |
| [`heal-before-misty`](../../scenarios/testrig/heal-before-misty.json) | 28 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]** |
| [`fight-through-cerulean-gym`](../../scenarios/testrig/fight-through-cerulean-gym.json) | 29 | see below | 0/60 | 0/60 | see below |
| [`heal-after-misty`](../../scenarios/testrig/heal-after-misty.json) | 30 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]** |

**How a zero was reached is worth more than the zero**, and here the four
floors are not all the same shape. `mash-a` is *stuck* on link 26 — it never
leaves the tile it starts on — and runs out of calls everywhere else. `random`
is the interesting one: on link 28 it ends **`violated` on all twelve**, because
that leg's rule is `battle flee` and random fighting wins battles; and on link
26 it wins two violations and flees 88 % of the seventeen battles it starts.

### And on the gym, the rig says the quiet part out loud

The gym plan carries one subject per starter line, twenty attempts each:

| subject | teacher | `mash-a` | `random` | gap [goal] teacher − random |
|---|---:|---:|---:|---|
| **WARTORTLE** | **20/20 = 100 %** | 0/20 | 0/20 | **+100.0 [+56.2, +143.8]** |
| **IVYSAUR** | **20/20 = 100 %** | 0/20 | 0/20 | **+100.0 [+56.2, +143.8]** |
| **CHARMELEON** | **0/20 = 0 %** | 0/20 | 0/20 | **+0.0 [+0.0, +0.0]** |

**On the Charmander line the expert's gap over a program that mashes A is
nothing at all** — and it is not that the expert plays badly there: it wins all
**39** of the trainer battles it fights on the way in, at 62.6 % HP left, and
loses to Misty every time. That is the strongest form the finding takes.
Expertise is worth a hundred points on the two lines that hold an answer and
zero on the line that does not.

**There is still no trained player in this repository**, so every split is
`n/a` and these are a ceiling and a floor rather than a distillation result.

## 9. Running it

```sh
cargo build --release --features scenario,image --example pewter_chain

# the whole chain, cold cartridge to the Cascade Badge
./target/release/examples/pewter_chain chain --rom <blue.gb> --seeds 3 --to 30

# one link, from a pool, at the chain's own gate
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/pewter/29-fight-through-cerulean-gym.json \
    --start-dir <link-28 exits> --runs 3000 --workers 12 --exits 3000 \
    --out target/cert/29-fight-through-cerulean-gym

# per starter line, with intervals and the paired gap
./target/release/examples/pewter_chain ab --rom <blue.gb> \
    --plan scenarios/testrig/fight-through-cerulean-gym.json \
    --start-dir <link-28 exits>

# what a pool of entry states actually holds, and any byte beside it
./target/release/examples/pewter_chain states --rom <blue.gb> \
    --start-dir <pool>/exits --peek 'blackout@0xD719' --peek 'badges@0xD356'

# Misty's own party, the type chart's answer to it, and where to catch one
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan misty
```

## 10. See also

* [`cerulean.md`](cerulean.md) — the road here, and the blackout model this arc rests on.
* [`brock.md`](brock.md) — the first gym, and the starter that cannot pass it.
* [`pewter-chain.md`](pewter-chain.md) — the chain, the gate, and the repair loop.
* [`battle-policy.md`](battle-policy.md) — the policy that fights all three of this gym's battles.
* [`../gen1/learning-moves.md`](../gen1/learning-moves.md) — the criterion §3 implements.
* [`test-rig.md`](test-rig.md) — the rig §11 runs.

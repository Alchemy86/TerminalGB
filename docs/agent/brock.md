# Past Pewter: the gym, the badge, and the precondition nobody had checked

Five links from arriving in Pewter City to standing on Route 3 with the Boulder
Badge. Everything here comes out of the cartridge — Brock's party, the gym's
layout, the badge bit, what proves a heal — and everything is measured against
it rather than argued from it.

Read [`docs/agent/pewter-chain.md`](pewter-chain.md) first: the fourteen links
before these, the solver, and the certification rules they all run under.

## 1. The one thing that makes this different

**Brock is the first link the chain has to WIN.**

Every battle link before this had a goal the cartridge sets whoever won. Link
05's is that the rival battle *happened*, and `HandlePlayerBlackOut`
(`engine/battle/core.asm`) special-cases the starter battle so the flag is set
after a loss as much as after a win — the test rig measured the expert passing
that link 300 of 300 while losing the fight up to 48% of the time
([`test-rig-results.md`](test-rig-results.md)).

A badge bit only sets if Brock is beaten. So this is the first link where a
passing rate is a measure of battling rather than of surviving — and the first
where the chain's own preconditions were tested against something that pushes
back.

## 2. What was already there, and what was wrong with it

Link 11 raises the lead to **level 10**, and that number was derived properly:
Viridian Forest's three bug catchers top out at level 9, so 10 is
`max(forest) + 1`. It is a precondition for the *forest*.

**Nothing in the chain had ever asked what Brock needs.** Measured, 40 attempts
per starter from the certified link-14 pool, every one of them arriving at
Pewter at exactly level 10:

| starter at level 10 | beat the gym | 95% CI |
|---|---:|---|
| SQUIRTLE | **90.0%** | 76.9 – 96.0 |
| CHARMANDER | **20.0%** | 10.5 – 34.8 |
| BULBASAUR | **0.0%** | 0.0 – 8.8 |

That is not a level being slightly short. Two of the three starters cannot beat
Brock at all at the level the chain delivers, and the reason is not a number.

## 3. The reason is a MOVE, and it is a fact about the cartridge

Brock's party, read from `TrainerDataPointers` (`0E:5D3B`, class 34) — and then
**read back out of `wEnemyMon` in the running game**, which is the check that
matters, because the level and the species come from the table while the stats
come from a formula and a DV pair:

| | level | HP | ATK | DEF | SPD | SPC | moves |
|---|---:|---:|---:|---:|---:|---:|---|
| GEODUDE | 12 | 33 | 26 | 30 | 11 | 14 | TACKLE, DEFENSE CURL |
| ONIX | 14 | 36 | 20 | 52 | 26 | 15 | TACKLE, SCREECH |

Every one of those numbers is what `$CFE5..$CFFD` holds mid-fight, on states
captured from real attempts, and `wEnemyMonDVs` reads **`$98` `$88`** — the
fixed trainer DVs, out of the machine rather than out of a memory. (Attack and
Defence read differently on a state taken later in a battle, because the
policy's GROWL and Geodude's own DEFENSE CURL have moved them. **Geodude curls
to Defence 120**, four times its base, which is why a Normal move is worthless
against it and the policy's Fire one — special, against Geodude's Special of 14
— is picked despite being 0.5x.)

Both are **ROCK/GROUND**, and the type chart (`TypeEffects`, `0F:6474`) makes
WATER, GRASS, FIGHTING, GROUND and ICE each **2x against both halves**, i.e.
**4x** in total. Both of Brock's Pokémon carry exactly one damaging move
between them and it is TACKLE at power 35.

What each starter's line holds, from `EvosMovesPointerTable` (`0E:B05C`):

| line | the move that answers ROCK/GROUND | first held at |
|---|---|---:|
| SQUIRTLE | BUBBLE, WATER, 4x | **level 8** |
| BULBASAUR | VINE WHIP, GRASS, 4x | **level 13** |
| CHARMANDER | — EMBER is FIRE, which is **0.5x** on ROCK | **never, on this line** |

So Squirtle arrives with the answer, Bulbasaur is three levels short of it, and
Charmander's line never gets one. Below level 13 a Bulbasaur's best move against
Geodude is TACKLE at 0.5x. Put through `CalculateDamage` with the stats §3
verified against the running game, that is **one or two damage** against 33 HP
with 35 PP to spend — so it does not lose slowly, it cannot win at all. The
0/40 above is the measurement; this paragraph is the arithmetic that explains
it.

**The precondition was Brock-blind, and the fix is not one number.** That is the
finding this work exists to report.

## 4. Where to train — and this is a bigger result than the gym

The obvious repair is "train higher". Measured on Route 1, where link 11 grinds:
raising the target from 10 to 12 **failed 30 attempts out of 30**, every one of
them running out its 600,000-frame budget.

Route 2's northern half is a different proposition. **Map `$0D`, rows 2 to 7,
48 grass tiles**, found by reading the tileset's own grass tile (`$52`) out of
the block map — nine steps from the map's northern exit and fifteen more to
Pewter's Pokémon Center door.

Measured: **level 10 to level 14 in 63,666 frames**, 625 decisions, 35 battles,
no faints. Against a Route 1 attempt that could not reach 12 in 600,000.

### It is the NORTHERN half, and that matters

This repository has already established that **Route 2 is two disjoint
components of the walkable graph** with Viridian Forest the only way between
them. Re-checked here with the same walkability grid: a breadth-first search
from the southern grass to the northern one returns **no path**.

* **Northern grass: rows 2-7, 48 tiles. Reached only through the forest.**
* Southern grass: rows 48-51, 24 tiles. Reachable before the forest.

So the fast training ground is **after** the forest. It is exactly what the
Brock preparation needs and it **cannot help link 11**, whose whole job is
training *before* the forest. Nobody should read this finding as a fix for
link 11.

### Is anything pre-forest better than Route 1? Measured: yes, by about 8%

Link 11 is the single largest consumer of the chain's emulated time, so this is
worth an answer rather than an assumption. Every pre-forest map with grass. The
first four columns are computed from `WildDataPointers` and `BaseStats`; the
last is a breadth-first path length over the walkability grid. Both are
cartridge reads, not measurements of a run.

| grass | map | mean exp/encounter | rate | exp per step | steps to the Viridian nurse |
|---|---|---:|---:|---:|---:|
| **Route 1**, rows 6-9 | `$0C` | 25.0 | 25/256 | 2.44 | **25** |
| Route 2 **south**, rows 48-51 | `$0D` | 29.4 | 25/256 | 2.87 | 56 |
| **Route 22**, rows 8-11 | `$21` | 27.0 | 25/256 | 2.64 | **43** |
| Viridian Forest | `$33` | 39.7 | **8**/256 | **1.24** | — |

Both candidates were run on link 11's own job — level 5-6 to 10, from the
certified link-10 pool, same seeds, same worker count, and **paired on the
seed**, because each attempt is the same entry state and the same phase offset
whichever grass it walks:

| | reached the goal | median frames | mean paired difference | faster on |
|---|---:|---:|---:|---:|
| **Route 1** (link 11, as shipped) | **40/40** | 65,904 | — | — |
| **Route 22** | **40/40** | **60,474** | **−7,709** (se 1,723, t = −4.47) | **29 of 40 seeds** |
| Route 2 south | 34/40 | 72,294 | +13,999 (se 4,041, t = +3.46) | 6 of 34 seeds |

**Route 22 is about 8% faster than Route 1, and Route 2 south is about 11%
slower.** Link 11 is 58% of the chain's emulated time, so 8% off it is roughly
**4.7% off the whole run** — modest, real, and on the record. *Link 11 is not
changed here; the captain's instruction was to measure and say.*

The 8.3% median gain tracks Route 22's 8% richer encounter table almost
exactly, which is the consistency check: the walk is a small enough share that
the table dominates. Route 2 south has the **richest** table of the three and
still loses, because its grass is 24 steps from its own exit before the city
crossing even starts — 56 steps to the nurse against Route 22's 43.

Viridian Forest is the interesting one and it loses on the term nobody looks
at: its encounter *rate* is 8/256 against Route 1's 25, so despite being 59%
richer per encounter it is **half as good per step**.

> **A correction worth keeping.** An earlier draft of this page did *not* run
> Route 22. It argued that Route 22 "loses on both terms to a candidate that
> already lost", from a nurse distance of 79-82 steps — and that number was
> **wrong**: the breadth-first search behind it started from a tile that is not
> walkable, and the real distance is 43. The conclusion inverted the moment the
> number was fixed. **An argument from two measured numbers is still an
> argument, and it is only as good as the weaker number.** Route 22 is measured
> now.

### And the lever on link 11 is not the location

Where a link-11 run's frames actually go — **one attempt**, per-map from its
own position track. One run is enough to answer "is it walking or fighting?",
which is a factor-of-four question; it is not a precise split:

| | frames | share | of which inside a battle |
|---|---:|---:|---:|
| Route 1 | 45,229 | 83.2% | 79.0% |
| Viridian City | 4,765 | 8.8% | 0% |
| Viridian Pokémon Center | 4,338 | 8.0% | 0% |
| **in battle, overall** | **35,721** | **65.7%** | |

**Two thirds of link 11 is spent inside battles and one sixth on the round trip
to the nurse.** So a perfectly placed Pokémon Center could save at most about
17%, and Route 1 already has the closest one. The cost driver is **frames per
battle**: 19 battles in 35,721 frames is **1,880 frames — thirty-one seconds of
game time — per wild encounter**, and a single Route 2 training run measures
the same 1,819 (63,666 frames over 35 battles). That number is the same on every map, because it is the game's own
animations and text rather than anything the route controls.

If link 11 is to be shortened, it is that constant or the number of battles, not
the address.

## 5. The links

| link | goal | what it is judged on |
|---|---|---|
| **15-train-for-brock** | `watch:lead_level>=16` | the level, in Route 2's northern grass |
| **16-heal-in-pewter** | `watch:last_blackout_map=0x02`, `party_missing_hp=0`, `map=0x3A` | the nurse, the whole party, inside the building |
| **17-fight-through-the-gym** | `badges>=1` | `wObtainedBadges` bit 0 |
| **18-heal-after-the-gym** | `party_missing_hp=0`, `map=0x3A` | the whole party, inside the building |
| **19-east-to-route-3** | `map=0x0E` | the map connection east |

The training link is a **repair**, the second the chain has needed, and it is
named so in its own objective. Link 11 exists because Viridian Forest's bug
catchers beat a level 6 lead; this exists because Brock beats a level 10 one.

## 6. The badge bit, verified the way this repository earns a flag

Snapshot, play the encounter, snapshot again, diff **every byte** of
`wEventFlags` (`$D747`, 320 bytes) plus the badge bytes. Across the gym exactly
five bytes move:

| address | before | after | what |
|---|---:|---:|---|
| `$D356` `wObtainedBadges` | `$00` | `$01` | **bit 0 — the Boulder Badge** |
| `$D72A` `wBeatGymFlags` | `$00` | `$01` | bit 0 |
| `$D755` | `$00` | `$C4` | bits 2, 6, 7 |
| `$D7EB` | `$81` | `$00` | bits 0 and 7 **cleared** |
| `$D35E` `wCurMap` | `$02` | `$36` | in the gym |

`$D755`'s three bits decode against `constants/event_constants.asm` as
`EVENT_BEAT_PEWTER_GYM_TRAINER_0` (2), `EVENT_GOT_TM34` (6) and
**`EVENT_BEAT_BROCK`** (7) — and `$D7EB`'s two as
`EVENT_1ST_ROUTE22_RIVAL_BATTLE` and `EVENT_ROUTE22_RIVAL_WANTS_BATTLE`.

Every one of them is written by a single block of the cartridge's own script,
`PewterGym.asm`'s `.gymVictory`, in that order:

```
	ld hl, wObtainedBadges
	set BIT_BOULDERBADGE, [hl]
	ld hl, wBeatGymFlags
	set BIT_BOULDERBADGE, [hl]
	...
	ResetEvents EVENT_1ST_ROUTE22_RIVAL_BATTLE, EVENT_ROUTE22_RIVAL_WANTS_BATTLE
	SetEvent EVENT_BEAT_PEWTER_GYM_TRAINER_0
```

`BIT_BOULDERBADGE` is 0 (`constants/ram_constants.asm`). The measurement and the
disassembly agree byte for byte, including the two clearings — which are the
game rearming the Route 22 rival encounter, not progress being lost.

The goal is `badges>=1`, which counts bits in `wObtainedBadges`. At this point
in the chain no other badge is reachable, so the count is the bit.

## 7. What proves a heal — and what cannot

**The first heal at a Pokémon Center is provable and a repeat one is not.**

`wLastBlackoutMap` (`$D719`, "the last Pokémon Center you healed at") holds the
**town**, not the Center's interior map. Measured on the certified pool it reads
`$01`, Viridian City, all the way to Pewter. Then:

* walking **into** Pewter's Pokémon Center leaves it at `$01` — measured;
* talking to the nurse flips it to **`$02`** — measured.

So for the first Pewter heal that flip is a proof of the interaction that "the
party is at full HP" cannot give, because a party that was never hurt is at full
HP too.

It also fires **at the start of the dialogue, before `HealParty` runs** — a run
whose goal was the flip alone stopped at the counter with the lead still on
26/29. So the goal is the flip *and* `party_missing_hp=0` *and* being in the
building.

**`party_missing_hp` is new** (`Progress`, `src/longplay/progress.rs`): the sum
over the party of maximum less current HP. A goal condition compares one number
against a constant, and "current equals maximum" is not expressible that way —
the maximum is not a constant, it moves with the party and with every level.
Carrying the shortfall makes "the party is whole" a single comparison.

For the **second** heal, at the same Center, the cartridge records nothing new:
`wLastBlackoutMap` is already `$02`. So link 18's goal is the strongest claim
available — **whole, and standing inside the Pokémon Center** — which forces
anyone who is hurt through the nurse and cannot be satisfied by walking past the
door. It still cannot distinguish a party that healed from one that arrived
whole. That is a limit of the cartridge, and it is stated rather than papered
over.

Both heal links therefore **finish inside the building**, and links 17 and 19
begin by walking out of it. That also removed a real failure: an earlier link 18
whose goal was `party_missing_hp=0` and `map=0x02` fired at the *gym* door for
every attempt that came out whole — 23 of 60, in under ten decisions, having
never gone near the nurse.

## 8. The gym, from the cartridge

Map `$36`, 10x14 tiles, two warps out at (4,13) and (5,13). Three objects:

| at | what |
|---|---|
| (4,1) | opponent 234 = 200 + class 34 = **BROCK**, roster 1 |
| (3,6) | opponent 205 = 200 + class 5 = **JR.TRAINER**, roster 1: DIGLETT Lv11, SANDSHREW Lv11 |
| (7,10) | the gym guide, no trainer record |

```
   0 ##########
   1 #...B....#     Brock, facing down
   2 #........#
   3 ####..####     <- pinch: only x=4,5
   4 #........#
   5 #.#..###.#
   6 #..J.....#     the junior trainer, facing right ($D3)
   7 #.#..###.#
   8 #........#
   9 ####..####     <- pinch: only x=4,5
  10 ...#..#G..
  11 ..........
  12 ..........
  13 ....WW....     the way in
```

**The junior trainer is avoidable, and it is column 1 that avoids him.** Rows 3
and 9 are solid but for x=4 and x=5, so the middle band is entered and left
through one two-tile gap at each end — and the straight climb up column 4 passes
(4,6), which is on his line. Column 1 runs from row 8 to row 4 and every tile of
it is to his **left**, so a rightward sight line cannot reach it at any range.

Both routes are measured, 120 attempts each from the same level-10 pool:

| route | reached the goal | where the failures ended |
|---|---:|---|
| up column 4, fighting him | 39/120 (32.5%) | **69 of 81 at (4,6) — his tile** |
| up column 1, avoiding him | 44/120 (36.7%) | all 76 at (4,2) — Brock's |

At level 10 the junior trainer kills more of the chain than Brock does. The
chain takes the avoiding route; the fighting one is kept as
`GYM_TO_BROCK_AVOIDING_THE_TRAINER`'s counterpart, so swapping them back is one
line.

## 9. Certified: 3,000 attempts a link, in order

The chain's standing gate, run in order from the certified link-14 pool, each
link entering from the previous one's exit pool:

| link | reached the goal | 95% CI | distinct trajectories |
|---|---:|---|---:|
| **15-train-for-brock** | **3000/3000** | 100.0 [99.9-100.0] | 3000 / 3000 |
| **16-heal-in-pewter** | **3000/3000** | 100.0 [99.9-100.0] | 3000 / 3000 |
| **17-fight-through-the-gym** | **2659/3000** | **88.6 [87.4-89.7]** | 3000 / 3000 |
| **18-heal-after-the-gym** | **3000/3000** | 100.0 [99.9-100.0] | 3000 / 3000 |
| **19-east-to-route-3** | **3000/3000** | 100.0 [99.9-100.0] | 3000 / 3000 |

**Four of the five clear the bar** — zero failures in 3,000 bounds the failure
rate at about 0.10% by the rule of three, which is the 99.9% claim links 01-06
carry. **The gym does not, and §10 is why.**

Every attempt is a distinct trajectory, so none of this is one attempt counted
three thousand times: the entry state, the driver seed and the idle-frame phase
all move (`--idle-spread 3001`). Wall clock is not comparable between the links
— another crew held ten to sixteen of the twenty cores for most of the run, load
average 15 to 43 — but frames, verdicts and trajectories are contention-free.

**The battle numbers, which for this arc are the point:**

| | battles | won | fled | HP left at the end | faints | items |
|---|---:|---:|---:|---:|---:|---:|
| 15-train-for-brock | 216,850 | 203,054 (93.6%) | 11,829 (5.5%) | 77.3% | **0** | 0 |
| 16-heal-in-pewter | 1,720 | 0 | 1,720 (100%) | 84.4% | 0 | 0 |
| **17-fight-through-the-gym** | **3,000** | **2,659 (88.6%)** | 0 | **92.6%** | **0** | **0** |

Three things in that table are worth reading twice. **Zero faints across
216,850 training battles** — every one of link 15's 11,829 runs was the policy
declining a fight it judged unsafe, not an escape from one it was losing (7
escapes failed in the whole run). **The gym is won at 92.6% HP or lost
outright** — there is no middle; a Wartortle or an Ivysaur two-shots both of
Brock's Pokémon and a Charmeleon does not. And **not one item was used in
220,000 battles**, because the bag is empty at every link of this chain, which
is §10's constraint stated as a measurement.

**The damage model, checked against what actually landed:** link 15 reports
**98.2% of 103,669 non-knockout predictions inside Gen 1's 217-255 roll range**,
13 under and 1,825 over — and "over" is what a critical hit looks like. The gym
reports 86.2% of 11,354, its higher over-rate being Onix's base speed of 70
against a wild Pidgey's 56. Both are measured against **the move the policy
chose**, which they would not have been before §11.

## 10. Charmander cannot reach the bar, and more levels do not fix it

**Every one of link 17's 341 failures ended on Brock's own tile** — map `$36`
(4,2), 336 by fainting and 5 out of decisions — from 341 *distinct* entry
states, so nothing is clustered on one bad start. And they are one line. Reading
the species out of each failing entry state:

| line | certified | 95% CI |
|---|---:|---|
| SQUIRTLE → WARTORTLE | **1000/1000** | 100.0 [99.6-100.0] |
| BULBASAUR → IVYSAUR | **989/1000** | 98.9 [98.0-99.4] |
| CHARMANDER → CHARMELEON | **670/1000** | **67.0 [64.0-69.8]** |

330 of the 341 failures are Charmeleon, 11 are Ivysaur, and **none is
Wartortle**. The pool is exactly a thousand of each, because link 16's exits are
link 15's are link 14's, and link 14's pool was 40/40/40.

**More levels do not fix it.** Per starter at three thresholds — 20 attempts
each at 14 and 18, 40 at 16, alongside the certified thousand:

| line | level 10 | level 14 | level 16 | level 18 |
|---|---:|---:|---:|---:|
| SQUIRTLE → WARTORTLE | 90% | 100% | **100%** | 100% |
| BULBASAUR → IVYSAUR | 0% | 100% | **98.9%** | — |
| CHARMANDER → CHARMELEON | 20% | 35% | **67.0%** | 65% |

**The Charmander line plateaus and does not converge.** Level 18 measured no
better than 16 — the intervals overlap heavily at n=20, so read that as flat
rather than as a decline, which is the point either way: the extra levels are
not buying wins.

That is what the type chart predicts. Wartortle and Ivysaur land a 4x move;
Charmeleon's best against Geodude is EMBER at **0.5x**, which only beats
SCRATCH at 1x because Geodude's Special is 14 against its Defence of 30 — and
Geodude curls that Defence to 120, which settles it. Charmeleon wins on the
opponent being weak, not on being strong against it.

**So link 17 cannot be certified at the chain's 99.9% bar for the Charmander
line, and no level reachable by grinding changes that.** What would: a second
Pokémon that answers ROCK/GROUND, or items. Both change the chain well before
Pewter — the bag is empty at every link so far (0 items used in 220,000
battles, §9), and nothing catchable before Pewter answers ROCK/GROUND
(`gen1_tables.py --plan brock` puts the nearest candidates on Route 4, Mt Moon
and Route 24, all past the gym). Neither is taken here; both are named.

Level 16 is the committed threshold (`BROCK_TRAIN_TO`). It is where all three
lines evolve, which is the only species-independent number in the
neighbourhood, and it is the best the Charmander line does.

## 11. Two counters that were reporting the wrong thing

Both were found by this work and both fix numbers the chain already publishes.

**A run that ends mid-battle scored the battle as a WIN.** `Drop for
ChainDriver` exists so a link whose goal fires as a battle ends still
contributes to the report, and it worked because in an ordinary win or loss the
driver keeps being asked for decisions while the "…fainted!" text plays, so its
last view is decisive. A run the harness stops on `party_hp=0` has no such view:
the last decision was taken one hit before the end, so the lead is standing in
that view and dead in the machine. Measured on the first gym attempt: a level 10
BULBASAUR fainted to the junior trainer, its final state reading `wIsInBattle =
2` and 0/30 HP, and the report said `battles 1: won 1 (100.0%) … faints 0`. An
indecisive view is now counted as **unresolved** and named as such; link 05's
report is unchanged (29 won, 11 lost of 40, before and after).

**The damage-prediction column was measuring the wrong move.** It recorded the
first *damaging usable* move in the list rather than the one the policy chose.
Those are the same move whenever the best one is in slot 1 — true for every link
before the gym, because a starter's only attack is slot 1. In Pewter it is not:
a SQUIRTLE holds TACKLE, TAIL WHIP, BUBBLE and the policy picks BUBBLE. The
column read **29.9% inside the roll range with 474 of 700 landing "over"**;
against the chosen move it reads **87.9% with 68 over**, which is what a
critical-hit rate explains. Link 05's control moved 85.8% → 87.8%.

Consequence worth stating: **the 81-85% damage agreement this project has
published was measured against the wrong move on any turn the policy did not
pick slot 1.** The pass rates are untouched — both fixes change only the
measurement, and the gym link scored 39/120 before and after.

## 12. Two route traps, both the shape this chain keeps finding

*A waypoint whose overshoot is unrecoverable is a waypoint pointing at a trap.*

**Pewter's south-west pocket.** Column 18 is solid on rows 18, 19 and 20 and row
21 is solid from x=0 to x=18, so the left half of those three rows has one way
in and out. A `goto` heading north-east from the Pokémon Center walks into it:
**23 of 23 failures of link 19 were stuck on row 20 between x=13 and x=17**, with
no battle involved at all. Going east along row 26 first and then straight up
column 19 is 3000/3000.

**Pewter is two halves joined at row 13.** For the same reason, every route
across the city goes up and over — the gym is ten waypoints from the Pokémon
Center door and the way back is *not* the reverse.

## 13. Running it

```
cargo build --release --features scenario,image --example pewter_chain

# the whole arc, from a certified link-14 pool
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/pewter/17-fight-through-the-gym.json \
    --start-dir <pool>/exits --runs 120 --workers 14

# per starter, with intervals and the paired gap
./target/release/examples/pewter_chain ab --rom <blue.gb> \
    --scenario scenarios/pewter/17-fight-through-the-gym.json \
    --start-dir <pool>/exits \
    --lead WARTORTLE --lead CHARMELEON --lead IVYSAUR --split n/a --runs 40

# what a pool of entry states actually holds
./target/release/examples/pewter_chain states --rom <blue.gb> \
    --start-dir <pool>/exits --peek 'badges@0xD356'

# the two pre-forest experiments (NOT links — scenarios/experiments/ is
# never read by chain_files, so nothing there can join the chain by accident)
for g in x-train-route-22 x-train-route-2-south; do
  ./target/release/examples/pewter_chain corpus --rom <blue.gb> \
      --scenario scenarios/experiments/$g.json \
      --start-dir <a certified link-10 pool>/exits --runs 40 --workers 6
done
# …then compare against link 11 itself, run the same way. Pair on the seed:
# the entry state and the phase offset are the same whichever grass is walked.
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/pewter/11-train-for-the-forest.json \
    --start-dir <a certified link-10 pool>/exits --runs 40 --workers 6
```

`states` is new, and so is its `--peek NAME@0xADDR`: **a flag a link is about to
be judged on gets read out of real entry states before the goal is written**,
which is how §6 and §7 were settled.

## 14. The film is its own cut, and it must stay that way

**The gym arc films separately from the journey that got here.** The
fourteen-link cut is 190 MB and 3 m 21 s, and **58% of its emulated time is
link 11 alone** — so sewing Brock onto the end makes a *longer* film in which
the new material is a *smaller* fraction. The identity
`seconds_out x speed = the emulated time there is` is the whole argument: the
gym arc is a few emulated minutes against the chain's thirty-one, so at any
shared pace it is a rounding error of the combined runtime.

Same tooling ([`docs/swarm-view.md`](../swarm-view.md),
`tools/pewter-chain/swarm_film.py`), separate output. **Do not re-stitch them
for tidiness.**

> **The shop went in at `18b` after this was written, so `--from 15 --to 19`
> now covers six files rather than five.** `link_number` reads the digits a
> file's own name carries — `18b` answers 18 — so the range still means what it
> says and `19-east-to-route-3` is still in it; what changed is that the arc now
> contains a trip to the mart. See [`cerulean.md`](cerulean.md) §5.

**`film` takes `--from`/`--to` now**, and that is what makes either cut
producible on purpose. It used to play every link there was, which was right
while the chain ended at Pewter and stopped being right the moment it did not:
one command that always plays everything can produce neither film. A slice that
does not begin at link 1 needs somewhere to begin, so `--from` above 1 requires
`--start-dir` (a pool; agent *ag* takes slot `ag % len`, the same rule `corpus`
uses) and refuses without one rather than filming agents on the Nintendo logo.

```sh
# the gym arc, one continuous track per agent, straight out of the emulator.
# 600 agents over 16 workers is 17 min of wall clock on a quiet 20-core box.
./target/release/examples/pewter_chain film --rom <blue.gb> \
    --runs 600 --workers 16 --from 15 --to 19 \
    --start-dir <a certified link-14 exit pool>/exits \
    --out target/gymfilm/tracks

# which starter each agent carries, per index, over the same sorted pool
# `film` hands agent `ag` slot `ag % len` from — see §14c for why this is a
# file rather than a rule.
./target/release/examples/pewter_chain states --rom <blue.gb> --all \
    --start-dir <a certified link-14 exit pool>/exits > pool.txt
#   ...then `n,<the evolved form of column 2>` into lines.csv, with a leading
#   `# by starter line` as its heading.

# --dry-run first: it prints the stage table and the --cover trade-off in
# seconds, and rendering is 20 minutes.
python3 tools/pewter-chain/swarm_film.py target/gymfilm/tracks \
    --out target/gymfilm/frames --speed 26 --fps 60 --cover 0.95 --width 1280 \
    --title "PAST PEWTER 15-19" --agent-labels lines.csv \
    --method "600 agents, 200 per starter line"
ffmpeg -y -framerate 60 -i target/gymfilm/frames/f%05d.png \
    -c:v libopenh264 -b:v 8M -pix_fmt yuv420p pewter-gym-15-19-600agents.mp4
# and a lighter copy for watching away from the machine
ffmpeg -y -framerate 60 -i target/gymfilm/frames/f%05d.png \
    -c:v libopenh264 -b:v 1400k -pix_fmt yuv420p \
    pewter-gym-15-19-600agents-mobile.mp4

# and the journey to Pewter, unchanged and still reproducible
./target/release/examples/pewter_chain film --rom <blue.gb> \
    --runs 600 --workers 8 --to 14 --out target/film14/tracks
```

`film` writes the `agent-NN.links.csv` span file too, so `swarm_film.py`'s
per-link bars and its stage timeline work from a `film` run alone —
`chain_tracks.py` is only needed when a film is stitched from separate corpus
runs. **The spans carry the link's number in the whole chain**, not its
position in the slice, so a span file lines up against a report.

**The arc's own shape**, from `swarm_film.py --dry-run` on a two-agent `film`
run: the training link is **88.5%** of the swarm's emulated time and the whole
arc is **47.6 emulated minutes**. That was a prediction made before the cut
existed; §14a is the same table measured on the 600 agents the film is actually
drawn from, and it held — 89.3% and 53.5 minutes filmed of a 59.2-minute
longest run.

**The same shape link 11 has in the main chain, and it is why the two cuts do
not belong together** — and why the pace has to be higher here than there.
`seconds_out x speed = the emulated time there is` puts 8x at **5 m 55 s**,
longer than the whole fourteen-link cut. The smoothness ceiling is
`stride <= walk_span`, and `walk_span` measures **31** emulated frames on these
tracks, so at 60 fps anything up to **~31x** stays smooth: **24-30x gives a
1½-2 minute film** and is the right pace for material that is seven-eighths
grinding. (The per-agent emulated time is what sets this and it barely moves
with the swarm size, but the *longest* run does, so re-read `--dry-run` before
committing to a pace — on two agents it was 52.0 emulated minutes filmed and on
six hundred it is 53.5.)

**Fedora's ffmpeg has no `libx264`** (patent-clean build); `libopenh264` is the
substitute and `-crf` is not accepted there.

The film is **not committed** — it is game art, and a 115 MB artefact does not
belong in git whatever is in it. The stills are, in
[`chain-stills/`](chain-stills/), under the rule
[`docs/link-trade/`](../link-trade/) established: our interface, doing
something, with the game incidental.

### 14a. The cut, rendered

**`/home/aaron/pokemon-run/pewter-gym-15-19-600agents.mp4`** — 600 agents,
links 15 to 19 in one continuous cut, 1280x720, **26x at 60 fps, 2 m 03 s**,
7,379 frames, `--cover 0.95`. Beside it
`pewter-gym-15-19-600agents-mobile.mp4`, the same frames at `-b:v 1400k`:
**115 MB and 38 MB**.

The tracks are one `film` run — 600 agents, 16 workers, **1041 s of wall clock**
on a quiet box — entering from `target/film11/14/exits`, the certified
fourteen-link corpus's own link-14 exit pool, 600 states, 200 of each starter.
**532 of the 600 finished the arc**, which is 88.7% against the gym's certified
88.6% (§9) — a figure the film reproduced as a side effect of being made.

**What the run measures about itself**, all from `swarm_film.py`'s own report on
the tracks the film is drawn from:

| | |
|---|---|
| emulated time filmed | 191,840 frames = **53.5 minutes** (the longest single run is 212,116) |
| pace | every 26th emulated frame; **glide span 31**, measured from these tracks, so 26 is inside the `stride <= walk_span` ceiling |
| agent motion, 3,724,299 samples | median 0.00 px, p90 **13.42**, p99 15.03; ≥12 px 11.43%, <4 px 81.31% |
| quantisation | **0.8%** of 737,503 moving samples land on the tile grid — 100% without the interpolation |
| camera coverage | mean **93.3%** of live agents on screen, median 94.5%, p5 90.2%, 4.7% of frames below 90% |
| the longest unbroken stretch below 90% | **142 frames (2.4 s), and it is the last 142** — the arrival shot, where the camera has followed the finishers to Route 3 and the last stragglers are still in the grass |
| camera steadiness | 14 distinct crop widths over 7,379 frames, resample ratio moved on 2.2%; still on 79.4% of frames in x and 76.2% in y; 0.89 and 1.19 direction reversals per second; largest single-frame pan 59 and 111 px |
| geometry | the 5-95 swarm box fits the widest view the world allows on **every** frame, so nothing lost is geometric |

**The `worst frame` line is a transient and the run length is what says so.**
It reports frame 1,496 with 599 of 599 off screen — which reads as a disaster
and is one output frame of a zoom snapping back while a first handful of agents
head north to heal. `report_camera` prints the longest unbroken stretch below
par now for exactly this: a mean and a worst frame cannot tell a blink from a
minute, and only the second is something a viewer sees.

**`--cover 0.95`, not the chain's 0.90, and the trade is measured rather than
preferred.** `report_cover` prints the whole ladder at `--dry-run`; the two
rows that decide it are 0.90 → **1 m 59.7 s** with 472 of 532 arrivals on
screen, and 0.95 → **2 m 03.0 s** with 502. Three seconds for thirty more
arrivals. The cost is the thinning tail, and it is small here: the share of the
film with fewer than a hundred agents live goes **3% → 6%**, against the
fourteen-link cut where the same step is much worse and 0.90 is right.

**The stage shares on the rendered cut** — the same table as above, re-measured
on 600 agents rather than two:

| link | emulated | share of the swarm's time | share of the film |
|---|---:|---:|---:|
| **15-train-for-brock** | **2561.0 s** | **89.3%** | **85.1%** |
| 16-heal-in-pewter | 49.0 s | 1.9% | 0.0% |
| 17-fight-through-the-gym | 149.1 s | 6.9% | **14.9%** |
| 18-heal-after-the-gym | 31.1 s | 1.1% | 0.0% |
| 19-east-to-route-3 | 22.6 s | 0.8% | 0.0% |

Three of the five are **never what most of the live swarm is doing**, and that
is not a rendering fault: an agent that finishes stops being drawn, so the
surviving population drifts towards the later links and the three short ones
are always a minority of it. They are in the per-link bars on every frame
regardless. The gym takes 14.9% of the runtime against 6.9% of the emulated
time for the same reason, in the other direction.

**The stage timeline is the thing that makes this watchable.** At 85% of the
runtime on one link, without a labelled strip and a playhead crossing it the
only reading available to a viewer is that the picture has stuck.

### 14b. What it looks like

| | what it shows |
|---|---|
| [9 — the Route 2 grind](chain-stills/9-the-route-2-grind.png) | 26.1 emulated minutes in and **474 of 599 agents in wild battles at once**, a solid red mass in the 48 grass tiles of Route 2's northern half (§4). This is 85% of the film, and the frame is the argument for the timeline: nothing moves, and the reason is that everything is fighting. |
| [10 — the gym, and the line that cannot win it](chain-stills/10-the-gym-and-the-line-that-cannot-win.png) | 36.4 minutes in, the camera holding both the grass and **Pewter Gym as an interior panel**, bottom left, with the orange trainer halo on Brock's own tile. `in a battle: 71 trainer 310 wild`. The by-starter block is the picture: **CHARMELEON 137 going, 2 through, 61 stopped** — against IVYSAUR and WARTORTLE, who between them have lost one agent, and that one on the training link. |
| [11 — the badge, and the three lines](chain-stills/11-the-badge-and-the-three-lines.png) | *The last frame.* The gold marker reading **502** sits on the Route 3 boundary, with Pewter's gym and mart in frame; `30/600 agents live`. And the arc's own result, on screen: **CHARMELEON 134 through and 66 stopped, IVYSAUR 170/1, WARTORTLE 198/1** — §10's certification, reproduced by the film that was made to show it. |

### 14c. The swarm splits along something the tracks cannot see

**Which starter an agent carries is a property of the entry pool it was handed,
not of its track**, so the film cannot know it and must not guess.
`swarm_film.py --agent-labels FILE` takes a CSV of `agent,label` rows — an
optional leading `# title` names the set — and draws a live count per label:
**going, got there, stopped**, with the stopped share as a bar behind the row.
The three counts are one partition of that label's agents, computed by
bisecting the same per-agent end frames the arrival markers use, so the block
and the markers can never disagree about who got there.

`pewter_chain states --all` is the other half: it prints the pool per index
over the same sorted list `film` hands agent `ag` slot `ag % len`, which is
what makes the two line up. On this pool the mapping is **`n % 3`** — 0
CHARMANDER, 1 SQUIRTLE, 2 BULBASAUR — because link 04's ball is `6 + seed % 3`
and 600 divides by three; checked against all 600 rows, **zero violations, 200
of each**. **Do not hard-code that rule anywhere.** It is a property of a pool
whose size happens to be a multiple of three, and it is the whole reason the
mapping is a file rather than an inference.

What the film then reports, on its own tracks, beside §10's thousand-attempt
certification:

| line | in the film | certified (§10) |
|---|---:|---:|
| SQUIRTLE → WARTORTLE | **199/200 (99.5%)** — and its one loss is on link 15, not the gym | 1000/1000 |
| BULBASAUR → IVYSAUR | **199/200 (99.5%)** | 989/1000 |
| CHARMANDER → CHARMELEON | **134/200 (67.0%)** | 670/1000 |

**66 of the film's 68 losses are Charmeleon at the gym.** The Charmeleon figure
lands on the certified 67.0% to the decimal on a fifth of the sample, which is
coincidence at that precision — but the shape is not, and it is the shape the
film was asked to show.

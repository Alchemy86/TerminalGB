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

### Is anything pre-forest better than Route 1? Measured: no

Link 11 is the single largest consumer of the chain's emulated time, so this is
worth an answer rather than an assumption. Every pre-forest map with grass, from
`WildDataPointers` and the walkability grid:

The first four columns are computed from `WildDataPointers` and `BaseStats`;
the last is a breadth-first path length over the walkability grid. Both are
cartridge reads, not measurements of a run.

| grass | map | mean exp/encounter | rate | exp per step | steps to the Viridian nurse |
|---|---|---:|---:|---:|---:|
| **Route 1**, rows 6-9 | `$0C` | 25.0 | 25/256 | 2.44 | **25** |
| Route 2 **south**, rows 48-51 | `$0D` | 29.4 | 25/256 | 2.87 | 56 |
| Route 22, rows 8-11 | `$21` | 27.0 | 25/256 | 2.64 | 79 – 82 |
| Viridian Forest | `$33` | 39.7 | **8**/256 | **1.24** | — |

Route 2's southern half looks the best of them on paper: the same encounter
table as the northern half, 17.6% richer per encounter than Route 1. **It is
measurably worse.** Same job (level 5-6 → 10), same entry pool, same seeds, same
worker count:

| | reached the goal | median frames | mean frames | median decisions |
|---|---:|---:|---:|---:|
| **Route 1** (link 11, as shipped) | **40/40** | **65,904** | 71,633 | 656 |
| Route 2 south (`scenarios/experiments/`) | 34/40 | 72,294 | 85,225 | 669 |

The thirty-one extra steps to the nurse, each way, more than eat the richer
table. **Route 1 is already the best pre-forest ground**, and it wins on the one
term that is not the encounter table: it is by far the closest to a Pokémon
Center.

Route 22 was **not run**. It is further from the nurse than Route 2 south (79-82
steps against 56) and less rich (27.0 against 29.4), so it loses on both terms
to a candidate that already lost. That is an argument from two measured numbers,
not a measurement, and it is labelled as such.

Viridian Forest is the interesting one and it loses on the term nobody looks at:
its encounter *rate* is 8/256 against Route 1's 25, so despite being 59% richer
per encounter it is **half as good per step**.

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

## 9. What the gym measures, at the level the chain now delivers

Threshold 16, **120 attempts** through the whole arc from the certified link-14
pool. **The chain's standing gate is 3,000 attempts per link and that has not
been run for these five** — zero failures in 120 bounds a failure rate only at
about 2.5% by the rule of three, nine times short of the 99.9% claim links 01-06
carry. Do not quote these as certified.


| link | reached the goal |
|---|---|
| 15-train-for-brock | **120/120** |
| 16-heal-in-pewter | **120/120** |
| **17-fight-through-the-gym** | **108/120 (90.0%)** |
| 18-heal-after-the-gym | **120/120** |
| 19-east-to-route-3 | **120/120** |

## 10. Charmander cannot reach the bar, and more levels do not fix it

The gym link's 90% is not spread evenly. Per starter, from the same pools the
arc above ran on — 20 attempts each at levels 14 and 18, **40 each at 16**:

| line | level 10 | level 14 | **level 16** (all three lines have evolved) | level 18 |
|---|---:|---:|---:|---:|
| SQUIRTLE → WARTORTLE | 90% | 100% | **100%** (40/40) [91.2-100] | 100% |
| BULBASAUR → IVYSAUR | 0% | 100% | **97.5%** (39/40) [87.1-99.6] | — |
| CHARMANDER → CHARMELEON | 20% | 35% | **72.5%** (29/40) [57.2-83.9] | 65% |

**The Charmander line plateaus and does not converge.** Level 18 measured worse
than 16 — the intervals overlap heavily at n=20, so read that as flat rather
than as a decline, which is the point either way: the extra levels are not
buying wins.

That is what the type chart predicts. Squirtle and Bulbasaur are landing a 4x
move; Charmeleon's best against Geodude is EMBER at **0.5x**, which only beats
SCRATCH at 1x because Geodude's Special is 14 to its Defence of 30. It wins on
the opponent being weak, not on being strong against it.

**So link 17 cannot be certified at the chain's 99.9% bar for the Charmander
line, and no level reachable by grinding changes that.** What would: a second
Pokémon that answers ROCK/GROUND, or items. Both change the chain well before
Pewter — the bag is empty at every link so far, and nothing catchable before
Pewter answers ROCK/GROUND (`gen1_tables.py --plan brock` puts the nearest
candidates on Route 4, Mt Moon and Route 24, all past the gym). Neither is taken
here; both are named.

Level 16 is the committed threshold (`BROCK_TRAIN_TO`). It is where all three
lines evolve, which is the only species-independent number in the neighbourhood,
and it is the best the Charmander line does.

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
column 19 is 120/120.

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

# the pre-forest experiment (NOT a link — scenarios/experiments/ is never
# read by chain_files)
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/experiments/x-train-route-2-south.json \
    --start-dir <link-10 pool>/exits --runs 40
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

**`film` takes `--from`/`--to` now**, and that is what makes either cut
producible on purpose. It used to play every link there was, which was right
while the chain ended at Pewter and stopped being right the moment it did not:
one command that always plays everything can produce neither film. A slice that
does not begin at link 1 needs somewhere to begin, so `--from` above 1 requires
`--start-dir` (a pool; agent *ag* takes slot `ag % len`, the same rule `corpus`
uses) and refuses without one rather than filming agents on the Nintendo logo.

```sh
# the gym arc, one continuous track per agent, straight out of the emulator
./target/release/examples/pewter_chain film --rom <blue.gb> \
    --runs 120 --workers 14 --from 15 --to 19 \
    --start-dir <a certified link-14 exit pool>/exits \
    --out target/gymfilm/tracks

# --dry-run first: it prints the stage table and the --cover trade-off in
# seconds, and rendering is not quick.
python3 tools/pewter-chain/swarm_film.py target/gymfilm/tracks \
    --out target/gymfilm/frames --speed 8 --fps 60 --cover 0.90 --width 1280 \
    --title "PAST PEWTER 15-19"
ffmpeg -y -framerate 60 -i target/gymfilm/frames/f%05d.png \
    -c:v libopenh264 -b:v 8M -pix_fmt yuv420p past-pewter-15-19.mp4

# and the journey to Pewter, unchanged and still reproducible
./target/release/examples/pewter_chain film --rom <blue.gb> \
    --runs 600 --workers 8 --to 14 --out target/film14/tracks
```

`film` writes the `agent-NN.links.csv` span file too, so `swarm_film.py`'s
per-link bars and its stage timeline work from a `film` run alone —
`chain_tracks.py` is only needed when a film is stitched from separate corpus
runs. **The spans carry the link's number in the whole chain**, not its
position in the slice, so a span file lines up against a report.

**The arc's own shape, from those spans** (one agent, 144,810 emulated frames):
the training link is **85% of it** — 123,426 frames against the gym's 12,294
and the two heals' 4,824 and 2,586. The same shape link 11 has in the main
chain, and the reason the two cuts do not belong together.

**Fedora's ffmpeg has no `libx264`** (patent-clean build); `libopenh264` is the
substitute and `-crf` is not accepted there.

The film is **not committed** — it is game art, and a 190 MB artefact does not
belong in git whatever is in it. The 01-14 cut's stills are in
[`chain-stills/`](chain-stills/) under the rule
[`docs/link-trade/`](../link-trade/) established: our interface, doing
something, with the game incidental.

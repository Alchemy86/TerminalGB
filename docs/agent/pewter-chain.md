# The Pewter chain — fourteen links from a cold cartridge, and the corpus they produced

A brand-new game of Pokémon Blue, played forward to Pewter City, cut into
fourteen [scenarios](../scenarios.md) whose goals are read out of the
cartridge's own memory. A scripted expert solves them; what comes out is a
corpus of runs — successes and failures — a chain of save states any link can
be started from instantly, and the list of places a solver reliably struggles.

```text
   cold cartridge
        │  01 new game ─▶ 02 out of the house ─▶ 03 Oak takes you to the lab
        │  04 a starter ─▶ 05 the rival ─▶ 06 Route 1 ─▶ 07 the parcel
        │  08 back to Pallet ─▶ 09 the Pokédex ─▶ 10 back to Viridian
        │  11 train ─▶ 12 heal and climb ─▶ 13 the forest ─▶ 14 Pewter
        ▼
   corpus + chained save states + heat maps
```

- **Scenarios**: [`scenarios/pewter/`](../../scenarios/pewter/) — fourteen JSON
  files, one per link.
- **Solver and runner**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs).
- **Cartridge tables**: [`tools/gen1-tables/gen1_tables.py`](../../tools/gen1-tables/gen1_tables.py).
- **Corpus report and heat maps**: [`tools/pewter-chain/corpus_report.py`](../../tools/pewter-chain/corpus_report.py).

---

## 1. What has and has not been done

**Nothing here is trained.** No policy has been learned, no weights exist, and
no number on this page is evidence about a model. What exists is the thing
before that: goals that cannot be argued with, a solver that reaches them, and
a corpus shaped so a learner could use it.

**The solver is scripted, and that is not a detail.** It implements the same
`Driver` trait a live model plugs into — the same JSON snapshot in, the same
JSON plan out, through the same parse-clamp-execute path — and it satisfies
`no_memory_writes` for the same reason a model would: it can only press
buttons. But it knows the route because a person wrote the route down. That is
exactly the role the PokéAgent Challenge winner gave its scripted policies:
solve each subgoal so the trajectories exist. Here the trajectories are the
product.

**No live model has solved any of these links.** `docs/scenarios.md` said that
before this work and it still says it.

---

## 2. The chain, and what defines success

Every goal is a memory read with a citation. Where the cartridge records the
milestone as an event flag, the flag is the goal; where it does not, `wCurMap`
is the most exact record there is, and the table says which is which.

| # | link | goal | defined by |
|---|---|---|---|
| 01 | new game | `map=0x26` and `watch:map_width>0` | `wCurMap` $D35E, `wCurMapWidth` $D369 |
| 02 | leave the house | `map=0x00` | `wCurMap` $D35E |
| 03 | Oak takes you to the lab | `watch:followed_oak_into_lab!=0` | **flag** $D747 bit 0 |
| 04 | take a starter | `watch:got_starter!=0` and `party>=1` | **flag** $D74B bit 2, `wPartyCount` $D163 |
| 05 | the rival battle | `watch:battled_rival_in_lab!=0` | **flag** $D74B bit 3 |
| 06 | cross Route 1 | `map=0x01` | `wCurMap` $D35E |
| 07 | collect Oak's Parcel | `watch:got_oaks_parcel!=0` | **flag** $D74E bit 1 |
| 08 | carry it back to Pallet | `map=0x00` | `wCurMap` $D35E |
| 09 | receive the Pokédex | `watch:got_pokedex!=0` | **flag** $D74B bit 5 |
| 10 | back to Viridian | `map=0x01` | `wCurMap` $D35E |
| 11 | train for the forest | `watch:lead_level>=10` | `wPartyMon1Level`, `wPartyMons` $D16B + 33 |
| 12 | heal and climb Route 2 | `map=0x33` | `wCurMap` $D35E |
| 13 | cross Viridian Forest | `map=0x2F` | `wCurMap` $D35E |
| 14 | arrive in Pewter | `map=0x02` | `wCurMap` $D35E |

Every one of those addresses is in
[AtlasGB's evidenced snapshot](../../third_party/atlasgb/README.md) with its
evidence tier, and every event index was computed by walking
`pokered`'s `constants/event_constants.asm` const list — `const_def`,
`const_skip`, `const_next` — rather than looked up. The anchor that says the
walk is right: `EVENT_FOLLOWED_OAK_INTO_LAB` comes out at index 0, byte 0,
bit 0, which is what `AGENTS.md` already recorded independently.

Each scenario file carries an `evidence` list saying this in prose, and
`scenario check` prints it. A scenario **without** one is reported as
`evidence: NONE STATED`, because a goal nobody can check is back to being a
proxy.

### The dependency order is not the obvious one

The Pokédex does not come with the starter. Read out of the scripts:

1. `PalletTownDefaultScript` fires on `wYCoord == 1` — its own comment says
   *"is player near north exit?"* — and the tiles at x=10,11 on rows 0 and 1
   are tileset tile `$52`, the Overworld grass tile. So the trigger is walking
   into the grass at the top of the map.
2. Oak **escorts** the player in; `OaksLabFollowedOakScript` sets
   `EVENT_FOLLOWED_OAK_INTO_LAB` when the simulated `PAD_UP` × 8 walk finishes.
3. Starter, then the rival — and `OaksLabRivalChallengesPlayerScript` opens
   `ld a,[wYCoord] / cp 6 / ret nz`, so he challenges you on row 6 of the
   laboratory and nowhere else.
4. `ViridianMartDefaultScript` hands over the parcel on the first visit with
   **no prerequisite at all**; entering the shop is the whole of that link.
5. `OaksLabOak1Text`'s `.check_got_parcel` branch needs the parcel in the bag,
   and `OaksLabOakGivesPokedexScript` sets `EVENT_GOT_POKEDEX` and
   `EVENT_OAK_GOT_PARCEL` together.
6. `ViridianCityCheckGotPokedexScript` tests `EVENT_GOT_POKEDEX` before it lets
   the player past the old man. **The Pokédex is the gate on the road north.**

---

## 3. The route came out of the cartridge, and told us two things

Every waypoint the solver walks is a turning point of a breadth-first path over
a walkability grid built from `maps/<Name>.blk`, the tileset's blockset and the
tileset's collision tile list, with the collision probe on the walk tile's
**bottom-left** 8×8 tile (block-definition indices 4, 6, 12, 14 — the anchor
the World Trade Centre work measured). Two things fell out of that grid that no
walkthrough would have said:

- **Route 1 northbound and southbound are different paths.** Its ledges are
  one-way, so the climb is not the descent reversed. The chain carries two
  waypoint lists for one map.
- **Route 2's two halves are separate components of the walkable graph.** The
  southern half reaches Viridian; the northern half reaches Pewter; nothing
  connects them on foot. Viridian Forest is not a scenic route, it is the only
  route — which is why the chain has to cross it.

And a third, from the same grid plus the map's object list:

- **The forest's three bug catchers are unavoidable.** Blocking their sight
  cones at four, five and six tiles and re-pathing gives **no route at any of
  them**. That is what makes link 11 necessary.

---

## 4. The most valuable entry: a link that scored 100% on the wrong task

The first version of link 03 was called `cross-pallet-to-the-lab`. Its goal was
`map=0x28 AND watch:oak_appeared_in_pallet!=0` — a position, plus the flag that
*arms* Oak's encounter rather than the flag that records it happening. It
solved 6/6.

It was measuring the wrong thing, and it was caught by a human reading the
results, not by any check we had. `EVENT_OAK_APPEARED_IN_PALLET` is set the
instant the player steps onto row 1; `map=0x28` would be satisfied by walking
to the laboratory door under your own steam, which is *not what the game asks
for at that point*. The milestone the cartridge records is
`EVENT_FOLLOWED_OAK_INTO_LAB`, set at the end of the escort.

**This is precisely the failure this project criticises in guide-derived work**
— a goal authored from a plausible idea of how the game goes — and the atlas
was right there. Two things came out of it:

- the link is re-authored against the flag, and every other link was audited
  the same way (nine have no flag behind the milestone at all and legitimately
  use `wCurMap`; the five that do have one already used it);
- scenarios gained the `evidence` field, so the next person can check the goal
  without re-deriving it, and `scenario check` complains when it is missing.

The frozen artefacts of the wrong version are kept at
`/home/aaron/pokemon-run-versions/v001-prefix-oak-goal-error/`. **A wrong
version is evidence.**

---

## 5. Measured

Machine: 20-core x86-64, headless, no audio, load average 11–18 with other work
on the box. Cartridge: the captain's own Pokémon Blue. Nothing below is an
estimate.

### The chain, end to end

Fourteen links from a cold cartridge — the Nintendo logo, no save state, no
memory writes — to standing in Pewter City. Six seeds per link, carrying each
link's finish forward as the next link's start:

| # | link | solved | median decisions | median frames |
|---|---|---|---|---|
| 01 | new game | 6/6 | 18 | 4,302 |
| 02 | leave the house | 6/6 | 6 | 846 |
| 03 | Oak takes you to the lab | 6/6 | 13 | 2,052 |
| 04 | take a starter | 4/6 | 32 | 4,716 |
| 05 | the rival battle | 3/6 | 40 | 8,400 |
| 06 | cross Route 1 | 6/6 | 24 | 4,032 |
| 07 | collect Oak's Parcel | 6/6 | 17 | 8,772 |
| 08 | carry it back to Pallet | 6/6 | 45 | 5,166 |
| 09 | receive the Pokédex | 5/6 | 22 | 6,108 |
| 10 | back to Viridian | 6/6 | 93 | 11,472 |
| 11 | train for the forest | 2/6 | 767 | 122,142 |
| 12 | heal and climb Route 2 | 6/6 | 61 | 19,050 |
| 13 | cross Viridian Forest | 4/6 | 93 | 13,794 |
| 14 | arrive in Pewter | 6/6 | 7 | 1,152 |

**Six seeds cannot certify anything** and this table is not offered as a solve
rate; it is what the chain did while it was being built.

### The corpus, per link

Attempts fanned across eight workers, each from a pool of entry states taken
from *different winning seeds* of the previous link — so a link is not measured
against one arrival but against several. Every attempt, successful or not, is
in `/home/aaron/pokemon-run/corpus/<link>/`.

| # | link | reached | rate | median decisions | how the rest ended |
|---|---|---|---|---|---|
| 01 | new-game | 120/120 | 100.0% | 18 | — |
| 02 | leave-the-house | 120/120 | 100.0% | 6 | — |
| 03 | oak-takes-you-to-the-lab | 400/400 | 100.0% | 13 | — |
| 04 | take-a-starter | 372/400 | 93.0% | 35 | out of calls ×28 |
| 05 | the-rival-battle | 250/600 | **41.7%** | 46 | party wiped ×350 |
| 06 | cross-route-1 | 599/600 | **99.8%** | 49 | timeout ×1 |
| 07 | collect-oaks-parcel | 199/200 | 99.5% | 10 | out of calls ×1 |
| 08 | carry-it-back-to-pallet | 225/300 | 75.0% | 56 | timeout ×74, out of calls ×1 |
| 09 | receive-the-pokedex | 196/200 | 98.0% | 22 | timeout ×3, out of calls ×1 |
| 10 | back-to-viridian | 300/300 | 100.0% | 69 | — |
| 11 | train-for-the-forest | 53/120 | **44.2%** | 878 | timeout ×67 |
| 12 | heal-and-climb-route-2 | 260/300 | 86.7% | 61 | timeout ×36, out of calls ×4 |
| 13 | cross-viridian-forest | 248/300 | 82.7% | 93 | out of calls ×32, party wiped ×16 |
| 14 | arrive-in-pewter | 197/200 | 98.5% | 20 | party wiped ×3 |

**4,140 attempts across the fourteen links.** Link 08's row is the *pre*-fix
figure — the map-settle change measured below takes it to 90.8% — and link 06
was re-run at 600 attempts for the showcase film, which is why it carries the
larger denominator.

**Not one of these is at the bar**, including the 100%s — see §7 — and three
are a long way from it. Multiplied out, the chain end to end at these rates is
about **6%**: link 05 alone (41.7%) and link 11 (44.2%) between them cost an
order of magnitude, and that is the compounding argument made concrete rather
than asserted. It is also why the chain is played link by link from a carried
state rather than end to end — which is exactly what the save-state chain is
for.

### The adjustment log

The interesting record is what changed and what it bought.

| link | diagnosis | change | before → after |
|---|---|---|---|
| 08 | 74 timeouts clustered on Viridian City row 28, x=27–32. The trace showed the mart exit reporting `(3,7)` — the *mart's* coordinate — with the map already reading Viridian City, so the first `goto` was aimed from where the player had been and walked past the one gap in row 27. | wait 24 frames on the first decision after a map change, so the observation has settled | **75.0% → 90.8%** (120 attempts) |
| 13 | party fainting in the forest | insert link 11, threshold read from `BugCatcherData` | unsolvable → solved |
| 05, 12 | wild encounters fought in the forest at ~15 decisions each for almost no experience | run from wild in the forest, fight on the routes | budget exhausted → crossed |
| all | seeds 0/1 and 2/3 byte-identical | mix the seed before forcing its low bit | half the corpus duplicated → 390 distinct end states of 400 |

**Link 05 has no adjustment, and that is the finding.** All 350 failures are
the party being wiped at (4,6)/(5,6) — the rival battle tile. At level 5 both
starters have a damaging move and a status move and neither type matters, so it
is close to a coin flip *by the game's design*. It is not a routing bug, and
more episodes will not move it: the repair, if there is one, is a precondition
(a stronger party) that the game does not make available before this point.

### Throughput, and determinism under it

`corpus` fans attempts across threads. Measured on link 06 with 400 attempts
and 8 workers:

```text
400 of 400 reached the goal (100.0%)
72.3 s wall, 5.53 episodes/second, peak concurrency 8   (757% CPU)
```

**Determinism survives the fan-out**, and it is checked rather than asserted:
`pewter_chain verify` plays the same seeds serially and across the workers and
requires identical final-state hashes and frame counts.

```text
seed     0  serial b4515720a865ad50/  9030 frames   parallel b4515720a865ad50/  9030   same
seed     1  serial 5db7d3d5ae74efc9/  5694 frames   parallel 5db7d3d5ae74efc9/  5694   same
seed     2  serial eb9ad5a4e2bafd9b/  7092 frames   parallel eb9ad5a4e2bafd9b/  7092   same
seed     3  serial f6f6d8941bc0a329/  5844 frames   parallel f6f6d8941bc0a329/  5844   same
seed     4  serial 1a271707712fd740/  7014 frames   parallel 1a271707712fd740/  7014   same
DETERMINISTIC — 5 seed(s) identical serially and in parallel
```

A run is a function of (entry state, scenario, seed) and nothing else: the
driver's randomness is a seeded xorshift, the emulator is deterministic, and no
two workers share anything but the read-only cartridge bytes.

---

### Where the wall clock actually goes

Measured over the same 400-episode round: 2,522,856 emulated frames took
570,395 ms of worker time against 578,400 ms of worker capacity (72.3 s × 8).

```text
per worker            4,423 frames/second = 0.226 ms/frame
emulation             98.6% of the round's worker capacity
the scripted driver   the remaining 1.4%, including thread and file overhead
```

**This substrate is environment-bound, not policy-bound, and by a wide margin.**
A purpose-built C toy environment reaches ~200M steps/second and its own
authors conclude that the *policy* is the cap there — a small MLP at roughly
650k inferences/second. Ours is ~4,400 frames/second per core, so we are
env-bound by something like **150×** relative to that.

Stated plainly, both halves: that is the price of running the real cartridge —
their environment has no PPU, no battles, and no game logic — and it means
**optimising the policy's inference cost here would be wasted effort**. If a
round needs to be faster, the emulator or the number of cores is the lever.

### In the unit the community reports, and we are not the fastest

The convention elsewhere is aggregate steps per second across environments, so
here it is in that unit rather than in ms/frame:

| | measured |
|---|---|
| per worker | **4,423 frames/s** (0.226 ms/frame) |
| 8 workers, load average ~12 | **34,894 frames/s aggregate** |
| 8 workers, load average ~40 | 2.25 episodes/s — **half** the above |

Hardware: 20-core x86-64, headless, no audio. **The load average belongs beside
every one of these**: this box is shared, and the same 160-attempt run at 4, 8
and 16 workers under load 37–45 gave 1.40, 2.25 and 3.47 episodes/second — real
scaling, but against a contended machine. A clean 20-worker figure was not
measured, so none is quoted.

**We are not the fastest and should never claim to be.** `pok2gpu-plus` runs
8,192 environments on a GPU at roughly 160k steps/second, about twice what this
does across twenty cores. The claim here is accuracy and verifiability; an
indefensible speed claim would only undermine the defensible ones.

### The directly comparable published work, and what the paper actually says

**SENSEI** (Sancaktar et al., *Semantic Exploration Guided by Foundation Models
to Learn Versatile World Models*, ICML 2025,
[arXiv:2503.01584](https://arxiv.org/abs/2503.01584),
[code](https://github.com/martius-lab/sensei)) plays **Pokémon Red from
pixels**, and its evaluation is this page's evaluation under other names: it
partitions the map into numbered segments from the start (**0 = Oak's Lab**,
**6 = Viridian Forest**, **9 = Brock's Gym**) and plots *the percentage of
random seeds that reach each segment*. That is a chain of links and a per-link
success rate.

It compares against **DreamerV3** and **Plan2Explore**, and reports that *"only
Sensei reaches the first Gym"*.

**One correction, from reading the paper rather than the summary of it.** A
widely repeated version of this result is "first gym badge within 2M steps of
exploration". The paper does not say that. Section 4.4 claims reaching the
Gym; the badge appears only after *a second round of VLM annotations refined
the reward* and the run was continued for a further 200 episodes. Cite the
paper.

**The contrast is the useful part, and it cuts both ways.** SENSEI's
exploration reward is a **model's semantic judgement** — a VLM prompted to rank
pairs of observations by interestingness, distilled into a reward. Ours is a
**bit the cartridge sets**. That is the olmOCR-2 distinction — ground truth
that does not depend on a model's opinion — applied against a directly
comparable system on the same game.

But: **theirs works, from pixels, and is published; ours is not trained at
all.** And their method **generalises to games where no atlas exists**, because
a VLM can rank observations anywhere; ours needs the memory archaeology done
first, per game, before it can express a single goal. That is a real limitation
of this approach and not a small one.

SENSEI is the comparison target for the screen-only student, when there is one.
(If this repository grows a `docs/prior-art.md`, this section belongs there.)

### On native ports, fairly

A view worth engaging with is that *"moving away from emulation to a native
port is the real way to make gains"*. For raw speed that is simply true, and
if approximate dynamics are acceptable it is a reasonable engineering choice.

But a native port is a **reimplementation**, and a reimplementation has no
ground truth: you are training against somebody's copy of Pokémon rather than
Pokémon. The people proposing it say the operational half themselves — that a
recompilation *"would probably break hooks and timing may be off"* (community
reporting) — which is the same objection from the inside. Every reward on this page is a bit the cartridge sets, which is a
guarantee a port cannot offer about itself — the same reconstruct-versus-
photograph argument the map art makes, applied to the environment. The two
goals are incompatible rather than one being wrong.

### One thing this is not a claim to

**Being first.** Community reporting (Discord, June 2026) has people claiming
Pokémon beaten end to end with RL agents, with Peter Whidden's own line being
*"beat it with caveats! im working on a new vid that explains more"*, and
others reported past the third gym. **The video explaining the caveats is not
out, so the nature of the achievement is not public.** The only concrete
caveat visible is from a *different* person about a *different* run — that it
"does skip lt surge and erika most of the time" — and nothing says whether the
completing run was one trained policy end to end, a chain of specialists,
heavily shaped, hand-helped at gates, or skipping content.

So: the claims exist and are attributed and dated; their content is unknown;
and **no argument here rests on either reading**. Anything still citing "no
open-source model has completed a Pokémon RPG" as an open opportunity should
stop, but the opposite claim is not available either.

The differentiator here was never priority, and it is not decomposition
either. **Decomposition is not our idea and it has already won**: the PokéAgent
Challenge winner generated subgoals with explicit success predicates, solved
each with a scripted policy and distilled the result, and third place used
deterministic pathfinding — two of the top three decomposed. The community has
also built real tooling: `poke-rl`, a map visualiser, twenty million battle
trajectories.

**The gap is narrower and duller than "nobody thought of this".** Investment
has gone into the *learner* and into *environment speed*, and very little into
the **game-truth layer**: an atlas with per-address evidence, map art verified
against the running cartridge, a battle engine checked against it. That layer
is what makes a reward a bit the cartridge set, `no_memory_writes` an enforced
rule rather than a request, and a seed reproduce a run.

That is a contribution to the substrate, not to the idea — and it is worth
saying without any smugness, because the people doing the other half publish
their own gap lists and release their code, which is more than most.

## 6. Diversity, measured rather than claimed

A corpus of one trajectory replayed teaches nothing, and "we varied the seed"
is not evidence that anything varied. Link 06, 400 attempts from a pool of
three entry states:

```text
attempts              400
reached the goal      400  (100.0%, 95% CI 99.0-100.0%)
distinct end states   390 of 400
decisions             min 23  p50 49  p90 73  max 178
emulated frames       min 3792  p50 5892  p90 8430  max 22608
distinct routes       344 of 400 (by the set of tiles stood on)
mean pairwise overlap 0.699   (1.0 = one path replayed, 0.0 = no shared ground)
time by map           Route 1 70%, Oak's Lab 18%, Pallet Town 12%
```

The seed moves: which of three preset names, which of three Poké Balls, which
column the rival is met on, how far the solver wanders before resuming, the
`goto` step budget, and which of several entry states the attempt begins from.

**One bug found by measuring it.** The driver seeded its generator with
`seed | 1`, the idiom `RandomDriver` uses to avoid a zero state — which maps
every even seed onto its odd neighbour. Seeds 0 and 1 produced byte-identical
recordings, as did 2 and 3: **half the corpus was a duplicate of the other
half**, and nothing in a success rate would ever have shown it. The seed is
mixed before the low bit is forced now.

---

## 7. The bar, and why none of this meets it

The standing bar is **99.9% per link**, because a chain compounds: fourteen
links at 99.9% completes 98.6% of the time, at 99% it is 87%, and at the 95%
several of these links sit at it is 49%.

**A 400-attempt run cannot demonstrate 99.9%.** By the rule of three, zero
failures in N bounds the failure rate at about 3/N, so 400 clean attempts
support only ≤0.75% — nine times worse than the bar. Roughly **3,000 clean cold
attempts** are needed to support a 99.9% claim, and no link here has had them.
Every rate on this page is written with its N and its interval for that reason,
and `corpus` prints the bound itself so it cannot be quoted without one.

**Links 04, 05, 09, 11 and 13 are below the bar and are therefore unfinished.**
Their failure modes are §8.

---

## 8. Where it gets stuck — with the evidence

Every one of these was found by running the chain and reading
`GB_CHAIN_TRACE=1`, which prints the map, position, stage, screen kind, party
and chosen plan for every decision.

### 8.1 A machine with no sound chip hangs in the Pokémon Center

`Gameboy::new` leaves `MMU.sound` at `None`, which is a machine with **no APU**
rather than a quiet one. The nurse's healing sequence never finishes on such a
machine: the party heals, "OK. We'll need your POKéMON." stays on screen, and
the game accepts no input again. Measured: **700 decisions parked at (3,3)**,
every walk refused.

This is the same rule `testharness/testroms.rs` already records — *never build
a test machine without an APU* — and it cost a day here because the symptom
looks like a routing bug. `sound::SilentPlayer` is the fix and it costs nothing.

### 8.2 A text box with no `▼` still blocks movement

The observer classifies a box the script has finished with as `text` rather
than `dialogue`, with `waiting_for_input` false — "there is text, but nothing
is asking for a button". A solver that reads that as "I may walk" issues walk
plans into a box that swallows every one.

The cure needs a bound, because it is the same deadlock the other way round:
Gen 1 redraws the tilemap when the player *moves*, so pressing A at a finished
box never clears it either. Three tries at an unchanged screen and then walk
anyway.

### 8.3 The battle menu cannot be navigated by a burst of presses

`HandleMenuInput` delays several frames after accepting a press, so the first
of a burst is routinely swallowed and the rest land one step out of phase.
A run policy that fired `down, right, A` blind opened the party list instead —
**2,500 decisions of it**. Reading the `▶` cursor out of the screen and pressing
one key per decision cannot go out of phase, because a swallowed press repeats.

### 8.4 A dead band between "too hurt to fight" and "hurt enough to heal"

With the fight threshold at half health and the heal threshold at a third, the
lead settled at **8 of 23 HP** — too hurt to pick a fight, not hurt enough to
go and get healed — and ran from everything for 2,402 decisions. The heal
threshold has to *be* the fight threshold.

### 8.5 A waypoint is not "near enough"

Allowing one tile of slack on waypoint arrival broke two links in opposite
ways. Link 03 parked at (10,2) with Oak's trigger on (10,1) and waited for
ever. Link 12 oscillated between (20,28) and (21,28) for 430 decisions because
Viridian City's row 27 is `#.###.##############` — two gaps in twenty tiles —
and the waypoint it skipped *was* the gap.

### 8.6 An exit state captured mid-battle forfeits the next link

Link 11's goal fires when the lead reaches level 10, which is inside a battle
about half the time. The next link's battle observer then saw that battle end
in a knockout and counted it as a win — and a `battle flee` leg forfeits on a
win. Three decisions, every seed. The observer primes on its first sample now,
and an inherited battle belongs to whoever produced the start state.

### 8.7 A held START leaks across a link boundary

Link 01's introduction plan ended `press A ×6, press START`. `wCurMapWidth`
becomes non-zero — the goal — while START is still held, so the state carried
forward opened the start menu, and link 02 spent its whole budget in the
OPTION screen changing the text speed. The introduction presses A only now, and
the solver has a start-menu detector that answers with B.

### 8.8 The training link is the slowest thing in the chain

Link 11 is 44.2% over 120 attempts and a median of **878 decisions** — an order
of magnitude more than any other link. Every failure is a timeout. It is doing
the only thing available: pacing Route 1's grass beating level 2–4 Pidgey and
Rattata for a level 6 starter, walking back to Viridian to heal whenever it
drops below half, and repeating until the lead reaches 10.

That is a real cost of the repair rather than a defect in it — the game offers
no faster experience before the forest — but it is where an agent would gain
most from something better than "pace and fight everything".

### 8.9 The forest, which is genuinely hard

Link 13 is the one that still fails, and it fails two ways:

- **it runs out of decisions** — 32 of the 52 failures over 300 attempts, the
  1,200-call ceiling spent in the maze.
- **the party faints** — the other 16. Three compulsory bug catchers, six
  Pokémon, top level 9, no healing between them, and tall grass throwing wild
  encounters the whole way. Observed ending at (1,18) and (26,19), which are
  the third and second trainers.

Fighting the forest's own wild Pokémon makes it worse, not better: Caterpie,
Weedle and their cocoons give almost no experience and cost about fifteen
decisions to beat against four to run from. Measured — fighting them spent the
entire budget without leaving the maze.

---

## 9. The repair loop, and the tables that make it actionable

Link 13 kept failing by fainting. **More episodes is the wrong response to
that**; the right question is which precondition is unmet, and the cartridge
answers it.

1. **Diagnose.** Read the party. The lead entered at level 6 with 21 HP.
2. **What is it up against?** `BugCatcherData`, read out of the ROM: the
   forest's three rosters are level 6 (Weedle, Caterpie), level 7 (Weedle,
   Kakuna, Weedle) and level 9 (Weedle).
3. **Are they avoidable?** No — blocking their sight cones and re-pathing gives
   no route at four, five or six tiles.
4. **So the precondition is a lead above their top level**, and the threshold
   is `max(roster level) + 1 = 10`. Not a round number somebody liked.
5. **Insert a remedial link** whose goal is exactly that:
   `watch:lead_level>=10`, an atlas read, trainable by the same machinery.

That is link 11, and the chain completes with it.

### The tables, read out of the cartridge

[`tools/gen1-tables/gen1_tables.py`](../../tools/gen1-tables/gen1_tables.py)
locates the type chart, the wild encounter tables, the base stats, the species
names and `PokedexOrder` **by signature in the ROM the user supplied**, then
reads their contents from it — the same technique
`plugins/battle-control/src/wram.rs` uses for WRAM, so a hack or a translation
is a mismatch rather than a silent mis-read.

```console
$ python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --report
  MonsterNames      @ 0x01C21E  190 names
  PokedexOrder      @ 0x041024
  BaseStats         @ 0x0383DE
  TypeEffects       @ 0x03E474  82 rows
  WildDataPointers  @ 0x00CEEB  bank 3 (BLUE), 55 maps with grass
```

`PokedexOrder` at `$41024` is where `AGENTS.md` independently records it, which
is the corroboration that says the signatures are landing on the right tables.

So *"what beats Rock, and where do I get one"* is a query rather than a wiki
lookup:

```console
$ … --effective-against rock
super effective against ROCK: FIGHTING, GRASS, GROUND, WATER

$ … --catch-for rock --max-level 12
  TENTACOOL    POISON/WATER     map 0x1E   water lv 5-40   10/10 slots
  SANDSHREW    GROUND           Route 4    grass lv 6-12    4/10 slots
  GEODUDE      GROUND/ROCK      Mt Moon 1F grass lv 8-10    2/10 slots
  PARAS        BUG/GRASS        Mt Moon 1F grass lv 8-8     1/10 slots
```

**And the derived answer for Brock is a negative one worth having.** Every map
in that list is *past* Pewter City. On Blue specifically, Route 22's grass is
Rattata, Spearow and the two Nidoran — Red's Mankey is not there — and Viridian
Forest is Caterpie, Metapod, Kakuna, Weedle and Pikachu, none of which is
super-effective against Rock. So before Pewter there is **nowhere to catch
anything effective against Brock**, and the precondition has to be satisfied
earlier in the chain, at link 04, by which Poké Ball is taken: Squirtle is
Water and Bulbasaur is Grass.

That is a derivation, not a walkthrough: the type chart said which types, the
encounter tables said where, and the answer came back "not here". **Brock is
not in the chain yet**, so this is the query working, not the repair loop
closing on a real gate.

---

## 10. Heat maps

`corpus_report.py` draws where the corpus actually spent its time, over
[PixelGB](https://github.com/Alchemy86/PixelGB)'s map art, per map, from the
per-frame position track. (Parallel work worth crediting: Philo Groves has been
extracting Gen 1's ROM resources — text, images, UI, audio — and re-rendering
them in Rust *with the game's own logic*, which is the same idea from the other
end and a useful comparison whenever PixelGB's art is discussed.) Dwell is weighted by *frames*, not by steps, so a
corner the solver hesitates at is visibly hotter than one it walks through.

Written to `/home/aaron/pokemon-run/scenarios/` (game art, so not in the
repository):

```text
heat-06-cross-route-1-map0C.png    Route 1 — the climb, with the ledge corners hot
heat-06-cross-route-1-map00.png    Pallet Town
heat-06-cross-route-1-map28.png    Oak's Lab
```

---

## 11. The films

Two, because they cannot be one: an overview has to compress tens of emulated
minutes to be watchable, and a showcase has to *not* compress if the walk cycle
is to be visible. Both come out of the same compositor
([`tools/pewter-chain/swarm_film.py`](../../tools/pewter-chain/swarm_film.py)),
and both are in `/home/aaron/pokemon-run/training/`.

| film | what it is |
|---|---|
| `pewter-chain-01-03-600agents.mp4` | the three certified links in **one continuous cut**, 600 agents, 1280x720, 7.2 s at **12×, 60 fps** — the original swarm film's own compression. Placed on PixelGB's Kanto atlas. |
| `route1-showcase-600agents.mp4` | 600 agents crossing Route 1, **2× real time**, 83 s. The columns, the bottlenecks and the ledges are the picture. |
| `pewter-chain-swarm-24agents.mp4` | the whole fourteen-link chain at 52×, 60 s, clearly labelled a timelapse. |

```sh
python3 tools/pewter-chain/chain_tracks.py --out target/chain/tracks \
    01-new-game:target/cert/01 02-leave-the-house:target/cert/02 \
    03-oak-takes-you-to-the-lab:target/cert/03
python3 tools/pewter-chain/swarm_film.py target/chain/tracks \
    --out target/chain/frames --seconds 7.2 --fps 60 --cover 0.99 --width 1280 \
    --method "links 01/02/03 each certified 3000/3000, 3000 distinct trajectories, failure rate under 0.10%"
ffmpeg -y -framerate 60 -i target/chain/frames/f%05d.png \
    -c:v libopenh264 -b:v 8M -pix_fmt yuv420p pewter-chain-01-03-600agents.mp4
```

### Runtime and speed are not independent, and this chain is short

**`seconds_out × speed = the emulated time there is.`** That is the whole
constraint and it decides both numbers, so neither is a taste call. The three
certified links run 86.3 emulated seconds for 99% of agents (154 for the single
slowest), which gives **7.2 s at 12×** — a 50-second cut at 12× would need
**10 emulated minutes** and this chain does not contain them. The fourteen-link
chain's 52× timelapse is not a different aesthetic, it is a different amount of
material.

**So the film's runtime is not a pace decision and cannot be fixed by one.**
The original swarm film ran 50 s because it filmed ten emulated minutes, not
because it was paced differently — it used the same 12×. Measured side by side:

| | original dot film | this cut |
|---|---:|---:|
| compression | 12.05× | **12.05×** |
| stride, emulated frames per output frame | 24 | **12** |
| frame rate | 30 fps | **60 fps** |
| material filmed | 602 emulated s | **86.3 emulated s** |
| output | 1,500 frames, 50 s | **432 frames, 7.2 s** |

The original's stride of 24 is *longer* than a 17-frame walk step, so it
teleported by construction — it had no interpolation and did not need one,
because a dot has no walk cycle to contradict. Drawing the cartridge's own
sprite is what makes the stride matter, and 60 fps is what buys the same
compression back.

**`--cover` is the other half and it matters more than it looks.** The swarm is
essentially finished by frame 4,000: at 40% through a `--cover 1.0` timeline
only 50 of 600 agents are still running, and at 50% only 8. Covering the single
longest straggler therefore spends **60% of the runtime on fewer than ten
agents** — measured, and visible as a film that empties out and then keeps
playing. `--cover 0.99` ends shortly after the mass does.

Peter Whidden's *Training AI to Play Pokémon with Reinforcement Learning* is
where the swarm view comes from and the credit is his. What differs here is a
property of the substrate rather than a flourish:

- **The map art is photographed from the running cartridge** (PixelGB), not
  reconstructed from data files.
- **The agents are the game's own player sprite**, read out of OAM and VRAM
  with the hardware's own transparency rule (colour index 0), facing the way
  the game says they face.
- **The world is read out of PixelGB's atlas manifest**, and out of nothing
  else. An agent at `(map, x, y)` is drawn at `rect.x + x * tile_px`; that is
  the whole of the placement. Outdoor maps sit where the cartridge's own
  connection records put them and interiors are grouped into blocks beside
  their own area. That matters because **half the chain does not happen on a
  map**: the starter, the rival, the parcel and the Pokédex are all indoors,
  and a pure overworld view shows only the walking between them.
- **The whole thing is deterministic**, so the film regenerates from the seeds.

### The layout used to be a guess, and the guess is deleted

Before the atlas existed the compositor carried its own: five outdoor origins
transcribed from the map headers' `connection` lines, a table putting each
interior beside its own door, a gutter-packer, and a connector line drawn from
each panel back to the door it belonged to. It was an honest stand-in and it
looked like one — a floating panel, a void between the two halves of the world,
and threads across the gap.

All of it is gone. `WORLD`, `INTERIOR`, `PANEL_GAP` and `build_layout` are
deleted rather than kept as a fallback, because two ways to place a map is one
way too many, and the per-map short-name table went with them (the manifest
names all 226).

**The swap immediately found something the guess had been hiding.** Every one
of the 600 agents carries exactly one row reading `OAKS_LAB (12,11)` — and
(12,11) is Pallet Town's own `warp_event 12,11, OAKS_LAB, 2`, while Oak's Lab
is ten tiles wide. A warp changes `wCurMap` one frame before it changes
`wXCoord`/`wYCoord`, so on that frame a track names the destination map and the
*departure's* door tile. It is the same trap `chain_tracks.py::settle` handles
at boot, arriving mid-track instead of at the head. The old layout hid it
because the stray point landed on top of a neighbouring sheet; the atlas has
real gaps, so an agent stood in one. `Atlas.holds` drops any row whose
coordinate is not on the map it names, the count is printed, and the agent
holds its last real tile until the destination arrives — which is what a warp
is.

### The colour is the heat map, and it is a declared exception

The world stays grey — a Game Boy frame is four shades already, so nothing has
to be desaturated to get there — and the agents carry the colour. The hue is
**how trodden the tile under an agent's feet is**, weighted by frames, from the
same visitation count `corpus_report.py` draws its heat maps from. So the
frontier lights up on its own: an agent doing something genuinely new stands
apart from the crowd milling in well-worn ground, and the colour *is* the
count-based exploration signal an RL agent would be rewarded on.

That is **a declared exception to the categorical colour guard**, not a silent
one. The guard — no colour on screen that was not in the source framebuffer —
still applies to the map layer, which is reproduced exactly. The agent overlay
is deliberately outside it.

### What made it smooth, which was not the compression

Lowering the compression was the obvious answer and it was not the cause. The
compositor positioned agents from `wXCoord`/`wYCoord`, **and those do not move
during a walk** — measured from the tracks themselves, `wWalkCounter` runs
7,6,5,4,3,2,1 with each value held two frames, and the coordinate updates on
the frame it reaches 0. So an agent drawn from coordinates teleports tile to
tile *by construction*, at any compression and any frame rate.

Reading that sub-tile offset out of WRAM fixes it only at sampling rates fine
enough to catch it. **Interpolating between consecutive recorded tiles fixes it
at every rate** — it is the same four lines Peter Whidden's own map visualiser
uses (`pwhiddy/pokerl-map-viz`, `visualizer_live.js`), it needs no re-simulation
because the tracks are already on disk, and it is far less work. One refinement
on top: the progress fraction runs over the **last fourteen frames before the
arrival** rather than the whole gap between tiles, so an agent that stood still
for a thousand frames and then took one step does exactly that instead of
drifting the whole time.

Measured over 120 agents at the showcase's own sampling rate, before and after:

| | ≥12 px snaps | <4 px slides |
|---|---:|---:|
| positioned from the tile coordinate | 68% | 25% |
| interpolated between recorded tiles | **0.0%** | **100.0%** |

**The compositor measures this itself now, on every run, dry or not** — the
figure above was read off a screenshot once, which is exactly the kind of claim
that goes stale unnoticed. It prints the displacement distribution and the
camera's coverage before it renders a single frame.

**And the 14-frame window was wrong for the data.** It was reasoned from
`wWalkCounter` and is right about the cartridge; measured over the certified
chain, **no adjacent-tile arrival is ever that close together** — the gaps are
17 frames (34%), 27–31 (40%) and 52–55 (9%), because a track is sampled every
2–3 frames and the solver's own decision cadence sits between some steps. A
14-frame glide inside a 17-frame gap freezes the agent for 3 frames before
*every* step, and inside a 31-frame gap for 17. That is precisely the reported
"agents jump then pause", and it is a constant that was never checked against
the tracks it was applied to. `walk_span` measures the modal step time from the
tracks in hand — 17 here — and the glide never exceeds the gap.

**Interpolation is what lets the cut be fast *and* smooth, and that is
measurable rather than asserted.** A walking agent covers 16 px per step, so at
a timelapse factor X it covers about 1.9·X source pixels per output frame: at
1× that is 1.9 px and sub-4px steps are 99.7% of all motion; at 8× it is 15 px
**by construction**, and no amount of interpolation can make a fast film move
in small steps. What interpolation removes at speed is the *quantisation*:

| of the moving samples in the 8× cut | land exactly on a whole-tile multiple |
|---|---:|
| positioned from the recorded tile | **100.0%** |
| interpolated between recorded tiles | **0.0%** |

Without it an agent teleports 0, 16 or 32 px depending on how the stride lands
against the step; with it the phase is right and the motion is continuous. That
is the entire reason the compression can go back up.

**The compositor prints that figure itself now, on every run** — see
`report_motion`. It matters more than the `>=12 px` line beside it, and the two
must not be confused: the absolute threshold saturates above about 6× (a
walking agent clears 12 px by construction) and is a teleport detector for slow
cuts only, whereas the quantised fraction is scale-free and is what a
compression decision actually turns on. Teeth proven by neutralising
`interpolate` — 100.0% at both 8× and 12×, against 0.0% and 18.0% with it.

### A cut is smooth only while its stride is shorter than one walk step

This is the ceiling, it is arithmetic, and it decides the compression:

> `stride = speed × 59.7275 / fps` emulated frames per output frame, and
> interpolation can only place an agent *inside* a step while `stride ≤ span`.

`span` is the step time measured from the tracks — **17** emulated frames on
this chain — so at 30 fps nothing above **8.54×** can be smooth, and the 8× cut
was sitting just underneath it without that being said. Push to 12× at 30 fps
and each output frame swallows a whole step: **18.0%** of moving samples land
back on the tile grid, which is the teleporting the interpolation exists to
remove.

**The way past it is frame rate, not a slower film.** At 60 fps the same 12×
needs a stride of 12, comfortably inside a 17-frame step, and the ceiling rises
to 17.08×. Measured on the same tracks, 12×/60 fps beats 8×/30 fps on *every*
motion figure while being half as long again:

| | 8× at 30 fps | **12× at 60 fps** |
|---|---:|---:|
| stride, emulated frames per output frame | 16 | **12** |
| walking agent, source px per output frame | 15.1 | **11.3** |
| whole-tile quantisation | 0.0% | **0.0%** |
| `>=12 px` snaps | 20.8% | **1.25%** |
| `<4 px` slides | 62.8% | **67.2%** |
| camera coverage | 98.3% | **98.8%** |

On the shipped 12× cut, 65,001 moving samples of 91,082:

```text
agent displacement per output frame, in source pixels (a walk tile is 16):
  median 0.00  p90 11.29  p99 192.00  max 449.14
  >=12 px (a snap) 1.25%   <4 px (a slide) 67.17%
  whole-tile quantisation 0.0% of 65001 moving samples land on the tile grid
camera coverage: mean 98.8% of live agents on screen
```

The tail past 40 px is warps, and a warp is a jump — `interpolate` refuses to
lerp across one by design, and the quantisation figure excludes them so the
renderer is not credited for the cartridge's own teleports.

### Where an agent is drawn, measured against the cartridge

Reported as agents sitting left of a path's centre line, straddling its edge.
It was a real anchor bug and it was settled by measurement, not by reasoning
about OAM — [`tools/pewter-chain/measure_anchor.py`](../../tools/pewter-chain/measure_anchor.py)
reproduces it in one command.

The method needs only two things we already have. `terminalgb-mcp` gives a real
game screen (`screenshot`) and the player's exact tile (`observe`); PixelGB's
four-shade atlas is photographed from the same cartridge, so **its tile grid is
the game's and its shades are the game's four**. Sweep the hypothesis "the
player's tile top-left is at screen `(ax, ay)`" over the whole frame and count
disagreeing pixels in the overlap:

```text
screen ( 64, 64)     153 of 12288 px differ =  1.25%
screen ( 64, 56)    1297 of 11264 px differ = 11.51%
screen ( 64, 48)    1469 of 10240 px differ = 14.35%
```

One answer, by a 9.2× margin, and the 153 pixels that do differ are the player
— absent from the atlas. Their bounding box is x 65..78, **y 60..75**: sixteen
tall, four pixels *above* the 64..79 tile box. So an agent stands in the middle
of its tile and its sprite is lifted a quarter of a tile. The compositor was
returning the tile's *corner* and then centring the sprite on it, which is half
a tile up and to the left — exactly what was reported.

**Both halves of a wrong answer can look like one bug.** The same pass found
the camera genuinely lagging *and* the off-frame count overstating it, and the
anchor error was independent of the atlas swap and would have survived onto any
corrected placement.

**Reported a second time as "still over to the left a little" — and that one
was the atlas, not the anchor.** Re-run against the cartridge it reproduces to
the pixel: screen (64, 64), 1.25%, a 9.2× margin, player box x 65..78. Its
horizontal centre is 71.5 against the tile's own centre of 72, so the sprite is
**dead centre on its tile by measurement** and there is no leftward bias left to
find. What the second report was looking at is a stale atlas that bundled every
Pallet Town interior to the *left* of the town regardless of which side its door
is on. The measurement is also structurally independent of that: it compares one
game frame against one map's own art and never consults where the map is placed,
so a layout change cannot move it.

**And the sweep is allowed to fail now, because it quietly did not.** Pointed at
a save that continues into the Viridian Pokémon Center — whose NPCs are in the
frame and not in the atlas — every hypothesis differs by ~36%, the best of them
is arbitrary at a 1.0× margin, and it printed `SPRITE_LIFT = 0.0000` in the same
confident sentence as the real measurement. A sweep always has a winner and a
winner is not an answer: it refuses below a 3× margin or above 10% mismatch and
names the scene that works.

### The camera follows the majority, and says who it left behind

A link is not left behind until nearly every agent clears it, so the mass is
guaranteed to advance and the story is wherever the mass is — the certification
gate *is* the camera track, with no hand-authored shot list. It fits the
5th–95th percentile of agent positions rather than the full extent, so a
handful of stragglers cannot force a permanent zoom-out.

**And it must not hide them.** A persistent line says how many agents are off
frame and how far behind the furthest is, because a clean-looking crowd must
never be mistakable for a clean run.

**A steady shot of the wrong place is not a good shot.** The five smoothing
rules — a slow EMA, a deadzone, a quantised zoom, a pan cap and an axis bias —
are every one of them calibrated for *jitter*, and when the swarm genuinely
walks somewhere else they are calibrated for the wrong thing. Measured on this
chain: the picture was perfectly steady and **78.7%** covered, with stretches
of hundreds of frames where most of the swarm was outside the frame. A sixth
rule keys a catch-up on how far the 5th–95th box pokes past the view, which is
*exactly zero* whenever the swarm is already on screen — so it is provably a
no-op on every frame that was already right. Coverage 78.7% → **98.9%**, and
smoothness held: one direction reversal in seventy seconds, 81% of frames with
the camera perfectly still on X, three distinct zoom levels in the whole cut.

**And part of that 78.7% was the measurement, not the shot.** The off-frame
test compared an agent's *centre* against the boundary, so seventy-one agents
sitting 0.15 px outside the left edge — drawn, plainly, half in frame — were
reported missing. It is judged by the sprite now. Both halves were real: the
camera was lagging *and* the number overstated it.

---

## 12. Reproducing it

Nothing here needs anything that is not in the repository except a cartridge.

```sh
# every link validated, its cost ceiling and its evidence printed, no ROM needed
for f in scenarios/pewter/*.json; do
  cargo run --release --features scenario --example scenario -- check "$f"
done

# the whole chain from a cold cartridge, six seeds a link
cargo run --release --features scenario --example pewter_chain -- \
    chain --rom <blue.gb> --seeds 6 --out target/pewter-chain

# a corpus for one link, fanned out, with per-frame position tracks
cargo run --release --features scenario --example pewter_chain -- \
    corpus --rom <blue.gb> --scenario scenarios/pewter/06-cross-route-1.json \
    --start-dir target/pewter-chain/entry/05 --runs 400 --workers 8 --paths

# determinism, serially against in parallel
cargo run --release --features scenario --example pewter_chain -- \
    verify --rom <blue.gb> --scenario scenarios/pewter/06-cross-route-1.json \
    --start target/pewter-chain/states/05.gbss

python3 tools/pewter-chain/corpus_report.py <corpus-dir>
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --report
```

`GB_CHAIN_TRACE=1` prints one line per decision — map, position, stage, screen
kind, party, plan. Every finding in §8 came out of it.

**The save states are not in the repository.** A Game Boy save state is a dump
of RAM and VRAM, so it carries the cartridge's own tile data; every state in
the chain is derived by playing forward from a fresh cartridge, which is the
point of the chain. The corpus, the heat maps and the films live under
`/home/aaron/pokemon-run/`.

---

## 13. What is designed and not built

Written down because several of these were asked for, are good ideas, and do
not exist:

- **No learner.** No behavioural cloning, no policy, no weights, no cold
  evaluation of anything. The corpus is shaped for one — one row per decision,
  observation in, action out, replayable to reconstruct the observations — and
  that is as far as it goes here.
- **No screen-only student.** The agreed deliverable is a policy that takes
  pixels and emits buttons. The solver takes the observer's memory-derived
  snapshot, so nothing here is evidence about that student, and a number from
  one would not be comparable with a number from this.
- **`battle_advice` is still not wired**, so the battle engine's verified
  outcome distribution is not being used to choose moves. The solver picks the
  first move.
- **Potential-based shaping is not implemented.** The walkable graph that would
  supply Φ (real path distance to the trigger tile, not straight-line) exists;
  nothing consumes it.
- **The milestone/precondition graph is not built.** §9 is one repair, done by
  hand with the tables the tool provides. There is no representation that holds
  the game's progression as a graph over its event flags, and no automatic
  diagnosis.
- **Brock is not a link**, so the repair loop has not been closed on a real
  gate.
- **Entry tolerance is measurable and unmeasured.** The corpus runner takes a
  pool of entry states and the chain keeps every winning seed's exit state, so
  "how far off nominal can a link start and still succeed" is one command away.
  It has not been run.
- **No link has 3,000 cold attempts**, so no link's rate is certified against
  the bar.

## 14. Related pages

- [scenarios](../scenarios.md) — the goal/rules/knowledge machinery this is
  built on, and the recording format.
- [long runs and episodes](../longplay.md) — the episode primitive underneath.
- [the swarm view](../swarm-view.md) — the null model a trained agent has to
  beat, and where the film work continues.
- [Gen 1 mechanics](../gen1/README.md) — the memory map, battle control and the
  citation standard these goals follow.
- [as an agent environment](../agent-environment.md) — determinism, throughput
  and save states as episode resets.

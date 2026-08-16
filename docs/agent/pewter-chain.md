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
rate; it is what the chain did while it was being built. The per-link numbers
that mean something are in the corpus below, and even those are two orders of
magnitude short of the bar — see §7.

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

### 8.8 The forest, which is genuinely hard

Link 13 is the one that still fails, and it fails two ways:

- **the party faints** — three compulsory bug catchers, six Pokémon, top level
  9, no healing between them, and tall grass throwing wild encounters the whole
  way. Observed ending at (1,18) and (26,19), which are the third and second
  trainers.
- **it runs out of decisions** — 1,200 spent in the maze, observed ending at
  (6,30) and (6,22).

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
per-frame position track. Dwell is weighted by *frames*, not by steps, so a
corner the solver hesitates at is visibly hotter than one it walks through.

Written to `/home/aaron/pokemon-run/scenarios/` (game art, so not in the
repository):

```text
heat-06-cross-route-1-map0C.png    Route 1 — the climb, with the ledge corners hot
heat-06-cross-route-1-map00.png    Pallet Town
heat-06-cross-route-1-map28.png    Oak's Lab
```

---

## 11. Reproducing it

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

## 12. What is designed and not built

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

## 13. Related pages

- [scenarios](../scenarios.md) — the goal/rules/knowledge machinery this is
  built on, and the recording format.
- [long runs and episodes](../longplay.md) — the episode primitive underneath.
- [the swarm view](../swarm-view.md) — the null model a trained agent has to
  beat, and where the film work continues.
- [Gen 1 mechanics](../gen1/README.md) — the memory map, battle control and the
  citation standard these goals follow.
- [as an agent environment](../agent-environment.md) — determinism, throughput
  and save states as episode resets.

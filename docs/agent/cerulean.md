# Past Pewter, towards Misty — Route 3, Mt Moon and Cerulean City

The road east of Pewter City, cut into the same shape every other link of
[the Pewter chain](pewter-chain.md) has: a goal read out of the cartridge's own
memory, rules the host enforces, a scripted expert that reaches it, and a rate
measured over thousands of attempts rather than argued for.

> Read [`pewter-chain.md`](pewter-chain.md) first and [`brock.md`](brock.md)
> second. This page assumes both: the chain's certification gate, the battle
> policy, and the fact that the party was **one Pokémon** for the whole of the
> chain's life until §7 of this page.

```text
   Pewter PC ─18b─▶ the Mart ─19─▶ Route 3 ─20─▶ Route 4 ─21─▶ the nurse ─22─▶ Mt Moon B1F
                                                                                    │
                                                     Cerulean ◀─25─ Route 4 ◀─24─ B2F ◀─23─┘
```

- **Scenarios**: [`18b-shop-in-pewter.json`](../../scenarios/pewter/18b-shop-in-pewter.json),
  [`20-cross-route-3.json`](../../scenarios/pewter/20-cross-route-3.json),
  [`21-heal-at-mt-moon.json`](../../scenarios/pewter/21-heal-at-mt-moon.json),
  [`22-down-into-mt-moon.json`](../../scenarios/pewter/22-down-into-mt-moon.json),
  [`23-down-to-mt-moon-b2f.json`](../../scenarios/pewter/23-down-to-mt-moon-b2f.json),
  [`24-cross-mt-moon-b2f.json`](../../scenarios/pewter/24-cross-mt-moon-b2f.json),
  [`25-east-to-cerulean.json`](../../scenarios/pewter/25-east-to-cerulean.json).
- **Solver**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) —
  `ChainDriver::use_item`, `ChainDriver::want_item`, `ChainDriver::wild_wants`,
  `MT_MOON_1F_TO_B1F`, `MT_MOON_B2F_TO_FOSSIL`, `ROUTE_4_TO_CERULEAN`.

**`18b` is an insertion, not a renumbering**, and the reason is that link 19's
certification is a published measurement tied to that name. The chain reads
`scenarios/pewter/*.json` in sorted order and `18b` sorts between `18-` and
`19-`, so the shop goes in where it is needed without moving anything that has
already been measured under a name. `chain`/`film` number links from their own
file names for the same reason.

---

## 0. Certified

The chain's standing gate, run in order from the certified link-19 exit pool,
each link entering from the previous one's exits, `--idle-spread 3001`, every
attempt a distinct trajectory:

| link | reached the goal | 95 % CI | |
|---|---:|---|---|
| **18b-shop-in-pewter** | **3000/3000** | 100.0 [99.9-100.0] | five balls, five potions and five antidotes |
| **19-east-to-route-3** | **3000/3000** | 100.0 [99.9-100.0] | |
| **20-cross-route-3** | **3000/3000** | 100.0 [99.9-100.0] | was 2980/3000 — **certified now** |
| **21-heal-at-mt-moon** | **3000/3000** | 100.0 [99.9-100.0] | was 2993/3000 — **certified now** |
| **22-down-into-mt-moon** | **3000/3000** | 100.0 [99.9-100.0] | re-routed: the (5,5) ladder |
| **23-down-to-mt-moon-b2f** | **3000/3000** | 100.0 [99.9-100.0] | 6,345 balls thrown, **1,247 benches caught** |
| **24-cross-mt-moon-b2f** | **3000/3000** | 100.0 [99.9-100.0] | 213 blackouts, every one recovered |
| **25-east-to-cerulean** | **3000/3000** | 100.0 [99.9-100.0] | **Cerulean City** |

**Every link past the gym is certified**, and the two that were "accepted, not
certified" are not any more. Zero failures in 3,000 bounds the failure rate at
about 0.10 % by the rule of three, which is the 99.9 % claim the rest of the
chain carries.

**213 of link 24's 3,000 attempts blacked out and every one of them still
reached the goal**, along with 18 on link 22. That is §2's model working, and it
is the number that matters most on this page: a blackout costs time and nothing
else.

### The published figures for links 15 and 17 do not reproduce here, and it is not this change

Re-certifying the whole chain on this base gives **15-train-for-brock 2997/3000**
and **17-fight-through-the-gym 2701/3000 (90.0 %)** where
[`brock.md`](brock.md) publishes 3000/3000 and 2659/3000 (88.6 %). Both moved,
in opposite directions, and **this branch is provably not the cause**: built at
`HEAD` in a separate tree and replayed from the same entry pools, link 15's
first 150 trajectories and link 17's first 300 are **identical**, hash for hash,
and HEAD's own binary scores the same 270/300 on link 17. So the two figures
belong to a different pool or a different base than the one they were published
against. Recorded rather than quietly overwritten; settling which is a job for
whoever owns that arc.

The rest of the chain is unchanged and re-certified on this base: links 01-10,
12, 13, 14, 16, 18, 18b and 19 all **3000/3000**, link 11 **2999/3000** exactly
as published.

---

## 1. The failure this page opens on: POISON, and nothing was reading the byte

Links 20 and 21 were **accepted, not certified** at 99.3 % and 99.8 %, and
every one of their twenty-seven failures was the same thing: **the lead is
poisoned in a fight it cannot avoid and dies walking afterwards.** Not one was
a battle.

> **Two things came out of that and only one of them is the cure.** Carrying an
> antidote is worth doing and this section is how; but the deeper answer is
> §2's — *a blackout is not a failure at all*. Read them in that order, because
> §2 is what actually certified the two links.

### Reproduced before it was fixed

`GB_AGENT_LOG` is wired into `play_with` now, so one command replays a failing
seed and prints every request the driver was handed — which is the only place
the party's `status` field is visible from outside:

```sh
GB_AGENT_LOG=/tmp/a21.ndjson ./target/release/examples/pewter_chain solve \
    --rom <blue.gb> --scenario scenarios/pewter/20-cross-route-3.json \
    --start <link-19 exit 21>.gbss --seed 21 --out target/repro
```

Seed 21 of the certified link-19 pool, deterministically:

| decision | HP | status | where | |
|---:|---|---|---|---|
| 1 | 48/48 | — | (0,10) | enters Route 3 whole |
| 82 | 47/51 | **PSN** | (14,5) | poisoned in the YOUNGSTER's fight |
| 184 | 24/53 | PSN | (27,5) | still walking |
| 249 | 2/53 | PSN | (57,8) | four tiles from the exit |
| 250 | 0 | — | (59,3) | `party_hp=0` |

**168 decisions poisoned, with five POTIONs in the bag, and the driver never
looked.** Gen 1 poison costs 1 HP every four steps in the overworld and it
faints; `battle::decide` reaches for a healing item only when it is *doomed in
a battle*, and a wild encounter on Route 3 is refused before that rule is
reached, so the bag had never once been opened outside a fight.

### The fix is three things, and the third is the one that was missing twice

**(a) The lead's status is on the wire.** `PartyMon::status` was observed and
serialised already; `Snap` simply never parsed it. It does now.

**(b) An ANTIDOTE is bought.** `18b` buys five, at ¥100 each read out of
`ItemPrices` (file offset `0x4608`, three-byte BCD per item, ids 1-based)
against a POTION's ¥300 — and an antidote *cures* poison where a potion buys
twenty HP of the same problem. Row 3 of Pewter's own stock, bag slot 4, so the
goal names `watch:antidote_id=0x0B` as well as the count.

**(c) The bag can be opened in the overworld.** `ChainDriver::use_item` is a
state machine over five screens, every one of them decoded off a real cartridge
with the MCP server rather than from a guide:

| screen | says | answer |
|---|---|---|
| the overworld | nothing | START |
| the start menu | `POKéDEX POKéMON ITEM <name> SAVE OPTION EXIT` | walk the cursor to ITEM, A |
| the bag | the item names and their counts | walk to the line, A |
| the sub-menu | `USE  TOSS` | A on USE |
| the party list | `Use item on which POKéMON?` | A |

and then **B** out of all of them, because A on the message box does not leave
it. The whole cure is fifteen decisions; traced end to end on seed 3 of the new
pool, the lead is poisoned at decision 79, the driver notices on the first idle
overworld frame at 86, and by 96 the status is gone and the count has moved
5 → 4.

**Two facts about that machine are load-bearing.** The bag line is
`wListScrollOffset + wCurrentMenuItem` and **both halves are needed** — four
items make the list scroll, and the cursor on the fourth reads scroll 1, item
2. And the list does *not* reopen at the top (measured: it came back at scroll
1, item 2), so the direction to press is computed from where the cursor is
against where the item sits in the bag, never counted. Both bytes are new on
the wire (`screen.menu_index`, `screen.menu_scroll`) and both are in the atlas.

### Red → green, on the same six hundred seeds

| | link 20, 600 attempts from a 48-state pool |
|---|---|
| before | **590 / 600** — ten deaths, every one on Route 3, six of them at x=59 |
| after | **600 / 600** |

and in 96 crossings the bag is opened **9 times to cure and once to drink**, so
roughly one crossing in ten is poisoned and the cure fires on all of them.

### A drink is not a cure, and its rule took three attempts

The potion trigger is **"spend one only when all of it lands"** —
`max_hp - hp >= 20` — and that is a statement rather than a threshold. Two
thresholds were tried first and both measured as the dead band this chain has
now opened three times: at a third of maximum the Mt Moon B2F lead arrived at
30 of 55 and died at 22 with five POTIONs untouched, and at a half it arrived
at the **same** 30 of 55, because 30 is 54.5 % and the boundary is exactly
where a worn-down lead lives. Missing at least one potion's worth has no such
boundary. `refused_a_fight` — the shared policy's own threat-relative "too
hurt" — sits beside it, and the whole rule is gated on the leg having no nurse,
so the bag and the walk never compete for the same decision.

**Neither trigger can fire before the mart link, and that bounds the blast
radius of the whole change**: the bag holds nothing but the gym's own TM34
until `18b`, so on every link the chain had already certified `holds` answers
zero and `want_item` returns `None` before it looks at anything else.

---

## 2. A BLACKOUT IS A LEGAL DETOUR, NOT A LOSS

**The captain's correction, and it is a claim about the game rather than about
the solver**: the party faints, you wake at the last Pokémon Center whole, and
you walk back. That is what the cartridge does and what a person does. Scoring
it as a failed run was this chain asserting something the game does not — and
it was costing two links their certification for something no player would call
a mistake.

So, past the gym:

- **`party_hp=0` is off the failure list of every leg from 20 to 25.** The goal
  is still the destination and nothing else changed about how a link is judged.
- **The solver knows the way back.** `recover_route(scenario)` is a third route
  table beside the leg's own and the walk to the nurse, switched on when the
  party reaches zero and off again when the player is standing somewhere the
  leg routes over and the walk back does not.
- **What a blackout costs is time**, which is exactly what it costs a person.
  The call budgets carry it: link 20 goes 1,200 → 2,000, link 25 600 → 2,000.

### Where you wake is measured, not assumed

Traced on Mt Moon B2F, with `party_hp=0` off the failure list so the run keeps
going:

| decision | map | | |
|---:|---|---|---|
| 217 | B2F (11,18) | `hp=[0, 0]` | both party members are out |
| 219 | **Route 4 (11,6)** | `hp=[54, 23]` | awake, whole, one tile below the Mt Moon Pokémon Center's door at (11,5) |

`HandlePlayerBlackOut` heals the party and `SpecialWarpIn` puts the player on
`wLastBlackoutMap` at that Centre's own doorstep. Past Pewter the last heal is
either Pewter's (links 20 and 21) or Mt Moon's (everything inside the
mountain), so **there are exactly two places to wake up** and two walks back.

### The walks, and the one place a map is not enough

| leg | wakes at | walks back through |
|---|---|---|
| 20 | Pewter Centre | Pewter City → Route 3 |
| 21 | Pewter Centre | Pewter City → Route 3 → Route 4 |
| 22 | Route 4 (11,6) | **nothing** — Route 4 is already on its own route |
| 23, 24 | Route 4 (11,6) | into the mountain, 1F, B1F |
| 25 | Route 4 (11,6) | into the mountain and **all the way through it again** |

The switch back is by map, so the tables are kept disjoint wherever the
geography allows it — the leg's own route and the walk back never share a map,
and `ChainDriver::recovered` is one line. **Route 4 is the one place that
cannot work**: its western and eastern mouths are the same map and are not
connected to each other (x=19-23 is solid on row 5, x=20-23 on rows 6-8), so a
blackout east of the mountain really does cost the whole crossing again, and
link 25 names an `x` as well as a map. That is the cartridge's price, not the
solver's.

### And it certified both links outright

Sixty attempts a link from the same pool, blackouts allowed to be detours:

| link | 60 attempts, before | 60 attempts, after | certified, 3000 |
|---|---|---|---|
| 20-cross-route-3 | 590/600 (pre-fix) | **60/60** | **3000/3000** |
| 21-heal-at-mt-moon | 2993/3000 (pre-fix) | **60/60** | **3000/3000** |
| 22-down-into-mt-moon | 59/60 | **60/60** | **3000/3000**, 18 blackouts recovered |
| 23-down-to-mt-moon-b2f | 60/60 | **60/60** | **3000/3000** |
| 24-cross-mt-moon-b2f | 55/60 | **60/60** | **3000/3000**, **213 blackouts recovered** |
| 25-east-to-cerulean | 60/60 | **60/60** | **3000/3000** |

**213 of link 24's 3,000 attempts blacked out and all 3,000 reached the goal.**
Seven per cent of the crossings of Mt Moon B2F end with the party on the floor,
and under the old model every one of those was a failed run.

---

## 3. The road, read out of the cartridge

Route 3 does **not** run east into Mt Moon. Its own header says otherwise: map
`$0E`'s connection byte is `0b1010`, and its two blocks name map `$0F` **north**
and map `$02` west, in the cartridge's north/south/west/east order. Route 4
(`$0F`) then connects east to Cerulean (`$03`), and holds all three of the
mountain's doors on one row.

| crossing | yalign | xalign | so |
|---|---:|---:|---|
| Pewter east → Route 3 | 248 (−8) | 0 | Pewter (39,18) → Route 3 **(0,10)** |
| Route 3 north → Route 4 | 17 | 206 (−50) | Route 3 (59,0) → Route 4 **(9,17)** |
| Route 4 east → Cerulean | 8 | 0 | Route 4 (89,y) → Cerulean **(0,y+8)** |

**Route 4's three doors are (11,5) the Pokémon Center, (18,5) the mountain's
west mouth and (24,5) the mouth it puts you back out of.** x=20-23 is solid on
rows 6, 7 and 8 and x=19-23 on row 5, so the eastern mouth cannot be walked to
from the other two — which is also why **there is no heal between the Mt Moon
Pokémon Center and Cerulean**, and why the antidotes and potions link 18b buys
have to carry the whole mountain.

---

## 4. Mt Moon is one ladder, and the chain was taking a different one

**B1F is four pockets that do not connect on foot, and each 1F ladder lands in
a different one.** Walked over the warp tables of all four maps:

| 1F ladder | lands on B1F at | that pocket's way down | which reaches |
|---|---|---|---|
| **(5,5)** | (5,5) | (21,17) → B2F (21,17) | B2F's 452-tile pocket, and its (5,7) ladder to B1F (23,3) — **the exit** |
| (17,11) | (25,9) | (17,11) → B2F (25,9) | a 69-tile B2F pocket whose only warp is the one back |
| (25,15) | (25,15) | (13,27) → B2F (15,27) | a 94-tile pocket, likewise |

So from either of the other two the way out of the mountain is back up to 1F,
across it to (5,5), and down again — **and (17,11) is the ladder link 22 used
to take.** Its `map=0x3C` goal was perfectly satisfied; the next link simply had
nowhere to go. The route takes (5,5) now.

### A warp tile is not an obstacle a walkability grid knows about

The first version of the corrected 1F route ran along row 15 from x=21 to
x=31 — which passes **(25,15)**, the third ladder. Stepping on a warp is taking
it: **sixty of sixty attempts arrived on B1F at (25,15)**, in the wrong pocket,
with the link's own goal satisfied and the next link stuck against a wall from
its first decision. The route steps up to row 14 at x=24 to pass it, and the
search that draws these waypoint lists now blocks every warp on the map except
the one it is aiming at.

### One fight on 1F is forced, and it is not the trainer under the ladder

Measured the way Route 3's three were: block one trainer's cone on the
cartridge's own grid and ask whether the entrance still connects to the ladder.
Rows 13-15 wall off x=12 and x=13 and x=18 walls off rows 16-18, so the only
way west into the room the ladder is in is row 16 between x=13 and x=17 — which
is the **YOUNGSTER at (12,16)**'s line, since he faces right. Nobody else
severs it.

His body lands on row 16 (seen at (14,16) at range two, `TrainerWalkUpToPlayer`
puts him on (13,16)), so the route drops to **row 17** the moment the fight is
over — the fifth instance of the rule links 04, 05, 08 and 20 each paid for.
And the trainer at (5,6) faces **down** column 5 while the ladder at (5,5) is
the tile *above* him: climbing column 6 and stepping left reaches it without a
fight at all.

---

## 5. Taking a fossil is compulsory, and it is geometry rather than a side quest

Rows 2-4 of Mt Moon B2F hold the corridor to the (5,7) ladder — the only ladder
that reaches B1F's exit pocket — and the **only** opening between them and the
rest of the floor is (12,5)/(13,5): row 5 is solid from x=4 to x=11 and from
x=14 to x=19. The two tiles below that opening, (12,6) and (13,6), are the two
fossil objects.

A Gen 1 sprite is solid (`IsSpriteInFrontOfPlayer`), so with both standing
there is **no path at all** from the ladder at (21,17) to the ladder at (5,7);
with either one gone there is a 109-step one. Measured by blocking every object
tile on the floor and asking.

It is also, incidentally, why the cartridge takes the *other* fossil away when
you pick one up: a fossil left behind would wall the mountain.

The route therefore stands at (12,7), faces up and presses A — `Finish::Talk`,
the same verb the chain already uses on Oak and on the nurse.

**Two fights are forced on B2F and both are engaged from a tile the trainer is
already beside.** The super nerd at (12,8) faces right and column 13 is the
only climb from row 18 to row 7, so (13,8) is unavoidable — but he is already
adjacent to it and does not move. The rocket at (11,16) faces down column 11,
and rows 16-18 between x=7 and x=17 are the only link between the x=7-11
corridor and the x=12-17 climb, so his line cannot be avoided either; the route
meets him **along row 18**, at range two, where his body lands on (11,17) and
off the row. Climbing column 11 instead enters his cone at (11,24), eight tiles
down, and parks his body at (11,23) — in the middle of the corridor.

---

## 6. Route 4 east: two ledge jumps, and neither is optional

A breadth-first search over the cartridge's walkability grid with ledge steps
forbidden finds **no path** from (24,5) to Route 4's eastern edge. With them it
is 73 steps, jumping (44,6) → (46,6) and (79,8) → (79,10). Both are in the
direction of travel and both are waypointed on the tile before *and* the tile
after, so a `goto` can neither overshoot onto one nor be asked to climb back up
it — `LedgeTiles` (`06:66CF`) has no entry for facing up. Route 4's only
trainer, at (63,3) facing right, is two rows above the route and is never
engaged.

---

## 7. Gen 1's only answer to confusion is a bench

Link 24's residual is not the walk and not poison. Traced: the lead meets a
level-13 ZUBAT, `SUPERSONIC` lands, the screen reads `is confused!` and then
`It hurt itself in its confusion!`, and the self-hit costs about a fifth of the
bar a turn. **Nothing in Pewter's stock cures confusion** — POKE_BALL, POTION,
ESCAPE_ROPE, ANTIDOTE, BURN_HEAL, AWAKENING, PARLYZ_HEAL — and the cartridge's
only answer is to switch the confused Pokémon out.

Split by starter line over sixty attempts of that leg with one Pokémon:

| lead | reached the goal |
|---|---|
| CHARMELEON | 19 / 19 |
| WARTORTLE | 20 / 21 |
| **IVYSAUR** | **16 / 20** |

The same starter-dependent wall [`brock.md`](brock.md) measured, one gym later.

### The catch

`catch::decide` and `plugins/agent/src/catch.rs` had been built and measured
against the cartridge (251 throws, 61.0 % caught against 59.8 % predicted) and
**never once wired into a run**. Three facts had to reach the driver before it
could be: `wEnemyMonActualCatchRate` `$D007`, `wBattleType` `$D05A`, and each
party member's **internal** species index. All three are additive on the wire.

A throw is expressed as `Action::Item { id: ball }` — because that is what it
is, choose ITEM and choose the ball — so the battle menus needed no new case
at all. The one difference from a POTION is that a ball is thrown *at* the
opponent rather than used on somebody, so the party list never appears.

**ZUBAT is the answer to its own problem and is the cheapest catch in the
game**: `gen1_tables.py --where zubat` puts it in five of B1F's ten encounter
slots at levels 7-11 and its base catch rate is **255**, the maximum. PARAS is
on the list too, at one slot in ten, because `--plan misty` names it as the one
catchable thing on this road that answers *both* of Misty's Pokémon.

### A measured negative: fighting Mt Moon's wild battles buys nothing

`GB_FIGHT_WILD` is an experiment knob beside `train_to`'s own `GB_TRAIN_TO` —
a comma-separated list of legs that may fight wild battles — so "would training
here help?" can be asked without inventing a scenario to ask it in. Asked, with
`GB_TRAIN_TO=24`, over sixty attempts a link:

| | 22 | 23 | 24 | total |
|---|---|---|---|---|
| flee, as the chain does | 59/60 | 60/60 | 55/60 | 174/180 |
| fight everything | 58/60 | 60/60 | 57/60 | 175/180 |

One attempt apart in a hundred and eighty, and 84 overworld drinks against 54.
**Not separable, and it is not what closed these links.** Recorded so nobody
runs it again.

### It rides the walk rather than being a link of its own, and that is measured

A leg whose **goal** was `party>=2` was built first: it paces B1F's first
pocket until something is caught and scores **56 of 60**. The four failures
spend their whole call budget without ever throwing — `catch::decide` refuses a
throw that costs a turn the lead would not survive, and there is no nurse
inside the mountain to undo the attrition. Hung off link 23, a walk that was
already 60 of 60, the same catch costs nothing:

| | link 23 | link 24 |
|---|---|---|
| no ball thrown | 60 / 60 | 55 / 60 |
| **opportunistic catch** | **60 / 60**, 117 balls thrown and 26 benches landing | **57 / 60** |

Certified at 3,000: **6,345 balls thrown and 1,247 catches landing**, so
**41.6 % of attempts leave Mt Moon B1F with a second Pokémon at no cost to the
link that catches it**. The bench is worth about two attempts in sixty on the
crossing that follows — real, and not what closed that link. §2 is what closed
it, and the bench is what Misty will need (§9).

### Three menus that only exist once there is a bench, and each cost a run

The chain has carried one Pokémon for its whole life, so three screens had
never been seen:

1. **`Will BLUE change POKéMON?`** — Gen 1 offers a free switch whenever a
   *trainer's* Pokémon faints. The driver read it as the move list and pressed
   keys at YES/NO for ever: fifteen of sixty attempts spent 900 decisions in
   one battle without a single HP moving. B is NO.
2. **`Bring out which POKéMON?`** — the forced replacement after a faint, and a
   **third** wording for the party list beside `Choose a POKéMON.` and `Use
   item on which POKéMON?`. It is not a menu B can leave.
3. **An item is used on whoever is OUT**, not on row 0. With a fainted lead in
   slot 0 the driver offered it a POTION, the cartridge answered `It won't have
   any effect.`, and the two alternated for the rest of the run.

All three are the same class of defect: *a screen that cannot appear until a
capability exists is a screen nobody has tested.*

---

## 8. The test rig, on the three new links

Every new link was handed to [the rig](test-rig.md) beside the expert, from its
own entry pool, paired on the seed, `--idle-spread 3001`, twelve attempts an
arm:

| plan | link | teacher | `mash-a` | `random` | gap [goal] |
|---|---|---:|---:|---:|---|
| [`down-to-mt-moon-b2f`](../../scenarios/testrig/down-to-mt-moon-b2f.json) | 23 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]**, p = 0.0005 |
| [`cross-mt-moon-b2f`](../../scenarios/testrig/cross-mt-moon-b2f.json) | 24 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]**, p = 0.0005 |
| [`east-to-cerulean`](../../scenarios/testrig/east-to-cerulean.json) | 25 | **12/12** | 0/12 | 0/12 | **+100.0 [+43.4, +156.6]**, p = 0.0005 |

**How a zero was reached is worth more than the zero.** Every one of the
thirty-six floor attempts ends `out-of-calls`; `mash-a` never leaves the tile it
starts on, and `random` fights — 57 battles on link 23, 154 on link 24, 19 on
link 25 — and still never finds a ladder, a fossil or a map edge. The teacher's
decision counts are **49, 211 and 11** against ceilings of 900, 1,800 and 2,000,
so the budgets that carry a blackout are nowhere near binding on a run that does
not have one.

**There is still no trained player in this repository**, so every split is
`n/a` and these are a ceiling and a floor rather than a distillation result.

---

## 9. Misty, and what is still missing

Not measured — no run has fought her — but it is a table fact and it decides
what the next crew builds:

```sh
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan misty
```

| | |
|---|---|
| MISTY | STARYU 18 (WATER), STARMIE 21 (PSYCHIC/WATER) |
| beaten by | GRASS, ELECTRIC — and BUG against STARMIE |
| BULBASAUR line | **VINE WHIP from level 13**, 4× on STARMIE and 2× on STARYU |
| SQUIRTLE line | nothing — WATER is resisted by both |
| CHARMANDER line | nothing, ever: EMBER is FIRE, which WATER halves |

So two of the three starters need something they do not have, and the bench §7
now sometimes brings out of Mt Moon is where it comes from: **ZUBAT**'s LEECH
LIFE (BUG) answers STARMIE, and **PARAS** answers both. The capability is
built; what is missing is a leg that *guarantees* one rather than catching one
43 % of the time, and the level to make it useful.

---

## 10. Reproducing it

```sh
cargo build --release --features scenario --example pewter_chain

# the pool this page's numbers are measured from, link by link
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/pewter/24-cross-mt-moon-b2f.json \
    --start-dir <link-23 exits> --runs 3000 --workers 12 --exits 3000 \
    --out target/cert2/24-cross-mt-moon-b2f

# one failing seed, with every request the driver was handed
GB_AGENT_LOG=/tmp/a.ndjson ./target/release/examples/pewter_chain solve \
    --rom <blue.gb> --scenario scenarios/pewter/20-cross-route-3.json \
    --start <entry>.gbss --seed 21 --out target/repro

# what a pool of entry states actually holds, and any byte beside it
./target/release/examples/pewter_chain states --rom <blue.gb> \
    --start-dir <pool>/exits --peek 'antidotes@0xD325' --peek 'party@0xD163'

# the walkability grid, the trainer cones and the severance sweep
python3 tools/pewter-chain/route_audit.py --rom <blue.gb>
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan misty
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --where zubat
```

## 11. See also

* [`pewter-chain.md`](pewter-chain.md) — the chain, the gate, and the repair loop.
* [`brock.md`](brock.md) — the gym arc, and the starter that cannot pass it.
* [`battle-policy.md`](battle-policy.md) — the policy the catch sits beside.
* [`../gen1/catching.md`](../gen1/catching.md) — the capability §6 finally spends.
* [`test-rig.md`](test-rig.md) — the rig §8 runs.

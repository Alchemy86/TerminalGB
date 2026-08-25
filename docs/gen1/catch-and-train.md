# Choosing a bench: what beats a gym, where it is caught, and what it costs

> **The headline worked examples on this page — why a WATER gym wants a bench that resists
> water, and why PARAS beats it where PIKACHU cannot — moved to
> [AtlasGB's catching page](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/catching.md#a-worked-example-choosing-a-target)**,
> adapted and cited alongside the capture mechanics they sit beside. Everything below is the
> fuller comparative sweep this repository's own `duel.py` produces, and the BFS route-planning
> and menu-driving tooling that AtlasGB does not carry.

It is the companion to [`cerulean-gym.md`](cerulean-gym.md), which holds Misty's own
party and her gym's map.

**The rates our solver achieved with a bench are not here** — under
[`../where-things-live.md`](../where-things-live.md)'s rule those belong in
[AgentGB](https://github.com/Alchemy86/AgentGB), and that is where they are:
[`docs/agent/catch-and-train.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catch-and-train.md). A pointer stub stands at the old path
in this repository so existing links still resolve.

The instrument for everything below is
[`tools/gen1-tables/duel.py`](../../tools/gen1-tables/duel.py), which reads the
trainer's party, the base stats, the learnsets, the evolutions, the move table
and the type chart out of the cartridge you point it at and puts each candidate
through Gen 1's own `CalculateDamage`:

```sh
python3 tools/gen1-tables/duel.py --rom <blue.gb> --against misty --levels 24
```

---

## 2. Every candidate, at the level the chain trains to

`duel.py --against misty --levels 24`, worst roll, on this cartridge. "turns to
kill" is against whichever of Misty's two is hardest to kill; "turns to die" is
against whichever kills you quickest.

| catch | form at 24 | best move | turns to kill | turns to die | |
|---|---|---|---:|---:|---|
| **PARAS** | **PARASECT** | LEECH LIFE | **4** | **13** | **WIN** |
| BELLSPROUT | WEEPINBELL | WRAP | 15 | 9 | lose |
| VOLTORB | VOLTORB | SELFDESTRUCT | 5 | 4 | lose |
| **PIKACHU** | PIKACHU | THUNDERSHOCK | 4 | **3** | **lose** |
| MAGNEMITE | MAGNEMITE | TACKLE | 12 | 5 | lose |
| *BULBASAUR* | *IVYSAUR* | *VINE WHIP* | *3* | *9* | *WIN* |
| *SQUIRTLE* | *WARTORTLE* | *BITE* | *6* | *9* | *WIN* |
| *CHARMANDER* | *CHARMELEON* | *EMBER* | *12* | *3* | *lose* |

**The three italic rows are the calibration and they are why the rest can be
believed.** They are the starters, and the table's verdict on each is the one
three certified 3,000-attempt runs measured on the real cartridge: Ivysaur wins
(99.7 %), Wartortle wins at 24 and is a coin flip at 20 (99.0 % and 45.0 %),
Charmeleon loses at every level anyone can reach (0.0 % at 20, 24 and 30).
A ranking instrument that reproduces three known answers is worth using on the
unknown ones.

It models damage and nothing else — no critical hits, no status, no stat
stages, no PP, no 1-in-256 miss. `crates/terminalgb-gen1-battle` is the engine;
this is a chooser.

**A NARROW MARGIN IS NOT A PREDICTION.** The Misty calibration works because
its margins are wide — IVYSAUR 3 turns against 9, CHARMELEON 12 against 3 — and
a wide margin survives everything the tool leaves out. Point the same command
at Brock and it calls a level 16 CHARMELEON a win at **6 turns against 9**,
where the chain measures that line at 67 %: Brock's GEODUDE uses DEFENSE CURL,
which is a stat stage, and six turns is long enough for one critical hit to
decide it. Read a two- or three-turn gap as "too close to call" and go and
measure it.

**And a moveset has two honest answers, so the tool takes whichever is asked
for.** By default it uses `WriteMonMoves` — the last four the species has
learned by the swept level, oldest pushed out — which is what a wild one you
meet *at* that level holds. `--caught-at N` instead starts from what one caught
at N holds and walks it up through the level-up prompt, answered with
[`learn_table.py`](../../tools/gen1-tables/learn_table.py)'s criterion. **The
two are different Pokémon on paper and conflating them flatters the higher
levels.** The verdicts above are unchanged either way; §7's BELLSPROUT is not,
and that is the one place it matters.

## 5. What a hunt costs

PARAS is 10/256 encounter rate at 1/10 or 2/10 slots across Mt Moon's three floors
(`gen1_tables.py --encounters`, which reads `WildDataPointers`) — Mt Moon B2F `0x3D` carries
2/10 of its slots, twice either floor above it, at levels 10-12. So on B2F a PARAS is
`(10/256) × (2/10)` per step — **about one step in 128** — and B2F is also the floor the chain
already ends a link standing on.

**The pacing ground is `x` 20-28, `y` 12-14, and it is trainer-free.** B2F
declares five trainers: (12,8) facing right, (11,16) and (15,22) facing down,
(29,11) facing up, (29,17) facing left. Block every sight cone out to eight
tiles and run a breadth-first search from where the B1F ladder lands at (21,17)
over the cartridge's own walkability grid: 299 tiles are reachable, and rows
12, 13 and 14 all run clear from x=20 to x=28. Column 21 from y=17 up to y=14
is the walk on to it.

**A hunt is not a catch and needs one extra rule.**
`terminalgb_plugin_agent::catch::decide` declines a ball above two thirds
health and says `Weaken` instead, which is right when another encounter is a
few steps away and wrong when the encounter *is* the one you have paced 128
steps for. So a hunting leg fights the thing it is hunting and runs from
everything else. The threshold is the cartridge's own: through
`plugins/trainer/src/capture.rs`, a full-health PARAS at base catch rate 190 is
a 25 % Poké Ball and one at half health is 50 %.

## 6. Driving the party menu — two screens that look alike

The three cartridge facts a catch-and-train capability rests on (`wPartySpecies`'s seven-byte
layout, experience going to whatever was sent out, and a caught Pokémon's moves being a function
of species and level alone) are already stated, with citation, on
[AtlasGB's party page](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/party.md)
— they were redundant with it from the moment that page existed. What is left here is specific
to *driving* the party menu from outside, which AtlasGB has no reason to know about.

### The party menu has two screens that cannot be told apart

`STATS / SWITCH / CANCEL` is drawn **over** the party list it came from, so the
list's own rows are still on screen and only the word `SWITCH` says which
screen this is — the same trap `USE / TOSS` sets over the bag. And
`SwitchPartyMon` prints **nothing at all** on its second pass: the same twenty
rows with the same cursor. Which of the two identical lists is on screen is not
readable from the screen.

### A promotion's goal fires inside the menu

The byte a promotion is judged on is written by `SwitchPartyMon` while the
party menu is still up, so a leg whose goal is the party order ends with the
menu open and the next leg starts on that frame. Left to a generic text
handler that is a loop — A on a party list chooses, A in the overworld reopens
the start menu — and a real run spent all 4,000 of its decisions alternating
between the two without taking a step. The leg that follows a promotion has to
know how to put a menu away, which is the same thing `19-east-to-route-3`
already does for the shop counter link 18b's goal leaves open.

## 7. Ruled out, with the reason

* **BELLSPROUT** (Route 24 `0x23` and Route 25 `0x24`, levels 12-14, 3 of 10
  slots, base catch rate 255) arrives holding **VINE WHIP** — a real GRASS
  attack — and it is **out on geography alone**. That correction is worth
  stating because the first version of this page got it wrong: the table in §2
  puts a level 24 WEEPINBELL's best move at WRAP, which is true of one
  *generated* at 24 and false of one *caught at 12 and trained*. The prompt's
  criterion protects the last damaging move, so a trained one keeps VINE WHIP
  and then **wins at 21, 24 and 27** (`--caught-at 12`: 3 turns to kill against
  7, 9 and 11 to die). It is the better bench of the two on every measure
  except the one that decides it.
  Route 24's grass is a 2×14 band at `x` 4-5, `y` 18-31 and no trainer's sight
  cone touches it — but you cannot get to it. Rows 32-35, which are the whole
  of the corridor up from Cerulean, are walkable only at `x` 10-11 and 13-14;
  the grass column does not exist below row 31, and at row 31 `x` 6-9 is solid.
  So the only way in is north up the bridge and back down, and the bridge is
  the trainers. A breadth-first search from all four southern entry tiles that
  avoids every trainer body and every sight cone reaches **73 tiles and not one
  square of grass, at every sight range from 3 to 8**. Nugget Bridge's seven
  trainers are unavoidable, which is what `CLAUDE.md` already recorded from the
  other direction.
* **VOLTORB** and **MAGNEMITE** are ELECTRIC, so they carry Pikachu's defensive
  problem, and both are on maps far past Cerulean.
* **ZUBAT → GOLBAT** wins narrowly at 24 (5 turns to kill, 6 to die) and is
  five of B1F's ten slots at catch rate 255, so it is by far the cheapest
  catch in the mountain. It is not the bench of choice because the margin is
  one turn where PARASECT's is nine — but it is the right answer if what you
  want is *a* second Pokémon rather than one that beats Misty.
* **CATERPIE / METAPOD → BUTTERFREE** hold no damaging move worth the name past
  level 18: their own learnset replaces CONFUSION with SLEEP POWDER.
* **ABRA → KADABRA** wins from 27, and is 3 of 10 slots on Route 24 — behind
  the same bridge.

## 8. The same question, asked of Brock

Nothing above is about Misty except the numbers. `duel.py --against brock` runs
the identical derivation against a ROCK/GROUND gym, and
[AgentGB's `brock.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/brock.md)
records that the Charmander line cannot pass that one either. The answer there
is a different species and the same three capabilities.

## See also

* [AtlasGB: catching](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/catching.md#a-worked-example-choosing-a-target) —
  the capture odds disassembled out of `ItemUsePokeBall`, and the PARAS/PIKACHU worked example
  this page's own headline used to carry.
* [`catching.md`](catching.md) — this repository's own discriminator tooling and the
  wild/trainer sampling-rate analysis.
* [`cerulean-gym.md`](cerulean-gym.md) — Misty's party, her movesets, her gym's
  map, and the trainer that sees through a wall.
* [`learning-moves.md`](learning-moves.md) — the level-up prompt a trained
  bench will meet, enumerated.
* [AgentGB's `catch-and-train.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catch-and-train.md) — what our solver did with all of
  this: the certified rates, and the starter line that goes from 0 of 775 to
  794 of 795.

# Choosing a bench: what beats a gym, where it is caught, and what it costs

Everything on this page is a fact about **Pokémon Red and Blue**, read out of
the cartridge, and would be true with no agent in the world. It is the
companion to [`cerulean-gym.md`](cerulean-gym.md), which holds Misty's own
party and her gym's map.

**The rates our solver achieved with a bench are not here** — under
[`../where-things-live.md`](../where-things-live.md)'s rule those belong in
[AgentGB](https://github.com/Alchemy86/AgentGB), beside `misty.md` and
`brock.md`. They are at [`../agent/catch-and-train.md`](../agent/catch-and-train.md)
in this repository only because the worktree that measured them cannot write to
that one, and that page says so at the top.

The instrument for everything below is
[`tools/gen1-tables/duel.py`](../../tools/gen1-tables/duel.py), which reads the
trainer's party, the base stats, the learnsets, the evolutions, the move table
and the type chart out of the cartridge you point it at and puts each candidate
through Gen 1's own `CalculateDamage`:

```sh
python3 tools/gen1-tables/duel.py --rom <blue.gb> --against misty --levels 24
```

---

## 1. A bench that beats a WATER gym has to RESIST water, and that is the half everybody skips

`gen1_tables.py --plan misty` answers *"what type beats this?"* and it is not
enough, twice over.

**A species can carry the right type and hold no move of it.** PARAS is
BUG/GRASS and its level-up learnset — `EvosMovesPointerTable`, read at
`0x03B05C` on this cartridge — is `STUN SPORE` 13, `LEECH LIFE` 20, `SPORE` 27,
`SLASH` 34, `GROWTH` 41. **It never learns a damaging GRASS move at all.** Its
answer to a water gym is a 20-power BUG move, and by type alone you would never
know.

**And a species can hit for double and still lose.** What decides a Gen 1 gym
is as much what comes back. Misty's STARMIE lands a same-type WATER GUN;
`TypeEffects` gives WATER × GRASS = 0.5 and WATER × ELECTRIC = 1.0, so a grass
bench takes **half** what an electric one takes from the same attack. That one
multiplication is the difference between a bench that takes the badge and one
that faints.

Both halves together are why the starter that beats Misty is IVYSAUR, which is
GRASS/POISON — it hits for 2× *and* is hit for 0.5×.

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

## 3. PIKACHU, evaluated properly — and it loses

Pikachu is the obvious early catch and the case for it is a good one: it is in
Viridian Forest at levels 3-5 in 2 of 10 slots, which is *before* Brock, so it
levels through every fight on the road between the two gyms; it arrives already
holding **THUNDERSHOCK** (`BaseStats` offset 15, the four moves a species is
generated with), which is ELECTRIC and therefore 2× on both of Misty's; it has
base Speed 90; and it learns THUNDER WAVE at 9, which quarters an opponent's
speed. Nothing about that is wrong.

**It still loses, and the reason is defensive.** Sweeping the level:

| level | best move | turns to kill | turns to die |
|---:|---|---:|---:|
| 14 | THUNDERSHOCK | 6 | 2 |
| 18 | THUNDERSHOCK | 6 | 2 |
| 21 | THUNDERSHOCK | 4 | 3 |
| **24** | THUNDERSHOCK | **4** | **3** |
| 27 | SWIFT | 6 | 4 |
| 30 | SWIFT | 4 | 5 |

Two cartridge facts make that table what it is.

* **ELECTRIC does not resist WATER.** `TypeEffects` has no ELECTRIC row for a
  water attack, so Pikachu takes a full same-type WATER GUN from a STARMIE with
  base Special 100 — onto a Pokémon whose base HP is 35 and base Defence 30. It
  dies in two or three turns at every level up to 30.
* **Its offence does not scale, because its base Special is 50.** Doubling a
  40-power move against a base-85-Defence STARMIE is still four turns.

The level 27 and 30 rows carry a third fact worth knowing on its own:
`WriteMonMoves` gives a *caught* Pokémon the **last four moves its species has
learned by that level**, so a Pikachu caught at 27 has already pushed
THUNDERSHOCK out of its own move list in favour of SWIFT. A Pikachu *trained*
from level 5 keeps it, because the level-up prompt's criterion drops GROWL
first.

**So the sweep was run again the fair way, and the verdict does not move.**
`duel.py --against misty --caught-at 5 --candidates pikachu --levels 21,24,27`
keeps THUNDERSHOCK throughout and gives 4 turns to kill against 3 to die at 21
and again at 24, and 3 against 4 at 27 — a one-turn margin, at a level three
above the one the chain trains to and reached only by roughly half as much
grinding again. **Pikachu's problem is not its move list, it is that ELECTRIC
does not resist WATER**, and no moveset model changes that.

**So the honest answer to "is Pikachu a good early catch?" is: yes for the road
and no for this gym.** It is the best thing in Viridian Forest, it fights well
between Brock and Misty, and it cannot take the Cascade Badge at any level the
chain can reach.

## 4. PARAS, and why the answer is a level rather than a species

PARAS at level 21 loses (6 turns to kill, 5 to die). At 24 it wins by a mile.
Nothing about the species changed:

> `EvosMovesPointerTable` gives PARAS **`EVOLVE_LEVEL 24 → PARASECT`**.

PARASECT's base stats are 60/95/80/30/80 against PARAS' 35/70/55/25/55 — and
`MISTY_TRAIN_TO` was already 24, because that is where the Squirtle line learns
BITE. **The bench and the starter want the same threshold**, which is luck
rather than design, and it is why one training leg serves both.

The typing does the rest: BUG/GRASS takes `WATER × BUG = 1.0` then
`WATER × GRASS = 0.5`, so a STARMIE's best attack is halved; and LEECH LIFE is
BUG, which is 2× on STARMIE's PSYCHIC half. Thirteen turns to die against four
to kill is the widest margin any catchable Pokémon on this road has.

**What it does not have is a good move.** LEECH LIFE is 20 power. PARASECT wins
on defence and on base Attack 95, not on its moveset, and it would win by more
with MEGA DRAIN (TM21) — which is on no map the chain walks.

## 5. Where they are, and what a hunt costs

`gen1_tables.py --encounters`, which reads `WildDataPointers`:

| map | rate | PARAS slots | level |
|---|---:|---:|---:|
| Mt Moon 1F `0x3B` | 10/256 | 1/10 | 8 |
| Mt Moon B1F `0x3C` | 10/256 | 1/10 | 10 |
| **Mt Moon B2F `0x3D`** | 10/256 | **2/10** | **10-12** |

So on B2F a PARAS is `(10/256) × (2/10)` per step — **about one step in 128** —
and every other floor is half that. B2F is also the floor the chain already
ends a link standing on.

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

## 6. The three cartridge facts a catch-and-train capability rests on

1. **`wPartySpecies` at `$D164` is seven bytes** — one **internal** species
   index per slot, then `$FF` (AtlasGB `pokemon-rb`, `wPartySpecies`, len 7).
   Slot two is `$D165`. PARAS' internal index is **109** and its Pokédex number
   is 46; mixing the two is the classic Gen 1 mistake, and `gen1_tables.py`
   reads both out of the cartridge so neither has to be remembered.
2. **Experience goes to what was sent out, and Gen 1 sends out the first party
   member with any HP.** A caught Pokémon on the bench therefore cannot be
   trained at all — it has to lead first, and the cartridge's own way to say so
   is `SwitchPartyMon` (`engine/menus/party_menu.asm`), reached from START →
   POKéMON → A → `SWITCH`. Driving that menu writes nothing: every byte it
   touches it would have touched for a player.
3. **A caught Pokémon's moves are a function of its species and its level and
   of nothing else** (`WriteMonMoves`: the last four its species has learned by
   that level). So what a level 10 PARAS can do is knowable before a ball is
   thrown — it is a 40-power SCRATCH, and that is the whole of it.

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

* [`catching.md`](catching.md) — the capture odds, disassembled out of
  `ItemUsePokeBall`, and how the wild/trainer discriminator is read off the
  screen.
* [`cerulean-gym.md`](cerulean-gym.md) — Misty's party, her movesets, her gym's
  map, and the trainer that sees through a wall.
* [`learning-moves.md`](learning-moves.md) — the level-up prompt a trained
  bench will meet, enumerated.
* [`../agent/catch-and-train.md`](../agent/catch-and-train.md) — what our
  solver did with all of this, and the rates it reached.

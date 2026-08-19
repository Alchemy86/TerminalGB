# Cerulean Gym, and what the cartridge says about Misty

Everything about the second gym that is a fact about **Pokémon Blue** rather
than about anything we did with it: Misty's party and her movesets, the gym's
own map and its three trainers, the bits the badge sets, the move a Pokémon is
offered on the way there, and where the grass that gets it there is.

> **The other half is [AgentGB's `docs/agent/misty.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/misty.md)** — the
> rates our solver achieved, the starter line that cannot pass, and the links
> that carry it. [`docs/where-things-live.md`](../where-things-live.md) is the
> rule that splits them: this page would still be true with no agent in the
> world, and that one would not.

## 1. Misty

`TrainerDataPointers` (`0E:5D3B`, class 35 — `TrainerNames`' own 35th record is
`MISTY`), with the stats built the way the cartridge builds a trainer's Pokémon
(DVs `$98`/`$88`, no stat experience) and the movesets the way `WriteMonMoves`
builds them, from the base-stats four plus the learnset up to the level:

| | level | HP | ATK | DEF | SPD | SPC | moves |
|---|---:|---:|---:|---:|---:|---:|---|
| STARYU | 18 | 41 | 24 | 27 | 38 | 33 | TACKLE, WATER GUN |
| STARMIE | 21 | 59 | 40 | 44 | 56 | 50 | TACKLE, WATER GUN, HARDEN |

**Her whole offence is one 40-power special WATER move.** STARYU is WATER;
STARMIE is PSYCHIC/WATER, so it takes 2× from BUG as well — but **it learns
nothing at all by level**, so it carries no PSYCHIC move and its four are its
base-stats four.

What the type chart (`TypeEffects`, `0F:6474`) and the starter learnsets
(`EvosMovesPointerTable`, `0E:B05C`) make of that:

| line | its answer to WATER | first held at |
|---|---|---:|
| BULBASAUR | VINE WHIP, GRASS, 2× on both | level 13 |
| SQUIRTLE | none 2×, but it **resists** WATER — and BITE is 60 power against its next best 40 | level 24 |
| CHARMANDER | none, ever — EMBER is FIRE, which WATER halves, and the line is **2× weak** to WATER GUN | — |

## 2. The gym

Map `$41`, 10×14 walk tiles, two warps out at (4,13) and (5,13). Three objects,
and the pools are water:

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

**The junior trainer cannot be avoided, and that is geometry.** Misty's only
free neighbours are (4,3) and (5,2); (5,2) is reachable only from (5,3), because
row 1 is walled at x=4 and x=5; so every route to her crosses row 3, which is
her own line — and (1,3) is a wall, so there is no tile on that row to her left
to enter from. A breadth-first search over the cartridge's walkability grid with
row 3 blocked from x=3 to x=7 finds no path at all.

**She then parks on (4,3).** `TrainerWalkUpToPlayer` brings a beaten trainer to
the tile beside the player, and she engages from the west while the player is at
(5,3) — so her body lands on the one tile a route along row 3 still needed.
(5,2) is Misty's other neighbour and she can be talked to from it facing left.

### A GEN 1 TRAINER SEES THROUGH A WALL

The swimmer stands at (8,7) facing **left**, and (7,7) and (6,7) are both solid.
A sight line that terrain could block would never reach anything — the only
walkable ground beside him is column 9, to his *right*. **He engages anyway, at
(5,7): three tiles away with two walls in between.** Traced on the first attempt
at this gym: `Splash! … I'm first up! Let's do it!`.

So Gen 1's trainer detection compares coordinates and does not test the tiles in
between. That is worth knowing beyond this gym, because this project's severance
sweeps — the ones that establish which trainers on Route 3, in Viridian Forest
and on Mt Moon are unavoidable — block a cone's *tiles* and ask whether the map
still connects. Blocking is the right model for "may the player stand here"; it
is **not** a model of occlusion, and none of those sweeps assumed it was. The
correction is to any future reading that a wall between a trainer and a route
buys something.

## 3. What beating her writes

Snapshot before, play the gym, snapshot after, diff **every one of the 320
bytes** of `wEventFlags` (`$D747`) plus the badge bytes. **Exactly three move,
and nothing is cleared:**

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
`EVENT_GOT_POKEDEX` on `$D74B` bit 5 — three flags derived independently
elsewhere in this repository, and agreeing with them is what says the counting
is right.

**Pewter's gym also *cleared* two bits of `$D7EB`** (it rearms the Route 22
rival encounter). Cerulean's clears nothing.

## 4. The Pokémon Center restores PP, and that has a consequence

Measured directly — a party read before and after the counter, `$D188`-`$D18B`:
TACKLE `$14 → $23` (20 of 35 → 35), VINE WHIP `$00 → $0A` (0 of 10 → 10), and
HP full. **`HealParty` refills PP as well as HP.**

Which means "the party is whole" is a weaker statement than it looks, and a
program that decides it has finished healing on HP alone can walk away with an
empty move. VINE WHIP has **10 PP** and the gym above holds five water Pokémon.

## 5. The grass on Route 4, and how it compares

Route 4's grass is **sixty walkable tiles at x=64-73, y=10-15** on map `$0F` —
the tileset's own grass tile `$52`, read out of the block map. It is a closed
room with one door: walled at row 9 and row 16 and at x=62 and x=75, entered
along row 14 through x=75-77, with **column 77 the only tile joining rows 12 and
14 without a ledge** (row 13 is solid at x=75, 76, 78 and 79). No trainer stands
in it.

Experience per step, computed from `WildDataPointers` and `BaseStats`' own
base-experience byte:

| grass | map | mean exp/encounter | rate | exp per step |
|---|---|---:|---:|---:|
| Route 4, x=64-73 | `$0F` | 96.6 | 20/256 | **7.55** |
| Route 5 | `$10` | 138.3 | 15/256 | 8.10 |
| Route 24 | `$23` | 111.1 | 25/256 | **10.85** |
| Route 25 | `$24` | 108.1 | 15/256 | 6.33 |
| Route 3 | `$0E` | 54.0 | 20/256 | 4.22 |
| Mt Moon B2F | `$3D` | 98.7 | 10/256 | 3.86 |

The two richer patches each cost something Route 4's does not. **Route 5's grass
is behind a one-way ledge descent** — three patches at rows 4-5, 8-9 and 12-13
with `LedgeTiles` entries at rows 3, 7, 11 and 15, so each can be dropped into
and never climbed back out of. **Route 24's is on the far side of Nugget
Bridge**: five trainers in a column at x=10-11 plus a ROCKET at the top, on a
two-tile bridge.

## 6. The mart

Cerulean is mart 2 in the cartridge's own list (file offset `0x02442`):
POKE_BALL, POTION, **REPEL**, ANTIDOTE, BURN_HEAL, AWAKENING, PARLYZ_HEAL — the
same medicines Pewter sells with a REPEL where its ESCAPE_ROPE is, and no
SUPER_POTION (that is Vermilion's).

## 7. See also

* [`learning-moves.md`](learning-moves.md) — every level-up prompt the game can ask, and the rule for answering one.
* [`catching.md`](catching.md) — the odds, and the wild-versus-trainer signal.
* [`discoveries.md`](discoveries.md) — the index of what this campaign found in the cartridge.
* [AgentGB `docs/agent/misty.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/misty.md) — what our solver did with all of it.

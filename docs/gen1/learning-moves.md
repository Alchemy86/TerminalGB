# Learning moves, and evolving

When a Pokémon levels up already knowing four moves, the game stops and asks
which one to forget. It looks like an open-ended judgement — *is SLEEP POWDER
worth more than BITE, on this Pokémon, for the fights ahead?* — and it is not.

**There are 151 species and every level-up learnset is fixed data in the
cartridge.** The set of questions the game can ever ask is therefore finite,
enumerable and computable offline. This page counts it, applies one stated rule
to every entry, and measures the two things that decide whether a screen-only
student could ever execute the answer: what the prompt puts on screen, and
whether the answer depends on anything that is not there.

> **TMs and HMs are deliberately out of scope.** A TM is a separate and much
> larger problem — 55 machines, each usable on any compatible species at any
> level — and nothing on this page reads the TM tables except §6, which is a
> route question rather than a learning one.

Every number below came out of the retail Blue cartridge. The tools are
[`tools/gen1-tables/learn_table.py`](../../tools/gen1-tables/learn_table.py)
(the tables and the rule) and
[`examples/gen1_learn_drill.rs`](../../examples/gen1_learn_drill.rs) (what the
game puts on screen), and the commands are in §7.

---

## 1. The closed table

`EvosMovesPointerTable` sits at ROM offset `0x03B05C`, in bank `$0E`. It is
found by property and then verified by content — the only run of 190 in-bank
pointers whose BULBASAUR entry dereferences to `01 10 09`, *evolve at level 16
into internal index 9* — so no address is remembered. Each record is a
variable-length evolution list terminated by `$00`, then `(level, move)` pairs
terminated by `$00`.

**The trap is the index.** The table has **190 slots and they are indexed by
internal index, not by Pokédex number**; reading the first 151 gives a mix of
real species and MissingNo and undercounts. `PokedexOrder` is what filters it
to the 151.

| | measured |
|---|---:|
| level-up learn entries, all told | **728** |
| distinct moves ever offered | 133 |
| evolution entries | **72** — 52 by level, 16 by stone, 4 by trade |

Learns per species: `0:12 1:3 2:4 3:7 4:11 5:51 6:49 7:9 8:5` — **at most eight
on any one species**, and twelve species that never learn anything by levelling
at all. Starting moves per species, from each `BaseStats` record's own
`db MOVE1..MOVE4`: `1:31 2:53 3:40 4:27` — so **27 species already hold four
moves the moment they exist**.

**Two independent checks say the parse is right.** Gen 1 has exactly four trade
evolutions and exactly sixteen stone evolutions, and both fall straight out of
the count. The 728 and the whole distribution were also reproduced by a second,
independently written reader.

---

## 2. 728 is the ceiling on *events*, not on decisions

A learn event is only a *decision* once all four slots are full, and the first
four fill for free. So the real number needs the starting move set — and that is
where Gen 1 is unusually kind:

> **A caught Pokémon's four moves are a function of its species and its level
> and of nothing else.** Gen 1 builds a wild or trainer Pokémon's moves from the
> species' four base-stats moves and then applies its learnset up to the level,
> pushing the oldest out when the slots are full (`WriteMonMoves`,
> `engine/pokemon/add_mon.asm`).

That makes the *starting states* enumerable rather than open. Measured against
the cartridge, three wild Pokémon read back exactly what the model predicts:

| species | level | predicted | the cartridge |
|---|---:|---|---|
| TENTACOOL — eight learnset entries, five of them past full | 40 | WATER GUN, CONSTRICT, BARRIER, SCREECH | `[55, 132, 112, 103]` ✓ |
| NIDORINO | 40 | HORN ATTACK, POISON STING, FOCUS ENERGY, FURY ATTACK | `[30, 40, 116, 31]` ✓ |
| PIDGEOTTO | 30 | GUST, SAND-ATTACK, QUICK ATTACK, WHIRLWIND | `[16, 28, 98, 18]` ✓ |

So: 151 species × 100 possible levels × every stone or trade branch = **17,100
trajectories**, each walked to level 100 under the rule below, deduplicated on
what a player can actually see.

| | measured |
|---|---:|
| **distinct decisions — (species, the four held, the move offered)** | **840** |
| species that ever ask | 135 of 151 |
| on the 79 lines a player raises from level 1 and only presses A | 236 |

**840.** That is the whole of it, and it is small enough to be a lookup table
rather than a reasoning problem. It is also an over-count in one honest
direction: it allows species/level combinations no route offers — the earliest
decision it finds is a level 13 VICTREEBEL, which cannot exist, since a
VICTREEBEL comes from a WEEPINBELL that itself only appears at level 21.

### How often it actually arises on the route we play

Raising each species from level 1, the first four-move prompt lands at:

| species | first prompt |
|---|---|
| BULBASAUR | level 22 — POISONPOWDER, holding TACKLE, GROWL, LEECH SEED, VINE WHIP |
| CHARMANDER | level 24 — RAGE, holding SCRATCH, GROWL, EMBER, LEER |
| SQUIRTLE | level 24 — BITE, holding TACKLE, TAIL WHIP, BUBBLE, WATER GUN |
| CATERPIE (through BUTTERFREE) | level 16 — the earliest anything on the route reaches |
| PIKACHU | level 26 | 
| PIDGEY | level 31 |

The [Pewter chain](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/pewter-chain.md) carries one Pokémon and its
training leg's goal is `lead_level>=10`. **So the prompt never arises before
Pewter at all** — the first one a run of ours can meet is twenty-odd levels in,
and that is a very different training problem from one that occurs constantly.

---

## 3. The rule

Ours, not the cartridge's, so it is stated in full and a reader can disagree
with it. It is implemented once, in `learn_table.py`.

Rank the five moves — the four held plus the one offered — and drop the loser.
If the loser is the offered move, refuse it.

**0. A move whose damage is not in the table is never forgotten.** Grouping
every move whose power byte is 0 or 1 by its effect byte gives, from the
cartridge itself:

```
effect 38: GUILLOTINE, HORN DRILL, FISSURE                  one-hit knock-out
effect 40: SUPER FANG                                       halves the target
effect 41: SONICBOOM, SEISMIC TOSS, DRAGON RAGE,            fixed damage
           NIGHT SHADE, PSYWAVE
```

All seven carry a placeholder power of 1 (NIGHT SHADE's is 0) because the damage
is computed by the effect's own code. Ranking them on `power x accuracy` puts a
HORN DRILL below a POUND — the one systematic error available here — so the rule
declines to price them rather than risking it. *(An earlier version did not have
this clause and had PRIMEAPE forgetting SEISMIC TOSS.)*

**1. The last damaging move is never forgotten.** A Gen 1 Pokémon with no
damaging move cannot end a battle; it STRUGGLEs. Forgetting is permanent, so the
one catastrophe available at this prompt gets a rule of its own rather than
falling out of a number.

**2. A damaging move outranks a non-damaging one.** Measured rather than
assumed: [the battle policy](../agent/battle-policy.md) reports the certified
chain spending 2,450 turns on stat moves across 3,000 attempts of one trainer
battle and nothing at all on any other non-damaging move. Every other turn is
damage.

**3. Within the damaging moves, order by expected damage per turn** —
`power x accuracy/255 x STAB`, where power, type and accuracy are the
cartridge's own three bytes and STAB is the 1.5 `CalculateDamage` applies when
the move's type is one of the holder's. PP breaks a tie, then the move number,
so the rule is deterministic.

**4. Non-damaging moves are NOT ranked against each other**, and that is a
stated limitation rather than an oversight: a move's effect is an index into a
jump table of *code*, so there is no number in the table that says SLEEP POWDER
is worth more than GROWL. They tie on PP and then on move number.

What the rule deliberately does not use: the rest of the party, the fights
ahead, the opponent, and the move's effect. Those are the context cases, and
they are counted rather than folded in.

### What it answers, over all 840

| | count | share |
|---|---:|---:|
| **refuse** — the offer is worth less than all four held | 209 | 24.9% |
| **status-victim** — a move that deals no damage is dropped | 573 | 68.2% |
| **damage-victim** — a damaging move is dropped for a better one | 58 | 6.9% |
| ...**coverage-loss** — and it was the holder's last move of that type | **11** | **1.3%** |
| **close-damage** — the two worst are within 10% of each other | 5 | 0.6% |
| **status-tie** — the two worst both deal no damage | 551 | 65.6% |
| **unpriced-present** — a HORN DRILL class move is in play | 85 | 10.1% |
| **no-answer** — every held move is protected | 0 | 0.0% |

Two of those rows are the honest weaknesses and both are large enough to name.
**65.6% of decisions come down to two status moves the table cannot separate**,
where the rule falls through to PP, which is arbitrary. And 10.1% have one of
the seven unpriced moves somewhere in the five, where the rule protects rather
than judges. Neither is a *wrong* answer; both are the rule declining to have an
opinion, which is the right shape for a permanent, irreversible choice.

---

## 4. Where the answer really needs party context

The maintainer's case — *"do I already hold a water attack?"* — is real, and the
question is whether it is a small minority or not. It is measurable, because it
is exactly the class where the rule drops a damaging move that was the holder's
**last** of its type:

**11 of 840 = 1.3%.**

They are worth reading, because the shape of the trade is visible in each:

```
L43  NIDORAN     offered DOUBLE KICK  holding [BITE TACKLE SCRATCH POISON STING]
                                                       -> forget POISON STING
L46  PRIMEAPE    offered THRASH       holding [SCRATCH SEISMIC TOSS KARATE CHOP
                                               FURY SWIPES]  -> forget SEISMIC TOSS
L47  JYNX        offered THRASH       holding [POUND ICE PUNCH LICK BODY SLAM]
                                                       -> forget LICK
```

On each of those a second Pokémon already covering the lost type, or a route
with nothing that type is good against, would justify the swap outright, and a
route that needs it would not. **On the other 98.7% the party cannot change the
answer**, because the victim is a status move the holder has a spare of, or the
offer is simply worse than everything held.

So the maintainer's framing holds with a measured margin: this is a closed lookup
with a 1.3% tail that wants context. A student that got that 1.3% wrong would
still be right 98.7% of the time on a decision that occurs a couple of dozen
times in a whole playthrough.

---

## 5. What the prompt looks like, and whether it can be read

The same treatment [`catching.md`](catching.md) gave the wild-versus-trainer
pair: drive a real cartridge into the prompt and trace every frame, decoding
`wTileMap` as text. Three scenarios,
[`examples/gen1_learn_drill.preset`](../../examples/gen1_learn_drill.preset):

### The move-learn prompt

A level 12 BULBASAUR holding four moves, levelled to 13. The sequence, with the
frame window each phrase is on screen for:

| | frames | width |
|---|---|---:|
| `LEARNER grew to level 13!` | 1307..1464 | 158 |
| `LEARNER is trying to learn VINE WHIP!` | 1514..1541 | 28 |
| `... can't learn more than 4 moves!` | 1700..1728 | 29 |
| `Delete an older move to make room for VINE WHIP?` | 1780..1853 | 74 |
| `Which move should be forgotten?` + the move list | 1952..1992 | 41 |
| `1, 2 and... Poof! LEARNER forgot HYPER BEAM.` | 2066..2088 | 23 |

And this is the screen that decides the whole perception question — the last
frame the move menu is up:

```
|      POKé             |
|                       |
|      HYPER BEAM       |
|      GROWL            |
|      TACKLE           |
|      LEER             |
|                       |
| Which move should     |
| be forgotten?         |
```

**Four move names. No power, no type, no accuracy, no PP.** The offered move's
name is on screen two boxes earlier (`trying to learn / VINE WHIP!`), and the
species is *not* on screen at any point — the box uses the nickname.

### So: is it answerable from the screen alone?

Yes, and this is the measurement that says so. Of the 840 decisions there are
**581 distinct (four held, offered) pairs**, and the number of those pairs whose
right answer differs depending on which species is holding them is:

**zero.**

The species reaches the rule only through STAB, and no two species ever end up
in the same five-move situation with a different answer. **So a student needs
nothing the prompt does not already show it**: five move names in, one choice
out. The perception problem does not vanish — it still has to read five names
off a tile map — but it shrinks from reasoning to recognition, and 581 rows is a
lookup a network can simply memorise.

### The evolution prompt, and telling the two apart

A level 15 BULBASAUR levelled to 16:

| | frames | width |
|---|---|---:|
| `LEARNER grew to level 16!` | 1310..1464 | 155 |
| `What? LEARNER is evolving!` | 1526..2124 | **599** |
| `LEARNER evolved into IVYSAUR!` | 2171..2415 | 245 |

**They are not confusable.** The evolution announcement blanks the battle
furniture and holds one sentence for ten seconds; the move-learn is a run of six
short boxes over the intact battle screen. No phrase is shared. And a sampler
catches every one of them: measured as the fraction of sampling *phases* that
miss a window entirely, **every interval up to 24 frames catches every prompt
from every phase** — far more forgiving than the 9-frame wild-versus-trainer
line that has to be latched.

### When both happen at once, the evolution goes first

ABRA evolves at 16 and KADABRA learns CONFUSION at 16, which is one of only four
places in the cartridge where an evolution and a level-up move land on the same
level. Measured: `is evolving` (1574..2221), `evolved into` (2267..2511), and
*then* `is trying to` (2564..2597) through to `Poof!` (3122..3144). The party
afterwards is species 38 — KADABRA — holding CONFUSION.

Two things fall out of that, and the second is the one nobody would guess:

1. **The evolved form's move for the level it evolved on IS offered.** Evolving
   does not cost you that level's move.
2. **A single battle that crosses two levels loses the move in between.** A
   level 6 BULBASAUR fed one CHANSEY gains 182 experience and lands on level
   **8**; the game announces one level per battle and checks the learnset for
   the level it ended on, so level 7's LEECH SEED is never offered at all.
   Measured — it is what made the first version of this drill report no prompt.

---

## 6. Evolution: never cancel, and here is exactly what that costs

The game offers a B press to cancel an evolution. The position to establish was
that a student should never take it.

### Nothing is ever lost by evolving. 44 of 52 delay something.

Evolving keeps every move already held, so the only thing at stake is what is
still to come. Comparing each pre-evolution's learnset above the evolution level
against the evolved form's:

| | measured |
|---|---:|
| level evolutions | 52 |
| ...that **lose** a move the evolved form never learns | **0** |
| ...that **delay** one | **44** |
| the longest delay any of them costs | **11 levels** — SOLARBEAM, 54 → 65, IVYSAUR → VENUSAUR |

Typical shape: `BULBASAUR -> IVYSAUR at 16` delays POISONPOWDER 20→22, RAZOR
LEAF 27→30, GROWTH 34→38, SLEEP POWDER 41→46, SOLARBEAM 48→54. Against that you
get the evolved form's base stats immediately.

### And cancelling is not permanent — it is re-offered at the very next level-up

Measured, because "it will come back" is the half of the argument that decides
how bad a wrong B press is. A level 15 BULBASAUR levelled to 16 with B pulsed
through the evolution window:

```
What? LEARNER is evolving!          frames 1502..1880
Huh? LEARNER stopped evolving!      frames 1870..1880
    after pressing B: species 153 (BULBASAUR), level 16
```

then levelled once more, pressing nothing but A:

```
What? LEARNER is evolving!          frames 1550..2148
LEARNER evolved into IVYSAUR!       frames 2195..2439
    species 9 (IVYSAUR), level 17
```

**So the whole cost of cancelling once is one level of the evolved form's
stats.** Nothing is lost, nothing is closed off, and the choice returns
immediately.

### The rule, therefore

> **Never cancel an evolution.** A screen-only student must simply never press B
> on that prompt.

That is the simplest possible rule and it is worth stating plainly: it needs no
table, no lookup and no judgement — only the ability to recognise the
announcement, which §5 shows is a 599-frame window that shares no phrase with
anything else. Recognising it is the *whole* task.

The one case where a human might still disagree is the stone evolutions, and
they are not this prompt: a stone is something you choose to apply, not
something the game interrupts you to offer, so the "never press B" rule never
meets them. Sixteen of the seventy-two evolutions are stones and this page does
not rule on them.

**A measurement trap worth keeping**, because two versions of the drill got it
wrong and reported "cancelling does not work": B has to be **pulsed** through
the evolution animation, not tapped once and not held. Gen 1 reads some buttons
through a routine that only sees a transition, so a held button registers
exactly once — and a four-frame pulse aimed at the *text* is long gone by the
time the animation asks.

---

## 6a. A route idea, checked against the cartridge

The maintainer raised one: *a Magikarp is obtainable around Mt Moon, it evolves to
Gyarados at level 20, and a BubbleBeam TM is available there — so train it up
and teach it BubbleBeam.* Four claims, and they do not all survive.

| claim | verdict |
|---|---|
| Magikarp evolves into Gyarados at level 20 | **true** — `EvosMoves` gives MAGIKARP `01 14 16`: evolve at level 20 into internal index 22, GYARADOS |
| a Magikarp is obtainable around Mt Moon | **true, at ¥500** — see below |
| a BubbleBeam TM is available there | **false** — TM11 lies on **no map at all** and is in **no mart**; what Mt Moon holds is **TM12 WATER GUN** |
| Gyarados can learn it | **true** — and it can learn TM12 as well |

**The TMs that are actually in Mt Moon**, read out of each map's own object
list. Map `$3B` is `MT_MOON_1F` — verified by this repository's own gated
map-name table in [`plugins/agent/src/observe.rs`](../../plugins/agent/src/observe.rs)
— and `$3C`/`$3D` follow it in the same const list:

| map | items lying on it |
|---|---|
| MT_MOON_1F `$3B` | POTION, MOON STONE, **RARE CANDY**, ESCAPE ROPE, POTION, **TM12 WATER GUN** |
| MT_MOON_B1F `$3C` | *(nothing)* |
| MT_MOON_B2F `$3D` | HP UP, **TM01 MEGA PUNCH** |

Sweeping all 248 maps and every mart list, **TM11 BUBBLEBEAM appears in
neither**. The cartridge's own text for it reads *"TM11 teaches BUBBLEBEAM! Use
it on an aquatic POKéMON!"* and sits immediately before *"The CASCADEBADGE makes
all POKéMON up to L30 obey!"* — the Cerulean Gym guide's lines — so it is
Misty's gift, i.e. **after** Mt Moon rather than in it. *(Grade: the two texts'
adjacency in the text bank is strong but it is an inference; which script prints
them was not established here.)*

**Where a Magikarp comes from, and it is not the encounter tables.** Magikarp
appears in **no grass or water slot on any map** — `WildDataPointers` has none,
because Gen 1 keeps fishing encounters in a separate table. Two routes exist:

* **The Old Rod**, whose routine loads `ld bc, $0585` at ROM offset `0x00E252`
  in bank 3 — level 5, internal index `$85`, MAGIKARP, unconditionally. The Old
  Rod (item `$4C`) lies on no map's ground either, so it comes from a script this
  crew did not locate.
* **A man selling one for ¥500.** The text is in the cartridge at file offset
  `0x08A977`: *"MAN: Hello, there! Have I got a deal just for you! I'll let you
  have a swell MAGIKARP for just ¥500! What do you say?"* It sits in a run of
  Pokémon Centre chatter that includes *"TEAM ROCKET attacks CERULEAN
  citizens"*. *(Grade: the offer and the price are read out of the cartridge;
  the map that sells it and the level of the Magikarp were **not** established
  here.)*

**What it costs.** MAGIKARP is on the **slow** experience curve, so level 5 to
level 20 is **9,844 experience** — about 128 level-10 Zubat, or 81 level-10
Geodude. And it cannot earn any of it by fighting: its only move is SPLASH
(power 0) until TACKLE at 15, and **it can learn no TM or HM whatsoever** —
zero of the 55, checked against its `BaseStats` bitfield. So the whole 9,844 has
to come from switching it in and out of battles something else wins.

**What you get.** GYARADOS is WATER/FLYING, base experience 214 against
Magikarp's 20, and it learns BITE at 20 — offered the moment it evolves, per §5.
It can learn 23 machines including TM11 BUBBLEBEAM (65 power, 100%) and TM12
WATER GUN (40 power, 100%), which is the one Mt Moon actually holds.

**So the corrected route is:** the Magikarp and the level-20 Gyarados are real
and so is the TM compatibility; the TM in Mt Moon is WATER GUN, not BubbleBeam,
and BubbleBeam arrives with the Cascade Badge. Whether 9,844 experience of
switch-training is worth it against a party that can fight for itself is a
question this page does not answer — but the price is now a number rather than
an impression.

---

## 7. Reproducing everything on this page

```sh
# the closed table, the 840 decisions, the rule's answers, the student question
python3 tools/gen1-tables/learn_table.py --rom <blue.gb>
python3 tools/gen1-tables/learn_table.py --rom <blue.gb> --decisions
python3 tools/gen1-tables/learn_table.py --rom <blue.gb> --evolutions
python3 tools/gen1-tables/learn_table.py --rom <blue.gb> --species magikarp

# the map items, the TM table, the marts
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --tms
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --items-on 0x3B,0x3C,0x3D
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --marts

# what the game puts on screen, and what a cancelled evolution does
cargo run --release --features plugin-battle-control,plugin-trainer \
    --example gen1_learn_drill -- trace   --rom <blue.gb>
cargo run --release --features plugin-battle-control,plugin-trainer \
    --example gen1_learn_drill -- cancel  --rom <blue.gb>
cargo run --release --features plugin-battle-control,plugin-trainer \
    --example gen1_learn_drill -- moveset --rom <blue.gb>
```

`GB_LEARN_SCREENS=1` on the drill prints the whole 20x18 screen at each prompt;
`GB_LEARN_DUMP=1` prints every text box the battle produced.

**None of it is in CI.** The drill needs the player's own cartridge, the same
rule `tools/sameboy-oracle/` and `testharness/paperclaims.rs` follow, and the
Python tools need one too. The drill **starts a new game**, so it reads no save
file, and it stages the ROM into `target/` first because the MBC writes its
`.sav` next to whatever path it is handed.

### What the drill declares

It writes to WRAM twice, and both are declared rather than hidden. It snapshots
the party mid-battle and writes it back afterwards, because
[`BattlePlugin`](battle-control.md) restores everything it wrote when the battle
ends and would otherwise hand back the Pokémon *without* the level it just
gained; and it tops the party's PP up on the same pass, because a drill that
fights a dozen battles on one HYPER BEAM runs out of its 5 PP and parks the
battle menu for ever.

---

## See also

* [The level-up move drill](level-up-drill.md) — the rule on this page **played
  out on the cartridge**: every level-up on the Charmander and Bulbasaur lines
  triggered directly, the prompt answered or declined, the party read back as
  the verdict, and the whole thing recorded as training data.
* [Battling](battling.md) — what the battle screen shows and withholds.
* [Catching](catching.md) — the same treatment applied to the wild-versus-trainer
  question, and where the sampling-window method comes from.
* [The battle policy](../agent/battle-policy.md) — what to do on a turn, and the
  measured justification for rule 2 above.
* [Discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) — the reasoning trails, including the wrong
  turns.

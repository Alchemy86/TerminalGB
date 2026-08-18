# Past Pewter, towards Misty — Route 3, Route 4 and the mouth of Mt Moon

The road east of Pewter City, cut into the same shape every other link of
[the Pewter chain](pewter-chain.md) has: a goal read out of the cartridge's own
memory, rules the host enforces, a scripted expert that reaches it, and a rate
measured over thousands of attempts rather than argued for.

> Read [`pewter-chain.md`](pewter-chain.md) first and [`brock.md`](brock.md)
> second. This page assumes both: the chain's certification gate, the battle
> policy, and the fact that the party is **one Pokémon with an empty bag**.

```text
   Pewter PC ─18b─▶ the Mart ─19─▶ Route 3 ─20─▶ Route 4 ─21─▶ the nurse ─22─▶ Mt Moon B1F
                                                                                    │
                                                                                    └─ 23+ ─▶ B2F ─▶ Route 4 east ─▶ Cerulean
```

- **Scenarios**: [`scenarios/pewter/18b-shop-in-pewter.json`](../../scenarios/pewter/18b-shop-in-pewter.json),
  [`20-cross-route-3.json`](../../scenarios/pewter/20-cross-route-3.json),
  [`21-heal-at-mt-moon.json`](../../scenarios/pewter/21-heal-at-mt-moon.json),
  [`22-down-into-mt-moon.json`](../../scenarios/pewter/22-down-into-mt-moon.json).
- **Solver**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) —
  `PEWTER_TO_MART`, `ChainDriver::shop`, `ROUTE_3_CROSSING`, `ROUTE_4_TO_POKECENTER`.

**`18b` is an insertion, not a renumbering**, and the reason is that link 19's
certification is a published measurement tied to that name. The chain reads
`scenarios/pewter/*.json` in sorted order and `18b` sorts between `18-` and
`19-`, so the shop goes in where it is needed without moving anything that has
already been measured under a name.

---

## 0. Certified

The chain's standing gate, run in order from the certified link-18 exit pool,
each link entering from the previous one's exits, `--idle-spread 3001`, every
attempt a distinct trajectory:

| link | reached the goal | 95 % CI | |
|---|---:|---|---|
| **18b-shop-in-pewter** | **3000/3000** | 100.0 [99.9-100.0] | the first item the chain has ever bought |
| **19-east-to-route-3** | **3000/3000** | 100.0 [99.9-100.0] | re-certified; the shop moved where it starts |
| **20-cross-route-3** | **2980/3000** | **99.3 [99.0-99.6]** | **accepted, not certified** — §7 |
| **21-heal-at-mt-moon** | **2993/3000** | **99.8 [99.5-99.9]** | **accepted, not certified** — §7 |
| **22-down-into-mt-moon** | **3000/3000** | 100.0 [99.9-100.0] | |

**Three of the five clear the bar** — zero failures in 3,000 bounds the failure
rate at about 0.10 % by the rule of three, which is the 99.9 % claim the rest of
the chain carries. **Two do not, they are labelled that way permanently as link
11 is, and the cause is one thing: poison.** Twenty of link 20's twenty failures
and all seven of link 21's are deaths on the walk; **not one is a battle**.

---

## 1. The road, read out of the cartridge

Route 3 does **not** run east into Mt Moon. Its own header says otherwise: map
`$0E`'s connection byte is `0b1010`, and its two blocks name map `$0F` **north**
and map `$02` west, in the cartridge's north/south/west/east order. Route 4
(`$0F`) then connects east to Cerulean (`$03`), and holds all three of the
mountain's doors on one row.

The coordinate map comes from the same bytes, so where a link starts is derived
before it is observed:

| crossing | yalign | xalign | so |
|---|---:|---:|---|
| Pewter east → Route 3 | 248 (−8) | 0 | Pewter (39,18) → Route 3 **(0,10)** |
| Route 3 north → Route 4 | 17 | 206 (−50) | Route 3 (59,0) → Route 4 **(9,17)** |
| Route 4 south → Route 3 | 0 | 50 | the same statement from the other end |

Both were then measured on real exit pools — 600 link-19 exits at (0,10), 598
link-20 exits at (9,17) — and agreed.

**Route 4's three doors are (11,5) the Pokémon Center, (18,5) the mountain's
west mouth and (24,5) the mouth it puts you back out of.** x=20-23 is solid on
rows 6, 7 and 8, so the third is not reachable on foot from the first two:
Mt Moon is not a shortcut round Route 4, it is the only way east.

---

## 2. Link 20 — cross Route 3

**Goal `map=0x0F`.** Rules: `no_memory_writes`, `battle decide`,
`require party>=1`, `require badges>=1`.

### Three of the eight trainers cannot be avoided at any sight range

Measured the way Viridian Forest's one was — block a single trainer's facing
cone on the cartridge's own walkability grid, and ask whether the west edge
still connects to the north edge:

| trainer | at | faces | party | severs at range |
|---|---|---|---|---|
| BUG CATCHER 4 | (10,6) | right | CATERPIE 10, WEEDLE 10, CATERPIE 10 | **1** |
| YOUNGSTER 1 | (14,4) | down | RATTATA 11, EKANS 11 | **2** |
| BUG CATCHER 6 | (24,6) | right | CATERPIE 11, METAPOD 11 | **3** |

Blocking all eight cones severs the map at *every* range, so at least one fight
was compulsory before any of them was named individually.

The geometry behind it: rows 8-13 are cut into three pockets by the solid
columns at x=17 and x=23; row 7 is a wall whose only gaps are x=11, 27, 37, 49
and 59; and row 5 is walled at x=28-33, so the upper shelf cannot carry the
player past x=27 either. The ascent at x=11 is the only way out of the west
pocket and (10,6) faces straight along it. The descent at x=25-27 is the only
way back onto the floor and (24,6) faces along all three of its tiles.

### Three repairs, and every one of them was a measurement

**(a) A defeated trainer parks on row 4, and row 4 was the route.** The first
version crossed the shelf along row 4 and scored **0 of 60** — 56 of them
standing on (19,4) with 172 battles won, none lost and 80.3 % of the lead's HP
still there. The LASS at (23,4) faces left with a sight range of **4**, which
the cluster reads off directly: row 4 is walkable only from x=15 to x=22, she
sees x=19 to x=22, so a walker heading east is stopped at the westmost tile of
her cone and `TrainerWalkUpToPlayer` leaves her body on (20,4). Row 4 is then
sealed, and row 5 is already sealed at x=19 by the BUG CATCHER's own body — from
(19,4) there is no tile east at all.

The shelf's lower storey is the way through: (18,5) drops to row 6, whose
x=18-22 corridor passes *under* the BUG CATCHER at (19,5) — his cone is the
single tile (19,6), because row 7 is a wall beneath him — and climbs back to row
5 at (20,6)/(20,5), east of his body. **Every trainer the route engages is
engaged from a tile he is already adjacent to**, so none of them can park on
anything, and the LASS is never walked into.

*This is the fourth instance of one rule*: a waypoint a defeated trainer can
stand on is a waypoint that stops existing. Link 04's `(4,4)`, link 05's row 4
and link 08's Viridian ledge were the first three.

**(b) DISABLE.** See §4 — a gap in the shared battle policy, not in this link.
**0 of 60 → 45 of 60** once the route was fixed, and the remainder was this.

**(c) The one fight on the route that can be lost.** With the shelf fixed and
DISABLE handled, 56 of 60 — and **all four failures were faints on the same
tile**, (33,8), the LASS at (33,10) facing up. Her JIGGLYPUFF is level 14 with a
base HP of 115, so it takes three or four turns to remove while SING and DISABLE
take turns away, and Route 3 has no Pokémon Center to arrive at it whole from.

Rows 11 and 12 run clear from x=24 to x=37, her body blocks row 10 rather than
row 11, and column 37 climbs from row 11 to row 5 through the same gap — so the
detour is **shorter** than the row-8 crossing and fights nobody. The same trade
[link 17](brock.md) makes with the gym's junior trainer, measured the same way:
119 of 120 against 56 of 60.

### Measured

**2,980 of 3,000 = 99.3 % [99.0-99.6]**, 3,000 distinct trajectories, phase
spread 3001, from the certified link-19 exit pool. **Zero faints** in 14,984
battles; 79.6 % of the lead's HP left at the end of one; 94.8 % of 24,889
non-knockout damage predictions inside Gen 1's 217-255 roll range. Median 201
decisions, p99 300, against a ceiling of 1,200.

**It does not clear the chain's 99.9 % bar, and §7 is why — every one of the
twenty failures is a death on the walk, and none of them is in a battle.**

---

## 3. Link 21 — heal at the Mt Moon Pokémon Center

**Goal `watch:last_blackout_map=0x0F` AND `party_missing_hp=0` AND `map=0x44`**
— the same three-part claim [link 16](brock.md) makes, on the same building.

**2,993 of 3,000 = 99.8 % [99.5-99.9]**, and the seven failures are §7's cause
carried across the map edge: five die on Route 4's spur and **two die at the
counter itself**, on (3,3), poisoned with the nurse's dialogue already open.

The value `0x0F` is measured rather than assumed: `wLastBlackoutMap` ($D719)
reads `$02` (Pewter City) on every one of 598 link-20 exits and `$0F` (Route 4)
on every one of this link's own. So the byte holds the map the Centre was
entered *from*, not the Centre's own `$44` — exactly as Pewter's holds the town
and not `$3A`.

Route 4's southern spur is a one-tile-wide corridor with one jog: row 11 begins
at x=12, so a straight climb up column 10 is walled.

---

## 4. DISABLE — a gap in the shared battle policy, found here

**A move a JIGGLYPUFF has disabled is still in the move list, still has PP, and
selecting it costs no turn.** So a policy that re-picks it does not lose the
fight — *it never finishes it*. Measured: 11 of 60 attempts ran out of model
calls standing on one tile, the cartridge answering "The move is disabled!" six
hundred times in a row.

`Move::usable()` is now `pp > 0 && !disabled`, which is the whole fix: every
rule in [`battle::decide`](battle-policy.md) reasons through it, and the
fallback at the end of `decide` already had "nothing damaging is left with PP →
use whatever is left" waiting to be asked.

### The byte, and why the obvious one is wrong

`wPlayerDisabledMove` ($D06D) is **`(slot + 1) << 4 | turns_left`**, zero when
nothing is disabled. The nibble order is measured, not read off a macro — eight
stuck states give

| `$D06D` | `wPlayerDisabledMoveNumber` ($CCEE) | the move's slot |
|---|---|---:|
| `$38` | EMBER | 2 |
| `$18` | TACKLE | 0 |
| `$17` | TACKLE | 0 |
| `$21` | GROWL | 1 |
| `$37` | EMBER | 2 |
| `$13` | TACKLE | 0 |
| `$34` | EMBER | 2 |
| **`$00`** | **WATER GUN** | — |

— every high nibble is that move's own slot plus one, and the low nibble moves
while the slot does not.

**The eighth row is why `$CCEE` is not read instead.** It still held WATER GUN
with `$D06D` back at zero: the move *number* goes stale when DISABLE expires,
and only `$D06D` says whether anything is disabled at all. A policy keyed on the
move number alone would refuse a perfectly usable move for the rest of the run.

Pinned device-free by `battle::tests::a_disabled_move_is_never_chosen` (teeth
proven: with the flag cleared the same view picks the damaging move) and
`disable_and_empty_pp_are_both_unusable`.

**This is shared driver code, so under the chain's standing rule every link
already certified is re-certified against it.** Nothing before link 20 has ever
met a DISABLE — no opponent in links 01-19 carries the move — so the change
cannot alter their behaviour, but the rule is that it is re-run rather than
reasoned about.

---

## 5. The shop, and the bag the chain has never had

**Goal: `map=0x38` AND the bag holds five POKé BALLs AND five POTIONs.**

Two things wanted a bag and neither could have one:
[`catch::decide`](../gen1/catching.md) can only answer `Unavailable` without a
ball, and a poisoned lead on Route 3 has nothing to drink. The shop is one
purchase in a map the chain already walks past, with money it already carries:
¥2,500 of ¥4,651, at prices read out of `ItemPrices` (file offset `0x4608`)
rather than typed in.

**Pewter rather than Viridian, and that is the whole of why it is link 18b.**
`docs/gen1/catching.md` §1 proposed Viridian's mart, which the chain already
enters at link 07 — but inserting a link there re-certifies everything from 07
onwards. Pewter's mart carries POKé BALLs too (its stock is POKE_BALL, POTION,
ESCAPE_ROPE, ANTIDOTE, BURN_HEAL, AWAKENING, PARLYZ_HEAL, read with
`gen1_tables.py --marts`), and inserting there re-certifies three links.

**The bag was not empty when this started, and catching.md's table stopped one
link too early.** On all 600 link-18 exits `wNumBagItems` reads **1** and
`$D31E` holds `$EA` — TM34, which the gym gives with the badge and which link
17's own evidence already records as one of the three bits in its `$D755`
change. So the two purchases land in bag slots 2 and 3, and the goal names the
item **ids** as well as the counts rather than trusting a slot.

### Four things the counter taught, three of them the hard way

**The clerk is talked to across the counter.** Map `$38`'s tile (1,5) is `$1E`,
one of the tileset's counter tiles, so standing at (2,5) facing *left* puts a
counter in front of the player and Gen 1 looks one tile further for the sprite.
Standing next to the clerk himself is impossible: (1,5) is the counter and
(0,4)/(0,6) are wall.

**The generic text handler buys the shop out.** `advance_text` is A up to
twenty-four times, and every screen in a Gen 1 shop is a menu with something
selected — so the first trace of this link, written before there was a shop
handler at all, bought nine POKé BALLs by accident and was still going. The
counter is answered one button per decision, and the branch that does it sits
*before* the generic one.

**What has been bought is read out of the bag, never counted off the screen.**
Two attempts at counting confirmations both failed in the same direction: a
confirmation box is on screen across more than one decision and the money in
its corner is redrawn mid-box, so neither "the text changed" nor "the box was
not there last time" is one purchase. The second of them paid for five and five
and left the counter holding **nine and one**. The bag is what the goal is
judged on, so it is what the driver reads — which is why the bag is now in the
snapshot at all (§5a).

**Gen 1 puts the list cursor back on the first line after a purchase.** A
driver that remembers how many `down` presses it has made therefore keeps
buying the first thing on the list while believing it is buying the second —
measured, **twenty-one POKé BALLs and one POTION** where five and five were
asked for. The cursor is read every decision instead.

**And it is read off the `▶`, not off `screen.selected`.** `selected` cuts an
entry at the column where any other row of the screen starts a word, and a shop
screen is four boxes side by side — so "POKé BALL" comes back as "POKé" and no
stock name ever matches. Measured: twenty-three POKé BALLs and no POTIONs, the
cursor never once believed to be anywhere. The same rule
[`pewter-chain.md`](pewter-chain.md) records for `HORN ATTACK` in a move list,
met again in a wider screen.

### 5a. The bag is in the snapshot now

`game.bag` — `[{id, count}]`, every decision, not only inside a battle. **A
player that cannot see its own bag cannot decide to use anything in it**, and
until the chain bought something there was nothing in it to see. Buying at a
counter, curing a poisoned lead in the overworld and answering "do we hold a
ball" are all bag questions and none of them happens in a battle. Additive to
the wire, so nothing that ignores it changes.

### 5b. Measured

**3,000 of 3,000 = 100.0 % [99.9-100.0]**, 3,000 distinct trajectories. Every
exit state carries exactly five POKé BALLs and five POTIONs with ¥2,151 left,
and the link takes about 90 decisions.

**Link 19 was re-certified from the new pool and cost nothing: 3,000 of 3,000.**
Its route changed with the insertion — the Mart's door is east of the pocket
that made the old crossing three waypoints, so leaving it is one straight line
along row 18 — and the shop's own exit state opens inside the stock list, which
is why link 19 carries an empty shopping list of its own (§5's last paragraph).

---

## 6. Link 22 — into the mountain, and down

**Goal: `map=0x3C`** — B1F, not the cave mouth. Arriving on 1F is not a
milestone the geography treats as one: **which of the floor's three ladders is
taken decides the whole rest of the crossing.** B1F is four pockets that do not
connect to each other on foot, and each 1F ladder lands in a different one —
(5,5) → B1F (5,5), (17,11) → B1F (25,9), (25,15) → B1F (25,15). The exit from
the mountain, B1F's warp[7] at (27,3), is in a **fourth** pocket that no 1F
ladder reaches at all and that is entered only from B2F. So every crossing of
Mt Moon is 1F → B1F → B2F → back up to B1F, and the ladder at (25,15) is a dead
end for it: from the B2F entry its pocket leads to, (15,27), there is no path to
the exit ladder at (5,7) at all.

### A collision list alone invents a route through a cave

The first route went west along row 22 and up the one-tile corridor at x=9 —
every tile of which is `$05`, and `$05` is in tileset 17's own walkable list.
**53 of 60 attempts ended stuck within four tiles of it**, the solver pressing
`left` eight times into a wall.

Gen 1 keeps a *second* table beside each tileset's collision list.
`TilePairCollisionsLand` is three-byte records of (tileset, tile, tile),
terminated by `$FF`, naming pairs that are each walkable on their own and that
the game refuses to let you step **between** — `CheckForTilePairCollisions` in
`home/overworld.asm` is what reads it. Its very first record is

```
11 20 05     CAVERN, $20, $05
```

and the floor beside that corridor is `$20`. Found in this cartridge at file
offset `0xC7E` by that record's own bytes, which occur exactly once.

[`tools/pewter-chain/route_audit.py`](../../tools/pewter-chain/route_audit.py)
models it now, and **every cave and forest route drawn here needs it**: the
table has four CAVERN entries and eight FOREST ones, and this chain crosses
Viridian Forest. The corrected grid gives a completely different route — up the
x=20 corridor, east along row 10, back west along row 7 — and it is **3,000 of 3,000 = 100.0 %
[99.9-100.0]** — 60 of 60 on the first sample and certified without a repair.
Zero faints in 9,967 battles, 92.0 % of the lead's HP left at the end of one.

**It turns north at x=31 rather than x=30 on purpose.** The LASS at (30,4) faces
down, so her cone runs along column 30: a walker that climbs it is stopped
inside it and `TrainerWalkUpToPlayer` then parks her body on the tile above,
sealing the climb. One tile east and she is engaged from row 7 or row 10 instead
and parks on a column neither leg uses. The fourth instance of the rule §2
records, met before it could cost anything.

---

## 7. The residual is POISON, and the chain has no answer to it yet

Both of link 20's 600-attempt failures are the same thing, and neither is a
battle: **the lead is poisoned and dies walking**, at (55,10) and (59,2) — ten
and four tiles from the exit.

Gen 1 poison costs 1 HP every four steps in the overworld and it faints. Route 3
has no Pokémon Center, the bag is empty, and both WEEDLE-carrying trainers on
the route (BUG CATCHER 4 and BUG CATCHER 5) are forced by the geometry above, so
POISON STING gets three or four chances at the lead every crossing.

**A heal detour back to Pewter does not fix it, and the arithmetic says so
before a run does.** The trigger is half of maximum, ~26 of 53 HP; the walk back
is ~75 steps of Route 3 plus ~35 of Pewter, which is ~28 HP of poison. The
detour kills the thing it is trying to save. And once past x≈27 the *goal* is
nearer than Pewter anyway — the Mt Moon Pokémon Center is twenty steps beyond
the map edge.

So the cure is an item, and the chain had never bought one. **§5 is that door,
and it is open now** — but opening it is not the same as walking through it, and
the honest statement is that the potions are in the bag and **have never been
used**: `battle::decide` reaches for a healing item only when it is `doomed`
*in a battle*, and on Route 3 a wild encounter is refused before that rule is
reached while a trainer battle rarely gets there. Measured on 240 crossings with
a full bag: `items used 0`.

**Two named repairs remain, and neither is guesswork about where to look.**
Using an item in the *overworld* is the one that fits the failure — six menu
screens, all screen-readable, and an ANTIDOTE cures poison outright rather than
buying steps. Reaching for one *before* running from a wild encounter is the
cheaper half: on Route 3's 20/256 grass rate a poisoned lead meets something
within a few tiles almost every time, and the battle ITEM menu is where the
policy already points.

---

## 8. Misty needs a second Pokémon for one starter, and the cartridge says so

Not measured yet — no run has reached Cerulean — but it is a table fact rather
than a guess, and it decides where the shop link has to go, so it is written
down now:

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

The catchable answers on the road ahead are **PARAS** (Mt Moon B1F 1/10 at level
10, B2F 2/10 at 10-12 — BUG/GRASS, which answers both of Misty's) and **ZUBAT**
(LEECH LIFE, BUG, which answers STARMIE only). PIKACHU is the classic answer and
is in **Viridian Forest**, six links behind. BELLSPROUT is past Misty.

So the shape of the work is: a mart link that opens the bag, and a catch inside
Mt Moon. Both are ahead of link 20 in the chain, which is the other reason the
shop goes in before Route 3 rather than after it.

---

## 9. Reproducing it

```sh
cargo build --release --features scenario --example pewter_chain

# the pool this page's numbers are measured from
./target/release/examples/pewter_chain corpus --rom <blue.gb> \
    --scenario scenarios/pewter/20-cross-route-3.json \
    --start-dir <link-19 exits> --runs 600 --workers 14 --exits 600 \
    --out target/pool/20-cross-route-3

# what a pool of entry states actually holds, and any byte beside it
./target/release/examples/pewter_chain states --rom <blue.gb> \
    --start-dir <pool>/exits --peek 'blackout@0xD719' --peek 'x@0xD362'

# the walkability grid, the trainer cones and the severance sweep
python3 tools/pewter-chain/route_audit.py --rom <blue.gb>
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan misty
```

## 10. See also

* [`pewter-chain.md`](pewter-chain.md) — the chain, the gate, and the repair loop.
* [`brock.md`](brock.md) — the gym arc, and the starter that cannot pass it.
* [`battle-policy.md`](battle-policy.md) — the policy DISABLE was missing from.
* [`../gen1/catching.md`](../gen1/catching.md) — the capability the shop opens.

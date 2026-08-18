# Past Pewter, towards Misty — Route 3, Route 4 and the mouth of Mt Moon

The road east of Pewter City, cut into the same shape every other link of
[the Pewter chain](pewter-chain.md) has: a goal read out of the cartridge's own
memory, rules the host enforces, a scripted expert that reaches it, and a rate
measured over thousands of attempts rather than argued for.

> Read [`pewter-chain.md`](pewter-chain.md) first and [`brock.md`](brock.md)
> second. This page assumes both: the chain's certification gate, the battle
> policy, and the fact that the party is **one Pokémon with an empty bag**.

```text
   Pewter City ── 19 ──▶ Route 3 ── 20 ──▶ Route 4 ── 21 ──▶ the nurse
                                                 │
                                                 └── 22+ ──▶ Mt Moon ──▶ Cerulean
```

- **Scenarios**: [`scenarios/pewter/20-cross-route-3.json`](../../scenarios/pewter/20-cross-route-3.json),
  [`21-heal-at-mt-moon.json`](../../scenarios/pewter/21-heal-at-mt-moon.json).
- **Solver**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs), `ROUTE_3_CROSSING`
  and `ROUTE_4_TO_POKECENTER`.

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

**598 of 600** = **99.7 %** [98.8-99.9] over 600 attempts from the certified
link-19 exit pool, 600 distinct trajectories, phase spread 3001. Zero faints in
2,972 battles; 80.4 % of the lead's HP left at the end of one; 94.5 % of 4,956
non-knockout damage predictions inside Gen 1's 217-255 roll range. Median 201
decisions, p99 300, against a ceiling of 1,200.

**It does not clear the chain's 99.9 % bar, and §5 is why.**

---

## 3. Link 21 — heal at the Mt Moon Pokémon Center

**Goal `watch:last_blackout_map=0x0F` AND `party_missing_hp=0` AND `map=0x44`**
— the same three-part claim [link 16](brock.md) makes, on the same building.

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

## 5. The residual is POISON, and the chain has no answer to it yet

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

So the cure is an item, and the chain has never bought one. That is the same
door [`docs/gen1/catching.md`](../gen1/catching.md) §1 is waiting on — a purchase
in a mart the chain already walks past, with money it already has (¥4,476-5,326
at link 19, a POTION at ¥300 and a POKé BALL at ¥200) — which is why the shop is
the next link rather than a heal detour.

---

## 6. Misty needs a second Pokémon for one starter, and the cartridge says so

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

## 7. Reproducing it

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

## 8. See also

* [`pewter-chain.md`](pewter-chain.md) — the chain, the gate, and the repair loop.
* [`brock.md`](brock.md) — the gym arc, and the starter that cannot pass it.
* [`battle-policy.md`](battle-policy.md) — the policy DISABLE was missing from.
* [`../gen1/catching.md`](../gen1/catching.md) — the capability the shop opens.

# A bench, and what it is worth — catching, promoting and training a second Pokemon

> **THIS PAGE BELONGS IN [AgentGB](https://github.com/Alchemy86/AgentGB), NOT
> HERE, AND IT IS ONLY HERE BECAUSE IT WAS MEASURED IN A WORKTREE THAT CANNOT
> WRITE TO THAT REPOSITORY.** Everything below is a rate our solver achieved or
> a route it took — [`../where-things-live.md`](../where-things-live.md)'s test
> puts that in AgentGB, beside `pewter-chain.md`, `brock.md`, `cerulean.md` and
> `misty.md`. **Moving it is one commit there and a stub here**, and the stub
> belongs in `EXEMPT` in [`docs/tools/docs_index.py`](../tools/docs_index.py)
> with its reason, exactly as the others are.
>
> **The cartridge half is already split out and stays here**:
> [`docs/gen1/catch-and-train.md`](../gen1/catch-and-train.md) holds which
> species beats which gym and why, where each is catchable, and the three
> cartridge facts the capability rests on — all of which would be true with no
> agent in the world.

Until now every run of this chain carried **one Pokemon**, and one of the three
starter lines could not take the second badge with it:
[`misty.md`](misty.md) measured CHARMELEON at **0 of 775** against Misty, a
plateau rather than a curve, because FIRE is 2x weak to a STAB WATER GUN and
its own best answer is a 40-power SCRATCH at 1x. No reachable level fixes that.

This is the other lever: **catch something, put it in front, and train it.**

```text
   Mt Moon B2F ─23b─▶ hunt a PARAS ─ ... ─26b─▶ put it in front ─27─▶ train it to 24 ─29─▶ MISTY
```

- **Scenarios**: [`23b-hunt-a-paras.json`](../../scenarios/pewter/23b-hunt-a-paras.json),
  [`26b-lead-with-the-paras.json`](../../scenarios/pewter/26b-lead-with-the-paras.json).
- **Solver**: [`examples/pewter_chain.rs`](../../examples/pewter_chain.rs) —
  `catch_wants`, `hunting`, `lead_with`, `ChainDriver::reorder_party`,
  `ChainDriver::weakening_move`, `MT_MOON_B2F_TO_HUNT`.
- **Chooser**: [`tools/gen1-tables/duel.py`](../../tools/gen1-tables/duel.py).

---

## 0. What was built, and why it is three things and not one

The captain's framing is the one to keep: **a viable catch is good for every
run, not a rescue for one starter.** So the target species is a parameter of
each of three capabilities, and none of them knows what a PARAS is.

| capability | what it is | where |
|---|---|---|
| **hunt** | pace a patch, fight the species you want and run from everything else, throw balls at it | `hunting`, `catch_wants` |
| **promote** | put a party member in slot one, through the cartridge's own party menu | `lead_with`, `ChainDriver::reorder_party` |
| **train** | raise whatever is in slot one to a level | `train_to` — already existed |

**Promoting is a capability of its own because Gen 1 gives experience only to
what was sent out, and sends out the first party member with any HP.** A caught
Pokemon on the bench cannot be trained at all. It is a separate *leg* from the
hunt because the hunt's length is a geometric variable and the promotion is a
dozen decisions and deterministic: folded together, a promotion bug reads as a
hunt that never found anything.

**And where the promotion happens is a measured constraint.** Promoting inside
the mountain would hand Mt Moon B2F's two forced trainer battles — and Route
4's crossing after them — to a level 10-12 PARAS whose only move is a 40-power
SCRATCH. It waits until Cerulean, after the last fight the starter has to win
and immediately before the leg that trains.

### Adding a bench for the next gym is five edits

Nothing below names a species, and the recipe is the test of whether the
capability is general or PARAS-shaped:

1. `duel.py --against <leader>` says **what** to catch and **at what level** —
   and `gen1_tables.py --where <species>` and `--encounters <map>` say where and
   in how many slots.
2. One `Want` in `catch_wants`, one entry in `hunting`, one in `lead_with`.
3. A hunt scenario whose goal is `watch:slot2_species=<internal index>`, and a
   promotion scenario whose goal is `watch:lead_species=<the same>`.
4. A `Finish::Grind` box in `route_for` on trainer-free ground, found the way §5
   of [the cartridge page](../gen1/catch-and-train.md) finds this one.
5. `train_to` for the level `duel.py` named, and the *next* leg's `lead_with`
   entry so it closes the party menu the promotion's goal leaves open.

`ChainDriver::reorder_party` and `ChainDriver::weakening_move` know nothing
about any species at all.

## 1. Choosing the species is a cartridge read, and the obvious answer is wrong

The full derivation is [`../gen1/catch-and-train.md`](../gen1/catch-and-train.md).
The short of it, from `duel.py --against misty --levels 24`:

| catch | form at 24 | turns to kill | turns to die | |
|---|---|---:|---:|---|
| **PARAS** | **PARASECT** | 4 | 13 | **WIN** |
| PIKACHU | PIKACHU | 4 | 3 | lose |
| BELLSPROUT | WEEPINBELL | 15 | 9 | lose |
| ZUBAT | GOLBAT | 5 | 6 | wins, by one turn |

The BELLSPROUT row is the caught-at-24 moveset; one caught at 12 and *trained*
keeps VINE WHIP and beats Misty from 21 — it is out on **geography**, because
Route 24's grass is unreachable without Nugget Bridge's seven trainers. A
narrow margin like ZUBAT's is not a prediction either: the tool models damage
and nothing else. Both caveats are §2 and §7 of
[the cartridge page](../gen1/catch-and-train.md).

**PIKACHU was evaluated properly and it loses.** It is the best thing in
Viridian Forest, it is caught before Brock so it levels through the whole road
between the gyms, and it arrives already holding THUNDERSHOCK, which is 2x on
both of Misty's. None of that is wrong. What sinks it is defence: ELECTRIC does
not resist WATER, so a STARMIE with base Special 100 kills it in two or three
turns at every level up to 30, while its own base Special of 50 needs four.
**A bench that beats a water gym has to resist water, and that means GRASS.**

PARAS is the one grass thing on the road the chain already walks — and the
answer is a *level* rather than a species: it evolves into PARASECT at exactly
**24**, which is the level `MISTY_TRAIN_TO` already aimed at for the Squirtle
line's BITE. At 21 it loses; at 24 it wins by nine turns.

## 2. Certified

Run in the chain's own order, each link entering from the previous one's
**frozen, read-only** exit pool, `--idle-spread 3001`, every attempt a distinct
trajectory, **one binary throughout** (`sha256 6bedd631…`, logged by the run
itself). Links 01-22 are unchanged by this work and were rebuilt at 1,000
attempts each as the pool for it; they reproduce every published rate, and
seven of them replay hash for hash against the final binary — 09, 14, 17, 20,
22, 23 and 23b, 60 or 30 attempts apiece, **0 different**.

| link | reached the goal | 95 % CI | frames (mean) |
|---|---:|---|---:|
| 23-down-to-mt-moon-b2f | **1000/1000** | 99.6-100.0 | 1,985 |
| **23b-hunt-a-paras** | **982/1000 = 98.2 %** | 97.2-98.9 | 15,316 |
| 24-cross-mt-moon-b2f | **988/1000 = 98.8 %** | 97.9-99.3 | 29,883 |
| 25-east-to-cerulean | **1000/1000** | 99.6-100.0 | 2,537 |
| 26-heal-in-cerulean | **1000/1000** | 99.6-100.0 | 1,558 |
| **26b-lead-with-the-paras** | **1000/1000** | 99.6-100.0 | 754 |
| 27-train-for-misty | **500/500** | 99.2-100.0 | 754,313 |
| 28-heal-before-misty | **500/500** | 99.2-100.0 | 3,923 |
| **29-fight-through-cerulean-gym** | **2996/3000 = 99.9 %** | 99.7-99.9 | 27,445 |
| 30-heal-after-misty | **500/500** | 99.2-100.0 | 5,037 |

**Seven of the ten are 100 %, and the gym is 2996 of 3000.** Link 27 gets 500
attempts rather than 3,000 because it is ~750,000 frames each and it is not the
leg being quoted; that is stated rather than hidden, and it is what `$SPEC` on
[`build_pool.sh`](../../tools/pewter-chain/build_pool.sh) exists for. The two
legs under the chain's 99.9 % bar are §4's, each diagnosed to the byte or the
tile.

### The answer, split by the starter line the entry state carries

Read from **party slot two**, because once the bench leads the starter is no
longer at the front, with the internal indices taken off the cartridge
([`split_by_line.py`](../../tools/pewter-chain/split_by_line.py)).

| line | without a bench ([`misty.md`](misty.md)) | **with a bench** | 95 % CI |
|---|---:|---:|---|
| BULBASAUR → IVYSAUR | 1107/1110 = 99.7 % | **1090/1091 = 99.9 %** | 99.5-100.0 |
| SQUIRTLE → WARTORTLE | 1104/1115 = 99.0 % | **1112/1114 = 99.8 %** | 99.3-100.0 |
| **CHARMANDER → CHARMELEON** | **0/775 = 0.0 %** | **794/795 = 99.9 %** | 99.3-100.0 |
| **all three** | 2211/3000 = 73.7 % | **2996/3000 = 99.9 %** | 99.7-99.9 |

**The three lines converge, and that is the result.** With a PARASECT in front
the gym is fought by a PARASECT whichever starter is on the bench, so the
Cascade Badge stops being a fact about the starter. The Boulder Badge still is
— Brock's gym costs the Charmander line a third of its runs before it ever gets
here, which is why that column's denominator is 795 against the others' ~1,100.

**The four failures are not a line.** Two ran out of model calls (both
Squirtle-line) and two lost the fight (one Bulbasaur, one Charmander). At
99.9 % this is the fight's own variance, which is what `misty.md` says about
the three and eleven it had.

**Chain-level, the honest product**: 0.982 (a bench is caught) × 0.988 (the
mountain is crossed) × 0.999 (the badge) ≈ **96.9 %** of runs that reach Mt Moon
B2F leave Cerulean with the second badge, against 73.7 % before — and for the
Charmander line, against nothing at all.

## 3. What it costs

### The paired arm, and why it is a control rather than a comparison

`23-down-to-mt-moon-b2f` stopped catching so the hunt's goal could be exact, so
**its exits are a pool with no bench in them at all** — and running the rest of
the arc from there is the same chain, from the same upstream pool and the same
binary, played without a second Pokémon. Nothing is stubbed and no code is
changed, which is what makes it a control. 240 attempts a leg.

| leg | with a bench | **without one** |
|---|---:|---:|
| 24-cross-mt-moon-b2f | 988/1000 = 98.8 % | **240/240 = 100.0 %** |
| 25-east-to-cerulean | 1000/1000 | 240/240 |
| 26-heal-in-cerulean | 1000/1000 | 240/240 |
| 27-train-for-misty | 500/500, **754,313 frames** | 240/240, **102,540 frames** |
| 28-heal-before-misty | 500/500 | 240/240 |
| **29-fight-through-cerulean-gym** | **2996/3000 = 99.9 %** | **176/240 = 73.3 %** |

**The control reproduces [`misty.md`](misty.md) on a pool it never saw** —
73.3 % against its published 73.7 %, and per line BULBASAUR 88/88, SQUIRTLE
88/89, **CHARMANDER 0 of 63**. Two independent measurements agreeing is the
check; this is what makes the bench column a comparison rather than an
assertion.

**And it settles link 24's residual**: 240/240 without a bench against
988/1000 with one, on the same entry pool. Its 1.2 % is the hunt's hand-over
position and nothing else, which is what §4's reproduction already showed one
seed at a time.

### The bench costs seven times the training leg, not twice

| | frames (median) | decisions (median) |
|---|---:|---:|
| starter, level ~20 → 24 | 102,540 | 942 |
| **PARAS, level 10-12 → 24** | **754,313** | **6,816** |

**7.4× the frames, and the experience arithmetic predicts 2.1×.** On the
medium-fast curve a starter needs 13,824 − 8,000 = 5,824 and a PARAS caught on
B2F needs 13,824 − 1,331 ≈ 12,500. The missing factor of three and a half is
that **a low-level lead cannot collect experience efficiently**: it declines
more fights (`battle::decide`'s "too hurt to pick a fight" is threat-relative),
flees more of the ones it takes, and walks to the nurse more often. The
arithmetic prices the experience; it does not price being weak.

In wall-clock terms that is about **650,000 extra emulated frames**, roughly
three emulated hours, added to a leg that was already the longest in the arc.
It is the whole price of the capability — every other leg is within noise.

### What the two new legs themselves cost

Over 300 attempts each, with the whole run's output kept (`build_pool.sh` keeps
only the last four lines, which is the battle report and not the bag report, so
the hunt's own numbers have to be captured separately):

| | per attempt |
|---|---|
| **the hunt** | **1.72 Poké Balls and 2.06 chipping turns** (517 and 619 over 300), 15,316 frames mean, median 9,282 |
| **the promotion** | **14 decisions, 754 frames**, 300/300, 9.9 s of wall clock for all 300 |

**1.72 balls per catch is the ~50 %-a-throw estimate from the other side**, and
it is why five is not enough for the tail: the first ball works about three
times in five, and the leg is only out of the game when all five miss.

**The promotion is free.** It is fifteen decisions and deterministic, which is
the argument for it being a leg of its own rather than folded into the hunt —
its length is not a variable and the hunt's is.

*(`tally::PROMOTED` reads zero on 26b, and that is correct rather than a bug:
the goal fires on the swap, so the run ends before the driver backs out of the
menu and reaches the line that counts it. It is the same reason a catch is only
ever seen to land on a leg that keeps playing afterwards.)*

### One thing the invalid first control did prove

The first attempt ran link 27 from **26**'s exits rather than 23's, meaning the
party already held a PARAS on the bench — and `lead_with` names the species on
link 27 as well as 26b, so that arm promoted and trained the bench exactly like
the certified one. It measured the same thing twice: 240/240, medians within
0.5 % (757,716 frames against 754,313).

Which is worth keeping, because it is the belt-and-braces working: **a run
whose promotion never happened still gets one on the leg that needs it.**

## 4. Findings

### The hunt's 1.8 % is five balls spent, not a PARAS that never appeared

**Every one of the 18 failures ran to the 1,500-call cap**, at 162k-173k frames
where a success takes a median of **9,282 frames and 88 decisions** (p90 24,936
and 221; the longest success 78,744 and 681). A tail that long against a median
that short is a different distribution, so it was reproduced rather than
assumed — four failing seeds (57, 78, 110, 339), and every one reports

```
the bag, in the overworld: ... 5 ball(s) thrown; 2 turn(s) chipping something
   down to catch it
battles 92: won 12 (13.0%), lost 0 (0.0%), fled 80 (87.0%)
```

**And it is one PARAS, not five.** Exactly two chipping turns each: the leg
finds its PARAS, chips it twice to bring the odds over `catch::decide`'s
one-third floor, and then throws every ball it has at that same target at about
even money. Five coin flips is 3 %; 1.8 % is measured. Afterwards
`catch::decide` answers `Unavailable` — the capability does not exist any more
— so the walk carries on correctly and pointlessly until the budget runs out.

Two repairs, **neither taken here** for the same reason `misty.md` leaves link
26's own two alone: each re-opens the certification of every link after it.

1. **Buy more balls.** `18b-shop-in-pewter` buys five at ¥200 out of the ¥4,651
   the chain carries out of Brock's gym; ten would put five consecutive
   failures near one run in a thousand. It costs ¥1,000 and a re-certification
   of links 18b to 30.
2. **Spend turns instead of balls.** `catch::decide` throws as soon as the
   chance passes a third, which is the right trade when balls are plentiful and
   turns are not; on a hunt it is the other way round — five balls and thousands
   of spare decisions. Chipping to the floor `weakening_move` already computes
   takes a Poké Ball at catch rate 190 from about even money to about 85 %,
   i.e. five consecutive failures from roughly 1 in 33 to 1 in 13,000.

### Link 24's 1.2 % is a warp tile the walkability grid does not know about

The `(23,14)` waypoint took link 24 from 969 to **988 of 1000**; the twelve left
are eight distinct entry states, four failing on both their seeds, every one
oscillating on B1F at `(19..21,16)`. Reproduced:

```
d   1 (61, 21, 15)  Let the battle text finish.     <- B2F, mid-battle
d   9 (60, 21, 17)  Let the new map settle.         <- B1F, down the ladder
d  11 (60, 21, 16)  Walls in the way — go round.    <- and there it stays
```

**The hunt's own approach corridor is column 21, rows 15 and 16**, because the
ladder lands at (21,17) and that column is the only way north out of it. A wild
encounter on that corridor that ends in the catch leaves the leg's exit state
*mid-battle* a tile or two above the ladder; the battle resolves, the step
completes, and the player is back on B1F — in a pocket from which the leg's own
(27,3) is not reachable on foot at all.

**The named fix, not taken here**: approach east along row 17 to (23,17) and
north up column 23, and pace `(23,12)-(28,14)` instead of `(20,12)-(28,14)`.
That leaves column 21 unvisited above row 17. It changes 23b, so it re-opens
every link after it.

**A pool with gaps is not indexed by its file names**, and that cost a wrong
diagnosis first time: `--exits N` keeps the first N *successful* exits and names
each after its own seed, so entry index 661 is file `00670.gbss`. Looking one up
by name reproduces a different, passing run.

### A hunt must not knock out what it is trying to catch

**The most expensive thing to have missed, and it would have failed for exactly
the line the work exists to rescue.**
`terminalgb_plugin_agent::battle::decide` picks the move that ends the fight
soonest. That is right for every fight this chain has ever had and exactly
wrong for the one it is trying not to win.

A level 20 CHARMELEON's EMBER is FIRE against BUG/GRASS, so it is 2× twice
over, and through `CalculateDamage` against a level 11 PARAS' 30 HP it does
**112**. Reproduced end to end from a real CHARMELEON entry state:

| | caught a PARAS | decisions spent |
|---|---:|---:|
| before `weakening_move` | **0 of 1** | 1,500 — the whole budget |
| after | **3 of 3** | 30, 58, 95 |

One ball and two chipping turns each. The rule is: among the moves that are
usable and do damage, take the one that hurts most while its **best** roll
still leaves the target standing; if every move can knock it out, take the
gentlest and accept the risk. `max` rather than `expected`, because a knockout
is not a tail risk to be traded off — it is the encounter gone, and on B2F that
is another 128 steps.

**Nothing measured it before because the chain had only ever hunted with a
WARTORTLE**, whose WATER GUN is *halved* by the same typing. The general
lesson: a capability tested from one entry state is tested on one game.

### Two defects the certification found that nothing smaller would have

Both were invisible at the scale everything else was tested at — three seeds
and a hand-driven chain — and both were diagnosed by **reproducing one failing
seed** with `GB_AGENT_LOG` rather than by reading code.

**THE PARTY MENU DOES NOT SCROLL, AND `wListScrollOffset` IS A TRAP ON IT.** A
party is at most six long and Gen 1 draws all of it — but it does not *reset*
that byte either, so the party menu inherits whatever the last scrolling list
left. **33 of 1,000 promotions failed, every one on a run whose bag had
scrolled on the way to an ANTIDOTE**, and the cursor then oscillated between
rows 0 and 1 answering "back up the party" to both, because `scroll + index` was
never the row. `use_item` must keep adding the two; a bag genuinely scrolls.
**967/1000 → 1000/1000.**

**CHANGING WHERE A LEG HANDS OVER CHANGES EVERY GREEDY PATH THE NEXT LEG
TAKES.** Link 23 used to hand link 24 a player standing *on* the B2F ladder, so
its walk east along row 17 could not re-enter it. A hunt hands over from
anywhere in its pacing box, and `Action::GoTo` is greedy on the larger remaining
delta — so from (20,12) it walked down column 20 to row 17 and stepped straight
back up. **969/1000 → 988/1000** with one waypoint. The residual is the same
tile and is above.

### A promotion's goal fires with the menu still open

The byte a promotion is judged on is written by `SwitchPartyMon` from inside
the party menu, so link 26b's exit state has that menu up and link 27 starts on
that frame. Left to the generic text handler it is a loop — A on a party list
chooses, A in the overworld reopens the start menu — and the first real run
**spent all 4,000 of its decisions alternating between the two without taking a
step.** `lead_with` therefore names the species on the leg *after* the
promotion as well, which makes `reorder_party` see the swap already done and
back out. It is the same thing `shopping_list` does for `19-east-to-route-3`,
whose own start state is inside the shop counter link 18b left open.

### Link 27's exit state is a level-24 PARAS mid-evolution

The goal fires the instant the level ticks over, which is inside the battle and
before the evolution animation — so link 27 hands over a **PARAS** at level 24
and the form becomes PARASECT during link 28. Read out of each link's own exit
state rather than assumed. A leg that pressed **B** through that animation
would cancel it; every leg here presses A.

### Link 23 stopped catching, so the hunt's goal could be exact

**A goal satisfiable at entry measures nothing** — [`misty.md`](misty.md)'s own
lesson about link 28. Link 23 used to catch a ZUBAT or a PARAS opportunistically,
so a hunt after it could begin with a party of two and `party>=2` would fire on
arrival. `catch_wants` for link 23 is empty now, the hunt is the only thing in
the mountain that throws a ball, and `watch:slot2_species=109` is exact.

Nothing was lost by it. The ZUBAT existed to give the SUPERSONIC-confusion
switch a bench; a hunted PARAS **is** that bench and is also the thing that
beats Misty — and every ball spent on a ZUBAT is one the hunt does not have.

### What is not measured, and why

**PIKACHU's verdict is `duel.py`'s, not a chain measurement.** Pikachu lives in
Viridian Forest, thirteen links before the gym and before the chain has any
Poké Balls at all, so measuring it through the chain means a ball-buying link in
Viridian and re-certifying every link from 12 to 30 — a bigger project than this
one, and one whose answer the cartridge's own damage formula already gives with
three certified calibration points. Said plainly rather than implied.

**No emulator run pits a Pikachu against Misty either.** `gen1_battle` can arm
the fight from a preset, but it has no battle policy behind it — a fixed
"always use move slot 1" policy would flatter a Pikachu, whose one good move is
in slot 1, and punish a PARASECT, whose best move against STARYU and against
STARMIE are different. A measurement that is unfair between candidates is worse
than none.

## See also

* [`../gen1/catch-and-train.md`](../gen1/catch-and-train.md) — the cartridge
  half: which species beats which gym, where it is caught, and what a hunt
  costs in steps.
* [`misty.md`](misty.md) — the Cascade Badge without a bench, and the
  per-starter split this page is a reply to.
* [`../gen1/catching.md`](../gen1/catching.md) — the capture odds and the
  wild-versus-trainer discriminator the hunt rests on.

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
  `MT_MOON_B2F_TO_HUNT`.
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
| ZUBAT | GOLBAT | 5 | 6 | edge |

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

<!-- CERT TABLE -->

## 3. What it costs

<!-- COST -->

## 4. Findings

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

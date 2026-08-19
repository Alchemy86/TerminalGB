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

<!-- FINDINGS -->

## See also

* [`../gen1/catch-and-train.md`](../gen1/catch-and-train.md) — the cartridge
  half: which species beats which gym, where it is caught, and what a hunt
  costs in steps.
* [`misty.md`](misty.md) — the Cascade Badge without a bench, and the
  per-starter split this page is a reply to.
* [`../gen1/catching.md`](../gen1/catching.md) — the capture odds and the
  wild-versus-trainer discriminator the hunt rests on.

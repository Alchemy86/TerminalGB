# A paper's Generation 1 claims, checked against the cartridge

**Eleven statements about Generation 1 mechanics, each measured on a retail
Pokémon Blue cartridge running in this emulator. Most of them hold.**

The claims come from Appendix G of *The PokéAgent Challenge: Competitive and
Long-Context Learning at Scale* ([arXiv 2603.15563v2](https://arxiv.org/abs/2603.15563),
Karten, Grigsby, Upaa et al.), which derives a state-space estimate for Generation 1
OU from a list of asserted mechanics. Those assertions are testable on hardware
behaviour, and this repository has a cartridge-driven Gen 1 battle harness, so we
tested them.

**The headline is that most of them are right**, including one detail —
Reflect and Light Screen being per-Pokémon volatiles rather than field-wide
screens — that a great many Pokémon references get wrong, and that **this
project's own battle engine got wrong**. That last part is the most valuable
thing on this page and it is written up in §6.

The paper published its gap list, released its code and data, and is careful
about what it is estimating. Nothing here is a takedown; it is a second
measurement, which is what a paper deserves.

---

## 1. The table

Verdicts are limited to four words. **Confirmed** — the cartridge does what the
paper says. **Different convention** — the paper's number is right under a
stated reading of "turns", and a different reading gives a different number.
**Disagreement** — no reading of the claim matches what the cartridge does.
**Not determinable** — the measurement could not settle it.

| # | Claim | Their figure | Measured on the cartridge | Verdict |
|---|---|---|---|---|
| 1 | Toxic residual damage | `⌊N/16⌋ × maxHP` on turn *N* | `max(1, ⌊maxHP/16⌋) × N` — **200 of 200** unclamped ticks agree, **0 of 240** match the stated form | **Disagreement** |
| 2 | The toxic counter's range | 1–15 | No bound in the code (an 8-bit counter, incremented, never compared). Without healing the target faints on tick 6 — **highest observed 5**. With a healing target, **10**. | **Disagreement** |
| 3 | Reflect and Light Screen are per-Pokémon volatiles, not field-wide screens | per-Pokémon | Bits 2 and 1 of `wPlayerBattleStatus3`; the opponent's copy of that byte reads `$00` at the same moment; a switch zeroes the whole `$D060-$D064` block | **Confirmed** |
| 4 | Confusion — 5 states: none, or 1–4 turns | 1–4 turns | Counter rolls **2–5** (97 episodes); it is decremented at the start of each of the confused Pokémon's turns and the Pokémon acts confused on **roll − 1 = 1–4** of them | **Confirmed** *(different convention on the raw counter)* |
| 5 | Disable — 29 states: off, or one of 4 moves × 1–7 turns | 1–7 turns, 4 slots | Roll is **1–8** (136 episodes, the disassembly's own comment says "1-8 turns disabled"); turns still blocked after each countdown are **0–7**; the slot is uniform over **1–4** | **Different convention** |
| 6 | Thrash lock — 1–3 turns = 3 states | 3 | Roll is **2 or 3** (161 episodes); the counter therefore takes the values **1, 2, 3** while the lock is up; the whole lock runs **3 or 4** attacking turns | **Confirmed** *(counter-value convention)* |
| 7 | Bide — 1–2 turns = 2 states | 2 | Roll is **2 or 3** (281 episodes) — *the same byte and the same roll shape as Thrash* — so the counter takes **1, 2, 3** and the state count is **3**, not 2 | **Different convention**, and not the one that makes rows 4-6 and 8 right |
| 8 | Partial trapping lock — 1–4 turns = 4 states | 4 | Roll is **1–4** (371 episodes) in the source's own 3/8, 3/8, 1/8, 1/8 shape; the move runs **2–5** attacking turns | **Confirmed** |
| 9 | Focus Energy is a 2-state volatile | 2 | One bit, `GETTING_PUMPED` in `wPlayerBattleStatus2` | **Confirmed** |
| 10 | *(context, not a claim of theirs)* what Focus Energy does | — | It **quarters** the critical-hit rate. Measured **0.2604** (250/960) without and **0.0612** (36/588) with; `CriticalHitTest` predicts 70/256 = 0.2734 and 17/256 = 0.0664 | **Confirmed** — their state count is right and the semantics are the famous trap |
| 11 | *(cross-check)* the Gen 1 critical-hit mechanism | — | `b = baseSpeed >> 1`, then `<<1` (or `>>1` with Focus Energy), then `>>1` for a normal move; a rotated random byte compared below `b`. Both absolute rates land inside their 95 % intervals. | **Confirmed** |

One arithmetic consequence follows from rows 6-8, and it is the only correction
to a *total* on this page: the paper's mutually-exclusive move-lock group sums
to **13**, not 12, because Bide contributes 3 states and not 2 (§4.5).

---

## 2. How this was measured

Everything below was produced by
[`examples/gen1_paper_claims.rs`](../../examples/gen1_paper_claims.rs), which
starts real battles on a real cartridge through
[`plugins/battle-control`](../../plugins/battle-control/), plays them by pressing
buttons, and reads the game's own work RAM every frame. **It does not read this
project's battle model at all.** That is deliberate: a check that asks our model
what the cartridge does would only prove that we are self-consistent.

```bash
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --trials 40
```

### Provenance

* **Cartridge**: `Pokemon - Blue Version (USA, Europe) (SGB Enhanced).gb`,
  sha1 `d7037c83e1ae5b39bde3c30787637ba1d4c48ce2`.
* **Disassembly**: [`pret/pokered`](https://github.com/pret/pokered) at
  `0cd19d3`, built with rgbds 1.0.3. Its `pokeblue.gbc` has **exactly that
  sha1** (`roms.sha1`, line 2), so the symbol file that supplied every address
  below describes the cartridge that was measured, byte for byte.
* **Addresses**: taken from that build's `pokeblue.sym` and cross-checked
  against [the atlas](../../third_party/atlasgb/README.md), where each one
  carries `rom,live` evidence:
  `wPlayerBattleStatus1..3` `$D062-$D064`, `wEnemyBattleStatus1..3`
  `$D067-$D069`, `wPlayerNumAttacksLeft` `$D06A`, `wPlayerConfusedCounter`
  `$D06B`, `wPlayerToxicCounter` `$D06C`, `wPlayerDisabledMove` `$D06D`,
  `wEnemyNumAttacksLeft` `$D06F`, `wEnemyConfusedCounter` `$D070`,
  `wEnemyToxicCounter` `$D071`, `wEnemyDisabledMove` `$D072`,
  `wCriticalHitOrOHKO` `$D05E`.
* **Battles**: [`examples/gen1_paper_claims.preset`](../../examples/gen1_paper_claims.preset).
  Each preset exists to put one counter into WRAM where it can be watched; none
  of them is a fair fight.

### The one methodological trap worth repeating

**An episode is keyed on the status FLAG, not on the counter.** A trapping move
that *misses* clears `USING_TRAPPING_MOVE` (`MoveHitTest`'s `.moveMissed`) and
leaves `wPlayerNumAttacksLeft` standing, so the next Wrap's fresh roll looks
exactly like a decrement of the old one. The first version of this harness
recorded a counter going `4 -> 2` in one step and would have reported a duration
the game has never rolled. Keyed on the flag, the same data reads correctly and
the early end is recorded as what it is — 73 of the 371 trapping episodes and 39
of the 161 thrash ones ended that way, and both figures are consistent with the
moves' own accuracy and with Gen 1's rule that hurting yourself in confusion
clears a thrash.

### Sample sizes

| claim | episodes / observations | battles |
|---|---:|---:|
| Toxic residual | 240 ticks (200 unclamped) | 40 |
| Toxic against a healing target | 118 ticks | 12 |
| Confusion | 97 | 40 |
| Disable | 136 | 40 |
| Thrash | 161 | 40 |
| Bide | 281 | 40 |
| Partial trapping | 371 | 40 |
| Reflect / Light Screen | 1 each (deterministic) | 1 each |
| Critical hits, no Focus Energy | 960 attacks | 24 |
| Critical hits, Focus Energy up | 588 attacks (72 discarded before it landed) | 24 |

Emulation is deterministic, so every battle idles a different number of frames
before it is armed: `Random_` folds `rDIV` in on each call, so *time* is the only
lever that decorrelates two runs from the same save state. This is the same
technique [`plugins/gen1-shinyhunt`](../shiny-hunt.md) uses, and for the same
reason.

---

## 3. Toxic: the formula, and the range

### 3.1 The formula is the operands the other way round

`HandlePoisonBurnLeechSeed_DecreaseOwnHP` (`engine/battle/core.asm`) computes the
residual as max HP shifted right four times — its own comment is
*"c = max HP/16 (assumption: HP < 1024)"* — floors it at 1, and then, if
`BADLY_POISONED` is set, **increments the counter and multiplies by it**:

```asm
    srl b / rr c / srl b / rr c / srl c / srl c   ; c = maxHP / 16
    ld a, c / and a / jr nz, .nonZeroDamage
    inc c                                          ; damage is at least 1
.nonZeroDamage
    bit BADLY_POISONED, [hl] / jr z, .noToxic
    ld a, [de] / inc a / ld [de], a                ; increment toxic counter
.toxicTicksLoop
    add hl, bc / dec a / jr nz, .toxicTicksLoop    ; bc = damage * toxic counter
```

So the damage on the *N*th tick is `max(1, ⌊maxHP/16⌋) × N`. The paper's
footnote has it as `⌊N/16⌋ × max-HP`.

**Measured**: over 240 residual ticks in 40 battles, **200 of 200 unclamped
ticks** matched `max(1, ⌊maxHP/16⌋) × N` exactly, and **0 of 240** matched
`⌊N/16⌋ × maxHP`. (A tick that would take the target below zero is clamped to
its remaining HP and says nothing about the formula, so those are excluded.)

The stated form is also self-contradicting: `⌊N/16⌋` is zero for every *N* in
1..15, so it would deal no damage at all before turn 16 and produce **one**
trajectory rather than fifteen. The conclusion the footnote is drawing — that the
counter produces distinct trajectories — is right; the formula is not.

### 3.2 The range is not 1–15, and 15 is not reachable in a plain battle

There is no cap in the code. The counter is one byte, `inc`remented and never
compared, so nothing in the game bounds it. What bounds it in practice is
arithmetic: after *N* ticks a badly-poisoned Pokémon has lost
`⌊maxHP/16⌋ × N(N+1)/2`, which reaches its whole HP bar when `N(N+1)/2 ≥ 16`,
i.e. at **N = 6** — whatever the HP bar is, because the unit scales with it.

**Measured, plain battle**: highest counter reached **5** (the sixth tick is the
one that kills, and is clamped).

**Measured, against a target that heals**: a Chansey using Softboiled every turn
pushed the counter to **10** before the tick outgrew the heal, with all 118 ticks
still matching the formula. So the counter is genuinely unbounded by the code and
genuinely small in play, and **15 is neither**.

As a practical ceiling for a state-space *estimate*, 15 is not an unreasonable
choice — the paper's table says these are upper bounds — and it is worth saying
so. It is not a hardware bound.

### 3.3 One detail worth knowing for anyone modelling this

The counter is shared: the disassembly's own comment is *"the toxic ticks are
considered even if the damage is not poison (hence the Leech Seed glitch)"*. A
badly-poisoned, seeded Pokémon has both drains multiplied and bumps the counter
twice a turn. And `wPlayerToxicCounter` is **not** in the block `SendOutMon`
zeroes, while `BADLY_POISONED` is — which is why switching out turns Toxic back
into ordinary poison but leaves the counter where it was.

---

## 4. The durations

Every one of these counters is decremented once per turn at the start of the
affected Pokémon's move, and the game acts on whether the result is zero. So
there are two honest ways to say "how long it lasts": the value the game
**rolled**, and the number of turns still to come **after** the current one. The
paper uses the second consistently for four of the five, and the fifth is the
only real discrepancy in the group.

### 4.1 Confusion — confirmed

`ConfusionSideEffectSuccess`: `BattleRandom & 3`, then `inc a` twice — the
source's own comment is *"confusion status will last 2-5 turns"*.

**Measured**: 97 applications, rolled 2 (23), 3 (21), 4 (32), 5 (21). Every
countdown stepped down by one and stopped at zero. The Pokémon acts confused on
`roll − 1` turns — 1, 2, 3 or 4 — and on the turn the counter reaches zero it
snaps out and acts normally.

The paper's "5: none or 1–4 turns" is exactly that, and its state count is right.
Worth knowing if you are reading WRAM rather than a simulator: the *byte* holds
2–5 when freshly applied.

### 4.2 Disable — different convention

`DisableEffect`: `BattleRandom & $7`, then `inc a`, with the source's own comment
*"1-8 turns disabled"*, packed into the low nibble of `wPlayerDisabledMove` with
the slot (1-4) in the high nibble.

**Measured**: 136 applications, rolled 1 (20), 2 (13), 3 (24), 4 (14), 5 (15),
6 (19), 7 (14), 8 (17) — uniform over 1–8. The slot came up 1 (35), 2 (48),
3 (38), 4 (43), so the paper's "one of 4 moves" is right.

The paper says 1–7. Under the same "turns remaining after the countdown"
reading that makes confusion right, a roll of 1 leaves **zero** turns — the
Pokémon's very next move prints *"is no longer disabled"* — and is
indistinguishable from "off", so the reachable non-off values are 1–7 and the
count is `1 + 4 × 7 = 29`. That reading is consistent and their number follows
from it.

A caveat for anyone using this: the *menu* refuses the disabled slot whenever
`wPlayerDisabledMove` is nonzero, and the countdown happens during move
execution, so how many of the disabled Pokémon's own menus are actually blocked
is `roll − 1` or `roll`, depending on which side moved first in the turn Disable
landed. There is no single number.

### 4.3 Thrash and Petal Dance — confirmed

`ThrashPetalDanceEffect`: `BattleRandom & 1`, then `inc a` twice — *"set
thrash/petal dance counter to 2 or 3 at random"*.

**Measured**: 161 locks, rolled 2 (69) and 3 (92). Each lock attacks on the turn
it starts and then once per decrement, so it runs **3 or 4** attacking turns, and
the counter takes the values 1, 2, 3 while the lock is up — the paper's 3 states.

39 of the 161 ended before their countdown finished. That is real behaviour, not
a measurement artefact: a Pokémon that hurts itself in confusion clears
`THRASHING_ABOUT` along with everything else in `wPlayerBattleStatus1`
(`.MonHurtItselfOrFullyParalysed`), and a thrash ends *by* confusing its user, so
the next thrash is often started by an already-confused Pokémon.

### 4.4 Partial trapping — confirmed

`TrappingEffect`: `BattleRandom & 3`; if the result is 0 or 1 use it, otherwise
roll again and use `& 3`; then `inc a`. The source annotates it *"3/8 chance for
2 and 3 attacks, and 1/8 chance for 4 and 5 attacks"*.

**Measured**: 371 locks, rolled 1 (145), 2 (125), 3 (49), 4 (52) — 39 %, 34 %,
13 %, 14 % against the intended 37.5 / 37.5 / 12.5 / 12.5. Counter values 1–4,
the paper's 4 states; the move runs 2–5 attacking turns.

73 ended early, which is Wrap's 85 % accuracy: `TrappingEffect` runs *before* the
hit test (the source says so in a comment), so a first-turn miss sets the flag
and the counter and then `.moveMissed` clears the flag again.

### 4.5 Bide — the one real discrepancy in this group

`BideEffect`: `BattleRandom & 1`, then `inc a` twice — *"set Bide counter to 2 or
3 at random"*. **The same byte, the same mask and the same two increments as
Thrash.**

**Measured**: 281 Bides, rolled 2 (160) and 3 (121). Every countdown stepped down
by one to zero, exactly as Thrash's does.

So Bide and Thrash cannot have different state counts: they are the same counter
with the same roll. Under the counter-value reading both are 3; under
"turns-remaining-after-this-one" both are 3; under "how many whole turns the lock
runs" both are 2 (Thrash 3 or 4 attacking turns, Bide 3 or 4 turns from the Bide
to the release). The paper gives Thrash 3 and Bide 2.

There *is* a reading that makes 1–2 true — the number of turns the user spends
storing and doing nothing, excluding both the Bide turn and the release turn —
but it is not the reading that makes any of its four neighbours right.

Taking the counter-value reading the paper uses everywhere else, the
mutually-exclusive move-lock group is:

> None (1) + Thrash (3) + two-turn/Fly/Dig (1) + must recharge (1) + Bide (**3**)
> + partial trapping (4) = **13**

against the paper's 12. The effect on the published exponent is far below the
rounding — `V_Gen1` goes from `1.4254 × 10⁸` to `1.5442 × 10⁸`, and the battle
state stays at `10¹⁹²` — so this is a correction to the derivation rather than to
the result.

---

## 5. Reflect, Light Screen, Focus Energy and critical hits

### 5.1 The screens are per-Pokémon volatiles — confirmed

This is the claim most worth checking, because it is the one most references get
wrong: in Generation 3 and later Reflect and Light Screen are *side* conditions
with their own turn counters that survive a switch. In Generation 1 they are not.

`constants/battle_constants.asm` puts `HAS_LIGHT_SCREEN_UP` at bit 1 and
`HAS_REFLECT_UP` at bit 2 of `wPlayerBattleStatus3` / `wEnemyBattleStatus3` —
the per-active volatile block. `SendOutMon` (and `EnemySendOut`, and
`EnemySendOutFirstMon`) writes zero to five consecutive bytes starting at
`wPlayerStatsToDouble`, which is `$D060`-`$D064`: **all three status bytes
included**.

**Measured**, driving the real battle menu to a real voluntary switch:

```
Reflect        raised: true  wPlayerBattleStatus3 while up: $04  (bit 2)
               wEnemyBattleStatus3 at the same moment: $00
               CLEARED by the switch — the bit is gone
               $D060-$D069 before the switch: 00 00 00 00 04 00 00 00 00 00
               $D060-$D069 after  the switch: 00 00 00 00 00 00 00 00 00 00

Light Screen   raised: true  wPlayerBattleStatus3 while up: $02  (bit 1)
               wEnemyBattleStatus3 at the same moment: $00
               CLEARED by the switch — the bit is gone
```

The opponent's copy of the byte reading `$00` while ours reads `$04` is the other
half of the claim: the two sides do not share a screen, and there is no field to
put one on.

### 5.2 Focus Energy is a 2-state volatile, and that is not the interesting part

`GETTING_PUMPED` is one bit of `wPlayerBattleStatus2` with no counter, so the
paper's 2 states is right, and there is nothing to correct.

The semantics are the interesting part, and they are the famous Generation 1
trap: `CriticalHitTest` carries its own comment,

> *bug: using focus energy causes a shift to the right instead of left,
> resulting in 1/4 the usual crit chance*

so Focus Energy makes a critical hit **less** likely, not four times more. This
is context for anyone modelling the game, not an error in the derivation — but a
state-space estimate that treats it as an offensive buff would be reasoning about
a different game.

### 5.3 The critical-hit mechanism, measured

`CriticalHitTest` takes `b = baseSpeed >> 1`; with Focus Energy it shifts right
again, otherwise it shifts left (capping at 255); a high-critical move then
shifts left twice more and an ordinary move shifts right once. The check is a
random byte rotated left three times and compared below `b`, and rotation is a
bijection, so the probability is exactly `b / 256`.

For the level 5 Electrode used here — base Speed 140, the highest in the game,
which is what makes a few hundred attacks enough:

| | arithmetic | predicted | measured | 95 % interval |
|---|---|---:|---:|---|
| no Focus Energy | `(140>>1)<<1>>1 = 70` | 0.2734 | **0.2604** (250 / 960) | 0.2336 – 0.2891 |
| Focus Energy up | `(140>>1)>>1>>1 = 17` | 0.0664 | **0.0612** (36 / 588) | 0.0445 – 0.0836 |

Both predictions land inside their intervals, so this is a check of the absolute
rates and not merely of the ratio — a ratio alone would pass a model that had
both halves wrong by the same factor. The measured ratio is **0.235** against a
predicted 0.243. It is not exactly a quarter, and the reason is the second
truncating shift: `35 >> 1` is 17, not 17.5.

**How the attacker was chosen**: the harness advances a battle by pressing A,
which always takes move slot 1, so a player-side Focus Energy could never be
followed by an attack. A wild Pokémon instead picks one of its four slots
uniformly at random (`SelectEnemyMove`, `.chooseRandomMove`), so Focus Energy in
slot 1 lands within a few turns and stays up — nothing clears `GETTING_PUMPED`
short of a switch — while the other three slots keep attacking. The two presets
differ in exactly that one slot. Attacks before the flag came up are discarded
and counted separately (72 of them).

---

## 6. What we got wrong ourselves

**This project's Gen 1 battle engine had the Reflect/Light Screen claim
backwards, and the paper had it right.**

`Volatile::on_switch_out` in
[`crates/terminalgb-gen1-battle/src/state.rs`](../../crates/terminalgb-gen1-battle/src/state.rs)
carried this comment:

> *What `SwitchPlayerMon` clears — everything except the screens, which in Gen 1
> belong to the side rather than the Pokémon and survive a switch.*
>
> *Reflect and Light Screen surviving a switch is a Gen 1 behaviour, not an
> oversight here; `wPlayerBattleStatus3`'s screen bits are cleared only by the
> end of the battle or by Haze.*

Both sentences are wrong, and the measurement in §5.1 says so: `SendOutMon`
zeroes `$D060`-`$D064`, and the bit is gone the moment the reserve Pokémon comes
out. It was a confident, well-commented, cited-sounding note that nobody had
asked the cartridge about — which is exactly the failure this repository's
standing rule about the cartridge being the oracle exists to catch, applied to a
corner of the model the differential harness had never reached.

The fix and its handling are recorded in
[`docs/conformance-notes.md`](../conformance-notes.md).

**The general lesson, and it is not about Pokémon**: a claim in a paper you are
checking is also a claim about *you*. Two of the eleven rows above came out of
looking at our own code because the paper disagreed with it, and the one where
the paper was right is worth more than the ones where it was not.

---

## 7. Beyond the named claims

Three observations that fall out of the same reading, offered as context for
anyone building on that appendix rather than as errors. The paper's table
describes these components as *exact upper bounds*, so an omission matters more
than an over-count.

* **Substitute carries HP.** `wPlayerSubstituteHP` (`$CCD7`) is a whole byte, so
  a substitute is not two states. The paper counts it as 2. Given it also states
  that PP is omitted deliberately, this may be the same kind of scoping choice —
  but it is a state dimension, not a flag.
* **Bide accumulates damage in 16 bits.** `wPlayerBideAccumulatedDamage` is a
  word, and it is what the release does damage with.
* **Minimize is real but cosmetic.** `wPlayerMonMinimized` (`$CCF7`) exists and
  is 2 states as the paper says, but in Generation 1 it only changes how the
  sprite is drawn (`engine/battle/animations.asm`) — there is no Stomp-style
  damage doubling until later generations.

And one confirmation from the same appendix, checked while we were there: **sleep
is 1–7 turns**. `SleepEffect` masks a random byte with `SLP_MASK` (7) and rerolls
on zero, and the source's own comment is *"set target's sleep counter to a random
number between 1 and 7"*.

---

## 8. Re-running any of this

```bash
# Everything. ~10 minutes with these sample sizes.
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --trials 40 --frames 9000

# One claim, with every episode printed.
cargo run --release --features plugin-battle-control,plugin-trainer \
  --example gen1_paper_claims -- --rom ~/roms/blue.gb --claim trapping --verbose

# `--trace` adds the frame of every counter change, which is how the
# missed-trapping-move behaviour in §2 was found.
```

The **gate** is [`testharness/paperclaims.rs`](../../testharness/paperclaims.rs),
which runs the same measurement code and fails if the cartridge stops behaving
the way this page says it does:

```bash
POKE_ROM=~/roms/blue.gb cargo test --release \
  --features plugin-battle-control,plugin-trainer --test paperclaims
```

It skips with exit 0 and a printed reason without `POKE_ROM`, like
[`testharness/colosseum.rs`](../../testharness/colosseum.rs) and the atlas
harness — this repository ships no game data and the cartridge is yours. It is
**not in CI** for that reason; run it by hand when the harness or this page
changes.

Its assertions are chosen so they cannot flake: no roll outside the documented
range, the toxic formula on every unclamped tick, the screens cleared by a
switch, and the two critical-hit rates inside a 99.9 % interval of what
`CriticalHitTest` predicts. A 95 % interval would cry wolf one run in twenty and
would end up loosened until it meant nothing.

**Teeth proven**, and the demonstration is this page's own subject: narrowing the
gate's documented Disable range to the paper's **1–7** turns it red on exactly
that row and prints the observed range beside it —

```
[FAIL] disable_rolls_one_to_eight_turns_and_picks_a_slot_of_four
       — 21 episode(s), rolled 1-8 (documented 1-7)

20 check(s), 1 failure(s)
```

— while all nineteen other checks stay green.

**Build trap, not ours**: this repository's `[profile.release] panic = 'abort'`
plus its three lib crate-types occasionally hands a `harness = false` test the
wrong rlib, and it shows up here as `` the trait bound `BattlePlugin: Plugin` is
not satisfied ``. `touch src/lib.rs` and rebuild; see
[`docs/pitfalls/builds.md`](../pitfalls/builds.md).

---

## See also

* [The Gen 1 battle engine](battle-engine.md) — the model, the cartridge oracle
  that holds it to the game, and the agents that play it.
* [Battle control](battle-control.md) — how a battle is started from outside.
* [The Gen 1 memory map](memory-map.md) and
  [the atlas](../../third_party/atlasgb/README.md) — where every address above
  comes from and what evidence it carries.
* [The measured Game Boy](../measured/README.md) — the same shape of page for
  the hardware itself: what the reference says, what we measured, and the test
  that pins it.

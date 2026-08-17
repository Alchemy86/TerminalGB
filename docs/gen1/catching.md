# Catching, in Pokémon Red and Blue

Three pieces of this repository had existed for a while and had never been
joined: the **wild encounter tables** ([`tools/gen1-tables/gen1_tables.py`](../../tools/gen1-tables/gen1_tables.py)),
the **capture odds** ([`plugins/trainer/src/capture.rs`](../../plugins/trainer/src/capture.rs),
431 lines disassembled from `ItemUsePokeBall`), and `Action::Switch` in
[`plugins/agent/src/battle.rs`](../../plugins/agent/src/battle.rs), which is
implemented and has never once fired because the Pewter chain carries one
Pokémon from link 04 to link 14 and there is nothing on the bench.

This page joins them. It is a **measurement** page: every number in it came out
of a run against the retail cartridge, and the commands that produced them are
here.

> **Nothing was caught by the chain while this was written, and could not be.**
> The capability does not exist on the route as it stands — see §1, which is the
> first thing to read.

---

## 1. The gate: catching starts at zero, and the chain never opens it

A Red or Blue player begins with an empty bag. There is no "throw a ball"
decision to make until a ball exists, and a policy that answers *no* has weighed
something a policy with no ball has not. So the first question is not *is this
worth a ball* but **do we hold one**, and the answer on the Pewter chain is
measured rather than assumed.

```sh
cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
  --example gen1_catch_drill -- bag --rom <blue.gb> \
  --state <chain>/states/01.gbss  ...  --state <chain>/states/10.gbss
```

Against the chain's own carried-forward states (`pewter_chain chain --from 1
--to 10`, ten seconds of CPU):

| state | party | money | bag |
|---|---:|---:|---|
| 01 – 03 | 0 | ¥3000 | *(empty)* |
| 04 – 06 | 1 | ¥3000 / ¥3175 | *(empty)* |
| 07, 08 | 1 | ¥3175 | OAK's PARCEL ×1 |
| 09, 10 | 1 | ¥3175 | *(empty)* |

**The bag is empty at every one of them.** So the capability opens at **no link
of the chain as it stands today**, and `Verdict::Unavailable` is the honest
answer for the whole of links 01–14, not `No`.

### The two doors, both read out of the cartridge

**A scripted gift of five.** At `07:52D9` the cartridge holds

```
21 4B D7    ld hl, $D74B          ; wEventFlags + 4
CB 66       bit 4, [hl]
CB E6       set 4, [hl]
20 0E       jr nz, .already
01 05 04    ld bc, POKE_BALL, 5
CD 2E 3E    call GiveItem
21 0E 53    ld hl, .gotThemText
```

and the text it then prints is Oak's own, decoded from the charmap at file
offset `0x9506D`:

> OAK: You can't get detailed data on POKéMON by just seeing them. You must
> catch them! Use these to capture wild POKéMON. \<PLAYER\> got 5 POKé BALLs!

`$D74B` bit 5 is `EVENT_GOT_POKEDEX` — that is link 09's own evidence — so bit 4
is the event immediately before it, `EVENT_GOT_POKEBALLS_FROM_OAK`. **Measured:
the bit is clear at states 09 and 10.** So the gift has not fired by the time the
chain leaves Oak's laboratory for the second time, and a probe that roams the
laboratory from link 09's exit state for 30,000 frames does not open it either
(see §1a). *The gift exists in the cartridge; the chain does not collect it, and
exactly what collects it is not settled here.*

**A purchase.** `ItemPrices` (file offset `0x4608`, exactly one match for its
opening four entries) prices a POKé BALL at **¥200**, a GREAT BALL at ¥600 and
an ULTRA BALL at ¥1200. The mart tables (file offset `0x02442`) list Viridian
first — `POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL`, which is also the first list
in the game and the *only* one before Pewter that carries no POTION. The chain
**already walks into that shop at link 07** to collect Oak's Parcel, and carries
¥3175 from link 05 onward.

```sh
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --marts
```

So the cheapest way to open the capability is a purchase inside a map the chain
already stands in, with money it already has. That is a scenario the chain does
not have; adding it belongs with the crew that owns `examples/pewter_chain.rs`.

### 1a. A finding for the chain crew: link 09 exits inside a cutscene

Link 09's goal is `watch:got_pokedex!=0`, and `OaksLabOakGivesPokedexScript` sets
that flag *before* it hands control back. Measured on its exit state:

| state | `wJoyIgnore` | `$D74B` |
|---|---|---|
| 08 | `$00` | `0x8F` |
| **09** | **`$FF`** | `0xAF` (got-pokedex set, balls-from-oak clear) |
| 10 | `$00` | `0xAF` |

`wJoyIgnore = $FF` means the game is discarding every button. A probe that
starts pressing at that state measures nothing and reports its negative result
with total confidence — which is exactly what the first version of this one did.
Link 10's solver survives it because it runs frames until control returns.

---

## 2. Wild or trainer: established twice, and the screen only says it once

A ball thrown at a trainer's Pokémon is refused. So the policy must know which
kind of battle it is in — and it must know it two ways, because the
memory-derived expert has a byte and a screen-only student does not.

**From memory.** `wIsInBattle` `$D057`: 1 wild, 2 trainer, `$FF` blacked out,
and [`BattleKind::from_flag`](../../plugins/trainer/src/gen1.rs) maps everything
else to `None`. Verified here, not trusted: the drill synthesises a matched pair
of battles from one save with only the preset's `wild =`/`trainer =` line
different, and reads 1 and 2 respectively on every trial.

**From the screen** — and this is where the interesting result is.

```sh
cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
  --example gen1_catch_drill -- kinds --rom <blue.gb> --state <state> --trials 50
```

Traced frame by frame from the battle's first frame to its own menu, over the
matched pair:

| | measured |
|---|---|
| cells that differ on **all 1200 paired frames** | **NONE** |
| fewest cells differing on any paired frame | **2** — and they are the level digits, because the two Rattata are level 5 and level 11 |
| the wild line (`Wild <NAME> appeared!`) is on screen for | **9 frames** (frames 329–337) |
| the trainer line (`<NAME> wants to fight!`) is on screen for | **11 frames** (frames 206–216) |

**There is no persistent screen signal.** At the moment a policy wants to throw,
a wild battle and a trainer battle are the same picture. The signal is the
opening line and it lasts about a sixth of a second.

That dictates the shape of the discriminator:
[`battlekind::Watcher`](../../plugins/agent/src/battlekind.rs) is a **latch** —
it reads the first line and remembers it for the rest of the battle, exactly as
a person does — and it must be `reset()` when the battle ends, because carrying a
verdict into the next battle is the failure the whole thing exists to prevent.

Scored against `wIsInBattle` at the battle menu, 50 battles of each kind:

| | wrong or unknown |
|---|---|
| **latched** (`Watcher`) | **0 / 100 = 0.00 %** |
| stateless (`from_tiles` at the menu) | 100 / 100 = 100.00 % |

### The sampling rate is part of the discriminator

A window nine frames wide is a window a slow sampler misses. Measured, as the
fraction of sampling *phases* that never land inside it:

| interval | 1 | 2 | 4 | 6 | 8 | 12 | 16 | 24 | 32 | 48 |
|---|---|---|---|---|---|---|---|---|---|---|
| wild line missed | 0/1 | 0/2 | 0/4 | 0/6 | 0/8 | **3/12** | 7/16 | 15/24 | 23/32 | 39/48 |
| trainer line missed | 0/1 | 0/2 | 0/4 | 0/6 | 0/8 | **1/12** | 5/16 | 13/24 | 21/32 | 37/48 |

**Every interval up to 8 frames catches it from every phase.** The agent plugin
samples every 6 frames, so it is inside the window — but only just, and anything
that slows to a 12-frame cadence starts losing battles silently. That is a
constraint on the *host*, not on the policy, and it is written down here because
nothing else would notice it.

### The safety net, and it is free

**A ball thrown in a trainer battle is refused and kept.** `ItemUseBall` branches
to `ThrowBallAtTrainerMon`, which prints `TrainerBlockedText` — "The trainer
blocked the BALL! / Don't be a thief!", decoded from this cartridge at file
offset `0xA6B0B` — and returns without touching the bag.

Measured over **64 throws in synthesised trainer battles: 64 blocked, 64 balls
kept, 0 spent.**

So mistaking a trainer battle for a wild one costs **a turn**, never a ball.
That is why the policy declines an *uncertain* screen rather than refusing to
run at all, and why declining is cheap enough to be the safe default.

---

## 3. The mechanics, measured against the odds we predicted

```sh
cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
  --example gen1_catch_drill -- drill --rom <blue.gb> --state <state> --trials 64
```

The drill synthesises five kinds of encounter, sweeps the opponent's HP across
its whole range and its status across none/sleep/poison, throws Poké, Great and
Ultra Balls in rotation, and compares what happened with what
[`capture::odds`](../../plugins/trainer/src/capture.rs) said *before* the ball
left our hand. **251 throws** (a further 64 refused in trainer battles, counted
separately and never folded in):

| predicted | n | caught | observed | expected |
|---|---:|---:|---:|---:|
| 0–10 % | 41 | 3 | 7.3 % | 3.1 % |
| 10–20 % | 18 | 6 | 33.3 % | 13.9 % |
| 20–30 % | 2 | 0 | 0.0 % | 27.2 % |
| 30–40 % | 13 | 2 | 15.4 % | 37.0 % |
| 40–50 % | 15 | 5 | 33.3 % | 45.5 % |
| 50–60 % | 29 | 17 | 58.6 % | 53.8 % |
| 60–70 % | 9 | 5 | 55.6 % | 64.8 % |
| 70–80 % | 40 | 32 | 80.0 % | 74.9 % |
| 80–90 % | 5 | 4 | 80.0 % | 82.4 % |
| 90–100 % | 79 | 79 | 100.0 % | 99.6 % |
| **all** | **251** | **153** | **61.0 %** | **59.8 %** |

**z = +0.36** against a binomial standard error of 3.09 %. The disassembled
closed form and the cartridge agree.

The buckets are noisy — 6 of 18 in the 10–20 % row, 2 of 13 in the 30–40 % — and
they are *supposed* to be at those counts; the aggregate is the claim. The
buckets are printed anyway because an aggregate can be right on average while
being wrong everywhere, and only the curve shows that.

Three throws out of 254 failed to resolve inside the drill's own patience
(all against Mewtwo, screen blank at the time). They are reported, not swallowed.

---

## 4. The target list, derived from what is ahead

Nothing here names a Pokémon. `gen1_tables.py --plan` reads the opposition's own
parties, their types, what beats those types, the candidate species' **starting
moves**, and where those species live — five cartridge tables, each located by
signature.

```sh
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan brock,misty --max-level 16
```

```
threats[4]{trainer,species,level,types,beaten_by}:
  BROCK,GEODUDE,12,GROUND/ROCK,FIGHTING GROUND WATER GRASS ICE
  BROCK,ONIX,14,GROUND/ROCK,FIGHTING GROUND WATER GRASS ICE
  MISTY,STARYU,18,WATER,GRASS ELECTRIC
  MISTY,STARMIE,21,PSYCHIC/WATER,BUG GRASS ELECTRIC
```

| species | where | levels | slots | catch rate | arrives with | answers |
|---|---|---|---:|---:|---|---|
| ZUBAT | Mt Moon 1F | 6–11 | 6/10 | 255 | LEECH LIFE (BUG 20) | Misty/Starmie |
| BELLSPROUT | map `0x23` | 12–14 | 3/10 | 255 | VINE WHIP (GRASS 35) | both of Brock's, both of Misty's |
| **PIKACHU** | **Viridian Forest** | **3–5** | **2/10** | **190** | **THUNDERSHOCK (ELECTRIC 40)** | **both of Misty's** |
| TENTACOOL | map `0x1E` | 5–40 | 10/10 | 190 | *(no effective move at level 1)* | both of Brock's |
| SANDSHREW | Route 4 | 6–12 | 4/10 | 255 | *(no effective move at level 1)* | both of Brock's |
| CATERPIE | Route 2 | 3–5 | 3/10 | 255 | *(no effective move at level 1)* | Misty/Starmie |

Pikachu is the answer the captain expected, and the tool reaches it without
being told: **Viridian Forest, 2 of 10 grass slots, levels 3–5, on a route the
chain already walks**, and — the part a type-only query cannot say — it *arrives
already knowing* an electric move.

That last column is why the plan reads starting moves out of each species'
base-stats record (`data/pokemon/base_stats/*.asm` puts `db MOVE1..MOVE4` at
offset 15) rather than stopping at the species' type. A Sandshrew is GROUND and
will beat Brock, eventually; a level-6 Sandshrew arrives with SCRATCH.

### The starter changes what is still needed — and by less than it looks

The Pewter chain varies the starter by seed, so the list has to be per starter.
The same `--plan` prints it:

| starter | type | arrives with | with the moves it has | by its own type, once it learns one |
|---|---|---|---|---|
| BULBASAUR | GRASS/POISON | TACKLE, GROWL | **nothing on the list** | both of Brock's, both of Misty's |
| CHARMANDER | FIRE | SCRATCH, GROWL | **nothing on the list** | **nothing on the list** |
| SQUIRTLE | WATER | TACKLE, TAIL WHIP | **nothing on the list** | both of Brock's |

Two different claims, and conflating them overstates: a Bulbasaur *will* beat
Brock — with VINE WHIP, at level 13. At the level it is caught, **no starter
arrives with a move that beats anything on the list**, which is the honest case
for catching rather than relying on the starter.

So: the target list is **the same for all three starters** through Brock and
Misty, and the per-starter difference is one of urgency rather than of contents.
A Charmander run needs it most and a Bulbasaur run least.

---

## 5. The policy

[`plugins/agent/src/catch.rs`](../../plugins/agent/src/catch.rs). It decides in
the order a wrong answer costs, exactly as
[`battle::decide`](../../plugins/agent/src/battle.rs) does:

1. **No ball** → `Verdict::Unavailable`. Not a decision.
2. **Not a wild battle, or the screen has not said** → `No`.
3. **`wBattleType` says the Pokémon never becomes ours** (the old man's
   demonstration) → `No`.
4. **Not on the target list, or already held** → `No`.
5. **A throw costs a turn we would not survive** — worst case including a
   critical hit, the same reading `battle::View::incoming_crit` makes → `No`.
6. Then: the **cheapest ball**, unless money has stopped being the scarce
   thing; and `Weaken` rather than `Throw` when the chance is under a third
   *and* the fight can still improve it.

The odds come from `capture::odds`, never from a second formula. The Master Ball
is never spent: it has no second copy and no shop, and every target a *route*
wants is a common wild Pokémon a ¥200 ball catches.

### Which ball, derived rather than tuned

`ItemPrices` puts a Great Ball at **three** Poké Balls and an Ultra Ball at
**six**. Their advantage is bounded and the bound is small: the resampling loop
narrows the first random byte to `0..=200` or `0..=150` instead of `0..=255`,
worth at most **1.27×** and **1.70×**, and the Great Ball's smaller HP divisor
(8 against 12, `03:5742`) at most a further **1.5×**. So a Great Ball is at most
1.91× a Poké Ball and an Ultra Ball at most 1.70×, against price ratios of 3×
and 6×.

**A Poké Ball therefore always wins on chance per ¥, at every catch rate and
every HP.** That is arithmetic over the routine, not a preference, and
`a_poke_ball_is_always_the_better_value` sweeps 735 states rather than asserting
it. The dearer balls are reached for in exactly one case: when even the cheap
ball's chance is under a third, the encounter is likelier to be lost than won,
and then a *turn* — not ¥400 — is the scarce thing.

(A first version used a "within 90 % of the best chance" margin instead. It
measured as spending a Great Ball on nineteen out of twenty Pikachu, because at
catch rate 190 the Great Ball really is 27 % better — and three times the price
for 27 % more chance is a bad trade nobody had written down as one.)

Measured end to end — the policy asked, and the throw it recommends carried out:

```sh
gen1_catch_drill policy       --rom <blue.gb> --state <state> --trials 24
gen1_catch_drill policy-empty --rom <blue.gb> --state <state> --trials 8
```

| bag | verdicts |
|---|---|
| **empty** | 40 encounters, **40 `unavailable`, 0 decisions** |
| Poké + Great Balls | 120 encounters, **120 decisions, 0 non-decisions**: 24 declined as a trainer battle, 72 declined as not on the list, **24 thrown** |

Of the 24 throws the policy chose — **19 Poké Balls and 5 Great Balls**, the
Great Balls only where the cheap one had dropped under the threshold — 14 caught
(58.3 %) against 57.7 % predicted, **z = +0.07**.

The empty-bag row is the captain's constraint made checkable: a run before the
capability exists produces *no catch decisions at all*, so a student trained on
these runs never sees a catch attempt before a ball is held and cannot learn to
try one.

---

## 6. Rotation is deliberately not built

`Action::Switch` still has nothing to do. Switching on type advantage is the
obvious first rule and the battle policy already reads both sides' types from
the cartridge's own chart — but the chain carries one Pokémon, the bag is empty,
and a rule that can never fire cannot be tested. **It waits for a bench**, which
waits for §1's door being opened. Building it now would be building it blind.

---

## 7. How this is trained

Two halves, and the split is principled.

**The mechanics are drilled en masse.** Opening the bag, selecting a ball,
throwing it, and handling all four outcomes — caught, broke free, refused, no
ball — is self-contained and has no route dependency. `battle-control` can start
any battle from `wCurOpponent`, so thousands of these are synthesised from one
save with no walking at all: the 251-throw table in §3 is 2 minutes 35 seconds
of one core. That is where the volume goes, and it is what a student needs the
most examples of.

**The decision is learned in course.** "Is this worth a ball" depends on where
you are, what you already hold and what is ahead — none of which exists outside a
real run. That belongs in the chain, as a scenario, and it cannot be written
until the chain holds a ball.

### What the next task will want from this one

The next piece of work is tens of thousands of **synthesised battles**, including
the game's own trainer battles, per starter. Three things here are built for it
and should be reused rather than rewritten:

* `examples/gen1_catch_drill.preset`'s **matched pairs** — the same challenger,
  the same save, and only `wild =` versus `trainer =` different. A discriminator
  or a policy scored on wild battles from one save and trainer battles from
  another is measuring the saves.
* `Trainers::rosters` in `gen1_tables.py` — every trainer class's parties, read
  from `TrainerDataPointers` and **bounded by the next class's pointer**. Without
  that bound, asking for a roster a class does not have walks into the next
  class's data and answers confidently with somebody else's team; the first
  version of it reported Brock's second roster as Misty's first.
* `Moves` and `starting_moves` — a synthesised opponent is only interesting if
  you know what it can actually do at the level it is generated at.

---

## Reproducing everything on this page

```sh
# the chain's own states, ten seconds
cargo run --release --example pewter_chain -- chain --rom <blue.gb> \
    --seeds 1 --from 1 --to 10 --out target/catch/chain

# the gate
gen1_catch_drill bag   --rom <blue.gb> --state target/catch/chain/states/NN.gbss ...
gen1_catch_drill gift  --rom <blue.gb> --state target/catch/chain/states/09.gbss --trials 20

# the discriminator, the mechanics, the policy
gen1_catch_drill kinds        --rom <blue.gb> --state <state> --trials 50
gen1_catch_drill drill        --rom <blue.gb> --state <state> --trials 64
gen1_catch_drill policy       --rom <blue.gb> --state <state> --trials 24
gen1_catch_drill policy-empty --rom <blue.gb> --state <state> --trials 8

# the cartridge tables
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --marts
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --gyms
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan brock,misty --max-level 16
```

`gen1_catch_drill` is `cargo run --release --features
plugin-battle-control,plugin-trainer,plugin-agent --example gen1_catch_drill --`.
It needs the player's own cartridge; none is distributed with this repository,
and it stages the ROM into `target/` before playing it because the MBC writes a
`.sav` beside whatever path it is handed.

**None of this is in CI.** It needs a commercial cartridge, the same rule
`tools/sameboy-oracle/` and `testharness/paperclaims.rs` follow. What *is* in a
plain `cargo test` is the policy's own decision table and the discriminator's
text rules — 18 unit tests across `catch.rs` and `battlekind.rs`, none of which
needs a ROM.

## See also

* [The capture routine](../../plugins/trainer/src/capture.rs) — the three things
  a modern formula gets wrong about Gen 1, and why the ball threshold is *help*.
* [Battle control](battle-control.md) — how a battle is started on demand.
* [Sharp edges](sharp-edges.md) and [discoveries](discoveries.md) — the traps.
* [The Pewter chain](../agent/pewter-chain.md) — the route this plans for.

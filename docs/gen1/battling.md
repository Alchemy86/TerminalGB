# Battling, in Pokémon Red and Blue

**The map of the subject.** A battle is where most of this project's Gen 1 work
lands, and it had been written down in five places — a policy page, an engine
page, a control page, a catching page and a discoveries page — with nothing
saying how they fit together or what a battle *screen* actually tells you.

This page is that. It is short on purpose: everything with a home of its own is
linked, not repeated.

| you want to know | the page |
|---|---|
| how to **start** a battle from outside | [battle control](battle-control.md) |
| what the **rules** are, held to the cartridge by a differential harness | [the battle engine](battle-engine.md) |
| what to **do** on a turn — fight, switch, item or run | [the battle policy](../agent/battle-policy.md) |
| whether the encounter is **worth a ball** | [catching](catching.md) |
| what happens **after** the battle — a new move, an evolution | [learning moves](learning-moves.md) |
| **playing** a level-up out on the cartridge, and the corpus it makes | [the level-up move drill](level-up-drill.md) |
| every **fault and corrected belief** found on the way | [discoveries](discoveries.md) |

Everything below is what did *not* have a home: what the battle screen shows,
what it withholds, and how long each thing is on it.

---

## 1. A wild battle and a trainer battle are the same picture

This is the sharpest measurement the catching work produced and it belongs
where a reader looking for "the battle screen" will find it. Traced frame by
frame over a **matched pair** — one save, one challenger, and only the preset's
`wild =` versus `trainer =` line different:

| | measured |
|---|---|
| cells that differ on **all 1,200 paired frames** | **none** |
| fewest cells differing on any paired frame | **2** — and they are the level digits |
| the wild line (`Wild <NAME> appeared!`) is on screen for | **9 frames** |
| the trainer line (`<NAME> wants to fight!`) is on screen for | **11 frames** |

So at the moment a policy wants to act — the battle menu — **there is no
persistent screen signal at all**. The only separator is the opening line and it
lasts about a sixth of a second.

That dictates the shape of the discriminator rather than being an inconvenience
to it. [`battlekind::Watcher`](../../plugins/agent/src/battlekind.rs) is a
**latch**: it reads the first line, remembers it for the rest of the battle, and
is `reset()` when the battle ends, exactly as a person does. Scored against
`wIsInBattle` at the battle menu over 50 battles of each kind:

| | wrong or unknown |
|---|---|
| **latched** | **0 / 100** |
| stateless, asked at the menu | 100 / 100 |

### The sampling rate is part of the discriminator

A window nine frames wide is a window a slow sampler misses. Measured as the
fraction of sampling *phases* that never land inside it:

| interval | 1 | 2 | 4 | 6 | 8 | 12 | 16 | 24 | 32 | 48 |
|---|---|---|---|---|---|---|---|---|---|---|
| wild line missed | 0/1 | 0/2 | 0/4 | 0/6 | 0/8 | **3/12** | 7/16 | 15/24 | 23/32 | 39/48 |
| trainer line missed | 0/1 | 0/2 | 0/4 | 0/6 | 0/8 | **1/12** | 5/16 | 13/24 | 21/32 | 37/48 |

**Every interval up to 8 frames catches it from every phase.** The agent plugin
samples every 6, so it is inside the window — but only just, and anything that
slows to a 12-frame cadence starts losing battles silently. That is a constraint
on the **host**, not on the policy, and it is the reason it is written down: it
is invisible from inside either.

### Getting it wrong costs a turn, never a ball

`ItemUseBall` branches to `ThrowBallAtTrainerMon`, prints `TrainerBlockedText`
— *"The trainer blocked the BALL! / Don't be a thief!"*, decoded from this
cartridge at file offset `0xA6B0B` — and returns **without touching the bag**.
Measured over **64 throws in synthesised trainer battles: 64 blocked, 64 balls
kept, 0 spent.**

So declining an *uncertain* screen is the cheap default, and that is why the
policy does it rather than guessing.

---

## 2. What the battle screen shows, and what it does not

Stated plainly, because every claim about a screen-only student turns on it. A
player looking at the battle screen sees:

* both Pokémon's **names** (the nickname, not the species — see below) and
  **levels**;
* both **HP bars**, and the player's own **exact HP** as a fraction;
* on the FIGHT menu, the four **move names** and, for the highlighted one, its
  **type and PP**.

It does **not** show either side's stats, the opponent's exact HP, or the
opponent's move list. The [battle policy](../agent/battle-policy.md) uses all
three, which is exactly why nothing it measures is evidence about a student.

**The name on the screen is the nickname.** Gen 1 defaults a nickname to the
species name, so for a Pokémon nobody renamed the two coincide — but a renamed
one shows the name its owner gave it and the species is not on the battle screen
at all. Anything that needs the species has to get it from the party menu or from
its own memory of what it caught.

---

## 3. The Gen 1 mechanics that surprised us

All four were found the hard way and all four are written up with their
reasoning on [discoveries](discoveries.md) and their consequences on
[the battle policy](../agent/battle-policy.md). They are listed here so a reader
arriving at "battling" meets them, not so they are told twice:

* **A critical hit is the worst case, not the top of the roll range.** Gen 1
  substitutes `2 x Level` for `Level`, so a crit is close to double an ordinary
  hit, and it happens about one turn in eleven for a Route 1 Pokémon. A faint
  test that ignores it lost 10 of 600 attempts.
* **A move's accuracy is not the number in the move table.** `MoveHitTest`
  scales it by the attacker's accuracy stage and the defender's evasion, so a
  Pidgey's SAND-ATTACK takes a 95% TACKLE to 47%.
* **The battle move menu's cursor is 1-based and the main menu's is not.**
  Getting it wrong is silent: the cursor is already where you wanted it, so A
  fires on the wrong move and the policy's choice is thrown away.
* **An escape is not free.** Running does not dodge the incoming hit — it dodges
  it only if it works, and 162 of 3,000 attempts had an escape fail. When two
  choices cost the same, prefer the one that ends the situation.

And one that is new here, from the drill that traced the post-battle prompts:

* **Crossing two levels in one battle loses the move in between.** A level 6
  BULBASAUR fed one CHANSEY gains 182 experience and lands on level **8**; the
  game announces one level per battle and checks the learnset for the level it
  ended on, so level 7's LEECH SEED is never offered at all. Measured — it is
  what made the first version of `gen1_learn_drill` report no prompt.

---

## 4. Reproducing the measurements on this page

```sh
# the wild/trainer pair, the sampling table, the refused throws
cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
    --example gen1_catch_drill -- kinds --rom <blue.gb> --state <state> --trials 50

# what the post-battle prompts look like, frame by frame
cargo run --release --features plugin-battle-control,plugin-trainer \
    --example gen1_learn_drill -- trace --rom <blue.gb>
```

Neither is in CI: both need the player's own cartridge, the same rule
`tools/sameboy-oracle/` and `testharness/paperclaims.rs` follow. `gen1_catch_drill`
needs a save state; `gen1_learn_drill` starts a new game and needs none.

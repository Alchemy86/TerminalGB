# Catching, in Pokémon Red and Blue — the cartridge half

> **This page was split on 2026-08-19.** What the *cartridge* does when a ball
> is thrown — the disassembled odds and their agreement with the hardware, how a
> wild battle is told from a trainer battle, and the two doors the ROM opens —
> is here, because it is a fact about the Game Boy. **What our agent decides,
> what it targets and what it has caught** moved to
> [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md).
> Neither page repeats the other's numbers. See
> [`docs/where-things-live.md`](../where-things-live.md).

Three pieces of this repository had existed for a while and had never been
joined: the **wild encounter tables** ([`tools/gen1-tables/gen1_tables.py`](../../tools/gen1-tables/gen1_tables.py)),
the **capture odds** ([`plugins/trainer/src/capture.rs`](../../plugins/trainer/src/capture.rs),
431 lines disassembled from `ItemUsePokeBall`), and `Action::Switch` in
[`plugins/agent/src/battle.rs`](../../plugins/agent/src/battle.rs).

This page joins them. It is a **measurement** page: every number in it came out
of a run against the retail cartridge, and the commands that produced them are
here.

---

## 1. The two doors, both read out of the cartridge

A Red or Blue player begins with an empty bag, so there is no "throw a ball"
decision to make until a ball exists. The cartridge offers exactly two ways to
get one.

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

`$D74B` bit 5 is `EVENT_GOT_POKEDEX`, so bit 4 is the event immediately before
it, `EVENT_GOT_POKEBALLS_FROM_OAK`.

**A purchase.** `ItemPrices` (file offset `0x4608`, exactly one match for its
opening four entries, three-byte BCD per item, ids 1-based) prices a POKé BALL
at **¥200**, a GREAT BALL at ¥600, an ULTRA BALL at ¥1200, a POTION at ¥300 and
an ANTIDOTE at ¥100. The mart tables (file offset `0x02442`) list Viridian
first — `POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL`, which is also the first list
in the game and the *only* one before Pewter that carries no POTION.

```sh
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --marts
```

**Which of the two our own chain walks through, and when it first holds a ball,
is a fact about our agent** and is
[in AgentGB](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md) §1.

---

## 2. Wild or trainer: established twice, and the screen only says it once

A ball thrown at a trainer's Pokémon is refused. So a policy must know which
kind of battle it is in — and it must know it two ways, because a
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

So mistaking a trainer battle for a wild one costs **a turn**, never a ball —
which is why a policy can afford to decline an *uncertain* screen rather than
refusing to run at all.

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

> **This is a validation of the routine against the hardware.** It is not a
> measurement of any policy: nothing here chose what to throw or when. The
> policy's own end-to-end run — 24 throws it selected itself, 14 caught,
> z = +0.07 — is
> [in AgentGB](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md) §4,
> and the two numbers must not be quoted as one.

---

## 4. The tables a route is planned from

`gen1_tables.py --plan` reads the opposition's own parties, their types, what
beats those types, the candidate species' **starting moves**, and where those
species live — five cartridge tables, each located by signature.

```sh
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan brock,misty --max-level 16
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --where zubat
```

```
threats[4]{trainer,species,level,types,beaten_by}:
  BROCK,GEODUDE,12,GROUND/ROCK,FIGHTING GROUND WATER GRASS ICE
  BROCK,ONIX,14,GROUND/ROCK,FIGHTING GROUND WATER GRASS ICE
  MISTY,STARYU,18,WATER,GRASS ELECTRIC
  MISTY,STARMIE,21,PSYCHIC/WATER,BUG GRASS ELECTRIC
```

The tool is here; **the target list it printed for our own route is a fact
about our route** and lives
[in AgentGB](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md) §2.

Two things about the tool are worth not re-deriving:

* **`Trainers::rosters` is bounded by the next class's pointer.** Read from
  `TrainerDataPointers` without that bound, asking for a roster a class does not
  have walks into the next class's data and answers confidently with somebody
  else's team; the first version of it reported Brock's second roster as
  Misty's first.
* **`starting_moves` comes out of each species' base-stats record**
  (`data/pokemon/base_stats/*.asm` puts `db MOVE1..MOVE4` at offset 15), not out
  of its type. A synthesised opponent is only interesting if you know what it
  can actually do at the level it is generated at.

---

## 5. Reproducing everything on this page

```sh
# the discriminator, the mechanics
gen1_catch_drill kinds --rom <blue.gb> --state <state> --trials 50
gen1_catch_drill drill --rom <blue.gb> --state <state> --trials 64

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

* [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md)
  — the other half: what our policy decides, and what it has caught.
* [The capture routine](../../plugins/trainer/src/capture.rs) — the three things
  a modern formula gets wrong about Gen 1, and why the ball threshold is *help*.
* [Battle control](battle-control.md) — how a battle is started on demand.
* [Sharp edges](sharp-edges.md) and [discoveries](discoveries.md) — the traps.

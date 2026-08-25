# Catching — the discriminator and the route tools

> **The cartridge half of this page moved to AtlasGB.** The two doors a ball comes through,
> `ItemUsePokeBall`'s disassembly, and the 251-throw measurement against the closed form are now
> on [AtlasGB's catching](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/catching.md)
> page — a fact about the cartridge, so AtlasGB owns it and cites more evidence than this page
> ever did. **What our agent decides, what it targets and what it has caught** stays on
> [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md).
> See [`docs/where-things-live.md`](../where-things-live.md).
>
> What is left here is specific to **this repository's own tooling**: the discriminator that
> tells a wild battle from a trainer battle without a persistent screen signal, and the route
> tables `gen1_tables.py` reads to plan a target.

---

## Wild or trainer: the discriminator, and why it has to be a latch

A ball thrown at a trainer's Pokémon is refused (measured on the cartridge and now on
[AtlasGB's battle page](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/battle.md#what-the-battle-screen-shows-and-what-it-doesnt)).
So a policy must know which kind of battle it is in — and it must know it two ways, because a
memory-derived expert has a byte and a screen-only student does not.

**From memory.** `wIsInBattle` `$D057`: 1 wild, 2 trainer, `$FF` blacked out, and
[`BattleKind::from_flag`](../../plugins/trainer/src/gen1.rs) maps everything else to `None`.
Verified here, not trusted: the drill synthesises a matched pair of battles from one save with
only the preset's `wild =`/`trainer =` line different, and reads 1 and 2 respectively on every
trial.

**From the screen.** AtlasGB's own measurement is what settles the interesting result: a wild
and a trainer battle are the same picture at every frame except a nine-to-eleven-frame opening
banner. **There is no persistent screen signal.**

```sh
cargo run --release --features plugin-battle-control,plugin-trainer,plugin-agent \
  --example gen1_catch_drill -- kinds --rom <blue.gb> --state <state> --trials 50
```

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

---

## The tables a route is planned from

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

## Reproducing everything on this page

```sh
# the discriminator
gen1_catch_drill kinds --rom <blue.gb> --state <state> --trials 50

# the cartridge tables
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --marts
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --gyms
python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --plan brock,misty --max-level 16
```

`gen1_catch_drill` is `cargo run --release --features
plugin-battle-control,plugin-trainer,plugin-agent --example gen1_catch_drill --`.
It needs the player's own cartridge; none is distributed with this repository,
and it stages the ROM into `target/` before playing it because the MBC writes a
`.sav` beside whatever path it is handed. `gen1_catch_drill drill` (the
251-throw run) is now the AtlasGB gate's territory; see that page for what
reproduces it.

**None of this is in CI.** It needs a commercial cartridge, the same rule
`tools/sameboy-oracle/` and `testharness/paperclaims.rs` follow. What *is* in a
plain `cargo test` is the policy's own decision table and the discriminator's
text rules — 18 unit tests across `catch.rs` and `battlekind.rs`, none of which
needs a ROM.

## See also

* [AtlasGB: catching](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/catching.md) —
  the two doors, the disassembly, and the 251-throw measurement against the closed form.
* [AgentGB `docs/agent/catching.md`](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent/catching.md)
  — the other half: what our policy decides, and what it has caught.
* [The capture routine](../../plugins/trainer/src/capture.rs) — the three things
  a modern formula gets wrong about Gen 1, and why the ball threshold is *help*.
* [Battle control](battle-control.md) — how a battle is started on demand.
* [AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md)
  and [discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) — the traps.

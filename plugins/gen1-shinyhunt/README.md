# `terminalgb-plugin-gen1-shinyhunt`

Shiny hunting for Pokémon Red and Blue: many headless emulators rolling
encounters in parallel, each reading the DVs out of memory the instant the game
generates them, all of them stopping the moment one matches.

**Generation I has no shiny Pokémon.** Shininess arrives in Generation II and is
derived from the DVs a Pokémon already carries, and those DVs cross the Time
Capsule unchanged — so "shiny hunting in Gen 1" means, and can only mean, hunting
the DV spread that *becomes* shiny when the Pokémon is traded forward:

> defence, speed and special all exactly **10**, and the attack DV one of
> 2, 3, 6, 7, 10, 11, 14, 15 (bit 1 set) — 8 spreads in 65,536, **1 in 8,192**.

That single rule lives in `terminalgb-plugin-trainer`
(`derived::shiny_if_traded`, cited to `CheckShininess`,
`pokecrystal/engine/gfx/color.asm`) and this crate **calls** it. There is one
copy of it in the tree, shared with the trainer panel's `★` and the
wild-encounter banner, and a test here enumerates all 65,536 spreads to keep it
that way.

Because the answer is read out of `wEnemyMonDVs` rather than seen on a sprite,
there is nothing to watch and nothing to miss — which is the whole difference
from the four-console hardware rig this imitates.

## The two halves

* **`ShinyHunt`** — the plugin. Declares `OBSERVE | READ_MEMORY` and nothing
  else, so the host *refuses* it `poke` and `press`. A hunter that could write
  memory could fabricate its own hit.
* **`hunt`** — the driver: N machines, an encounter loop each, stop on a hit,
  replay it in a fresh machine before believing it. Written against
  `hunt::HuntMachine`, an eight-method seam the emulator implements for
  `Gameboy`, because loading a save state and driving the clock are things the
  plugin API has no verb for. See `docs/shiny-hunt.md` §3 — that gap is filed as
  a finding, not worked around.

## Using it

Interactively, as a watcher on your own game (the status panel tells you whether
the wild Pokémon on screen matches):

```toml
# ~/.config/gameboy/config.toml
plugins = gen1-shinyhunt
```

`GB_SHINY_TARGET` sets what it watches for (default `shiny`).

For the parallel hunt, which is a different program:

```bash
cargo run --release --features plugin-gen1-shinyhunt --example shiny_hunt -- --help
```

## Targets

`shiny`, `perfect`, `atk=10`, `spd>=14`, `atk&2`, `total>=70`,
`species=NIDORAN` — comma or space separated, all must hold. Every target
reports its **exact** probability, enumerated over all 65,536 DV pairs rather
than multiplied: the HP DV is derived from the low bit of the other four, so the
terms are not independent and a closed form gets them wrong.

A common target such as `spc=10` (1 in 16) is how you demonstrate the machinery
without waiting for 1 in 8,192.

## Tests

```bash
cargo test -p terminalgb-plugin-gen1-shinyhunt
```

28 tests, no ROM and no emulator — the driver loop runs against a fake machine
whose rolls depend on input timing, so decorrelation is proved in CI in **both**
directions (decorrelated instances must differ; identically-seeded ones must
agree exactly).

Full write-up, measured rates and the cartridge sharp edges:
[`docs/shiny-hunt.md`](../../docs/shiny-hunt.md).

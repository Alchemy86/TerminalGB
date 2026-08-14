# `trainer` — live Pokémon Red/Blue trainer view

Press **`T`** while playing Pokémon Red or Blue and a panel appears beside the game: your party
live (nickname, species, level, HP bar, status, types), your badges, and — while you are in a
battle — the opponent's level, HP, stats and moves. **`d`** opens a detail view with moves and
PP, DVs, the stat experience the game keeps hidden, Hidden Power, and whether the Pokémon
**would be shiny if traded to Gen 2**. **`Tab`** cycles the party.

The focused Pokémon's sprite is decompressed straight out of *your* cartridge, so it works in
the overworld and not only in battle.

**And the moment a wild Pokémon appears**, a banner announces whether it would be shiny if
traded to Gen 2 — before you spend a Ball — with the DVs beside it so a near miss is legible.
Only *catchable* encounters get a verdict; a trainer's Pokémon has fixed DVs and cannot be
caught, so a verdict there would be noise.

## Capabilities: observe + read. That is the whole list.

It declares `OBSERVE | READ_MEMORY` and nothing else, so `Machine::poke` **refuses it
outright**. A tracker that can corrupt a save is worse than no tracker, and the guarantee is
the host's — enforced per call — rather than a comment's.

## History

The shape of the feature is the [Ironmon Tracker](https://github.com/besteon/Ironmon-Tracker)
(MIT, Lua, Gen 3): a script for BizHawk/mGBA that reads party and enemy structures out of
emulator memory each frame and draws a panel beside the game. This is the same idea built
*into* the emulator instead of bolted on through a scripting bridge — which removes the
tracker's hardest problem, because it reads **between frames** and never has to guess whether
the data it is looking at is mid-update. None of the Tracker's code is here; it informed
*which data earns screen space*.

It is also the plugin that shaped the contract. Two gaps it reported are why the API looks the
way it does: `on_event` receives the `Machine` alongside the event (so a plugin can read state
*in response to* one), and `Machine::peek_bank` reads an explicitly numbered ROM bank whatever
the game has mapped, without touching the bank registers.

## How it knows the addresses are right

Every address is derived from a verified build of the
[`pret/pokered`](https://github.com/pret/pokered) disassembly whose ROM SHA-1s match the retail
cartridges, and every constant in [`src/gen1.rs`](src/gen1.rs) carries its symbol name. Nothing
is quoted from a wiki. **Re-derive rather than edit by hand.**

Three traps that produce plausible-but-wrong data, all handled:

- Gen 1 species indices are **not** Pokédex numbers (`MonsterNames` is indexed by internal
  index, `BaseStats` by dex number, converted through `PokedexOrder` — Mew special-cased).
- Every HP and stat is **big-endian**.
- `party_struct` (44 bytes) and `battle_struct` (28) put level and stats at *different* offsets.

A cartridge that is not verified English Red or Blue is **refused with an explanation** — title
*and* header global checksum are checked — rather than displayed wrongly. Yellow
(`pret/pokeyellow`) and Gen 2 (`pret/pokecrystal`) are named as extensions, not pretended.

## Cost

Nothing is read per frame. The plugin samples every 6 emulated frames (~10 Hz — faster than a
human reads and far slower than the game runs) and **only while its panel is visible**
(`set_active`). Sprites, the only expensive part, are decompressed once per species and cached,
so a party that has not changed costs nothing at all. A session with no plugin attached pays
one `is_empty` test per frame.

## Layout

| file | what it holds |
|---|---|
| [`gen1.rs`](src/gen1.rs) | the address table, every entry cited by `pokered` symbol |
| [`rom.rs`](src/rom.rs) | cartridge identification and the tables located by signature |
| [`sprite.rs`](src/sprite.rs) | Gen 1 picture decompression, reimplemented from the documented format |
| [`derived.rs`](src/derived.rs) | shiny-if-traded, Hidden Power, stat experience, DV total |
| [`encounter.rs`](src/encounter.rs) | wild-encounter detection and the shiny banner |
| [`capture.rs`](src/capture.rs) | catch-chance arithmetic |
| [`text.rs`](src/text.rs) | the Gen 1 character encoding |
| [`lib.rs`](src/lib.rs) | the `Plugin` impl and the sampled snapshot the frontend draws |

## Tests

```bash
cargo test -p gameboy-plugin-trainer              # 58 unit tests, no cartridge needed
cargo test --release --test trainer               # the real-cartridge harness (skips without one)
```

The sprite decompressor is proved **byte-identical** to `pokered`'s own 2bpp output for 13
species covering every "Pics" bank and both special cases. The harness plays a *new* game to
the overworld, drives the START menu to SAVE so the **game** writes an internally consistent
battery file, edits a known party in, boots into it, and compares the panel against **the
game's own party menu** — `wTileMap` holds charmap codes directly, so decoding it reads back
what a player sees.

No game asset is committed to make any of this hermetic. A check that needs real graphics takes
them from a path you supply at run time (`POKERED_GFX=<dir>`) and skips when it is absent.

📖 **[Full design, address table and verification → `docs/trainer.md`](../../docs/trainer.md)**
· [the plugin contract](../../docs/plugins.md)

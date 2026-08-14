# `battle-control` — start any Gen 1 battle, and script it

**Pokémon Red and Blue decide to start a battle by reading one byte.** Write it and the engine
does the rest: the transition, the sprite, the music, the AI, the animations, the cries, the
experience. Nothing here simulates a battle — it *asks* for one.

```bash
cargo build --release --features plugin-battle-control
GB_PLUGINS=gen1-battle cargo run --release --example terminal -- ~/roms/blue.gb
#   B  start the selected battle      N  choose the next preset
```

> **The directory is `battle-control`; the name you type is `gen1-battle`.** The crate is named
> for what it does to any game; the plugin is named for the game it does it to, because that is
> what a user is choosing between. The feature is `plugin-battle-control`.

## The mechanism, and why it is not a trick

The overworld loop tests `wCurOpponent` on every iteration and, if it is not zero, calls
`NewBattle` → `InitBattle` (`home/overworld.asm`, `.checkForOpponent`). Below `OPP_ID_OFFSET`
(200) the byte is a species and the engine builds a wild encounter; at or above it, it is a
trainer class plus that offset and the engine reads that class's roster out of the cartridge.

This is exactly what the game's **own debug menu** does (`engine/debug/debug_menu.asm`, which
gives the player a Rhydon and then writes `RHYDON` to `wCurOpponent`). So the plugin writes
four bytes and lets the engine do everything else.

## What it guarantees about your save

Three things, in the order they matter:

1. **It never writes cartridge RAM.** A `.sav` is written from cartridge RAM (`$A000-$BFFF`).
   Every write goes through `BattlePlugin::write`, which refuses any address outside WRAM and
   HRAM ([`wram::is_writable`](src/wram.rs)) and *counts* the refusal. A party injected into
   WRAM can only reach a `.sav` if the player then walks into a Pokémon Center and saves —
   which is a decision, not an accident.
2. **Everything it writes, it puts back.** Before overwriting the party it snapshots the exact
   byte ranges it is about to change and restores them when the battle ends. A preset with no
   `player =` line never touches the party at all.
3. **Nothing fires by itself.** It is not attached unless named in `plugins = …` /
   `GB_PLUGINS`, it does nothing until armed, and while attached the frontend marks it `!` in
   the status panel — the write marker.

It holds `OBSERVE | READ_MEMORY | WRITE_MEMORY | CONTROL_INPUT`. `CONTROL_INPUT` is separate
from `WRITE_MEMORY` on purpose: pressing a button is something the game sanctions and a poke is
not.

## A preset is a text file

```text
[charizard-flamethrower]
player   = Charizard
level    = 50
moves    = Flamethrower, Slash, Fly, Earthquake

wild     = Pidgey
level    = 3

do = await-battle              # …and a script that plays it out
do = press-until b FIGHT
do = press-until a FLAMETHROWER
do = press-until a "used FLAMETHROWER"
do = press-until b fainted
do = await-overworld
```

`press-until` waits on **what the screen says**, not on a frame count — Gen 1 tile ids *are*
character codes, so the tilemap reads back as text. That is what makes a script robust against
animation length and emulation speed, and it is the same technique the trade harness uses.

## Reproducible, and it films itself

The same script from the same starting state produces **byte-identical frames** — proved by
running it three times and comparing a hash of *every* frame. It records through the existing
video path with no special handling:

```bash
cargo run --release --example gen1_battle -- --preset charizard-flamethrower --video - \
  | ffmpeg -f rawvideo -pix_fmt rgba -s 160x144 -framerate 59.7275 -i - out.mp4
```

A cinematic rendered by the actual game, not a video file.

## Which games

English Pokémon **Red and Blue** only. The cartridge is identified by title *and* header global
checksum, and then every address the plugin writes is **re-derived from the cartridge's own
machine code** (`wram::verify`) rather than trusted from a table.

## Layout

| file | what it holds |
|---|---|
| [`lib.rs`](src/lib.rs) | the `Plugin` impl, arming, the snapshot/restore |
| [`preset.rs`](src/preset.rs) | the preset file format and validation |
| [`script.rs`](src/script.rs) | the `do = …` script engine, driven by screen text |
| [`wram.rs`](src/wram.rs) | the writable-address guard and the address re-derivation |
| [`tests.rs`](src/tests.rs) | 50 unit tests against `terminalgb::testing::TestMachine` |

This crate is also the worked example of a plugin that **needs a real cartridge to test**: it
takes `terminalgb` as a *dev*-dependency with `default-features = false, features = ["testing"]`,
which gives it `TestMachine` without pulling a frontend into the graph.

```bash
cargo test -p terminalgb-plugin-battle-control
```

📖 **[How it works, what "reproducible" precisely means, and the traps →
`docs/gen1/battle-control.md`](../../docs/gen1/battle-control.md)**

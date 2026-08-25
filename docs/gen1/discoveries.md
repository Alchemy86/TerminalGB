# Discoveries — it lives at AtlasGB now

[← Gen 1 area](README.md) ·
**[AtlasGB: discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md)**

**Every fault, quirk and corrected belief this project found while driving a retail Pokémon
Blue cartridge, with the reasoning that found it.** It was built here and it is now its own
project's page:
**<https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md>**.

This page is a pointer, deliberately, and not a second copy — the same reason
[the memory atlas](atlas/README.md) moved first.

## Why it moved

A fault or a corrected belief about Pokémon Red and Blue is a fact about the game, and AtlasGB
exists specifically to document facts about the game with evidence and a permanent citation
slug. Keeping a second copy here would be exactly the "two sources of truth" this project has
already decided against.

**Every heading and slug in the new file is byte-identical to the old one, with one exception:**
*"A cartridge can tell there is no sound chip"* is an **emulator** fact — `Gameboy::new` leaving
the APU `None` is a fact about how this emulator is built, not about the cartridge — so it was
deliberately not migrated. It now lives in [`AGENTS.md`](../../AGENTS.md), beside the two other
independent rediscoveries of the same "never build a test machine without an APU" rule.

## Where to read it

| | |
|---|---|
| [AtlasGB: discoveries](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/discoveries.md) | every fault and corrected belief, with the reasoning trail and a permanent slug |
| [AtlasGB: sharp edges](https://github.com/Alchemy86/AtlasGB/tree/main/atlases/pokemon-rb/docs/sharp-edges.md) | the one-line warning form of the same findings |
| [`docs/gen1/sharp-edges.md`](sharp-edges.md) | what stayed here: this repository's own tooling traps |

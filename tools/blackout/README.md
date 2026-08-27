# The blackout reference — asking mGBA the same question

Gen 1 does not end a run when your last Pokémon faints. `HandleBlackOut`
(`home/overworld.asm`) fades the screen out, stops the music, halves your money
and warps you to `wLastBlackoutMap` — the last Pokémon Centre you healed at,
which before Viridian is Pallet Town. The screen comes back and you are standing
outside your own front door.

[`examples/blackout_probe.rs`](../../examples/blackout_probe.rs) measures that
on TerminalGB. This asks **mGBA** the same question from the same setup, so the
expected answer is not one TerminalGB supplied.

mGBA is MPL-2.0 — the same licence as this repository — and nothing here is
vendored: it is fetched, built and linked at run time, exactly as
[`tools/sameboy-oracle/`](../sameboy-oracle/README.md) and `tools/bgb-interop/`
do for their references. Not in CI, for the same reason those are not.

## Running it

```sh
tools/blackout/fetch-and-build.sh          # ~2 min, into target/mgba

cargo run --release --features image,plugin-battle-control,plugin-trainer \
    --example blackout_probe -- --rom "$HOME/roms/blue.gb"

target/mgba/mgba-blackout --rom "$HOME/roms/blue.gb" \
    --party target/blackout-probe/party.bin \
    --setup target/blackout-probe/setup.txt
```

The probe writes `party.bin` (the 404-byte party block, `wPartyCount` `$D163`
onwards) and `setup.txt` (the wild opponent to poke into `wCurOpponent`). mGBA
copies those bytes in and lets `OverworldLoop` start the battle itself — the
cartridge's own mechanism, the one its debug menu uses — so both emulators fight
the same Pokémon with the same party.

## What it measured

Pokémon Blue, a level 5 Charmander against a level 60 Rattata, from a brand-new
game in Red's bedroom (map 38), `wLastBlackoutMap` at its starting value of 0:

| | `wCurMap` | `wIsInBattle` | screen | `wAudioFadeOutControl` | frames |
|---|---|---|---|---|---|
| TerminalGB, **no sound chip** | 38, unchanged | `$FF` | 1 shade, never came back | `$FF` — spinning | never, 3000+ |
| TerminalGB, **with a sound chip** | **0 — Pallet Town** | `$00` | 4 shades | `$00` | 235 |
| **mGBA** | **0 — Pallet Town** | `$00` | came back | `$00` | 203 |

mGBA runs the cartridge as a Super Game Boy (Blue sets the SGB flag) where the
probe's `Model::Auto` gives a DMG, so its shade count and its handful of
`Unimplemented SGB command` lines are that and not the whiteout. The frame counts
differ for the same reason. **Where the player ends up does not.**

## Two traps in the compile line, both of which segfault rather than fail to build

Use `fetch-and-build.sh`. If you must hand-roll it:

1. **`mgba/flags.h` is not included by mGBA's own translation units.** It is
   generated for downstream users of an *installed* mGBA. Force-including it
   adds defines the library never saw (`USE_JSON_C`, `USE_LUA`), and
   `struct mCore` has members behind `#ifdef`s — so your translation unit and
   the library disagree about where `init` lives, and `core->init(core)` jumps
   to address zero.
2. **`ENABLE_DIRECTORIES` is on the library's command line and is *not* in
   `flags.h`**, and it guards a by-value `struct mDirectorySet` member near the
   top of `struct mCore`. Miss it and every function pointer after it is at the
   wrong offset.

Take the defines from `build/CMakeFiles/mgba.dir/flags.make` and pass them as
separate words — **this shell is zsh, which does not word-split an unquoted
`$VAR`**, so `cc $DEFS` hands the compiler one enormous argument and silently
applies none of them. That is the same trap `AGENTS.md` already records.

And call `mCoreInitConfig(core, NULL)` before the first `core->reset(core)`:
mGBA's cores read their options out of a config table on reset, and without it
`_GBCoreReset` dereferences an uninitialised hash table.

# Surfing Pikachu — extracted reference material

Source of truth: [`pret/pokeyellow`](https://github.com/pret/pokeyellow), a verified disassembly
of the retail Pokémon Yellow ROM (same methodology the `trainer` plugin uses — re-derive from
the disassembly, never guess from raw ROM bytes or a wiki). Pulled 2026-09-28.

**Provenance note:** this is Nintendo/Game Freak's copyrighted game logic and art, disassembled
by the pret community for personal/educational use. Fine to keep locally and build a private
terminal port from, per Aaron owning the cartridge/ROM. Not for redistribution or publishing the
extracted assets verbatim.

## What's here

| file | what |
|---|---|
| `surfing_pikachu.asm` | the whole minigame, 2877 lines, ~165 labelled routines |
| `data/surfing_pikachu_oam.asm` | sprite OAM layout data |
| `data/surfing_pikachu_frames.asm` | animation frame tables |
| `data/wram_layout.txt` | the RAM struct (`wSurfingMinigame*`) — state, HP, speed, score, distance |
| `gfx/pikachu_sprite_sheet.png` | Pikachu riding/jumping/landing/crashed sprite sheet (16×96, 2bpp) |
| `gfx/surfing_pikachu_1a/1b/1c.png` | wave/water background tiles |
| `gfx/surfing_pikachu_2.png` | more background tiles (beach, status bar) |
| `gfx/to_surf_rad.tilemap` | the "TO SURF RAD!" intro screen tilemap |
| `gfx/gfx_manifest.asm` | pret's own manifest of how these PNGs assemble into tile data |

## The game, as extracted

**State machine** (`wSurfingMinigamePikachuState`): Riding → Jumping → Landing → Crashed →
GameEnd → InitResults → Results. Each state has its own `SurfingMinigame_Update*Pikachu` update
routine, dispatched once per frame from `RunSurfingMinigameRoutine`.

**The wave**: a 128px-wide window scrolls under Pikachu; `SurfingMinigame_GetWaveDataPointers` /
`_GenerateBGMap` build the visible strip from one of **29 wave patterns**
(`SurfingMinigameWavePattern00`–`1C`) plus a flat "beach" pattern, chosen by
`_ChooseNextWaveSequence`. Pikachu's board angle (`wSurfingMinigameBoardAngleOffset`) is derived
each frame from the wave height under it (`_UpdateBoardAngle`), which is what actually reads as
"riding the wave" rather than sliding over flat ground.

**Input** (`SurfingMinigame_DPadAction` / `_TileInteraction`): Up speeds up
(`_SpeedUpPikachu`), Down/steep-wave-angle can trigger a jump (`_TryStartJump`) or a crash
depending on speed and board angle at the moment of the tile transition. A-button-timed jumps
while airborne chain into tricks, scored by `_CalculateAndAddRadnessFromStunt` into
`wSurfingMinigameRadnessMeter` / `RadnessScore` (consecutive tricks are worth more — a
combo system).

**Damage/end condition**: `wSurfingMinigamePikachuHP` (BCD) ticks down on crashes
(`_Deduct1HP`); hitting 0 or reaching the goal distance (`wSurfingMinigameDistance`, 3-byte
BCD) ends the run and rolls into the results screen (HP left + radness + total, each added to
the running score with its own little pause-and-tally animation — `_WriteHPLeftAndWait` etc.),
then checks `DidPlayerGetAHighScore`.

**Framerate trick**: `SurfingMinigame_UpdateLYOverrides` / `_InitScanlineOverrides` — the wave's
horizontal wobble is a per-scanline X-scroll effect (a classic GB "raster split"), not a sprite
or tile animation. That's the one piece of `SurfingPikachuMinigame` that doesn't translate
directly to a tile/sprite renderer and needs an explicit per-row offset in the port.

## Porting to Rust — assessment

This is **tractable**, not a token-shredder: ~165 small, single-purpose routines, no floating
point (everything's fixed-point/BCD, which maps cleanly to `u8`/`u16` + a BCD helper), no
dependency on the rest of the game's engine except joypad input and PRNG (`wSurfingMinigameWaveRandomValue`
— one LCG-ish byte, easy to reproduce). The art is already decoded PNGs, not compressed 2bpp
blobs to reverse — a straight `image` crate load away from being usable.

**Two different things "play it in the terminal" could mean** — worth deciding before building:

1. **Standalone reimplementation**: a from-scratch Rust port of this state machine + wave
   physics, rendered as its own terminal minigame (reusing the emulator's terminal-rendering
   code, but not the GB CPU/PPU at all). Doesn't need a ROM or save file to run once built;
   plays from any terminal, launched on its own.
2. **In-emulator plugin**: a `Plugin` (`CONTROL_INPUT` + `READ_MEMORY`/`WRITE_MEMORY`) that
   drives the *real* minigame inside the actual emulated ROM — e.g. warps you straight to it,
   or adds a practice/replay mode around the authentic code running on real Yellow. Needs the
   ROM every time; is literally the original, not a port.

They're not mutually exclusive later, but the first build should pick one. See conversation —
flagging this as the next decision point.

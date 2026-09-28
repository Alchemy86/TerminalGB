# What this port was built from

Source of truth: [`pret/pokeyellow`](https://github.com/pret/pokeyellow), a
verified disassembly of the retail Pokémon Yellow ROM. Everything in
[`src/`](src/) is cited against it by symbol name so a reviewer can check any
claim without taking one on trust.

The disassembly and the tile art are Nintendo and Game Freak's work, published
by the pret community for study. They are not in this repository and are not
ours to redistribute — see the licence boundary in the root
[`AGENTS.md`](../../AGENTS.md), which is explicit that game assets never enter
the tree even as a fixture. `.gitignore` keeps
`plugins/surfing-pikachu/reference/` out.

## Getting the material

```sh
git clone --depth 1 https://github.com/pret/pokeyellow
```

| what | where in pokeyellow |
|---|---|
| the minigame, ~165 routines | `engine/pikachu/surfing_pikachu.asm` |
| animation frame tables | `data/sprite_anims/surfing_pikachu_frames.asm` |
| sprite OAM layouts | `data/sprite_anims/surfing_pikachu_oam.asm` |
| the RAM struct | `ram/wram.asm`, the `wSurfingMinigame*` block |
| background tiles `$00`–`$4f` | `gfx/surfing_pikachu/surfing_pikachu_1a.png` |
| every object tile | `gfx/surfing_pikachu/surfing_pikachu_1b.png` |

To play with the real art, pass the directory holding the two PNGs:

```sh
cargo run --release -p terminalgb-surfing-pikachu -- --gfx path/to/pokeyellow/gfx/surfing_pikachu
```

## Where each routine went

| pokeyellow | here |
|---|---|
| `SurfingPikachuLoop` | `Game::step` |
| `RunSurfingMinigameRoutine` and its jump table | `Game::run_routine` |
| `SurfingMinigame_RunGame` | `Game::run_game` |
| `SurfingMinigameAnimatedObjectFn_Pikachu` and its state table | `Game::update_pikachu_object` |
| `_UpdateRidingPikachu` … `_UpdateResultsPikachu` | `Game::update_riding` … `update_results` |
| `_DPadAction`, `.StartTrick` | `Game::dpad_action`, `Game::start_trick` |
| `_TryStartJump`, `_GetSpeedDividedBy32` | `Game::try_start_jump`, `speed_divided_by_32` |
| `_UpdatePikachuHeight`, `_NTimesDE` | `Game::update_pikachu_height`, `arc_step`, `rise_step` |
| `_TileInteraction` | `Game::landing_for`, `tile_interaction` |
| `_UpdateSurfingFrame`, `_UpdateBoardAngle` | `Game::update_surfing_frame`, `update_board_angle` |
| `_SpeedUpPikachu`, `_ReduceSpeedBy64/128` | `Game::speed_up_pikachu`, `reduce_speed` |
| `_CalculateAndAddRadnessFromStunt`, `_AddRadness` | `Game::stunt_value`, `calculate_and_add_radness_from_stunt` |
| `_IncreaseRadnessMeter` | folded into `Game::start_trick` |
| `_Deduct1HP`, `_AddPointsToTotal` | `Game::deduct_1hp`, `bcd::add_saturating` |
| `_AddRemainingHPToTotal`, `_AddRadnessToTotal` | `Game::add_remaining_hp_to_total`, `add_radness_to_total` |
| `DidPlayerGetAHighScore` | `Game::did_player_get_a_high_score` |
| `_GetWaveDataPointers` and its 124-entry table | `wave::next_slice`, `wave::WAVE_FUNCTIONS` |
| `_ChooseNextWaveSequence`, `_WaveSequenceStarts` | entry 0 of `wave::next_slice`, `wave::SEQUENCE_STARTS` |
| `SurfingMinigameWavePattern00`–`1C`, `BeachPattern` | `wave::PATTERNS` |
| `_BGMetatileTable` | `wave::METATILES` |
| `_GenerateBGMap`, `_ScrollAndGenerateBGMap`, `_CoastAfterGoal` | `Game::generate_bg_map`, `scroll_and_generate_bg_map` |
| `_ReadBGMapBuffer` | `Game::read_bg_map_buffer` |
| `_SetPikachuHeight`, `_SpawnWaterSpray` | `Game::set_pikachu_height`, `spawn_water_spray` |
| `_UpdatePikachuDistance` | `Game::update_pikachu_distance` |
| `_UpdateLYOverrides`, `_InitScanlineOverrides` | `Game::update_ly_overrides`, `LY_SINE` |
| `SurfingPikachu_Sine`, `sine_table 32` | `game::sine`, `game::SINE` |
| `SurfingPikachu_GetJoypad_3FrameBuffer` | the counter in `Game::step`, `JOY_SAMPLE_RELOAD` |
| `_DrawResultsScreen`, `.PlaceTextbox`, `_WriteHPLeft`/`Radness`/`Total` | `Game::draw_results_screen`, `write_tiles`, `print_bcd16`, `game::text` |
| `SurfingPikachuOAMData` shapes | the `*_SHAPE` tables in `render.rs` |
| `_InitStaticSpriteLayout`, `_DrawStaticTilemapLayout` | `Game::objects`, `render::window_map` |
| `_MoveClouds`, `_MoveBannerToCenter`, `Fn_FlippingPika` | `Game::move_clouds`, `Game::tick_popups` |
| `ldpal` palettes, `get_monochrome_pal_val` | `render::BGP`, `OBP0`, `OBP1`, `SHADES` |
| `DMGPalToCGBPal`, `CGBBasePalettes`' `PAL_PIKACHUS_BEACH` | `render::colour`, `render::CGB_BEACH`, `render::bg_register` |

## Deliberately not ported

**`Random`.** `_RunGame` seeds the wave chooser from pokered's `Random`, which
folds `rDIV` — the hardware divider, i.e. how long the console has been running
— into a pair of running bytes. There is no `rDIV` here, and a course that
cannot be replayed cannot be tested, so [`rng.rs`](src/rng.rs) is a seeded
xorshift whose low byte stands in for it. What the byte then feeds is unchanged:
`(random - 1) & 7`, with zero meaning "stay on open water".

**Sound.** `SurfingMinigame_UpdateMusicTempo`, `_ResetMusicTempo`, the five
`SFX_SURFING_*` effects and the two Pikachu cries on the results screen.

**The intro.** `SurfingPikachuMinigameIntro` and
`DrawSurfingPikachuMinigameIntroBackground` need `beach_intro.tilemap`,
`title.tilemap`, `use_control_pad.tilemap` and `surfing_pikachu_1c.png`, which
were not in the extracted set. The title screen is this port's own.
`beach_outro.tilemap` is missing for the same reason, so the results screen's
beach is composed from the course's own beach metatiles.

**The Super Game Boy palette path.** `SurfingPikachuMinigame_SetBGPals` has a
`wOnSGB` branch that swaps in `$e4` and a different palette; the port always
takes the other one, which is what a Game Boy and a Game Boy Color both take.

**`hAutoBGTransfer`, `hVBlankCopy` and the redraw queue.** The port applies
their effects directly, with the one-frame lag that matters
(`_ReadBGMapBuffer`) reproduced explicitly and the rest — a column of tiles
written 160 pixels ahead of the viewport — applied immediately.

**Unreferenced routines.** `SurfingMinigame_ToggleStartFlag`,
`_AdvanceWaveFunctionFromA`, `_MaskCurrentAnimatedObject`, `SurfingPikachu_Cosine`,
and the `.GoalText` / `.Plus750Pts` / `.UnusedFrontPikachu` frames.

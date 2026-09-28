# `surfing-pikachu` — the minigame, ported

Pokémon Yellow's Surfing Pikachu, rebuilt in Rust from pret's disassembly and
played in a terminal. No ROM, no save file, no emulator: the state machine, the
wave generator, the jump arc, the scoring and the raster effect are all here.

```sh
cargo run --release -p terminalgb-surfing-pikachu
```

Drag or flick left and right while you are in the air to spin the board. Land
the right way up. That is the whole game.

```
plugins/surfing-pikachu/
├── src/
│   ├── game.rs    the state machine, the physics, the scoring
│   ├── wave.rs    29 wave patterns, a beach, and the chooser between them
│   ├── input.rs   gestures in, pad state out
│   ├── render.rs  160x144 of Game Boy picture, raster split included
│   ├── tiles.rs   the art: the real thing at run time, or ours
│   ├── bcd.rs     DAA, and the four-digit counters built on it
│   ├── fixed.rs   8.8 fixed point
│   └── rng.rs     the one thing that is not a port
├── examples/      probe (play a run under a policy), shot (one frame, as
│                  text or a PPM), bench (what a frame costs) — all headless
└── REFERENCE.md   what was read, cited by symbol name
```

## What the original's controls actually are

The summary this port was commissioned from describes a different game, and the
difference is the whole design of the input layer, so it is worth stating.
`grep -n 'PAD_' surfing_pikachu.asm` returns six lines. Select and Start outside
the run, A on the two end screens, and Left/Right in `_DPadAction`. That is all.

- **Nothing reads Up or Down.** `SurfingMinigame_SpeedUpPikachu` is called
  unconditionally, every riding frame. You accelerate by staying on your board.
- **Nothing triggers a jump.** `SurfingMinigame_TryStartJump` takes no input:
  cross a wave crest at 1.25 px/frame or better and you launch. What you choose
  is how fast you arrive.
- **A is never read during play.** Tricks are Left and Right, *held*, and only
  while airborne.
- **HP is a clock.** `SurfingMinigame_Deduct1HP` runs once a frame in the
  running state, unconditionally — 6000 frames, a hundred seconds. A wipeout
  costs you the 96 frames of its animation and resets your speed to a crawl.
  That is the damage. Reach section 24 before the clock runs out and you win.

So there are exactly three things for a gesture to mean, and `input.rs` maps
them: swipe or drag to spin, tap to confirm, long press to leave. There is
deliberately no accelerate gesture and no jump gesture. Adding one would change
what the game is; if one is ever wanted, that module is the only place it would
go.

## The art

Nothing in this repository is Nintendo's. Point `--gfx` at a directory holding
pret's extracted `surfing_pikachu_1a.png` and `surfing_pikachu_1b.png` and the
port draws the real tiles; without it, it draws a set written for this port and
says which it used on the title screen. The tile *ids* gameplay asks about mean
the same thing either way — `$06` is a rising slope, `$14` is a crest — so the
game is identical and only the skin changes. [`REFERENCE.md`](REFERENCE.md) says
where to get the files.

## The pieces that were interesting

**The raster split is vertical, not horizontal.**
`SurfingPikachuMinigame_LoadGFXAndLayout` writes `rSCY - $ff00` into
`hLCDCPointer`, so the HBlank handler rewrites the **vertical** scroll once per
scanline from a sine of amplitude two, and
`SurfingMinigame_UpdateLYOverrides` walks that buffer up one scanline a frame.
The water ripples up the screen. It is the one part of the game that cannot be
reimplemented as a sprite or a tile animation — every row of the picture samples
the background from a different place — and `render.rs` keeps it as a per-row
offset for exactly that reason.

**Every decision is made against a stale sample.**
`SurfingMinigame_ReadBGMapBuffer` does not read the tile under Pikachu; it
schedules a VBlank copy of it. So the tile that `_TryStartJump`,
`_UpdateSurfingFrame`, `_SetPikachuHeight` and `_TileInteraction` all branch on
is the one that was there *last* frame. Removing the lag changes when jumps fire
and what landings are worth, so the port keeps it.

**The wave scrolls at a constant 1.5 px/frame.** Speed does not scroll it
faster. What speed buys you is distance along the 24-section course, a louder
music tempo, and a bigger jump — `magnitude = speed / 32`, floored at 10.

**One negation in the jump arc is wrong, and it is in the arc everyone played.**
`SurfingMinigame_UpdatePikachuHeight` negates a 16-bit value a byte at a time:
`l = -l`, `h = ~h`. That is the true negation only when the low byte is
non-zero, and the value is `4 * a * a`, which is a multiple of 256 at a = 0, 8
and 16. On exactly those frames the ascent gets one extra pixel of lift.
Reproduced, not fixed.

**Landing upside down is always a wipeout.** Frame sets 8 to 14 are 1 to 7
flipped both ways, and `_TileInteraction` compares only 1 to 7 — everything else
falls through to the wipeout arm. That single fall-through is what makes
spinning a gamble, and it is what the scoring is paying for.

## Tests

```sh
cargo test -p terminalgb-surfing-pikachu     # 70 tests
```

The game rules are small and completely specified, so there is no excuse for
leaving them untested. The ones that earn their place:

| what it pins | why it would otherwise rot silently |
|---|---|
| `the_landing_table_matches_the_disassembly` | all four tile classes against all seven board angles, transcribed from `_TileInteraction`'s arms |
| `landing_upside_down_is_always_a_wipeout` | the fall-through above is invisible in the source and is the whole risk of the game |
| `the_scoring_table_matches_its_own_comment` | 50 / 150 / 350 / 180 / 500, against the comment in `_CalculateAndAddRadnessFromStunt` |
| `the_same_seed_plays_the_same_course` | a course that cannot be replayed cannot be debugged |
| `every_sequence_start_terminates_in_a_reset` | a sequence that never reset would repeat one wave to the horizon |
| `one_swipe_is_worth_exactly_one_spin` | drives the real machine; one frame short and every swipe is worth nothing |
| `one_swipe_returns_the_board_to_the_angle_it_started_at` | the property that makes a swipe free rather than a gamble |
| `daa_matches_decimal_over_valid_bcd` | all 10,000 pairs, both directions |
| `no_background_tile_relies_on_colour_index_one` | see below |
| `a_clean_run_reaches_the_goal_and_is_not_a_game_over` | four seeds, played end to end |
| `running_out_of_hp_is_a_game_over_and_scores_nothing` | spinning constantly really does kill you |

## Traps this cost

**`SWIPE_FRAMES` was one sample short and every swipe was worth nothing.** A
right-hand spin registers when `_DPadAction`'s counter reaches `$d` — fourteen
samples — and held input only reaches that counter every *other* frame, because
`SurfingPikachu_GetJoypad_3FrameBuffer` reloads its counter with 2. Fourteen
samples two frames apart, plus a frame of phase, is 28. At 26 the swipe expired
one sample early, silently, and looked exactly like a physics bug.

Note the disassembly's own comment on that routine says three frames. The reload
value is 2 and it is decremented once a frame, which gives two; two is also what
makes the scoring table's "3 or more flips" reachable inside one jump. This is
the one timing that has not been checked against a real cartridge.

**Colour index 1 is invisible in the background.** This game's `BGP` is `$d0`,
which paints index 0 and index 1 the *same white*. The first beach was drawn in
index 1 and rendered as bare sky — a tile that is present, correct and
completely absent from the picture. Nothing catches that but looking at it, so
`no_background_tile_relies_on_colour_index_one` now does.

**`ldpal` lists colour 3 first.** Reading it left to right as 0, 1, 2, 3 inverts
every palette in the game, and the result is plausible enough to keep going with:
the sky comes out black and the water comes out white. The check is that
`ldpal a, SHADE_BLACK, SHADE_DARK, SHADE_LIGHT, SHADE_WHITE` has to be `$e4`,
the normal Game Boy palette, because that is what the routine it appears in is
called.

**The fixed-point literals are Q8.8, and the disassembly does not say so.**
`ld a, 0.25` is `$40`, not `$4000`. It is provable from the code rather than
assumed: `LoadGFXAndLayout` writes `0.25` into the low byte of a 16-bit speed
and zero into the high byte, and the acceleration literal is commented as
1/128 per frame. Both only hold if one unit is 1/256.

## Measured

On one box, release build, in a 150x44 terminal: **16745 µs a frame against the
Game Boy's 16743 µs budget, with no deadline resyncs** — 59.72 frames a second
against the hardware's 59.73. Simulating a frame costs 66 µs and composing the
picture 688 µs, so 15.99 ms of every frame is spent idle. Re-measure before
quoting this; it belongs to that box and that terminal.

`cargo run --release -p terminalgb-surfing-pikachu --example bench` prints the
first two figures. `SURFING_PIKACHU_TRACE=1` prints the frame budget on exit,
which is how the resync count above was got: the first attempt at this
measurement said 52 fps, and the missing 12% was crossterm's
keyboard-enhancement query waiting out its timeout against a test harness that
never answers it — a startup cost, counted as if it were frame time.

## Playtest notes

Seeds 1, 7, 42 and 1234, each played to the end under three policies. `ride`
touches nothing, `oneflip` swipes once per jump, `spin` holds a direction for
the whole time it is airborne. `cargo run --release -p
terminalgb-surfing-pikachu --example probe -- <seed> <policy>` reproduces any
row.

| policy | finished | score | jumps | wipeouts | airborne |
|---|---|---|---|---|---|
| `ride` | 4 of 4 | 816–1046 | 22–31 | 7–9 | 26–32% |
| `oneflip` | 4 of 4 | 1566–2052 | 22–31 | 7–9 | 26–32% |
| `spin` | 1 of 4 | 0 or 2296 | 23–26 | 11–15 | 21–24% |

Riding it straight is nearly safe, because the board sets itself to the slope
it is crossing. Every point of danger is of your own making.

One swipe a jump is free. The jump and wipeout counts under `oneflip` are
*identical* to touching nothing — same seeds, same course, same crashes — and
the score roughly doubles, because a complete rotation puts the board back
where it launched. That is not luck, it is the shape of `_DPadAction`: fourteen
frame sets, stepped once per sample, and a trick registers before the
fourteenth. On seed 1 that is 21 landings each worth 50, which is exactly the
1050 points by which the total moves.

Holding the direction is the gamble. Wipeouts go from 7–9 to 11–15 and three of
the four seeds run the clock out before section 24, because a second spin
usually leaves the board part-way round and everything from frame set 8 upward
lands upside down.

The wave reads right. Crests arrive every few seconds, you can see one coming
and feel the board tip as the slope passes under it, and jumps fire at the lip
rather than at arbitrary moments. The vertical ripple is only two pixels and
does most of the work of making flat water look like water.

## Not done

- **No sound.** The original drives its music tempo from Pikachu's speed
  (`SurfingMinigame_UpdateMusicTempo`, five tempos over the speed range) and
  plays five sound effects. None of it is here.
- **No intro.** The original opens on a beach with "TO SURF RAD!" over it; those
  tilemaps were not in the extracted reference set, so the title screen is ours.
  The beach on the results screen is composed from the course's own beach
  metatiles for the same reason.
- **The animated-object VM is simplified.** Spray, splashes, score pops and
  banners keep their timings from `SurfingPikachuFrames` but are not run through
  the general animation machine the original shares with the rest of the game.
  None of them feeds back into the simulation.
- **It has not been diffed against a real cartridge.** Every claim here comes
  from the disassembly. Running the original in the emulator beside this and
  comparing frame for frame is the obvious next check, and the one that would
  settle `JOY_SAMPLE_RELOAD`.

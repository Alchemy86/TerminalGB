# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project aims to follow
[Semantic Versioning](https://semver.org/spec/v2.0.0.html) (pre-1.0: minor versions may
carry breaking changes).

This changelog begins where this repository diverged from upstream — the emulator did **not**
appear from nothing. Its foundation is **[raphamorim/gameboy](https://github.com/raphamorim/gameboy)**
by Raphael Amorim, itself closely modelled on **[mvdnes/rboy](https://github.com/mvdnes/rboy)**;
the entries below record only the changes made in this fork and do not reconstruct the upstream
history. See [Credits & lineage](README.md#credits--lineage).

## [Unreleased]

### Added
- **Music mode — your ROMs as a shelf of albums.** `terminal music` (or `M` from the ROM
  picker) turns the configured `rom_dir` into a library: **cover art taken from each game's
  own title screen**, the cartridge's header title, and its own music playing while you
  browse. `w` saves what you are hearing as a WAV, `c` cycles cover candidates, `g` leaves the
  shelf and plays the game properly. Behind the `music` cargo feature, so the embedding build
  (`--no-default-features`) carries none of it. Full write-up:
  [`docs/music-mode.md`](docs/music-mode.md).
  - **It says which mode it got.** A Game Boy cartridge has no "play track 5" interface, so
    what music mode does is boot it and listen — which gets the **title theme**, and the panel
    says exactly that. Whole soundtracks need a game's own sound test or its sound driver, and
    that per-game knowledge belongs in a plugin; music mode does not pretend to have it. The
    wording is a tested property, not a convention.
  - The cover is found by looking for the part of the boot that **stays** — a title screen is
    where a game waits for you — rather than the frame that looks busiest, which picked a fade
    for one game and the sea behind another's intro. Several candidates are kept because titles
    animate in, and each candidate carries the machine state at its own start, so playback and
    export begin where the picture does even for a game with a minute of intro.
  - **Music mode never opens a save file.** Every machine it builds is constructed with no
    cartridge path, so browsing a shelf cannot touch a `.sav`; pinned on both the probe and the
    playback path, and both proven to fail when the path is put back. No plugins are attached.
  - New config keys `music_dir` and `music_export_seconds`; new headless evidence tool
    `cargo run --release --example music_probe` (with `--demo`, which needs no ROM at all).
  - The WAV encoder is the chiptune renderer's `terminalgb_chiptune::wav`, deliberately shared
    so there is exactly one in the workspace.

### Changed
- **The project is now called TerminalGB.** It was already the name the product used — the
  terminal window title and the virtual camera's idle card both said `TerminalGB` — so this
  makes the codebase match what a user sees. The rename is mechanical and covers the crate
  (`gameboy` → `terminalgb`, so `use terminalgb::…` becomes `use terminalgb::…`), the plugin
  crates (`terminalgb-plugin-*` → `terminalgb-plugin-*`, and `crates/terminalgb-plugin-api/` →
  `crates/terminalgb-plugin-api/`), the MCP binary (`gameboy-mcp` → `terminalgb-mcp`) and the
  name that server reports to a client (`gameboy` → `terminalgb`), the generated FFI header
  (`gameboy.h` → `terminalgb.h`) and static library (`libgameboy.a` → `libterminalgb.a`), the
  wasm module (`gameboy.wasm` → `terminalgb.wasm`), the docs, and the user-visible strings
  (the desktop window title, the ROM picker's header, the quit prompt, the `terminalgb:`
  prefix on warnings and fault messages).
  - **Nothing you already have moves.** Save files (`<rom>.sav`), save states and the ROM
    directory are untouched. The XDG directories — `~/.config/gameboy/config.toml`,
    `~/.local/state/gameboy/` (recents, remembered volume, save states) and
    `~/Pictures/gameboy` / `~/Videos/gameboy` — are still read *and written* exactly where
    they are: every lookup prefers the new `terminalgb` name and falls back to the old one
    when the old one is what exists on disk (`screen::config::app_dir_in`). Move them
    yourself if you like; move all of them or none.
  - `$TERMINALGB_CELL_SCALE` is the new spelling of `$GAMEBOY_CELL_SCALE`; the old name is
    still honoured.
  - **The repository is still `Alchemy86/gameboy`** — clones, remotes, badges and permalinks
    all keep working. The emulated machine is still a Game Boy, so the `Gameboy` type and the
    `gameboy` module inside the crate keep their names.
  - The `gameboy` value of the `palette` config key (an alias for `green`) is unchanged.

### Added
- **Gen 1 shiny hunting, in parallel** (`plugins/gen1-shinyhunt/`,
  `examples/shiny_hunt.rs`, `plugins = gen1-shinyhunt`) — N headless emulators run their own
  encounter loop from one save state, read the DVs out of `wEnemyMonDVs` the instant the game
  rolls them, and all stop the moment one matches. The software answer to a hunter's four
  Nintendo DS consoles wired to one controller through an input splitter, and it wins for a
  reason unrelated to parallelism: **it never looks at a sprite**. **Gen 1 has no shiny
  Pokémon** — what is hunted is the spread that becomes shiny once traded to Gen 2, using the
  one implementation of that rule already in the tree (the trainer panel's), never a copy.
  Instances starting from identical bytes would be one instance wearing N hats, so they are
  decorrelated by varying input *timing* (Gen 1's `Random_` folds the free-running `rDIV` in on
  every call) and `--probe` proves it **in both directions** — decorrelated instances must
  differ, identically-seeded ones must be identical. **And divergence turned out not to be
  enough**: `Random_` builds both DV bytes from one `rDIV` sample, so `lo − hi` *is* that sample
  and reloading the same state before every roll pins every result to one thin band — **968 of
  the 65,536 spreads, measured**, which is why a 97,212-encounter hunt found nothing while
  reporting a healthy 81/s and a 1.7-minute ETA throughout. An attempt is therefore a *run* of
  encounters with a flee between them (the RNG drifts through the battle engine the way a real
  hunter's console does), which lifts coverage to ~6,150, and `--probe` now **measures and
  reports** coverage with a verdict rather than assuming it. **Measured on a 20-core box: 8–18
  encounters/s and 176–367x real time on 12 instances.** A hit preserves the save state, a report
  and the frame, and the driver replays the winning attempt in a fresh machine before it will
  call anything a hit. The plugin declares `OBSERVE | READ_MEMORY` and nothing else, so a
  hunter provably cannot fabricate its own result. See
  [`docs/shiny-hunt.md`](docs/shiny-hunt.md).
- **[The measured Game Boy](docs/measured/README.md)** — a documentation section built on the
  one thing [gbdev.io](https://gbdev.io) does not have: our measurements. Eight pages, each
  shaped *"Pan Docs says X → here is what we measured → the test that pins it → what it looks
  like when an emulator gets it wrong"*: a [scoreboard](docs/measured/scoreboard.md) with a
  named, evidenced reason for **every** row still red across all seven suites; the calibrated
  [APU constants](docs/measured/apu.md) with the ROM that pins each and the measured cost of
  moving it; the [two PPU engines](docs/measured/ppu.md) and the one row that keeps the fast
  one default; [timing](docs/measured/timing.md), [boot state](docs/measured/boot.md) and
  [cartridges](docs/measured/cartridges.md); and a
  [symptom → cause index](docs/measured/symptoms.md) over the whole conformance corpus,
  including what **no** test ROM can catch. It **cites and links** gbdev and Pan Docs for the
  hardware reference rather than reproducing any of it. Much of this material previously
  existed only inside a 320 KB agent-facing `AGENTS.md`.
  - Recorded there and worth stating plainly: **Pan Docs is CC0 1.0 Universal — public
    domain — not CC BY-SA.** Verified against `gbdev/pandocs`'s `LICENSE`, unchanged since
    January 2020. There is no copyleft hazard for this MPL-2.0 repository; the reason we do
    not reproduce it is editorial, not legal.
- **Super Game Boy support** (`src/sgb.rs`, `GB_MODEL=sgb` / `sgb2`) — the half of an SGB a
  cartridge can actually observe: the ICD2 command-packet receiver on `$FF00`, `MLT_REQ`
  multiplayer and the joypad index a game reads back, plus the SGB/SGB2 boot registers.
  **All three published GB Emulator Shootout SGB rows now pass**, pixel-exact against the
  shootout's own reference screenshots (`cpp/sgb-ext-test`,
  `samesuite/sgb/command_mlt_req`, `..._1_incrementing`) — of the eighteen emulators in that
  table only GSE passes all three; SameBoy passes one, bgb one, mGBA two. Mooneye
  `acceptance/boot_regs-sgb` and `-sgb2` pass too (Mooneye 86 → 88 of 103, SameSuite
  69 → 71 of 76, everything else unchanged). The **border, the SGB
  palettes and SGB sound are a documented non-goal**: they live on the SNES side, a Game Boy
  program cannot read them back, and no test ROM asserts them. `Model::Auto` never picks an
  SGB — it is an explicit choice, as it is in the shootout's own manifest. See
  [`docs/components/sgb.md`](docs/components/sgb.md).
- **The shootout's `cpp` group is gated** (`examples/cpp_suite.rs`,
  `testharness/cpp_baseline.txt`) — four published rows (three MBC3/RTC tests and
  `sgb-ext-test`) judged against their author's reference screenshots by shade rank, that
  nothing in CI was watching. **4/4.**
- **A second PPU: the per-dot pixel FIFO** (`src/gpu/fifo.rs`), selected with `GB_PPU=accurate`,
  the `ppu` config key or `Gameboy::set_render_mode`, and shown in the terminal Status panel's
  `PPU` row. It runs the real pipeline — OAM scan one sprite per two dots, BG/window fetcher,
  8-pixel FIFO, sprite fetch stalls, window restart, `SCX & 7` discard — with colour *indices*
  in the FIFO and the palette applied at the output, so a mid-line `BGP` write moves a boundary
  along a scanline instead of recolouring what is drawn. **Mode 3's length is derived rather
  than assumed** (172 dots plus `SCX & 7`), which wins Mooneye `acceptance/ppu/hblank_ly_scx_timing-GS`.
  The whole-scanline renderer stays the **default** and is unchanged; the two are a cost choice,
  not an appearance one — palettes and upscaling live above the PPU and are identical either
  way. Measured: 0.219 vs 0.463 ms/frame on dmg-acid2 (2.1x, 2.8 % of a frame, 36x real time),
  both pixel-exact against its reference image. Mealybug is **still 3/79**: several tests are
  one to two orders of magnitude closer and two collapsed, which is the calibration ahead, not
  a missing feature — the full accounting, including the two rows where the accurate engine is
  *behind* the default, is `docs/mealybug.md` §7.
- **`testharness/ppuengines.rs`** (`cargo test --release --test ppuengines`, in CI) holds the
  two engines against each other and against the clock, hermetically from the vendored MIT
  dmg-acid2 ROM: pixel-exact against its author's reference, frame-identical to the default
  engine on well-behaved ROMs, and a 4 ms/frame collapse detector. Nothing else in CI runs the
  non-default engine.
- **The DMG OAM-corruption bug is modelled** (`src/gpu/oambug.rs`). On a DMG, any bus cycle
  that puts an address in `$FE00-$FEFF` on the bus while the PPU is scanning OAM corrupts
  OAM — including cycles with no memory access behind them, because the CPU's 16-bit
  increment/decrement unit is wired to the address bus. **Blargg `oam_bug` 3/8 sub-tests →
  8/8 on a DMG**, taking the in-repo test-ROM suite from **33/35 to 34/35 rows**.
  `oam_bug@cgb` stays red on purpose: Game Boy Color silicon does not have the defect, even
  running monochrome software, so a real CGB fails that ROM too. Cost ~1.5 % on the suite's
  wall clock; no save-state format change. Reasoning, sub-test-by-sub-test:
  [`docs/conformance-notes.md`](docs/conformance-notes.md).
- **An AI agent can play the game, and narrate it for a stream** (`src/agent/`, `A`/`G` in the
  terminal frontend). Structured observation (map, coordinates, facing, the game's own warp
  table, the screen decoded as text, party and battle state), a bounded action vocabulary above
  raw presses (`walk`, `goto`, `advance_text`, `press`, `wait`), and a commentary panel written
  for a Twitch viewer arriving with no context — a standing goal line first, short history
  behind it. Full design, the OBS/Twitch recipe and the honest gaps:
  [`docs/claude-plays.md`](docs/claude-plays.md).
  - **Plan-then-execute**, because a model takes seconds and the console runs at 59.73 Hz: the
    agent returns a goal, one line of commentary and ≤12 actions, and local code carries them
    out over hundreds of frames. Measured: **~1 model call per 1,100 emulated frames.**
  - **The emulator never blocks on the agent** — drivers are polled, never awaited; the
    subprocess driver runs its child on another thread. **0.24 ms/frame with a driver that never
    answers**, asserted in CI.
  - **Hard caps in the host**: calls, tokens, spend and wall clock, plus a circuit breaker and
    escalating stuck-detection. A runaway loop is not possible.
  - **The driver is swappable** — `ScriptedDriver` (deterministic, free, used throughout the
    tests), `RandomDriver`, and `CommandDriver` (any program: state on stdin, plan on stdout).
    Two adapters ship in `tools/claude_plays/`; no HTTP client was added to the crate.
  - **Everything the panel can render is treated as public and permanent**: driver failures reach
    a viewer as a category, and every line is scrubbed of credentials, paths, URLs and escapes.
- **`Machine::press`, a scheduled level on the battle crew's `Capability::CONTROL_INPUT`**
  (`crate::input::InputQueue`). Presses are scheduled in *frames* and applied once per emulated
  frame before the frame runs, so an AI driver, a scripted test and a battle-control script
  cannot invent three different definitions of "a press" — same capability and same latch as
  `set_button`, which stays the raw hold/release for anything genuinely thinking frame by frame.
  A plugin that may press buttons is marked `>` in the status panel and `>>` once it has, beside
  the existing `!` / `!!` for memory.
- **`testharness/agent.rs`** — the whole stack proved in CI against a **hand-assembled
  Gen-1-shaped cartridge** (no commercial ROM, no game data): observation, the executor, the
  budget cap, the circuit breaker and stuck detection.
- **`examples/claude_plays.rs`** — headless runner for scripted, random or real-model runs, with
  screenshots, an NDJSON decision log and a printed run summary.
- **`docs/gen1/agent-play.md`** filled in — the page the battle-control crew reserved for this
  work, answering its seven questions and recording the Gen 1 facts an agent needs that a script
  does not: the naming screen wants START, a door fires on the step *through* it, and
  `wCurMapHeight`/`wCurMapWidth` are the real "am I in control?" test because `wCurMap` already
  reads the bedroom during Oak's speech.
- **`agent` object in the telemetry record** — phase, decisions, calls, failures, tokens, spend,
  budget fraction and halt reason, beside `speed_pct` so the invariant that a *thinking* agent
  costs the emulator nothing is checkable. A field addition, so no `SCHEMA_VERSION` bump.
- **A volume control — and it starts quieter than it used to.** There was none: the only lever
  was the desktop mixer. **`,`** / **`.`** move the level a rung, **`m`** mutes and unmutes back
  to the level you were at (never to full). The level is a row on the Status panel, mute is
  called out on the top bar, and both the keys and the "playback only" caveat are in the
  Controls list and the `?` overlay. A **`volume`** config key (`0`–`100`, or `off`) sets where
  a fresh install starts — default **50 %**, −12 dB, because full scale was the complaint —
  and whatever you last set with the keys is remembered across sessions in
  `${XDG_STATE_HOME}/gameboy/volume`; editing the config key overrides the remembered value,
  so neither expression of intent is silently ignored. The desktop frontend gets the same three
  keys, with the level in its window title.
  **The attenuation is applied to the samples handed to the sound card, never to the Game Boy's
  own master volume (NR50)** — NR50 is emulated behaviour the accuracy suites measure, so
  turning it down to quieten the room would change what the emulator models. It lives in the
  cpal device callback, so emulation is bit-identical at every setting (`dmg_sound` 12/12 and
  SameSuite 61/76 before and after, every baseline unchanged) and the frame path is untouched
  (`emulate_ms` with a real device 0.236 → 0.235). Measured through the real sound card: the
  ladder tracks its `(level/100)²` taper to three decimal places, and the crest factor and
  harmonic ratio are constant across it — the waveform is scaled, not degraded. `examples/
  audio_volume.rs` walks the ladder so it can be judged by ear. Full write-up:
  [`docs/audio.md`](docs/audio.md) §6b.

### Fixed
- **CI ran no plugin crate's tests at all.** The root `Cargo.toml` is both a `[package]` and a
  `[workspace]`, so a bare `cargo test` covers the `gameboy` package only — 263 tests across
  `plugins/*` and `crates/*` were invisible. The CI step is `cargo test --release --workspace`
  now.
- **Two documentation claims that later measurement in this same repository had already
  contradicted.** `docs/testing.md` called a cycle-stepped PPU *"a structural commitment the
  project has not made"* thirty lines above its own description of the gated pixel-FIFO
  engine; and `docs/components/ppu.md`'s "Honest approximations" described fixed mode 3,
  coarse line stepping and whole-scanline rendering without qualification, while §4.0 of the
  same page documents the per-dot engine that does all three properly. Both now say *"this is
  the default engine"* and link to the measured trade. Also fixed two broken in-page anchors
  in `docs/testing.md` — `make docs-check` verifies file and line targets but not `#anchor`
  fragments, so those had been silently dead.
- **The DMG boot ROM's leftover Nintendo-logo tilemap is written** (`$9904`-`$990F`,
  `$9924`-`$992F`, `$9910`). A ROM that draws on part of the screen and never clears the
  tilemap renders its own tiles in rows 8 and 9 on hardware, and rendered nothing here — a
  *whole-screen* difference under a screenshot-comparing suite, because with one shade fewer
  on screen every white pixel ranks differently. It is why both SameSuite `sgb/` ROMs could
  report their own verdict as `PASS` while failing the shootout's image compare, and it is a
  boot-state gap rather than an SGB one.
- **A channel-1 re-trigger no longer loses the note when a frequency sweep is running.**
  A trigger reloads the sweep unit's shadow register from the frequency registers, and that
  reload takes three M-cycles: for 12 T-cycles afterwards the sweep's overflow check reads
  the shadow as **zero**. We reloaded it instantly, so a 128 Hz sweep event landing just
  after a re-trigger saw the new frequency and disabled the channel — a music driver that
  sweeps *and* re-triggers on the beat silently lost notes. The check's addend is now also
  latched when the check is **armed** rather than when it fires, so an NR10 shift written
  inside the window cannot change it. **SameSuite `channel_1_sweep_restart_2` FAIL → PASS
  (68/76 → 69/76, APU half 63/69 → 64/69)**; Mooneye, Mealybug, `testroms`, `audiocapture` (`dmg_sound` still 12/12,
  every hash unchanged), `savestate`, `battery` and `trainer` all byte-identical.
  `savestate::FORMAT_VERSION` 12 → 13 (the addend and the hold are live machine state).
  Why `channel_1_sweep_restart`'s last cell is deliberately still red — it is a square
  re-trigger constant, not the sweep unit, and the four rows that pinning it would cost are
  measured: [`docs/conformance-notes.md`](docs/conformance-notes.md).
- **The virtual camera is 16:9, so a video call stops slicing the top and bottom off the game.**
  Reported as a crop and diagnosed as our scaler filling instead of fitting; it is neither.
  `fit_size`/`fit_maps` have letterboxed correctly since the feature landed — at 640×480 they
  put the game at exactly 480×432 with 80/24 bars, and an `ffmpeg -f v4l2` grab off the device
  shows the whole picture. **The crop happens in the consumer**: a video-call client lays a
  camera out in a 16:9 tile and fills it, losing 12.5 % off the top and 12.5 % off the bottom.
  Measured in real Chrome against the real device, `getUserMedia` reports `resizeMode: "none"`
  and delivers the whole frame, so nothing at this end can detect it and no amount of better
  letterboxing helps — the consumer crops the bars *and* the picture. A camera's only lever is
  to be the shape of the tile, so `v4l2::DEFAULT_SIZE` is now **1280×720**, where the game
  happens to land at an exact ×5 filling the full height. An explicit `video_size` is still
  honoured, with a warning naming the consequence. Cost 1.83 ms/frame against 0.77 at 640×480,
  paid only while a stream is live; `768x432` is the cheap 16:9 fallback. Reasoning, both
  reproductions and the conformance record:
  [`docs/video-out.md`](docs/video-out.md) §4a.
- **The idle card no longer makes the feed jump.** It was drawn edge to edge while the game was
  letterboxed, so pressing `v` appeared to grow and shrink the picture. It is now drawn into
  the game's own rectangle. `examples/idle_card.rs` renders through the real stream path
  (`--game` for the comparison), rather than drawing the card into a buffer nothing ships.
- **Nearest-neighbour fitting samples pixel centres, not left edges.** Identical for integer
  magnification — the ×3/×5 camera path is unchanged — but a frame *smaller* than the source
  used to lose its last row and column and sit half a pixel up and left.
- **The mapper is chosen from the cartridge header again, with or without a save path.**
  `get_mbc` short-circuited to an **MBC3 for every cartridge** whenever no filepath was
  given — which is *always* the case for the WebAssembly build, the FFI build, and any
  embedder calling `Gameboy::new(rom, None)`. An MBC1 or MBC5 game was mis-banked from its
  first bank switch, and a ROM-only cartridge gained a bank register it does not have. The
  save path is now purely about persistence: every `MBCx::new` takes `Option<PathBuf>`.
  Pinned by `mbc::test::the_mapper_is_chosen_from_the_header_with_or_without_a_save_path`
  (proved to fail against the old code). **No test ROM in the suite catches this** — the
  harnesses all run with a path — which is itself a conformance finding.
- **A malformed cartridge is an error, not a process abort.** An image shorter than the
  header (`0x0150` bytes) indexed out of bounds; with `[profile.release] panic = 'abort'`
  that takes the host process down with it. `get_mbc` refuses it, and the new
  `Gameboy::try_new_with_model` / `Cpu::try_new_with_model` return the reason instead of
  panicking — the constructors to use for bytes a user handed you.
- **APU corner cases: SameSuite 50/76 → 57/76.** Three independent accuracy bugs, none of
  which any Blargg sound test can see (`dmg_sound` was and stayed 12/12 throughout).
  (1) *Frequency sweep.* The overflow check that kills a sweeping channel ran the instant
  the sweep wrote its new frequency; on hardware the shifter takes `shift + 1` M-cycles and
  the check lands at the end of that, so a swept note was clipped short at its top end
  (`channel_1_sweep`). Clearing NR10's shift field now **parks** a calculation already in
  flight instead of letting it fire. (2) *Volume envelope.* Its period counter is
  free-running and the step it schedules is carried by a separate latch acted on at the next
  512 Hz DIV-APU event — so a game that enables the envelope by writing NRx2 mid-note (the
  standard volume-ramp trick, since only eight envelope speeds exist) now ramps at all, and
  every later step lands on time instead of a 64 Hz period late
  (`channel_{1,2}_nrx2_glitch`, `channel_{1,2}_nrx2_speed_change`). (3) *Noise restart.*
  Re-triggering a channel-4 that is still playing — every drum roll and hi-hat — started one
  LFSR sample early, so the noise burst was a different sequence of bits from the one
  hardware plays (`channel_4_lfsr_restart`, `_fast`). No other suite moved (Mooneye
  `acceptance` 53/75, Mealybug 3/79, in-repo `testroms` 33/35 unchanged) and the
  audio-attached frame cost is inside its own measurement noise. Five of the remaining
  SameSuite failures assert **CGB revisions ≤ B** and are ROMs a CGB-E emulator should fail;
  the reasons for every remaining red row are recorded in
  [`docs/conformance-notes.md`](docs/conformance-notes.md) §10-13.
- **You can leave the Cable Club again.** The Gen 1 link partner answered the trade menu's
  cancel nybble `$6F` in only one of its six protocol phases. In every other phase it replied
  `$60` (or, from the confirmation phase, `$62` — silently counting a trade that never
  happened), and since `pokered`'s `.cancelMenuItem_APressed` only returns the player to the
  room when that round comes back `$F`, the player was sealed in the Trade Center: the game kept
  rendering and the clock kept running, but pressing A on CANCEL did nothing, for ever
  (measured on the retail cartridge: 2,475 presses). The Trade Center has no warp events at
  all, so that screen is the room's only exit. `$6F` is now first-class in every phase except
  the middle of a section exchange, where it is party data, and it returns the partner to
  "connected" with the exchange scratch cleared so a second trade — or simply going back in —
  works. `examples/gen1_trade.rs` now drives three console visits (trade, cancel-only, trade)
  and `--quick-load` reproduces the original failure by resetting plugin state mid-session the
  way the terminal frontend's F5/F8 keys do. Full record in
  [`docs/conformance-notes.md`](docs/conformance-notes.md).
- **The terminal frontend runs in ordinary terminals again.** It had started refusing to
  launch — one error line, exit 1, nothing drawn — on any terminal without the kitty keyboard
  protocol, which includes GNOME Terminal / Ptyxis, Konsole and xterm. A missing *input
  nicety* now costs only that nicety: without the protocol the frontend falls back to
  auto-repeat-driven key holds, prints **one** line saying what is degraded and how to get the
  better experience, and plays. The `Input` row in the status panel names the live mode. The
  only refusal left is neither stdout nor stdin being a terminal. Verified in Ptyxis (99.98 %
  speed, half-blocks) and in WezTerm both with and without `enable_kitty_keyboard`; a tap is
  still exactly one tile and menus still step once per press, while holding a direction takes
  one step, pauses for the OS auto-repeat delay, then walks continuously. Diagonals and
  sustained two-button holds need the protocol and are not claimed without it.
  **`testharness/basicterm.rs`** is the regression test and closes a real coverage gap: every
  other harness builds a `Gameboy` directly and never enters the frontend, so nothing in the
  suite could see the entry path at all. It allocates a pty, *is* the terminal on the far end,
  and runs two scenarios differing in a single reply — answer the kitty keyboard query and the
  frontend must report `kitty`, ignore it and it must report `basic` — then checks it draws,
  takes keys, does not let auto-repeat strobe a one-shot control key, and quits cleanly. No
  window, no synthetic desktop input; it runs under a plain `cargo test`.
- **Audio no longer paces the emulator.** With `--features audio` one emulated frame cost
  **17.93 ms** against a 16.743 ms budget (0.29 ms without the feature), so the terminal
  frontend sat permanently in catch-up and displayed **6 fps**. Profiling showed **97.6 % of
  that frame was blocked, not executing**: `AudioPlayer::play` runs inside `Cpu::do_cycle`,
  and the cpal player *waited* at its queue's high watermark to make the sound card the master
  clock, fighting the frontend's own 59.7275 Hz deadline accumulator. The hand-off is now
  non-blocking — above the watermark the oldest samples are discarded down to the cushion,
  which is the policy fast-forward already documented. `emulate_ms` **17.93 → 0.34** against a
  real device; end-to-end latency **44.8 → 36.3 ms**; the 120 s soak goes FAIL → PASS (0
  underruns, 0 drops); `dmg_sound` unchanged at 12/12 and every audio-capture hash unchanged.
  Full write-up and conformance record: [`docs/audio.md`](docs/audio.md) §4c.

### Changed
- Save-state `FORMAT_VERSION` is now **7** (the sweep unit's pending-calculation timer and
  the volume envelope's pending-step latch are machine state and are serialised).
- **The terminal window is titled `TerminalGB`** (plus the cartridge once one is loaded, with
  the prefix held stable). This is the emulator-side half of the supported route for getting
  the side panels into a stream — an OBS Window Capture source finds its window by title. The
  `?` overlay now also states plainly that the camera feed is the game picture only, and points
  at [`docs/stream-panel-routes.md`](docs/stream-panel-routes.md), which gained a scene recipe.
  The recommendation there (OBS window capture → OBS Virtual Camera, rather than building a
  text compositor) was re-checked and stands.

### Added
- **The emulator core is genuinely embeddable — `--no-default-features` is now a real build.**
  It did not compile before: `screen::desktop` and `screen::tui` were unconditional, so the
  crate could not be taken without `glutin`, `gl`, `ratatui`, `ratatui-image` and `image`.
  The frontends are now behind `desktop` and `tui` features (both still on by default, so
  nothing about the existing binaries changes). The result: **193 crates on the default
  build, 1 with no features** (`blip_buf`, the APU resampler). Full write-up in the README's
  new **"Embedding the core"** section — the API, the feature table, and an honest list of
  what is and is not portable.
- **`examples/headless.rs`** — the claim, executable: runs a cartridge with no window, no
  terminal, no audio device, no image codec and no `$HOME`, takes input, round-trips a save
  state and prints the frame as ASCII. `make embed` runs it with `HOME`, `DISPLAY` and
  `WAYLAND_DISPLAY` unset, and a new **`embed` CI job** runs it on a runner with no system
  libraries installed, asserts the dependency tree is exactly one crate, and cross-checks the
  core for `wasm32-unknown-unknown`, `x86_64-pc-windows-gnu` and `aarch64-unknown-linux-gnu`.
- **`Gameboy::new_with_model`** — the reproducible constructor. `Gameboy::new` resolves the
  console via `Model::Auto`, which consults `$GB_MODEL`; an embedder that cannot vouch for its
  process environment now has a way to say what machine it wants.
- **`Gameboy::try_new_with_model` / `Cpu::try_new_with_model`** — construction that returns
  `Err` rather than aborting on a cartridge that cannot be mapped.
- **`RenderMode::Headless`** — always available, and now the default: it hands the loop back
  to the caller. `RenderMode::Desktop`/`::Terminal` exist only with their features.
- **`testharness/audioperf.rs`** — emulation speed *with a host audio player attached*, the
  check nothing in CI performed (every benchmark ran headless with no player, which is why the
  above shipped). Needs the `audio` feature but no sound card; `make test-audio-perf`.
- **`examples/audio_frame_cost.rs`** — per-frame wall time beside thread CPU time against a
  real device, so "emulation got slow" can be answered rather than guessed.
- **Shiny-if-traded verdict for wild encounters** (terminal frontend, Pokémon Red/Blue). The
  moment a *catchable* wild Pokémon appears, a row across the top of the terminal names it,
  prints its DVs, and says whether it **would be shiny if traded to Gen 2** — while a Ball is
  still unspent. A shiny verdict fills the row, raises a toast and rings the terminal bell
  once; an ordinary encounter gets the same row quietly, with the reason it is not shiny. It
  works with the trainer panel closed (the plugin peeks one byte at ~10 Hz), the verdict is
  latched so it persists for the encounter, and trainer battles and the old man's catching
  demonstration are excluded — neither Pokémon can be caught. **Gen 1 has no shiny Pokémon;**
  every string says "would be shiny if traded", at every terminal width, and a test asserts it.
  Rule cited to `CheckShininess` (`pokecrystal/engine/gfx/color.asm`); the DV roll and its
  survival through capture to `LoadEnemyMonData` and `_AddPartyMon` (`pret/pokered`). New
  config key `shiny_alert` (`on` / `quiet` / `off`). Verified against a real encounter under
  **both** `GB_MODEL=dmg` and `GB_MODEL=cgb` — the verdict reads fixed WRAM addresses and never
  the console model, though `BattleRandom` is seeded differently so the particular DVs a
  scripted run rolls are not reproducible across models. See
  [`docs/trainer.md`](docs/trainer.md#the-wild-encounter-banner).
- Trainer panel: the opposing Pokémon's DVs are now shown in every battle, and a Safari Zone
  or old-man-tutorial battle is labelled as such.
- Trainer harness: a fifth check that walks a **brand-new game** out of the bedroom, through
  Pallet Town and onto Route 1, paces the grass until a wild battle starts, and compares the
  latched verdict against `wEnemyMonDVs` read independently with the Gen 2 rule re-implemented
  in the harness. It **fails when it meets fewer encounters than it asked for** rather than
  passing with a smaller number, and the walk nudges itself free (and says so) if it stalls —
  which caught a real wedge where the post-battle clean-up pressed A one frame late, opened the
  START menu, and spent the rest of its budget pressing a direction key into it.
- Repository furniture: this `CHANGELOG.md`, `CONTRIBUTING.md`, and GitHub issue / pull-request
  templates.
- README badges (CI status, licence, Rust version) and a "Credits & lineage" section naming
  the fork parent and rboy.

### Fixed
- Trainer view: `wIsInBattle == $FF` ("the player blacked out", `home/overworld.asm`
  `.allPokemonFainted`) was read as a *trainer battle*, so blacking out put a battle panel —
  opponent, level, HP, stats, moves, all stale — on screen while the player stood in the
  overworld. `ram/wram.asm` documents four values above the label; the reader had a catch-all
  arm that swallowed the `-1` sentinel. It now reads as no battle, as does every value the game
  never writes, and the test iterates the whole byte range rather than the named cases. No test
  ROM catches this and none can — the cartridge was correct; the defect was in a consumer of
  emulated state. Recorded in [`docs/conformance-notes.md`](docs/conformance-notes.md).
- Terminal Status panel: the box was sized `13 + plugins.len()` against a twelve-line list, so
  with two plugins attached the second one's row was clipped away entirely — a plugin could be
  active and invisible, which `docs/plugins.md` §4 promises cannot happen
  (`an_attached_plugin_is_visible_in_the_rendered_status_panel` was failing on `main`). The box
  is now sized from its own line list, so a newly-added row — `Audio` most recently — can never
  silently cost a plugin its row again; the worst case (two plugins plus a live encounter) is
  pinned by a test.

### Changed
- **`savestate::FORMAT_VERSION` 4 → 5.** A machine built with no save path now writes the
  block of the mapper its header asks for rather than an MBC3's, so a v4 state from the FFI
  or wasm build would be mis-parsed. States from the desktop and terminal frontends were
  unaffected by that fix, but the container version is global — existing slots are rejected
  with a clear error rather than loaded wrongly.
- `Cargo.toml` metadata: `repository` now points at this fork, authorship represents both the
  original author and this fork's contributors, `version` bumped to `0.2.0`, and the
  description updated.

### Removed
- `.github/FUNDING.yml`, which pointed sponsorship at the upstream author from this fork (see
  the notes in the pull request).

## [0.2.0] - 2026-08-13

A large accuracy, audio, rendering, and tooling pass on top of the fork baseline.

### Added
- **Headless test-ROM correctness harness** (`testharness/testroms.rs`): Blargg serial-string
  capture + dmg-acid2/Blargg-screen frame hashing, gated in CI against a checked-in
  known-failures baseline (`testharness/baseline.txt`); red only on a change from baseline.
- **Battery saves**: cartridge RAM now persists to a `.sav` file *while the game runs*
  (atomically, not only on a clean exit), using the ecosystem `.sav` format, with a durability
  harness (`testharness/battery.rs`).
- **Save states**: a versioned, ROM-checked native `GBSS` format in the core
  (`Gameboy::save_state`/`load_state`), with a byte-identical round-trip proof harness
  (`testharness/savestate.rs`), exposed as quick-save/-load slots in the terminal frontend.
- **Pixel-art upscaler** (`src/upscale/`): runtime-selectable `nearest`/`scale2x`/`mmpx`/`lcd`,
  dependency-free and wasm/FFI-safe, wired into the desktop, terminal, and web frontends.
- **Audio pipeline**: an offline audio-capture/analysis harness (`testharness/audiocapture.rs`)
  and a measured queue-policy/pacing implementation for real host playback.
- **Telemetry**: NDJSON run data (`src/telemetry.rs`) mirroring the stats overlay, off by
  default (`GB_TELEMETRY=<path>`); a one-shot `render_bench --json` summary mode for CI.
- **Terminal frontend**: a proper `ratatui` TUI with a ROM picker, recents, in-app ROM
  switching, per-session audio, stats overlay, speed ladder, and save slots.
- **Documentation**: per-component reference pages (`docs/components/`), a top-level code map
  with generated GitHub links (`docs/codemap.md` + `make docs-links`/`docs-check`), and design
  docs for testing, audio, save states, upscaling, terminal rendering, telemetry, debugging,
  and the planned conformance tool.

### Changed
- **Audio is now off by default**: `cpal` is an optional dependency behind the `audio` feature,
  so a fresh clone builds/tests/runs with no system audio library (no ALSA on Linux).
- **Timer stepped per CPU memory access** and the **APU clocked per access**, so reads/writes
  land on the correct sub-instruction cycle.
- **Terminal render path**: transmitted image sized by the frame, not the window; emulation
  decoupled from rendering; correct 59.7275 Hz pacing.
- **Desktop rendering**: crisp integer-scaled, aspect-correct (10:9) output.
- Removed dead code (`src/gpu/old_gpu.rs`).

### Fixed
- **Accuracy**: Blargg `mem_timing`/`mem_timing-2` pass (per-access timer); the DMG/CGB
  **HALT bug** is modelled so `halt_bug` passes; `dmg_sound` improved 1/12 → 12/12 via
  per-access APU clocking and the DMG wave-RAM access window.
- **Audio**: fixed "fast and rough" playback (frame pacing + device negotiation) and cut
  end-to-end latency from ~299 ms to ~53 ms with no underruns.
- **Terminal HiDPI**: corrected the "tiny picture" on scaled Wayland displays via a
  `cell_scale` configuration key.
- **MBC**: three crashes fixed alongside adopting the ecosystem `.sav` format.

[Unreleased]: https://github.com/Alchemy86/gameboy/compare/v0.2.0...HEAD
[0.2.0]: https://github.com/Alchemy86/gameboy/releases/tag/v0.2.0

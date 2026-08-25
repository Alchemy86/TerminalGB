# TerminalGB documentation

Every page in this directory, grouped, with one line on what it is for.

**This is the index; the [top-level README](../README.md) is the shop window.**
Its *"Every page, and what it is for"* section is a curated selection with long
descriptions, aimed at a visitor deciding whether to read anything at all. This
one is complete and terse, for someone already inside who needs to find a
specific thing. They are allowed to describe a page differently. They are not
allowed to disagree about which pages exist —
[`docs/tools/docs_index.py`](tools/docs_index.py) checks that on every
`make docs-check`, because **a page in no index is a page nobody reads again**,
and that had quietly happened to more than half of this directory.

---

## Start here

| page | what it is |
|---|---|
| [what an emulator actually is](emulation/README.md) | the concepts, for a reader with no hardware background — start here if the rest looks like jargon |
| [quick start](quickstart.md) | GitHub page to running emulator: the Codespace, the container, `terminalgb-run` |
| [code map](codemap.md) | one orientation page — a frame through the system, a memory access CPU → MMU → device, where the frontends attach |
| [where it stands](status.md) | the scoreboard: every suite, the abilities, the honest gaps, the roadmap |
| [every control](controls.md) | the full written key list, each row linking to the page that explains that key — generated from the same table the in-app panel renders |
| [pitfalls & gotchas](pitfalls/README.md) | symptom-first index of the traps that cost us hours, written for someone who just arrived from a search |

## Understanding the machine

| page | what it is |
|---|---|
| [what an emulator actually is](emulation/README.md) | the loop, the three rates, and what the job actually is |
| [the machine, chip by chip](emulation/the-machine.md) | CPU, PPU, APU, cartridge, link port — what each one is and why it exists |
| [where "convenient" stops and "exact" begins](emulation/accuracy.md) | the three cracks in the naive emulator loop, and what each one breaks |
| [how we know it is right](emulation/test-roms.md) | test ROMs as an oracle, and a gallery of dmg-acid2 faces each deformed by one broken behaviour |
| [emulation vs. an FPGA](emulation/fpga.md) | what "no emulation" on a hardware product actually claims |

## The components

| page | what it is |
|---|---|
| [component reference index](components/README.md) | one deep-dive page per component: hardware, history, our implementation, honest approximations, expected behaviours as future tests |
| [CPU](components/cpu.md) | the Sharp LR35902 (SM83) — registers, flags, the opcode tables, interrupt dispatch. It is not a Z80 |
| [PPU](components/ppu.md) | the picture: mode cycle, background/window/sprites, palettes, STAT/LYC, and the DMG OAM-corruption bug |
| [MMU](components/mmu.md) | the 64 KB bus — address routing, OAM DMA, CGB HDMA/GDMA, the `IF`/`IE` wiring |
| [cartridges (MBC)](components/mbc.md) | bank switching for MBC0/1/2/3/5, the MBC3 clock, battery-backed saves |
| [APU](components/apu.md) | the four channels register by register, and the frame sequencer |
| [timer & divider](components/timer.md) | `DIV`/`TIMA`/`TMA`/`TAC`, the programmable interrupt heartbeat |
| [serial](components/serial.md) | the link port, and the channel Blargg's test ROMs report through |
| [Super Game Boy](components/sgb.md) | the ICD2 command channel — everything of an SGB a cartridge can observe, and why the rest is a non-goal |
| [joypad](components/input.md) | the 2×4 button matrix at `$FF00` and the joypad interrupt |
| [frontends](components/frontends.md) | desktop (OpenGL), terminal (ratatui), web (canvas) and the C/Go FFI |

## What we measured

| page | what it is |
|---|---|
| [the measured Game Boy](measured/README.md) | the section index: what the reference says, what we measured, the test that pins it, what a wrong emulator does |
| [scoreboard](measured/scoreboard.md) | every suite scored, with a reason for every red row |
| [timing](measured/timing.md) | the timer as a real circuit, interrupt dispatch, OAM DMA — and what each was worth |
| [the PPU](measured/ppu.md) | two picture engines, and what the difference is actually worth |
| [the APU](measured/apu.md) | ten calibrated constants, each measured against a named ROM |
| [boot state](measured/boot.md) | the Game Boy is nine machines, not four, and no two hand over the same state |
| [OAM DMA and work RAM](measured/oam-dma-work-ram.md) | the address bus is shared: what a work-RAM access does under a transfer, settled with a probe cartridge and a second implementation |
| [cartridges](measured/cartridges.md) | mappers, the clock, and the class of bug that eats somebody's save |
| [the link port](measured/serial.md) | the half no ROM-driven suite can reach |
| [the Super Game Boy](measured/sgb.md) | a joypad port that is also a command channel |
| [against the field](measured/against-the-field.md) | other emulators built and timed here on the same core, ROM and clock — one claim held, one did not, and where the difference comes from |
| [speed ledger](measured/speed-ledger.md) | every optimisation attempted — what each was worth, the eleven measured negatives with their numbers, the field built and timed here, and what is queued |
| [headless throughput](measured/throughput-baseline.md) | how fast the core runs with nothing attached, the method that reproduces it, and which of the two 1000× targets each number belongs to |
| [the accuracy baseline, 2026-08-25](measured/accuracy-baseline-2026-08-25.md) | every suite re-run and totalled on `c575bb4`, with the submodule check done explicitly and the failures classified by what the frame really contained |
| [where the three dots are not](measured/mode3-phase-2026-08-27.md) | three hardware suites, three anchors, and no constant that satisfies all three — with the probe artefact that produced the previous page's parity split corrected |
| [the sprite mode-3 calibration](measured/sprite-mode3-calibration-2026-08-26.md) | Gambatte's 113 one-M-cycle sprite pairs turned into an instrument: the OBJ penalty formula is right, the defect is a three-dot offset between the mode cycle and `LY`, and seven models measured against it |
| [the STAT line's phase](measured/stat-line-phase-2026-08-25.md) | the two picture engines bracket hardware by two M-cycles — the calibration ladder that measures it, the model it points to, and two models tested and rejected with numbers |
| [the pool sweep, 2026-08-25](measured/pool-sweep-2026-08-25.md) | every pooled worktree and branch checked against the real `main`: why the ancestry counts are meaningless after the history rewrite, and why nothing is stranded |
| [the host boundary](measured/host.md) | defects that leave the framebuffer perfect — the pacing clock, the build, the audio device, the terminal |
| [symptoms](measured/symptoms.md) | "my emulator does X" → what it actually is |
| [Mealybug Tearoom](mealybug.md) | what the mid-scanline suite measures, where we stand, and what the rest would cost |

## Accuracy & conformance

| page | what it is |
|---|---|
| [the accuracy yardstick](testing.md) | which suites exist, how each is judged, and how our harness detects a verdict |
| [per-fix conformance notes](conformance-notes.md) | every accuracy bug we fixed, with symptom, catching test and hardware citation — the raw corpus |
| [the GitHub Action](conformance-action.md) | scoring *someone else's* emulator from CI: the adapter contract, badges, licensing, honest gaps |
| [the standalone-tool design](conformance.md) | where the conformance work is going, and why |
| [GBSelfTest](gbselftest.md) | a cartridge that tests the machine it runs on and prints a code you can look up |
| [stable codes](pitfalls/codes.md) | the `TGB-<AREA>-<NNN>` registry a failure points at — a contract, not a list |
| [the GB Emulator Shootout](shootout.md) | what we would score on the public board, what their runner requires, and the prepared entry |

## Pitfalls

| page | what it is |
|---|---|
| [the symptom index](pitfalls/README.md) | the way in: a literal error string → the entry that explains it |
| [builds & toolchains](pitfalls/builds.md) | the two rlibs of one crate, the panic strategy, the cross-target traps |
| [tests & measurement](pitfalls/tests.md) | the harness that skips silently, the check with no teeth, the baseline that proves nothing |
| [host, clock & environment](pitfalls/environment.md) | deadlines measured in the wrong clock, terminals, permissions, `pkill -f` |
| [accuracy work](pitfalls/accuracy.md) | the constant absorbing somebody else's error, and other ways to fix the wrong thing |
| [when it is not your bug](pitfalls/not-our-bug.md) | upstream defaults, stale READMEs, and suites that disagree with themselves |
| [techniques](pitfalls/techniques.md) | the instruments that found everything else in the section |

## The picture

| page | what it is |
|---|---|
| [how the picture reaches you](picture.md) | **start here for anything visual** — framebuffer to terminal, the four published protocols we implement, why it is sharp, and the two screenshot keys |
| [getting a frame into a terminal](rendering.md) | the transport ladder from shared memory (81 bytes/frame) through indexed PNG, and how it is negotiated |
| [terminal render performance](terminal-render.md) | why a big window used to cost frames, and what caps the cost now |
| [the three modes](modes.md) | `identical`, `standard` and **speed mode**: what each guarantees, what each is for, and what speed mode must never be used to judge |
| [picture modes](picture-modes.md) | `standard` vs `identical`: what each draws, what each scores, what the exact one costs |
| [display scaling](display.md) | how each frontend enlarges 160×144, and how to make it crisp rather than blurred |
| [pixel-art upscaling](upscaling.md) | the four algorithms, their measured costs, and the licensing constraint that shaped the set |
| [video out & virtual webcam](video-out.md) | the picture as a raw frame stream and as a V4L2 camera, with the exact commands |
| [getting the panel into the feed](stream-panel-routes.md) | three routes for streaming the text panels too, and why we recommend OBS over building a compositor |

## Sound

| page | what it is |
|---|---|
| [the audio pipeline](audio.md) | APU → `blip_buf` → `cpal` → speaker, the pacing maths, the latency budget, and what is genuine DMG character |
| [web audio](web-audio.md) | the browser's half — why the wasm build had no sound *chip*, and the three constants a real Chrome run decided |
| [the chiptune renderer](chiptune.md) | sound-chip instructions in, real Game Boy audio out: the score/`.vgm`/register formats and the accuracy evidence |
| [music mode](music-mode.md) | cartridges as a record shelf — cover art picked by scene, and why track selection has no general solution |

## Running it, and looking inside

| page | what it is |
|---|---|
| [save states](savestates.md) | the `GBSS` format, the block layout, and why not BESS |
| [telemetry](telemetry.md) | machine-readable NDJSON run data — fps, per-stage breakdown, emulated speed |
| [debug tooling](debugging.md) | how to inspect, step and capture the running machine, and the surface external tools emit from |
| [portability](portability.md) | what the core actually runs on, byte-identically — including a PSP that plays Pokémon |
| [throughput](throughput.md) | how fast the core is in both units, what the build profile was costing, skippable rendering — and the three optimisations that measured worse |
| [brand assets](brand/README.md) | the mark, and the rule that it is generated from a script with no font embedded |

## The link cable

| page | what it is |
|---|---|
| [two consoles, one wire](link-cable.md) | the cable itself: start a second emulator and they find each other, plus the lockstep argument and the Gen 1 ↔ Gen 2 rules |
| [trading a Pokémon in](link-trade.md) | the whole journey with screenshots, and what to do if it hangs |
| [the Cable Club Colosseum](link-battle.md) | a link battle, and the crash that destroyed saves — plus the guard that now stops it |
| [the link cable over a network](bgb-link.md) | BGB's protocol over TCP, verified against a second implementation |
| [the trade portal client](portal.md) | asynchronous friend trades over HTTPS, offered from the save rather than a form |
| [the World Trade Centre](worldlobby.md) | several independent emulators walking around one shared Gen 1 room |

## Agents, and the emulator as an environment

| page | what it is |
|---|---|
| [Claude plays Pokémon](claude-plays.md) | the agent substrate: plan-then-execute, the budget and breaker, the OBS recipe, the honest gaps |
| [long runs and episodes](longplay.md) | an agent plays unattended for hours and reports in — and the **episode** primitive that makes it measurable |
| [scenarios](scenarios.md) | a goal plus **rules the host enforces rather than asks for**, and a run recorded as a file that replays with no model calls |
| [the MCP server](mcp.md) | driving the emulator from Claude Desktop, Claude Code or anything else that speaks MCP |
| [the agent reference](agent-reference.md) | every hook, field and payload, with examples generated by running the thing |
| [as an agent environment](agent-environment.md) | determinism across architectures, headless throughput, save states as episode resets, and how an agent will cheat your score |
| [example: MCP transcript](agent/examples/mcp-transcript.md) | every tool, request and reply, captured from the real server |
| [example: agent snapshots](agent/examples/agent-snapshots.md) | what a driver is actually handed, captured from a real run |
| [the battle policy](agent/battle-policy.md) | fight, switch, item or run, decided from the cartridge's own type chart, move table, HP, PP and bag — one capability every link shares, with the battle report that goes beside a pass rate |
| **[what the agent did](https://github.com/Alchemy86/AgentGB/tree/main/docs/agent)** *(AgentGB)* | **the chain from a cold cartridge to Cerulean City, the gym and the badge, the catch policy, the bench that beat a wall levelling could not, and the rig that compares two players — moved out of this repository on 2026-08-19, because this is the emulator and that is the project that trains the thing. Pointers stand at the old paths; [`where-things-live.md`](where-things-live.md) is the rule** |
| [Cerulean, and the Cascade Badge](agent/misty.md) *(pending a move to AgentGB)* | what our solver did with the second gym: the five links, the certified rates, the starter line that cannot pass at any level, and the heal-link hole HP was hiding. It belongs in AgentGB by the rule on the page above; its cartridge half is already split out into the Gen 1 section |
| [the screen mosaic](agent/screen-mosaic.md) | sixty agents' Game Boy screens tiled into one frame — what the swarm *sees* rather than where it is, the capture mode it needs, and what a downscaled tile cannot tell you |
| [the swarm's paths](agent/path-arrows.md) | every tile the swarm ever walked, drawn as a directional arrow over the atlas — what the colour encodes, and the survivorship the picture cannot help carrying |
| [position tracks, and sampling which runs get filmed](agent/track-format.md) | every run records where it was now, so a film is a selection over a certification that already happened — the compact format, where a corpus's bytes really are, and the selectors that make "film me the failures" a one-liner |
| [`chainrender`](agent/chainrender.md) | the parts every Python renderer here shares — the atlas, the tracks, the palettes and the camera — why they were four copies, and the two equality gates that keep the consolidation honest |
| [encoding a film](agent/film-encoding.md) | why the films were huge and often soft — the two independent causes measured, the recipe that came out of it, and the 4:4:4 compatibility trap that cost a shipped film |
| [the swarm view](swarm-view.md) | twenty-four emulators from one save state drawn as dots on a map — the null-model baseline a trained agent has to beat, and an object lesson in what the rest of this tooling adds up to |
| [why Peter Whidden's swarm reads smoother](agent/swarm-render-comparison.md) | a cited, honest comparison against his actual tooling — interpolation, walk-cycle sprites, camera, codec — ranked cheap wins before expensive ones, and the finding that his "battle swarm" is likely no such thing |
| [shiny hunting](shiny-hunt.md) | N emulators hunting one DV pattern in parallel, and the decorrelation problem that makes it hard |

## Extending it

| page | what it is |
|---|---|
| [writing a scenario](writing-a-scenario.md) | zero to a running scenario: pick a goal, find the byte that proves it, write the watch, check it, run it — and the five ways it goes wrong, with the output each one prints |
| [writing a plugin](writing-a-plugin.md) | zero to a working plugin for someone who has never seen this codebase |
| [the plugin contract](plugins.md) | what a plugin may do, how you can tell one is attached, the trust model, the version boundary |
| [the live trainer view](trainer.md) | the read-only plugin that reads a running Pokémon cartridge — the address table, the derivation, the verification |

## Pokémon Generation 1

| page | what it is |
|---|---|
| [the Gen 1 index](gen1/README.md) | the section's entry point, and the standards every page in it is held to |
| [the memory map](gen1/memory-map.md) | where Red and Blue keep everything, and how each address was graded |
| [the memory atlas](gen1/atlas/README.md) | every address with the evidence we watched the cartridge use it — its own project now ([AtlasGB](https://github.com/Alchemy86/AtlasGB)); this emulator produces the evidence |
| [reading the screen as text](gen1/reading-the-screen.md) | the tilemap trick that lets a headless bot navigate menus |
| [starting a battle from outside](gen1/battle-control.md) | the one byte the engine polls, and how to script what follows |
| [the battle engine](gen1/battle-engine.md) | a model of the battle rules held to the retail cartridge as an oracle, and agents that play it |
| [battling](gen1/battling.md) | the map of the subject, and the one thing that had no home: what a battle screen shows, what it withholds, and for how long |
| [learning moves, and evolving](gen1/learning-moves.md) | every move-learning decision the game can ever ask, counted — 728 learn entries, 840 distinct decisions — with a stated rule for each, and why a student should never cancel an evolution |
| [the level-up move drill](gen1/level-up-drill.md) | that rule played out on the cartridge: every level-up on the Charmander and Bulbasaur lines triggered directly, the prompt answered or declined, the party read back as the verdict, and the whole thing recorded as training data |
| [catching](gen1/catching.md) | whether an encounter is worth a ball — the capability's gate measured on the chain's own states, a wild-versus-trainer discriminator established twice, and the catch rate against the odds we predict |
| [choosing a bench](gen1/catch-and-train.md) | which catchable Pokemon actually beats a gym and at what level, put through the cartridge's own damage formula — why a water gym wants a GRASS bench and not an ELECTRIC one, why PIKACHU loses to Misty at every reachable level, and the three cartridge facts a catch-and-train capability rests on |
| [a paper's Gen 1 claims, checked](gen1/paper-claims.md) | eleven asserted mechanics measured on the cartridge — what held, what did not, and the one our own engine had backwards |
| [Cerulean Gym, and Misty](gen1/cerulean-gym.md) | the second gym as the cartridge holds it — Misty's party and her movesets, the gym map and its three trainers, a Gen 1 trainer seeing through a wall, the three bytes the badge writes, the nurse restoring PP, and the grass tables |
| [driving the link cable from outside](gen1/link-from-outside.md) | impersonating the far end of the wire, and what the protocol actually carries |
| [leaving the Cable Club](gen1/leaving-the-cable-club.md) | the report that was not our bug, measured against the cartridge |
| [authoring an area](gen1/authoring-an-area.md) | a new destination reached through the cartridge's own unedited door |
| [agent play](gen1/agent-play.md) | reading state and pressing buttons — the Gen 1 facts an agent needs |
| [discoveries](gen1/discoveries.md) | every fault and quirk this campaign found in Pokémon Blue, with the reasoning that found it — a thirty-year-old soft-lock, mechanics the community states one way and the cartridge another, and the corrections of our own beliefs |
| [sharp edges](gen1/sharp-edges.md) | the Gen 1 traps that produce plausible-but-wrong data |

## Pokémon Generation 2

| page | what it is |
|---|---|
| [the Gen 2 index](gen2/README.md) | what is established about Gold, how each address was graded, and a blunt list of what is not |
| [the memory map](gen2/memory-map.md) | where Gold keeps what we have actually looked at |
| [the save file](gen2/the-save-file.md) | three WRAM runs into three SRAM runs, the checksum, and why editing the map does not move the player |
| [the Time Capsule](gen2/time-capsule.md) | the only door between the generations: what Gold refuses, what it requires |
| [reading Gold's screen](gen2/reading-the-screen.md) | the Gen 2 charmap, and the two facts inverted from Gen 1 |
| [building the disassembly](gen2/building-the-disassembly.md) | making a `pokegold` build byte-identical to the cartridge, so its symbols are exact |
| [sharp edges](gen2/sharp-edges.md) | the Gen 2 traps, including the ones we have not closed |

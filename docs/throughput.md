# Throughput: how fast the core actually is, and what made it faster

This page is about **wall clock**, not accuracy. Everything else in `docs/measured/` asks whether
the emulator is right; this asks what it costs, in the two units two different audiences read, and
it records the experiments that came back **negative** as carefully as the ones that worked —
because a negative result saves the next person the afternoon.

Nothing here changed a single emulated byte. Every measurement below was taken with the frame hash
and a WRAM hash printed beside it, and **all eighteen external baselines, the in-repo test-ROM
harness and both picture engines are byte-identical across every configuration on this page**. An
emulator "optimisation" that changes behaviour is a bug, not an optimisation.

- **Instrument:** [`examples/core_bench.rs`](../examples/core_bench.rs)
- **Scripts:** [`tools/perf/`](../tools/perf/) — `build-variants.sh`, `measure.sh`, `pgo.sh`,
  `build-times.sh`, `swarm.sh`
- **Where the frame goes:** `core_bench --breakdown` (needs `--features profile`)

**§1–§7 measure one emulator; §8–§10 measure the swarm**, which is what actually runs and is a
different workload. Short version of the second half: the fan-out was spending **46 % of its wall
clock in the filesystem** writing battery saves and staged cartridges nothing reads, and another
40 % of its emulation drawing pixels and resampling audio for an observer that does not exist. One
worker went **23.0 s → 9.5 s** on the same twenty attempts, and the emulation share of a swarm
frame went from about half to about ninety-nine per cent.

---

## 1. The two units, and why both

We publish **ms/frame** and **x real time**. The reinforcement-learning field publishes
**aggregate steps per second across a fan-out**, and states the convention explicitly: *count
multiple environments in your favour.* Neither number is comparable to the other, so a reader
coming from that direction could not place us at all.

`core_bench` prints both, always, with the worker count and the machine — never bare:

```sh
cargo run --release --example core_bench -- --rom game.gb --state s.gbss \
    --frames 25000 --workers 20
```

Two conventions inside the tool are worth knowing, because both were wrong in its first version:

- **The wall clock starts after every worker has warmed up**, behind a barrier. Without that the
  aggregate figure divides the measured frames by a clock that also contains the warmup and
  understates itself by 13 % at the defaults. The community unit is the one an outside reader
  compares us on, so it has to be honest in our disfavour as well as our favour.
- **The per-instance figure is CPU time, not wall time** (`/proc/thread-self/schedstat`). On a box
  with other work on it a wall clock reports the scheduler's decisions as if they were the
  emulator's cost. The aggregate figure stays wall-clock, because that one is wall-clock *by
  definition* — it is what the fan-out actually delivers.

---

## 2. What changed, and what it bought

**The shipping profile optimised for size.** `[profile.release]` said `opt-level = "s"` from 2022
until 2026: upstream `raphamorim/gameboy` set `z`, then `s`, before this fork existed and before
any of the constrained targets did. Nothing here ever chose it — and the one target that *had*
thought about the question chose against it, `tools/psp-build.sh` overriding it to 3 with a note
that "s" costs the PSP a millisecond a frame.

A Game Boy emulator is a branchy interpreter in a hot loop. `opt-level = "s"` is the wrong
instrument for one.

**Measured** — Pokémon Blue from a real overworld save state, `tools/perf/measure.sh`, eight
interleaved 25,000-frame rounds, best round, Intel Core Ultra 7 265 (20 cores), load ~1:

| opt-level | lto  | codegen-units | flags | ms/frame | vs `"s"` |
|---|---|---|---|---:|---:|
| `s` *(was)* | thin | 1 | — | 0.1133 | — |
| `2` | thin | 1 | — | 0.0988 | +14.7 % |
| `3` | thin | 1 | — | 0.0928 | +22.1 % |
| **`3` *(now)*** | **fat** | **1** | — | **0.0912** | **+24.2 %** |
| `3` | fat | 16 | — | 0.0891 | +27.2 % |
| `3` | fat | 1 | `-Ctarget-cpu=native` | 0.0895 | +26.6 % |
| `s` | fat | 1 | `-Ctarget-cpu=native` | 0.1084 | +4.5 % |
| `3` | thin | 1 | PGO | 0.0968 | +17.0 % |

A second independent run reproduces every row to about 1 %. The frame hash and the WRAM hash are
**identical across every variant and every round**, which is the only reason any of the
milliseconds mean anything.

**Read the last two rows before reaching for an exotic knob.** `opt-level` is nearly the whole
term: `"s"` *with* fat LTO *and* `-Ctarget-cpu=native` is +4.5 %, against +24 % for changing the
one setting. The instrument was wrong, not the settings around it.

### The profiles now

| profile | for | settings |
|---|---|---|
| `release` | everything native — `cargo install`, the release zip, the harnesses, the fan-out | `opt-level = 3`, fat LTO, one codegen unit |
| `wasm-release` | `make build-wasm`, the one target where bytes are the currency | inherits `release`, `opt-level = "z"` |
| `quick` | **iteration** — see §5 | `opt-level = 2`, no LTO, 16 units, incremental, unwinds |
| `dev` | debugging | unchanged |

---

## 3. Three experiments that came back negative

These are here so nobody spends the afternoon again.

**`-Ctarget-cpu=native` buys nothing measurable — 0.0895 against 0.0912, inside the run-to-run
spread.** There is deliberately **no `.cargo/config.toml`** setting it. That is a good outcome
rather than a disappointing one: a native-tuned binary is not portable, and a flag file in the
repository would have silently applied to released artefacts and to anything CI publishes. The
emulator's hot loop is byte shuffling and unpredictable branches; there is no vector work for a
newer instruction set to find.

**Profile-guided optimisation made it slower.** Against its fair comparator — `opt-level 3`, thin
LTO, one codegen unit, because **`-Cprofile-use` and fat LTO do not combine on the pinned rustc
1.83** (`module flag identifiers must be unique … !"CG Profile"`) — PGO measured **0.0968 against
0.0928, a 4.3 % regression**, on a profile collected from the real workload it was then scored on.
PGO usually helps interpreters; here the profile is dominated by one loop that LLVM already lays
out well, and the extra instrumentation-derived block ordering costs more than it recovers.
`tools/perf/pgo.sh` is kept so the experiment is one command to repeat, and is wired into no build
target: the profile is a file on one developer's disk, so a build that depended on it would produce
different binaries for different people.

**Optimising dependencies in a debug build does nothing here.** `[profile.dev.package."*"]
opt-level = 2` is standard advice and it is wrong for this crate: the debug build's cost is *our
own* code, which is a local crate and stays at `opt-level = 0` either way, and the dependencies are
not on the emulation path at all. Measured: `cargo test --lib` **3.10 s with it against 3.07 s
without**, for **+1.2 s on a cold build**. Removed; the mechanism that does work is §5.

**And one that was measured but not taken.** `codegen-units = 16` is not measurably slower than 1
(0.0891 against 0.0912, marginally ahead, reproduced twice) and halves the cold build (30.9 s
against 66.7 s). It stays at 1 anyway, and the reason is caution rather than a number: after fat
LTO the merged module is partitioned by an LLVM heuristic, so "the hot loop happened to stay in one
partition" is a property of today's source, not of the setting. This is the shipping build and it
is built rarely. The measurement is done if a future crew wants those two minutes back.

---

## 4. Where the frame actually goes — and the pixels nobody reads

Flags were worth 24 %. **The structural finding is worth more than that**, and it came from asking
where the time goes before optimising anything.

`core_bench --breakdown` removes one component's work at a time and times whole frames — the method
in [`src/profile.rs`](../src/profile.rs), because a single aggregate figure cannot be optimised,
only guessed at. Pokémon Blue, overworld save state, 2,000 frames, best of five:

| row | ms/frame | delta | instr/frame | guest-visible? |
|---|---:|---:|---:|---|
| baseline | 0.1017 | — | 2529 | — |
| **no pixels** | **0.0674** | **+50.9 %** | **2529** | **no** |
| no timer | 0.0856 | +18.8 % | 2530 | yes |
| no bus tick | 0.0979 | +3.9 % | 2530 | yes |
| no stat line | 0.1006 | +1.1 % | 2529 | yes |
| no idle batch | 0.1756 | −42.1 % | 14653 | no |

**Read the instruction count in every row: a row whose count differs from the baseline's is not an
A/B.** The `no pixels` row is 2529 against 2529 — exact. (The last row is the honest "before" for
`MemoryManagementUnit::idle_mcycles`, which is why it is negative: batching a halted CPU's idle
M-cycles is worth 42 %, and Pokémon Blue is halted for most of its frame.)

So **the PPU is half the frame, and in a headless collector whose observation comes out of memory
nobody ever looks at what it draws.**

### `set_render_pixels` — skippable, never unavailable

[`Gameboy::set_render_pixels(false)`](../src/gameboy.rs) lets the whole-scanline renderer skip
`Gpu::renderscan` entirely: no background fetch, no sprite composite, no 92,160 pixel stores a
frame.

**It is opt-in and it stays opt-in.** Pixels are on for every frontend, every harness and every
default build, because the picture is the product, and the screen-only student this project is
aiming at needs the framebuffer. Nothing here makes pixels *unavailable*; it only lets a caller
that has said it does not want them stop paying.

Three properties make it safe, and each has a test that fails when the mechanism is removed:

1. **The cartridge cannot tell.** The framebuffer is emulator *output* — no Game Boy program can
   read it back — so the CPU executes the same instructions either way. The one piece of
   non-output state rendering advances is the window's internal line counter, and the skip path
   advances it identically. `gpu::tests::skipping_pixels_changes_nothing_the_cartridge_can_see`
   drives two PPUs dot for dot with the window toggled mid-frame and compares every field except
   the picture, every scanline.
2. **Turn pixels back on at a frame boundary and the next frame is exact** — byte-identical to one
   from a machine that never stopped drawing. That is the guarantee a policy acting every *k*
   frames rests on: skip the intermediate frames, ask for the one it will actually look at.
   `gameboy::tests::pixels_turned_back_on_at_a_frame_boundary_give_an_exact_frame` proves it on a
   scrolling picture. (Toggling *mid*-frame leaves that frame half-drawn — the lines already past
   are not revisited — so toggle between `frame()` calls, never inside one.)
3. **The per-dot engine ignores it, deliberately.** Under `identical` the fetcher *is* the timing:
   mode 3 lasts exactly as long as producing the pixels takes, so not producing them would change
   what the cartridge observes. `Gpu::pixels_effective` reports the difference rather than letting
   a caller believe it has saved something it is still paying for.

**Measured on a real cartridge**, and note the second column — this is the end-to-end proof of
point 1, not a claim about it:

| workload | pixels on | pixels off | gain | WRAM hash |
|---|---:|---:|---:|---|
| overworld | 0.0927 | 0.0599 | **1.55x** | identical |
| battle | 0.1671 | 0.1338 | **1.25x** | identical |

The battle is worth less because the CPU has more to do there, so pixels are a smaller share of a
larger frame. Both are far larger than any build flag.

**The APU is free for a caller that installs no player**, because `Sound::do_cycle` returns
immediately when there is none. *(That sentence used to end "and a headless machine has none, so
there is nothing to skip." It is wrong for the collector this whole page is about, and §8 below is
what corrects it: a swarm run **must** install a player, because a machine with no sound chip is
one a cartridge can tell apart, and the APU then costs 14 % of its frame.)*

### It is wired into the MCP server

`terminalgb-mcp --no-pixels` declares it for an agent whose observation comes from `observe` and
`read_memory`. With it set, the `screenshot` tool is **removed from `tools/list` and refused if
called anyway**, naming the flag — the same rule `write_memory` follows when it was not granted,
and for a sharper reason: the framebuffer holds whatever was last drawn, so a `screenshot` that
answered would return a stale frame *as if it were the current one*. Absent beats confidently
wrong.

---

## 5. Compile time

`codegen-units = 1` plus fat LTO is the slow-compile combination and it is the right choice for a
build that ships. It is the wrong one to iterate under, and `--release` was the only optimised
build available.

**`[profile.quick]`** is the iteration profile: `opt-level = 2`, no LTO, sixteen codegen units,
incremental on. Measured with [`tools/perf/build-times.sh`](../tools/perf/build-times.sh), which
appends a comment to a source file and removes it again, each profile in its own target directory:

| profile | cold | one-line change in the core | one-line change in a frontend |
|---|---:|---:|---:|
| `release` | 64.5 s | 32.9 s | 32.6 s |
| **`quick`** | **20.1 s** | **1.7 s** | **1.7 s** |
| `dev` | 33.3 s | 2.7 s | 2.7 s |

**A one-line change costs 1.7 s instead of 32.9 — nineteen times faster** — and the result still
runs fast enough to drive a harness.

```sh
cargo test --profile quick
cargo run --profile quick --example core_bench -- --rom game.gb
```

Two deliberate choices in it:

- **It is not `release` with the knobs loosened, and it has a different name on purpose.** A number
  measured under `quick` is not a number this project publishes, and giving it its own name is what
  stops one being quoted as if it were.
- **It unwinds.** `[profile.release]` aborts on panic — load-bearing, see `src/mbc/battery.rs` —
  but `cargo test` forces test targets to unwind, so a `cargo test --release` run builds this
  crate's rlib **twice** and intermittently links the wrong one. That is the recurring failure
  [`docs/pitfalls/builds.md`](pitfalls/builds.md) catalogues: *"the trait bound X: Y is not
  satisfied"* for a type that plainly implements it, *"perhaps two different versions of crate
  X"*, `unresolved import` for a module that is there. One panic strategy across the whole profile
  means one rlib and none of it.

---

## 6. The headline, in both units

Pokémon Blue from an overworld save state, 25,000 frames a worker, best of two, Intel Core Ultra 7
265 (20 cores, load ~1). The 20-worker figure is the community unit, counting every environment in
our favour, as they do.

| | ms/frame (1 worker) | fps (1 worker) | aggregate steps/s (20 workers) |
|---|---:|---:|---:|
| before — `opt-level = "s"`, thin LTO | 0.1159 | 8,629 | 106,043 |
| after — `opt-level = 3`, fat LTO | 0.0918 | 10,896 | 153,754 |
| after, **`--no-pixels`** | **0.0596** | **16,784** | **232,746** |

**+26 % from the profile, 2.19x with the pixel skip on top**, and every frame hash unchanged.

Scaling is not linear and the page should say so: one worker is 16,784 fps, so twenty *perfectly*
scaled would be 335,000. We get 232,746 — 69 % efficiency, lost to memory bandwidth and to the
turbo bins a 20-core part gives up when all twenty are busy.

### Against the field, fairly

`docs/measured/` and `/home/aaron/pokemon-run/THROUGHPUT.md` record a GPU implementation of Pokémon
Red reporting roughly **160k fps across 8,192 environments**, against our previously-published
~84k. On the numbers above we are now past that on a CPU — and **that comparison should be made
carefully, or not at all**:

- Different hardware, and theirs is one GPU against our twenty cores. "Faster per box" is not
  "faster per watt", "per pound", or "per rack".
- Their figure is from their own report, not a measurement anybody here reproduced.
- **Their caveat is the important one and it cuts our way, not theirs.** Their own note is that
  *"emulation on GPU is hard, because many different branches"*, and the field's other direction —
  a native port of the game's logic — has **no ground truth at all**: what is being trained
  against is somebody's reimplementation of Pokémon, with whatever divergences it carries.

**Our claim is accuracy and verifiability, and it should stay that.** What this page supports is
narrower and more defensible: *the accurate substrate is no longer the slow one.* That is worth
saying precisely because it removes the reason anybody had to trade the ground truth away.

---

## 8. The swarm — and the 46 % of it that was the filesystem

§1–§7 measure **one emulator**. This section measures the thing that actually runs: a
certification fan-out, twenty processes' worth of `pewter_chain corpus` playing a link of the
Pewter chain thousands of times. The two are not the same workload, and the gap between them was
where all of this round's speed was.

**The unit is emulated frames per second of wall clock across the whole fan-out.** Episodes per
second — what `corpus` printed before — cannot be compared between links, because an episode is
however long it is; it cannot be compared with anybody else's harness either. `corpus` now prints
both, plus the split between time inside `Gameboy::frame` and time around it:

```text
9.4 s wall, 21.19 episodes/second, peak concurrency 20
104666 emulated frames/s aggregate (987736 frames); 149.9 s emulating,
150.2 s in the run loop, 38.6 s of worker time outside it
```

That third line is the whole diagnostic. `emulating` is `RunResult::emulate_ms` summed across
workers — two clock reads a frame, 0.04 % of a frame that costs tens of microseconds, and worth it
because "how much of a step is emulation" is otherwise a guess.

### What the profile actually said

`perf` is not installed on this box, so the profile is `eu-stack` sampled 500 times against a
single worker and attributed by leaf frame. Before any change, with the phase offsets and the run
frames together:

| leaf | share | what it is |
|---|---:|---|
| `syscall` (blocked) | large | **the filesystem** |
| `Gameboy::frame` | 14.8 % | the CPU interpreter, inlined |
| `Gpu::draw_map_run` | 4.5 % | pixels nobody reads |
| `Sound::do_cycle` + channels + `read_samples` | ~4 % | samples nobody hears |
| everything in the agent plugin | **0.6 %** | — |

**The per-frame harness was never the problem, and that is worth saying loudly**, because it is the
thing everyone assumes: the plugin dispatch, the six-frame sample, the referee and the guidance
together are under one per cent. What was expensive was two kinds of work nobody consumed.

### The headline, in the swarm's own unit

Pokémon Blue, link 01 of the Pewter chain, `pewter_chain corpus`. **Every pair below is an
interleaved A/B against a binary built from the parent commit**, because for most of this session
another crew was running a twelve-worker certification on the same twenty cores and interleaving is
the only shape that survives that. Load average is stated with every figure, as this page's own
rule demands.

**Twenty workers, 200 attempts, 987,736 emulated frames, four interleaved rounds** (load 10–18):

| | wall | emulated frames/s aggregate |
|---|---:|---:|
| before, typical (r2–r4) | 13.55 s | 72,900 |
| before, best round | 9.85 s | 100,311 |
| **after, typical (r2–r4)** | **9.51 s** | **103,900** |
| **after, best round** | **8.20 s** | **120,409** |

**1.43x on the typical round, 1.20x on the best.** The *before* column has a 38 % spread across
four identical runs and the *after* column has 16 %, which is the finding restated: the old path
was partly I/O-bound, and I/O time is a function of the filesystem's writeback state rather than of
the work.

**One worker, 20 attempts, 95,073 emulated frames, pinned to one core, three interleaved rounds**
(load ~13) — this pair separates the two halves, because CPU time is immune to a busy box in a way
wall clock is not:

| | wall | CPU (user) | wall − CPU |
|---|---:|---:|---:|
| before | 19.35 s | 17.69 s | 1.66 s |
| **after** | **12.45 s** | **12.42 s** | **0.03 s** |

**1.42x of CPU time** — that is the pixels and the samples, and it is a contention-immune number.
**1.55x of wall clock at this load**; on a quiet box earlier in the session the same pair measured
**23.0 s → 9.7 s, 2.37x**, because the filesystem half was much larger then. The old path's wall
clock ranged 15.7–26.9 s across the session for identical work; the new one's ranged 8.7–13.4 s and
tracks nothing but the CPU it was given.

**Every one of these runs reached the goal 200/200 (or 20/20) and emulated exactly the same number
of frames**, which is the first check that a speed-up did not quietly change the work.

### 8a. A collector must not write a battery save — 1.87x, on its own

`play()` staged the cartridge into `<out>/cart.gb` on **every attempt** and built the machine with
that path, which wires up a battery. So a 3,000-attempt certification wrote three thousand
identical copies of one megabyte, and then wrote a `.sav` beside each of them through
`Battery::write_atomic`, which `fsync`s the file **and** its directory — from twenty workers at
once.

No attempt has ever read a `.sav`. An attempt's cartridge RAM arrives inside a save state and
leaves inside one; there is nothing to stage and nothing to persist.

The A/B that sized it needed no code change at all — point the output directory at `tmpfs`, where
`fsync` is a no-op:

| output directory | wall | CPU |
|---|---:|---:|
| `target/…` (disk) | **23.0 s** | 12.25 s |
| `/dev/shm/…` (tmpfs) | **12.4 s** | 12.33 s |

Identical CPU time, half the wall clock. **The filesystem was 46 % of a single-worker run.** The
cartridge is read once per command into an `Arc` now and the machine is built with `None`, and the
trajectory and final-state digests are byte-identical either way, checked in both directions.

*(A second thing falls out: with a save path, attempt *k+1* inherited attempt *k*'s `.sav`, so
attempts in one worker directory were coupled through a file. Measured not to change link 01's
trajectories — but a corpus whose whole purpose is independent attempts should not have had the
coupling at all.)*

### 8b. Pixels and samples, in the swarm — 1.7x on the machine a collector runs

§4 established that pixels are half a frame and that `set_render_pixels(false)` is guest-invisible.
**No headless runner in this repository was using it.** Only `terminalgb-mcp --no-pixels` was.

And §4's closing claim — that the APU costs a headless caller nothing — is false for a swarm.
`Sound::do_cycle` returns immediately when no player is installed, but a swarm run **installs
one on purpose**: a machine with no sound chip is a machine a cartridge can tell apart, and
Pokémon Blue's Pokémon Center healing sequence never finishes without one. So the collector pays
for a band-limited resample it throws away.

[`Sound::set_render_audio(false)`](../src/sound.rs) is the exact sibling of the pixel skip and rests
on the same argument: samples are emulator *output*, no address a cartridge can name carries them,
and the `blip` buffers are explicitly not machine state. **The construction is deliberately
narrow.** Everything that decides what the guest can read still runs unchanged — the frame
sequencer, the length counters, the sweep and envelope units, the duty and wave positions, the
LFSR, `since_fetch`, the channel-enable flip-flops behind `NR52`, and `last_amp`. What is skipped
is `blip_buf::add_delta`, `end_frame`, and with them the mixdown and the hand-off.

Because every *serialised* field keeps its value, the claim can be made in the strongest available
form: **a save state taken with samples off is byte-identical to one taken with them on.** That is
what `gameboy::tests::skipping_samples_changes_nothing_the_cartridge_can_see` asserts — 90 frames
of a cartridge driving channel 1, then every APU register compared and then the whole save state.
Teeth proven both ways: make `set_render_audio` a no-op and the "samples reached the player"
assertion goes; make `Sound::do_output` return early instead of calling `Sound::run` — the obvious
next optimisation, and the wrong one, because that is where the channel state machines advance —
and the state comparison goes.

Measured on **the machine a collector actually runs** (an APU installed), Pokémon Blue, one worker,
best of three:

| | ms/frame | fps | WRAM hash |
|---|---:|---:|---|
| pixels on, samples on — what the swarm ran | 0.0832 | 12,012 | `0x19c7ab93d768a82e` |
| pixels off | 0.0549 | 18,219 | `0x19c7ab93d768a82e` |
| **pixels off, samples off** | **0.0483** | **20,695** | `0x19c7ab93d768a82e` |

**+51.7 % for the pixels, +13.6 % for the samples, 1.72x together** — and the WRAM hash is the
same in all three rows, which is the end-to-end evidence for the two constructions rather than a
claim about them.

`ScenarioRun::collecting()` is how a front end declares it; `filming()` wins over it, resolved
together at the top of `play()` so the two commute.

**Declare it before the phase offset, not only in the runner.** A `corpus` attempt burns
`phase_for(seed, spread)` frames before the agent is attached — averaging 1,500 against an
attempt's 3,500, so **a third of everything the program emulates** — and those frames happen before
`ScenarioRun` exists. Leaving the declaration to the runner left that third drawing pixels for
nobody: worth a further 9,941 → 10,646 frames/s once fixed.

### 8c. What a collector's save state carries

A save state holds the framebuffer, so a state produced with pixels off carries the picture as of
the last frame anyone drew. That is exactly what `set_render_pixels` documents, and it has one
visible consequence: **the `final_state` and `start_state` hashes a corpus records move.**

It was measured rather than assumed, on link 02 with the same entry pool under both builds:

- every decision list — **identical**
- every frame count — **identical**
- every ending — **identical**
- `final_state` / `start_state` hashes — **different, on all 40 attempts**

The runs played the same game, button for button and frame for frame; only the bytes of the picture
inside the state differ. Nothing in the repository commits those digests, `verify` is self
consistent within a build, and `pewter_chain verify` is green. If a link's exit state is ever
loaded into a frontend, the first frame drawn after re-enabling pixels is exact — §4, point 2.

### 8d. Where it does *not* apply, and why

`examples/scenario.rs` and `examples/longplay.rs` are headless too and are deliberately left alone.
Both are paced by a live model, so the emulator is not their bottleneck, and both write a save
state on failure that a human then *looks at* — trading a debugging artefact for a speed-up nobody
needs is the wrong trade. The declaration is a front end's to make, which is why it is a builder
and not a default.

**A finding for another crew rather than a change made here:** `terminalgb-mcp` installs an APU
only under `--audio`, so without it the server runs a machine with **no sound chip** — the same
trap `testharness/testroms.rs` records and the one that parks Pokémon Blue in the Pokémon Center.
`set_render_audio` now makes the fix nearly free (install `SilentPlayer`, skip the samples, ~1.5 %
rather than ~14 %), but it changes what that server emulates and belongs in its own commit.

---

## 9. How the swarm scales, how many agents fit, and what a reset costs

### Scaling is silicon-limited, not contention-limited

Pokémon Blue, `--audio --no-pixels --no-audio-out` (the collector's configuration), 2,500 frames a
worker, best of three, on an Intel Core Ultra 7 265: **8 P-cores at 5.3 GHz and 12 E-cores at
4.6 GHz**, no SMT.

| workers | aggregate steps/s | ms/frame per instance | vs 1 worker |
|---:|---:|---:|---:|
| 1 | 20,543 | 0.0488 | 1.00x |
| 2 | 40,271 | 0.0494 | 1.96x |
| 4 | 75,014 | 0.0512 | 3.65x |
| 8 | 148,016 | 0.0521 | **7.21x** |
| 12 | 192,163 | 0.0543 | 9.35x |
| 16 | 250,377 | 0.0568 | 12.19x |
| 20 | 281,879 | 0.0615 | 13.72x |

13.72x from twenty cores reads like a 31 % loss and is not one. The first eight workers deliver
18,502 steps/s each; workers 9–20 deliver 11,155 each — **60 %, which is the P-core to E-core
ratio.** Predicted total from those two rates: `8 × 18,502 + 12 × 11,155 = 281,876`. Measured:
281,879. **The swarm scales as well as the silicon allows**, and the per-instance cost rises only
26 % from one worker to twenty, which is memory pressure and turbo bins.

So "how do we get more throughput" has no answer in contention on this box. It has one in cores.

### How many agents fit: about 3.3 MiB each, and cores bind two hundred times sooner

Peak RSS of the whole process, measured from `/proc/<pid>/status`:

| workers | peak RSS |
|---:|---:|
| 1 | 7 MiB |
| 4 | 17 MiB |
| 8 | 30 MiB |
| 16 | 56 MiB |

**3.27 MiB per additional agent** (about a megabyte of it the cloned cartridge), on a ~4.5 MiB
base. On this 30 GiB box that is a ceiling of roughly nine thousand agents against a core count of
twenty — memory is not the constraint and is not close to being one. **That is why the cartridge is
not shared between agents**: an `Arc<[u8]>` through all six mappers would save a megabyte an agent
of something nothing is short of, and would touch every mapper, the save-state codec and the FFI.

### A reset is not a cost

A certification is thousands of attempts and every one is a `load_state`, so the question is
whether an episode is long enough to amortise it. `core_bench --reset-every N` answers it in the
unit an episode is measured in — by resetting and timing whole frames, never by timing one call:

| `--reset-every` | ms/frame | overhead |
|---:|---:|---:|
| never | 0.0696 | — |
| 3500 | 0.0697 | +0.14 % |
| 1000 | 0.0701 | +0.72 % |
| 300 | 0.0707 | +1.58 % |
| 100 | 0.0682 | −2.01 % |
| 30 | 0.0689 | −1.01 % |

A reset costs roughly **0.3–0.5 ms**, four to seven frames' worth. Against the Pewter chain's
~3,500-frame episode that is 0.01 %. The last two rows are negative, which is the honest way of
saying the effect is under the noise. **An episode would have to be shorter than about fifty frames
before a reset was worth thinking about.**

---

## 10. Three more negative results

Each with the one command that repeats it, so nobody spends the afternoon again.

**An allocator swap buys nothing here.** Tested without adding a dependency at all — `LD_PRELOAD`
of the system tcmalloc, which is the fair test to run *before* proposing a crate:

```sh
LD_PRELOAD=/lib64/libtcmalloc_minimal.so.4 \
  ./target/release/examples/pewter_chain corpus --rom blue.gb \
  --scenario scenarios/pewter/01-new-game.json --runs 160 --workers 16 --quiet
```

Two interleaved rounds each: glibc 57,916 and 49,761 frames/s, tcmalloc 52,481 and 56,934. The
ranges overlap completely and the run-to-run spread on a shared box is larger than any effect —
which the profile predicted, `_int_malloc` and `_int_free_chunk` together being 0.3 % of samples.

**Sharing the cartridge between agents is not worth its blast radius.** See §9: it would save about
a megabyte per agent of memory that is not scarce, and it would change `Vec<u8>` to `Arc<[u8]>`
through all six mappers.

**The per-frame harness is not worth optimising.** `emulate_ms` says 7.5 s of a 7.5 s run loop is
inside `Gameboy::frame`; the profile says every agent-plugin frame together is 0.6 %. Both
instruments agree, which is why this is a conclusion rather than a suspicion.

### And one measurement trap that produced a spectacularly wrong answer

**Never measure throughput with `--features profile` enabled.** `src/profile.rs`'s counters are a
single `static mut` by design (it documents itself as single-threaded), so at twenty workers every
instruction retired writes the same cache line. Measured: **12,686 aggregate steps/s with the
feature on against 246,376 with it off** — a factor of nineteen, and it reads exactly like a
catastrophic scaling collapse in the emulator. Build a separate binary for `--breakdown` and never
quote a throughput number from it.

---

## 11. Reproducing all of it

```sh
# where the frame goes
cargo run --release --features profile --example core_bench -- \
    --rom blue.gb --state overworld.gbss --breakdown

# the build-configuration table (cold-builds seven variants, then scores them
# round-robin so no variant is measured in a different afternoon from another)
tools/perf/build-variants.sh
POKE_ROM=blue.gb POKE_STATE=overworld.gbss tools/perf/pgo.sh
POKE_ROM=blue.gb POKE_STATE=overworld.gbss tools/perf/measure.sh 8 \
    --frames 25000 --warmup 3000

# compile time
tools/perf/build-times.sh

# the SWARM, in its own unit: `corpus`'s last line is emulated frames/s
# aggregate, and the split between inside `Gameboy::frame` and around it
cargo run --release --example pewter_chain -- corpus --rom blue.gb \
    --scenario scenarios/pewter/01-new-game.json --runs 200 --workers 20 --quiet

# how it scales — every point measured once going up and once coming down,
# because this box drifts under sustained load and a one-way sweep hides it
tools/perf/swarm.sh blue.gb

# the collector's own configuration, one worker, and the WRAM hash that says
# the two skips are guest-invisible
cargo run --release --example core_bench -- --rom blue.gb --state s.gbss \
    --audio --no-pixels --no-audio-out --json

# what a reset costs, in the unit an episode is measured in
cargo run --release --example core_bench -- --rom blue.gb --state s.gbss \
    --audio --no-pixels --no-audio-out --reset-every 300 --json
```

**Use long rounds.** An earlier attempt at the variant table used 2,000-frame rounds — 0.3 s each —
and produced results that reordered the variants between runs, twice. At that length the sample is
mostly the CPU frequency governor ramping up after process start. With 25,000-frame rounds two
independent runs agree to about 1 %.

**Report the load average with any number from this box.** The same binary measured 0.0918 ms/frame
with the machine quiet and 0.1336 with eight cores busy elsewhere — a 44 % difference, larger than
every build-configuration effect on this page. `core_bench` prints both clocks so the gap between
them is visible rather than silently folded into the result.

**And that applies twice over to a swarm figure, which is wall-clock by definition and has no CPU
clock to fall back on.** Measured while writing §8: the identical single-worker command read 10,646
frames/s on a quiet box and 7,405 after fifteen minutes of twenty-thread runs — 30 %, all in one
direction. `tools/perf/swarm.sh` therefore measures every point twice, once ascending and once
descending, and prints both: if the two disagree by more than a few per cent the box was drifting
and the sweep is not a measurement. During this work another crew was running a twelve-worker
certification on the same twenty cores, which is exactly the condition that produces it, so every
comparison here is **interleaved A/B** — the only shape that survives steady contention.

No cartridge ships with this repository; every command above needs your own.

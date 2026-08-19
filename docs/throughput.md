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

**§12–§13 are the round after that**, and they are the first where the time was inside the
emulator rather than around it: the timer's per-M-cycle loop, and the pixel loop. Short version:
**+19-22 % on a collector's frame and +24-25 % on a frame with the picture on**, from two
changes that are exactly equivalent to what they replace rather than opt-in trades — plus four experiments that
came back negative, including three separate attempts at the pixel loop.

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

---

## 12. Round three: the timer's loop, and the pixels a word at a time

§8–§10 removed the work nobody consumed. What was left really was the emulator, so this round
starts with a profile of it and lets the profile choose.

### 12.1 Where the time was, before anything changed

`perf` is not installed on this box; the profile is `eu-stack` sampled 800 times against one worker
and attributed **by source line** (`addr2line` over a `debug = 1` build, whose line table follows
inlining), then grouped. Pokémon Blue, an overworld save state, the collector's own configuration
(`--audio --no-pixels --no-audio-out`):

| group | share | what it is |
|---|---:|---|
| the CPU interpreter | 24.0 % | `Cpu::operation`, `get_byte`, the flag helpers |
| **the timer** | **15.5 %** | `Timer::do_mcycle`, per M-cycle |
| the PPU's mode machine | 12.8 % | `Gpu::do_cycle`, `stat_signal` — *not* pixels, which are off |
| MMU address dispatch | 13.4 % | `rb`/`wb`/`read_rom` |
| `bus_tick`'s own frame | 8.1 % | the per-access hook itself |
| the APU | 7.6 % | `Sound::do_cycle` and the channel synthesis |
| the mapper | 7.1 % | `MBC3::readrom` — every opcode fetch |
| OAM DMA | 3.4 % | the "is a transfer running?" test |

**It is flat.** There is no 40 % item left, which is itself the finding: round two's work landed,
and this round is a set of changes worth ten to thirty per cent each rather than one worth a
factor.

The other instrument — `core_bench --breakdown`, which removes one component's work and times whole
frames — agrees and is sharper, because it measures a *marginal cost* rather than a sample count.
It gained three rows here (`no timer batch`, `no apu`, `no oam dma`) and it now takes the run's own
configuration as its baseline, so `--breakdown --audio --no-pixels --no-audio-out` breaks down the
machine a **collector** runs rather than a bare `Gameboy`:

| row | ms/frame | delta | guest-visible? |
|---|---:|---:|---|
| baseline (collector) | 0.0720 | — | — |
| no bus tick | 0.0591 | +21.9 % | yes |
| **no apu** | **0.0592** | **+21.6 %** | yes |
| no timer | 0.0630 | +14.3 % | yes |
| **no timer batch** | 0.0844 | **−14.6 %** | no — the honest "before" for §12.2 |
| no oam dma | 0.0710 | +1.4 % | yes |
| no stat line | 0.0704 | +2.3 % | yes |
| no idle batch | 0.2084 | −65.4 % | no |

and with the picture **on**, on the same frames: **pixels +56.8 %**, apu +20.3 %, bus tick +11.9 %.

It also prints the idle batch's own shape now, which answers a question the counters were already
collecting and nobody was reading:

```text
halted 13275 M-cycles a frame in 349 batches (38.0 each); could not batch because
[dma 0 serial 0 irq 0 no-ppu-horizon 0], bounded by [timer 0 ppu 1048430]
```

**Every batch is bounded by the PPU's next mode edge — never by the timer, never by a refusal, and
never by `IDLE_BATCH_MAX`.** So the cap is not the constraint and raising it would buy nothing.

### 12.2 The timer's loop was three quarters of the timer — closed form, +17 %

A halted CPU is batched up to 64 M-cycles at a time and Pokémon Blue is halted for four fifths of
its frame, so of the ~17,500 `Timer::do_mcycle` calls a frame about **13,300 are iterations of
`Timer::do_cycle`'s `while left >= 4` loop** whose only job is to add four to a counter. The batch
already guarantees nothing *observes* the timer inside that span; what it did not do was tell the
timer.

[`Timer::batch_mcycles`](../src/mmu/timer.rs) counts what the span produces instead of stepping it.
Two facts make that exact rather than approximate:

* `TIMA` counts **falling edges of one bit** of the system counter, whose period `P` is at least
  16 T, and the step is exactly 4 T — so the bit can only fall on the M-cycle that carries the
  counter across a multiple of `P`, and the number of those telescopes to `(start + span)/P −
  start/P`. `P` divides 65536, so the counter's own 16-bit wrap is one of them and 32-bit
  arithmetic counts it correctly.
* The serial shift clock is the same argument with a period of 512 T, taken one M-cycle ahead
  (`Timer::observed_counter` — get that wrong and `acceptance/serial/boot_sclk_align` fails by one
  instruction, which is what the old note in `AGENTS.md` records).

**It declines rather than approximating.** A reload already in flight, a `TIMA` overflow anywhere in
the span, or a counter off the 4 T grid all fall back to the loop, which stays the definition. Those
are a handful of M-cycles a frame.

`timer::tests::the_batch_matches_the_loop_everywhere_it_is_taken` is the proof and it is exhaustive
in the dimensions that matter: **133,120 starting states** — every phase of the longest tap, every
`TAC` including the disabled ones, the `TIMA` values either side of an overflow, and the counter's
own wrap — each stepped both ways and compared field by field, with an assertion that the batch was
actually *taken* on more than 100,000 of them so the test cannot degenerate into checking the loop
against itself. Teeth: count crossings of `P/2` instead of `P`, drop the grid guard, or take the
serial edge from `div` rather than `observed_counter`, and it goes red.

**Measured: `no timer batch` is −14.6 % on an overworld state and −10.0 % on a cold boot**, i.e. the
batch is worth +17.1 % and +11.1 % of those frames respectively. The difference is how much the game
halts.

### 12.3 A monochrome tile, eight pixels at a time — +31 % of the renderer

With the picture on, **pixels are 56.8 % of the frame** and essentially all of that is
`Gpu::draw_map_run`'s pixel loop. That loop is not doing wasted work — it is doing necessary work a
byte at a time: two bit tests, a palette lookup off the stack, three byte stores into the
framebuffer and one into the priority map, 23,040 times a frame.

[`Gpu::blit_dmg_tile`](../src/gpu/mod.rs) does a whole monochrome tile at once:

* a 256-entry `BIT_SPREAD` table turns a tile-data byte into eight bytes, one per pixel, so two
  reads and a shifted OR give all eight colour indices;
* the palette is applied by **mask** rather than by lookup — `x * 0xFF` over bytes that are 0 or 1
  gives `0x00`/`0xFF` per byte, and four masked ORs select between the four broadcast shades;
* the framebuffer is written **two pixels at a time**, read-modify-write so the alpha byte keeps
  whatever it held. Alpha is not ours to choose: `Gpu::clear_screen` writes 255 into it and
  `make test-cross` pins a hash of the whole buffer, so it is preserved rather than assumed.

Everything it cannot handle — a Color tile, a flip, the partial tile at either end of a run — falls
through to the pixel loop, which is unchanged. `gpu::tests::the_word_blit_draws_what_the_pixel_loop_draws`
checks the two against each other over **all 65,536 tile-data pairs** and four palettes, including
the alpha byte it may not touch.

### 12.4 The headline

Pokémon Blue, an overworld save state, **pinned to one P-core** so the measurement is not the
scheduler's choice of core, thread CPU clock, interleaved against a binary built from the parent
commit, best of twelve rounds of 15,000 frames. Load average 19–27 throughout: another crew held
twelve to sixteen of the twenty cores for this whole session.

| configuration | before | after | |
|---|---:|---:|---:|
| collector — `--audio --no-pixels --no-audio-out` | 0.0722 ms/frame | **0.0608** | **+18.8 %** |
| player — picture on | 0.1208 ms/frame | **0.0974** | **+24.0 %** |
| the renderer alone (`baseline − --no-pixels`, same binaries) | 0.0486 ms/frame | **0.0366** | **+32.8 %** |

A second twelve-round run gives +22.2 %, +25.1 % and +30.0 % — so the honest claim is **about a
fifth off a collector's frame, about a quarter off a player's, and about a third off the
renderer.**

**In the swarm's own unit**, `pewter_chain corpus` on link 01 of the Pewter chain, 20 workers, 200
attempts, 987,736 emulated frames — and this is where the box's other occupant hurts most, because
a fan-out figure is wall clock by definition. Four interleaved rounds **alternating which binary
runs first**, because the load was climbing through the run and a fixed order charges that drift to
one side:

| round | before | after | |
|---|---:|---:|---:|
| 1 | 40,666 | 47,577 | +17.0 % |
| 2 | 42,026 | 45,621 | +8.6 % |
| 3 | 43,231 | 44,692 | +3.4 % |
| 4 | 43,333 | 47,161 | +8.8 % |

**Best round against best round, 43,333 → 47,577 frames/s, +9.8 %; median +8.7 %.** An earlier
six-round run in fixed order gave 36,890–44,277 against 43,763–46,618, which is the same answer
with a wider spread. The contention-immune version of the same thing — one worker pinned to one
core, **user CPU seconds**, eight interleaved rounds — is **12.95 s → 12.06 s, +7.4 %**.

**Say why that is +7 % where the emulator is +19 %, because the gap is the interesting part:**

1. **Link 01 is a cold boot, not gameplay.** `--breakdown` measures the timer batch at −10.0 % there
   against −14.6 % on an overworld state: the intro halts the CPU less, and the batch is worth what
   the halting is worth.
2. **About 18 % of a `corpus` attempt is outside `Gameboy::frame`** — `corpus` prints the split
   itself (27.1 s emulating inside 32.7 s of worker time on the run that produced these digests).
3. **The renderer change, the larger of the two, buys a collector nothing at all.** A collector has
   already declared it does not want pixels (§8b). That change is for the frontends and for the
   screen-only student, and it is measured as such above.

**Nothing the cartridge can see moved, and here that is stronger than usual**: a 40-attempt corpus
run under both builds is **identical in every recorded field except `wall_ms`** — every decision
list, every frame count, every ending, and both the `start_state` and `final_state` digests. Round
two's pixel skip moved those digests (§8c); this round's changes do not, because they are
equivalences rather than declarations.

### 12.5 How it scales

The scaling curve §9 established — silicon-limited, the first eight workers delivering 60 % more
each than the other twelve because that is the P-core to E-core ratio — **could not be re-measured
this session and the honest thing is to say so**: with twelve to sixteen of the twenty cores
belonging to another crew all day, a 1-to-20 sweep measures the scheduler. What *is* measurable
under contention is the interleaved pair at each width, and it answers the question this round
actually raises — does the gain survive fan-out?

| workers | before, ms/frame | after, ms/frame |
|---:|---:|---:|
| 1 | 0.1006 | 0.0846 |
| 2 | 0.1045 | 0.0830 |
| 8 | 0.1036 | 0.0854 |
| 12 | 0.1072 | 0.0884 |
| 16 | 0.1029 | 0.0866 |
| 20 | 0.1031 | 0.0890 |

**The per-instance figure is flat in both columns and the gap is the same at twenty workers as at
one** — the change adds no shared state and no contention, which is the property round two's
`--features profile` disaster is a warning about. (The aggregate at 20 workers was 81,258 → 94,349
in the same run, but that number belongs to a box carrying somebody else's certification and should
not be quoted as this machine's throughput.)

`tools/perf/swarm.sh` was run anyway, because its own design says what to do with a drifting box —
every point measured once ascending and once descending, and both printed:

| workers | up | down | x vs 1 |
|---:|---:|---:|---:|
| 1 | 6,006 | 7,515 | 1.00x |
| 2 | 14,363 | 12,415 | 1.91x |
| 4 | 24,641 | 23,682 | 3.28x |
| 8 | 35,921 | 35,085 | 4.78x |
| 12 | 42,390 | 42,825 | 5.70x |
| 16 | 43,771 | 43,566 | 5.82x |
| 20 | 44,667 | 44,052 | 5.94x |

**Read that as a measurement of the box, not of the emulator, and the table says so itself.** The
two directions agree to 2 % from four workers up and disagree by 25 % and 16 % at one and two — a
single worker is a small enough slice of an oversubscribed machine to be at the scheduler's mercy,
which is precisely the condition the up/down sweep exists to expose. And it **saturates at twelve
workers**, gaining 5 % from twelve to twenty where §9's quiet-box sweep gained 46 %: that is what
"about five of the twenty cores are actually free" looks like. §9's curve — silicon-limited, the
first eight workers delivering 60 % more each than the other twelve because that is the P-core to
E-core ratio — is still the shape, and nothing in this round introduces sharing that could change
it. Re-run the sweep on a quiet box before quoting a scaling number from this one.

---

## 13. Four more negative results

Each with the command, because a negative result saves the next person the afternoon — and three of
these four are attempts at the *pixel loop*, which is the most attractive-looking target on the
page.

**Devirtualising the mapper buys nothing, and boxing it does not rescue it.** Every opcode fetch is
a ROM read through `Box<dyn MemoryBankController>`, which is an indirect call that also stops LLVM
inlining a two-line array read into `MemoryManagementUnit::rb`. Replacing it with a closed
`enum Cartridge` that delegates to the same trait — a small, contained change, the trait object
appears in one file — measured **4 % slower** on the collector configuration (0.0599 against 0.0574
ms/frame), and `Box<Cartridge>`, which devirtualises *and* keeps the MMU struct small, measured
0.0611. Both were interleaved against the same third binary in the same runs and both lost twice.
The branch predictor sees one answer for a whole session either way, and inlining the mapper into
`rb` grows the emulator's hottest function. Reverted.

> **And a measurement trap that produced a spectacularly wrong answer first.** An unpinned run of
> that same comparison reported the boxed enum **35 % faster**. This box has 8 P-cores at 5.3 GHz
> and 12 E-cores at 4.6 GHz, and a fresh process lands on whichever the scheduler picks — so the
> per-instance figure, *even though it is CPU time*, moves by 15 % or more between invocations of
> the identical binary. **Pin single-worker comparisons to one core** (`taskset -c 4`). Every figure
> in §12 is pinned; the ones in §8–§9 were taken on a quieter box where this was smaller.

**Three restructurings of the pixel loop all measured worse.** Before the word blit of §12.3, the
obvious shapes were tried and each was interleaved against the same baseline:

| variant | render component |
|---|---:|
| unchanged | 0.0445 ms/frame |
| outputs sliced to the run once (removes four bounds checks a pixel) | 0.0470 |
| per-tile outer loop + `split_at_mut` + a const-generic console | 0.0476 |
| flat loop over `chunks_exact_mut(4)` zipped with the priority map | 0.0472 |

The disassembly says why: LLVM had already hoisted almost everything those changes were aimed at,
and what is left per pixel is the byte traffic. **That is the reason the word blit works and these
do not** — it removes stores rather than branches.

**The APU's per-M-cycle call is not the APU's cost.** `Sound::do_cycle(4)` runs before every memory
access, and it looked like an obvious candidate to accumulate in the MMU and flush lazily at the
points that observe it. Sized first, by simply moving it to the end of the instruction and measuring
the upper bound: **0.0409 → 0.0403 ms/frame, about 1.5 %.** The `no apu` row's 21.6 % is almost all
`Sound::run` — the channel synthesis loops, which still advance duty positions, wave positions and
the LFSR when `set_render_audio(false)` has switched the samples off, because those *are*
guest-visible. Not attempted; recorded as the largest named target left in a collector's frame.

**And two things that were considered and left alone with a reason.** Extending the idle batch past
the PPU's next mode edge when no STAT source is enabled is sound in principle — the counters above
say every batch is PPU-bounded — but it is capped by `IDLE_BATCH_MAX` at 64 M-cycles against a
current average of 38, so it is worth about 3 % and it changes when a STAT edge is sampled.
Merging `Gpu::do_cycle`'s 80-dot chunks up to `Gpu::idle_dots` is the same trade for the same size.

### Reproducing §12 and §13

```sh
# the breakdown, in the collector's own configuration
cargo run --release --features profile --example core_bench -- \
    --rom blue.gb --state overworld.gbss --audio --no-pixels --no-audio-out --breakdown

# the before/after pair: build the parent commit's core_bench into
# target/perf/bin/before, this one into .../after, then round-robin them
POKE_ROM=blue.gb POKE_STATE=overworld.gbss tools/perf/measure.sh 12 --frames 15000

# the timer batch against the loop it replaces, exhaustively
cargo test --lib the_batch_matches_the_loop_everywhere_it_is_taken

# the word blit against the pixel loop, all 65,536 tile-data pairs
cargo test --lib the_word_blit_draws_what_the_pixel_loop_draws
```

**Pin it.** `taskset -c <a P-core>` on every single-worker figure, and interleave; without both, the
run-to-run spread on this box is larger than everything §12 measures.


---

## 14. Round four: the per-dot engine, and the one number nobody had written down

Rounds one to three were all `standard`. This one is `identical` — and it starts by noticing
that the *ratio* between the two engines had never been measured on a real cartridge.
`RenderMode::IDENTICAL_COST` is calibrated on dmg-acid2, which is a still picture; on Pokémon
Blue from an overworld save state the per-dot engine was **3.24×** the batched one
(0.1777 → 0.5757 ms/frame).

### 14.1 Where the time was

`eu-stack`, 400 samples of the worker thread, leaf frame, `identical`, collector configuration:

| symbol | share |
|---|---:|
| `Gpu::do_cycle` (which inlines the dot loop) | 41 % |
| `bg_fetch_dot` | 16.5 % |
| `Gpu::refresh_stat_line` | 8.25 % |
| everything else (CPU, MMU, APU, mapper) | 34 % |

Two ablations sized the candidates. Skipping mode 2's dots outright — wrong, but a
measurement — took 0.5755 → **0.2524**: the **80 OAM-scan dots of every scanline were 56 % of
the whole engine's cost**. Making `refresh_stat_line` a no-op inside the dot loop was worth a
further 8.4 %.

### 14.2 Mode 2 is 80 dots of one job

[`Gpu::oam_scan_span`](../src/gpu/fifo.rs) does the whole run of mode-2 dots the caller's budget
reaches, at once. It is exact rather than approximate because nothing can change under it
mid-call: `do_cycle_accurate` is reached from `bus_tick` with one M-cycle's worth of dots, or
from the idle batch, which is bounded by `Gpu::idle_dots` — so `LCDC.2` (read live, per entry,
because `m3_lcdc_obj_size_change` moves it), `WY`, `LYC` and OAM all hold still. The scan list
is not read until `start_mode3` sorts it, and the STAT line's mode-2 term is asserted for the
whole window, so 79 of the 80 `refresh_stat_line` calls could not have changed anything.

**It costs no row of any suite in either picture mode.**

### 14.3 The per-dot engine had never been able to idle-batch at all

`Gpu::idle_dots` opened with `if self.render_mode != RenderMode::Fast { return 0 }`, so under
`identical` a halted CPU stepped **one M-cycle at a time** — losing the idle batch that
`--breakdown` prices at **−58 % of a frame** on the batched renderer. Pokémon Blue halts for
about four fifths of its frame.

The per-dot engine knows where its own edges are, so it can answer. What it may batch is a dot
count after which nothing a halted CPU can observe has changed, and a halted CPU observes
exactly one thing: an interrupt.

**Only the drawing window is taken, and the reason the others are not is measured.**
`accurate_idle_dots` bounds a mode-3 span by `WIDTH - lx`, because `lx` advances by at most one
per dot, so the mode-0 strobe cannot land before then. Worth **0.535 → 0.332 ms/frame**.

The three windows that are *not* batched all looked defensible on paper — the STAT line's terms
are constant across mode 0, across VBlank and across the OAM window — and each was implemented
and scored against the suites rather than argued about:

| batched window | Gambatte `identical` | other |
|---|---|---|
| mode 3 (**taken**) | +64 rows, −0 | — |
| mode 0 and VBlank | −10 rows (`dma/gdma_cycles_*`, `dma/hdma_start_ly0`), and **blocks 22 more** | — |
| the OAM window | — | dmg-acid2 **8 of 23,040 pixels**, `cgb-acid2` 158 → 166 px |

The OAM one has a mechanism: `mode` still reads 0 at dot 0, so a batch started there runs
across the mode 0→2 transition, which is where the mode-0 term of the STAT line drops.
The mode-0/VBlank one does not, and is left unexplained rather than fitted.

**And the full horizon was also *slower* than the narrow one** on a real cartridge — 0.442
against 0.332 ms/frame — so there was not even a trade to weigh. `identical` spends rows on
nothing.

### 14.4 The two-budget rule this round is the first to use

The captain's framing, and it is what made the table above a decision rather than an argument:
**`identical` is where every last row is chased, and the performance path may take liberties so
long as nothing a game can see changes.** Both halves are load-bearing here. Under one budget
the mode-0 batch would have been a judgement call about ten rows against fifteen per cent; under
two it is simply not `identical`'s to take.

### 14.5 A negative result: the APU's channel loops are not the APU's cost

`--breakdown` prices the whole APU at **+16.7 %** of a collector's frame, and
`SquareChannel::run_span`'s per-expiry loop is the obvious target — it is a `while` over a
period that can be as short as 4 T-cycles, i.e. 17,556 iterations a frame in the worst case.
A closed form for it (the duty position telescopes; the amplitude only changes at the two
pattern edges per eight steps) was written, proved exhaustively equivalent over 36,864 starting
states including the emitted blip deltas sample for sample, and **measured at 0.3 %.**

The reason is one counter, and it should have been printed first: **the square loops run 141
iterations a frame on an overworld state and 87 on a cold boot**, the wave channel 193 and 97,
because real music uses periods of a thousand T-cycles and not four. The ceiling on the whole
idea is 0.4 %. Reverted.

`Sound::do_cycle` is called **11,051 times a frame** and does nothing 99.7 % of them —
`Sound::run` fires 31 times — which is where the APU's cost actually is, and §13 already sized
moving that call at 1.5 %. **The largest named APU target remains `Sound::run`'s channel
synthesis**, which is guest-visible and stays.

### 14.6 Reproducing §14

```sh
# the profile
GB_PPU=accurate ./target/release/examples/core_bench --rom blue.gb --state s.gbss \
    --frames 400000 --audio --no-audio-out &
for i in $(seq 1 400); do eu-stack -p $! -n 2; done   # attribute per THREAD, not per sample

# the ratio, on a real cartridge rather than on a still picture
for m in standard accurate; do
  GB_PPU=$m taskset -c 4 ./target/release/examples/core_bench --rom blue.gb \
      --state s.gbss --frames 5000 --warmup 800 --audio --no-audio-out --repeat 2 --json
done

# the rows, which is the half that decides it
GB_PPU=accurate ./target/release/examples/csp_suite --quiet --suite gambatte \
    --dir target/test-suites/csp --baseline testharness/csp_gambatte_accurate_baseline.txt
```

---

## 15. Round four: the per-access path, and the first look at the field

§12 found the frame flat and said so. This round takes the flat items anyway — five of them,
worth **+16.7 %** together — and then does something the page had never done: **builds two other
emulators and times them here**, because "as fast as possible" needs a ceiling somebody else has
demonstrated rather than one we assumed.

Method as §11: pinned to a P-core, thread CPU clock, interleaved against a binary built from the
parent commit, Pokémon Blue from an overworld save state, load average 19. **Every frame hash and
every WRAM hash byte-identical on every measurement** — these are equivalences, not declarations.

**0.1364 → 0.1136 ms/frame, 123× → 148× real time.**

### 15.1 The profile said something the sample counts did not

`eu-stack`, leaf frame only, 700 samples, attributed by **source line** through `addr2line` with
the PIE bias subtracted:

| line | share | what |
|---|---:|---|
| `Gameboy::frame` (the inlined interpreter) | 33.6 % | dispatch and execute |
| `MemoryManagementUnit::bus_tick` | 13.1 % | the per-access hook |
| `Sound::do_cycle` | 12.9 % | called 11,051 times a frame |
| `MemoryManagementUnit::rb` | 11.0 % | address dispatch |
| `MBC3::readrom` | 10.1 % | every opcode fetch, through `Box<dyn>` |
| `Gpu::do_cycle` | 9.1 % | the mode machine |
| `Gpu::stat_signal`'s early-out, *one line* | 4.5 % | four bool loads |

**Two of those overstate their marginal cost and one understates it, which is why every row
below was measured rather than reasoned about.** `Sound::do_cycle` at 12.9 % of samples turned
out to be worth 4.3 %, and `MBC3::readrom` at 10.1 % to be worth 3.1 % — sampling attributes a
call's return path to the callee. The `stat_signal` line was worth roughly what it looked like.

### 15.2 The APU clock is batched — exactly, not to the end of the instruction

`Sound::do_cycle` runs once per CPU memory access and **does nothing but add to a counter on
99.7 % of them**: the APU acts once every `output_period` (24,344 T), and `Sound::run` fires 31
times a frame against those 11,051 calls.

`MemoryManagementUnit::flush_apu` accumulates instead, and the budget is
[`Sound::cycles_to_output`](../src/sound.rs) — so the flush lands on the same cycle `do_output`
would have and **no sample moves**. §13 sized the cheaper approximation ("move the call to the
end of the instruction") at 1.5 % and left it; this is exact, removes the call rather than
moving it, and is worth **+4.3 %**. Everything that observes the APU flushes first: `catch_up`
before an `NRxx` or wave-RAM access, `div_write` on a `$FF04` write, the instruction's own
remainder, and a speed switch.

### 15.3 The opcode fetch does not go through the trait object

Every opcode and immediate came through `Box<dyn MemoryBankController>`. §13 tried devirtualising
it — a closed `enum Cartridge` — and measured **4 % slower**, because the branch predictor sees
one answer all session and inlining a mapper grows the hottest function.

The answer is not to devirtualise the call but to **not make it**. Each mapper publishes its two
16 KiB windows as byte offsets (`MemoryBankController::rom_window_bases`, re-read after every
write into `$0000-$7FFF`, which is the only thing that can move them) into an `Arc<[u8]>` it
**shares** with the MMU — one allocation, two owners, no copy, so a swarm agent's 3.3 MiB does
not move. `MemoryManagementUnit::fetch` is the fetch-only path: the OAM-bug hook cannot apply to
a program counter, and the boot overlay and a DMA conflict are both still tested. **An index
outside the ROM falls through to the mapper**, so MBC5 keeps answering `0` where the others
answer `$FF`. Worth **+3.1 %** as a cache and **+4.6 %** once `fetch` skips the dispatch as well.

### 15.4 Three one-line items

* `Gpu::stat_signal`'s early-out was four bool loads; it is one packed `any_inte`, re-derived
  where the four are written.
* `OamDma::idle()` was two loads from the far end of a 150 KiB struct on **every memory access**
  to answer "no". The MMU caches the answer beside the rest of its per-access state — the
  function's own doc comment predicted this.
* `MemoryManagementUnit::do_cycle`'s unconditional `intf |= x.interrupt; x.interrupt = 0` pairs
  are gated on the event having happened, which is the argument `bus_tick` already applies to
  the timer, extended to the keypad, the serial port, the PPU and the serial shift clock.

### 15.5 Two negative results, both from the same wrong intuition

Both were "remove work from the common path", both measured **worse**, and both are the same
lesson: **once a check is one packed test, replacing it with a branch is a loss.**

| change | result |
|---|---|
| skip `refresh_stat_line` on PPU chunks that moved neither the mode nor the line | 0.1138 → 0.1147, **0.8 % slower** |
| gate `Serial::take_pending` and `check_and_reset_gpu_updated` on the event, instead of storing unconditionally | 0.1138 → 0.1168, **2.7 % slower** |

The second is the more interesting: those two stores really do dirty a cache line once per
instruction for events that happen ~137 and ~1 times a frame, which is exactly the shape §12's
`bus_tick` note treats as waste. It is not waste when the line is hot anyway and the branch is
not free.

### 15.6 And then: what does anybody else actually do?

Full page, with the method and the licences: [`docs/measured/against-the-field.md`](measured/against-the-field.md).
Short version, all on this box, one core, one ROM, 7,168 frames from reset, thread CPU clock,
every side optimised:

| | ms/frame | × real time | cycle-accurate? |
|---|---:|---:|---|
| SameBoy (`-O3 -ffast-math` + LTO), render off | 0.3854 | 43× | yes |
| **TerminalGB**, pixels off | **0.0405** | **414×** | yes |
| PyBoy 2.7.0 (Cython), render off, sound off | 0.0193 | 869× | no |

(Best of three interleaved on a quiet box; the same set at load 16 read 28×, 249× and 562×,
which is the same ordering with every row depressed together.)

**The "1,000-10,000× for a fast interpreter" figure is not what a cycle-accurate emulator does
on this hardware — the reference implementation reaches 43× and we are 9.5× past it.** PyBoy's
real 2.3× lead is bought by not being cycle-accurate, and its technique is in its own source
comment: every peripheral publishes a `_cycles_to_interrupt` horizon and is advanced only when
one is reached. That is our idle batch, applied always rather than only while the CPU is halted.

Costing the parts on both sides says where the gap is and it is not where it looked: **our
renderer is within 16 % of PyBoy's and our silent APU is an order of magnitude cheaper. The
whole difference is the core**, and specifically that we do peripheral bookkeeping once per
*instruction* where PyBoy does it once per *horizon*. Dropping the per-access tick — the accuracy
we keep on purpose — is only **+13.6 %** of that.

**So the next structural item is named: make the timer and the PPU lazy the way the APU already
is.** `Sound::catch_up` is the proof the pattern costs no accuracy. **§16 does the PPU half**;
the timer is what is left.

---

## 16. Round five: the PPU is advanced lazily — +17.5 %, and the technique came from somebody else

§15 measured PyBoy at 2.3× this emulator and read its source for *why*. The answer was in its own
comment: every peripheral publishes a horizon and is advanced only when one is reached. This is
that idea applied where we do not need sub-instruction resolution, and it is the largest single
win on this page since the idle batch itself.

**0.1134 → 0.0965 ms/frame, 147× → 173×**, interleaved on one P-core against the parent commit,
**frame hash and WRAM hash byte-identical**, and **all 5,320 Gambatte rows match in both picture
modes**.

### 16.1 The batched renderer paid a whole chunk per call

`Gpu::do_cycle` was called once per instruction with that instruction's dots. Whatever the
count, the call pays a full pass: the line-boundary test, the three-way mode test, and
`refresh_stat_line`. Twelve dots and eighty dots cost the same.

`MemoryManagementUnit::flush_gpu` accumulates the dots instead and applies them together. A
crude sizing hack — defer by a fixed eight instructions, which is wrong — measured **+47 %**,
which is what made it worth doing properly.

### 16.2 The bound, and why it is exact

[`Gpu::defer_dots`](../src/gpu/mod.rs) is the distance to the next dot at which the PPU does
anything a CPU can observe — a mode edge, an `LY` increment, a Color console's VBlank strobe
lead. It is the same horizon `Gpu::idle_dots` already hands the halted CPU.

`MemoryManagementUnit::do_cycle` flushes on the instruction whose **own dots reach** that
horizon, not on the one after, so a mode change, a STAT or VBlank interrupt, `updated` and the
HBlank latch all land on exactly the instruction they landed on before. Everything that reads
the PPU out of turn flushes first: VRAM, OAM, the `$FF4x` and `$FF6x` registers, a VRAM DMA, an
OAM DMA, the OAM-corruption hook, a render-mode switch and a save state.

It returns **zero for the per-dot engine**, which is stepped per bus access by design, and with
the LCD off, where `do_cycle` returns at its first line anyway.

### 16.3 Four things the gates caught, in the order they caught them

This is the value of a suite whose expectations are somebody else's. Each of these was a real
defect that a plausible-looking implementation had, and each was named by the rows that moved.

| what the rows said | the defect |
|---|---|
| 210 rows across `miscmstatirq`, `halt`, `lycEnable`, `m0enable` | `bus_tick` hands the PPU dots **directly** during interrupt dispatch, so anything still owed has to go in first or the two arrive out of order |
| the same groups, still 181 | `flush_gpu` folded `gpu.interrupt` into `intf`. A flush can now happen **part-way through an instruction** — a `$FF41` read causes one — so folding there handed the CPU a STAT interrupt earlier than the batched engine ever did. The fold stays at the end of the instruction. |
| 38 rows, all `halt/late_m0int_halt_m0stat_*` | `idle_mcycles` measured its horizon from the PPU's **lagging** position, so a halted CPU idled `gpu_owed` dots too long and woke late. It subtracts the owed dots now. |
| `savestate`: "state diverged one instruction after loading" | **a save state is an observation.** `Gameboy::save_state` is `&mut self` and brings the PPU current, which is also why this needed no `savestate::FORMAT_VERSION` bump and every existing state still loads. |

The OAM-corruption hook needed the same treatment for the same reason — it predicts which OAM
row a bus cycle hits from the PPU's own free-running dot counter, so the counter has to be
current.

### 16.4 What it does not do

**Nothing for `identical`**, by construction: the per-dot engine is the accuracy engine and is
stepped per bus access. And nothing for a machine with the LCD off. The gain is the batched
renderer's, which is the default and the one a collector runs.

### 16.5 The session, end to end

Cold boot, collector configuration, interleaved against the commit this session started from,
one P-core, thread CPU clock, **quiet box (load 1.3)**:

| | ms/frame | × real time |
|---|---:|---:|
| session start | 0.0592 | 283× |
| **after §15 and §16** | **0.0454** | **369×** |

**+30.4 %**, with every frame hash and every WRAM hash byte-identical at each step and every
gate green throughout. With no APU installed at all — PyBoy's configuration — the same build is
**0.0405 ms/frame, 414×**.

**What is left, in order of what it is worth.** The timer is the other half of §16's idea and is
priced at 13.0 % by `--breakdown`. The per-access tick is 13.6 % and is accuracy we keep. After
those the profile has nothing above 6 % and the next thing would be the interpreter itself.

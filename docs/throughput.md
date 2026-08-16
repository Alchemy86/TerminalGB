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
  `build-times.sh`
- **Where the frame goes:** `core_bench --breakdown` (needs `--features profile`)

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

**The APU is already free for this caller** and needs no equivalent switch: `Sound::do_cycle`
returns immediately when no player is installed, and a headless machine has none. There is nothing
to skip.

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

## 7. Reproducing all of it

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
```

**Use long rounds.** An earlier attempt at the variant table used 2,000-frame rounds — 0.3 s each —
and produced results that reordered the variants between runs, twice. At that length the sample is
mostly the CPU frequency governor ramping up after process start. With 25,000-frame rounds two
independent runs agree to about 1 %.

**Report the load average with any number from this box.** The same binary measured 0.0918 ms/frame
with the machine quiet and 0.1336 with eight cores busy elsewhere — a 44 % difference, larger than
every build-configuration effect on this page. `core_bench` prints both clocks so the gap between
them is visible rather than silently folded into the result.

No cartridge ships with this repository; every command above needs your own.

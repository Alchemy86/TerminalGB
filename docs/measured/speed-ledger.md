# The speed ledger — every optimisation attempted, and what each was worth

One consolidated record: what the emulator runs at, what bought it, and — at
greater length, because they are worth more — **what was tried and did not work.**
A negative with a number attached stops the next person spending a week on advice
that sounds authoritative and does not hold here.

Deeper narrative per round lives in [`../throughput.md`](../throughput.md); this
page is the standing summary. Modes: [`../modes.md`](../modes.md).

---

## 0. How everything here was measured

Every number on this page follows the same method, and a number that does not
say otherwise was taken this way:

- **`examples/core_bench`**, which reports **thread CPU time** from
  `/proc/thread-self/schedstat` and prints the wall figure beside it. A wall
  clock on a shared box measures the box's afternoon: the same binary reads
  0.0918 ms/frame quiet and 0.1336 with eight cores busy elsewhere.
- **Pinned to one P-core** (`taskset -c 4`). This box is 8 P-cores at 5.3 GHz
  and 12 E-cores at 4.6 GHz and a fresh process lands on whichever the
  scheduler picks, so an unpinned single-worker figure moves 15 % between
  invocations of the *identical* binary. One experiment reported **35 % faster**
  unpinned and 4 % *slower* pinned.
- **Interleaved A/B**, alternating binaries within one session, best of three or
  more rounds. Never two runs an hour apart.
- **Long rounds** — 9,000 to 25,000 frames. At 2,000 frames the frequency
  governor reorders the variants between runs, twice.
- **The `standard` picture engine**, which was the default when every figure
  here was taken and is not one now
  ([`picture-modes.md`](../picture-modes.md), 2026-08-20). Put
  `GB_PPU=standard` in front of any command on this page — a bare `core_bench`
  runs the per-dot engine and reads about four times slower, and speed mode is
  refused there anyway, so every row below would be measuring nothing.
- **The load average is stated** whenever it is not near zero.
- **Frame hash and WRAM hash compared** either side of every change. A frame
  hash proves the picture; the WRAM hash proves the cartridge's own memory,
  which is the one that says a game cannot tell.

Aggregate figures are **wall clock by definition** and start behind a barrier,
so warmup is not divided into the total.

---

## 1. Where it stands

Pokémon Blue, one P-core, quiet box, collector configuration unless stated.

| | before | now | |
|---|---:|---:|---|
| `standard`, player configuration | 0.0592 | **0.0454** ms/frame | 283× → **369×** (+30.4 %) |
| `standard`, certification configuration | — | — | **414×** (no APU installed) |
| `identical` | 0.5755 | **0.3320** | 29× → **50×** (+73 %) |
| aggregate, 20 workers | — | — | **3,735×** real time |
| concurrent copies at full DMG speed | — | — | **~820** (`identical`), **~5,000** (`standard`) — [`concurrency.md`](concurrency.md) |
| aggregate, 4 workers | — | — | 1,066× — past 1000× on four cores |
| certification run, `GB_SPEED=speed` | — | — | **+27 %** cold boot and **+39 %** on an overworld state, over the accurate machine — and the chain runs identically |

**Frame hash and WRAM hash are unchanged across every one of these.** The
emulated machine did not move; only what it costs to run it did.

Scaling is silicon-limited rather than contention-limited: the first eight
workers deliver 18,502 steps/s each and workers 9–20 deliver 11,155 — 60 %,
which is the P/E core ratio, and `8 × 18,502 + 12 × 11,155 = 281,876` against a
measured 281,879.

---

## 2. What bought it

| change | worth | how it is safe |
|---|---:|---|
| **Lazy PPU advance** (`flush_gpu`, `Gpu::defer_dots`) | **+17.5 %** on its own round, **+23.1 %** of the standard frame measured end to end | the bound is the distance to the next dot the CPU could observe — a mode edge, an `LY` increment, the Color VBlank strobe lead. Exact, and 5,320 Gambatte rows say so in both picture modes. |
| **`Arc<[u8]>` ROM window** — the opcode fetch indexes the ROM directly | +4.6 % | one allocation, two owners, no copy. An index outside the ROM falls through to the mapper, so MBC5 still answers `0` where the others answer `$FF`. |
| **APU flush budget** (`flush_apu`, `Sound::cycles_to_output`) | +4.3 % | the flush lands on the same cycle `do_output` would have, so **no sample moves**. `Sound::do_cycle` was called 11,051 times a frame and did nothing but add to a counter on 99.7 % of them. |
| **Batched OAM scan** (`Gpu::oam_scan_span`) | 56 % of the per-dot engine, by ablation | exact: nothing changes under it mid-call. `LCDC.2` is read live per entry, and `WY`, `LYC` and OAM all hold still. |
| **Idle batching** for a halted CPU | −58 % of a frame when it landed | Pokémon Blue is halted for four fifths of its frame. Bounded by the same horizon. |
| **Timer closed form** (`Timer::batch_mcycles`) | +17.1 % of an overworld frame | proved against the loop over 133,120 starting states, with an assertion that the batch is actually taken on 100,000 of them so it cannot degenerate into checking the loop against itself. |
| **Word-at-a-time monochrome tile blit** | +32.8 % of the renderer | checked against the pixel loop over **all 65,536** tile-data pairs. |
| Cached `oam_dma_busy`; packed STAT enable bit (`any_inte`) | ~1 % each | pure caching of a value already computed. |
| `[profile.release]` `opt-level = 3` + fat LTO | **+24 %** | it had said `opt-level = "s"` since 2022 and nobody here ever chose it. |
| **The horizon batch** (`SpeedFlags::horizon_batch`) — the peripherals advanced once per *horizon* rather than once per instruction | **+13 %** cold boot and **+21 %** on an overworld state, on top of the preset it joins | **it is a flag, not a default**, because unlike the row above it is not lossless. What makes it safe enough to certify is the flush list, not the horizon: every `$FE00-$FF7F` access, every interrupt dispatch, every halt, every save state and every frame boundary empties the batch first, so a program's own reads are exact. Byte-identical to `instruction-bus` over 92 Pokémon save states and over the 26-link chain — see [`../modes.md`](../modes.md) §4b. |

---

## 3. The measured negatives

**These are the more valuable half of the page.** Every one is a standard
recommendation that does not hold on this codebase, with the number that says so.

| tried | result | why it does not hold here |
|---|---:|---|
| **`-C target-cpu=native`** | **3.3 % slower** (0.0608 → 0.0628) | no `.cargo/config.toml`, which is the good outcome — a flag file would silently make released artefacts non-portable. |
| **PGO** | **8.1 % behind** the shipping build | `-Cprofile-use` refuses fat LTO on the pinned rustc 1.83, so it yields a *thin* binary. fat 0.0616, thin 0.0694, thin+PGO 0.0666. PGO is +4.2 % over its fair comparator and still loses. The LTO it costs is worth more than the profile it buys. |
| **Devirtualising the mapper** (`Box<dyn>` → `enum Mbc`) | **moot**, and 4 % slower when tried | counted: `readrom` **0.0 calls a frame**, `readram` 0.0, `writeram` 0.0, `writerom` 29.1. The `Arc<[u8]>` window already took the read path off the trait object. |
| **A page-table memory map** | reaches **~1 %** of accesses | 4,764 of a frame's 6,607 reads are fetches that already bypass the dispatch and 914 more are ROM taking one compare. The hoist that measured +0.7 % was **incorrect** — it skipped `wram_bus_address`, and the hashes only matched because this cartridge does not exercise it. |
| **Flag packing** — one branchless `set_flags` | **neutral** (0.0624 → 0.0626) | LLVM had already coalesced the read-modify-writes. 46 rewrites of the most correctness-critical file in the CPU, for nothing. |
| **Deferring the timer** the way the PPU is deferred | **8.5 % slower** (0.0436 → 0.0473) | the PPU pays a chunk of bookkeeping per *call* whatever the dot count; the timer pays per M-cycle because per M-cycle is what it does. |
| **Gating per-instruction stores** on their event | **2.7 % slower** | it really does dirty a cache line for an event that happens ~1 time a frame — and that is not waste when the line is hot anyway and the branch is not free. |
| **A closed form for the APU channel loops** | **0.4 % ceiling** | proved exhaustively equivalent over 36,864 states including the blip deltas, then measured at 0.3 %. **Print the counter first:** the square loops run 141 iterations a frame, because real music uses periods of a thousand T-cycles, not four. |
| **Bounds-check elision** via masked bank indices | **neutral to 0.4 % slower** | only 591 work-RAM reads a frame and the branch predicts perfectly. |
| **An allocator swap** (`tcmalloc` by `LD_PRELOAD`) | **no effect** | ranges overlap completely, and the profile predicted it: `_int_malloc` + `_int_free_chunk` are 0.3 % of samples. Tested without adding a dependency, which is the fair test to run before proposing a crate. |
| **`codegen-units = 16`** | not measurably slower, halves the cold build | left at 1 anyway, on caution not a number: after fat LTO the partitioning is an LLVM heuristic. |
| **Giving `MemoryManagementUnit::do_cycle`'s body a second caller** | **5-7 % slower**, three ways | see §3a — the finding that shaped the horizon batch, and the reason its flush path is a *different function* rather than a second call of the same one. |

### 3a. The one-call-site rule — a negative that turned into a design constraint

**The per-instruction body must have exactly one call site, or it stops being
inlined into `Gameboy::frame` and the accurate machine loses 5 %.**

`MemoryManagementUnit::do_cycle`'s body is ~4 KiB of machine code and in the
shipping build it is inlined into `Gameboy::frame`, which is the emulator. Every
attempt to reach it from a second place cost the same 5-7 %, and the shape of
the attempt did not matter:

| what was tried | accurate frame | what `nm` said |
|---|---:|---|
| body split out, `#[inline]`, called from `do_cycle` **and** the flush | **5 % slower** | `do_cycle_now` appears as its own 4,242-byte symbol; `Gameboy::frame` **shrinks** by 554 bytes |
| the same, with the branch that chooses between them compiled out | **6.7 % slower** | unchanged — so it is not the branch |
| body `#[inline(always)]`, second caller `#[inline(never)]` and cold | **6.6 % slower** | `frame` grows; a second 3.6 KiB copy exists |
| body reached through `do_cycle(0)` so there is one *body* but two callers of `do_cycle` | **9 % slower** | the wrapper is outlined instead |
| **body left with one call site; the flush given its own smaller function** | **0 %** (0.0407 against 0.0407) | `frame` unchanged |

Measured on one P-core, Pokémon Blue, 7,168 frames from reset, interleaved
against a binary built from the parent commit, three rounds each.

**The corollary bit twice more in one afternoon and is the useful half:** the
same is true of anything the body calls. Adding a second caller for
`Timer::do_cycle` took *it* out of `Gameboy::frame` too — it has no symbol in
the shipping build and gains one — and cost 3 % until it was marked
`#[inline(always)]`. **Check `nm --size-sort -S` for a symbol that was not there
before**; it is a two-second test and it explains a whole class of "why did that
get slower?".

### 3b. What the flag costs the machine that does not use it

**About 2 %, and it is the flush test in every memory-accessing opcode.**

`Gameboy::frame` is the emulator — the interpreter, `read_at` and `wb` are all
inlined into it — so the two `$FE00-$FF7F` tests that empty the batch are not
one branch, they are a branch at every one of the dozens of sites `read_at` and
`wb` are inlined at. `frame` grows from 32,168 bytes to 35,199.

Measured best-of-8 over 25,000-frame rounds, one P-core, thread CPU clock,
interleaved against a binary built from the parent commit, load average 4.3-6.9:
**−2.3 % on a cold boot and −1.6 % on an overworld state.** Two shapes of the
test were tried (`pending_ticks != 0 && range` and `horizon_batch && range`) and
a `#[cold]` horizon; they are all within noise of each other.

**It is stated rather than hidden**, because this page's own rule is that a
difference under 2 % is code layout and this one straddles that line. The
version that would cost nothing is to move the flush into `read_at`'s twenty
individual I/O match arms instead of testing the range in front of them —
cheaper on the hot path and one missed arm away from a silent correctness bug,
which is why it was not taken for a 2 %.

Agreed and **not attempted**: computed goto (Rust has no labels-as-values, and
the opcode `match` is already a jump table — four of rustc's, verified in
`objdump`), and mmap + `SIGSEGV` for bank switching (a signal handler in a
library's hot path, and incompatible with the wasm target this project ships).

Deprioritised **with numbers** rather than skipped: a pre-decoded tile cache and
SIMD tile decoding help only a *player*. The whole renderer is 0.0434 ms of a
player's 0.1106 ms frame and a collector does not run it at all, so an
optimistic third of the renderer is ~9 % of a player's frame and **0 % of a
certification run's**.

---

## 4. The bus tick — refused, then reshaped

The per-access bus tick is the largest single accuracy cost in the emulator.
`core_bench --breakdown` prices skipping it at 17.7 % — and its `wram` column
reads **`wram MOVED`**: Pokémon Blue's own memory ends up different, so it is not
a liberty no game can see, and it stayed out of both picture modes.

**That refusal was right about the shortcut it measured and wrong as a general
conclusion**, and the difference matters. `--breakdown`'s switch returns from
`bus_tick` immediately, which *also* stops the PPU stepping through interrupt
dispatch — and a PPU frozen through a dispatch cannot redirect it, which is
observable. The narrower shortcut that keeps the dispatch stepping is
[`SpeedFlags::instruction_bus`](../modes.md), and on the same state its **WRAM
hash is byte-identical**. It is worth **9.3 %** in the shipping build, not 17.7 %
— the larger figure is measured with the `profile` feature on, which perturbs.

Two lessons, both general:

- **A shortcut is not one thing.** Two versions of "skip the bus tick" differ by
  one branch and one of them is observable.
- **Do not quote a `--breakdown` row as a shipping figure.** It is for *ranking*
  components; the counters it needs are a `static mut` and they cost.

---

## 5. Against the field, built and timed here

Not quoted from anybody's README. One core, one cartridge, 7,168 frames from
reset, thread CPU clock, every side optimised, interleaved in one session.

| | ms/frame | × real time | cycle-accurate? | Mealybug (24 published rows) |
|---|---:|---:|---|---:|
| SameBoy, `CONF=release`, render off | 0.5925 | **43×** | yes | 15 |
| **TerminalGB**, pixels off | 0.0692 | **414×** | yes | **26** |
| **TerminalGB**, pixels off, `GB_SPEED=speed` | — | **+27 % over the row above** | **no — that is what the flag is** | unchanged (`identical` refuses every flag) |
| PyBoy 2.7.0 (Cython), render + sound off | 0.0298 | **869×** | **no** | **0** |

So we are **~9.6× SameBoy at equal accuracy** and **~2.1× behind PyBoy, which
scores zero where we score 26**. `tools/perf/{sameboy-bench.c,pyboy-bench.py}`
make both reproducible.

**The "1,000–10,000× for a fast interpreter" figure in circulation is not what a
cycle-accurate emulator does on this hardware.** The reference implementation —
the same SameBoy this project already pins as an oracle, with `-ffast-math` and
LTO — reaches 43×. Arithmetic worth keeping: 1000× is about **one host clock per
Game Boy T-cycle** at 5 GHz, and 10,000× is a tenth of one, which is not an
interpreter.

**Cost the parts before blaming one.** Turning each half on and off on both
sides: our renderer is within **16 %** of PyBoy's (+0.0434 against +0.0374
ms/frame) and our silent APU is an order of magnitude cheaper (+0.0034 against
+0.0255). The entire gap is the core, and specifically that
`MemoryManagementUnit::do_cycle` does peripheral bookkeeping **once per
instruction** where PyBoy does it once per horizon.

Both were **read for technique and cited, never copied**: PyBoy is LGPL-3.0-only
and SameBoy is Expat. Neither has a line of code in this repository.

---

## 6. Where the frame goes now, and what is queued

Line-level profile of the standard frame on a quiet box (`eu-stack` sampling,
attributed by source line with the PIE bias subtracted):

| | share |
|---|---:|
| `bus_tick`'s entry | 9.4 % |
| `Cpu::operation`'s `match op` | 6.8 % |
| `MemoryManagementUnit::fetch`'s ROM read | 6.3 % |
| `Timer::do_mcycle` | 6.1 % |
| everything else | nothing above 3 % |

**It is flat, and that is the finding.** What is left at the top is the
per-access hook, the opcode dispatch and the fetch — the three things an
interpreter *is*.

Queued, in the order the profile ranks them:

1. ~~**Make the timer and the PPU lazy the way the APU already is**~~ —
   **closed.** The PPU half landed in `docs/throughput.md` §16 (+17.5 %, in the
   default and lossless); the timer half is §3 above at 8.5 % slower; and §18
   took the *whole per-instruction pipeline* to a horizon as
   `SpeedFlags::horizon_batch`, which is +13 % to +22 % and is a **flag**
   because it is not lossless. There is no lazy-peripheral item left.
2. **A different execution model** — block compilation or threaded code. Sizing
   from the published GBA figures a JIT is about 6× over a good interpreter, and
   worth stating: a JIT for a *cycle-accurate* emulator is materially harder,
   because you cannot reorder memory accesses whose sub-instruction timing is
   observable — and §4 above proves that is not hypothetical here.
3. **More speed flags.** The mechanism is built — named switches, a preset over
   them, and a flag earning the preset by re-running the certified links rather
   than by argument ([`../modes.md`](../modes.md)). What is queued is *more
   flags*, and the honest note is that the obvious ones are taken: the two that
   exist beyond the preset buy 0.0 % and +25.9 % respectively, and the second
   is out because it freezes a whole peripheral. The next real one comes from
   §6.1, not from looking harder at what is already there. **§18's
   `horizon-batch` is what that sentence predicted** — it came from the profile
   and from reading somebody else's source, not from the flag list.

Not queued, and deliberately: anything that buys a player frames it does not
need. The frontend is paced to 59.7275 Hz and has been comfortably inside its
budget since the transport work.

---

## 7. Three traps that produced a wrong answer first

- **A between-binary difference under 2 % is code layout, not your change.**
  With fat LTO a change in one function moves everything after it: two binaries
  differing only in `draw_map_run` reported **pixels-off** figures 2 % apart, on
  a path that never calls it. Measure an isolated component as a *difference
  within each binary*.
- **A fan-out sweep with a fixed frame count per instance measures the box's
  clock, not the emulator's scaling — and it manufactures a collapse.** The
  P-cores here hold 4,540 MHz for about five seconds of all-core load and then
  settle at **3,100 MHz** for as long as the load lasts, and 4540/3100 predicts
  the per-frame cost to within 0.3 %. So the small points of a sweep finish at
  burst clock and the large ones run almost entirely at sustained clock, and
  the difference reads as a 40 % scaling collapse past 200 copies. The control
  needs no instance count: the identical 20-worker 2,000-frame run reads
  **0.2145 ms/frame cold, 0.3116 straight after a long run, and 0.2197 again
  after 45 s idle**. Warm every point to steady state at its own load
  (`core_bench --warm-seconds`), run points back to back, and scale frames so
  every timed window is the same wall duration —
  [`concurrency.md`](concurrency.md) and `tools/perf/fanout.sh`.
- **Never measure throughput with `--features profile` enabled.** Its counters
  are a single `static mut` by design, so at twenty workers every retired
  instruction writes the same cache line: **12,686 aggregate steps/s with it on
  against 246,376 with it off**, a factor of nineteen, and it reads exactly like
  a catastrophic scaling collapse in the emulator.

# Against the field — measured here, on this box, with the same method

<!-- codemap:asof -->

> **"Ensure our efforts to boost speed actually try to verify some of the claims
> made on the others. If true we can just compare code and see where they gained the benefits."**
> — the captain, and he is right: a summary of what other emulators achieve is not a
> measurement, and this page exists because two of those claims went opposite ways when they
> were actually run.

Every figure here was produced on **one machine, one core, one cartridge, one frame count, and
the same clock**. Nobody's marketing number appears.

---

## 1. The method

* Pokémon Blue, **7,168 frames from reset** — 120 emulated seconds — on every emulator.
* **Pinned to a single P-core** (`taskset`). This box is 8 P-cores at 5.3 GHz and 12 E-cores at
  4.6 GHz, and an unpinned process lands on whichever the scheduler picks, which moves the
  answer by 15 % or more between runs of the *identical binary*.
* **Thread CPU time**, not wall clock, on every side.
* **Every side optimised.** SameBoy is built with its own `CONF=release`
  (`-O3 -ffast-math -DNDEBUG` plus LTO); ours is `[profile.release]` (`opt-level = 3`, fat
  LTO); PyBoy is the published Cython wheel.
* **Rendering and sound off on every side** where the emulator offers it, because that is the
  configuration a headless measurement campaign runs in.
* Load average stated: **16.5** throughout, another crew holding most of the box. That
  depresses *every* row equally through frequency scaling, which is exactly why the comparison
  is run in one session rather than against a remembered number.

Reproduce it: [`tools/perf/sameboy-bench.c`](../../tools/perf/sameboy-bench.c),
[`tools/perf/pyboy-bench.py`](../../tools/perf/pyboy-bench.py), and
**`GB_PPU=standard`** `core_bench --rom blue.gb --frames 7168 --no-pixels`.

> **`GB_PPU=standard` is required now and was not when this was measured.** These
> rows predate 2026-08-20, when `identical` became the default picture engine
> ([`picture-modes.md`](../picture-modes.md)) — a bare `core_bench` runs the
> per-dot engine and reads about four times slower, which would look like a
> catastrophic regression and is a different measurement. `standard` is the fair
> comparator here: it is what a throughput campaign runs, and both of the other
> emulators in the table are being run with their rendering off.

## 2. The result

| emulator | ms/frame | × real time | cycle-accurate? |
|---|---:|---:|---|
| **SameBoy** (Expat, `-O3 -ffast-math` + LTO), rendering off | 0.3854 | **43×** | yes |
| **TerminalGB**, pixels off, no APU | **0.0405** | **414×** | yes |
| **PyBoy 2.7.0** (LGPL-3.0, Cython), rendering and sound off | **0.0193** | **869×** | **no** |

**We are 9.5× faster than SameBoy and 2.1× slower than PyBoy** — and 1.7×
slower with `GB_SPEED=speed`, which is the horizon batch taking PyBoy's own
technique as a named, opt-in liberty (§4).

All three rows are the best of three runs **interleaved with each other on the same core in the
same minute**, on a quiet box (load 1.3). A single-shot reading of any of them drifts by 30 % as
the box's other occupant comes and goes — an earlier set taken at load 16 read 28×, 249× and
562×, i.e. the same ordering and the same ratios with every row depressed together. **Quote the
load average or do not quote the number.**

### And where PyBoy's 2.3× is, which is the useful half

Costed by turning each part on and off on both sides in the same session:

| component | PyBoy | us |
|---|---:|---:|
| **core only** (no rendering, no sound) | **0.0198–0.0298** | **0.0672** |
| the renderer, on top | +0.0374 | +0.0434 |
| the APU, on top | +0.0255 (producing samples) | +0.0034 silent, +0.0400 producing |

**Our renderer is within 16 % of PyBoy's and our APU is an order of magnitude cheaper when
silent, which is the configuration a collector runs in. The entire gap is the core.**

## 3. The claim that did *not* hold

The figure in circulation is that a fast Game Boy interpreter reaches **1,000–10,000× real
time**. On this hardware, with this method, **the reference cycle-accurate emulator reaches
43×.** SameBoy is not a slow implementation — it is the accuracy oracle this repository already
uses to settle disputes ([`tools/sameboy-oracle`](../../tools/sameboy-oracle/README.md)), it is
written in C, and it is compiled with `-ffast-math` and LTO.

So "1,000× single core is roughly where we should already be" is **not supported by anything
measurable here**. Whatever those figures describe, it is not a cycle-accurate emulator on a
5 GHz x86 part. Some arithmetic for why: 1,000× is 16.7 µs for 70,224 emulated T-cycles, i.e.
**about one host clock per Game Boy T-cycle**. 10,000× would be a tenth of a host clock per
T-cycle, which is not an interpreter at all.

**There is no technique to copy from SameBoy, because we are ahead of it.** The difference is
structural and it is ours: SameBoy advances every peripheral on a T-cycle grid, while this
emulator ticks them **per bus access** and batches the M-cycles a halted CPU spends
([`MemoryManagementUnit::idle_mcycles`](../../src/mmu/mod.rs), worth −58 % of a frame on its
own — [`docs/throughput.md`](../throughput.md) §12).

## 4. The claim that *did* hold — and where its win comes from

PyBoy really is 2.3× faster than us headless, and it is worth being precise about what it is
buying that with.

**PyBoy is not cycle-accurate**, and this repository's own scoreboard already records the
consequence: on the GB Emulator Shootout's 24 published Mealybug rows, **SameBoy scores 15, bgb
scores 2, PyBoy scores 0** ([`docs/mealybug.md`](../mealybug.md)), where the per-dot engine here
scores <!--score:mealybug_accurate-->29 / 79<!--/score-->.

Its technique is stated in its own source comment, in `pyboy/core/mb.py`'s `tick()`:

> *"Fast-forward to next interrupt: as we are halted, we are guaranteed that our state cannot be
> altered by other factors than time."*

It takes `min(timer._cycles_to_interrupt, lcd._cycles_to_interrupt, lcd._cycles_to_frame,
sound._cycles_to_interrupt, serial._cycles_to_interrupt)`, runs the CPU that far in one go, and
then catches every peripheral up in a single step. **Each peripheral publishes a horizon and is
only advanced when one is reached.**

That is precisely the shape of our own idle batch — the difference is that PyBoy applies it
**always**, and we apply it only while the CPU is halted, because we tick per bus access so
that a read or a write lands on the right sub-instruction M-cycle. That is what Blargg's
`mem_timing`/`mem_timing-2` measure and what fourteen Mooneye instruction-timing tests measure,
and it is the deliberate trade this project makes.

**PyBoy is read for technique and cited. It is LGPL-3.0-only and none of its code is here.**

### What we took from it

The transferable half is that **a peripheral horizon is not only for a halted CPU.** Our PPU is
stepped once per instruction and walks its mode machine every time (9.1 % of a headless
profile's leaf samples, plus about 3 % for `refresh_stat_line`), when what a CPU can observe of
it changes only at a handful of dots a line. That is the same trick applied where we do not
need sub-instruction resolution, and it is lossless.

### And then what we did take, as a flag

`docs/throughput.md` §18. The whole per-instruction pipeline is batched to a
horizon under [`SpeedFlags::horizon_batch`](../modes.md), which is in the
`speed` preset and off by default — worth **+13 %** on a cold boot and **+21 %**
on an overworld state on top of the preset it joins, and **+27 % / +39 % over
the accurate machine**.

It is a flag and not a default because, unlike the PPU half above, it is **not
lossless**. What keeps it usable is the flush list rather than the horizon:
every `$FE00-$FF7F` access, every interrupt dispatch, every halt, every save
state and every frame boundary empties the batch, so a program's own reads are
exact. Measured, it changes **nothing that `instruction-bus` had not already
changed** — byte-identical over 92 Pokémon save states, identical run-for-run
over the 26-link Pewter chain, and the same rows lost on GBSelfTest and on the
conformance action.

Measured on a quiet box (load 1.5), the same 7,168 frames from reset, the same
core, interleaved in one session:

| | ms/frame | × real time | vs PyBoy |
|---|---:|---:|---:|
| PyBoy 2.7.0 | 0.0189 | 886× | — |
| **TerminalGB**, pixels off | 0.0413 | 405× | 2.19× behind |
| **TerminalGB**, `GB_SPEED=speed` | **0.0317** | **528×** | **1.68× behind** |

**It narrows the gap and does not close it, and it should not be read as
closing it**: PyBoy gives up cycle accuracy *always* and scores 0 on Mealybug's
24 published rows, where this emulator scores 26 in the mode this flag cannot
touch. What is left of the 1.68× is the per-access tick the flag *keeps*
wherever a program can observe it, and the interpreter itself.

### And what we did not

Dropping the per-access tick entirely — the thing PyBoy does not pay — is priced by
`core_bench --breakdown` at **+13.6 %** of a collector's frame. It would take us from 414× to
about 470×, and it would cost `mem_timing`. It is available under the two-budget rule as a
*performance-mode* liberty, and it is written down here rather than taken quietly.

**It is also nowhere near the 2.3×, and that is the finding.** The rest is that we do peripheral
bookkeeping **once per instruction** — `MemoryManagementUnit::do_cycle` runs 3,698 times a frame
and advances the timer, the PPU, the APU, OAM DMA, the serial port and the cartridge clock every
time — where PyBoy does it **once per horizon**, which is typically many instructions.

**The PPU half of that was taken and is worth +17.5 %** ([`docs/throughput.md`](../throughput.md)
§16): the batched renderer is advanced lazily now, bounded by a horizon, with every gate green
and 5,320 Gambatte rows unmoved.

**The timer half was sized and refused.** Deferring it measures **8.5 % slower**, because
`Timer::do_mcycle` is a handful of field operations while the closed-form batch carries three
guards — the batch wins over a *loop* of 38 M-cycles and loses to a direct step over eight. A
peripheral that genuinely has to count is not overhead.

So what is left of PyBoy's lead is the per-access tick itself, and the interpreter. That is a
different execution model, not another pass.

## 6. 2026-09-08 — the harness was flattering us, and the idle horizon was too narrow

Two things moved on the same day and they go in opposite directions. Read them together or
neither is true.

### 6a. The PyBoy figure above is superseded, and the correction is against us

Every PyBoy row before this section was produced by `tools/perf/pyboy-bench.py` timing
`tick(1, False)` in a **Python `for` loop** — 7,168 Python-level calls that our side never
pays, because `core_bench` runs `Gameboy::frame()` in a Rust loop with no binding in the way.
The identical work in a single `tick(7168, False)`:

| PyBoy 2.7.0, rendering and sound off | ms/frame | × real time |
|---|---:|---:|
| `loop` — `tick(1)` ×7168, **what §2 and §4 quoted** | 0.0189–0.0192 | 869–886× |
| **`batch` — `tick(7168)`, the emulator core** | **0.0157–0.0159** | **1053–1064×** |

**17 % of the published PyBoy figure was our harness's Python overhead**, so every "× behind"
in §2 and §4 understates the gap by that much. The bench prints both now and says which to
quote; `loop` is what a Python consumer stepping frame by frame experiences, `batch` is the
core, and the core is what this page compares. **The target is 0.0158, not 0.0190.**

### 6b. The idle batch was stopping at edges no interrupt could be taken at

`Gpu::idle_dots` returned the distance to the next PPU **mode boundary**. A halted CPU cannot
observe one: all it observes is `IE & IF`, and the PPU writes exactly two of those bits —
VBlank, and STAT. So a mode edge is only worth stopping on when a STAT interrupt could
actually be *taken* there: a source selected in `STAT` **and** bit 1 of `IE` set.

Measured over 7,168 frames of Pokémon Blue: **no STAT source is selected in a single frame,
and `IE` masks STAT off in 7,150 of them.** Every one of those stops — three or four a
scanline — was for an edge that could raise nothing, and it was the binding constraint:
`--breakdown` reported the PPU bounding the batch 1,281,189 times and the timer bounding it
none. The batch went **38.0 → 505.5 M-cycles** (320 batches a frame → 24) and the halt path
from about 54 % of a frame to about 0.5 %.

With it: `IDLE_BATCH_MAX` 64 → 512 (swept, flat past 512), the APU made a horizon in
`idle_mcycles` so that raising the cap is safe rather than lucky, and `bus_tick`'s condition
folded out of the `Gpu` struct into one flag beside the live enable.

### 6c. Where that leaves the matrix

**Both axes, because a figure without both is meaningless** — the *picture engine* is what a
frame looks like, the *timing core* is what the CPU's neighbours may skip
([`docs/picture-modes.md`](../picture-modes.md), [`docs/modes.md`](../modes.md)). Interleaved
on one P-core in one session with `tools/perf/field.sh`, load 2.6, frame and WRAM hashes
pinned:

| picture | timing | before | after | × real time | vs PyBoy 0.0158 |
|---|---|---:|---:|---:|---:|
| PyBoy 2.7.0 (not cycle-accurate) | — | — | 0.0158 | 1060× | — |
| **standard** | **accurate (default)** | 0.0430 | **0.0382** | 438× | 2.42× behind |
| **standard** | **fast** (`--speed-mode`) | 0.0331 | **0.0263** | 573× | 1.65× behind |
| **identical** | **accurate (default)** | 0.3489 | **0.3375** | 50× | 21.4× behind |
| identical | fast | — | **refused by design** | — | — |

The accurate default is **11 % faster** and the fast mode **20 %**; `identical` gains 3 %.
Paired interleaved A/B of the bus-hook change alone, which is the trustworthy form: **+12.0 %
accurate, +3.5 % fast**.

**Nothing the cartridge can see moved.** Frame and WRAM hashes are identical on both picture
engines, and `make test-external` matches **all 6,315 rows of all 18 suites** — Mealybug
79/79, Gambatte 5,320/5,320, Mooneye 103/103 + 8/8, SameSuite 76/76.

### 6d. Mealybug belongs to a cell of that table, not to the emulator

We score <!--score:mealybug_accurate-->29 / 79<!--/score--> pixel-exact on the per-dot engine
where PyBoy scores 0 on the shootout's 24 published rows, and **that claim belongs to
`identical` + `accurate` and to nothing else**. A fast-mode throughput figure quoted beside it
would be a sentence about two different machines.

That is now structural rather than a convention:

* `identical` **refuses every speed flag** (`Gameboy::set_speed_flags`), so the fast cell of
  the per-dot row does not exist to be measured.
* `examples/support/suite_baseline.rs::gate` **exits 2 rather than score any suite with
  `$GB_SPEED` set.** That variable is inherited by every process started from a shell that
  exported it. The picture axis is protected by a header a reader can check; this axis has no
  second baseline a row could legitimately belong to, so it is a refusal and not a warning a
  CI log would bury.
* `core_bench` prints the picture engine and the timing core it **actually ran as** — not what
  was asked for, since `identical` refuses flags — in the human output and in the JSON
  (`picture`, `timing`, `cycle_accurate`), and shouts `*** FAST — NOT CYCLE-ACCURATE ***`.

### 6e. What is left, honestly

**The remaining accurate-mode gap is the per-access tick, and the profile now says so with a
number rather than an argument.** `bus_tick` is 28.7 % of a headless accurate frame and
`MemoryManagementUnit::do_cycle` 16.1 % — **44.8 % together, and that is precisely the
per-access peripheral work PyBoy does not do.** 0.0382 × (1 − 0.448) = 0.0211, which is the
same neighbourhood as PyBoy's 0.0158 once its own horizon bookkeeping is allowed for. So
closing it in the *default* mode means giving up what `mem_timing` and fourteen Mooneye
instruction-timing tests measure.

In **fast** mode the profile is a different shape: `Cpu::operation` 44.9 %, `fetch`/`get_byte`
17.6 %, `bus_tick` 11 %. What is left there is **the interpreter**, at roughly 66 host cycles
an emulated instruction against PyBoy's ~39, and §17 of [`docs/throughput.md`](../throughput.md)
has already measured the standard remedies — flag packing, page tables, devirtualising the
mapper, `target-cpu=native`, PGO — as neutral or negative. **Beating 0.0158 from there is a
different execution model, not another pass.**

Re-verified rather than quoted, because the question was asked directly: `o3-fat` (the
shipping default) is **+5.1 %** over thin LTO; `-Ctarget-cpu=native` is **1 % slower** than
it; PGO is **+0.4 %** against its fair comparator and **5.5 % worse** than the fat LTO it
cannot combine with on rustc 1.83. None of the three is a lever nobody had pulled.

## 5. What this changes about the goal

"As fast as possible, lossless in ways that matter" is the right framing, and it is better
served by a real ceiling than by a borrowed number:

* Against the accuracy reference we are **9.5× ahead**, and that lead is the design.
* Against the fastest thing measured here we are **2.1× behind**, and 13.6 points of that is
  accuracy we are choosing to keep.
* **Aggregate throughput** — which is what a certification run actually experiences — is
  already **1242× real time** on a contended box (see
  [headless throughput](throughput-baseline.md)).

## See also

* [headless throughput](throughput-baseline.md) — our own number, the method, and which 1000×
* [`docs/throughput.md`](../throughput.md) — the full engineering record and every negative result
* [the scoreboard](scoreboard.md) — what the accuracy costs buy

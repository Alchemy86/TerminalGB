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
`core_bench --rom blue.gb --frames 7168 --no-pixels`.

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
scores <!--score:mealybug_accurate-->26 / 79<!--/score-->.

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

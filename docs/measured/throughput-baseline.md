# How fast this emulator runs, headless — the baseline, and which 1000× is which

<!-- codemap:asof -->

> Nobody had ever written the headless throughput down, which is why the captain had to ask for
> it. This page is the answer, the method, and — because the two are very different pieces of
> engineering — **which of the two possible 1000× targets each number belongs to.**

[`docs/throughput.md`](../throughput.md) is the *engineering* record: what was tried, what it
bought, and the negative results. This page is the shorter thing a reader actually wants: **the
number, how to reproduce it, and what it does and does not mean.**

---

## 1. The headline

Pokémon Blue, a real in-game save state, one worker, **pinned to a P-core**, thread CPU clock.
The DMG's own frame is 16.7427 ms, so 1× real time is 59.7275 frames a second.

| configuration | ms/frame | × real time | load |
|---|---:|---:|---:|
| **collector** — pixels off, APU installed, samples off | **0.0454** | **369×** | 1.3 |
| the same, no APU installed at all | 0.0405 | **414×** | 1.3 |
| the same, at load 16 | 0.0772 | 217× | 16 |
| player — picture on, APU silent | 0.1582 | 106× | 16 |
| `identical` picture mode, picture on | 0.4962 | 34× | 16 |

The first and third rows are the same binary; the difference is entirely the box's other
occupant, through frequency scaling. **Report the load average with any number from this box.**

Measured against the commit this session started from, interleaved on one core in one minute on
a quiet box: **0.0592 → 0.0454 ms/frame, 283× → 369×, +30.4 %** — see
[`docs/throughput.md`](../throughput.md) §15 and §16 for what each part of that was worth, and
[against the field](against-the-field.md) for what the number means beside two other emulators
built and timed here.

**Aggregate, 20 workers, collector configuration: 223,073 emulated frames a second = 3,735×
real time** on a quiet box, and the whole curve is worth having because of where it crosses:

| workers | frames/s | × real time | per instance |
|---:|---:|---:|---:|
| 1 | 16,590 | 278× | 0.0602 ms |
| **4** | **63,687** | **1,066×** | 0.0624 ms |
| 8 | 126,633 | 2,120× | 0.0627 ms |
| 12 | 153,455 | 2,569× | 0.0679 ms |
| 16 | 197,543 | 3,307× | 0.0727 ms |
| 20 | 223,073 | 3,735× | 0.0770 ms |

**Four cores are already past 1000×.** The per-instance figure rises only 28 % from one worker
to twenty, which is what "silicon-limited, not contention-limited" looks like — see
[`docs/throughput.md`](../throughput.md) §9 for the arithmetic that ties the shape to this box's
8 P-cores and 12 E-cores. On a contended box the same command reads — both readings taken while another crew held most of the box, which is
what a fan-out figure's spread looks like when it is wall clock by definition. On a quiet box
[`docs/throughput.md`](../throughput.md) §6 measures 232,746 frames a second, which is
**3,896×**.

## 2. So: which 1000×?

They are not the same goal and they are not the same work.

* **Aggregate 1000× is already met, and met on four cores.** Twenty give 3,735×. Certification already fans out — a
  3,000-attempt link is twenty workers — so this is the number a *measurement campaign*
  actually experiences.
* **Single-core 1000× is about 3× away and is not available by shaving.** The arithmetic is worth
  stating because it saves somebody a month: `core_bench --breakdown` prices the per-access bus
  tick at **+21 %** of a collector's frame, the whole APU at **+16.7 %** and the whole timer at
  **+14 %**. Deleting *all three outright*, which would not be an emulator any more, is about
  **1.9×** — roughly **550×**. Everything else on the page is a few per cent each.

  Single-core 1000× therefore needs a different execution model, not another pass of
  micro-optimisation. **And the shape of it is now named rather than guessed**: measured beside
  PyBoy, which is 2.3× faster and not cycle-accurate, the whole gap is that peripheral
  bookkeeping happens **once per instruction** here and **once per horizon** there. Making the
  timer and the PPU lazily caught-up the way the APU already is
  ([against the field](against-the-field.md) §4) is the project, and `Sound::catch_up` is the
  proof it costs no accuracy.

**When somebody quotes a throughput figure for this emulator, they should say which.**

## 3. The method, so anyone can repeat it

```sh
cargo build --release --example core_bench
taskset -c <a P-core> ./target/release/examples/core_bench \
    --rom blue.gb --state overworld.gbss \
    --frames 20000 --warmup 3000 --repeat 3 \
    --audio --no-pixels --no-audio-out
```

Four things about that command are load-bearing, and three of them were learned by getting a
wrong answer first:

1. **Pin it.** This box is 8 P-cores at 5.3 GHz and 12 E-cores at 4.6 GHz, and a fresh process
   lands on whichever the scheduler picks — so the per-instance figure moves by 15 % or more
   between invocations of the *identical binary*, even though it is CPU time.
2. **`core_bench` reports thread CPU time**, not wall clock, and prints the wall figure beside
   it so the gap is visible. Contention still reaches it through frequency scaling: the same
   binary measured **0.0581 ms/frame at load 0.3 and 0.0909 at load 16**, a 56 % spread.
   **Report the load average with any number from this box.**
3. **Use long rounds.** 2,000-frame rounds are mostly the frequency governor ramping up after
   process start and reorder the variants between runs.
4. **A fan-out figure is wall clock by definition** and has no CPU clock to fall back on. Quote
   the load average or do not quote the number.

## 4. Reconciling `examples/headless`

`examples/headless` is the natural thing to reach for — it is the "the core stands alone" demo
— and it reads much lower, which is *correct* and worth writing down so nobody chases it:

| how it was run | 7,200 frames (120 emulated s) | × real time |
|---|---:|---:|
| pinned to a P-core | 1.33 s | **90×** |
| pinned to an E-core | 1.84 s | 65× |
| unpinned, box at load 16 | 1.49–1.87 s | 64–80× |

(That table predates §15's work by a few hours; the shape of the reconciliation is what it is
for, not the digits.)

It is not a throughput benchmark and does not claim to be. It **renders every pixel**, hashes
the whole framebuffer, round-trips a save state and runs 60 more frames to check it, and a
`time` around it also buys process start and the ROM read. The gap between 90× and 288× is
that work plus the picture; the gap between 90× and 30× is core placement on a busy box.

**If you want a throughput number, use `core_bench`. If you want to know the core still runs
with no features, use `headless`.**

## 5. What a certification run actually costs

The unit that matters to this project is not ms/frame, it is *how many measurements a day*. A
`pewter_chain corpus` link is 3,000 attempts of a few thousand frames each, fanned across the
box. At the aggregate figure above that is minutes, not hours — and
[`docs/throughput.md`](../throughput.md) §8–§10 is the record of getting it there, including
the finding that **46 % of a swarm run was once the filesystem** rather than the emulator.

## See also

* [`docs/throughput.md`](../throughput.md) — the full engineering record, every negative result
  with the command that produced it
* [the PPU](ppu.md) — the two picture engines and what the difference is worth
* [the host boundary](host.md) — defects that leave the framebuffer perfect

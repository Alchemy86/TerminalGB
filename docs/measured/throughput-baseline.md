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

| configuration | ms/frame | × real time |
|---|---:|---:|
| **collector** — pixels off, APU installed, samples off | **0.0581** | **288×** |
| player — picture on, no audio | 0.1237 | 135× |
| `identical` picture mode, picture on | 0.3316 | 51× |

**Aggregate, 20 workers, collector configuration:** **74,155 emulated frames a second = 1242×
real time** — and that was measured while *another crew held sixteen of the twenty cores*. On a
quiet box [`docs/throughput.md`](../throughput.md) §6 measures 232,746 frames a second, which
is **3,896×**.

## 2. So: which 1000×?

They are not the same goal and they are not the same work.

* **Aggregate 1000× is already met.** It was met before this page was written, on a contended
  box, and a quiet one is nearly four times past it. Certification already fans out — a
  3,000-attempt link is twenty workers — so this is the number a *measurement campaign*
  actually experiences.
* **Single-core 1000× is 3.5× away and is not available by shaving.** The arithmetic is worth
  stating because it saves somebody a month: `core_bench --breakdown` prices the per-access bus
  tick at **+21 %** of a collector's frame, the whole APU at **+16.7 %** and the whole timer at
  **+14 %**. Deleting *all three outright*, which would not be an emulator any more, is about
  **1.9×** — roughly **550×**. Everything else on the page is a few per cent each.

  Single-core 1000× therefore needs a different execution model — block compilation, or a
  threaded-code interpreter — not another pass of micro-optimisation. That is a project, and it
  should be chosen deliberately rather than arrived at.

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

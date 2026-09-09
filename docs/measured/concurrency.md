# How many Game Boys at once — the measured ceiling

**About 820 copies of Pokémon Blue, each running at full DMG speed, on a
twenty-core desktop.** On the cheaper `standard` picture engine, about 5,000.
Past that the box does not fall over and does not slow down in aggregate — it
divides the same ~50,000 emulated frames a second into thinner slices, and each
copy drops below real time in proportion.

Measured on an Intel Core Ultra 7 265 (8 P-cores at 5.3 GHz, 2 at 6.5, 10
E-cores at 4.6; 30 MiB shared L3; 31.5 GiB RAM), collector configuration
(`--audio --no-pixels --no-audio-out`), default `identical` picture engine,
cycle-accurate timing, no speed flags. **The box was not quiet:** a
`localsearch` indexer held a full core for the whole session, so roughly 5 % of
the machine was gone throughout and every figure here is that much
conservative.

---

## 1. The operating table

The number that matters is not the largest count that runs. It is the largest
count at which **a copy still keeps up with a real Game Boy**, because below
1.0x a copy is no longer playing the game in any useful sense — it is being
sampled slowly.

Best of three 60-second windows at each point, `identical` engine:

| copies | each copy | aggregate | peak RSS |
|---:|---:|---:|---:|
| 1 | 87x real time | 5,044 f/s | 4.0 MiB |
| 20 | 40x | 48,033 f/s | 12.6 MiB |
| 200 | 4.0x | 48,120 f/s | 40.9 MiB |
| 600 | 1.36x | 48,769 f/s | — |
| **~820** | **1.00x** | **~49,000 f/s** | ~136 MiB |
| 1,000 | 0.87x | 52,019 f/s | 166.8 MiB |
| 16,000 | 0.050x | 48,512 f/s | 2,521 MiB |

On `standard`, the same shape four times further out: aggregate
~300,000-313,000 f/s, 5,000 copies at 1.03x and 5,500 at 0.95x, so the
crossover is **about 5,200 copies**.

**Aggregate is flat from 20 copies to 16,000.** That is the whole scaling
story: the box delivers what its cores deliver, and asking for more copies
divides it rather than growing or shrinking it. There is no collapse to find.

### 16,000 copies is verified, and it is not an operating point

It is quoted here only because it is the largest count whose *output* was
checked, and because the claim is easy to misread. At 16,000 each copy runs
**3.0 frames/s — 0.050x real time**, which is useless for anything that needs a
copy to keep up. Aggregate is 48,512 f/s, 93 % of peak, so nothing breaks; the
copies are simply each getting 1/800th of a core. Quote 820, not 16,000.

---

## 2. The trap: a naive sweep measures the box's clock, not its scaling

**This is the finding most worth carrying, because it manufactures a scaling
collapse that is not there, and the shape of the error is convincing.**

A sweep with a fixed frame count per copy runs its small points for a fraction
of a second and its large ones for a minute. This box does not hold one clock
across that. Read directly out of `/proc/cpuinfo` during a sustained run:

| | P-core clock | ms/frame |
|---|---:|---:|
| first ~5 s of all-core load | 4,540 MHz | 0.2145 |
| every second after | **3,100 MHz** | **0.3149** |

4540/3100 = 0.683 predicts 0.2145/0.683 = **0.314** against a measured
**0.3149** — agreement to 0.3 %, so the mechanism is confirmed rather than
inferred.

The control needs no instance count at all. The identical 20-worker,
2,000-frame run reads:

- **0.2145 ms/frame** on a cold box
- **0.3003** and **0.3116** immediately after a long run
- **0.2197** again after 45 s idle

and at a fixed 20 workers, purely by running longer: 2,000 frames **0.2231**,
10,000 frames **0.2903**, 50,000 frames **0.3545**.

So the first sweep here reported aggregate falling 79,251 → 48,255 f/s between
200 and 500 copies and it was **entirely** the sustained-clock policy: the
500-copy point ran for 22 s and the 200-copy point for 6. Every point is now
warmed to steady state at its own load (`--warm-seconds`), points run back to
back with no idle, and frames per copy are scaled so every timed window is the
same wall duration.

**This corrects a published curve.** `AGENTS.md` records a sweep ending "flat
from 16 to 256, declining past 512" — that decline is this artifact. The
absolute figures there are not challenged; the *shape* is.

---

## 3. What actually breaks

**Nothing, up to 16,000 copies.** Not the scheduler, not file handles, not the
allocator, not correctness. What is left is memory, and it is linear:

| shape | MiB per copy | 16,000 copies | ready |
|---|---:|---:|---:|
| one OS thread per copy | **0.399** | 6,381 MiB | 1,394 ms |
| pool of 20 threads | **0.158** | 2,521 MiB | 500 ms |

At today's free memory (~13 GiB) and a 2 GiB floor, that is roughly **29,000
copies** thread-per-instance or **75,000** pooled. Neither was attempted:
32,000 thread-per-instance predicts 12.9 GiB against 13.9 GiB free, which is
inside the margin, so it was refused rather than run. **A ceiling not reached
because it was unsafe is still an answer.**

Thread creation is not the problem it looks like either. 1,000 copies are live
in **66 ms** and 16,000 in **1.4 s** (500 ms pooled) — and an earlier figure of
22.6 s was this instrument counting its own 300-frame warm-up as start-up time,
not the emulator.

### The pool is 2.5x cheaper, and the reason is per-thread stack

The machine itself is ~162 KiB — a 51 KiB inline struct (16 KiB VRAM, 32 KiB
WRAM), a 92 KiB framebuffer and 32 KiB of cartridge RAM. One thread per copy
costs 0.399 MiB, so **60 % of it is not the machine.** It is stack pages
touched constructing and moving a 51 KiB `Gameboy`, plus the thread's own
guard and TLS pages, and it is paid once per *thread*. Give one thread
twenty-five machines and it is paid once for twenty-five: 0.399 → 0.158
MiB/copy, measured identically at 1,000, 4,000, 8,000 and 16,000 copies.

So **run hundreds of copies through a pool of about core-count threads, not one
thread each.** `pewter_chain corpus` already has this shape.

---

## 4. The measured negatives

**Cache locality does nothing here, and that was the leading hypothesis.** With
500 machines and 20 threads held fixed and only the interleaving granularity
moving — 1 frame per machine before switching, up to 2,000, which is the
difference between maximal thrash and running each machine to completion —
ms/frame reads 0.3323, 0.3282, 0.3185, 0.3286, 0.3244, 0.3265. A 4 % spread,
which is noise.

That was worth testing because the arithmetic looked compelling: per-copy hot
set ~60 KiB against a 30 MiB L3 puts the crossover at ~500 copies, which is
exactly where the apparent collapse sat. It was a coincidence. **The chunk
experiment is the one measurement thread-per-instance cannot run**, and its
answer is that the interleaving does not matter.

---

## 5. Correctness at scale

Every count from 1 to 16,000 produced frame hash **`0xc850103d09c4eb25`**, and
at each count all copies were compared against each other and agreed. The pool
produces the same pair as thread-per-instance, including at a chunk size that
divides neither the frame count nor the machine count.

**What that is not:** no conformance suite was run at 16,000 copies.
Correctness at scale rests on hash identity against the single-instance
reference, which is a real check and a narrower one than a suite.

The ROM is real Pokémon Blue, executing its real boot code from reset — no save
state, no input. A wide fan-out, not a long one: 400 frames per copy is 6.7
seconds of Game Boy time.

---

## 6. Reproducing it

```sh
cargo build --release --example core_bench
SHAPE=pool tools/perf/fanout.sh <rom.gb> 1 20 50 100 200 500 1000
```

`fanout.sh` warms the box, runs points back to back, scales frames so every
window is the same duration, sweeps up and down and **prints the disagreement**
— if the two directions differ by more than a few per cent the box was drifting
and the run is not a measurement. That rule is `swarm.sh`'s and it is what
caught the thermal artifact.

One point by hand:

```sh
./target/release/examples/core_bench --rom <rom.gb> \
    --audio --no-pixels --no-audio-out \
    --workers 20 --instances 820 --chunk 64 --frames 3600 --warm-seconds 10
```

Two flags exist for this page and are worth knowing. `--warm-seconds` does real
work at the point's own load before the timed window, so the clock has settled;
it necessarily leaves each worker on a different frame, so the cross-worker
determinism check reports `not-checked` rather than a failure it invented.
`--chunk` holds machines, threads and total frames fixed and moves only the
interleaving, so a difference across it is locality and can be nothing else.

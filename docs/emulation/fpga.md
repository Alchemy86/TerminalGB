# Emulation vs. simulation — the FPGA question

> An emulator *re-implements* what a chip does. An FPGA *reconfigures gates* to be the
> circuit. That difference is real, it is smaller than the marketing suggests, and it is
> larger than emulator authors like to admit — and knowing exactly where the line falls is
> the clearest way to understand what "accuracy" means.
>
> [What an emulator is](README.md) · [the machine chip by chip](the-machine.md) ·
> [where exact begins](accuracy.md) · [how we know it is right](test-roms.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`72a5454`](https://github.com/Alchemy86/TerminalGB/tree/72a545438d2a2422f1b55c3452032d76259993be) · regenerate with `make docs-links`, verify with `make docs-check`.

## What an FPGA actually is

A field-programmable gate array is a chip full of unspecified logic: thousands of tiny
lookup tables, each of which can be configured to compute any function of a handful of input
bits, plus flip-flops to hold state, plus a routing fabric of switches that decides which
output feeds which input. A **bitstream** loaded at power-on sets every lookup table and
every switch.

The result is not a processor running a program that *describes* a circuit. It is a circuit.
Signals propagate through gates at the speed of the silicon; every part of the design is
computing simultaneously, because there is no shared instruction stream and nothing takes
turns.

That is the whole distinction, and it is worth being precise about the vocabulary, because
"simulation" and "emulation" get used interchangeably and here they mean different things:

|  | Software emulator | FPGA implementation |
|---|---|---|
| What exists | Data structures and functions that *reproduce observable behaviour* | Gates and flip-flops configured to *be* a logic circuit |
| Concurrency | Simulated — the author picks an order and a granularity | Actual — everything settles at once, every clock |
| Cost of finer timing | More CPU, sometimes a great deal | Usually none; it is already per-clock |
| Cost of a mistake | Recompile | Re-synthesise (minutes to hours) and re-fit |
| What it can be checked against | The same test ROMs | The same test ROMs |

Note the last row. **The verification story is identical**, and that turns out to be the most
important thing on this page.

## Why FPGA is held up as the gold standard

Three claims are usually made, and two and a half of them are true.

**True: everything happens at once, so there is no ordering to get wrong.** A software
emulator has to choose what happens first when a memory write and a timer tick fall in the
same cycle. A circuit does not choose; the write and the tick are separate wires and both are
live. Every one of the three "cracks" on the [accuracy page](accuracy.md) is an
ordering-and-granularity problem that a gate-level design does not have, because in a gate-level
design the granularity is one clock and there is nothing finer to get wrong.

**True: latency can be made genuinely low.** [MiSTer](https://mister-devel.github.io/MkDocs_MiSTer/)
— a project built on the DE10-Nano board that "aims to recreate classic computers, video game
consoles, and arcade machines using modern hardware" — and the
[Analogue Pocket](https://www.analogue.co/pocket) can drive a display straight from the core,
with no operating system, compositor or graphics-API queue in the path. A software emulator
on a desktop is at the mercy of everything between the framebuffer and the panel, and the gap
is measured in frames.

**True, but not for the reason people think: analogue output.** A real Game Boy's screen has
ghosting and its sound comes out of a real digital-to-analogue converter with real filtering.
An FPGA core is closer to being able to reproduce that *if it is driving comparable analogue
hardware* — but an FPGA driving HDMI is producing digital pixels exactly like everyone else,
and reproducing an LCD's ghosting is a shader, not a gate. This one is about the *board*, not
the *FPGA*.

**And the one that is not true: that an FPGA core is accurate by construction.** It is not,
and this is the misconception worth spending a paragraph on.

## The myth: "it's hardware, so it must be right"

Analogue's own page for the Pocket puts the claim about as bluntly as it can be put:
*"Completely engineered in two FPGAs. No emulation."* The first sentence is a fact about the
implementation. The second is a claim about the *result*, and it is the one worth examining.

Unless a core was derived from die photographs of the original chip, an FPGA implementation is
a **reverse-engineered reimplementation**, exactly like a software emulator. Its author read
Pan Docs, ran test ROMs, probed a real console with a logic analyser, and wrote a description
of what they concluded — in Verilog or VHDL instead of C or Rust. If they concluded wrong,
the gates are wrong. There is no mechanism by which configuring hardware makes a
misunderstanding correct.

What the medium changes is the **default granularity of the mistakes you can make**. In
software, "advance everything after the instruction" is the path of least resistance, and it
is a whole class of error. In a hardware description language you write per-clock behaviour
because that is what the language is for, so that class of error is largely unavailable and
your mistakes are different ones — a wrong term in a condition, a signal registered one clock
late, a state machine that misses a corner.

Which is why FPGA cores are tested with the *same* ROMs, are listed on the *same* accuracy
boards, and have *the same kind* of bug reports. If FPGA implied correctness, none of that
machinery would exist.

## Where software genuinely matches, and it is most of the surface

Here is the argument that should settle it, and it is short.

**A test ROM is a program running on the emulated processor. It can only observe what the bus
and the registers expose.** It cannot probe a wire. It cannot measure a propagation delay. It
reads a register, counts its own cycles, catches its own interrupt, and looks at a picture.

So: if a software emulator produces the right value at the right cycle for every
program-observable question, **no cartridge can tell the difference** — not because the
emulator is convincing, but because there is no experiment left for the cartridge to run. And
"program-observable" is a huge surface. Every instruction result and flag. Every timer edge.
Interrupt dispatch to the M-cycle. Which pixel a mid-line palette write affects. The exact
cycle a wave-RAM read is allowed through. All of that is digital, all of it is on the bus, and
all of it is reachable in software at whatever granularity you are willing to pay for.

The empirical version of the same point: the accuracy suites are led by **software**
emulators. SameBoy, an emulator written in C, is the reference implementation people compare
FPGA cores *against*, and it is the source of the SameSuite audio tests everybody uses. If
software were structurally handicapped on digital behaviour, that could not be true.

## Where software structurally cannot match

Three places, and the honest thing is that they are all outside the framebuffer.

**1. The analogue tail.** The DMG's screen is a reflective LCD with slow pixels; the sound
leaves through a real converter, an amplifier and a small speaker. None of that is in the
digital domain, so none of it is something a correct emulator *has*. It can be approximated —
a ghosting shader, a filter on the audio — but an approximation is what it will remain, and
this project deliberately does not attempt it (the four-bit grit of the sound chip is called
"genuine DMG character" in [`docs/audio.md`](../audio.md) and is meant to stay).

**2. Input-to-photon latency.** This is not an emulation problem at all and it is the one
players actually feel. A keypress crosses a USB stack, a kernel, an event loop, a frame of
emulation, a compositor and a display pipeline. A core driving a panel directly skips most of
that. Software can reduce this — pace correctly, avoid buffering, use a low-latency output
path — but it cannot delete the operating system, and the floor is meaningfully higher.

**3. True concurrency at sub-clock granularity.** A software emulator must serialise. It
picks an order and a step size, and *within* a step it has a state that the real machine
never occupies. Ours is now one M-cycle for the bus and one dot for the picture — fine
enough that no test ROM has been found that can see the seam — but "fine enough that nothing
has caught it" is a genuinely different claim from "there is no seam". A gate-level design
does not make that claim because it does not need to.

There is also a practical fourth: **cost scales differently.** Making a software emulator
twice as exact makes it slower, and this repository can put a number on it — the per-dot
picture pipeline costs **2.11×** the whole-line one, measured. In an FPGA the per-clock
behaviour was the starting point and finer accuracy costs logic area, not time. That is why
software emulators ship *modes* and FPGA cores generally do not.

## Which side we are on, and what our results actually say

**We are a software emulator, and not a cycle-stepped one by default.** That should be stated
plainly before any score is quoted.

The default picture mode, `standard`, draws a whole scanline at once and reads each register
once per line. The optional `identical` mode is the real per-dot pipeline. Everything else —
the processor, the bus, the timer, the sound chip, DMA — runs at **one M-cycle granularity,
per memory access**, which is finer than the instruction-granular loop most emulators start
with and coarser than a gate.

Now the scores, and read them against the three structural limits above. Every figure is
generated from the checked-in baselines and verified in CI.

| Suite | `standard` | `identical` | What it is asking |
|---|---:|---:|---|
| In-repo harness (Blargg + dmg-acid2) | <!--score:harness-->46 / 47<!--/score--> | <!--score:harness-->46 / 47<!--/score--> | digital, bus-observable, coarse |
| Mooneye GB — `acceptance` | <!--score:mooneye_acceptance-->67 / 75<!--/score--> | <!--score:mooneye_acceptance_accurate-->75 / 75<!--/score--> | digital, bus-observable, M-cycle exact |
| Mooneye GB — `emulator-only` | <!--score:mooneye_emulator_only-->28 / 28<!--/score--> | <!--score:mooneye_emulator_only_accurate-->28 / 28<!--/score--> | cartridge mappers |
| SameSuite | <!--score:samesuite-->73 / 76<!--/score--> | <!--score:samesuite-->73 / 76<!--/score--> | sound chip, sub-sample timing |
| **Mealybug Tearoom** | **<!--score:mealybug-->3 / 79<!--/score-->** | **<!--score:mealybug_accurate-->29 / 79<!--/score-->** | **the picture, mid-scanline** |

The shape of that table is the argument of this page, and it is not flattering by accident:

- On everything that is **digital, on the bus, and reachable at M-cycle resolution**, we are
  at or near the ceiling: <!--score:mooneye_emulator_only-->28 / 28<!--/score-->,
  <!--score:mooneye_cgb-->8 / 8<!--/score-->, <!--score:cpp-->4 / 4<!--/score-->,
  <!--score:harness-->46 / 47<!--/score--> with its one red row a bug that Color hardware
  genuinely does not have. There is no structural reason software cannot be exact here, and
  we mostly are.
- On the suite that asks questions **inside a scanline**, we are at
  <!--score:mealybug-->3 / 79<!--/score--> by default and
  <!--score:mealybug_accurate-->29 / 79<!--/score--> with the pipeline engaged. That is the
  closest a test ROM can get to probing a wire mid-cycle, and it is exactly where software
  emulation is most expensive and most fiddly. (It is also expensive for everyone: on the 24
  Mealybug rows the Shootout publishes, SameBoy scores 15 and bgb scores 2.)

So the honest summary is: **the gap between us and a gate-level implementation is not
uniform.** It is negligible on most of the machine, real and measurable on mid-scanline
picture timing, and largest of all in a place no test ROM can see at all — the host.

## The fourth axis nobody scores: the host

This project has an unusual claim to make here, because its main frontend renders into a
**terminal**, which is the most hostile presentation path anyone has volunteered for. That
turned out to be a feature: it made an entire class of defect impossible to ignore.

Every one of these left the emulated framebuffer byte-for-byte correct and was still a
serious bug:

- pacing frames at 16 ms rather than 16.743 ms — 4.64% fast, which plays the music sharp;
- blocking inside the audio hand-off, so the sound card became the master clock: 17.93 ms per
  frame against a 16.743 ms budget, **97.6% of it blocked rather than executing**;
- sizing the transmitted image by the window rather than by the console — 39.7 ms per frame
  at fullscreen, for a picture that is 160 × 144 whatever the window does;
- and the discovery that the terminal's own compositing cost is a function of **area, not
  bytes** — about 0.42 ms per megapixel of composited picture, while the payload on the wire
  stayed flat at 86-88 bytes a frame.

**No test ROM catches any of these and none ever can**, because the cartridge cannot see the
host. An FPGA console has this axis too — it is simply much shorter, and nobody scores it
there either.

This is the real reason the accuracy-versus-hardware question is more interesting than
"emulator bad, FPGA good": the two are competing on *different sums*. A software emulator can
match a circuit on everything a program can observe, and then lose a player's experience to a
frame timer. Full record: [measured/host.md](../measured/host.md).

## Reading further

- **[`docs/picture-modes.md`](../picture-modes.md)** — the two engines as a user-facing
  choice, with the cost stated at the moment the choice is offered.
- **[`docs/measured/ppu.md`](../measured/ppu.md)** — the per-dot pipeline in detail, including
  the Color console's PPU revisions, and why a borrowed structure that does not fit this
  codebase's shape measured *worse* than an approximation of it.
- **[`docs/mealybug.md`](../mealybug.md)** — the mid-scanline campaign, row by row, with the
  split between what is winnable now and what needs more of the rewrite.
- **[`docs/measured/host.md`](../measured/host.md)** — the host boundary: defects that leave
  the framebuffer perfect.
- **[`docs/shootout.md`](../shootout.md)** — where we stand on the public board, measured
  through their own manifest and screenshot comparison.

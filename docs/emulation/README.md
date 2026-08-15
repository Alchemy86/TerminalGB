# What an emulator actually is

> Four pages for the curious reader, written from inside one. **What the machine is** ·
> **where the imitation has to stop being convenient** · **how we know it is right** ·
> **and what a chip made of real gates does differently.**
>
> No hardware background assumed. If you finish these knowing how a computer works — not
> merely how ours is laid out — they did their job.

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`72a5454`](https://github.com/Alchemy86/TerminalGB/tree/72a545438d2a2422f1b55c3452032d76259993be) · regenerate with `make docs-links`, verify with `make docs-check`.

An emulator is usually introduced as a program that "pretends to be a Game Boy", and that is
true enough to get started with and misleading enough to be worth correcting in the first
paragraph.

Here is the more useful framing. A Game Boy is not one thing. It is a small board carrying
**several independent chips**, all clocked from the same crystal, all wired to the same
bundle of address and data lines, and all doing their own work at the same time: a processor
executing instructions, a picture processor pushing pixels at a screen, a sound chip, a
timer, a link port, a button matrix, and a cartridge with its own bank-switching logic
soldered inside it. The board's job is to let them talk. The **program** on the cartridge
never addresses any of them by name — it reads and writes numbered locations, and the wiring
decides what a given number means.

An emulator is a software re-creation of *all of that*, the wiring included. Not a
translator for the processor with some graphics bolted on: a set of objects standing in for
a set of chips, plus the switchboard that connects them, stepped forward together in a loop
that mimics one shared clock.

That crystal runs at **4.194304 MHz**. A picture is finished every **70,224** of its ticks,
which is **59.7275 frames per second** — not 60. The difference is 0.46%, which sounds
ignorable and is not: run the machine at a round 60 Hz and it produces audio 0.46% faster
than it is consumed, so a 41 ms audio cushion is exhausted in about nine seconds and stays
exhausted, and the music plays measurably sharp for as long as the game is on. Both numbers
are in the source as
[`src/gameboy.rs:59`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gameboy.rs#L59), because a project whose entire value is
accuracy cannot afford a rounded constant.

## The loop, and what it hides

Strip everything away and an emulator is this:

```
loop {
    let cycles = cpu.execute_one_instruction();   //  1 to 6 M-cycles
    devices.advance_by(cycles);                   //  timer, sound, picture, link
    if picture.finished_a_frame() { show_it(); }
}
```

In this repository that is literally [`Gameboy::frame`](../../src/gameboy.rs) calling
[`Cpu::do_cycle`](../../src/cpu/core.rs) in a loop until the picture processor raises a flag.
Running dmg-acid2, that loop executes **17,288 instructions per frame** — an average
instruction costs 4.06 of the crystal's ticks — which is about a million instructions a
second, sixty times a second. A modern processor eats that: measured on this machine, one
emulated frame costs **0.218 ms** of a 16.743 ms budget, so being a Game Boy takes about
**1.3%** of real time and the console runs some **77× faster than itself** if you let it.

It is worth separating the three rates in that paragraph, because they get conflated
constantly and the emulator that conflates them runs at the wrong speed: the **crystal**
ticks 4,194,304 times a second, the **loop above** goes round roughly a million times a
second, and a **picture** is finished 59.7275 times a second. Emulator authors reach for
"sixty frames a second" and "thousands of instructions a second" and both are wrong by
enough to hear.

The reason there is anything left to say is that the loop above is a **lie of granularity**,
and every hard problem in this repository is a consequence of it.

Inside a real console the chips do not take turns. While the processor is halfway through a
four-cycle instruction, the timer is still counting, the picture processor is still drawing
the pixel it was drawing, and the sound chip is still stepping its waveform. A program can
notice. It can start a memory write on one cycle and read a timer register on the next, and
the answer it gets depends on where inside the instruction the write landed. It can change
the background palette between two pixels of the same scanline and expect the change to
appear at that pixel and nowhere else.

Nothing in the loop above can express any of that, because it advances every device *after*
the instruction has already finished. So the honest description of what this project has
spent its time on is: **finding every place where "after the instruction" is not good enough,
and making it not do that.** That is [where "convenient" stops](accuracy.md).

## The pages

| Page | What it answers |
|---|---|
| **[The machine, chip by chip](the-machine.md)** | What each piece of a Game Boy is and what it actually does — processor, bus, picture, sound, timer, link, buttons, cartridge — each with the one thing that makes it hard and a pointer to our deep page. |
| **[Where "convenient" stops and "exact" begins](accuracy.md)** | Sub-instruction timing, the pixel FIFO, mid-scanline register writes. The difference between an emulator that runs Tetris and one that passes the test ROMs — and how much the second costs. |
| **[How we know it is right](test-roms.md)** | The test ROMs. dmg-acid2 draws a face, and every PPU mistake deforms it in its own way — with our own measurements of exactly how. Then Blargg, Mooneye and Mealybug, and why a picture is stronger evidence than a number. |
| **[Emulation vs. simulation — the FPGA question](fpga.md)** | An emulator re-implements behaviour; an FPGA reconfigures gates to *be* the circuit. Why FPGA is held up as the gold standard, where software genuinely matches it, where it structurally cannot, and which side of that line we are on. |

## Where this sits among the other docs

This section is the **conceptual** layer. It deliberately owns no numbers of its own and
duplicates no reference. Three neighbours carry the detail, and every page here hands off to
them rather than restating:

- **[`docs/components/`](../components/README.md)** — *how our implementation works.* One
  deep-dive page per chip: hardware background, `path:line` references into the source,
  honest approximations, and expected behaviours phrased as future tests.
- **[`docs/measured/`](../measured/README.md)** — *what we measured, and how we know.* The
  calibrated constants with the sweeps behind them, every suite scored with a named reason
  for each red row, and a symptom → cause index.
- **[`docs/codemap.md`](../codemap.md)** — *where things are in the tree.* A frame through
  the system, a memory access from processor to device, and where the frontends attach.
- **[`docs/portability.md`](../portability.md)** — the point of the opening paragraph made
  concrete. Because the machine is re-created in software rather than in silicon, it is not
  tied to any one processor: the core has been *run*, frame-identical, on 64-bit and 32-bit
  ARM and on 32-bit x86, and it plays Pokémon Blue on a PlayStation Portable at about 42 fps.
  That page is careful to say which rows were run and which are only a compile.

And two more, one level out: **[`docs/testing.md`](../testing.md)** is the mechanics of the
yardstick — which suites exist and how each publishes a verdict — and
**[`docs/conformance-action.md`](../conformance-action.md)** is the same machinery turned
outward, so it can score somebody *else's* emulator.

## Two rules these pages are written under

Both are inherited from [`docs/measured/`](../measured/README.md), and both are the reason a
figure on this page can be trusted.

**Every number here is either measured or generated.** Suite scores are wrapped in a marker
that `make status` rewrites from the checked-in baselines and `make docs-check` verifies in
CI, so a page cannot quietly disagree with the tests. The pixel counts in the
[acid2 gallery](test-roms.md) were produced by deliberately breaking this emulator one
behaviour at a time and rendering the result — they are not estimates and not quoted from
anyone.

**Hardware claims cite a source, and the source is linked rather than copied.**
[Pan Docs](https://gbdev.io/pandocs/) is the reference (it is CC0 — public domain — so the
reason we do not reproduce it is editorial: a second copy of a maintained reference goes
stale). Where another emulator answered a question, it is named by repository, file and
symbol so a reader can check the claim themselves.

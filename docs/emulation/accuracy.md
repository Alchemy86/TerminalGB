# Where "convenient" stops and "exact" begins

> The naive emulator loop is a lie of granularity, and every hard problem in this repository
> is a consequence of it. Three cracks in that loop, what widening each one costs, and why
> the difference matters even though Tetris never notices.
>
> [What an emulator is](README.md) · [the machine chip by chip](the-machine.md) ·
> [how we know it is right](test-roms.md) · [the FPGA question](fpga.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`72a5454`](https://github.com/Alchemy86/TerminalGB/tree/72a545438d2a2422f1b55c3452032d76259993be) · regenerate with `make docs-links`, verify with `make docs-check`.

## The loop, restated as a claim

```
let cycles = cpu.execute_one_instruction();
devices.advance_by(cycles);
```

Read as a statement about hardware, that says: **nothing else in the machine moves while an
instruction is executing, and then everything moves at once.** Both halves are false.

It is false in a way that mostly does not matter. A commercial game reads the buttons once a
frame, waits for the vertical blank, writes some tiles, and never asks a question whose
answer depends on where inside an instruction anything happened. That is why an emulator can
be written in a weekend and play Tetris convincingly — and why "it runs games" is a much
weaker claim than it sounds.

The gap between that and hardware is the whole subject of this project. It has three parts,
in increasing order of how much they cost to close.

---

## The first crack: a memory access is a moment in time

Take a four-M-cycle instruction that writes a byte. In hardware, the write happens on one
specific M-cycle of the four — and the timer, the sound chip and the DMA engine have all
advanced by that point. Advance the devices *after* the instruction and the write lands
against a machine state that never existed.

A program can measure this directly, and Blargg's `mem_timing` does: it configures the timer
to increment often, then reads it at carefully chosen offsets and reports which M-cycle each
access appeared to land on.

**The fix is to move the tick inside the access.** Every `rb` and `wb` the CPU performs now
advances the rest of the machine by one M-cycle *first*, and the instruction's remaining
internal cycles are applied at the end
([`MemoryManagementUnit::bus_tick`](../../src/mmu/mod.rs), gated by a flag the CPU sets around
each instruction so nothing else pays for it):

```rust
fn bus_tick(&mut self) {
    if self.bus_tick_enabled {
        self.timer.do_cycle(4);
        self.intf |= self.timer.interrupt;
        …
        self.serial_shift();                 // the link clock is tapped off the same counter
        if let Some(ref mut sound) = self.sound { sound.do_cycle(4); }
        self.oam_dma_tick();                 // one byte per M-cycle, 160 of them
        …
    }
}
```

The comments in that function are worth reading in full, because they are a list of the
things that turned out to need it: the timer (Blargg `mem_timing` and `mem_timing-2`), the
serial shift clock (an `SC` write landing on an edge M-cycle must not eat that edge), the
sound chip (Blargg's `dmg_sound` subtests 9, 10 and 12 measure a wave-RAM access window two
ticks wide), and OAM DMA.

**What it cost.** The repository's own measurements, both on pure-CPU headless benchmarks:
stepping the timer per access is about **15%** (still ~60× real time), and the wider
core-timing work it made possible — the circuit-accurate timer, five-M-cycle interrupt
dispatch and timed OAM DMA — about **6%** more (0.303 → 0.321 ms/frame), verified transparent
to real games by running 2,400 frames of Pokémon Blue before and after and comparing the
framebuffers byte for byte. What it bought was `mem_timing`, `mem_timing-2`, and — because
fourteen Mooneye instruction-timing tests time an access by DMAing a known byte into object
memory and pointing the stack pointer at it — a whole dimension of CPU accuracy that had
been unmeasurable while DMA was instantaneous.

That last point generalises, and it is the most useful thing on this page:

> **An unmodelled peripheral can hide accuracy in a completely different component.** Fourteen
> tests of the *processor* could not pass, however correct the processor was, because the
> *DMA engine* finished too quickly for them to use as a stopwatch.

Full record: [measured/timing.md](../measured/timing.md).

---

## The second crack: the picture is drawn one pixel at a time

Ask an emulator author what the picture processor does and the natural answer is "it draws a
scanline". Ask the hardware and the answer is "it shifts out one pixel per tick of the
crystal for as long as the line lasts".

Those are the same thing right up until a program changes a register in the middle of a line
— and then they are not the same thing at all, because the whole-line renderer read that
register once, at the end, and painted 160 pixels with whatever it found.

Games do this on purpose. A racing game splits the screen by changing the scroll register on
a chosen scanline. A menu changes the background palette part way down. Those are *whole
line* effects and a scanline renderer handles them fine. What it cannot handle is a change
*within* a line, which is what the hardware actually permits — and there is a whole test
suite, [Mealybug Tearoom](https://github.com/mattcurrie/mealybug-tearoom-tests), built to
photograph the results on real consoles and hand you the pictures.

**The fix is to build the real pipeline.** Two things run concurrently, both clocked by the
dot ([`src/gpu/fifo.rs`](../../src/gpu/fifo.rs)):

- a **fetcher** reading one background or window tile every 6 ticks — tile number, then
  bitplane 0, then bitplane 1, two ticks each — and pushing 8 pixels into an 8-deep queue
  whenever the queue empties;
- an **output** shifting one pixel out of that queue per tick and painting it, mixing in
  whatever the object queue holds at the same position.

The fetcher makes 8 pixels every 6 ticks and the output consumes 8 every 8, so the fetcher
spends 2 ticks per tile idle — and that idle is precisely where an object fetch slips in,
which is why the per-object cost is variable rather than a constant.

Two consequences fall out of this structure rather than being special-cased, and that is the
sign it is the right structure:

- Mode 3's minimum is **172** ticks rather than 160, because at the head of the line the
  fetcher runs twice for the first tile and the first result is discarded — 12 wasted ticks.
- Mode 3 is longer when the horizontal scroll is not a multiple of 8, because `SCX & 7`
  pixels are shifted out of the front of the queue and thrown away, still costing a tick
  each.

And the register discipline changes. **A colour index goes into the queue, never a shade:**
the palette is applied when the pixel is shifted *out*. So a palette write part way along a
line moves a boundary part way along that line, exactly as the reference photographs show.
The tile-map select bit is read at the tile-number fetch, `LCDC.4` at each bitplane fetch,
and the vertical scroll at all three — which is why a Mealybug ROM that changes the tile-map
select mid-fetch produces a tile *mixed from two maps*, and why an emulator that reads its
registers once per line cannot produce that picture by any amount of tuning.

**What it cost.** Measured on this machine over 400 frames of dmg-acid2: **0.218 ms/frame
standard, 0.461 ms/frame identical — a ratio of 2.11×**, and that constant is not a
hand-written note. `RenderMode::IDENTICAL_COST` in
[`src/gpu/mod.rs:58`](https://github.com/Alchemy86/TerminalGB/blob/main/src/gpu/mod.rs#L58) is checked against a live measurement by
`testharness/ppuengines.rs` on every release test run, and the build fails if it has drifted.
A performance figure shown to users has to be a measurement, because this repository has
already been bitten once by a number that outlived its fix.

Full record: [measured/ppu.md](../measured/ppu.md) · [Mealybug, measured](../mealybug.md) ·
[picture modes](../picture-modes.md).

---

## The third crack: *when* a write becomes visible

The first two cracks are about ordering at M-cycle resolution. There is a finer one.

Our per-access tick advances the PPU a whole M-cycle *before* the access. But a write to a
PPU register does not take effect at the end of its M-cycle — it commits **partway through
it**, and how far through depends on which register and which console. So the correct
behaviour is to advance the PPU by only part of the M-cycle, let the write land, then
advance it by the remainder (`MemoryManagementUnit::ppu_write_commit` and `gpu_defer` in
[`src/mmu/mod.rs`](../../src/mmu/mod.rs)).

That model was read out of [docboy](https://github.com/Docheinstein/docboy) (MIT) and
reimplemented here in this codebase's idiom, with every arm citing the Mealybug row that
measures it — including the Game Boy Color's DMG-compatibility one-dot palette glitch.

The finest-grained thing on the machine is finer still. On CPU CGB D and later the picture
processor **latches** the sum of the vertical scroll and the current line at the tile-number
fetch and reuses it for both bitplanes; on CPU CGB C and earlier it does not, so the two
bitplanes can come from different rows and mix. That is not two emulator settings — it is two
*machines*, and Mealybug ships separate hardware photographs for each, which is the suite
telling you they are different consoles. We model it as
[`mode::PpuRevision`](../../src/mode.rs), defaulting to CGB-D.

---

## So what does exactness actually buy?

Here is the honest answer, and it is not "better compatibility".

**On the picture, nothing a player will see in a commercial game.** `standard` is the default
and the whole in-repo test suite scores identically under both engines, row for row. Games
were written against hardware, and hardware's mid-line behaviour is mostly a thing games
avoid rather than exploit.

**On the tests, everything.** Here is the same emulator scored both ways — every figure below
is generated from the checked-in baselines by `make status` and verified in CI, so this table
cannot drift away from the tests:

| Suite | `standard` | `identical` | What it measures |
|---|---:|---:|---|
| In-repo harness (Blargg + dmg-acid2) | <!--score:harness-->46 / 47<!--/score--> | <!--score:harness-->46 / 47<!--/score--> | instructions, memory and interrupt timing, sound, a reference picture |
| Mooneye GB — `acceptance` | <!--score:mooneye_acceptance-->67 / 75<!--/score--> | <!--score:mooneye_acceptance_accurate-->75 / 75<!--/score--> | core timing, interrupt dispatch, boot state |
| Mooneye GB — `emulator-only` | <!--score:mooneye_emulator_only-->28 / 28<!--/score--> | <!--score:mooneye_emulator_only_accurate-->28 / 28<!--/score--> | cartridge mappers |
| Mooneye GB — `misc` (Color) | <!--score:mooneye_cgb-->8 / 8<!--/score--> | <!--score:mooneye_cgb-->8 / 8<!--/score--> | Color-specific behaviour |
| SameSuite | <!--score:samesuite-->73 / 76<!--/score--> | <!--score:samesuite-->73 / 76<!--/score--> | sound hardware, DMA, Super Game Boy |
| CasualPokePlayer | <!--score:cpp-->4 / 4<!--/score--> | <!--score:cpp-->4 / 4<!--/score--> | cartridge clock, Super Game Boy |
| **Mealybug Tearoom** | **<!--score:mealybug-->3 / 79<!--/score-->** | **<!--score:mealybug_accurate-->17 / 79<!--/score-->** | **mid-scanline picture timing** |

Read the last row twice. The suite built specifically to ask questions *inside* a scanline is
the one where the two engines differ by a factor of five — and it is also the suite where we
are furthest from perfect, which is not an accident either. It is the hardest thing on the
machine to imitate in software, and the [next page but one](fpga.md) is about why.

Two cautions about that number, both learned by getting it wrong:

- **A Mealybug row is a pixel *rank* distance, not a percentage.** Ranks are computed per
  frame, so one extra distinct shade re-ranks every pixel and reports ~23,040 out of 23,040.
  Four wrong pixels have scored 22,070. Never convert it to a percentage.
- **Say which mode.** A score without one is ambiguous, and comparing a `standard` figure to
  somebody else's cycle-stepped figure is not a comparison.

**And on games, occasionally, something real.** Two examples worth keeping, because they
answer "does any of this matter?" concretely:

- The status-line interrupt is **one signal**, not four independent triggers: the enabled
  sources are ORed and the interrupt fires only on the 0 → 1 transition of that OR. An
  emulator that fires per source runs a raster split driven off two of them at **twice the
  correct rate** — a visible defect in a real game, from a fact that sounds like pedantry.
- Modelling the timer as the circuit rather than as a specification is what makes a `DIV`
  write able to *advance* the timer. Games rely on it.

---

## The part that is not accuracy at all

One more crack, and it is the one this project has been bitten by hardest — not because the
emulation was wrong, but because the emulation was perfect and the *player* still had a bad
time.

An emulator does not only have to be a correct Game Boy. It has to be a correct Game Boy **at
the right speed, presented to a human**. Every one of the following left the framebuffer
byte-for-byte correct and was still a serious defect:

- Pacing a frame at 16 ms instead of 16.743 ms — 4.64% fast, which overflows the audio queue
  and plays the music sharp.
- Blocking inside the audio hand-off so the sound card became the master clock: measured
  17.93 ms per frame against a 16.743 ms budget, of which **97.6% was blocked rather than
  executing**. It was misdiagnosed as the sound chip four separate times.
- Sizing the transmitted terminal image by the window instead of by the console: 39.69 ms per
  frame to encode at fullscreen, and 11.6 MB of escape sequence on the wire, for a picture that
  is 160 x 144 whatever the window does.
- Refusing to start at all in a terminal that did not answer one optional capability query.

**No test ROM catches any of these, and none ever can** — the cartridge cannot see the host.
That gap is why this project emits machine-readable telemetry beside the picture, and it is
the argument in [`docs/conformance.md`](../conformance.md) for the telemetry stream being the
reference adapter of the conformance tool. Full record:
[measured/host.md](../measured/host.md).

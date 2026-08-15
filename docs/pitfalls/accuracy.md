# Accuracy-work pitfalls

← [Pitfalls index](README.md)

Traps specific to making an emulator *exact*: contested constants, correlated failures, and
tests that confidently blame the wrong component. The hardware facts behind these live in
[the measured Game Boy](../measured/README.md); this page is about the reasoning that goes
wrong on the way to them.

---

## One tunable, two suites, and no value that satisfies both

**Symptom.** A single constant has to be at two different values at once. With
`OAM_STROBE_LEAD = 4` this emulator scored **70 of 75** Mooneye `acceptance` rows and **17 of
79** Mealybug rows; with `OAM_STROBE_LEAD = 0` it scored **75 of 75** and **6 of 79**. The
trade was one-for-one and it went both ways, every time.

> The two numbers above are historical measurements of two *rejected* configurations and are
> deliberately not wrapped in generated markers. The shipped score today is
> <!--score:mooneye_acceptance_accurate-->75 / 75<!--/score--> and
> <!--score:mealybug_accurate-->17 / 79<!--/score--> — both at once.

**What it looks like.** It looks like a genuine hardware ambiguity, and it invites splitting
the difference: pick the value that loses fewest rows, write it down as "the contested
constant", and move on. We did exactly that, in the source and in the docs, and it survived
several rounds of work. It also invites the opposite error — deciding one suite is simply
wrong.

**Root cause.** The constant was absorbing an error that was somewhere else entirely, and the
two suites differed in **one specific machine state**.

`OAM_STROBE_LEAD` says the mode-2 OAM STAT strobe rises four dots before its scanline, i.e.
*part-way through an M-cycle* rather than on a boundary. Every other interrupt source on this
machine flips on an M-cycle boundary — VBlank, the timer, the serial port, the mode-0 and
mode-1 STAT sources — so a CPU sampling once per M-cycle sees them at the same instant
whatever it is doing. The mode-2 strobe is the exception, and **a halted CPU is one M-cycle
slower to act on it than a running one**.

And that is exactly, and only, how the two suites differ. Mooneye's five
`acceptance/ppu/intr_2_*` run `ei / halt / nop` and are woken by the strobe. Mealybug's ROMs
run a 1200-instruction `nop` slide and are interrupted out of it. Both then count fixed CPU
cycles to a PPU observation. Nothing else about them differs.

**Resolution.** Model the difference where it belongs — in how the two CPU states consume one
request, not in when the request is made. `Gpu::take_stat_irq_mid_mcycle` records that this
particular request was asserted inside an M-cycle; `Cpu::mid_mcycle_request_reaches_halt_late`
spends it on the halt path only. The constant did not move.

**Side effects and blast radius.**

* `savestate::FORMAT_VERSION` 18 → 19. The marker is live across an instruction boundary, so
  it is machine state and states written by the previous build are refused.
* Gated on the per-dot (`identical`) engine. All five `standard`-mode baselines are
  byte-identical, so the default picture mode is untouched.
* **Nothing else's interrupt timing may move, and that is checkable, not hopeful.**
  `acceptance/di_timing-GS` and `halt_ime1_timing2-GS` measure *halt-dispatch minus
  run-dispatch* off the VBlank flag, which pins that difference to zero outright;
  `ppu/intr_1_2_timing-GS` wakes on the mode-1 STAT source and `ppu/hblank_ly_scx_timing-GS`
  on the mode-0 strobe, both boundary-aligned. All four were green before and after. All four
  go **red** under the naive fix (see below), which is how you know the scope is real.

**Three things it is not, each measured** — recorded so nobody repeats them, and because two
of them are what localised the answer:

* **Not a blanket extra M-cycle on the `HALT` wake.** Charging one unconditionally fixes all
  five `intr_2_*` and breaks the four rows above: 71/75. The delay belongs to the *request*.
* **Not the PPU write-commit phase.** It spans exactly one M-cycle and sits at `early: 3`, so
  it has one dot of slack, not four.
* **Not the PPU-versus-CPU phase.** Sweeping the LCD-on phase 0..7 moves Mooneye and leaves
  Mealybug untouched at every value — *because both suites anchor on the strobe and are
  therefore invariant to a global phase shift*. That invariance is what localised the error:
  it cannot be in the phase, nor in the pixel stream relative to the strobe (Mealybug measures
  exactly that and is exact), so it has to be in how two CPU states consume one request.

**How we found it.** `d48a6af`. Two references, read as documentation and reimplemented rather
than copied, describe the same mechanism from opposite sides: docboy (MIT) tabulates interrupt
latency per source, per halt state and per T-cycle of the M-cycle — its DMG STAT row is
`{1,1,1,2}` running against `{1,2,2,2}` halted; SameBoy (Expat) advances the clock around its
halt-path `IE & IF` sample and not around its run-path one. The end-to-end test
`cpu::core::tests::a_halted_cpu_meets_the_oam_strobe_one_m_cycle_later_than_a_running_one`
drives a real machine down both paths from one starting point and requires exactly four dots
between them; with the fix disabled both read 456.

> ### The generalisable rule
>
> **When one tunable makes two suites disagree, the tunable is usually absorbing a real error,
> and the two suites usually differ in one specific machine state.** Before splitting the
> difference, stop looking at the number and go and read what the ROMs were *doing* when the
> event arrived. A knob that trades tests one-for-one in both directions is not modelling
> anything; it is a symptom.

---

## Rows that move together are one bug

**Symptom.** Eleven picture-judged test rows fail. Each is a different ROM exercising a
different register.

**What it looks like.** Eleven problems. The natural response is to open the worst one and
start on it, which is eleven times the work and gives no signal until the last is done.

**Root cause.** They were failing by **a uniform four-pixel horizontal shift, in both
directions, every time** — one error, expressed eleven ways. A single mechanism upstream of
all of them (the interrupt they all synchronise on) was one M-cycle out.

**Resolution.** Correlate before you fix. Characterise every failing row's *signature* — the
offset, the affected region, the direction — and group them. Rows in one group share a cause;
a group of eleven is one investigation, not eleven.

**Side effects and blast radius.** None from the technique itself, and that is the point: it
is free, and it is the difference between "62 rows to go" and "12 rows are actually in play".
Do it before you scope the work, not after.

**How we found it.** The measurement was recorded in the conformance notes for `d48a6af`: any
change that fixed the five Mooneye rows cost those eleven Mealybug rows *a uniform four-pixel
horizontal shift*, in both directions. That symmetry is the tell — a coincidence does not
reverse cleanly.

> **A correction to an earlier draft of this page.** It claimed those eleven rows "correlate at
> 99.9 % under a `+4 px` offset". No such correlation was ever computed here. The measured
> statement is the one above: a uniform four-pixel shift, observed in both directions. The
> weaker, true claim is the one to repeat.

See [techniques.md](techniques.md#characterise-failures-before-you-fix-them) for the tooling
form of this, and [not-our-bug.md](not-our-bug.md#most-of-a-suites-remaining-rows-can-be-unwinnable)
for what the same characterisation says about which rows are winnable at all.

---

## A test that synchronises through one subsystem to measure another names the wrong one

**Symptom.** A test that measures component A fails, and the failure text points at A.

**What it looks like.** A bug in A. It is usually not.

**Root cause.** A test ROM has to *get* to the thing it is measuring, and the route often runs
through another subsystem. When the route is broken the measurement fails, and the failure is
labelled with the destination, not the road.

Two of ours, both expensive:

* **Blargg `interrupt_time`** was written off as *"the CGB double-speed switch does not take
  effect"*. The switch worked. The ROM synchronises through the APU — it spins on `NR52` —
  and the APU was clocked at CPU speed, so at double speed it ran twice as fast and the sync
  missed. Fixing the APU's base clock turned the row green.
* **Fourteen Mooneye instruction-timing tests** named the CPU. They measure *which M-cycle an
  access lands on* by DMAing a known byte into OAM and pointing `SP` at it. With an
  instantaneous OAM DMA none of them can pass however correct the CPU is.

**Resolution.** Read what the ROM does before it takes its measurement, not only what it
claims to measure. Both cases are obvious from ten lines of the test's own source.

**Side effects and blast radius.** Fixing the *road* moves everything else that travels it.
The APU base-clock fix moved audio output for every double-speed cartridge (and, because the
audio queue paced the emulator, moved the frame rate: Blargg `cpu_instrs` measured 33.7 fps /
48 % frames late before, 59.70 / 0 % after). Timing OAM DMA moved fourteen rows at once. Both
are the good kind of blast radius; expect to re-bless baselines in the same commit.

**How we found it.** Both are in [`docs/conformance-notes.md`](../conformance-notes.md);
the summary form is in [symptoms.md](../measured/symptoms.md).

---

## A defect masked by the very thing that consumes it

**Symptom.** Nothing. No test fails, no verdict is wrong, and the emulator is nevertheless
raising **two** interrupts per scanline where hardware raises one.

**What it looks like.** Correct behaviour, for as long as anybody only reads verdicts.

**Root cause.** The mode-2 STAT strobe dropped at each line boundary and rose again one dot
later. The second edge lands *inside* the first edge's interrupt dispatch, which clears the
flag at M4 — after it. So the pair collapses into a single delivered interrupt and every
program that services the interrupt at once sees exactly the right thing.

Line 0 was the exception, because it had nothing to collapse into, and that is where the whole
error surfaced — four pixels, on one line, on every ROM that synchronises off the mode-2
source.

**Resolution.** Derive the strobe from `LY` and the dot counter rather than from `mode` (which
lags `LY` by one dot). And, for the class: **count edges, not flags.**
`gpu::tests::a_visible_line_strobes_the_oam_source_exactly_once` is the test shape — it counts
transitions over a scanline instead of asking whether an interrupt eventually arrived.

**Side effects and blast radius.** Accurate-engine only; the default renderer changes `mode`
at instruction granularity, so a one-dot signal there is mixing resolutions. Mealybug
6 → 12 rows and Mooneye `acceptance` +1 when it landed; the `standard` baselines did not move.

**How we found it.** `54a0729` and the entry before it in
[`docs/conformance-notes.md`](../conformance-notes.md). Full write-up:
[`docs/measured/ppu.md`](../measured/ppu.md).

> **The general form.** A defect whose only consumer also *erases* it produces no wrong
> verdict anywhere. Verdict-based testing is structurally blind to it. If a signal has an
> edge, assert on the edge count.

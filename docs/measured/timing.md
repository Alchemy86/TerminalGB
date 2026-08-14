# Timing — the timer, interrupt dispatch, and OAM DMA

← [The measured Game Boy](README.md)

Three peripherals, measured against [Mooneye GB](https://github.com/Gekkio/mooneye-test-suite),
whose ROMs were verified by their author on real DMG, MGB and CGB units. Together they took
Mooneye `acceptance` from **13/66 to 52/66** in one pass, and every `timer/` and `oam_dma*`
row is green today.

Hardware reference, not restated here:
[Timer and Divider Registers](https://gbdev.io/pandocs/Timer_and_Divider_Registers.html) ·
[Timer Obscure Behaviour](https://gbdev.io/pandocs/Timer_Obscure_Behaviour.html) ·
[Interrupts](https://gbdev.io/pandocs/Interrupts.html) ·
[OAM DMA Transfer](https://gbdev.io/pandocs/OAM_DMA_Transfer.html). Component deep-dives:
[`docs/components/timer.md`](../components/timer.md) ·
[`docs/components/cpu.md`](../components/cpu.md) ·
[`docs/components/mmu.md`](../components/mmu.md).

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`f69fff9`](https://github.com/Alchemy86/TerminalGB/tree/f69fff9889fe64e40dc194949e820c2b6ead6b52) · regenerate with `make docs-links`, verify with `make docs-check`.

## What the numbers were

Attribution, measured all three ways on the same build. The two criteria are Mooneye's own
(`LD B,B` + Fibonacci registers) and the Shootout's screenshot comparison:

| | register criterion | screenshot criterion |
|---|---:|---:|
| before either change | 13/66 | 6/66 |
| a real DMG boot mode alone | 15/66 | — |
| this timing work alone | 50/66 | 32/66 |
| **both** | **52/66** | **52/66** |

Of the 39 rows gained, **37 are the timing work and 2 are the DMG boot mode**
(`bits/unused_hwio-GS`, `boot_regs-dmgABC`); no regressions in either direction.

**The two criteria agreeing exactly is the finding.** Before a real DMG mode existed, the same
work scored 50 on registers and only 32 on screenshots — and that entire 18-row gap was the
boot-register *dump* Mooneye prints beside its verdict, not behaviour. Two verdict methods
disagreeing is a signal; do not pick the flattering one.

**Cost:** ~6 % on a pure-CPU headless benchmark (0.303 → 0.321 ms/frame, still ~52× real time,
2 % of a 16.74 ms frame). Verified transparent to real games: 2400 frames of a commercial
cartridge produce byte-identical framebuffers before and after.

---

## The timer is the circuit, not a divider and a counter

Pan Docs describes `DIV` as an 8-bit register incrementing at 16384 Hz and `TIMA` as a counter
at a rate `TAC` selects. That description is *behaviourally* correct and *structurally*
misleading, and every "obscure behaviour" on Pan Docs' second timer page falls out of the
structure for free once you build the circuit instead.

There is **one 16-bit system counter**. `DIV` is its top byte. A write to `DIV` clears all
sixteen bits — not just the visible eight. And `TIMA` counts **falling edges** of

```
div[TAP[tac & 3]]  AND  (tac & 4)
```

with `TAP = [9, 3, 5, 7]` ([`src/mmu/timer.rs:75`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/timer.rs#L75)). Every path that
can move that expression — the clock, a `DIV` write, a `TAC` write — goes through one
`set_div` ([`src/mmu/timer.rs:88`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/timer.rs#L88)), so the glitches are not
special-cased anywhere; there is nowhere for them to be special-cased.

What that single expression buys, each of which Pan Docs documents separately as a quirk:

| Behaviour | Falls out because |
|---|---|
| Writing `DIV` can increment `TIMA` | clearing the counter drops the tapped bit 1→0 — a falling edge |
| Writing `TAC` can increment `TIMA` | the multiplexer's *output* changes even though the counter did not |
| Disabling the timer can increment `TIMA` | `tac & 4` going low takes the AND output with it |
| Changing `TAC`'s rate can increment `TIMA` | a different tap may currently read 0 where the old one read 1 |

`TIMA` overflow is separately timed: it reads `$00` for four T-cycles before `TMA` lands, with
the write-cancel and write-on-reload rules. **`acceptance/timer/*` is 13/13.**

### The one genuinely subtle bit: `TAC` latches *before* the increment

This is the only part that is an artefact of *our* structure rather than the hardware's, and it
is worth stating because the obvious simplification is wrong.

Our bus applies the M-cycle counter increment *before* the access
([`MemoryManagementUnit::bus_tick`](../../src/mmu/mod.rs), `src/mmu/mod.rs:620`), so a `TAC`
write arrives one increment "late". `Timer::wb` ([`src/mmu/timer.rs:142`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/timer.rs#L142))
therefore re-runs the edge across that increment with the newly latched `TAC`, using the
previous counter value.

**Exactly one ROM in any suite detects it**, and only through one enable landing precisely on
the tap's own 1→0 boundary: `acceptance/timer/rapid_toggle`. Without the correction the test
overflows one loop iteration late and reports `BC = $FFD8` where it wants `$FFD9`. That is the
whole difference — one iteration, one register, one ROM. **Do not "simplify" it away.**

---

## Interrupt dispatch is five M-cycles, and picks its vector halfway through

Pan Docs' [Interrupts](https://gbdev.io/pandocs/Interrupts.html) gives the dispatch as 5
M-cycles. Laid out as hardware does it
([`Cpu::handleinterrupt`](../../src/cpu/core.rs), `src/cpu/core.rs:346`): two idle cycles,
`PC`-high push, `PC`-low push, vector fetch — each ticking the bus
([`src/mmu/mod.rs:724`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L724)) so the timer keeps phase *through* the dispatch.

**The vector is decided between the two pushes**, from `IE & IF` as they stand at the start of
M4. Consequences a ROM can measure:

- A high-byte push that lands on `IE` (`SP = $0000`) can **cancel** the dispatch — `PC = $0000`,
  no `IF` bit acknowledged, `IME` still cleared — or redirect it.
- The low-byte push's own write to `IE` is **too late** to change anything.

Pinned by `acceptance/intr_timing` and `acceptance/interrupts/ie_push`.

It also means a PPU interrupt can **redirect another interrupt's dispatch**, which is how a
Color console's one-M-cycle VBlank/STAT gap reconciles with hardware photographs that show no
reordering — see [ppu.md § the CGB strobes STAT one M-cycle early](ppu.md#the-cgb-strobes-stat-one-m-cycle-before-vblank).

### `EI`'s delay is a latch, not a restartable counter

`DI` is immediate and cancels a pending `EI`. `RETI` enables `IME` with no delay. And `0xFB`
seeds its two-cycle delay **only when the delay is currently zero** — otherwise a run of `EI`
instructions re-arms the counter every cycle and interrupts are *never* enabled at all.

`acceptance/ei_sequence` executes eighteen `EI`s in a row for precisely this.

---

## OAM DMA is a timed 160 M-cycle transfer

Not an instantaneous `memcpy` ([`src/mmu/oamdma.rs:94`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/oamdma.rs#L94)):

- write at M=0, transfer starts at **M=2**, one byte per M-cycle;
- while it runs, CPU reads of OAM return `$FF` and writes are **dropped**;
- `$FF46` reads back the last page written;
- a restart lets the previous transfer run through M=1.

**`OamDma::pos` is advanced at the top of `step()`, not after the copy.** That is what keeps
`active()` ([`src/mmu/oamdma.rs:80`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/oamdma.rs#L80)) true for the CPU access sharing
the M-cycle with the last byte — the single cycle `acceptance/oam_dma_timing` measures.

### The lesson worth keeping: an unmodelled peripheral hid a whole dimension of CPU accuracy

**Fourteen Mooneye *instruction-timing* tests measure which M-cycle a memory access lands on
by DMAing a known byte into OAM and pointing `SP` there.** With an instantaneous DMA, not one
of them could pass however correct the CPU was — and the failures name the CPU, not the DMA.

Two CPU corrections fell straight out of fixing the DMA, in
[`src/cpu/stack.rs`](../../src/cpu/stack.rs):

1. A 16-bit push writes the **high** byte first, at `SP-1`.
2. The *internal* M-cycles of `PUSH` / `RST` / `CALL` / `RET cc` sit **before** their memory
   accesses, not as end-of-instruction slack. `CALL` also fetches its operand before pushing
   the return address, and conditional `JP`/`JR`/`CALL` fetch their operand **even when not
   taken**.

Generalise it: *when a test synchronises through one subsystem to measure another, its failure
names the wrong one.* The same trap appears twice more in this repo — Blargg's `interrupt_time`
reported a CGB double-speed failure that was really the APU's base clock, and `interrupt_time`
"could not run headless" for months because the harness installed no APU and the ROM spins on
`NR52`.

---

## The timer runs per memory access, not per instruction

Both the timer and the APU are stepped once per CPU bus access, before the access, with the
instruction's remaining internal cycles applied at the end
([`src/mmu/mod.rs:620`](https://github.com/Alchemy86/TerminalGB/blob/main/src/mmu/mod.rs#L620)). That is what puts reads and writes on the
correct *sub-instruction* cycle.

| Result | |
|---|---|
| Blargg `mem_timing` + `mem_timing-2` | **PASS** |
| Blargg `dmg_sound` 09 / 10 / 12 (wave-RAM windows) | **PASS** — impossible at instruction granularity, since `catch_up()` could only ever sync to instruction boundaries |
| Cost | ~15 % on a pure-CPU benchmark (headless still ~60× real time); APU stepping ~25 µs/frame with audio enabled, **zero** without |

PPU rendering is untouched by this — it stays batched — so frame cost does not move.

---

## What a wrong emulator looks like

| Symptom a player or ROM sees | Cause |
|---|---|
| Games freeze or mis-time interrupts after a `HALT` with `IME=0` and an interrupt pending | the HALT bug not modelled — `HALT` re-reads the byte after it without incrementing `PC` ([Pan Docs, HALT](https://gbdev.io/pandocs/halt.html)). Blargg `halt_bug` catches it |
| A game seeds its RNG from `DIV` and gets the same value every launch | the system counter starting at zero instead of the boot ROM's leftover — see [boot.md](boot.md) |
| Music tempo drifts, or a raster split runs at double rate | a timer built as "a divider and a counter" rather than one tapped counter; or a STAT interrupt fired per source rather than on the line's 0→1 edge ([ppu.md](ppu.md)) |
| Everything an instruction-timing suite reports is wrong, and the CPU looks broken | OAM DMA is instantaneous |
| A run of `EI` instructions never enables interrupts | `EI`'s delay implemented as a restartable counter |

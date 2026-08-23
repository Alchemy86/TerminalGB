# Timer & Divider

> Source: [`src/mmu/timer.rs`](../../src/mmu/timer.rs) · Related: [MMU](mmu.md) · [CPU](cpu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/TerminalGB/tree/main); line references documented as of commit [`851c0c6`](https://github.com/Alchemy86/TerminalGB/tree/851c0c6e6b0f8f8412ffd95f11e8115d616246c3) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The timer is a small counter that ticks at a chosen rate and fires an interrupt when it
overflows. Games use it as a metronome — a steady heartbeat for music tempo, animation
pacing, or any "do this N times a second" logic — independent of how long each frame takes
to draw. Alongside it is the **divider** (`DIV`), a free-running counter that never stops
and is commonly used as a cheap source of randomness.

## 2. The real hardware

Four registers (Pan Docs — [Timer and Divider](https://gbdev.io/pandocs/Timer_and_Divider_Registers.html)):

- **`DIV` (`0xFF04`)** — increments at **16384 Hz** and can't be stopped. **Writing any
  value resets it to 0.** It is really the top 8 bits of a 16-bit counter that runs off the
  main clock.
- **`TIMA` (`0xFF05`)** — the counting timer. Increments at the rate `TAC` selects.
- **`TMA` (`0xFF06`)** — the reload value: when `TIMA` overflows past `0xFF`, it reloads
  from `TMA` (not 0) and requests the **Timer interrupt** (`IF` bit 2).
- **`TAC` (`0xFF07`)** — control: bit 2 enables `TIMA`, bits 0–1 pick the frequency:

  | `TAC` bits 1–0 | Period | Frequency (DMG) |
  |---|---|---|
  | `00` | every 256 M-cycles | 4096 Hz |
  | `01` | every 4 M-cycles | 262144 Hz |
  | `10` | every 16 M-cycles | 65536 Hz |
  | `11` | every 64 M-cycles | 16384 Hz |

The subtle part: `DIV`, `TIMA`, and `TAC` are all derived from **one internal 16-bit
counter**, and the timer increments on a **falling edge** of a selected bit of that
counter. That shared origin is the source of several famous quirks (§5).

## 3. History and versions

The timer is essentially identical across **DMG, MGB, SGB, and CGB**. The only meaningful
variation:

- **SGB** runs slightly faster, so its derived frequencies are ~0.4 % higher
  (`DIV` ≈ 16780 Hz).
- **CGB double-speed mode** doubles the internal clock, so `DIV` and `TIMA` tick twice as
  fast (`DIV` at 32768 Hz) when armed.

**This emulator** drives the timer on double-speed-aware `cputicks` from the MMU (see
[MMU](mmu.md) §4), so double-speed scaling is handled at the tick level.

## 4. How it works here

**The `Timer` struct** — [`src/mmu/timer.rs`](../../src/mmu/timer.rs): `div` (the shared
16-bit system counter — `DIV` is its top byte), `tima`, `tma`, `tac`, the two latches that
carry the overflow window (`reload_pending`, `reloading`), `prev_div` (the counter as it
stood at the start of the current M-cycle, needed for the `TAC` latch rule below) and
`interrupt`.

**The tap table.** `TIMA` counts falling edges of `div[TAP[tac & 3]] AND (tac & 4)`, where
`TAP = [9, 3, 5, 7]` — i.e. 4096 / 262144 / 65536 / 16384 Hz. Every path that can change
that expression (the clock, a `DIV` write, a `TAC` write) runs the same edge check, which
is why the DIV-write and TAC-stop glitches fall out rather than being special-cased.

**Registers** — `rb`/`wb`: `DIV` reads `(div >> 8)` and a write clears **all sixteen bits**;
`TAC` reads back `0xF8 | (tac & 7)`. `TIMA`/`TMA` writes have the reload-window rules
described in §5.

**Counting** — `do_cycle` advances in 4 T-cycle (M-cycle) steps. Each step first retires a
pending overflow reload (`TIMA = TMA`, `IF` bit 2 set — 4 T-cycles after the overflow
itself), then advances the counter and re-checks the tap edge.

**A span of M-cycles with nothing to observe it is counted rather than stepped**
(`Timer::batch_mcycles`). A halted CPU is batched up to 64 M-cycles at a time, so most of
those steps used to be a loop iteration adding four to the counter. `TIMA` counts falling
edges of a bit whose period is at least 16 T against a 4 T step, so an edge is exactly a
crossing of a multiple of that period and the number of them in a span is a difference of
two divisions; the serial shift clock is the same argument at 512 T, one M-cycle ahead.
It is a shortcut, never an approximation: a reload in flight, a `TIMA` overflow anywhere in
the span, or a counter off the 4 T grid all fall back to the loop, and
`timer::tests::the_batch_matches_the_loop_everywhere_it_is_taken` holds the two together
over 133,120 starting states. Worth about 17 % of an emulated frame —
[`docs/throughput.md`](../throughput.md) §12.2.

### Sub-instruction access timing (why `mem_timing` passes)

The *phase* at which an instruction's memory access samples the timer is correct. `MemoryManagementUnit`
advances the timer by one M-cycle at **each** CPU memory access, *before* the access is
performed (`bus_tick`), and applies the instruction's remaining internal cycles
afterward. So an instruction that reads `TIMA` reads it on its **last** M-cycle (and a
read-modify-write reads on the next-to-last and writes on the last), matching hardware.
**Blargg `mem_timing` and `mem_timing-2` PASS** on the strength of this. Only the timer is
stepped per-access; the PPU/APU remain batched (see [CPU §4](cpu.md)).

### Honest approximations

The timer is now the hardware circuit: one shared 16-bit system counter (`div`), `DIV` as
its top byte, and `TIMA` counting **falling edges** of `div[tap] AND TAC.enable`
([`src/mmu/timer.rs`](../../src/mmu/timer.rs)). The `TIMA` overflow window (4 T-cycles of
`$00` before `TMA` lands, with its write-cancel and write-on-reload rules) is modelled too.
What remains approximate:

- **The counter advances one M-cycle at a time, not one T-cycle.** Every tap is bit 3 or
  above (period ≥ 16 T), so no edge can be skipped, and the overflow window is exactly one
  M-cycle — but *sub*-M-cycle ordering between a CPU register write and the counter's own
  increment is not directly representable. One case needed an explicit rule: `TAC` is
  latched *before* the increment, whereas `bus_tick` applies the increment before the
  write, so `Timer::wb` re-runs the edge across that increment with the newly latched
  `TAC`. Mooneye `timer/rapid_toggle` is what pins that; without it the test overflows one
  loop iteration late (`BC = $FFD8` instead of `$FFD9`).
- **Double-speed mode is not special-cased.** `DIV` advances with the CPU clock either way.
  No wired test covers it.
- **The counter is seeded one M-cycle behind the documented hardware value.**
  `Timer::set_boot_counter` takes the value a CPU read observes on the machine's *first*
  M-cycle (`$ABCC` on a DMG; on a Color console `$2678` for a DMG-header cartridge and
  `$1EA0` for a genuine CGB-flagged one, which take different routes through the same boot
  ROM) and stores it four T-cycles lower,
  because `bus_tick` advances the counter before the access that observes it. That keeps
  every *read* exact, which is what Mooneye `boot_div-dmgABCmgb` and `misc/boot_div-cgbABCDE`
  measure. It does leave the timer's tap edges one M-cycle out of phase against those reads
  in absolute terms — invisible to every test in the suite, because every other timer test
  is relative to a `DIV` write, which resets both together. `Timer::observed_counter` exists
  for the one consumer that is *not* relative: the serial shift clock.

## 5. Known quirks and edge cases

- **DIV-write falling-edge increment — modelled.** Clearing `div` while the tap bit is high
  is a falling edge and increments `TIMA` (Mooneye `timer/div_write`).
- **`TAC` glitch on stop / frequency change — modelled**, including the early-latch case
  above (Mooneye `timer/rapid_toggle`).
- **`TIMA` reload delay and its write window — modelled.** `TIMA` reads `$00` for 4
  T-cycles; a `TIMA` write in the window cancels reload *and* interrupt; a `TIMA` write on
  the reload cycle loses to `TMA`; a `TMA` write on the reload cycle is picked up by it
  (Mooneye `timer/tima_reload`, `tima_write_reloading`, `tma_write_reloading`).
- **`DIV` keeps counting regardless of `TAC` enable** — correct: `TAC` only gates the tap.
- **Overflow requests the interrupt but doesn't dispatch it** — the MMU folds
  `timer.interrupt` into `IF` and clears it; the CPU dispatches (see [CPU](cpu.md)).
- **The link port's shift clock is divided from this counter too** — `serial_edges` /
  `take_serial_edges`, one falling edge of bit 8 per 512 T. It lives here rather than in
  [Serial](serial.md) because that is where the hardware takes it from, which is exactly
  what Mooneye `acceptance/serial/boot_sclk_align-dmgABCmgb` proves: a transfer's edges
  align to the counter's reset, not to the `SC` write.
- **The counter does not start at zero.** It is free-running from power-on, so the boot ROM
  hands over with it part-way through a cycle — `DIV` reads `$AB` on a DMG, and on a Color
  console `$26` for a DMG-header cartridge or `$1E` for a genuine CGB-flagged one, which
  take different routes through the same boot ROM. Games seed RNGs from it.

## 6. Expected behaviours as tests

> **Coverage today: Mooneye `acceptance/timer/*` — 13 of 13 pass** (measured with
> `cargo run --release --example mooneye_scout -- <mooneye>/acceptance/timer`, which uses
> Mooneye's own `LD B,B` + Fibonacci-register criterion). The unit tests in
> `src/mmu/timer.rs` pin each behaviour individually so a regression is named, not just a
> count.

1. **`DIV` rate.** `DIV` increments once per 256 T-cycles (16384 Hz). *(Covered:
   `div_write_clears_the_whole_system_counter`.)*
2. **`DIV` write clears the whole 16-bit counter**, not just the visible byte — so the
   timer's phase restarts with it. *(Covered: same test, and Mooneye `timer/tim00`.)*
3. **`TIMA` rate follows `TAC`.** *(Covered: Mooneye `timer/tim00`, `tim01`, `tim10`,
   `tim11`.)*
4. **Overflow reloads from `TMA` and sets `IF` bit 2 — four T-cycles later.** *(Covered:
   `tima_reads_zero_for_four_cycles_before_tma_is_loaded`, Mooneye `timer/tima_reload`.)*
5. **`TAC` disable freezes `TIMA` but not `DIV`, and does not reset the phase.**
   *(Covered: `stopping_the_timer_with_the_tap_bit_high_increments_tima`.)*
6. **`TAC` read-back** ORs the unused bits (`| 0xF8`). *(Covered:
   `tac_reads_back_with_unused_bits_high`.)*
7. **DIV-reset falling-edge increment.** *(Covered:
   `a_div_write_with_the_tap_bit_high_increments_tima`, Mooneye `timer/div_write`.)*
8. **A `TIMA` write inside the overflow window cancels the reload and the interrupt.**
   *(Covered: `a_tima_write_in_the_overflow_window_cancels_the_reload`.)*
9. **A `TMA` write during the reload cycle** takes effect on that same reload, and a `TIMA`
   write on that cycle does not. *(Covered:
   `the_reload_cycle_beats_a_tima_write_and_honours_a_tma_write`.)*
10. **Enabling the timer on the M-cycle the tap itself falls still increments.** *(Covered:
    `enabling_on_the_taps_own_falling_edge_still_increments`, Mooneye
    `timer/rapid_toggle`.)*
11. **The counter's post-boot value and phase.** At `PC=$0100` a read must see `$ABCC`
    (DMG) / `$2678` (Color running a DMG-header cartridge) / `$1EA0` (Color running a
    genuine CGB-flagged one), low bits included. *(Covered:
    `mmu::boot_state_tests::the_system_counter_boots_with_the_right_phase` and
    `mmu::timer::tests::a_genuine_cgb_cartridge_boots_div_where_gambatte_measures_it`,
    Mooneye `acceptance/boot_div-dmgABCmgb` and `misc/boot_div-cgbABCDE` and gambatte
    `div/start_inc_{1,2}_cgb04c` — all pass.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Mooneye `acceptance/timer/`** (`div_write`, `tim00`–`tim11`, `tima_reload`, `tima_write_reloading`, `tma_write_reloading`, `rapid_toggle`) | The full timer edge-case matrix | The single best timer suite. **13/13 pass.** Not a vendored submodule — run it with `examples/mooneye_scout.rs` against a local checkout of [mooneye-test-suite](https://github.com/Gekkio/mooneye-test-suite). |
| **Blargg `instr_timing`** | Indirectly leans on timer/interrupt timing | A gross cycle-count error. |
| **Blargg `interrupt_time`** | Timing of the timer/serial interrupts relative to instructions | Would catch dispatch-timing gaps. |
| **Mooneye `acceptance/boot_div-dmgABCmgb`, `misc/boot_div-cgbABCDE`** | The counter's exact value *and phase* at `PC=$0100` — six `DIV` reads at known offsets | **Both pass.** A failure means the post-boot seed in `src/mmu/mod.rs` is wrong, or the per-access tick has moved relative to the access. |

The `retrio/gb-test-roms` submodule (Blargg) is initialised and wired; the decisive timer
tests are in Mooneye, which is not. Wiring Mooneye is the single largest coverage win
available to this component. See [testing](../testing.md).

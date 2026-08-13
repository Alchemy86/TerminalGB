# Timer & Divider

> Source: [`src/mmu/timer.rs`](../../src/mmu/timer.rs) · Related: [MMU](mmu.md) · [CPU](cpu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`303f15f`](https://github.com/Alchemy86/gameboy/tree/303f15fe64435d9724d20e2ac651d480e8a7eaca) · regenerate with `make docs-links`, verify with `make docs-check`.

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

**The `Timer` struct** — [`src/mmu/timer.rs:1`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L1): `divider` (DIV),
`counter` (TIMA), `modulo` (TMA), `enabled`, `step` (T-cycles per TIMA tick), two internal
accumulators `internaldiv`/`internalcnt`, and `interrupt`.

**Registers** — `rb`/`wb` ([`timer.rs:28`,`46`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L28)): writing `DIV`
resets it to 0 ([`timer.rs:48`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L48)); writing `TAC` sets `step` to
`16 / 64 / 256 / 1024` T-cycles ([`timer.rs:59–67`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L59)), i.e.
262144 / 65536 / 16384 / 4096 Hz.

**Counting** — `do_cycle` ([`timer.rs:96`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L96)) accumulates ticks:
every 256 ticks bumps `DIV`; when enabled, every `step` ticks bumps `TIMA`, and on overflow
reloads from `TMA` and sets the interrupt:

```rust
if self.counter == 0 {              // timer.rs:82
    self.counter = self.modulo;
    self.interrupt |= 0x04;
}
```

### Sub-instruction access timing (why `mem_timing` passes)

Although the timer still uses two accumulators (below), the *phase* at which an
instruction's memory access samples the timer is now correct. `MemoryManagementUnit`
advances the timer by one M-cycle at **each** CPU memory access, *before* the access is
performed (`bus_tick`), and applies the instruction's remaining internal cycles
afterward. So an instruction that reads `TIMA` reads it on its **last** M-cycle (and a
read-modify-write reads on the next-to-last and writes on the last), matching hardware.
**Blargg `mem_timing` and `mem_timing-2` PASS** on the strength of this. Only the timer is
stepped per-access; the PPU/APU remain batched (see [CPU §4](cpu.md)).

### Honest approximations

The emulator uses **two independent accumulators** (`internaldiv`, `internalcnt`) rather
than one shared 16-bit system counter. Several documented quirks therefore **do not
occur** (none is exercised by a ROM in the wired suite — Mooneye's timer edge tests are
not vendored — so each remains a documented divergence rather than a red test):

- **DIV-write does not glitch `TIMA`.** On hardware, resetting `DIV` can produce a spurious
  `TIMA` increment (a falling edge on the selected bit). Here `DIV` and `TIMA` are separate
  counters, so writing `DIV` never touches `TIMA` ([`timer.rs:48`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L48)).
- **Changing `TAC` mid-count doesn't glitch.** Hardware can generate an extra increment
  when the selected bit changes; not modelled.
- **`TIMA` overflow reload is instantaneous.** Real hardware has a **4-cycle window**
  between overflow and reload during which `TIMA` reads `0x00`, and writes to `TIMA`/`TMA`
  in that window have special effects (the "TMA write on the reload cycle" behaviour).
  Here reload happens immediately in the same tick ([`timer.rs:108`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/timer.rs#L108)).

## 5. Known quirks and edge cases

- **DIV reset falling-edge increment — NOT modelled** (§4).
- **TIMA reload delay & the write window — NOT modelled** (§4). This is precisely what
  Mooneye's `tima_reload`, `tima_write_reloading`, and `tma_write_reloading` probe.
- **`TAC` glitch on frequency change — NOT modelled.**
- **`DIV` keeps counting regardless of `TAC` enable** — correctly modelled (`internaldiv`
  is always advanced).
- **Overflow requests the interrupt but doesn't dispatch it** — the MMU folds
  `timer.interrupt` into `IF` and clears it ([`src/mmu/mod.rs:368`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L368));
  the CPU dispatches (see [CPU](cpu.md)).

## 6. Expected behaviours as tests

> **Coverage today: none.**

1. **`DIV` rate.** `DIV` increments once per 256 T-cycles (16384 Hz). *(Not covered.)*
2. **`DIV` write resets to 0.** Any `wb(0xFF04, x)` makes `rb(0xFF04) == 0`. *(Not
   covered.)*
3. **`TIMA` rate follows `TAC`.** With `TAC = 0b101` (enabled, `01`), `TIMA` increments
   every 4 M-cycles. *(Not covered.)*
4. **Overflow reloads from `TMA` and sets `IF` bit 2.** With `TMA = 0xFC`, after `TIMA`
   passes `0xFF` it becomes `0xFC` and `IF` bit 2 is set. *(Not covered.)*
5. **`TAC` disable freezes `TIMA` but not `DIV`.** Clearing `TAC` bit 2 stops `TIMA`;
   `DIV` keeps counting. *(Not covered.)*
6. **`TAC` read-back.** `rb(0xFF07)` ORs the unused bits (`| 0xF8`). *(Not covered.)*
7. **DIV-reset falling-edge increment.** Resetting `DIV` at the wrong moment increments
   `TIMA`. **Expected to FAIL — separate counters.** *(Not covered; known-divergent.)*
8. **`TIMA` reload delay.** `TIMA` reads `0x00` for one M-cycle after overflow before the
   `TMA` reload. **Expected to FAIL — instantaneous.** *(Not covered; known-divergent.)*
9. **`TMA` write during the reload cycle** takes effect on that same reload. **Expected to
   FAIL — not modelled.** *(Not covered; known-divergent.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Mooneye `acceptance/timer/`** (`div_write`, `tim00`–`tim11`, `tima_reload`, `tima_write_reloading`, `tma_write_reloading`, `rapid_toggle`) | The full timer edge-case matrix | The single best timer suite; §4/§5 approximations are the expected reds. **Not the declared submodule; not wired.** |
| **Blargg `instr_timing`** | Indirectly leans on timer/interrupt timing | A gross cycle-count error. |
| **Blargg `interrupt_time`** | Timing of the timer/serial interrupts relative to instructions | Would catch dispatch-timing gaps. |

The declared `retrio/gb-test-roms` submodule (Blargg) is **uninitialised**; the decisive
timer tests are in Mooneye, which is unwired. See [testing](../testing.md).

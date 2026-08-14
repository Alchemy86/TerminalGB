# CPU — Sharp LR35902 (SM83) core

> Source: [`src/cpu/`](../../src/cpu) · Related: [MMU](mmu.md) · [Timer & Divider](timer.md) · [PPU](ppu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`0ea589a`](https://github.com/Alchemy86/gameboy/tree/0ea589a24943593937a91697b42e2aa04c77bda2) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The CPU is the part that *runs the game's code*. A Game Boy cartridge is a program: a
long list of tiny numbered instructions ("load this byte", "add these two registers",
"jump if the last result was zero"). The CPU fetches one instruction at a time from
memory, does what it says, and moves on — millions of times a second. Everything else
in the machine (graphics, sound, buttons) is just memory the CPU reads and writes.

In this emulator the CPU is a Rust struct that keeps a copy of the console's registers
and, each "tick", decodes the next opcode and mutates that state. It does not run on the
real chip; it *imitates* what the real chip would do, byte for byte.

## 2. The real hardware

The Game Boy's processor is a **Sharp LR35902**, whose CPU core is usually called the
**SM83**. It is frequently, and wrongly, described as a "Z80". It is not. The SM83 is a
custom core that sits *between* the Intel **8080** and the Zilog **Z80**:

- It borrows the Z80's `CB`-prefixed bit-manipulation instructions (`BIT`/`SET`/`RES`,
  rotates and shifts) and a handful of Z80-isms.
- But it **drops** most of the Z80: no `IX`/`IY` index registers, no alternate register
  set (`AF'`/`BC'`…), no `IN`/`OUT` port instructions, no block instructions (`LDIR`),
  no signed-relative `JP (IX+d)` addressing. Its register file is essentially the 8080's.
- It **adds** things neither parent had: `LD (HL+),A` / `LD (HL-),A` auto-increment
  loads, `LDH` (fast I/O page access at `0xFF00+n`), `ADD SP,r8`, `LD HL,SP+r8`, `STOP`,
  and `SWAP`.
- The **flag register semantics differ**: the half-carry (`H`) and subtract (`N`) flags
  exist specifically to make the `DAA` (decimal adjust) instruction work, and the SM83
  only has four flags (Z, N, H, C) — the low nibble of `F` is permanently zero.

Key facts:

- **Clock:** 4.194304 MHz (DMG). One "machine cycle" (M-cycle) = 4 clock ticks
  ("T-cycles"). Every instruction's duration is a whole number of M-cycles.
- **Registers:** eight 8-bit registers `A F B C D E H L`, paired as `AF BC DE HL`, plus
  16-bit `SP` (stack pointer) and `PC` (program counter).
- **Interrupts:** five sources dispatched through a master enable flag (IME) and the
  `IE`/`IF` register pair.

> Source for hardware behaviour: [Pan Docs — CPU registers & instruction set](https://gbdev.io/pandocs/CPU_Registers_and_Flags.html),
> [gbdev opcode table](https://gbdev.io/gb-opcodes/optables/). The "not a Z80" framing follows the
> gbdev community convention (SM83).

## 3. History and versions

The same SM83 core shipped in every model, at the same 4.19 MHz base clock:

| Model | CPU | Notable difference |
|---|---|---|
| **DMG** (1989, original) | LR35902 | The baseline. 4 grey shades, no boot handshake beyond the logo. |
| **MGB** (Pocket, 1996) | LR35902 | Same CPU; smaller/true-black screen. Boot register `A=0xFF` (vs DMG `0x01`) — the one software-visible CPU-state difference. |
| **SGB** (Super Game Boy) | LR35902 | Same core, clocked slightly faster off the SNES (~4.295 MHz). |
| **CGB** (Color, 1998) | CPU CGB (SM83) | Adds a **double-speed mode** (`STOP` after arming `KEY1`) that runs the core at 8.388608 MHz, plus a WRAM/VRAM bank switch. Boots with `A=0x11`, which is how a cartridge detects "am I on a Color?". |

**This emulator targets CGB** and always boots the CGB core (see §4), falling back to a
"CGB-in-DMG-compatibility" register profile for non-Color cartridges.

## 4. How it works here

The core is a faithful **rboy-lineage** interpreter (the only literal `rboy` credit in the
tree is the frame-timing note at [`src/gameboy.rs:50`](https://github.com/Alchemy86/gameboy/blob/main/src/gameboy.rs#L50), but the CPU
structure mirrors it closely). It is a plain decode-and-execute loop, not a JIT or a
micro-op pipeline.

**The `Cpu` struct** — [`src/cpu/core.rs:15`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L15):

```rust
pub struct Cpu<'a> {
    pub registers: Registers,
    pub ime: bool,        // interrupt master enable
    pub setei: u32,       // EI delay counter (DI is immediate)
    pub halt: u32,
    pub halt_bug: bool,
    pub stop: u32,              // STOP_MODE, or STOP_LOCKED on an undefined opcode
    pub speed_switch_halt: u32, // T-cycles left of a CGB speed switch
    pub memory: MemoryManagementUnit<'a>,
    _executed_operations: Vec<u8>,   // debug only
}
```

**Boots the console the cartridge asks for.** `Cpu::new`
([`src/cpu/core.rs:48`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L48)) resolves a
[`Model`](../../src/mode.rs): a cartridge whose CGB flag (`0x0143` bit 7) is set boots a
Game Boy Color, anything else boots a DMG, and `$GB_MODEL=dmg|cgb` overrides that.
`Cpu::new_with_model` ([`src/cpu/core.rs:53`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L53)) forces one.

*This is new.* Until it existed, `Cpu::new` called `MemoryManagementUnit::new_cgb`
unconditionally and the classic path was unreachable from the main entry point — so a
DMG-only cartridge always ran in the Color console's **compatibility mode**, which is a
third machine and not a DMG (see the table: `ColorAsClassic` boots `A=0x11 HL=0x007C`,
`Classic` boots `A=0x01 HL=0x014D`). Nine Mooneye acceptance tests print those registers
and changed verdict when the DMG mode landed. Boot register values branch on the resolved
mode in `Registers::new`
([`src/cpu/registers.rs:52`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/registers.rs#L52)):

| Mode | A | F | BC | DE | HL | SP | PC |
|---|---|---|---|---|---|---|---|
| `Classic` | `0x01` | `0xB0` | `0x0013` | `0x00D8` | `0x014D` | `0xFFFE` | `0x0100` |
| `ColorAsClassic` | `0x11` | `0x80` | `0x0000` | `0x0008` | `0x007C` | `0xFFFE` | `0x0100` |
| `Color` | `0x11` | `0x80` | `0x0000` | `0xFF56` | `0x000D` | `0xFFFE` | `0x0100` |

**The main loop.** `do_cycle` ([`src/cpu/core.rs:218`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L218)) is thin —
it runs `exec()` and then advances the rest of the machine by that many T-cycles:

```rust
pub fn do_cycle(&mut self) -> u32 {
    let ticks = self.exec() * 4;
    self.memory.do_cycle(ticks)
}
```

`exec()` ([`src/cpu/core.rs:197`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L197)) does three things in order:
`updateime()` (advance EI/DI scheduling), `handleinterrupt()` (maybe service an
interrupt and return early), then either emulate a NOP if halted or run `operation()`.

**`EI` is delayed by one instruction; `DI` is immediate.** `updateime` counts `setei`
down `2 → 1 → IME = true`, and `EI` seeds `setei = 2` **only if the counter is idle** — a
run of consecutive `EI`s must not keep restarting the delay, or interrupts are never
enabled at all (Mooneye `acceptance/ei_sequence`). `DI` sets `IME = false` and clears
`setei` on the spot, which is DMG/MGB/SGB behaviour and what Mooneye
`acceptance/di_timing-GS` and `acceptance/rapid_di_ei` require. `RETI` also enables `IME`
with no delay.

**Opcode tables.** The main 256-entry `match` is `operation()`
([`src/cpu/core.rs:291`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L291)); the `0xCB`-prefixed table is
`misc::cbmap` ([`src/cpu/misc.rs:108`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/misc.rs#L108)). Load/arithmetic/stack
helpers live in `ld.rs`, `data.rs`, `stack.rs`.

**Flags.** `Registers::flag` masks `F &= 0xF0` on every write
([`src/cpu/registers.rs:101`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/registers.rs#L101)), so the low nibble always
reads zero, matching hardware.

### Honest approximations

- **Instruction-granular for the PPU/APU; per-access for the timer.** The CPU executes a
  whole opcode and *then* ticks the MMU/PPU/APU by the full instruction's cycles
  ([`core.rs:218`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L218)). The **timer**, however, is now advanced one
  M-cycle at each CPU memory access, *before* the access is performed (see
  `MemoryManagementUnit::bus_tick`), so a read/write of `TIMA`/`DIV` lands on the correct
  M-cycle. This is why Blargg **`mem_timing` and `mem_timing-2` PASS** (covered). The
  PPU is still batched — invisible to `mem_timing` (which only samples the timer) but is
  what sub-instruction *rendering* tests (mode-3 length) would probe.
- **Interrupt dispatch costs the hardware's 5 M-cycles / 20 T-cycles** and lays them out
  as hardware does: two idle cycles, the `PC`-high push, the `PC`-low push, then the vector
  fetch — with the bus ticked across all five, so the timer keeps its phase through
  dispatch. The vector itself is decided *between* the two pushes, which is why a push
  that lands on `IE` (`SP = $0000`) can cancel or redirect the interrupt. Mooneye
  `acceptance/intr_timing` pins the 5 cycles and `acceptance/interrupts/ie_push` pins the
  sampling point; both pass.

  Blargg `interrupt_time` **runs** (it is `REQUIRE_CGB` and polls `NR52` via
  `sync_apu`/`get_cpu_speed`, so it used to hang on its title screen with no APU; the
  harness now always installs one), measures the **correct 13 cycles** — its own source
  says *"Should take 13 cycles at either speed"* — and **passes**. Earlier readings of its
  `Failed` blamed the double-speed switch; the switch was working and the APU clock was
  not (see `../conformance-notes.md`).
- **Undefined opcodes lock the CPU up, and no longer abort the process.** `$D3 $DB $DD
  $E3 $E4 $EB $EC $ED $F4 $FC $FD` are not instructions; hardware hangs the CPU until a
  reset (Pan Docs, "Unused opcodes"). We latch `stop` and burn one M-cycle per `exec`, so
  the PPU/timer/APU keep running and the frontend stays alive — while `PC` sits on the
  offending byte for a debugger to find. It used to `panic!`, which under
  `[profile.release] panic = 'abort'` killed the process *and* any unwritten battery RAM;
  a runaway `PC` in any game reaches it. `$FC` was separately special-cased as a `NOP`,
  which is neither what hardware does nor what the other ten did. See
  `cpu::core::lockup_tests`.
- **`STOP` is the real instruction** — [`Cpu::stop_instruction`](../../src/cpu/core.rs)
  implements Pan Docs' whole decision chart: STOP mode (ended only by a selected `P1`
  line going low, never by an interrupt), the CGB speed switch (`DIV` reset, then
  `0x20000` T-cycles of the *new* clock in HALT — 143.7 scanlines of PPU time, which is
  the display collapse Pan Docs warns about), the held-button and pending-interrupt
  branches, and the 1-byte/2-byte opcode length. A stopped machine's panel goes blank on
  a DMG and black on a Color, *unless* the PPU was in mode 3 when the CPU stopped, in
  which case it keeps the VRAM access it already had and the picture stays.
  `daid/stop_instr` (both consoles), `daid/stop_instr_gbc_mode3` and
  `daid/speed_switch_timing_div` pass; `speed_switch_timing_ly`/`_stat` do not, for a PPU
  phase reason measured and written up in `../conformance-notes.md`.
- **~900 lines of legacy interrupt/HALT logic are commented out** below `do_cycle`
  ([`core.rs:222`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L222)) — dead code, not the live path.
- **`src/cpu/z80`** (461 lines, no extension) is a **non-compiled reference file** — a
  commented port of Imran Nazar's JavaScript jsgb interpreter that early handlers were
  derived from. It is not a module.

## 5. Known quirks and edge cases

- **The HALT bug — MODELLED (covered).** On real hardware, executing `HALT` while
  `IME=0` *and* an interrupt is already pending (`IE & IF & 0x1F != 0`) does **not** halt;
  instead the CPU fails to increment `PC`, so the byte after `HALT` is read **twice**. The
  `0x76` handler now sets a `halt_bug` flag under that condition instead of halting, and
  `operation()` rewinds `PC` by one after the next opcode fetch so the following byte (and
  any operands, shifted one byte early) is re-read. **Blargg's `halt_bug` ROM now renders
  `Passed`.** (Pan Docs — [halt](https://gbdev.io/pandocs/halt.html).)
- **EI timing.** `EI` enables interrupts *after the following instruction*, so
  `EI; DI` never services an interrupt in between. Modelled via the `setei` delay; `DI`
  cancels it immediately.
- **Where an instruction's internal cycles fall is observable.** `PUSH`/`RST` spend their
  idle M-cycle *before* the two writes, `CALL` spends it after the operand fetch and before
  the pushes, and `RET cc` spends one evaluating the condition *before* the two reads.
  Leaving those as end-of-instruction slack — which is what happens when only bus accesses
  advance the clock — moves every one of those accesses an M-cycle early. Mooneye measures
  it directly through the OAM-DMA guard (`rst_timing`, `call_timing*`, `push_timing`,
  `ret_cc_timing`, …); see [`src/cpu/stack.rs`](../../src/cpu/stack.rs) and
  [MMU](mmu.md).
- **A 16-bit push writes the high byte first**, at `SP-1`, then the low byte at `SP-2`.
  The order is observable both through the OAM-DMA guard and at `SP = $0000`, where the
  first push wraps onto `IE`.
- **Interrupt priority.** Lowest bit wins: `handleinterrupt` uses
  `triggered.trailing_zeros()` ([`core.rs:182`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L182)), giving VBlank
  (bit 0) highest priority through Joypad (bit 4). Vector = `0x40 | (n << 3)`.
- **HALT wake without dispatch.** If `IME=0` but an interrupt becomes pending while
  halted, the CPU wakes and continues *without* jumping to the vector — modelled at
  [`core.rs:173`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L173) (`halt = 0` is set before the `!ime` early
  return).
- **`n >= 5` panics.** A sixth interrupt bit triggers `panic!("Invalid interrupt
  triggered")` ([`core.rs:184`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L184)) — defensive, not hardware.

## 6. Expected behaviours as tests

> The specification we test *against*, before changing any emulation. Each is phrased so
> it could become an assertion. **Coverage today: none of these have a Rust test** — the
> only CPU-adjacent tests in the tree are the input/MBC checksum ones.

1. **Boot register state (CGB).** After `Cpu::new` on a Color cartridge, `A=0x11`,
   `F=0x80`, `SP=0xFFFE`, `PC=0x0100`. *(Not covered.)*
2. **Boot register state (DMG/Classic).** In Classic mode: `A=0x01`, `F=0xB0`,
   `BC=0x0013`, `DE=0x00D8`, `HL=0x014D`. *(Not covered.)*
3. **`F` low nibble is always zero.** After any ALU op, `registers.f & 0x0F == 0`.
   *(Not covered.)*
4. **Flag correctness for `ADD`/`SUB`/`CP`/`DAA`.** Z/N/H/C match hardware for boundary
   inputs (e.g. `0x0F + 0x01` sets H; `0xFF + 0x01` sets Z and C). *(Not covered — this
   is exactly what Blargg `cpu_instrs` verifies.)*
5. **`EI` is delayed one instruction, `DI` is not, and repeated `EI`s do not restart the
   delay.** *(Covered: Mooneye `acceptance/ei_timing`, `ei_sequence`, `di_timing-GS`,
   `rapid_di_ei`.)*
6. **Interrupt dispatch pushes `PC` and jumps to the vector.** After a VBlank interrupt
   with `IME=1`, `SP` has decreased by 2, `[SP]` holds the old `PC`, and `PC == 0x0040`;
   `IF` bit 0 is cleared and `IME` is now false. *(Covered: Mooneye
   `acceptance/interrupts/ie_push`.)*
7. **Interrupt dispatch timing = 20 T-cycles**, with `PC`-high pushed before `PC`-low and
   the vector latched between the two pushes. *(Covered: Mooneye
   `acceptance/intr_timing`, `acceptance/interrupts/ie_push`, `acceptance/reti_intr_timing`,
   `acceptance/halt_ime1_timing*`; and Blargg `interrupt_time` now prints the correct 13.)*
8. **Interrupt priority.** With `IF` and `IE` both `0x1F`, VBlank (vector `0x40`) is
   serviced first. *(Not covered.)*
9. **HALT with `IME=1` and pending interrupt** wakes, services the interrupt, and returns
   after `HALT`. *(Not covered.)*
10. **The HALT bug.** `HALT` with `IME=0` and `IE & IF & 0x1F != 0` must re-read the
    following byte (no `PC` increment). **COVERED — modelled; Blargg `halt_bug` renders
    `Passed`** (frame-hash pinned in `baseline.txt`).
11. **`CB` bit ops.** `BIT 7,H` on `H=0x80` sets Z=0, H=1, N=0 and leaves C untouched.
    *(Not covered.)*
12. **`LD (HL+),A` auto-increments HL** by one after the store. *(Not covered.)*
13. **An undefined opcode locks the CPU up and does not kill the emulator.** After
    executing any of `$D3 $DB $DD $E3 $E4 $EB $EC $ED $F4 $FC $FD`, `PC` stops advancing,
    no interrupt wakes it, and the PPU keeps producing frames. *(Covered:
    `cpu::core::lockup_tests` — no test ROM exercises this, since test ROMs only execute
    valid opcodes.)*

## 7. Test ROMs that exercise it

The repo declares the [`retrio/gb-test-roms`](https://github.com/retrio/gb-test-roms)
submodule in [`.gitmodules`](../../.gitmodules) as `tests/`, but it is **uninitialised**
(`git submodule status` → `-c240dd7…`). The Blargg "results" in the old README are manual
OpenEmu screenshots, not an automated harness.

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg `cpu_instrs`** (11 sub-tests) | Every opcode's result and flag behaviour | A decode/flag bug in `operation()`/`cbmap`. *Claimed passing today — lock it in.* |
| **Blargg `instr_timing`** | Per-instruction M-cycle counts | Wrong cycle table entry (affects everything downstream). |
| **Blargg `mem_timing` / `mem_timing-2`** | *When* within an instruction memory is touched | **PASS** — timer stepped per memory access (§4). |
| **Blargg `halt_bug`** | The HALT bug | **PASS** — modelled (quirk §5). |
| **Blargg `interrupt_time`** | Interrupt dispatch timing | **Runs and prints `Failed`** — but the dispatch time it measures is now correct (13). The remaining failure is the CGB double-speed switch (§4). Needs an APU installed or it hangs on its title screen. |
| **Mooneye `acceptance/`** (66 tests) | Interrupt edge timing, instruction M-cycle placement, timer, OAM DMA | The gold standard for the corners above. **52/66 pass** (same under Mooneye's register criterion and the shootout's screenshot compare). Run it with `cargo run --release --example mooneye_scout -- <mooneye-test-suite>/acceptance`; Mooneye is **not** a vendored submodule. |

Blargg ROMs report pass/fail over the **serial port** — see [Serial](serial.md) for the
capture plumbing that a harness would use.

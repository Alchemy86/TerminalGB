# CPU — Sharp LR35902 (SM83) core

> Source: [`src/cpu/`](../../src/cpu) · Related: [MMU](mmu.md) · [Timer & Divider](timer.md) · [PPU](ppu.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`6ec57e1`](https://github.com/Alchemy86/gameboy/tree/6ec57e1ab5c6ad3d69285098251279a666ccb064) · regenerate with `make docs-links`, verify with `make docs-check`.

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

**The `Cpu` struct** — [`src/cpu/core.rs:14`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L14):

```rust
pub struct Cpu<'a> {
    pub registers: Registers,
    pub ime: bool,        // interrupt master enable
    pub setdi: u32,       // DI delay counter
    pub setei: u32,       // EI delay counter
    pub halt: u32,
    pub stop: u32,
    pub memory: MemoryManagementUnit<'a>,
    _executed_operations: Vec<u8>,   // debug only
}
```

**Always boots CGB.** `Cpu::new` calls `MemoryManagementUnit::new_cgb(...)`
([`src/cpu/core.rs:34`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L34)); the classic-only `MemoryManagementUnit::new`
path is effectively unreachable from the main entry point. Boot register values still
branch on the detected mode in `Registers::new`
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

**EI/DI are delayed by one instruction.** `updateime`
([`src/cpu/core.rs:140`](https://github.com/Alchemy86/gameboy/blob/main/src/cpu/core.rs#L140)) counts `setei`/`setdi` down `2 → 1 →
applied`. `DI` seeds `setdi = 2`, `EI` seeds `setei = 2`, `RETI` seeds `setei = 1`.

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
- **Interrupt dispatch costs 16 T-cycles**, not the hardware's 20 — *still divergent, but
  not test-driven-fixable in this repo yet.* `handleinterrupt` returns `4`
  ([`src/cpu/core.rs`](../../src/cpu/core.rs)), ×4 in `do_cycle` = 16 T-cycles. Real
  dispatch is **5 M-cycles / 20 T-cycles** (Pan Docs). The one ROM that would prove a
  red→green fix, Blargg `interrupt_time`, **cannot run headless**: it is `REQUIRE_CGB`,
  switches to double speed, and calls `sync_apu`/`get_cpu_speed` which poll `NR52` — with
  no APU instantiated in the headless build those polls never terminate, so the test
  hangs on its title screen before any timing is measured. Changing 16→20 blindly (no
  failing test to validate against) is deliberately **not** done here; see §7.
- **`STOP` is minimally handled** (the `stop` field exists; double-speed switching is
  plumbed through the MMU, not a faithful `STOP` model).
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
  `EI; DI` never services an interrupt in between. Modelled via the `setei` delay.
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
5. **`EI` is delayed one instruction.** `DI; EI; DI` with a pending interrupt never
   dispatches. *(Not covered.)*
6. **Interrupt dispatch pushes `PC` and jumps to the vector.** After a VBlank interrupt
   with `IME=1`, `SP` has decreased by 2, `[SP]` holds the old `PC`, and `PC == 0x0040`;
   `IF` bit 0 is cleared and `IME` is now false. *(Not covered.)*
7. **Interrupt dispatch timing = 20 T-cycles.** Servicing an interrupt should advance the
   machine clock by 5 M-cycles. **This emulator charges 16 T-cycles.** *(Not covered;
   known-divergent — **documented non-goal**: the only ROM that exercises it,
   `interrupt_time`, hangs headless with no APU, so there is no red→green test to fix
   against. See §4 and `testing.md`.)*
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
| **Blargg `interrupt_time`** | Interrupt dispatch timing | **Documented non-goal** — hangs headless (no APU; polls `NR52`). Would catch the 16-vs-20 T-cycle gap (test #7) if it could run. |
| **Mooneye `acceptance/interrupts/`, `acceptance/instr/`, `acceptance/timer/`** | Interrupt edge timing (`ie_push`, `halt_ime*`) | The gold standard for the corners above. **Mooneye is not the declared submodule** and is not wired. |

Blargg ROMs report pass/fail over the **serial port** — see [Serial](serial.md) for the
capture plumbing that a harness would use.

# MMU — memory bus, DMA & interrupt wiring

> Source: [`src/mmu/mod.rs`](../../src/mmu/mod.rs) · Related: [Cartridges/MBC](mbc.md) · [PPU](ppu.md) · [Timer](timer.md) · [Serial](serial.md) · [Joypad](input.md)
> [← Component index](README.md) · [🗺️ Code map](../codemap.md)

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`50ecb56`](https://github.com/Alchemy86/gameboy/tree/50ecb56d60eaa1153604ae57ca887959688eeb39) · regenerate with `make docs-links`, verify with `make docs-check`.

## 1. What it is

The MMU ("Memory Management Unit", `MemoryManagementUnit` here) is the **switchboard**.
The CPU only ever says "read the byte at address `X`" or "write byte `V` to address `X`".
The MMU decides what that address actually *is*: cartridge ROM, video RAM, work RAM, a
hardware register, the joypad, the sound chip, and so on. It is the single hub every other
component hangs off; almost every interesting hardware behaviour is really "what happens
when you touch address `X`".

It also owns two things that don't belong to any one chip: the **DMA engines** (bulk
memory copies), and the **interrupt flags** (`IF`/`IE`) that let the components signal the
CPU.

## 2. The real hardware

The Game Boy CPU has a **16-bit address bus**, so it can address **65,536 bytes (64 KB)**.
That space is a fixed map (Pan Docs — [Memory Map](https://gbdev.io/pandocs/Memory_Map.html)):

```
0x0000–0x3FFF  ROM bank 0 (fixed)        0xE000–0xFDFF  Echo of C000–DDFF
0x4000–0x7FFF  ROM bank N (switchable)   0xFE00–0xFE9F  OAM (sprite attributes)
0x8000–0x9FFF  VRAM (bank 0/1 on CGB)    0xFEA0–0xFEFF  Prohibited
0xA000–0xBFFF  Cartridge RAM             0xFF00–0xFF7F  I/O registers
0xC000–0xCFFF  WRAM bank 0               0xFF80–0xFFFE  High RAM (HRAM)
0xD000–0xDFFF  WRAM bank 1–7 (CGB)       0xFFFF         Interrupt Enable (IE)
```

Notable real-hardware behaviours the map implies:

- **Echo RAM** (`0xE000–0xFDFF`) mirrors `0xC000–0xDDFF`; using it is discouraged but legal.
- **The I/O page** (`0xFF00–0xFF7F`) is not memory — each address is a different chip's
  register with its own read/write rules (many bits read back as 1).
- **DMA:** `0xFF46` triggers an **OAM DMA** (copy 160 bytes into OAM); on CGB, `0xFF51–55`
  drive **HDMA/GDMA** (VRAM block copies). During OAM DMA the CPU can only touch HRAM.
- **`IF` (`0xFF0F`) and `IE` (`0xFFFF`)** are the request/enable pair for the five
  interrupts; the CPU services one when `IME` is set and `IF & IE != 0`.

## 3. History and versions

| Model | Memory differences |
|---|---|
| **DMG / MGB / SGB** | 8 KB WRAM, 8 KB VRAM, single banks. One flat map. |
| **CGB** | **32 KB WRAM** in 8 switchable banks (`SVBK`/`0xFF70`), **16 KB VRAM** in 2 banks (`VBK`/`0xFF4F`), the HDMA/GDMA engine (`0xFF51–0xFF55`), a **double-speed switch** (`KEY1`/`0xFF4D`), and three undocumented registers (`0xFF72/73/75`). Non-Color cartridges run in a compatibility mode. |

**This emulator always constructs the CGB MMU** (`new_cgb`) and gates the CGB-only
registers behind mode checks, returning `0xFF`/no-op in Classic mode.

## 4. How it works here

Mirrors rboy's MMU: components expose `rb`/`wb`/`do_cycle` and raise interrupts by ORing a
per-device `interrupt` byte, which the MMU folds into `IF`.

**The struct** — [`src/mmu/mod.rs:45`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L45). Owns `wram` (`0x8000` = 8
banks), `zram` (HRAM, note it's named `zram`), `gpu`, `timer`, `serial`, `keypad`,
`Option<Sound>`, `mbc`, the interrupt bytes `inte` (IE) and `intf` (IF), the HDMA state,
`wrambank`, and mode/speed fields.

**Three constructors over one builder.** `raw` ([`mod.rs:103`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L103))
maps the cartridge and `fill_random`s the WRAM but does *not* choose a console; `new`
([`mod.rs:144`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L144), DMG, rejects CGB-only carts), `new_cgb`
([`mod.rs:159`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L159)) and `new_with_model`
([`mod.rs:168`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L168)) then commit a mode via `set_mode`
([`mod.rs:216`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L216)) and run `set_initial`
([`mod.rs:180`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L180)) to write the post-boot I/O defaults — this is the
**boot-ROM handoff** (see §5). Sharing `raw` is deliberate: the DMG and CGB paths used to be
two copies of the same struct literal and could drift apart. `new_with_model` takes a
[`Model`](../../src/mode.rs) (`Auto`/`Dmg`/`Cgb`) and is what `Cpu::new` calls.

**The dispatch** — `rb` ([`mod.rs:391`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L391)) / `wb`
([`mod.rs:450`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L450)) are the address `match`. Echo RAM is folded into the
WRAM arms with `& 0x0FFF` masking ([`mod.rs:399–400`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L399)). `IF` reads
back with the top three bits set (`intf | 0b11100000`, [`mod.rs:409`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L409)).

**`do_cycle`** ([`mod.rs:347`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L347)) ticks the sub-components and folds
their interrupts into `intf`: it runs VRAM DMA first, then `timer` and `sound` on
double-speed-aware `cputicks`, and `gpu`/VRAM-DMA on `gputicks`. **The timer, however, is
advanced per memory access, not just here.** While a CPU instruction executes, every
`rb`/`wb` first calls `bus_tick` — one M-cycle of timer — so accesses land on the correct
sub-instruction cycle (this is why Blargg `mem_timing` passes); `do_cycle` then applies
only the instruction's *remaining* internal cycles to the timer so the per-instruction
total is unchanged. `bus_tick` is a no-op unless a CPU instruction is executing, so OAM
DMA (which re-uses `rb`/`wb`) and boot-time setup do not double-count — `oamdma` toggles
the flag off around its copy.

**OAM DMA** — `oamdma` ([`mod.rs:521`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L521)):

```rust
fn oamdma(&mut self, value: u8) {
    let prev = self.set_bus_ticking(false); // don't tick the timer per byte
    let base = (value as u16) << 8;
    for i in 0..0xA0 {
        let b = self.rb(base + i);
        self.wb(0xFE00 + i, b);
    }
    self.set_bus_ticking(prev);
}
```

**HDMA/GDMA (CGB)** — `hdma_write`/`hdma_read` ([`mod.rs:549`,`534`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L549)),
`perform_gdma` (all rows at once, [`mod.rs:603`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L603)) and `perform_hdma`
(one 16-byte row per HBlank, gated on `gpu.may_hdma()`, [`mod.rs:590`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L590)).

### Honest approximations

- **OAM DMA is timed but only *OAM* is guarded.** [`src/mmu/oamdma.rs`](../../src/mmu/oamdma.rs)
  models the real schedule — the write to `$FF46` is M=0, the transfer starts at M=2, and
  it copies one byte per M-cycle for 160 M-cycles — and while it runs the CPU reads `$FF`
  from OAM and its writes there are dropped. What is **not** modelled is the rest of the
  bus restriction: hardware also stops the CPU seeing ROM/RAM during the transfer (which is
  why DMA-wait routines must run from HRAM), and a CPU read that collides with the
  transfer's own bus returns the byte the DMA is moving. Programs that run their wait loop
  from HRAM — i.e. all of them — see no difference.
- **VRAM/OAM access is never blocked by PPU mode.** The CPU can read/write VRAM and OAM in
  any mode; hardware returns `0xFF` during modes 2/3. This interacts with the OAM
  corruption bug (see §5).
- **GDMA/HDMA cycle costs are approximate** (`len*8` / `8` ticks;
  [`mod.rs:610`,`593`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L610)) and GDMA does not stall the CPU for its full
  duration.
- **Prohibited region `0xFEA0–0xFEFF`** is not given the documented OAM-block-dependent
  read behaviour; unmapped reads return `0xFF` ([`mod.rs:442`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L442)).

## 5. Known quirks and edge cases

- **OAM corruption bug (DMG) — NOT modelled (documented non-goal).** On DMG, certain
  16-bit operations (`INC`/`DEC` of a register used as an address, etc.) that touch the
  `0xFE00–0xFEFF` range while the PPU is in mode 2 corrupt OAM in a specific pattern.
  Because this emulator never blocks OAM by mode, the precondition can't even arise, and
  reproducing it would require the mode-dependent OAM bus model below. **Deliberately not
  fixed:** it is a DMG-only hardware *defect* that the emulator boots in CGB mode
  (`new_cgb`) anyway, essentially no commercial game depends on, and modelling it is real
  work (mode-2 bus tracking + the corruption pattern) for ~zero practical gain. Blargg
  `oam_bug` therefore stays red-by-design and is pinned as such in `baseline.txt`. (Pan
  Docs — [OAM corruption](https://gbdev.io/pandocs/OAM_Corruption_Bug.html).)
- **OAM DMA restricted bus.** During a real OAM DMA the CPU running from ROM would read
  DMA-bus garbage unless it runs from HRAM; games copy a tiny wait loop into HRAM for
  exactly this. The emulator's instant DMA sidesteps the whole mechanism.
- **The boot-ROM handoff.** There is no boot ROM here; `set_initial`
  ([`mod.rs:176`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L176)) writes the exact post-boot I/O register values
  (`NR*`, `LCDC=0x91`, `BGP=0xFC`, timer regs, …) the boot ROM would have left, and the
  CPU starts at `PC=0x0100` with the boot registers (see [CPU](cpu.md)). The Nintendo logo
  scroll and the header checksum lock are therefore skipped.
- **Echo RAM** is faithfully mirrored (writes to `0xE000+` change `0xC000+`).
- **CGB double-speed** halves per-cycle divisions for timer/sound and doubles DMA cost via
  `cpudivider` ([`mod.rs:348`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L348)), but `STOP`-based speed switching
  itself is only plumbed, not cycle-accurate.

## 6. Expected behaviours as tests

> **Coverage today: none.**

1. **Address routing.** `wb(0xC000, v)` then `rb(0xC000) == v`; VRAM writes land in `gpu`,
   cartridge-RAM writes route to the MBC. *(Not covered.)*
2. **Echo RAM mirror.** `wb(0xC000, 0xAB)` makes `rb(0xE000) == 0xAB`, and vice versa.
   *(Not covered.)*
3. **`IF` read-back.** After `wb(0xFF0F, 0x00)`, `rb(0xFF0F) == 0xE0` (top three bits
   read as 1). *(Not covered.)*
4. **OAM DMA copies 160 bytes.** After `wb(0xFF46, 0xC0)` and 162 M-cycles, OAM
   `0xFE00–0xFE9F` equals `0xC000–0xC09F`. *(Covered: Mooneye `acceptance/oam_dma/basic`.)*
5. **OAM DMA takes 160 M-cycles, starting two M-cycles after the write, and OAM is the
   DMA's for the duration.** *(Covered: `src/mmu/oamdma.rs` unit tests, and Mooneye
   `acceptance/oam_dma_start`, `oam_dma_timing`, `oam_dma_restart`, `oam_dma/basic`,
   `oam_dma/reg_read`, `oam_dma/sources-GS` — all pass.)* Restricting the **rest** of the
   bus is still not modelled and would need a cycle-stepped CPU. *(Partially covered.)*
6. **VRAM is unreadable during mode 3** (returns `0xFF`). **Documented non-goal — not
   enforced.** Same structural dependency as #5 (needs cycle-accurate PPU/CPU bus
   arbitration). *(Not covered; structural non-goal.)*
7. **CGB WRAM banking.** With `SVBK` selecting bank 3, `0xD000` accesses WRAM bank 3;
   bank 0 selection aliases to bank 1. *(Not covered.)*
8. **CGB VRAM banking.** `VBK` (`0xFF4F`) switches the `0x8000–0x9FFF` bank. *(Not covered.)*
9. **GDMA transfers `(len+1)*16` bytes** and reports complete via `HDMA5` bit 7. *(Not
   covered.)*
10. **HDMA transfers one 16-byte row per HBlank** and stops after the programmed length.
    *(Not covered.)*
11. **Post-boot register defaults.** After construction, `rb(0xFF40) == 0x91` (LCDC),
    `rb(0xFF47) == 0xFC` (BGP). *(Not covered.)*
12. **OAM corruption bug.** A qualifying mode-2 OAM access corrupts OAM per the documented
    pattern. **Documented non-goal — precondition not modelled** (DMG-only defect; we boot
    CGB; ~no games depend on it). Blargg `oam_bug` stays red-by-design in `baseline.txt`.
    *(Not covered; deliberate non-goal — §5.)*

## 7. Test ROMs that exercise it

| Suite | What it checks | Meaning of a failure |
|---|---|---|
| **Blargg `oam_bug`** | The DMG OAM corruption bug | **Documented non-goal** — bug not modelled (§5); pinned red in `baseline.txt`. |
| **Blargg `mem_timing` / `mem_timing-2`** | Cycle-exact timing of memory reads/writes | **PASS** — timer stepped per memory access (see [CPU §4](cpu.md), [Timer §4](timer.md)). |
| **Mooneye `acceptance/oam_dma*`** (`basic`, `reg_read`, `sources-GS`, `oam_dma_start`, `oam_dma_timing`, `oam_dma_restart`) | OAM-DMA copy, start delay, duration, restart, legal sources | **6/6 pass.** A failure means the transfer schedule or the OAM guard has moved. |
| **Mooneye instruction-timing tests** (`rst_timing`, `call_timing*`, `push_timing`, `ret*_timing`, `jp*_timing`, `add_sp_e_timing`, `ld_hl_sp_e_timing`) | Which M-cycle each memory access of an instruction lands on — measured *through* the OAM-DMA guard | **All pass.** They are only measurable because OAM DMA is timed; see [`src/mmu/oamdma.rs`](../../src/mmu/oamdma.rs). |
| **Mooneye `acceptance/bits/`** (`unused_hwio-GS`, `reg_f`) | Unused-bit read-back behaviour of hardware registers | Bad `\| 0b…` masks in `rb`. Blargg's `bits-unused`/`bits-bank1` are the repo's known failures here. |
| **Mooneye `emulator-only/mbcX/`** | (routes into the MBC — see [MBC](mbc.md)) | — |

The declared submodule `retrio/gb-test-roms` (Blargg) is **uninitialised**; Mooneye is a
separate, unwired suite. See [testing](../testing.md).

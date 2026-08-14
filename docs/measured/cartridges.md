# Cartridges — mappers, the clock, and the class of bug that eats a save

← [The measured Game Boy](README.md)

Mooneye `emulator-only` is **28/28** (`mbc1` 13/13 · `mbc2` 7/7 · `mbc5` 8/8) and ax6's
`rtc3test` went **0/3 → 3/3**. But the most important material on this page is not a suite
score at all: it is the set of defects that **destroy the player's save file**, which no public
test ROM catches, because no test ROM tries to crash its host or looks at the filesystem.

Hardware reference, not restated here:
[MBCs](https://gbdev.io/pandocs/MBCs.html) ·
[MBC1](https://gbdev.io/pandocs/MBC1.html) · [MBC3](https://gbdev.io/pandocs/MBC3.html) ·
[MBC5](https://gbdev.io/pandocs/MBC5.html) ·
[The Cartridge Header](https://gbdev.io/pandocs/The_Cartridge_Header.html). Component page:
[`docs/components/mbc.md`](../components/mbc.md).

---

<!-- codemap:asof -->
> 🗺️ **Code map** · source links target [`main`](https://github.com/Alchemy86/gameboy/tree/main); line references documented as of commit [`5092a34`](https://github.com/Alchemy86/gameboy/tree/5092a34) · regenerate with `make docs-links`, verify with `make docs-check`.

## Persistence: never in a `Drop` impl

**`Drop` is not a shutdown hook.** The desktop frontend's event loop never returns — winit
calls `process::exit(0)` — so the `Gameboy` moved into that closure is never dropped.
`[profile.release] panic = 'abort'` skips destructors too, as do a closed terminal window,
`SIGTERM` and `kill`.

Cartridge RAM used to be written **only** in `impl Drop for MBCx`. That is why in-game saves
vanished.

How it works now: a RAM write marks the battery dirty; `Gameboy::frame` calls `autosave`
([`src/mbc/mod.rs:80`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L80)) once per frame, which writes out after 60 quiet
frames (~1 s) or 600 dirty ones. `Gameboy::flush_battery`
([`src/gameboy.rs:696`](https://github.com/Alchemy86/gameboy/blob/main/src/gameboy.rs#L696)) forces it. `Drop` still flushes as a last chance
only.

Two further rules, both learned the expensive way:

- **Writes are atomic** (`<file>.tmp` → `fsync` → `rename`). A truncated save is worse than no
  save.
- **A session that never wrote cartridge RAM writes nothing.** The old code unconditionally
  dumped its pristine all-zero RAM on exit, so *one launch that failed to load permanently
  erased a real save* — observed here as a 32,776-byte file overwritten with zeros. One bad
  launch, and the save is gone with no error anywhere.
- **A save that cannot be read is never overwritten.** The battery poisons itself on a read
  error, so loading fails loudly instead of silently starting a new game over an existing save.

### The `.sav` format is the ecosystem's, not ours

Settled by studying FlashGBX — *formats and constants only, never its code or its bundled
database*, per [`CONTRIBUTING.md`](../../CONTRIBUTING.md).

| | |
|---|---|
| `<rom>.sav` | raw cartridge RAM, bank 0 first, **no header**, exactly `ramsize` bytes |
| MBC3 clock | a **48-byte footer appended to that same file**, all little-endian: 5 × `u32` registers (S, M, H, day-low, day-high — bit 0 = day bit 8, bit 6 = HALT, bit 7 = CARRY), the same five again as the latched copy, then a `u64` UNIX timestamp |
| Written only for | header types `0x0F`/`0x10`. **Type `0x13` has no clock and must get nothing extra.** |
| Detected on load by | size: `filesize - ramsize == 48` (or 44 for an older 32-bit-timestamp variant) |
| Also read | `.srm` |

Legacy `<rom>.gbsave` (an 8-byte **big-endian** RTC anchor *prepended* to the RAM) is still
read when no `.sav` exists, migrated, and **left on disk untouched as a backup**. That prefix
was applied even to type `0x13`, so **every foreign save used to load 8 bytes shifted** — and
with a save checksum that is indistinguishable from "NEW GAME". Never reintroduce a header.

> **A real cartridge is a reference oracle for saves**, now that our format matches: play in
> the emulator, write the `.sav`, restore it to the cart with a dumper, read it back, compare.
> No emulator-only suite can make that check.

**MBC2 stores the low nibble** (`v & 0x0F`) and ORs in `0xF0` on read. Storing `v | 0xF0` plays
identically and makes our `.sav` differ byte-for-byte from a hardware dump.

---

## Four aborts a cartridge could trigger — and why that class is the worst

Under `panic = 'abort'` a panic is process death **and** the loss of any cartridge RAM not yet
written. **Cartridge-supplied input must never be able to kill the process.**

| Defect | What triggers it |
|---|---|
| `% rambanks` divide-by-zero | a RAM-less MBC5+RUMBLE cart (types `0x19`/`0x1C`). **Bit 3 of the bank register is the rumble motor**, so every rumble game hits it on its first motor poke |
| `% rombanks` divide-by-zero | `rom_banks()` returning 0 for unofficial `0x148` codes — `0x52`/`0x53`/`0x54` are real. It falls back to `rom.len()/0x4000`, minimum 2 ([`src/mbc/mod.rs:206`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/mod.rs#L206)) |
| unchecked RAM indexing | a short or foreign `.sav`. Loaded saves are normalised to the declared RAM size and every access is bounds-checked |
| out-of-range VRAM-DMA destination | a CGB game pointing an HDMA near `$9FFF`; the counter walked into the PPU's catch-all `panic!`. It is `0x8000 \| (dst & 0x1FFF)` now, which is also exactly what SameSuite `dma/gdma_addr_mask` measures |

Plus the same shape one level up: **undefined opcodes** (`$D3 $DB $DD $E3 $E4 $EB $EC $ED $F4
$FC $FD`) used to abort. Hardware *locks the CPU up* until reset ([Pan Docs, unused
opcodes](https://gbdev.io/pandocs/CPU_Instruction_Set.html)), so `Cpu::exec` now returns 1
M-cycle while stopped — the PPU, timer and APU keep running and the frontend stays alive with
`PC` parked on the offending byte. **A runaway `PC` in any game reaches this.** `$FC` had been
special-cased as a `NOP`, which is neither hardware's behaviour nor its ten siblings'.

> **No public suite catches an emulator aborting on legal-but-odd guest input.** Every test ROM
> executes only valid opcodes and none tries to crash its host. That gap is a finding in its
> own right, and it is why these are Rust tests rather than test-ROM rows.

---

## The save guard: a crashed game must never reach the battery file

A Game Boy has no fault handler, so a program counter that walks off its own code keeps
executing and reliably *stores bytes into cartridge RAM*. Measured on a real cartridge crashing
out of a Cable Club link battle: **4,378 bytes of cartridge RAM changed within six frames**,
all of which the autosave would have persisted over a real save.

`MemoryManagementUnit::report_fault` ([`src/mmu/mod.rs:1182`](https://github.com/Alchemy86/gameboy/blob/main/src/mmu/mod.rs#L1182)) does
exactly one thing: **it withdraws the emulator's right to write the save file**
([`src/mbc/battery.rs:181`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/battery.rs#L181)). Sticky for the
session, first reason kept, loud on stderr, and a `Fault: SAVE NOT WRITTEN` row in the UI. It
deliberately does *not* stop, reset or "fix" the machine — detectors are heuristics, so only
the irreversible act is taken away.

Three detectors:

1. an **undefined opcode**;
2. **fetching an opcode from `$FEA0-$FF7F`** — the unusable block and I/O, which genuinely
   cannot hold code;
3. **a plugin reporting a fault, which needs no capability at all** — every other call lets a
   plugin *do* something; this one only takes a right away, so fail-safe is to allow it.

Neither CPU-level detector fires on that link-battle crash (measured over 600 post-crash
frames: the runaway code stays in executable memory), which is the entire argument for the
third.

> **The window is `$FEA0-$FF7F`, not `$FE00-$FF7F`, and getting that wrong poisoned real
> saves.** Three Mooneye ROMs that **pass** (`add_sp_e_timing`, `ld_hl_sp_e_timing`,
> `oam_dma_start`) execute code from OAM *on purpose*, to time an access onto a known byte.
> OAM is real RAM the CPU may read outside PPU modes 2 and 3. With the old window, a game
> legitimately running a routine out of OAM **silently stopped saving for the rest of the
> session.** A runaway `PC` that walks *into* OAM still faults — it keeps walking and reaches
> `$FEA0`.

---

## MBC1M multicarts are a different *wiring*, not a different mapper

BANK1's top bit is not connected, and BANK2 drives bank bits **4–5** instead of 5–6 — so in
mode 1 the `$0000-$3FFF` region shows `$00/$10/$20/$30`, which is how a compilation cart
switches games. The `00 → 01` translation still uses all five *written* bits before bit 4 is
dropped, so writing `$10` selects bank `$00` and writing `$00` selects `$01`. That single rule
is what Mooneye's expected-bank table turns on.

All three differences live in three one-line helpers, so the ordinary path is unchanged and the
state layout did not move — **no save-state format bump**.

**Detection is a guess, and has to be.** Such a cart is indistinguishable from a plain MBC1 by
its own header — the ROM's own readme says so: *"MBC1 multicarts cannot be detected from the
header alone."* We require exactly 1 MiB **and** a second Nintendo header at `$40104` (bank
`$10`), which is Pan Docs' own identification rule, with `GB_MBC1_MULTICART=1|0` to force it
either way.

**The false-positive direction is the dangerous one:** misreading an ordinary 1 MiB cart as a
multicart moves BANK2 down two bits and strands half the ROM. That is why the heuristic is
narrow and both directions are unit-tested.

*Symptom without it:* a compilation cartridge (*Bomberman Collection*, *Mortal Kombat I & II*,
*Genjin Collection*, *Super Chinese 1・2・3*) boots its menu, and picking any game but the first
runs the wrong one or garbage.

---

## The MBC3 clock runs on **emulated** time

`Rtc::tick` ([`src/mbc/rtc.rs:138`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/rtc.rs#L138)) counts T-cycles of *console* time
into a sub-second divider and ticks once every `CYCLES_PER_SECOND` (4,194,304) of them — fed
from a figure already divided by the CGB speed multiplier, because **the crystal is on the
cartridge and does not know the console changed gear**.

The old code derived every register from the host's wall clock, so the in-game clock ran at
whatever multiple of real time the emulator did (60× too fast headless, frozen while paused),
there was no sub-second phase at all, and it *panicked* if the host clock predated the UNIX
epoch. **The host clock is now used for exactly one thing: the gap between a save being written
and reloaded.**

Three behaviours that only a counter model can express, each an ax6 `rtc3test` sub-test:

| Behaviour | |
|---|---|
| **The registers are counters, not a time.** Seconds/minutes are 6-bit, hours 5-bit, days 9-bit. Each carries into the next **only** on rolling over from its natural maximum (59/59/23); a value loaded *above* that maximum counts up to the top of its physical width and wraps to 0 **without carrying** — 62 minutes → 63 → 0, hours untouched. | the "range tests" |
| **`DH` bit 6 (HALT) freezes the sub-second divider too**, so a clock stopped 600 ms into a second and restarted later ticks 400 ms after the restart, not a full second. | "RTC off/400" |
| **Writing the seconds register restarts the divider; writing any other RTC register does not.** | "Sub-second writes", measured to 1.5 ms |

One helper (`advance_counter`, [`src/mbc/rtc.rs:75`](https://github.com/Alchemy86/gameboy/blob/main/src/mbc/rtc.rs#L75)) implements the
carry rule for both the per-second tick and the shutdown catch-up, so the two cannot disagree.

### A withdrawn "deviation": any single write to `$6000-$7FFF` latches

This repo used to record the `$00`→`$01` sequence as the hardware rule and our any-write
behaviour as a known deviation. **It is the other way round**, and the note has been withdrawn.

CasualPokePlayer's `latch-rtc-test` writes five random register values and then **one random
byte** to `$6000` per row, and its reference dump has every row reading back what was just
written. Implementing the strict pair was tried here and turned that ROM into a screen of
zeroes — measured, FAIL; any-write restored it. Pan Docs describes the *idiom games use*, not
the decode.

**Do not "fix" it, and do not add the conformance check.** This is a worked example of why a
stale note is worth flagging: acting on it would have made a passing row fail.

**Cost:** 400 M cycles headless, min of 3 — a cartridge *without* a clock 1.526 → 1.545 s
(+1.3 %, code-layout noise), a cartridge *with* one 1.410 → 1.484 s (**+5.2 %**), still ~64×
real time.

---

## The mapper is chosen from the header, with or without a save path

`get_mbc` used to force **MBC3 for any cartridge when built without a file path** — which is
always the case for the WebAssembly build, the FFI build, and every embedder. So a
ROM-only/MBC1/MBC5 game was mis-banked from its first bank switch, in the browser, while the
identical ROM was fine on the desktop.

Persistence and mapper selection are now separate concerns: every `MBCx::new` takes an
`Option<PathBuf>`.

*Caught by:* nothing in any suite — **because every harness passes a path**, which is itself
the finding. Now `the_mapper_is_chosen_from_the_header_with_or_without_a_save_path`, proved to
fail against the old code. *Hardware:* the mapper is header byte `0x0147`, and nothing about a
save file can change it.

---

## What a wrong emulator looks like

| Symptom | Cause |
|---|---|
| Save in-game, quit, relaunch → "NEW GAME" only | persistence in a `Drop` impl that never runs |
| A save from another emulator or a real cartridge loads shifted by 8 bytes, last 8 bytes gone | an RTC header prepended to `.sav` |
| Any rumble game crashes the emulator on its first motor poke | `% rambanks` on a RAM-less MBC5+RUMBLE cart |
| A bootleg or 1.1/1.2/1.5 MiB cart crashes on its first bank switch | unofficial `0x148` ROM-size code sizing to zero banks |
| A game that crashes stops the *emulator*, and the save is gone with it | any `panic!` on a guest-controlled value under `panic = 'abort'` |
| A game crashes, is recoverable on hardware, but the save file is now corrupt | no save guard — a runaway `PC` writes to cartridge RAM and the autosave persists it |
| A compilation cartridge's menu runs the wrong game | MBC1M driven as an ordinary MBC1 |
| The in-game clock runs 60× fast headless, or freezes while paused | the RTC driven from the host's wall clock |
| A ROM-only or MBC5 game runs correctly on the desktop and produces garbage in the browser | the mapper forced to MBC3 when no save path was supplied |

### One deviation kept deliberately

**Undersized cartridge RAM does not mirror** — we allocate a full 8 KiB bank for RAM code
`0x01` (2 KiB). Not fixed, and recorded rather than hidden: mirroring is how FlashGBX recovers a
cartridge's true save size, so it is a real behaviour and a real conformance check, just not one
anything we run measures.

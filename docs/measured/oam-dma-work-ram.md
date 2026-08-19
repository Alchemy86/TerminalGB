# The address bus is shared — what a work-RAM access does under an OAM DMA

<!-- codemap:asof -->

> **Pan Docs is right, the test ROM is right, and they were never in conflict.** What was
> missing is that a CPU access to work RAM during an OAM DMA does not use the address the CPU
> asked for.

This page exists because a note in [`docs/conformance-notes.md`](../conformance-notes.md) stood
for months saying *"about 19 rows are one unexplained Color behaviour, and the obvious
explanation is ruled out"*. It is explained now, and the way it was settled is the point:
**a probe cartridge, run through two independent implementations, rather than an argument from
a wiki.**

---

## 1. The question, and why it was stuck

Gambatte's `oamdma_srcXXXX_busypushC001` family primes work RAM, starts an OAM DMA, and pushes
`$55AA` at `SP = $C001` while the transfer runs — so the CPU writes `$55` to `$C000` and `$AA`
to `$BFFF` — then reads memory back and draws the result as hex digits.

Its `cgb04c` column expects the `$55` that was written to `$C000` to be found at **`$D000`**.

The obvious reading is that the two windows alias. It was implemented, measured (it does move
rows) and **rejected**, because Pan Docs is explicit
([SVBK](https://gbdev.io/pandocs/CGB_Registers.html)) that bank 0 is always available at
`$C000-$CFFF` and that writing 0 selects bank 1. Forcing our `$D000` window onto bank 0 to make
a test pass is exactly the thing this project refuses to do. So the rows were left red and
labelled unexplained.

**Both halves of that were correct and the conclusion still cost us 60-odd rows**, because
there is a third possibility nobody had tested.

## 2. How it was settled: one probe, two implementations

A cartridge that does the sequence and nothing else
([`/tmp` generator recipe in §6](#6-reproducing-it)): set `SVBK`, prime `$C000` and `$D000`,
copy a routine into HRAM (which is on neither bus), start an OAM DMA from a chosen page, push
`$55AA` at `SP = $C001`, wait the transfer out, then read `$C000`, `$D000` in every `SVBK`
bank, cartridge RAM and OAM back into a scratch area.

It is then run twice — through this emulator with
[`examples/peek_probe.rs`](../../examples/peek_probe.rs) and through **SameBoy** with
[`tools/sameboy-oracle`](../../tools/sameboy-oracle/README.md)'s `peek` mode, which takes the
same arguments on purpose. Neither number is ours to choose: one is a foreign implementation,
and Gambatte's own expectations were verified by its author on a DMG-CPU-08 and a CPU CGB C.

**Trap worth recording: 30 frames is inside the boot ROM.** SameBoy runs real boot code, so a
short `peek` returns the same bytes for every cartridge — which reads exactly like a probe that
does not work. Use 400.

## 3. What it measured

`SVBK = 2`, `$C000` primed `$33`, `$D000` (bank 2) primed `$11`, the push writing `$55` to
`$C000`. SameBoy on `cgbC`, every source page:

| DMA source page | that address's bit 12 | which bus | `$C000` | `$D000` bank 2 |
|---|---:|---|---|---|
| `$00`, `$20`, `$40` | 0 | cartridge | **`55`** | `11` |
| `$30`, `$50`, `$7F`, `$B0`, `$F0` | 1 | cartridge | `33` | **`55`** |
| `$80` | 0 | video | `55` | `11` |
| `$90` | 1 | video | `55` | `11` |
| `$C0`, `$D0` | 0 / 1 | work RAM | `33` | `11` |

And with `SP = $D001`, so the push writes to `$D000` instead:

| DMA source page | bit 12 | `$C000` | `$D000` bank 2 |
|---|---:|---|---|
| `$00` (cartridge) | 0 | **`55`** | `11` |
| `$7F` (cartridge) | 1 | `33` | **`55`** |
| `$80` (video) | 0 | `33` | `55` |

**The rule falls straight out of it, and it is symmetric:**

> While an OAM DMA is transferring **from the external bus**, a CPU access to `$C000-$DFFF`
> (and its echo) has **address bit 12 replaced by bit 12 of the address the transfer is
> driving**. Bits 0-11 stay the CPU's, and `SVBK` still selects the bank of the upper half.

* It is **external-bus only** — a video-bus source (`$80-$9F`) does not redirect at all.
* A **work-RAM** source conflicts on its own bus, and that write is dropped before any of this
  matters; the Color console simply does not give the CPU the cycle.
* Only bit 12 moves. Measured by reading `$C000-$C00F` and `$D000-$D00F` back: the `$55` lands
  at offset 0 of the *other* half, not at the transfer's own index.

**Reads follow the identical rule.** With the transfer sourced from `$7F00`, a read of `$C000`
returns what `$D000` holds; with it sourced from `$0000`, a read of `$D000` returns what
`$C000` holds; a video-bus source changes neither.

## 4. Why Pan Docs was never wrong

The banked window is exactly where SVBK says it is. Nothing aliases. What the measurement adds
is that **the address bus is shared and only the data buses are separate**, which this
repository had already written down as a physical principle —
[`docs/conformance-notes.md`](../conformance-notes.md), 2026-08-18: *"the DMA controller has no
bus of its own"* — and had applied to the *data* half only. This is the same sentence applied
to the address half, and it is the reason a conflicting write "never reaches its own address":
part of that address is coming from somewhere else.

**A Pan Docs page that documents the register is not wrong for failing to document what happens
to the register's decode during a DMA.** The correction is ours, not theirs.

## 5. What it was worth

Implemented in
[`MemoryManagementUnit::wram_bus_address`](../../src/mmu/mod.rs), applied to both the read and
the write path, `wram_own_bus` (i.e. Color) only, and gated on `bus_tick_enabled` so a debug
peek or a plugin read is never redirected.

| suite | before | after |
|---|---:|---:|
| Gambatte, `standard` | <!--score:gambatte-->3109 / 5320<!--/score--> | **+32 rows** |
| Gambatte, `identical` | <!--score:gambatte_accurate-->3486 / 5320<!--/score--> | **+64 rows** |

Every changed row is FAIL → PASS in both modes; nothing else in either gate moved. The probe
itself now agrees with SameBoy on **8 of 8** cases where it previously differed on 5.

Not serialised: `dma_a12` is republished by `oam_dma_tick` before anything can read it, the same
argument `dma_bus`/`dma_byte`/`idu_share` already make.

## 6. Reproducing it

```sh
tools/sameboy-oracle/fetch.sh          # once, ~1 min

# build a probe: SVBK=2, prime $C000/$D000, OAM DMA from <page>, push $55AA at SP=$C001
# from HRAM, then read everything back into $C800.. — the generator is in the commit that
# added this page.
cargo build --release --example peek_probe
GB_MODEL=cgb ./target/release/examples/peek_probe probe.gbc 400 C800 C801 C802 C803

O=target/sameboy-oracle/oracle; B=target/sameboy-oracle/SameBoy/build/bin/tester
$O $B cgbC peek probe.gbc 400 C800 C801 C802 C803     # 400, not 30 — see §2
```

`examples/peek_probe.rs` takes the oracle's arguments deliberately, so the two answers can be
laid side by side without translating anything.

## 7. What is still open here

The remaining `busypush`/`busypop` reds are a different question — the prohibited block's
per-revision value (`busypushFEA1`, `busypushFF01`; see
[`docs/conformance-notes.md`](../conformance-notes.md)'s `$FEA0-$FEFF` entry, where the two
suites name different consoles) and sprite-fetch timing against a transfer ending mid-scanline
(`late_sp*`, which belongs with the PPU work). **Neither is this.**

## See also

* [the scoreboard](scoreboard.md) — every suite, with a reason per red row
* [cartridges](cartridges.md) — mappers, saves, and the class of bug that eats somebody's save
* [`tools/sameboy-oracle`](../../tools/sameboy-oracle/README.md) — asking a foreign
  implementation the same question

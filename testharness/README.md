# Test-ROM compatibility & progress

The at-a-glance scoreboard for the headless accuracy harness. It reflects the same source of
truth the CI gate uses — [`baseline.txt`](baseline.txt) — so what you read here is exactly
what the suite enforces. For *how* the harness works (serial capture, printed verdicts,
reference-image comparison, the baseline-diff CI model, the self-checks) see
[`docs/testing.md`](../docs/testing.md); for *why* each divergence exists, the per-component
pages under [`docs/components/`](../docs/components/README.md).

**Every ROM runs on both consoles.** A row is one ROM on one machine — a DMG or a Game Boy
Color — because they are different machines and a boot-register difference alone is enough to
change a verdict. A cartridge whose header says it will not run on a DMG (`0x0143 == 0xC0`)
has no DMG row.

**No expectation here is our own output.** Verdicts come from the ROM's serial string, the
text the ROM writes to cartridge RAM at `$A000`, or a reference image shipped by the ROM's
author. This used not to be true: four ROMs were pinned as an FNV hash of our own framebuffer,
which cannot fail — a regression just changes the hash, and the documented response to a
changed hash was to re-bless it.

## Progress

| Category | Passing | Notes |
|---|---:|---|
| **Blargg serial (link-port PASS/FAIL)** | **26 / 26** | 13 ROMs × 2 consoles |
| **Blargg printed verdict** | **4 / 7** | `mem_timing-2` + `halt_bug` pass on both; `interrupt_time` (CGB-only) and `oam_bug` are documented non-goals |
| **PPU reference image** | **2 / 2** | `dmg-acid2`: 0 of 23,040 pixels differ, on both consoles |

**Overall: 32 of 35 rows pass; the 3 that don't are deliberate, reasoned non-goals (not open
bugs).**

```
Blargg serial   ██████████████████████  26/26  ✅
Printed verdict ████████████░░░░░░░░░░   4/7   (3 non-goals)
PPU acid2       ██████████████████████   2/2   ✅
```

> **Read this next to the wider suites.** 18 ROMs is a narrow slice. Measured against the
> [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)'s 264 tests, with their ROMs and
> reference images, this emulator scores **92** — Mooneye acceptance 13/66, Mealybug 0/71,
> SameSuite APU 2/61, SameSuite DMA 0/4, MBC3 RTC 0/3.

## Compatibility matrix

Legend: ✅ pass · 🟩 pass (verdict read from the ROM's printed text or a pixel diff) ·
🚫 documented non-goal (pinned red on purpose — see reason).

### CPU instructions — Blargg `cpu_instrs` (serial)

All pass on **both** consoles.

| ROM | DMG | CGB | What it checks |
|---|:--:|:--:|---|
| `01-special` | ✅ | ✅ | misc special-case opcodes |
| `02-interrupts` | ✅ | ✅ | `EI`/`DI`/`RETI`, IF/IE dispatch |
| `03-op sp,hl` | ✅ | ✅ | 16-bit `SP`/`HL` ops |
| `04-op r,imm` | ✅ | ✅ | register/immediate ALU |
| `05-op rp` | ✅ | ✅ | register-pair ops |
| `06-ld r,r` | ✅ | ✅ | register-to-register loads |
| `07-jr,jp,call,ret,rst` | ✅ | ✅ | control flow |
| `08-misc instrs` | ✅ | ✅ | remaining misc opcodes |
| `09-op r,r` | ✅ | ✅ | register/register ALU |
| `10-bit ops` | ✅ | ✅ | `CB` bit/shift/rotate |
| `11-op a,(hl)` | ✅ | ✅ | ALU against `(HL)` |

### Timing & CPU quirks (Blargg)

| ROM | Verdict from | DMG | CGB | What it checks |
|---|---|:--:|:--:|---|
| `instr_timing` | serial | ✅ | ✅ | per-instruction M-cycle counts |
| `mem_timing` | serial | ✅ | ✅ | *which* M-cycle a memory read/write occurs on — fixed by stepping the timer per memory access (`MemoryManagementUnit::bus_tick`); [CPU §4](../docs/components/cpu.md), [Timer §4](../docs/components/timer.md) |
| `mem_timing-2` | cart RAM `$A000` | 🟩 | 🟩 | same, prints `01:ok 02:ok 03:ok Passed` |
| `halt_bug` | screen | 🟩 | 🟩 | `HALT` with IME=0 + pending IRQ re-reads the next byte — HALT bug modelled (`halt_bug` flag + PC rewind); [CPU §5](../docs/components/cpu.md) |
| `interrupt_time` | screen | — | 🚫 | 16-vs-20 T-cycle interrupt dispatch. Prints `Failed`. CGB-only cartridge (`0x143 == 0xC0`), so no DMG row |

### PPU / MMU

| ROM | Verdict from | DMG | CGB | What it checks |
|---|---|:--:|:--:|---|
| `dmg-acid2` | reference image | 🟩 | 🟩 | BG/window/sprite composition (the "face" test) — **0 of 23,040** pixels differ from `third_party/dmg-acid2/reference-dmg.png`, compared by shade rank |
| `oam_bug` | cart RAM `$A000` | 🚫 | 🚫 | DMG OAM-corruption hardware defect. Prints `01:ok 02:02 03:ok 04:03 05:02 06:ok 07:01 08:02 … Failed #2` — **3 of 8 sub-tests pass** |

## Documented non-goals (pinned red on purpose)

These stay red in `baseline.txt` by intent, so a *change* still trips CI, but they are not
counted as open defects. Each is recorded on its component page.

- **`interrupt_time`** — it runs to completion and prints **`Failed`**. (It used to be
  recorded as "cannot run headless"; the real cause was that the harness installed **no APU**,
  and the ROM's `sync_apu` / `get_cpu_speed` poll `NR52`. Every machine the harness boots now
  has an APU, so this is a measured failure.) Passing needs cycle-accurate interrupt dispatch
  at *both* speeds — the 16→20 T-cycle change — which is a structural cycle-accuracy
  commitment. ([CPU §4/§7](../docs/components/cpu.md).)
- **`oam_bug`** — the DMG OAM-corruption bug is a hardware *defect*; essentially no commercial
  game relies on it, and modelling it needs mode-2 OAM bus tracking for ~zero practical gain.
  Now quantified rather than opaque: sub-tests 1, 3 and 6 pass; 2, 4, 5, 7 and 8 fail.
  ([MMU §5](../docs/components/mmu.md).)

Related structural approximations: PPU **mode-3 length is fixed** and **OAM DMA is
instant/unguarded** — both need a cycle-stepped PPU/CPU sharing the bus dot-by-dot. There
*are* wired tests for these outside this suite (Mealybug Tearoom 0/71, SameSuite DMA 0/4,
Mooneye `acceptance/ppu` and `oam_dma`); they are simply not in this harness yet. The timer's
separate DIV/TIMA counters are likewise tested by Mooneye `acceptance/timer`, 10 of 13 red.
See [PPU](../docs/components/ppu.md), [MMU](../docs/components/mmu.md) and
[Timer](../docs/components/timer.md).

## Running it

```sh
# Run the whole matrix (release; 2.9 s measured). Audio is off by default, so no ALSA
# needed — the harness installs a device-free APU itself, which costs +0.02 ms per
# emulated frame (1.1x) against a 16.743 ms budget. That is NOT the ~17.9 ms/frame the
# `audio` cargo feature currently costs: cpal and the producer-side queue throttle are
# not compiled here at all. See docs/testing.md.
cargo test --release --test testroms

# Re-bless the baseline after a deliberate, reviewed change:
UPDATE_BASELINE=1 cargo test --release --test testroms

# Dump every ROM's final frame to target/testrom-artifacts/<id>__<model>.png:
DUMP_FRAMES=1 cargo test --release --test testroms
```

Before re-blessing any moved baseline, rule out the console first —
`GB_MODEL=cgb cargo test --release --test audiocapture` forces the pre-DMG-mode machine, and
if the old hashes come back byte-for-byte the move is the console, not a regression.

CI is **green when reality matches `baseline.txt`** and red on **any** change, in either
direction. Two startup self-checks run first, so a green result means something: the reference
image must decode, compare equal to itself, and report a difference when a single pixel is
flipped; and the same ROM must render byte-identically twice in one process. See
[`docs/testing.md`](../docs/testing.md) for the full rationale.

<!-- Keep this table in sync with testharness/baseline.txt whenever a ROM's status changes. -->

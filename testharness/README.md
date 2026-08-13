# Test-ROM compatibility & progress

The at-a-glance scoreboard for the headless accuracy harness. It is generated from the
same source of truth the CI gate uses — [`baseline.txt`](baseline.txt) — so what you read
here is exactly what the suite enforces. For *how* the harness works (serial capture vs
frame-hash, the baseline-diff CI model, reproducibility self-check) see
[`docs/testing.md`](../docs/testing.md); for *why* each divergence exists, the per-component
pages under [`docs/components/`](../docs/components/README.md).

## Progress

| Category | Passing | Notes |
|---|---:|---|
| **Blargg serial (objective PASS/FAIL)** | **13 / 13** | every serial ROM the harness runs |
| **Blargg screen-only (frame hash)** | **2 / 4** | `mem_timing-2` + `halt_bug` pass; `interrupt_time` + `oam_bug` are documented non-goals |
| **PPU pixel test** | **1 / 1** | `dmg-acid2` renders pixel-correct vs the reference |

**Overall: 16 of 18 wired ROMs pass; the 2 that don't are deliberate, reasoned non-goals
(not open bugs).** Recent movement: `mem_timing` **FAIL → PASS** and `halt_bug`
**FAIL → PASS** (serial suite 12/13 → **13/13**).

```
Blargg serial   ██████████████████████  13/13  ✅
Frame-hash      ███████████░░░░░░░░░░░░   2/4   (2 non-goals)
PPU acid2       ██████████████████████   1/1   ✅
```

## Compatibility matrix

Legend: ✅ pass · 🟩 pass (verdict read from the rendered screen / pixel diff) ·
🚫 documented non-goal (pinned red on purpose — see reason) · verdict source in the last
column.

### CPU instructions — Blargg `cpu_instrs` (serial)

| ROM | Status | What it checks |
|---|:--:|---|
| `01-special` | ✅ | misc special-case opcodes |
| `02-interrupts` | ✅ | `EI`/`DI`/`RETI`, IF/IE dispatch |
| `03-op sp,hl` | ✅ | 16-bit `SP`/`HL` ops |
| `04-op r,imm` | ✅ | register/immediate ALU |
| `05-op rp` | ✅ | register-pair ops |
| `06-ld r,r` | ✅ | register-to-register loads |
| `07-jr,jp,call,ret,rst` | ✅ | control flow |
| `08-misc instrs` | ✅ | remaining misc opcodes |
| `09-op r,r` | ✅ | register/register ALU |
| `10-bit ops` | ✅ | `CB` bit/shift/rotate |
| `11-op a,(hl)` | ✅ | ALU against `(HL)` |

### Timing & CPU quirks (Blargg)

| ROM | Kind | Status | What it checks | How the fix landed |
|---|---|:--:|---|---|
| `instr_timing` | serial | ✅ | per-instruction M-cycle counts | (already passing) |
| `mem_timing` | serial | ✅ | *which* M-cycle a memory read/write occurs on | **timer stepped per memory access** (`MemoryManagementUnit::bus_tick`) — [CPU §4](../docs/components/cpu.md), [Timer §4](../docs/components/timer.md) |
| `mem_timing-2` | frame | 🟩 | same, screen-reported (`01:ok 02:ok 03:ok Passed`) | fixed by the same per-access timer work |
| `halt_bug` | frame | 🟩 | `HALT` with IME=0 + pending IRQ re-reads the next byte | **HALT bug modelled** (`halt_bug` flag + PC rewind) — [CPU §5](../docs/components/cpu.md) |
| `interrupt_time` | frame | 🚫 | 16-vs-20 T-cycle interrupt dispatch | **non-goal** — see below |

### PPU / MMU (frame hash + pixel diff)

| ROM | Status | What it checks | Detail |
|---|:--:|---|---|
| `dmg-acid2` | 🟩 | BG/window/sprite composition (the "face" test) | **zero** differing pixels vs `third_party/dmg-acid2/reference-dmg.png` across the 4 gray levels |
| `oam_bug` | 🚫 | DMG OAM-corruption hardware defect | **non-goal** — see below |

## Documented non-goals (pinned red on purpose)

These stay red in `baseline.txt` by intent, so a *change* still trips CI, but they are not
counted as open defects. Each is recorded on its component page.

- **`interrupt_time`** — **cannot run headless.** It is `REQUIRE_CGB`, switches to
  double speed, and calls `sync_apu` / `get_cpu_speed`, which poll `NR52`. With host audio
  off by default the APU length/status still runs, but the test spins waiting on a channel
  state it never reaches, so it hangs on its title screen before any timing is measured.
  Even if unblocked, passing needs cycle-accurate interrupt dispatch at *both* speeds — a
  structural cycle-accuracy commitment — and no other wired ROM proves the 16→20 T-cycle
  change, so it is not made blind. ([CPU §4/§7](../docs/components/cpu.md).)
- **`oam_bug`** — the DMG OAM-corruption bug is a hardware *defect*; the emulator boots in
  CGB mode where it does not apply, essentially no commercial game relies on it, and
  modelling it needs mode-2 OAM bus tracking for ~zero practical gain.
  ([MMU §5](../docs/components/mmu.md).)

Related structural approximations with no wired test (not in the matrix): PPU **mode-3
length is fixed** and **OAM DMA is instant/unguarded** — both need a cycle-stepped PPU/CPU
sharing the bus dot-by-dot. See [PPU](../docs/components/ppu.md) and
[MMU](../docs/components/mmu.md).

## Running it

```sh
# Run the whole matrix (release; ~2 s). Audio is off by default, so no ALSA needed.
cargo test --release --test testroms

# Re-bless the baseline after a deliberate, reviewed change:
UPDATE_BASELINE=1 cargo test --release --test testroms

# Dump every ROM's final frame to target/testrom-artifacts/<id>.png:
DUMP_FRAMES=1 cargo test --release --test testroms
```

CI is **green when reality matches `baseline.txt`** and red only when a result *changes* —
a regression (was passing, now fails) or an improvement (bless it). See
[`docs/testing.md`](../docs/testing.md) for the full rationale.

<!-- Keep this table in sync with testharness/baseline.txt whenever a ROM's status changes. -->

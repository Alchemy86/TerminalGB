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
| **Blargg printed verdict** | **6 / 7** | `mem_timing-2` + `halt_bug` on both, `interrupt_time` (CGB-only), `oam_bug@dmg` **8 of 8 sub-tests**; `oam_bug@cgb` is the one deliberate red |
| **Blargg `cgb_sound`** | **12 / 12** | CGB-only cartridges, so one row each |
| **PPU reference image** | **2 / 2** | `dmg-acid2`: 0 of 23,040 pixels differ, on both consoles |

**Overall: 46 of 47 rows pass.** The one red row — `oam_bug@cgb` — is deliberate and must
stay red: the OAM-corruption bug is DMG/MGB/SGB silicon only, so a real Game Boy Color fails
that ROM too.

```
Blargg serial   ██████████████████████  26/26  ✅
Printed verdict ███████████████████░░░   6/7   (1 by design)
cgb_sound       ██████████████████████  12/12  ✅
PPU acid2       ██████████████████████   2/2   ✅
```

> **Read this next to the wider suites.** 30 ROMs is still a narrow slice, and the wider
> suites are now gated too — `make test-external` scores Mooneye GB, SameSuite and the
> Mealybug Tearoom tests against their own baselines on every push. Measured 2026-08-14:
> Mooneye acceptance **67/75** (the community's 66-row denominator drops the nine
> named-machine ROMs; all nine pass here, so quote 67/75), Mooneye `emulator-only` **28/28**,
> Mooneye `misc` **8/8**, SameSuite **72/76**, Mealybug **3/79**. See
> [docs/testing.md](../docs/testing.md#gating-the-external-suites--mooneye-gb-samesuite-mealybug-tearoom-cpp).

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
| `interrupt_time` | screen | — | 🟩 | interrupt dispatch time at both CPU speeds. Prints `… 08 0D 01 … Passed` — 13 cycles, the ROM's own stated answer. CGB-only cartridge (`0x143 == 0xC0`), so no DMG row |

### PPU / MMU

| ROM | Verdict from | DMG | CGB | What it checks |
|---|---|:--:|:--:|---|
| `dmg-acid2` | reference image | 🟩 | 🟩 | BG/window/sprite composition (the "face" test) — **0 of 23,040** pixels differ from `third_party/dmg-acid2/reference-dmg.png`, compared by shade rank |
| `oam_bug` | cart RAM `$A000` | 🟩 | 🚫 | DMG OAM-corruption hardware defect — modelled, so `@dmg` prints `01:ok … 08:ok Passed`, **8 of 8**. `@cgb` is pinned red on purpose: Color silicon has no such defect, so a real CGB fails this ROM too |

### Sound — Blargg `cgb_sound` (printed verdict, CGB-only cartridges)

Wired 2026-08-14. All twelve pass. Every one of these cartridges says `0x0143 == 0xC0`, so
each is one row, not two. The DMG half of Blargg's APU suite (`dmg_sound`, also 12/12) is
scored by [`audiocapture.rs`](audiocapture.rs), which additionally listens to the samples the
APU produces; wiring the Color half here is what stops those twelve from regressing unseen.

| ROM | CGB | What it checks |
|---|:--:|---|
| `01-registers` | 🟩 | register read-back masks |
| `02-len ctr` | 🟩 | length counters |
| `03-trigger` | 🟩 | channel trigger |
| `04-sweep` | 🟩 | CH1 sweep |
| `05-sweep details` | 🟩 | CH1 sweep quirks |
| `06-overflow on trigger` | 🟩 | sweep overflow disables the channel |
| `07-len sweep period sync` | 🟩 | frame-sequencer phase |
| `08-len ctr during power` | 🟩 | length counters across APU power — a CGB clears them, a DMG does not |
| `09-wave read while on` | 🟩 | wave-RAM access window (a CGB reads the current byte) |
| `10-wave trigger while on` | 🟩 | wave-RAM corruption on trigger (DMG only) |
| `11-regs after power` | 🟩 | register state after an APU power cycle |
| `12-wave` | 🟩 | CH3 wave playback |

## The external suites live beside this one

Mooneye GB, SameSuite and the Mealybug Tearoom tests are **not** in this harness — their
ROMs are not ours to vendor, so they are fetched at run time and gated separately by
`make test-external`, against `mooneye_baseline.txt`, `mooneye_cgb_baseline.txt`,
`samesuite_baseline.txt` and `mealybug_baseline.txt` in this directory. Same rules: the
ROM's own verdict, red on any change in either direction, deliberate re-blessing
(`UPDATE_EXTERNAL_BASELINE=1`). See
[docs/testing.md](../docs/testing.md#gating-the-external-suites--mooneye-gb-samesuite-mealybug-tearoom-cpp).

**The thirteen c-sp suites are gated the same way**, one `csp_<suite>_baseline.txt` per suite
here plus a `_accurate_` twin for the per-dot picture engine — AGE, Bully, cgb-acid2,
cgb-acid-hell, Gambatte, GBMicrotest, little-things-gb, MBC3 Tester, the wilbertpol Mooneye
fork, rtc3test, Scribbltests, Strikethrough and TurtleTests. `csp_gambatte_baseline.txt` alone
is 5,225 scored rows, so **expect any PPU or boot-state change to produce a large but readable
diff there**: that is the point, and the same trade the Mealybug baselines make with their
pixel counts. Current scores per suite, both engines:
[docs/measured/scoreboard.md](../docs/measured/scoreboard.md#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time).

## Documented non-goals (pinned red on purpose)

These stay red in `baseline.txt` by intent, so a *change* still trips CI, but they are not
counted as open defects. Each is recorded on its component page.

- **`oam_bug@cgb`** — the DMG OAM-corruption bug is a defect of DMG/MGB/SGB silicon that
  Game Boy Color hardware does not have, even running a monochrome cartridge (Pan Docs, "OAM
  Corruption Bug"). A real CGB fails this ROM, so turning the row green would mean emulating
  a machine nobody has. The `@dmg` row passes all eight sub-tests.
  ([MMU §5](../docs/components/mmu.md).)

The remaining structural approximation is the fixed mode-3 length of the **default `standard`
[picture mode](../docs/picture-modes.md)**. A cycle-stepped PPU sharing the bus dot by dot
exists beside it as the selectable `identical` mode —
[`src/gpu/fifo.rs`](../src/gpu/fifo.rs), `GB_PPU=identical` — and derives mode 3's length
instead of assuming it: Mealybug is **3/79** in `standard` and **12/79** in `identical`.
Those tests live outside this harness and are gated by `make test-external` (`standard`)
and `make test-external-accurate` (`identical`), both pinned with pixel counts so a rewrite's
progress *and* its regressions are visible. The two modes are held against each other
hermetically by [`ppuengines.rs`](ppuengines.rs), which also re-measures the ~2.1x cost figure
the product quotes. See [PPU](../docs/components/ppu.md),
[MMU](../docs/components/mmu.md) and [mealybug.md §7](../docs/mealybug.md).

## Running it

```sh
# Run the whole matrix (release; 3.6 s measured, of which the twelve cgb_sound rows are
# ~0.8 s). Audio is off by default, so no ALSA
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

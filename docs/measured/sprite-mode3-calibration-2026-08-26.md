# The sprite half of the mid-scanline wall, calibrated to the dot

> **PARTLY SUPERSEDED, 2026-08-27.** §3 and §5's "parity split" — stems needing `+2` and
> `+4`, and a constant three dots no integer shift could express — was an artefact of the
> probe used here, which could find the mode 3 → 0 boundary on a *different line* from the
> one the ROM measures. Scored by the ROMs' own verdicts instead, the structure is a single
> sharp optimum at a three-dot mode-3 tail (104 of 113 pairs fixed, none made worse), and it
> is rejected for a different and stronger reason. See
> [`mode3-phase-2026-08-27.md`](mode3-phase-2026-08-27.md) §0. Everything else on this page
> — the instrument, the OBJ penalty's exoneration, the model table — stands.

Measured 2026-08-26 on `e4edb9a`. **No emulator change is landed here** — `src/` is
byte-identical to the branch point, and both picture engines cost exactly what they cost
before. What *is* landed is the calibration this wall had never had, and it changes the
diagnosis: **our OBJ penalty formula is correct, and what is wrong is a three-dot offset
between the PPU's mode cycle and `LY`.** Seven models were built and measured against
5,225 Gambatte rows with Mooneye and Mealybug as controls; all seven trade rows, and §6
shows why none of them can be right.

## 1. The instrument nobody had used: 113 one-M-cycle boundary pairs

`gambatte/sprites/space/` is 226 ROMs — **113 pairs**, each pair one byte of padding apart:

```
$ diff …nr10space10_m3stat_ds_1….asm …nr10space10_m3stat_ds_2….asm
100c100
< .text@10a9
---
> .text@10aa
```

Each pair sets ten sprites, takes an `LY == LYC` STAT interrupt, runs a `nop` sled, and
reads `STAT & 3`. Variant 1 must still see mode **3**; variant 2, one M-cycle later, must
see mode **0**. So each pair brackets the mode 3 → 0 boundary to **one M-cycle — two dots,
because every ROM in this directory is double speed.**

The family is a *sweep*, not a set of cases. `nr10spaceN` varies only the tenth sprite's
`X` (`$51`…`$5d`) and moves the read to track the boundary; `scxN` sweeps `SCX` 1-7;
`wxN` sweeps the window; `NoverlapM` sweeps overlapping sprites.

**Baseline: 113 pairs, 105 fail variant 1 and 8 fail variant 2** — a systematic, one-sided
error. Our mode 3 ends too early, in 105 of 113 configurations.

Scored with a probe reporting the dot of the ROM's own `STAT` read and our own mode 3 → 0
boundary. The target is `boundary − read_dot == 6`:

```
stems measured 96   EXACT(=6): 0   off: 96
histogram of (boundary − read_dot): {2: 63, 4: 33}
```

**A trap in reading that probe:** `bus_tick` advances the PPU one M-cycle *before* each
access, so the value `ldff a,(c)` latches is the PPU at `read_dot + 4`, not at `read_dot`.
And a probe that takes the first mode 3 → 0 after enabling OBJ can land on the **LCD-restart
line**, which has no OAM scan at all (`skip_first_oam`) and therefore no sprites — that
reads as a 176-dot mode 3 and is not the line the ROM measures.

## 2. Our OBJ penalty formula is CORRECT — twice over

`Gpu::start_obj_fetch` charges six dots per object plus `max(0, 5 − ((X + SCX) mod 8))` for
the first object on each background tile — Pan Docs' algorithm. Two independent checks say
the *formula* is right:

**Mooneye's `intr_2_mode0_timing_sprites` matches it exactly, on all 39 arrangements.**
Reading `intr_2_mode0_timing_sprites.s` rather than trusting the ROM: it is a straddle of
the same kind, `41 + N` nops must already be in mode 0 and `40 + N` must not. Every
arrangement checks out against the formula, including the ones that matter most here:

| arrangement | expected | formula |
|---|---:|---:|
| 10 sprites 8 apart from X0=0 (**10 distinct tiles**) | 27 | 60 + 10×5 = 110 → 27 ✓ |
| …from X0=1 | 25 | 60 + 10×4 = 100 → 25 ✓ |
| …from X0=2 | 22 | 60 + 10×3 = 90 → 22 ✓ |
| …from X0=4 | 17 | 60 + 10×1 = 70 → 17 ✓ |
| 10 sprites all at X=0 (**one tile**) | 16 | 60 + 5 = 65 → 16 ✓ |
| 5 at X=0 + 5 at X=160 (**two tiles**) | 17 | 60 + 5 + 5 = 70 → 17 ✓ |
| 1 sprite at X=8 | 2 | 6 + 5 = 11 → 2 ✓ |

**And Gambatte's own `nr10space` sweep reproduces the same shape.** Reading the boundary out
of each variant's read position gives, relative to `X mod 8 = 7`, a curve every one of whose
windows contains `max(0, 5 − (X mod 8))`.

So the penalty is not the defect. Confirmed a third way by experiment: adding one dot per
object moved the boundary by **ten** dots on a ten-sprite line (`{12: 83, 14: 13}`), which is
what a per-object error looks like and is not what we have.

## 3. The defect is a THREE-DOT offset, and it is the anchor that separates the suites

Solving the measured windows for a base `C`: our true penalty is `C + p` with **C = 106**,
and hardware's is `C_hw + p` with **C_hw = 109**, using the same `p` — every one of the
eight `X mod 8` cases fits. **A constant three dots.** The observed shortfall alternates
2/4 only because the measurement resolves two dots and 3 is odd.

**Why Mooneye and Mealybug cannot see it, and this is the useful part.** Mooneye's sprite
test waits for **mode 2** and measures an *interval*; Mealybug's ROMs arm on the **mode-2**
STAT source and measure the fetcher. Shift the whole mode cycle relative to `LY` and both
ends of a mode-2-anchored measurement move together — the interval is unchanged and the test
still passes. Gambatte's ROMs anchor on the **LYC interrupt**, which is `LY`, so they see it.

That is measured, not argued: across *every* experiment below that shifted this phase,
**Mooneye stayed 103/103 and Mealybug stayed at 29 pixel-exact rows, to the row and to the
pixel.** Their being green was never evidence that this phase is right.

This is the same axis as
[`stat-line-phase-2026-08-25.md`](stat-line-phase-2026-08-25.md), reached from a completely
independent family — that page's ladder is STAT-interrupt tests, this one is sprites.

## 4. Seven models, every one measured

Baselines: Gambatte **3622**/5225, Mooneye **103**/103, Mealybug **29** pixel-exact,
SameSuite 73/76. All are the `identical` engine; `standard` is untouched by every
experiment, since all of this lives in `src/gpu/fifo.rs`.

| # | model | Gambatte | Mooneye | Mealybug | verdict |
|---|---|---:|---:|---:|---|
| A | +1 dot per object (6 exclusive fetch dots) | — | — | — | boundary moved **+10** on a 10-sprite line; per-object is the wrong axis |
| B | +3 dots at the **first** object of the line | 3715 (+188/−95) | 102 | **21** | delays the background fetcher for the rest of the line: 76 losses in `bgtiledata`/`bgtilemap`/`scy`, and it costs 8 Mealybug rows |
| C | +3 dots held at the **end** of mode 3 | 3798 (+184/−8) | 102 | 29 | fits **all 96 stems exactly**; Mealybug untouched; but it lengthens mode 3, which Mooneye's `intr_2_mode0_timing_sprites` forbids (`TEST #00`) |
| D | as C, armed by `LCDC.1` at mode-3 start | 3798 | 102 | 29 | identical to C |
| F | as C, armed by live `LCDC.1` at mode-3 end | 3798 | 102 | 29 | identical to C |
| G | `LY` leads the mode cycle's wrap by 2 dots | 3854 (+375/−143) | **103** | **29** | controls invariant, as predicted — but 141 genuine regressions, and SameSuite's `dma/hdma_mode0` |
| G′ | as G, VBlank kept on the mode cycle | **3864** (+387/−145) | **103** | **29** | the best score of any model, and still 145 losses |
| H | 2-dot lead on the LYC **interrupt** term only | 3739 (+199/−82) | 103 | 29 | fewer losses, fewer gains; still trades |
| J | G′, double speed only | 3733 (+192/−81) | 103 | 29 | touches 272 `_ds` rows and 1 single-speed row; still trades |
| K | J with a 1-dot lead | 3674 (+66/−14) | 103 | 29 | smallest trade; still not clean |

## 5. The `+3` is not reachable, and that is arithmetic

Two facts, both measured:

1. **A lead of `L` dots moves the boundary by `2L`.** Checked against individual stems:
   `nr10space11` (baseline `boundary − read_dot` = 2) goes to 6 under `L = 2`, and
   `nr10space10` (baseline 4) goes to **8** — overshooting the target of 6 and turning a
   passing variant into a failing one. Under `L = 1` the two swap roles.
2. **The 113 stems split into two groups needing `+2` and `+4`**, and the split is exactly
   the parity of the OBJ penalty: `X mod 8 ∈ {0,2,4}` gives an odd penalty and needs `+2`;
   everything else needs `+4`.

So no single even shift satisfies both groups, and the odd shift they jointly imply — the
three dots of §3 — is not expressible as an integer lead. The only model that *did* fit all
96 stems (C, a three-dot tail on mode 3) does so by lengthening mode 3, which Mooneye's
39-arrangement table forbids outright.

**That is the wall, stated precisely.** It is not "mid-scanline timing is hard": it is a
three-dot offset between the mode cycle and `LY`, pinned from both sides by two
hardware-verified suites, that the current one-dot-granular fetcher cannot express without
moving something else. `docs/mealybug.md` §8.9 budgets the per-tile fetcher calibration this
needs in weeks, and that estimate now has a number attached to it.

## 6. What this cost, and what it did not

`src/` is byte-identical to `e4edb9a`, so both engines cost exactly what they did before.
Measured on the clean tree with the repository's own gate (`cargo test --release --test
ppuengines`, dmg-acid2):

```
cost  fast 0.0561 ms/frame (298.4x realtime)   accurate 0.4542 ms/frame (36.9x realtime)
```

An earlier run of the identical binary on the same box, under load from another crew, read
`fast 0.0707 / accurate 0.4954`. **Quote the load average with any figure from this box** —
the spread between those two readings is 26 %, larger than most changes worth measuring.
Both engines remain pixel-exact on dmg-acid2 and agree with each other.

Had any of §4's models landed it would have gone **behind the `identical` engine only** —
every one of them is in `src/gpu/fifo.rs`, which `standard` never executes — and none adds
per-frame work to the shared path.

## 7. GBSelfTest: not checkable, and why

Nothing here is checkable from inside a running cartridge, and the reason is the same one
that makes the defect hard: **a cartridge cannot measure the mode cycle's phase against `LY`
except by taking an `LY`-anchored interrupt and reading `STAT` — which is exactly what
Gambatte's 226 ROMs already do.** A GBSelfTest check would be a reimplementation of them
with an expectation this work could not settle, and asserting a number chosen to match our
own output is inventing ground truth. The honest position is that the corpus already has the
instrument; what it lacks is the model.

`GB-PPU-16` stays correctly red and unaffected — it is the DMG STAT-write glitch, ruled out
as a cause of this in `stat-line-phase-2026-08-25.md` §1.

## Reproducing

```
# the baseline structure: 105 of 113 pairs fail their first variant
grep '^sprites/space/' testharness/csp_gambatte_accurate_baseline.txt | grep -c FAIL   # 113

# Mooneye's own table, which is the control
git clone --depth 1 https://github.com/Gekkio/mooneye-test-suite
sed -n '70,200p' mooneye-test-suite/acceptance/ppu/intr_2_mode0_timing_sprites.s

# Gambatte's sources are not in the c-sp bundle, which ships ROMs only
git clone --depth 1 https://github.com/pokemon-speedrunning/gambatte-core
```

Both are read as **test specifications** — facts about what each ROM measures — never as
emulator code.

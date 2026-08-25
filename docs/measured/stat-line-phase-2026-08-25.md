# The two picture engines BRACKET hardware on the STAT line's phase

Measured 2026-08-25 on `c575bb4`. **No fix is landed here.** What is landed is the
measurement the previous attempt at this area explicitly asked for, and a model to replace
the sweep it warned against — `docs/conformance-notes.md` (2026-08-22) ends:

> score both models against Gambatte's `miscmstatirq` group **row by row against the ROM's
> own `<old>_<new>` filename encoding** … A third model … should be checked against that
> row-by-row table **before** being built, not after.

That is what this page is. It also **rules the DMG STAT-write glitch out** as the cause of
the largest failing cluster, which the previous attempt could not do.

## 1. The `<old>_<new>` truth table says the DMG glitch is not what is failing

`gambatte/miscmstatirq/lycstatwirq_trigger_<old>_<new>` is a complete 4×4 table over
`STAT` values `$00`, `$40` (LYC select only), `$bf` (modes 0/1/2, **no** LYC) and `$ff`.
The scenario, read from `gambatte-core`'s own `.asm`: take an `LY == LYC` STAT interrupt,
write `STAT = old`, clear `IF`, write `STAT = new`, read `IF & 3`.

Hardware's DMG column depends **only on `old`**: an interrupt fires iff `old` has bit 6
clear. That is ordinary edge detection — if the LYC source was already selected while
`LY == LYC`, the line was already high, so the glitch pulse produces no *edge*. And
`old = $bf` firing proves the PPU is in **mode 3** at that instant, because `$bf` selects
modes 0/1/2 and the line still has to be low.

Our results against that table:

| | DMG | CGB |
|---|---|---|
| `00_00`, `bf_00`, `bf_bf` | **FAIL** (want an interrupt) | pass |
| `bf_40`, `bf_ff` | **FAIL** | **FAIL** |
| `00_bf` | pass | **FAIL** |
| the other ten | pass | pass |

The rows that fail **on both consoles** are the finding. The DMG STAT-write glitch is
DMG-only silicon, so a row failing identically on a Color console cannot be that glitch —
it is ordinary edge detection reaching a different answer because **we are in the wrong
PPU mode at the moment of the write**. Three rows want mode 3 where we report mode 0/2.

## 2. The calibration ladder measures the error exactly: 2 M-cycles

`lycstatwirq_trigger_m0_early_ly44_lyc44_08_40_<1..9>` is the same program nine times with
**one byte of padding** between variants — one `nop`, one M-cycle. Confirmed by diffing the
sources: the only difference is `.text@102c` versus `.text@102d`. It sweeps a
`STAT = $40` write across the mode 3 → mode 0 transition; the earlier variants see the
line still low (interrupt, `outE2`), the later ones see it already high (`outE0`).

| variant | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 |
|---|---|---|---|---|---|---|---|---|---|
| **hardware** | E2 | E2 | E2 | E2 | E2 | E2 | E2 | E2 | **E0** |
| `standard` | E2 | E2 | E2 | E2 | E2 | E2 | E2 | E2 | **E2** ✗ |
| `identical` | E2 | E2 | E2 | E2 | E2 | E2 | **E0** ✗ | **E0** ✗ | E0 |

**Hardware flips between 8 and 9. `identical` flips two M-cycles early, between 6 and 7.
`standard` never flips inside the ladder at all — it is at least one M-cycle late.** The
two engines bracket the truth.

Probing the emulator directly at the write (`Gpu::modeclock`, GPU flushed first) confirms
the mechanism rather than inferring it — on `identical`, line 68:

```
variant 4  mode=3 dot=248  stat_line=false
variant 5  mode=3 dot=252  stat_line=true     <- our mode-0 source rises here
variant 6  mode=0 dot=256
```

Our mode-0 STAT source rises at dot 252 = 80 + 172, the canonical minimum mode 3 with
`SCX = 0`, sprites off (`LCDC = $91` from boot: bit 1 clear) and the window off. Hardware's
must be at ~260.

## 3. Why no existing passing test contradicts this — and what the model is

`acceptance/ppu/intr_2_0_timing` and `intr_2_mode0_timing` pass, and they pin mode 3's
length. They are not in conflict, because **they are anchored on the mode-2 interrupt**:
they measure the *interval* from the mode-2 strobe to the mode-0 strobe. Shift the PPU's
whole mode cycle relative to `LY` and both ends move together, so the interval is
unchanged and the test still passes. Only a probe anchored on `LY` — which the
`lycstatwirq` family is, and Mooneye has none of — can see it.

**The model this points to: `LY` increments ~8 dots before the PPU's mode-2 window
begins, rather than simultaneously with it.** Everything measured is consistent with it:
the mode-2/mode-0 interval is untouched (Mooneye stays green), while every `LY`-anchored
probe finds mode 0 arriving two M-cycles later than we place it.

This is deliberately **not implemented**. It restructures where a scanline begins relative
to `LY`, it would move hundreds of rows in both directions, and `AGENTS.md` is explicit
that a constant fitted to one suite without a model is how this area goes wrong. It is
also very likely the same axis as the `boot_ppu_dot` sweep that
`mmu::boot_ppu_dot`'s own comment calls "a jagged profile with no clean optimum, i.e. a
constant absorbing an error somewhere else" — a single constant cannot fit two independent
offsets, which is exactly why that sweep had no optimum.

## 4. The corroborating population: 459 rows where the DEFAULT engine loses

`identical` has been the default since 2026-08-20 and it does dominate — but nobody had
enumerated where it does not:

```
gambatte     accurate LOSES 446 rows that standard passes   (and WINS 892: net +446)
gbmicrotest  accurate LOSES  13
```

The losses fall in the *same groups* as the wins (`window` 107 lost / 137 won, `lycEnable`
34 / 49, `m1` 33 / 54, `m0enable` 16 / 46) — a phase difference, not a missing feature.

The gbmicrotest thirteen are off-by-one with one engine right — but **§6 shows they are
resolving `DIV`, not the interrupt line, so read them as a separate question rather than
as corroboration of §2**:

| row | `standard` | `identical` |
|---|---|---|
| `lyc1_int_halt_a@dmg` | PASS | FAIL got=02 want=01 |
| `lyc1_int_nops_a@dmg` | PASS | FAIL got=02 want=01 |
| `lyc2_int_halt_a@dmg` | PASS | FAIL got=02 want=01 |
| `lyc_int_halt_a@dmg` | PASS | FAIL got=05 want=04 |
| `oam_int_if_edge_a@dmg` | PASS | FAIL got=E2 want=E0 |
| `oam_int_if_edge_c@dmg` | PASS | FAIL got=E0 want=**E2** |
| `lcdon_to_if_oam_a@dmg`, `line_144_oam_int_d@dmg`, `lyc1_write_timing_b@dmg` | PASS | FAIL |
| `poweron_{oam_119,oam_233,vram_025,vram_139}@dmg` | PASS | FAIL |

`oam_int_if_edge_a` and `_c` are a **mirror pair** — one wants the flag set and gets it
clear, the other the exact reverse — which is what a boundary one position off looks like
and what a missing behaviour never does. (Those two read `IF` rather than `DIV`, so unlike
their `lyc*_int_*` neighbours they are not caught by §6's retraction.) The four `poweron_*` rows are a different axis:
`standard` passes them by not modelling VRAM/OAM blocking at all, and they belong with
`boot_ppu_dot`.

**The method worth reusing: when the two engines disagree and one matches hardware, that
is a bisect, not a guess.** Diffing the engines localises the defect with no hardware
speculation at all — a tool this repository has had since the per-dot engine landed and
had not used this way.

## 5. Two models tested and rejected, with numbers

**Scaling the strobe leads with the CPU clock.** `OAM_STROBE_LEAD` and
`VBLANK_STROBE_LEAD` are 4 *dots*, and `OAM_STROBE_LEAD`'s own doc comment says its whole
content is "asserted inside an M-cycle". A double-speed M-cycle is 2 dots, so a 4-dot lead
spans two of them — the same defect class as the `c575bb4` LCDC stagger, and double-speed
rows do fail at 45 % against 30 % overall. Implemented (a `Gpu::set_double_speed` beside
the identical call the APU already takes, republished on `load_state`), verified live on
the PPU, and it moved **zero rows**. Reverted, and on reflection the reasoning inverts: the
PPU asserts at a fixed dot in *its own* clock, which never changes, so a CPU sampling twice
as often legitimately sees it a CPU-M-cycle earlier. That is correct behaviour. The
constant is right as it stands.

**`OAM_STROBE_LEAD = 0`.** Re-measured rather than quoted, because `AGENTS.md`'s figure
predates several PPU fixes and might have gone stale. It has not, and the trade is worse
than recorded: gbmicrotest **339/513 → 339/513 (no gain at all)**, Mooneye still 103/103
(the halt-late compensation carries it), and Mealybug **29 → 10 pixel-exact rows**. Keep
the lead at 4.

## 6. A third and fourth model, and the sharpest result in the set

**`LYC_STROBE_LEAD` — a lead on the LYC *interrupt* term only.** The code already
separates an interrupt source from the register that reports it (`hblank_strobe` versus
the `STAT` mode bits), so the minimal expression of §3's model is to lead the LYC term in
`stat_signal` while leaving `lyc_signal` — which `STAT` bit 2 reads — where it is.
Implemented and measured at both distinguishable values (the CPU samples on M-cycle
boundaries, so leads 1-4 are one machine and 5-8 another):

| | ladder | Mooneye | Mealybug | gbmicrotest | Gambatte |
|---|---|---|---|---|---|
| lead 0 (shipping) | flips 6→7 | 103/103 | 29 | 339 | 3622 |
| **lead 4** | **9/9 exact** | 103/103 | 29 | 337 | 3631 |
| lead 8 | flips 9→✗ | 103/103 | 29 | 337 | 3529 |

**Mooneye and Mealybug do not move by a single row at either value** — which is §3's
prediction confirmed, and it is the most useful thing here: those two suites genuinely
cannot see this axis, so their being green is not evidence that it is right.

**Lead 4 makes the ladder hardware-exact and is still rejected.** Gambatte gains 448 and
loses 439, in the *same groups on both sides* (`sprites` 202 gained against 203 lost). That
is a boundary being dragged across a population, not a defect being fixed —
`AGENTS.md`'s own rule: *a knob that trades tests one-for-one is not modelling anything*.
The +9 net is noise on a 900-row swap.

### The gbmicrotest rows are DIV-quantised, and that RETRACTS part of §4

The trade there is the clean signal: lead 4 gains `lyc1_int_halt_a`, `lyc1_int_nops_a`,
`lyc2_int_halt_a`, `lyc_int_halt_a` and loses `lyc1_int_halt_b`, `lyc1_int_nops_b`,
`lyc2_int_halt_b`, `lyc_int_halt_b`. Reading the sources, `_a` and `_b` are **one `nop`
apart** and every one of them finishes with `ldh a, (DIV)`:

```
lyc1_int_nops_a.s:  nops 25   test_finish_div 1
lyc1_int_nops_b.s:  nops 26   test_finish_div 2
```

So they do not measure the interrupt's phase — they measure **elapsed `DIV`**, and they
resolve it to 4 T-cycles. Hardware's `DIV` boundary falls *inside* the pair; ours falls
outside it on one side at lead 0 (both read 2) and outside on the other at lead 4 (both
read 1). Solving that: our residual is **δ ∈ (0, 4) T-cycles — strictly sub-M-cycle**.

**A sub-M-cycle residual cannot be an interrupt-dispatch phase error at all**, because the
CPU only ever acts on the interrupt line at an M-cycle boundary; moving the strobe within
an M-cycle changes nothing it can observe. It has to be the phase of **`DIV` itself
relative to the PPU**. So these thirteen rows are a *separate question* from §2's ladder,
and §4's reading of them as corroboration is withdrawn: they move when the LYC strobe
moves, but what they are actually resolving is a different clock.

The ladder is the clean instrument — pure `IF`, no `DIV` anywhere in it.

**And the obvious follow-up is also a measured negative.** If the residual is the LCD-on
restart phase (`Gpu::modeclock = 4` in the `LCDC` bit 7 0→1 arm, which every one of these
tests goes through), a value one to three dots higher would shorten that first line and
pull the interrupt earlier. Swept: gbmicrotest **339 → 312 (5), 311 (6), 287 (7)**. The
shipping 4 is right and this is not where the residual lives.

## 7. What of this belongs in the GBSelfTest cartridge — and what does not

The standing rule is that every fix ends as a fix, a check in the cartridge, and/or a
documented behaviour carrying a stable code. Judged honestly, item by item:

- **The Gambatte diagnostic** (the one thing landed) is **not checkable from a cartridge**
  and never could be. It is a property of *our runner* — which glyph it reports and what
  the expectation file records — and nothing a program running on the console can observe.

- **The `2 ↔ 0` STAT phase itself is checkable in principle, and is deliberately not added
  yet.** A cartridge could measure it with the nop-sled binary search `GB-PPU-07`/`-08`
  already use: take an `LY == LYC` interrupt, switch `STAT` to mode-0 select, clear `IF`,
  run *N* `nop`s, read `IF`. That is exactly the ladder, self-contained. **What is missing
  is a validated expectation.** Hardware's answer is known only *in the frame of Gambatte's
  own instruction sequence*; a new cartridge check is a different program, so the expected
  M-cycle count would have to be re-derived — and the derivation is the very thing §2-§6
  could not settle. Asserting a number picked to match our own output would be inventing
  ground truth, which is the one thing a conformance cartridge must never do. The precedent
  is `GB-DMA-05`, which shipped as *reported, not judged* until the rule behind it was
  established, and was only promoted to a real check once it was.

  It also could not be validated the usual way. `tools/sameboy-oracle` reads a verdict byte
  or cartridge RAM; these ROMs report by **drawing to the framebuffer**, which the oracle
  cannot reach — checked, `peek $9800` returns a constant across all nine ladder variants.
  So the second-implementation cross-check that every GBSelfTest addition gets was not
  available here either.

- **`GB-PPU-16` already covers the neighbouring behaviour and is correctly red.** It should
  stay on `tools/gbselftest-gate.py`'s known-gap list. §1 narrows what it is waiting for:
  the DMG STAT-write glitch is **not** the cause of the large `2 ↔ 0` cluster, so closing
  `GB-PPU-16` and closing those 226 rows are two jobs, not one.

**Net: no check added, and the reason is that the finding is a measured discrepancy rather
than a settled behaviour.** The moment §3's model is confirmed by an independent source,
the check is ~40 lines in the idiom `GB-PPU-16` already uses.

## Reproducing

```
# the ladder, both engines
for i in 1 2 3 4 5 6 7 8 9; do
  f=$(ls target/test-suites/csp/gambatte/miscmstatirq/ | grep "m0_early_ly44_lyc44_08_40_${i}_")
  grep -F "miscmstatirq/${f%.gbc}@dmg08 " testharness/csp_gambatte_accurate_baseline.txt
done

# the engine inversion
python3 - <<'PY'
def load(p):
    d={}
    for l in open(p):
        l=l.strip()
        if l and not l.startswith('#') and '=' in l:
            k,v=l.split('=',1); d[k.strip()]=v.strip()
    return d
a=load('testharness/csp_gambatte_accurate_baseline.txt')
s=load('testharness/csp_gambatte_baseline.txt')
print(sum(1 for k in a if s.get(k,'').startswith('PASS') and not a[k].startswith('PASS')))
PY
```

The `.asm` sources are not in the c-sp bundle, which ships ROMs only:
`git clone --depth 1 https://github.com/pokemon-speedrunning/gambatte-core`. They are read
as **test specifications** — facts about what each ROM measures — never as emulator code.

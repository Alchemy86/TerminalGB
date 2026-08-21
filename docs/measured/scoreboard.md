# Scoreboard — every suite, and a reason for every red row

← [The measured Game Boy](README.md)

What this emulator scores, group by group, **and why each remaining failure is still there**.
A total on its own is not evidence; a named reason per red row is.

> On the ROM versions pinned in
> [`tools/fetch-test-suites.sh`](../../tools/fetch-test-suites.sh), with the **default
> (`standard`) picture mode** unless a row says otherwise.

### Which numbers on this page are gated, and which are not

Read this before quoting anything here, because the answer is not uniform — and a silent stale
score is worse than an admitted one.

| | |
|---|---|
| **Gated** — re-derived from `testharness/*_baseline.txt` by [`docs/tools/status_numbers.py`](../tools/status_numbers.py), rewritten by `make status`, and **CI red whenever the page and the baselines disagree** | every current suite score on this page, in the tables **and in the prose**, each wrapped in a `<!--score:…-->` marker naming its suite |
| **Not gated, and cannot be** | the [Shootout](#the-shootout-is-a-different-population-again) figure (measured through their manifest and screenshot comparison, not derivable from any baseline) and other emulators' scores (published board figures) |
| **Deliberately not gated** | historical attribution figures (`13/66 → 52/66`) and measured counterfactuals ("moving that constant costs 56/76"). Those are *not* current scores and must not track the baselines |

That split exists because this file drifted exactly once and nothing caught it: the generated
table was verified on every CI run while a **sentence** two sections down carried a Mealybug
figure five rows out of date. `make docs-check` passed the whole time, because it only ever
looked at the generated block. **A score written into a sentence is gated now too** — the
tool's docstring records why a regex hunting prose for `N/M` was tried and rejected.

---

## Today

| Suite | Rows | Pass | Judged by |
|---|---:|---:|---|
| In-repo harness (Blargg + dmg-acid2) | <!--score:harness:total-->47<!--/score--> | **<!--score:harness:pass-->46<!--/score-->** | the ROM's own printed verdict / the author's reference image |
| Mooneye GB `acceptance` | <!--score:mooneye_acceptance:total-->75<!--/score--> | **<!--score:mooneye_acceptance:pass-->67<!--/score-->** | the ROM's own `LD B,B` + Fibonacci register check |
| Mooneye GB `emulator-only` | <!--score:mooneye_emulator_only:total-->28<!--/score--> | **<!--score:mooneye_emulator_only:pass-->28<!--/score-->** | as above |
| Mooneye GB `misc` (CGB) | <!--score:mooneye_cgb:total-->8<!--/score--> | **<!--score:mooneye_cgb:pass-->8<!--/score-->** | as above |
| SameSuite | <!--score:samesuite:total-->76<!--/score--> | **<!--score:samesuite:pass-->73<!--/score-->** | the ROM's own `RESULT_CODE` at `$CFFE` |
| Mealybug Tearoom | <!--score:mealybug:total-->79<!--/score--> | **<!--score:mealybug:pass-->3<!--/score-->** pixel-exact | the author's hardware photographs |
| `cpp` (CasualPokePlayer MBC3/RTC + SGB) | <!--score:cpp:total-->4<!--/score--> | **<!--score:cpp:pass-->4<!--/score-->** | the author's reference screenshots |
| **[GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/)** | **264** | **223 (84.5 %)** | their screenshot comparison — **5th of 19** (18 published emulators, plus us). *Not baseline-derived — see below* |

Re-derive every one of the first seven from the checked-in baselines, in about a second, with
no ROMs and no emulator run — the baselines *are* the expectations:

```python
# python3, from the repo root
for f in ['baseline', 'mooneye', 'mooneye_cgb', 'samesuite', 'mealybug', 'cpp']:
    p = f'testharness/{f}_baseline.txt' if f != 'baseline' else 'testharness/baseline.txt'
    rows = [l.split('=', 1) for l in open(p)
            if l.strip() and not l.startswith('#') and '=' in l]
    print(f'{f:14s} {sum(v.strip().startswith("PASS") for _, v in rows):4d} / {len(rows)}')
```

Run them for real with `make test-external-accurate` (the default engine) or
`make test-external` (the cheap one) and
`cargo test --release --test testroms` (~3.6 s). Mechanics — how each suite publishes a
verdict and how the harness reads it — are in [`docs/testing.md`](../testing.md); this page is
only the result.

---

## Mooneye GB — every remaining failure is a PPU test

`acceptance` **<!--score:mooneye_acceptance-->67/75<!--/score-->**, and the eight reds are *all
eight* of the `ppu/` group's failures:

```
acceptance/ppu/hblank_ly_scx_timing-GS      acceptance/ppu/intr_2_mode3_timing
acceptance/ppu/intr_2_0_timing              acceptance/ppu/intr_2_oam_ok_timing
acceptance/ppu/intr_2_mode0_timing          acceptance/ppu/lcdon_timing-GS
acceptance/ppu/intr_2_mode0_timing_sprites  acceptance/ppu/lcdon_write_timing-GS
```

Everything else is green — and it is worth listing what "everything else" covers, because the
groups are the hard ones:

| Group | Score | |
|---|---:|---|
| `timer/*` | **13/13** | including `rapid_toggle`, which only one implementation detail passes — see [timing.md](timing.md#the-one-genuinely-subtle-bit-tac-latches-before-the-increment) |
| `oam_dma*` (6 ROMs) | **6/6** | a timed 160 M-cycle transfer, not an instantaneous copy |
| instruction timing (`call_*`, `ret_*`, `push`, `pop`, `rst`, `jp`, `jr`, `add_sp_e`, `ld_hl_sp_e`) | **all** | 14 of these are only *measurable* once OAM DMA is timed |
| `interrupts/`, `intr_timing`, `ei_*`, `di_timing`, `halt_*`, `rapid_di_ei`, `reti_*` | **all** | |
| `boot_regs` / `boot_div` / `boot_hwio` across five consoles | **all** | see [boot.md](boot.md) |
| `serial/boot_sclk_align` | **PASS** | |
| `emulator-only/mbc1` · `mbc2` · `mbc5` | **13/13 · 7/7 · 8/8** — <!--score:mooneye_emulator_only-->28/28<!--/score--> together | includes MBC1M multicarts — see [cartridges.md](cartridges.md) |
| `misc/*` (CGB) | **<!--score:mooneye_cgb-->8/8<!--/score-->** | |

**Why the eight are red, honestly:** they sample OAM/VRAM *accessibility* and mode-3 length at
one-M-cycle resolution. `lcdon_timing-GS` and `lcdon_write_timing-GS` in particular are **not
separable** from a per-dot PPU — and that is now settled rather than predicted: **both pass on
the [`identical` mode](../picture-modes.md)**, which has the mode-3 length and the per-access
stepping the rule needs, and both stay red here. Do **not** add mode-based VRAM/OAM blocking to
the `standard` mode: with mode 3 fixed at 172 dots where hardware varies it 172–289, blocking
would silently drop writes a game placed legally in HBlank, which is strictly worse than not
blocking. See [ppu.md](ppu.md).

### The `identical` picture mode scores differently — and mostly dominates

The [`identical` picture mode](../picture-modes.md) (`GB_PPU=identical`, formerly `accurate`)
gives **<!--score:mooneye_acceptance_accurate-->75/75<!--/score-->** acceptance against the
default `standard` mode's <!--score:mooneye_acceptance-->67/75<!--/score-->. The delta is three
rows and it is one-directional:

| Row | `standard` | `identical` |
|---|---|---|
| `acceptance/ppu/hblank_ly_scx_timing-GS` | FAIL | **PASS** |
| `acceptance/ppu/lcdon_timing-GS` | FAIL | **PASS** |
| `acceptance/ppu/lcdon_write_timing-GS` | FAIL | **PASS** |
| `acceptance/ppu/intr_1_2_timing-GS` | PASS | PASS |
| the five `acceptance/ppu/intr_2_*` | FAIL | **PASS** |

`identical` has **no red rows left in `acceptance` at all**. The five `intr_2_*` were the last,
and they were red for the same reason `OAM_STROBE_LEAD` was documented as a contested knob:
they take the mode-2 STAT interrupt out of **`HALT`**, where Mealybug's ROMs take it out of a
**`nop` slide**, and a request asserted part-way through an M-cycle reaches those two CPUs one
M-cycle apart. Modelling that difference satisfies both suites with the strobe untouched — see
[`conformance-notes.md`](../conformance-notes.md) (2026-08-15, "a mid-M-cycle interrupt reaches
a halted CPU late").

`intr_1_2_timing-GS` used to be the one row `identical` lost, and it was the last thing
blocking a default flip *in Mooneye*. It is fixed — see
[mealybug.md §9](../mealybug.md#9-the-line-0-stat-phase-679--1279-and-the-last-blocking-row).
The mode is gated separately (`make test-external-accurate`, against
`testharness/mooneye_accurate_baseline.txt`) so progress *and* regression are both visible.

> **`identical` IS the default, since 2026-08-20.** Through the GB Emulator Shootout's own
> 264-row manifest the two engines are **223 and 238 — fifteen gains, zero losses** — and
> across every checked-in baseline, `SKIP` rows in neither column, **3691 / 6210** against
> **4274 / 6210**. `make test-external` is the *cheap* engine's gate now, and both targets
> name their engine on every line so no baseline changed hands. Full record:
> [`conformance-notes.md`](../conformance-notes.md) (2026-08-20) and
> [`picture-modes.md`](../picture-modes.md).

**Measured mode-for-mode, every gate, same build** — every figure in this paragraph is gated:

| Gate | `standard` | `identical` |
|---|---:|---:|
| In-repo harness (Blargg + dmg-acid2) | <!--score:harness-->46/47<!--/score--> | <!--score:harness-->46/47<!--/score--> — *identical verdicts row for row* |
| Mooneye `acceptance` | <!--score:mooneye_acceptance-->67/75<!--/score--> | **<!--score:mooneye_acceptance_accurate-->75/75<!--/score-->** |
| Mooneye `emulator-only` | <!--score:mooneye_emulator_only-->28/28<!--/score--> | <!--score:mooneye_emulator_only_accurate-->28/28<!--/score--> |
| Mooneye `misc` (CGB) | <!--score:mooneye_cgb-->8/8<!--/score--> | <!--score:mooneye_cgb-->8/8<!--/score--> |
| SameSuite | <!--score:samesuite-->73/76<!--/score--> | <!--score:samesuite-->73/76<!--/score--> |
| `cpp` | <!--score:cpp-->4/4<!--/score--> | <!--score:cpp-->4/4<!--/score--> |
| Mealybug — rows pixel-exact | <!--score:mealybug-->3/79<!--/score--> | **<!--score:mealybug_accurate-->27/79<!--/score-->** |
| Mealybug — pixels wrong *(lower is better)* | <!--score:mealybug:px-->1,212,282<!--/score--> | **<!--score:mealybug_accurate:px-->497,312<!--/score-->** |

(Rows with one baseline are scored once: the picture mode does not reach what they measure, so
the same figure stands in both columns.)

**On these six gates no row goes the other way**, and the cost is **~8.8x** — 0.058 → 0.546
ms/frame, 30x real time headless, re-measured on every `cargo test --release --test
ppuengines`. (It was ~6.8x until `standard` learnt to draw a monochrome tile with word arithmetic;
a ratio is two numbers, and this one has only ever moved because the *default* engine got faster —
[`docs/throughput.md`](../throughput.md) §12.)

> **Correction, 2026-08-15: rows DO go the other way, on gates this table did not have.**
> The thirteen [c-sp suites](#the-c-sp-aggregation--thirteen-suites-measured-for-the-first-time)
> wired up that day found three places where `identical` scored *worse* than the default:
> `cgb-acid2` (pixel-exact on `standard`, 158 pixels wrong on `identical`), and Gambatte's
> `sprites` (245 → 241) and `window` (259 → 227) groups. `identical` still won overwhelmingly
> — Gambatte 2,682 → 3,059 and GBMicrotest 251 → 338 — but "loses no row anywhere" was a claim
> about the six gates that existed when it was written, and it was not true of the twenty that
> exist now.
>
> **Update, 2026-08-20: two of those three are closed, and the cause was one condition.** The
> window pixel-insertion glitch needs the window to be *drawing* on Color silicon and does not
> on a DMG — docboy's `handle_pixel_slice_fetcher_push` gates it `#ifdef ENABLE_CGB && w.active`
> with the comment "on CGB the glitch seems to happen only if we are fetching a window tile
> right now". Ungated, a Color console inserted a pixel at `WX - 7` on every line where the
> window's Y condition had latched and the window was *off*: `cgb-acid2` has `WX = 95` and
> `SCX & 7 == 0`, so a push landed exactly on column 88 and the whole of every line came out
> one pixel to the right — 158 pixels, all of them in rows 133-139 where the picture is not
> uniform. `cgb-acid2` is **pixel-exact on both engines** now and Gambatte's `window` group
> goes 108 losses / 137 gains, **net +29**. Four small groups are still net-negative —
> `sprites` −3, `lyc153int_m2irq` −2, `lycint_lycflag` −2, `m2int_m0stat` −1, **eight rows
> between them against 891 gains.**

> This table is where the drift happened. Its Mealybug figure read **12** for as long as it
> took somebody to notice, five rows after `identical` reached
> <!--score:mealybug_accurate-->27/79<!--/score-->. The row is generated now.

> **Both baselines must move together when a non-PPU change flips a row.** Boot registers, the
> timer and the APU do not depend on `GB_PPU`, so such a change flips the row under both
> modes and both files need re-blessing in the same commit — otherwise the `identical` gate
> goes red on merge and reads as a PPU regression that isn't one.

### Three different denominators, all called "Mooneye"

| Denominator | `acceptance` rows | Why it differs |
|---:|---|---|
| **75** | ours | every ROM in the directory, including the nine named-machine variants |
| **66** | the community's usual figure | drops nine `-S`/`-sgb`/`-sgb2`/`-mgb`/`-dmg0` ROMs, because most emulators cannot run those machines |
| **66** | the [Shootout](https://gbdev.io/GBEmulatorShootout/)'s manifest | its `testroms/mooneye.py` comments out those same nine (plus six in `misc`) |

**All nine of ours pass.** This is not a footnote — it is the reason a real accuracy gain can
measure as zero. The work that added the five extra console revisions moved eleven rows on our
own gates and moved the Shootout number by **exactly zero**, verified by re-scoring all 264 and
diffing: no row changed in either direction. *Check which population a number belongs to before
comparing two of them.*

### The Shootout is a different population again

Three separate traps, and each one has produced a wrong comparison here at least once. **Never
put one of these numbers beside another without saying which population it belongs to.**

| | |
|---|---|
| **The Shootout suite is 264 rows, not 311.** `testroms/mealybug.py` builds three lists and ends with `all = dmgs # + cgbcs + cgbds`, so 47 of the `Test(...)` objects it constructs are never run | a manifest generator that collects *constructions* instead of each module's `all` gives 311 and buries a DMG-only score under CGB rows nobody is graded on |
| **Its Mooneye population is 97 rows, not our 103** — `acceptance` 66 (not <!--score:mooneye_acceptance:total-->75<!--/score-->) and `misc` 2 (not <!--score:mooneye_cgb:total-->8<!--/score-->) | its `testroms/mooneye.py` comments out **all fifteen** model-variant ROMs — exactly the population the console-revision work fixed, which is why eleven rows of real accuracy moved the Shootout number by zero |
| **Its SameSuite population is not ours either.** Only two of the seven rows the CH4/CGB-revision work moved are in the 311-row manifest at all; the five `extra_length_clocking-*` ROMs are not in that corpus | so that work's honest published-264 delta is *at most* +2, and screenshot-judged, which it did not measure |

Our own figure — **223 / 264, 84.5 %, 5th of 19** — is measured through their manifest and
their screenshot comparison, and is **not derivable from any baseline**, which is why it is the
one score on this page the generator leaves alone. Re-derive it with
`python3 tools/shootout/manifest.py <checkout> > m.tsv` and
`cargo run --release --features image --example shootout_scout -- m.tsv` (~17 s). An older
report's **92/264** is stale by a wide margin. Full method: [`docs/shootout.md`](../shootout.md).

---

## SameSuite — three evidenced reds

**<!--score:samesuite-->73/76<!--/score-->.** All three reds are APU rows, and each is red for a
*measured* reason. Every claim in this table was re-opened on 2026-08-15 and checked against
SameBoy at a pinned commit — run
[`tools/sameboy-oracle/`](../../tools/sameboy-oracle/README.md) and see for yourself, because
two of the excuses that used to sit here turned out to be wrong.

| Row | Why it is red |
|---|---|
| `apu/channel_4/channel_4_freq_change` | The ROM's own header says *"the logic behind it is still unclear"*, and it is: the LFSR is clocked from a **free-running 14-bit counter** whose phase survives an NR43 write, plus a table of write glitches SameBoy's source annotates *"non-deterministic"* and *"no clue why"*. An exhaustive search over the clean counter model (both reload policies, every trigger phase, every read offset) reaches **61 of the 64 cells** and no further. |
| `apu/channel_1/channel_1_freq_change_timing-cgb0BC` | CPU CGB 0/B/C — a **machine we do not boot**, and reachable only by modelling two of its behaviours at once: the PCM12 read-back glitch (SameBoy masks the read on `model <= CGB_C`) *and* a double-speed-only condition on the reload correction. A latch value alone cannot do it: sweeping `SQUARE_RELOAD_LATCH` over `1..=7` never turns this row green at any value. |
| `apu/channel_1/channel_1_sweep_restart` | **143 of its 144 cells match**, and the cause is *not* the sweep unit and *not* `SQUARE_RESTART_SPEEDUP`. See [apu.md](apu.md#channel_1_sweep_restart--one-duty-step-in-4100). |

Two excuses that used to sit in this table and **did not survive being checked**:

* *"SameSuite's own README says SameBoy fails `channel_4_freq_change` too."* Measured: current
  SameBoy on CGB-E **passes** it, and passes `channel_1_sweep_restart` as well. That README
  predates its own emulator. A row a leading implementation passes is a row with a working
  model behind it, whatever its header says about clarity.
* *"`-A` is a Game Boy Advance, a console this emulator does not model."* It models one — for
  its boot state — and now for its APU too: `channel_1_freq_change_timing-A` **passes**, on an
  AGB, for the same reason `channel_1_extra_length_clocking-cgb0B` passes on a CPU CGB 0. See
  [apu.md](apu.md#the-frequency-reload-latch-is-narrower-on-an-agb).

And a counterpoint that keeps the first row honest: we **pass**
`apu/channel_1/channel_1_sweep_restart_2`, the *other* test that README names SameBoy as
failing. The suite is not simply harder than us in one direction.

Group totals: `apu` **<!--score:samesuite_apu-->66/69<!--/score-->** · `dma` **<!--score:samesuite_dma-->4/4<!--/score-->** · `ppu` **<!--score:samesuite_ppu-->1/1<!--/score-->** · `sgb` **<!--score:samesuite_sgb-->2/2<!--/score-->**.

---

## Mealybug Tearoom — and the number is not a percentage

Mealybug photographs a real console mid-scanline. It is the suite that separates a
whole-scanline renderer from a pixel FIFO, and it is *hard*: on the Shootout's 24 published
Mealybug rows, **SameBoy scores 15, bgb scores 2, PyBoy 0.** This is not a suite a competent
cycle-stepped PPU sweeps. (Those three are published board figures — **not** gated here, and
they belong to the Shootout's 24-row Mealybug population, not our
<!--score:mealybug:total-->79<!--/score-->.)

> **Where the remaining rows are most likely to come from: reading the emulators ahead of us.**
> Five of the seventeen `identical` rows were won that way in one pass rather than in weeks —
> and the reference list now includes **[mGBA](https://github.com/mgba-emu/mgba), which is
> MPL-2.0, the same licence as this repository** (verified against its root `LICENSE`, not
> assumed). That is a materially different position from docboy's MIT and SameBoy's Expat:
> compatible in the strong sense, so its code is reusable with file-level attribution, not
> merely readable for behaviour. Prefer reimplementing in this codebase's idiom anyway, and
> cite repo, file and symbol for anything taken. Full table and the reasoning:
> [README § reading other emulators](README.md#reading-other-emulators-what-the-licences-actually-allow).

| Picture mode | Rows pixel-exact | Total pixel distance |
|---|---:|---:|
| `identical` (the default) | **<!--score:mealybug_accurate-->27 / 79<!--/score-->** | **<!--score:mealybug_accurate:px-->497,312<!--/score-->** |
| `standard` (`GB_PPU=standard`) | <!--score:mealybug-->3 / 79<!--/score--> | <!--score:mealybug:px-->1,212,282<!--/score--> |

The baseline pins the **pixel count per row**, not just PASS/FAIL, precisely because at
<!--score:mealybug-->3/79<!--/score--> a
pass/fail file is a flat wall that cannot show a rewrite getting closer. Consequence to expect:
**any PPU change turns that file red.** Read the diff, re-bless with the new counts in the same
commit — that is the point.

**And plan against eleven rows, not fifty-three.**
[`docs/mealybug.md` §11](../mealybug.md#11-the-denominator-45-of-the-failing-rows-cannot-be-won-by-ppu-work-15-aug-2026)
sorts every failure by what closing it would take: 27 need the boot ROM's `®` glyph, 8 compare
our greyscale against a colour photograph and 7 are a ROM/reference mismatch in the suite
itself. **45 of the 79 cannot be won by PPU work at all**, which puts the reachable ceiling at
34 and the rows genuinely in play at eleven.

> **A pixel count is a rank distance and it is wildly non-linear.** Shades are ranked per
> frame, so one extra distinct shade re-ranks everything and reports ~23,040 of 23,040.
> `m3_lcdc_obj_size_change_scx` is **four wrong pixels** and scores 22,070. Read a row near
> 22,500 as *"wrong shade count"*, not *"nothing works"*, and never convert the total into a
> percentage.

Eight rows cannot be compared at all: the CGB references for `m3_wx_4/5/6_change` and
`m3_lcdc_win_en_change_multiple_wx` carry **125 distinct luminances** — a Color console
colourising a DMG cartridge from the boot ROM's title-hash palette — against four or fewer
everywhere else. That is 26 % of the remaining pixel distance and no PPU work moves it.

Full scoping, including which rows are winnable now versus after the rewrite:
[`docs/mealybug.md`](../mealybug.md).

---

## The c-sp aggregation — thirteen suites measured for the first time

[c-sp/game-boy-test-roms](https://github.com/c-sp/game-boy-test-roms) is the community's
standard bundle: eighteen suites, one pinned release. Five of them were already scored above.
**The other thirteen had never been run here at all**, which is 4,510 ROMs and, once each is
scored on the consoles its own file names name, **6,045 rows**. They are gated now, in both
picture modes, by [`examples/csp_suite.rs`](../../examples/csp_suite.rs).

| Suite | `standard` | `identical` | Judged by |
|---|---:|---:|---|
| Gambatte | **<!--score:csp_gambatte:pass-->3173<!--/score-->** / <!--score:csp_gambatte:total-->5225<!--/score--> | **<!--score:csp_gambatte_accurate:pass-->3617<!--/score-->** / <!--score:csp_gambatte_accurate:total-->5225<!--/score--> | a hexadecimal digit drawn in the top-left corner, its audio, or the author's screenshot |
| GBMicrotest | **<!--score:csp_gbmicrotest:pass-->251<!--/score-->** / <!--score:csp_gbmicrotest:total-->513<!--/score--> | **<!--score:csp_gbmicrotest_accurate:pass-->338<!--/score-->** / <!--score:csp_gbmicrotest_accurate:total-->513<!--/score--> | the verdict byte the ROM writes to `$FF82` |
| Mooneye GB (wilbertpol fork) | **<!--score:csp_mooneye-wilbertpol:pass-->62<!--/score-->** / <!--score:csp_mooneye-wilbertpol:total-->117<!--/score--> | **<!--score:csp_mooneye-wilbertpol_accurate:pass-->82<!--/score-->** / <!--score:csp_mooneye-wilbertpol_accurate:total-->117<!--/score--> | `$ED` then the Fibonacci registers |
| AGE test roms | **<!--score:csp_age:pass-->3<!--/score-->** / <!--score:csp_age:total-->59<!--/score--> | **<!--score:csp_age_accurate:pass-->10<!--/score-->** / <!--score:csp_age_accurate:total-->59<!--/score--> | `LD B,B` then the Fibonacci registers, or the author's screenshot |
| cgb-acid2 | **<!--score:csp_cgb-acid2:pass-->1<!--/score-->** / <!--score:csp_cgb-acid2:total-->1<!--/score--> | **<!--score:csp_cgb-acid2_accurate:pass-->0<!--/score-->** / <!--score:csp_cgb-acid2_accurate:total-->1<!--/score--> | the author's reference image, in colour |
| cgb-acid-hell | **<!--score:csp_cgb-acid-hell:pass-->0<!--/score-->** / <!--score:csp_cgb-acid-hell:total-->1<!--/score--> | **<!--score:csp_cgb-acid-hell_accurate:pass-->0<!--/score-->** / <!--score:csp_cgb-acid-hell_accurate:total-->1<!--/score--> | as above |
| rtc3test | **<!--score:csp_rtc3test:pass-->6<!--/score-->** / <!--score:csp_rtc3test:total-->6<!--/score--> | **<!--score:csp_rtc3test_accurate:pass-->6<!--/score-->** / <!--score:csp_rtc3test_accurate:total-->6<!--/score--> | the author's screenshots, one per sub-test |
| MBC3 Tester | **<!--score:csp_mbc3-tester:pass-->2<!--/score-->** / <!--score:csp_mbc3-tester:total-->2<!--/score--> | **<!--score:csp_mbc3-tester_accurate:pass-->2<!--/score-->** / <!--score:csp_mbc3-tester_accurate:total-->2<!--/score--> | the author's screenshots |
| Scribbltests | **<!--score:csp_scribbltests:pass-->7<!--/score-->** / <!--score:csp_scribbltests:total-->9<!--/score--> | **<!--score:csp_scribbltests_accurate:pass-->7<!--/score-->** / <!--score:csp_scribbltests_accurate:total-->9<!--/score--> | the author's screenshots |
| TurtleTests | **<!--score:csp_turtle-tests:pass-->2<!--/score-->** / <!--score:csp_turtle-tests:total-->2<!--/score--> | **<!--score:csp_turtle-tests_accurate:pass-->2<!--/score-->** / <!--score:csp_turtle-tests_accurate:total-->2<!--/score--> | the author's screenshots |
| Strikethrough | **<!--score:csp_strikethrough:pass-->0<!--/score-->** / <!--score:csp_strikethrough:total-->2<!--/score--> | **<!--score:csp_strikethrough_accurate:pass-->2<!--/score-->** / <!--score:csp_strikethrough_accurate:total-->2<!--/score--> | the author's screenshots |
| Bully | **<!--score:csp_bully:pass-->0<!--/score-->** / <!--score:csp_bully:total-->1<!--/score--> | **<!--score:csp_bully_accurate:pass-->0<!--/score-->** / <!--score:csp_bully_accurate:total-->1<!--/score--> | the author's screenshot |
| little-things-gb | **<!--score:csp_little-things-gb:pass-->1<!--/score-->** / <!--score:csp_little-things-gb:total-->2<!--/score--> | **<!--score:csp_little-things-gb_accurate:pass-->1<!--/score-->** / <!--score:csp_little-things-gb_accurate:total-->2<!--/score--> | the author's screenshot |

Rows marked `SKIP` in a baseline count in **neither** column and each says why. There are 96 of
them: 95 Gambatte ROMs that upstream itself disables (a `_xout0` name makes its own
`find("_out")` miss them) or that are state dumpers with no expectation at all, and
`tellinglys`, which only reaches a verdict after a human has pressed every button in an
arbitrary order.

### Every verdict here is the suite author's

Each suite in the release ships a `game-boy-test-roms-howto.md` stating its exit condition and
its pass criterion, and those documents — not our judgement — are what
[`examples/csp_suite.rs`](../../examples/csp_suite.rs) implements. Nothing on this page is
compared against a frame we produced. Where a suite ships no reference at all
(Scribbltests' `failrylake` and `winpos`, which its own howto says are missing), the row is
`SKIP`, not a baseline invented from our own output.

The one place a *runner* decision was needed is colour, and it is decided from evidence rather
than assumed: a reference drawn in the four DMG greys, or in the Color boot ROM's eight
compatibility colours, is compared by shade index; a real colour reference is taken back to
15-bit RGB (`(X << 3) | (X >> 2)` is exactly invertible) and pushed through *our* colour
correction, so our framebuffer is never re-ranked. **A shade-rank compare cannot do the
compatibility case** — eight distinct luminances in the reference against four in our frame
re-ranks every pixel.

### What the thirteen suites bought, immediately

* **A save-visible mapper bug.** MBC3 Tester walks all 256 banks of a 4 MiB cartridge and draws
  a tick for each one it can read back. It drew **128 `F`s**: our ROM-bank register masked
  `v & 0x7F`, so banks `$80-$FF` were unreachable. Pan Docs' [MBC3] page is right that the
  register is seven bits — but its [MBC30] section describes the eighth line, on the mapper
  that shipped in the Japanese Pokémon Crystal, and no cartridge-type byte tells the two apart.
  A cartridge with more than 128 banks cannot be addressed by seven bits at all, so its ROM
  size is what decides. Fixed, with the seven-bit behaviour kept for everything smaller;
  `mbc3-tester` went **FAIL → PASS** on both consoles.
* **A `boot_ppu_dot` comment that was out of date.** `src/mmu/mod.rs` recorded that no ROM
  measures where the PPU sits when a plain DMG's boot ROM hands over, so no position was
  invented. GBMicrotest's `poweron_ly_*` and `poweron_stat_*` measure exactly that — they are a
  run of `nop`s and one `LDH A,[$FF44]` — and they disagree with us: hardware reads `LY = 0` at
  119 M-cycles and `1` at 120, where we roll over at 114. **The constant was still not
  changed**: sweeping it scores 237/513 at dot 69900, 251 at line 0 (today), 262 at 70168 and
  250 at 70184, a jagged profile with no clean optimum — the signature of a constant absorbing
  an error somewhere else, which this repository has been careful about before.
* ~~**A `cgb-acid2` row the `identical` engine loses.**~~ **Closed, 2026-08-20** — the window
  pixel-insertion glitch is Color-gated now and the row is pixel-exact on both engines; see the
  update in ["the `identical` picture mode scores differently"](#the-identical-picture-mode-scores-differently--and-mostly-dominates)
  above. It was a real counter-example while it stood, and it was a PPU finding rather than a
  runner one, which is why one condition closed it and 63 Gambatte rows came with it.
* **`cgb-acid-hell` is two pixels away.** `(80, 68)` and `(80, 69)` are swapped: the reference
  wants yellow above black and we draw black above yellow, one row of one column of the devil's
  face. Everything else in the picture is exact.

[MBC3]: https://gbdev.io/pandocs/MBC3.html
[MBC30]: https://gbdev.io/pandocs/MBC3.html#mbc30

### Where the remaining reds are — and where each group goes next

Grouped the way the Mealybug campaign groups its rows, so the denominator is honest. **A suite
is an input, not a scoreboard**: every group below ends up as a fix, as a check in
[GBSelfTest](../pitfalls/codes.md), or as a documented behaviour with a **stable code** a
failing check can print — see [the stable-code registry](../pitfalls/codes.md), which carries
the full triage (row counts, whether a public suite already covers the behaviour well, and
whether it is decidable from inside a cartridge at all).

* **Mid-scanline PPU timing is most of it, and it is the same wall Mealybug describes.** In
  Gambatte, `scy` scores 0/67 and `bgtilemap` 0/40 on the default engine and 54/67 and 36/40 on
  the per-dot one; `scx_during_m3` goes 3/178 → 30/178. In GBMicrotest the `int_hblank_*_scx`
  family is 0/24. These are not separate problems from the Mealybug campaign — they are the
  same one, measured by four more authors.
* **OAM and VRAM access blocking.** GBMicrotest's `poweron_oam_006` and `poweron_vram_026` want
  `$FF` from a locked bus and get the real byte. That is deliberate on the default engine and
  documented at `Gpu::cpu_can_read_vram`: with mode 3 fixed at 172 dots where hardware varies it
  172-289, blocking would silently drop writes a game placed legally in HBlank. The per-dot
  engine has both halves and does block, which is most of its +87 rows here.
* **Console revisions we do not model.** AGE names the CPU revision in every file name
  (`-cgbBC` versus `-cgbE`), and where a name lists several (`-cgbBCE`) no single machine can
  satisfy all of them. The runner forces a stepping only when the name gives exactly one.
* **`firstwhite` prints an error on a DMG and not on a Color console** — it draws its message
  at about frame 21 and holds it, on both picture modes. Unexplained; the ROM is
  [pinobatch/little-things-gb](https://github.com/pinobatch/little-things-gb)'s and its readme
  says an emulator that shows text there "probably shows 1-frame glitches in *Pokémon
  Pinball*".
* **`bully` draws nothing at all** — our frame is entirely black where the reference is a page
  of text. That is a failure to *start*, not a failure of a check, and it is the first thing to
  look at in this group.

### Re-running them

```sh
make test-external-accurate     # every suite, GB_PPU=identical — the DEFAULT (~45 s)
make test-external              # the same, GB_PPU=standard — the cheap engine (~20 s)

# one suite, with the per-row detail and the frames written out:
cargo run --release --example csp_suite -- \
    --suite gbmicrotest --dir target/test-suites/csp --out target/cspshots
```

`--filter <substr>` narrows to one ROM and `--frames N` overrides a suite's frame budget for
investigating a row by hand; neither is used by the gate, whose budgets come from each suite's
own howto.

**The Gambatte baseline is 5,320 rows and about 400 KB, twice over.** That is deliberate: at
that size a PASS/FAIL total is a flat wall, and what a PPU campaign needs is to see *which*
rows moved. Expect any PPU change to produce a large, readable diff there — that is the point,
and it is the same trade the Mealybug baselines make with their pixel counts.

---

## The in-repo harness — the one red must stay red

**<!--score:harness-->46/47<!--/score-->.** 30 ROMs × console = <!--score:harness:total-->47<!--/score--> rows (`interrupt_time` and the twelve `cgb_sound` ROMs are CGB-only
cartridges, so they have no `@dmg` row). Blargg `cpu_instrs` 11/11, `instr_timing`,
`mem_timing`, `mem_timing-2`, `halt_bug`, `interrupt_time`, `cgb_sound` 12/12 and dmg-acid2
(**0 of 23,040 pixels differ**) all pass on both machines.

**`oam_bug@cgb` is pinned FAIL on purpose.** The OAM-corruption bug is DMG/MGB/SGB silicon
only — Pan Docs' [OAM Corruption Bug](https://gbdev.io/pandocs/OAM_Corruption_Bug.html) says
CGB and Advance are unaffected *even running monochrome software* — so a real Game Boy Color
fails this ROM too. Turning that row green would mean emulating a machine nobody has.
`oam_bug@dmg` passes **8 of 8** sub-tests.

---

## Where the numbers are pinned

Every pin lives in [`tools/fetch-test-suites.sh`](../../tools/fetch-test-suites.sh) and nowhere
else, so a laptop, a CI job and a published score measure identical bytes: Mooneye from
Gekkio's own prebuilt archive (sha256-checked), Mealybug from the author's repo at a pinned
commit — his prebuilt ROMs **and** his hardware-photographed references — and SameSuite plus
`cpp` from the copies vendored by the Shootout at a pinned commit, which is also the exact
corpus the published 264-row score is measured on.

**Nothing is vendored.** These are other people's ROMs under other people's licences, fetched
at run time — the same reason the conformance action downloads Blargg's rather than committing
them. Bumping a pin will move baselines; the two must land in the same commit.

**Teeth proven, not assumed.** A deliberate one-line timer break (`TAP[3] 7→6`) turns the
Mooneye gate red; a one-line background-scroll break moves 26 Mealybug rows. Re-run those two
if you ever doubt the gate — and note the sharp edge that cost real time here: the scouts are
`cargo build` targets, so **after breaking source for a teeth test you must rebuild** before
trusting any later measurement. A stale binary once reported `timer 11/13` and looked exactly
like a regression on `main`.

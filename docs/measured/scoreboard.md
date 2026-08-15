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
| SameSuite | <!--score:samesuite:total-->76<!--/score--> | **<!--score:samesuite:pass-->72<!--/score-->** | the ROM's own `RESULT_CODE` at `$CFFE` |
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

Run them for real with `make test-external` (all four external suites, ~1.5 s) and
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

### The `identical` picture mode scores differently — and now dominates

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
blocking a default flip. It is fixed — see
[mealybug.md §9](../mealybug.md#9-the-line-0-stat-phase-679--1279-and-the-last-blocking-row).
The mode is gated separately (`make test-external-accurate`, against
`testharness/mooneye_accurate_baseline.txt`) so progress *and* regression are both visible.

**Measured mode-for-mode, every gate, same build** — every figure in this paragraph is gated:

| Gate | `standard` | `identical` |
|---|---:|---:|
| In-repo harness (Blargg + dmg-acid2) | <!--score:harness-->46/47<!--/score--> | <!--score:harness-->46/47<!--/score--> — *identical verdicts row for row* |
| Mooneye `acceptance` | <!--score:mooneye_acceptance-->67/75<!--/score--> | **<!--score:mooneye_acceptance_accurate-->75/75<!--/score-->** |
| Mooneye `emulator-only` | <!--score:mooneye_emulator_only-->28/28<!--/score--> | <!--score:mooneye_emulator_only_accurate-->28/28<!--/score--> |
| Mooneye `misc` (CGB) | <!--score:mooneye_cgb-->8/8<!--/score--> | <!--score:mooneye_cgb-->8/8<!--/score--> |
| SameSuite | <!--score:samesuite-->72/76<!--/score--> | <!--score:samesuite-->72/76<!--/score--> |
| `cpp` | <!--score:cpp-->4/4<!--/score--> | <!--score:cpp-->4/4<!--/score--> |
| Mealybug — rows pixel-exact | <!--score:mealybug-->3/79<!--/score--> | **<!--score:mealybug_accurate-->17/79<!--/score-->** |
| Mealybug — pixels wrong *(lower is better)* | <!--score:mealybug:px-->1,212,282<!--/score--> | **<!--score:mealybug_accurate:px-->630,266<!--/score-->** |

(Rows with one baseline are scored once: the picture mode does not reach what they measure, so
the same figure stands in both columns.)

**No row goes the other way**, so the accuracy precondition for making `identical` the default
is met; what remains is a cost decision — **~2.1x**, 0.221 → 0.461 ms/frame, 36x real time
headless, re-measured on every `cargo test --release --test ppuengines`.

> This table is where the drift happened. Its Mealybug figure read **12** for as long as it
> took somebody to notice, five rows after `identical` reached
> <!--score:mealybug_accurate-->17/79<!--/score-->. The row is generated now.

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

## SameSuite — four evidenced reds

**<!--score:samesuite-->72/76<!--/score-->.** All four reds are APU rows, and each is red for a
stated reason rather than for want of effort:

| Row | Why it is red |
|---|---|
| `apu/channel_4/channel_4_freq_change` | **SameSuite's own `apu/README.md` says SameBoy fails it too**, and the ROM's header says *"the logic behind it is still unclear."* A row the suite's author does not pass has no reference implementation to calibrate against. |
| `apu/channel_1/channel_1_freq_change_timing-A` | `-A` is a **Game Boy Advance**, a console this emulator does not model. Every CGB stepping tag in the suite is spelled `cgb…`; passing this would be an accident — and we briefly did, which is how the frequency latch's lower bound was found. |
| `apu/channel_1/channel_1_freq_change_timing-cgb0BC` | CGB revisions ≤ C. It differs in the **PCM12 read-back**, not the frequency timer — a glitch the suite's README lists as understood and documented by nobody. |
| `apu/channel_1/channel_1_sweep_restart` | **143 of its 144 cells match.** The one that differs wants `SQUARE_RESTART_SPEEDUP = 0`, which was measured: it costs four other green rows. See [apu.md](apu.md#the-constants-and-what-moving-each-one-costs). |

And a counterpoint that keeps the first row honest: we **pass**
`apu/channel_1/channel_1_sweep_restart_2`, which is the *other* test that same README names
SameBoy as failing. The suite is not simply harder than us in one direction.

Group totals: `apu` **<!--score:samesuite_apu-->65/69<!--/score-->** · `dma` **<!--score:samesuite_dma-->4/4<!--/score-->** · `ppu` **<!--score:samesuite_ppu-->1/1<!--/score-->** · `sgb` **<!--score:samesuite_sgb-->2/2<!--/score-->**.

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
| `standard` (default) | <!--score:mealybug-->3 / 79<!--/score--> | <!--score:mealybug:px-->1,212,282<!--/score--> |
| `identical` (`GB_PPU=identical`) | **<!--score:mealybug_accurate-->17 / 79<!--/score-->** | **<!--score:mealybug_accurate:px-->630,266<!--/score-->** |

The baseline pins the **pixel count per row**, not just PASS/FAIL, precisely because at
<!--score:mealybug-->3/79<!--/score--> a
pass/fail file is a flat wall that cannot show a rewrite getting closer. Consequence to expect:
**any PPU change turns that file red.** Read the diff, re-bless with the new counts in the same
commit — that is the point.

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

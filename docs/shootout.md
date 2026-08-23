# Getting listed on the GB Emulator Shootout

The [GB Emulator Shootout](https://gbdev.io/GBEmulatorShootout/) is the
community's public accuracy board: 264 test ROMs, 18 emulators, one screenshot
comparison each. This page is the whole of our submission — what the runner
requires, what we had to build to satisfy it, what we would score, and the
files to drop into a fork of `gbdev/GBEmulatorShootout`.

**Nothing here is opened without the captain's word.** It is outward-facing and
carries the project's name.

---

## 1. Where we would land

Measured against the shootout's own ROMs, its own reference screenshots and its
own pass criterion, re-scored from a fresh clone of the shootout on
**2026-08-21** and reconfirmed with a live `shootout_scout` run on
**2026-08-22** (the `m3_wx_5_change`/`m3_wx_6_change` fix below had only been
reasoned about arithmetically until this run — see the former dagger note,
now resolved):

> ### **243 / 264 — 92.0 %**
>
> (**223 / 264 — 84.5 %** on `standard`, which is where this page stood until
> 2026-08-21. The `standard`/`identical` flip that day was **fifteen gains and
> zero losses** row for row, the fifteen named in §1a, taking it from 223 to 238;
> the further five rows to 243 are later `identical`-only picture work, two of
> them [`mealybug.md` §16](mealybug.md#16-m3_wx_5_changem3_wx_6_change-it-was-never-a-re-trigger-it-was-a-stale-register-read-21-aug-2026).)

That is **5th of 19** by pass count and by percentage, four rows behind fourth:

| | emulator | score | % |
|---|---|---:|---:|
| 1 | docboy | 258/261 | 98.9 |
| 2 | Emulicious | 254/261 | 97.3 |
| 3 | SameBoy | 253/264 | 95.8 |
| 4 | Beaten Dying Moon | 247/264 | 93.6 |
| **5** | **this emulator** | **243/264** | **92.0** |
| 6 | GSE | 218/264 | 82.6 |
| 7 | bgb | 205/264 | 77.7 |
| 8 | GambatteSpeedrun | 169/207 | 81.6 |
| 9 | mGBA | 150/264 | 56.8 |
| 10 | gameroy | 149/167 | 89.2 |
| … | binjgb, VBA-M, PyBoy, Emmy, Goomba, No$gmb, VBA, KiGB, ares | 144 → 26 | 69.6 → 10.0 |

<sup>Their column is whatever the board showed on 2026-06-04 (scraped from the
live page, not copied from an earlier report). Denominators differ because an
emulator that declines a model or a feature is not scored on those rows —
gameroy is DMG-only, GambatteSpeedrun/binjgb/VBA-M declare no `PCM`.</sup>

By suite:

| suite | ours | `standard` was | notes |
|---|---:|---:|---|
| `blargg` | **51 / 51** | 51 / 51 | including `oam_bug` 7/7 and `interrupt_time` |
| `mooneye` | **97 / 97** | 89 / 97 | acceptance **66/66** · emulator-only 28/28 · misc 2/2 · manual-only 1/1. **Every red here was a PPU mode-3 / LCD-on row, and the per-dot engine takes all eight** |
| `samesuite` | 66 / 68 | 66 / 68 | `channel_1_sweep_restart`, `channel_4_freq_change` — both red on measured evidence, see `docs/measured/apu.md` § "Three rows left red, all on evidence" |
| `cpp` | 4 / 4 | 4 / 4 | MBC3/RTC + `sgb-ext-test` |
| `ax6` | 3 / 3 | 3 / 3 | `rtc3test` |
| `acid` | **5 / 5** | 4 / 5 | `cgb-acid-hell` closed on 2026-08-21 — the Color `TILE_SEL` glitch, [`mealybug.md` §13](mealybug.md#13-tile_sel-reset-on-a-bitplane-read-the-tile-index-is-the-data-21-aug-2026); `cgb-acid2` is pixel-exact on **both** engines since the Color gate on the window pixel-insertion glitch |
| `daid` | 5 / 9 | 5 / 9 | `ppu_scanline_bgp` ×2, `speed_switch_timing_{ly,stat}` — a PPU-vs-counter phase error |
| `ashiepaws` | **1 / 3** | 0 / 3 | `strikethrough.gb` — see the 2026-08-21 entry of [`conformance-notes.md`](conformance-notes.md); `bully.gb` ×2 stay red and are **not** the CGB-mode boot counter — that was changed to `$1EA0` and their rows are byte-identical either side of it; the residual is the PPU's hand-off phase (see §2) |
| `mealybug-tearoom-tests` | **11 / 24** | 1 / 24 | the DMG column of [`docs/mealybug.md`](mealybug.md); of the 13 still red, **9 cannot be won by PPU work at all** — see §1b |
| **total** | **243 / 264** | 223 / 264 | |

**Every one of the 21 failures is a documented open area, and 9 of them are not
ours to win.** Mealybug 13 (9 blocked on the boot-ROM glyph — §1b), Mooneye 0,
`daid/ppu_scanline_bgp` ×2, `daid/speed_switch_timing_{ly,stat}`,
`ashiepaws` 2, SameSuite 2. There is no surprise in the list, and §1c measures
every one of them so the next crew can start at the cheapest.

<sup>`ashiepaws/strikethrough.gb` was the 26th until 2026-08-21, when the OAM
DMA was given the address bus it drives — the row is 7 pixels on one scanline
and the whole of it is that a DMA running under an object scan stops the PPU
reading OAM. It also moved four Gambatte `oamdma/late_sp*` rows, which are the
same behaviour measured with resolution instead of pixels.</sup>

<sup>`m3_wx_5_change` and `m3_wx_6_change` — two of §1b's six genuine
mid-scanline rows — went pixel-exact on 2026-08-21
([`mealybug.md` §16](mealybug.md#16-m3_wx_5_changem3_wx_6_change-it-was-never-a-re-trigger-it-was-a-stale-register-read-21-aug-2026)).
This table used to carry that as a same-day arithmetic prediction with a
dagger, unconfirmed by the real screenshot judging; **confirmed by a live
`shootout_scout` run on 2026-08-22** — 243/264, `mealybug-tearoom-tests`
11/24, no other row moved. The dagger is gone because there is nothing left
to confirm.</sup>

### 1a. The fifteen rows the default flip moved

Scored through the same manifest, same build, `GB_PPU` the only difference:

| | rows |
|---|---|
| **gained** | `mooneye/acceptance/ppu/` **hblank_ly_scx_timing-GS, intr_2_0_timing, intr_2_mode0_timing, intr_2_mode0_timing_sprites, intr_2_mode3_timing, intr_2_oam_ok_timing, lcdon_timing-GS, lcdon_write_timing-GS** · `mealybug/ppu/` **m3_bgp_change, m3_lcdc_win_en_change_multiple, m3_scx_high_5_bits, m3_window_timing, m3_window_timing_wx_0, m3_wx_4_change, m3_wx_4_change_sprites** (all DMG) |
| **lost** | none |

`acid/cgb-acid2` was the one loss while it stood, at 158 pixels; it is closed
and the row passes on both engines.

### 1b. Thirteen of the sixteen remaining Mealybug rows are not winnable here

[`docs/mealybug.md` §11](mealybug.md#11-the-denominator-45-of-the-failing-rows-cannot-be-won-by-ppu-work-15-aug-2026)
sorts every Mealybug failure by what it would take to close. Against the
shootout's DMG-only 24-row population that gives:

* **9 rows** need the boot ROM's `®` glyph in VRAM at hand-off (block A) —
  `m3_bgp_change_sprites`, `m3_lcdc_bg_map_change`, `m3_lcdc_obj_en_change`,
  `m3_lcdc_obj_en_change_variant`, `m3_lcdc_tile_sel_change`,
  `m3_lcdc_tile_sel_win_change`, `m3_lcdc_win_map_change`, `m3_obp0_change`,
  `m3_scx_low_3_bits`. The emulator supports a user-supplied boot ROM and the
  gated score deliberately never uses one; shipping the glyph is a licence call,
  not a PPU one.
* **6 rows** were genuine mid-scanline error and were the ones to chase; **two
  are now closed**, and this table is corrected for §15's own same-day fix to
  a third (`m3_lcdc_win_en_change_multiple_wx`, whose figure had gone stale in
  the hours between that fix landing and this page being written):

  | row | pixels wrong | group ([`mealybug.md` §11.4](mealybug.md#114-the-twelve-winnable-rows-grouped-by-the-code-they-live-in)) |
  |---|---:|---|
  | `m3_lcdc_bg_en_change` | **4** | G — one pixel at `x = 0` on four lines |
  | `m3_lcdc_obj_size_change` | 81 | F — the object fetch. Its sibling `m3_lcdc_obj_size_change_scx` was closed on 2026-08-21 ([§12](mealybug.md#12-the-objects-high-bitplane-reads-lcdc2-a-dot-later-21-aug-2026)); **this one cannot follow by the same route, and §12.3 proves it** — the fault is the head-of-line OBJ penalty, not the register |
  | `m3_scy_change` | 257 | E — tile 1 renders the previous line's `SCY` |
  | `m3_lcdc_win_en_change_multiple_wx` | 437 | D — window / `WX`, closed *partway* on 2026-08-21 ([§15](mealybug.md#15-the-signed-position-refactor-check_window_startcheck_window_stop-share-one-grid-21-aug-2026)) — the LCDC.5-toggle sibling of the fix below, still needing `check_window_stop`'s own resume unified with the same grid |
  | ~~`m3_wx_6_change`~~ | **0** — closed 2026-08-21 | D — [§16](mealybug.md#16-m3_wx_5_changem3_wx_6_change-it-was-never-a-re-trigger-it-was-a-stale-register-read-21-aug-2026): the activation check was reading `WX` a dot too soon, not refusing to re-trigger |
  | ~~`m3_wx_5_change`~~ | **0** — closed 2026-08-21 | D — same fix as above |

The reachable ceiling on this suite's shootout population is still **15 of
24** and the published total's ceiling is still **247 / 264** if the block
lands in full — two of the original six rows are now part of the confirmed
11/24 (§1, §16) rather than still owed, which is why the ceiling itself did
not move even though the current total did. Anybody planning work against this
row block should plan against the remaining four, and start at the top of
that table.

### 1c. The remaining 25, measured rather than assumed (21 Aug 2026)

**Two of the entries above were stale and this section is the correction.** Every
row here was re-measured on the day, against the shootout's own ROMs and its own
50-luma tolerance, and the cheapest-per-row order the next crew should work in
falls straight out of it.

| row(s) | distance today | what it actually needs |
|---|---:|---|
| `mealybug/m3_lcdc_obj_size_change` | 81 px | group F — and **not** the fix that closed its sibling; [`mealybug.md` §12.3](mealybug.md#123-why-m3_lcdc_obj_size_change-cannot-follow-with-the-arithmetic) shows no `LCDC` visibility dot can satisfy it |
| `mealybug/m3_scy_change` | 257 px | group E |
| `mealybug` × 3 (window / `WX`) | 1,273 · 4,792 · 13,660 px | group D |
| `mealybug` × 9 | whole-screen | the boot ROM's `®` glyph — §1b, a licence call |
| `daid/ppu_scanline_bgp` (DMG) | **crashes, not a pixel diff** | needs a real boot ROM — see below |
| `daid/ppu_scanline_bgp` (GBC) | **2,204-3,309 px** | not a near miss — see below |
| `daid/speed_switch_timing_{ly,stat}` | — | the PPU-vs-counter phase error `CLAUDE.md` records under `STOP` |
| `ashiepaws/bully.gb` ×2 | whole screen | **not** the CGB-mode boot counter any more — that was changed to `$1EA0` and this row is byte-identical either side of it; the residual is the PPU's hand-off phase, which no ROM pins ([`conformance-notes.md`](conformance-notes.md), 2026-08-23) |
| `samesuite` ×2 | — | red on measured evidence, [`measured/apu.md`](measured/apu.md) |

**`cgb-acid-hell` is CLOSED — 21 Aug 2026 — and the whole trace that closed it is
[`docs/mealybug.md` §13](mealybug.md#13-tile_sel-reset-on-a-bitplane-read-the-tile-index-is-the-data-21-aug-2026).**
The two pixels were `(80, 68)` and `(80, 69)`, both engines gave the same wrong
answer, and the earlier analysis in this section was right about everything
except the conclusion: it recorded that hardware's high bitplane is `$55` on
line 68 and `$49` on line 69, and in the same breath that the tilemap holds
`$98A0 = $55` and `$98C0 = $49` there. The ROM clears `LCDC.4` on the very dot of
the bitplane read, and every Color stepping but CPU CGB D then **uses the tile
index as the bitplane data** (Matt Currie, *The Comprehensive Game Boy PPU
Documentation*, `TILE_SEL`). The two exhaustive negatives that pointed away from
it — the commit schedule swept over all eight configurations, and (added when it
was closed) the Color fetcher's three read dots swept over all eight — were both
correct and both about *timing*, which is what should have said the byte was not
in the tile at all.

**`daid/ppu_scanline_bgp` is not a near miss on either console, and the DMG row
is not even a pixel comparison any more — measured 22 Aug 2026, re-checked
against a live `shootout_scout` run and not merely reasoned about.** The DMG row
now **faults**: `terminalgb: FAULT — the CPU executed undefined opcode 0xfc at
0x80cc`. That address is tile `$0C`'s 13th byte inside VRAM's `$8000`-block
(`$8000 + 16×12 + 12`), i.e. inside the **boot-logo tile region**
`set_boot_logo_tiles` decompresses the cartridge's own header bytes into (the
Blocker-C fix, §8.6 of `mealybug.md`) — and the ROM's own, fixed, ROM-resident
`$0040` VBlank vector is `JP $80C3`: it jumps **straight into that VRAM tile
data as code**, three bytes into tile `$0C`, within the first ~6,200
instructions of boot. Traced instruction by instruction: the eight bytes from
`$80C3` to `$80CB` are the correctly-decompressed logo's own zero rows (`NOP`
×8, present from the very first instruction, never written by the game), and
`$80CC` is `$FC` — the first non-zero nibble the standard Nintendo logo
decompresses to at that offset. **Independently re-derived by hand from the
documented algorithm against this ROM's own (confirmed-standard) header
bytes, byte for byte**, so the decompression is not the defect — `$FC` is
exactly what the documented, licence-clean algorithm computes there, and it is
what a real boot ROM's decompression would produce too. The technique itself
is boot-ROM-dependent: it relies on VRAM holding *exactly* what a real DMG
boot ROM leaves behind at the moment it hands off, which is precisely the
gap `set_boot_logo_tiles` documents that it cannot close (`mealybug.md` §8.6)
without embedding real Nintendo boot-ROM bytes — explicitly out of scope. The
policy-correct answer already exists and is deliberately not wired into this
scout: a user-supplied `$GB_BOOT_ROM` (`src/boot.rs`), which every published
score in this repository — this one included — is measured *without*, on the
same "gated suites must never pick one up automatically" rule §8.6 states for
`examples/mealybug --boot-rom`. No fix is appropriate here; this row stays red
for a reason outside the emulator's control. **The GBC row does not crash**
(`set_boot_logo_tiles` is gated to `is_dmg()` machines only, so Color VRAM at
that address is genuinely zero, and the vector reads eight `NOP`s and one more
byte harmlessly) and keeps its older pixel-diff character: it writes `BGP` ten
times across mode 3 of every line to draw a picture, and ours has **the right
structure** — the band boundaries and the one-line anomaly at `LY = 47` line up
exactly — but **2,755 to 3,309 pixels over tolerance across 129 of the 144
lines**, against all three of its DMG-shaped references. Sliding our frame
horizontally to look for a pure phase error gets the best match at **+4 pixels
and still 2,204 px**, so it is a phase error *and* a palette-content error, not
one or the other. The suite ships three references because the ROM's own
timing drifts frame to frame; matching any one of them passes. Neither
finding moves the `daid` 5/9 total — both rows were already counted as
failing before this measurement, on both engines; only *why* was stale.

#### The shootout's Mooneye population is not ours — 97 rows, not 103

**`0e099b0` moved our Mooneye score by eleven rows and moved this number by
zero, and that is correct rather than a measurement error.** The shootout
comments out **all fifteen** model-variant ROMs in its `testroms/mooneye.py`
— nine from `acceptance` (`boot_div-dmg0`, `-S`, `boot_div2-S`,
`boot_hwio-dmg0`, `-S`, `boot_regs-dmg0`, `-mgb`, `-sgb`, `-sgb2`) and six from
`misc` (`bits/unused_hwio-C`, `boot_div-A`, `boot_div-cgb0`, `boot_hwio-C`,
`boot_regs-A`, `ppu/vblank_stat_intr-C`). Those fifteen are precisely the
population that commit fixed: ten of its eleven rows are boot ROMs for the DMG0,
MGB, SGB, CGB 0 and AGB, and the eleventh is `misc/ppu/vblank_stat_intr-C`.

So the shootout's `acceptance` denominator is **66**, not our 75, and its `misc`
denominator is **2**, not our 8. Verified both ways after the rebase: 264 rows
scored, **zero changed verdict in either direction**.

The general rule, and it is the same trap the SameSuite notes already record:
**check which population a number belongs to before comparing two of them.**
Our gates score every ROM the suite ships; the shootout scores the subset it
chose. Neither is wrong and they are not interchangeable.

### Reproduce it

```sh
git clone https://github.com/gbdev/GBEmulatorShootout
python3 tools/shootout/manifest.py GBEmulatorShootout > manifest.tsv
cargo run --release --features image --example shootout_scout -- manifest.tsv
```

Seventeen seconds, no Windows, no display.
[`examples/shootout_scout.rs`](../examples/shootout_scout.rs) uses
their model selection, their frame budget, their ~10 Hz polling with early
exit, their `INFO`-is-not-a-failure rule, and their `compareImage` — a **50/255
greyscale tolerance**, which is what lets our DMG shades
(`000000/606060/c0c0c0/ffffff`) match references drawn in
`000000/555555/aaaaaa/ffffff`.

> **Trap for anyone regenerating the manifest.** `testroms/mealybug.py` builds
> three lists — DMG, CGB RevC, CGB RevD — and ends with
> `all = dmgs # + cgbcs + cgbds`. Collecting every `Test(...)` a module
> *constructs* rather than what it puts in `all` gives 311 rows, not 264,
> and 47 of them are never run by anybody. The published table has 264 rows and
> our manifest reproduces that set exactly, name for name.

---

## 2. How they measure, and why it dictated the work

`emulator.py` launches the emulator as a **GUI process on Windows**, finds its
window by title, and screenshots the *client rect* with `pyautogui` every
100 ms until a reference matches or the budget runs out. There is no serial
capture and no register check anywhere in the project — Blargg's `cpu_instrs`
is judged by whether the emulator draws the same "Passed" screen as the
reference image.

Two consequences worth stating plainly:

* **The comparison covers the whole screen, including boot-register dumps and
  palette.** Nine Mooneye tests print their boot registers, so a DMG row run in
  a Color console's compatibility mode is marked FAIL however correct the CPU
  is. A real DMG boot mode is therefore worth real rows, and it is emulation
  work, not packaging. That work landed some time ago (`src/mode.rs`,
  `Model::{Auto,Dmg,Cgb,Sgb,Sgb2}`) and is most of why the score moved.
* **Nothing may be special-cased for their runner.** Every flag added below is
  an ordinary flag a person can use, and the machine it produces is the machine
  everyone else gets.

---

## 3. What the runner requires of us — the checklist, re-verified

Against `gbdev/GBEmulatorShootout` at `38b926b`:

| requirement | evidence in their code | status |
|---|---|---|
| Windows binary as a **GitHub release asset** matched by name | `util.py:downloadGithubRelease(..., filter=lambda n: "win" in n)` | **built**: [`.github/workflows/release.yml`](../.github/workflows/release.yml) publishes `terminalgb-windows-x64.zip`. Not yet run — needs a tag. |
| builds and runs on `windows-latest` | `emulator-runner.yml` | **type-checked**: `cargo check --target x86_64-pc-windows-gnu --features audio --example desktop --example terminal` is clean. The release workflow builds natively on `windows-latest` and smoke-tests the result. Not yet *run* on Windows. |
| GUI window with a stable title | `emulator.py:16` `title.startswith(name)` | **done**: `screen::desktop::WINDOW_TITLE = "TerminalGB"`, pinned by a unit test; everything the frontend appends comes after it |
| client area exactly 160×144 | `util.py:getScreenshot` → `GetClientRect` | **done**: `--scale 1` fixes the client area in **device** pixels and makes the window non-resizable |
| ROM path as argv, no dialog | every adapter | already worked |
| model selection DMG / CGB / SGB | `startProcess(rom, model=…)` | **done**: `--model dmg\|cgb\|sgb\|sgb2\|auto` |
| turbo, so the suite finishes in CI | `self.speed`; gameroy uses 15.0 | **done**: `--speed 15` |
| optional `PCM` feature declaration | `Emulator(features=(PCM,))` | **done**: `$FF76`/`$FF77` are implemented (`src/sound.rs`), so all 57 SameSuite PCM rows are run rather than skipped — we pass 55 of them |

### Two things the checklist did not list, and both would have been fatal

1. **The emulator paused when its window lost focus.** A runner launched from a
   script may never hand the window focus at all — Windows refuses focus
   changes requested by a background process — and a screenshot of a paused
   emulator is a screenshot of the title screen. Nothing about it is visible
   from the outside; it would have read as a catastrophic emulation failure.
   Measured on the real frontend, two instances racing for focus over ~11 s,
   counting frames through `--video-out`:

   | | emulated frames | |
   |---|---:|---|
   | with `--run-unfocused` | **653** | ≈ real time (59.7 Hz) |
   | without | **29** | stopped the instant it lost focus |

   Hence `--run-unfocused`. The default is still to pause.

2. **A build with no sound device had no APU at all.** `Sound::do_cycle`
   returns immediately while no player is installed, so a frontend that only
   called `enable_audio` when it found a device was not running a quiet Game
   Boy — it was running one with **no sound chip**: `NR52` reads back powered
   off, the frame sequencer never advances, PCM12/PCM34 report nothing. That is
   observable from the cartridge, and it is worth roughly **85 rows** here
   (`dmg_sound` 12, `cgb_sound` 12, `interrupt_time`, and 57 SameSuite APU
   ROMs). `sound::SilentPlayer` closes it: the desktop frontend now always
   installs an APU, real or silent. The release build also ships
   `--features audio` so the real one is there.

---

## 4. The flags this added

All on the desktop frontend (`examples/desktop`,
`screen::desktop::DesktopOptions`). Defaults reproduce the old behaviour
exactly.

```
--model <console>         auto (default), dmg, cgb, sgb, sgb2
--scale <N>               fixed client area of N x 160 x N x 144, non-resizable
--upscale <algo>          none, scale2x, mmpx, lcd — overrides config.toml
--upscale-factor <N>      overrides config.toml
--speed <N>               emulation speed, 1 = real time (e.g. 15 for turbo)
--run-unfocused           keep emulating when the window is not focused
```

`--scale` and `--upscale` are a pair: `--scale` sizes the *window*,
`--upscale` decides what is drawn into it. `--scale 1 --upscale none
--upscale-factor 1` is the combination that puts the untouched 160×144 frame in
a 160×144 client area **whatever a user's `config.toml` says** — which is the
whole reason a flag is needed, since the frontend is otherwise sized by the
configured upscaler.

`--speed` shortens the emulated frame period but not the draw period: above 1×
the extra frames are emulated in a burst inside one draw rather than drawn.
Drawing at 15× would mean 900 texture uploads and buffer swaps a second, and on
the software OpenGL stack a CI runner has, *that* would be what limits the
speed. Emulated frames are never dropped; displayed ones are — the same trade
the terminal frontend's speed ladder makes.

### Proven end to end, not just in a parser

The headless score above is a core measurement. The GUI path was checked
separately, by capturing the real frontend's window on X11:

```sh
./target/release/examples/desktop dmg-acid2.gb \
    --model dmg --scale 1 --upscale none --speed 15 --run-unfocused
# window title:  "TerminalGB"
# import -window <id> shot.png  ->  160 x 144
# vs testroms/acid/dmg-acid2.png, PIL greyscale: max pixel difference 22
#                                (the shootout's PASS threshold is <= 50)
```

That 22 is our `c0c0c0` against their `aaaaaa`, i.e. the palette difference and
nothing else. The combined `blargg/cpu_instrs` ROM — ~31 s of emulation at 1× —
reached "Passed all tests" in under 10 s at `--speed 15` in the same window.

**Residual risk, stated:** this was measured on Linux/X11 with Mesa. Nobody has
run the Windows binary on Windows yet. The picture path is identical
(framebuffer → GL texture at 1:1), so the risk is in window sizing and DPI, and
the shootout already calls `setDPIScaling` on every emulator it runs.

---

## 5. The submission

Files are in [`docs/shootout/`](shootout/), ready to copy into a fork of
`gbdev/GBEmulatorShootout`:

| file | goes to |
|---|---|
| `terminalgb.py` | `emulators/terminalgb.py` |
| `main.py.snippet` | one entry appended to `EMULATOR_SPECS` in `main.py`, one to `matrix.include` in `.github/workflows/ci-all.yml` |
| `ci-terminalgb.yml` | `.github/workflows/ci-terminalgb.yml` |

Verified by putting them in a real checkout and driving them through their own
`main.py`: the spec resolves, the keyword filter finds it, `title_check`
accepts `"TerminalGB"` and `"TerminalGB — nearest x1 — vol 50%"` and rejects
`"SameBoy"`, and `startProcess` returns a correct argv for all three models.

### Steps, when the captain says go

1. **Tag a release** (`v0.3.0` or whatever the version is by then). The
   Release workflow builds Windows and Linux zips and publishes them. Check the
   asset is named `terminalgb-windows-x64.zip` — `"win" in name` is what the
   runner filters on, and `require_asset=True` in the adapter makes a rename
   fail loudly rather than silently downloading the source zipball.
2. Fork `gbdev/GBEmulatorShootout`, copy the three files, run
   `python main.py --emulator TerminalGB` on a Windows machine or via the
   `CI - TerminalGB` workflow, and check the resulting `terminalgb.json`.
3. Open the PR. Merges are not instant — one third-party PR has sat approved
   for a while — so expect weeks, not days.

### The name on the board

**`TerminalGB`.** This branch originally used `gameboy` and flagged the choice
as the captain's, because a board entry called "gameboy" sits next to SameBoy,
bgb, mGBA and Emulicious and a reader cannot tell it from the console. The
rename landed on `main` while this was in progress (`def8db4`), which settles
it: the submission files say `TerminalGB` throughout, and so does
`screen::desktop::WINDOW_TITLE`.

Two things stay `gameboy` on purpose, and both are load-bearing:

* **The repository slug is `Alchemy86/TerminalGB`** (renamed from
  `Alchemy86/gameboy`; GitHub redirects the old slug). `downloadGithubRelease`
  takes the slug, not the name — `docs/shootout/terminalgb.py` carries the new one.
* **The window title and the board entry are separate strings.** The adapter's
  `title_check` matches `WINDOW_TITLE`; the entry's display name is the `name`
  in the spec. Changing either later does not force the other.

---

## 6. Recommendation on timing: **submit**

An earlier scouting report recommended waiting, on the grounds that publishing
92/264 beside SameBoy's 253 would read worse than we are. That reason is gone.

* We are at **243/264**, which is 5th of 19 and ahead of GSE, bgb, mGBA and
  binjgb, four rows behind fourth. Nobody reading that table would think we were
  padding it.
* The board is a snapshot with a date on it, and each emulator has its own
  `workflow_dispatch` CI job — **re-scoring is one button, not a new PR**. So
  the Mealybug campaign's gains land on the board whenever we ask for them, and
  waiting for it buys nothing except the wait. That prediction has now been
  tested once: the default picture-mode flip moved this number by fifteen rows
  and needed no change to anything the shootout runs, because the flag it
  changed is an ordinary one a person can type.
* Merges are slow. Starting the clock now means the Mealybug work is likely to
  land *before* the PR is merged, not after.
* Every remaining failure is a documented open area we can name, which is a
  better position to be reviewed in than a score with unexplained holes.

The one thing worth doing first is **cutting the release and running the
`CI - TerminalGB` workflow in our own fork**, so the PR arrives with a real
`terminalgb.json` behind it rather than a promise. That is step 1 and 2 above and
costs a day at most.

---

## 7. What is deliberately not done

* **No PR is opened.** By instruction, and correctly: it carries the project's
  name.
* **No release is tagged.** Tagging publishes binaries under our name; that is
  the captain's act, not a crew's. The workflow's manual trigger produces a
  **draft** release for exactly this reason.
* **The Windows binary has not been run on Windows.** There is no Windows
  machine here. The cross-target check is clean and the smoke test in the
  release workflow runs the vendored MIT dmg-acid2 ROM on the runner itself, so
  a build that cannot start will fail the release rather than reach a PR.
* **`ashiepaws/bully`, `strikethrough` and the PPU rows are not chased here.**
  They belong to the PPU campaign, and pretending otherwise in a submission
  would be worse than a 41-row deficit anyone can read off the table.

# The accuracy baseline, measured 2026-08-25

Measured on `main` at `c575bb4` (the CGB LCDC double-speed write-commit fix), on a
20-core box, load average 6.0 at the start of the run. **Every figure here was produced
by running the suite, not copied from a document** — which is the repository's own
standing rule, and it matters: `AGENTS.md`'s closing figures happened to agree exactly,
but that is a fact you can only state after re-running.

## The submodule check, done explicitly

`testharness/testroms.rs` panics without the `tests` submodule, and a `cargo test` that
silently skips the accuracy harness looks green while proving nothing. Checked:

```
$ git submodule status
 c240dd7d700e5c0b00a7bbba52b53e4ee67b5f15 tests (heads/master)
$ find tests -type f | wc -l
230
```

Pinned at `c240dd7d` — the commit `docs/conformance-action.md` names — with all five
Blargg families present (`cpu_instrs`, `mem_timing`, `halt_bug.gb`, `oam_bug`,
`dmg_sound`). The external suites were already fetched under `target/test-suites/`
(mooneye, mealybug, samesuite, cpp, csp). **The harness really runs the ROMs.**

## Totals, non-`SKIP` rows, across every checked-in baseline

| | `standard` | `identical` (default) |
|---|---:|---:|
| **TOTAL** | **3696 / 6210** | **4296 / 6210** |

Both modes reproduced every baseline file exactly (`make test-external` and
`make test-external-accurate` each ended green on all 18 suite groups).

### Where the failures are

| suite | `standard` | `identical` |
|---|---:|---:|
| gambatte | 3176/5225 | 3622/5225 |
| gbmicrotest | 252/513 | 339/513 |
| mooneye acceptance + emulator-only | 95/103 | 103/103 |
| mooneye misc (cgb) | 8/8 | 8/8 |
| mooneye-wilbertpol | 62/117 | 82/117 |
| samesuite | 73/76 | 73/76 |
| mealybug | 3/79 | 29/79 |
| age | 3/59 | 11/59 |
| scribbltests | 7/9 | 9/9 |
| strikethrough | 0/2 | 2/2 |
| bully | 0/1 | 0/1 |
| cgb-acid-hell | 0/1 | 1/1 |
| everything else | full marks | full marks |

## What the failures actually are — measured, not assumed

**1,369 of the 1,603 `identical` gambatte failures reported `not drawn`, and that was
misleading on 99 % of them.** Probing what the frame really contained:

| what we drew | rows |
|---|---:|
| a clean, valid hex digit — just the wrong one | **1,289** |
| a shape that is not one of Gambatte's sixteen glyphs | 69 |
| genuinely blank | **11** |

So these ROMs run, reach their output stage, and disagree with hardware by a *value*.
That is fixed in this branch — see the gate note below.

Cross-tabulating want against got, two families dominate and they are unrelated:

- **`3 ↔ 0`, 408 rows**, dominated by `sprites` (226) and `window` (98). This is the
  documented mid-scanline fetcher wall (`docs/mealybug.md` §8.9).
- **`2 ↔ 0`, 226 rows**, and *every* top group is a STAT-interrupt group — `m2enable`,
  `miscmstatirq`, `lycEnable`, `m0enable`, `halt`, `m2int_m0irq`, `lyc153int_m2irq`. The
  near-perfect symmetry (113/113 for `0↔2`) is the signature of a boundary landing on the
  wrong side, not of a missing feature. See
  [`stat-line-phase-2026-08-25.md`](stat-line-phase-2026-08-25.md).

## Mealybug's remaining headroom is smaller than the row count suggests

Of the 50 `identical` failures, the ten ROMs that reference tile `$19` are blocked by the
settled no-boot-ROM policy. Verified rather than assumed: `reset_vram` in the suite's own
`inc/utils.asm` clears `$9800-$9BFF` — the **tilemap** — and never touches tile data, so
the boot ROM's `®` glyph survives into every one of them:

```
m3_lcdc_obj_en_change_variant  m3_lcdc_tile_sel_change  m3_lcdc_tile_sel_win_change
m3_lcdc_win_map_change  m3_obp0_change  m3_bgp_change_sprites  m3_wx_4_change_sprites
m3_scx_low_3_bits  m3_lcdc_bg_map_change  m3_lcdc_obj_en_change
```

That accounts for most of the near-misses that look tempting in a sorted list —
`m3_bgp_change_sprites` at 972 px on all three consoles is not a sprite-timing bug, it is
the same glyph missing from nineteen sprites and an eighteen-tile background column.

## Reproducing

```
make test-external            # standard
make test-external-accurate   # identical
```

Totals recomputed from the baseline files with a short script; the per-suite numbers above
are that script's output.

# Pool sweep, 2026-08-25 — nothing is stranded

Every worktree under `/home/aaron/.treehouse/gameboy-795d23/` and
`/home/aaron/.treehouse/gameboy-b03097/`, and every `fm/gameboy-*` branch, checked
against the **real** `main` at `c575bb4`. **Result: no lost work.** Everything below is
already on `main`, most of it in a more developed form than the copy in the pool.

## THE ANCESTRY NUMBERS ARE MEANINGLESS HERE, AND MORE SO THAN EXPECTED

The brief warned that a pooled clone's own `main` ref goes stale, so `main..HEAD` counts
of 300+ are an artefact. Measured, it is worse than that: **`main` has no merge-base with
any pre-rewrite branch at all.**

```
$ git merge-base fm/gameboy-ppufifo-f4 main ; echo $?
1                       # no merge base
$ git rev-list --max-parents=0 main                b076a6c…
$ git rev-list --max-parents=0 fm/gameboy-ppufifo-f4  4d35a4a…
```

Two different root commits. `main` was rebuilt from an **orphan root** when the history was
rewritten to strip AI attribution, so every branch predating that rewrite is on a wholly
unrelated graph. `git merge-base --is-ancestor` reports `NOT-ancestor` and
`git rev-list --count $b..main` reports `behind=647` (i.e. the whole of `main`) for all of
them — both true and both telling you nothing.

**A trap worth avoiding: `git merge-base A B` prints nothing and exits 1 when there is no
merge base, so `git log -1 $(git merge-base A B)` silently falls back to showing `HEAD`.**
A first pass at this sweep read that as "merge-base is main's tip" — the exact opposite of
the truth. Check the exit code.

So the only valid test is **content**. Two were used, and they agree:
1. the branch tip's commit *subject* looked up in `git log --format=%s main`;
2. a distinctive symbol the branch introduced, grepped in `main`'s tree.

## Branches

| branch | tip | what it is | on main? | state |
|---|---|---|---|---|
| `fm/gameboy-ppufifo-f4` | `94c7c69` | per-dot PPU engine + its own gated baselines | yes (`ACCURATE_OAM_BUG_PHASE`, `mealybug_accurate_baseline`) | finished, landed |
| `fm/gameboy-apusweep-a9` | `52d98f5` | CH1 sweep-shadow reload takes three M-cycles | yes (`SWEEP_RESTART_HOLD`) | finished, landed |
| `fm/gameboy-apunoise-a7` | `a5eb0af` | CH4 trigger halves both noise counters; CGB revisions | yes (`trigger_divisor`, `CgbRevision`) | finished, landed |
| `fm/gameboy-sgb-s3` | `8747e7d` | Super Game Boy ICD2 packet receiver | yes (`MLT_REQ`) | finished, landed |
| `fm/gameboy-cigate-c9` | `2ced28b` | gate the external suites in CI | yes (`examples/support/suite_baseline.rs`) | finished, landed |
| `fm/gameboy-screentext-s6` | `5cbaad0` | screen as a first-class agent output | yes (`naming_keyboard`) | finished, landed |
| `fm/gameboy-trainerui-y3` | `08902d5` | half-blocks are ours; `panel_theme.rs` | yes — landed **renamed** as `src/screen/panel.rs` | finished, landed |
| `fm/gameboy-tcrecord-t4` | `6c20451` | Web Audio APU for the wasm build | yes (`web_audio`) | finished, landed |
| `fm/gameboy-sixel-x8b` | `884694b` | probe for sixel, stop discarding a capability answer | yes (`probe_graphics`) | finished, landed |
| `fm/gameboy-tx-compress-c8` | `9cf2de9` | shm + indexed colour frame transport | yes (`shmframe`) | landed; its own tip is only a `make docs-links` stamp regen |
| `shm-fix` | `e3206e2` | reliable shm handshake | yes (`shm_reply_ok`) | finished, landed |
| `fm/gameboy-catchmisty-p3` | `799c05b` | catch/train a bench for Misty | **ancestor of main** | landed |
| `fm/gameboy-perfbuild-v2` | `099b1b3` | build profiles + `core_bench` | **ancestor of main** | landed |
| `fm/gameboy-pewterchain-n8` | `d9d9896` | the Pewter chain | **ancestor of main** | landed |
| `fm/gameboy-refilm-q6` | `d049094` | film encoding | **ancestor of main** | landed |
| `backup/pre-rebase-r7` | `69210ed` | a deliberate pre-rebase safety copy | 13 commits ahead of `d049094`, all present on main by subject | its stated purpose; nothing to recover |

Four branches (`catchmisty-p3`, `perfbuild-v2`, `pewterchain-n8`, `refilm-q6`) are genuine
**ancestors** of `c575bb4` — they are post-rewrite and merged. The rest are pre-rewrite.

## The one substantial uncommitted diff — and it is not lost either

`gameboy-795d23/1/gameboy`, detached at `0c2dad5`, reflog showing `fm/gameboy-ppufifo-p9`:

```
 src/gpu/fifo.rs |  53 ++++--
 src/gpu/mod.rs  |  12 +-
 src/mmu/mod.rs  | 268 +++++++++++++++++++++++++++++++++++++++-
 3 files changed, 318 insertions(+), 15 deletions(-)
```

**318 insertions, last touched 14 Aug** — this is the diff the brief describes as having sat
loose. It introduces `ppu_write_commit`, `struct WriteCommit`, `LCDC_DMG_STAGE_BITS`,
`LCDC_DMG_END_BITS` and a `mod commit_probe`.

All of it is on `main`, and `main`'s copy is **strictly more developed**: `ppu_write_commit`
is 114 lines there against 80 here, and it carries the speed-aware `early` that `c575bb4`
added. The loose copy is a superseded draft. **Nothing to recover; safe to discard.**

## Everything else dirty in the pool

| worktree | what is uncommitted | verdict |
|---|---|---|
| `795d23/2` (screentext-s6) | staged rebase leftovers; `examples/screen_text.rs` untracked | on main |
| `795d23/3` (catchmisty-p3) | staged deletions of files that **moved to AgentGB** | intended; on main or moved by policy |
| `795d23/4` (perfbuild-v2) | `examples/core_bench.rs`, `tools/perf/` untracked | both on main |
| `795d23/5` (refilm-q6) | `tools/pewter-chain/encode_film.py` modified | on main |
| `795d23/8` | `tests` submodule pointer moved | not work |
| `795d23/9` (trainerui-y3) | `src/screen/panel_theme.rs` untracked | landed renamed as `src/screen/panel.rs` |
| `b03097/2` (cigate-c9) | the four suite baselines + `tools/fetch-test-suites.sh` untracked | all on main |
| `b03097/3,4,5,6` | rebase leftovers of already-landed APU/SGB/PPU work | on main |
| `b03097/5` | `examples/_sgb_scout.rs` — leading underscore, a scratch file | not on main, not wanted |
| `b03097/8` | `crates/` untracked | on main |
| `b03097/9` | `src/plugin/shinyhunt/` untracked | landed moved as `plugins/gen1-shinyhunt/` |

## Overlap with the accuracy work

`ppufifo-f4` and the loose `ppufifo-p9` diff are the only accuracy-relevant items, and both
are superseded by `main`. **There is nothing in the pool to build on** — the accuracy work
starts from `c575bb4` with no debt.

# Handover — `fm/gameboy-emulator-r7`

Written at a clean stop. Branch is committed, tree is clean, and it
fast-forwards from `main` (rebased onto `799c05b`, the bench work).

## State

- **Branch:** `fm/gameboy-emulator-r7`, 14 commits on top of `main`.
- **Safety ref:** `backup/pre-rebase-r7` = the pre-rebase tip. Delete it once
  you are happy; nothing else depends on it.
- **Gates: all ten green after the rebase** — `testroms`, `savestate`,
  `battery`, `ppuengines`, `audiocapture`, `gen1atlas`, 888 lib tests,
  `test-external`, `test-external-accurate`, `embed`, the GBSelfTest gate,
  `lint`, `docs-check`, `fmt`, `test-cross`.
- **External baselines byte-identical in BOTH picture modes** — standard 18
  baseline files, accurate 15, including Gambatte's 5,320 rows in each mode,
  GBMicrotest 513, Mooneye 103+8, SameSuite 76, Mealybug 79, cpp 4. Zero
  mismatches, zero re-bless prompts. The emulated machine did not move.

## What landed

**Speed flags** (`src/speed.rs`, `docs/modes.md`). Every shortcut is a named
flag; **speed mode is a preset over them, not a second code path**; explicit
beats preset in both directions, last wins. `GB_SPEED=speed`,
`speed,-instruction-bus`, `no-oam-bug`; also `core_bench --speed`,
`ScenarioRun::speed_flags`, `Gameboy::set_speed_flags`, and the conformance
adapter. `identical` refuses every flag and the refusal is not order-dependent
(both directions tested).

| flag | measured | in preset |
|---|---:|---|
| `instruction-bus` | +9.2 % | **yes** |
| `no-oam-bug` | 0.0 % | no — safe but worthless |
| `no-apu-state` | +25.9 % alone, +40.8 % with the preset | no — freezes a peripheral |

**GBSelfTest 87 → 91 checks** (its own repo, committed at `5f8e81e`, re-pinned
here). `GB-CYC-07` detects the instruction-bus shortcut; `GB-DMA-04` catches the
OAM DMA bus conflict; `GB-MEM-06` covers `$FEA0-$FEFF`; `GB-DMA-05` *reports*
the CGB address-bus behaviour. Skips now print what they saw.

**Docs:** `docs/modes.md` (new, the one door), `docs/measured/speed-ledger.md`
(new, eleven measured negatives), `docs/conformance-notes.md` (the three checks
the standalone tool should run), README and index links.

## Pick up here — in order

1. **A/B the hunt link, `23b-hunt-a-paras`.** This is the one thing I started
   and did not finish. Both runs were killed mid-flight; nothing is recorded
   from them and `docs/modes.md` says so explicitly rather than implying a
   result. The command, from an entry state that already exists:

   ```bash
   P=target/release/examples/pewter_chain; R=target/perf/blue.gb
   S=target/rb-base/states/23.gbss     # written by the baseline chain run
   $P corpus --rom $R --scenario scenarios/pewter/23b-hunt-a-paras.json \
      --start $S --runs 60 --workers 14 --out target/hb-none
   GB_SPEED=speed $P corpus ... --out target/hb-speed
   ```

   It takes a while — a failing attempt is ~200k frames and the budget is 1500
   decisions. Put the two rates into `docs/modes.md` §4's table.

2. **`RenderMode::IDENTICAL_COST` is 7.6 and `ppuengines` measured 8.70** under
   contention. The gate's band is ±1.5× so it passes, but the constant is shown
   to users and should be re-measured **on a quiet box** and moved if it really
   is ~8.7. `docs/tools/status_numbers.py` hard-codes it too, and `make status`
   will put the old one back if you only edit `src/gpu/mod.rs`.

3. **Re-run the per-link A/B on links main changed.** My 800-vs-799 evidence
   (links 05/17/27/29 at 200 attempts) was taken *before* the rebase, and main
   changed link 27 and the party the later links carry. The numbers in
   `docs/modes.md` §4 are honest about what they measured, but they describe the
   pre-bench chain.

## Traps I hit, so you do not

- **`grep` on this box reads a cached view of a file being appended to.** Gate
  logs read as "still running" long after they finished. Use
  `tail -c 300 <file>` instead.
- **`pkill -f` / `pgrep -f` match your own shell.** Kill by PID from
  `pgrep -x pewter_chain`. (`pgrep -a` is the reliable "is it alive".)
- **The rebase used a `merge=union` driver for `AGENTS.md`**, installed in the
  *shared* git dir (`$(git rev-parse --git-common-dir)/info/attributes`) and
  **removed immediately afterwards**. It is gone; check it is if a merge ever
  behaves oddly. `rerere` was enabled and has been unset too.
- **Only one file truly conflicted** — `examples/pewter_chain.rs`, where main
  and I had independently fixed the same pre-existing clippy comparison chain.
  I took main's. My branch's copy is now byte-identical to main's.

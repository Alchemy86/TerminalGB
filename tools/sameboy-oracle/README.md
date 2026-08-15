# A second opinion — SameBoy as an oracle

`"SameBoy fails that one too"` is the commonest excuse behind a red row in this
repository. Until this existed it was always a **quotation**, usually of
SameSuite's own [`apu/README.md`](https://github.com/LIJI32/SameSuite) — which
is years out of date. This turns it into a **measurement**.

```sh
tools/sameboy-oracle/fetch.sh                 # clone + build, ~1 min, once
O=target/sameboy-oracle/oracle
B=target/sameboy-oracle/SameBoy/build/bin/tester

$O $B cgbE samesuite target/test-suites/samesuite/apu/channel_4/channel_4_freq_change.gb
$O $B dmg  blargg    tests/oam_bug/oam_bug.gb 4000
```

It prints the **ROM's own** verdict, exactly as our two harnesses read it — the
`RESULT_CODE` byte at `$CFFE` for SameSuite, the cartridge-RAM status byte and
report text behind the `$DE $B0 $61` signature for Blargg. Nothing here is a
hash of anybody's framebuffer, and nothing here is an expectation of *ours*: it
asks a foreign implementation the same question the ROM asks, so a disagreement
names a real defect in one of us.

Consoles: `dmg mgb sgb cgb0 cgbB cgbC cgbD cgbE agb` — the same names
`$GB_MODEL` and `$GB_CGB_REVISION` take here.

## What it is for

Three questions it answers in one command, all of which have been got wrong
here by reading prose instead:

1. **Does the reference implementation actually pass this row?** Measured
   2026-08-15 at the pinned commit: SameBoy on CGB-E **passes**
   `channel_4_freq_change` and `channel_1_sweep_restart`, both of which
   SameSuite's README still lists as SameBoy failures. A stale README is not
   evidence.
2. **Which machine is this ROM's table for?** Run it on all nine and see. The
   three `channel_1_freq_change_timing-{A,cgb0BC,cgbDE}` ROMs pass on exactly
   one console family each — AGB, CPU CGB 0/B/C, CPU CGB D/E — which is what
   says our red on two of them is a *machine* we do not boot, not a bug.
3. **Is our red the correct behaviour?** `oam_bug` on `cgbE` produces
   `01:ok 02:02 03:ok 04:03 05:02 06:ok 07:01 08:02 … Failed #2` — character
   for character what `testharness/baseline.txt` records for
   `blargg/oam_bug@cgb`. Agreeing with an independent implementation *in the
   detail* is a much stronger claim than "we fail too".

## House rules

* **Never vendored, and never in CI.** SameBoy is somebody else's repository
  under somebody else's licence (Expat, except its `iOS/` and `HexFiend/`
  directories); the pin lives in `fetch.sh` and nowhere else, the same rule
  `tools/fetch-test-suites.sh` and `tools/bgb-interop/` follow. Nothing this
  writes is added to the repository, and none of it is needed to build or test
  the emulator.
* **It is an oracle, not an expectation.** A row does not become green because
  SameBoy passes it, and does not become acceptable because SameBoy fails it —
  hardware is the authority. What this buys is knowing *whether a working model
  exists* before spending a crew on one.
* **Read the source, do not copy it.** `CONTRIBUTING.md`'s rule applies:
  behaviour learned from SameBoy is reimplemented in this codebase's idiom and
  cited at the consumer (repository + file + symbol).

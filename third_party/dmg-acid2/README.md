# dmg-acid2 (vendored)

Prebuilt, MIT-licensed test ROM + reference image from
[mattcurrie/dmg-acid2](https://github.com/mattcurrie/dmg-acid2).

| File | Source |
|---|---|
| `dmg-acid2.gb` | [release v1.0 asset](https://github.com/mattcurrie/dmg-acid2/releases/download/v1.0/dmg-acid2.gb) |
| `reference-dmg.png` | `img/reference-dmg.png` at upstream `master` |
| `LICENSE` | upstream MIT license |

## Why vendored instead of a submodule

Upstream ships **source only** — building `dmg-acid2.gb` needs the RGBDS assembler
and a nested `mgblib` submodule, which is awkward to run in CI. The prebuilt ROM is
MIT-licensed and only 32 KB, so it is vendored directly. The `**/*.gb` entry in the
repo's [`.gitignore`](../../.gitignore) is negated for this one file so it stays tracked.

The test-ROM harness ([`testharness/testroms.rs`](../../testharness/testroms.rs)) renders
this ROM headlessly and compares the framebuffer **pixel by pixel against
`reference-dmg.png`**, by luminance rank rather than by colour (our shades are
`000000/606060/c0c0c0/ffffff`, the reference's are `000000/555555/aaaaaa/ffffff`). The
resulting count goes into [`testharness/baseline.txt`](../../testharness/baseline.txt) as
`0/23040 pixels differ …` — **not** a hash of our own output, which is what it used to be and
which could never fail. The dumped frame is written to
`target/testrom-artifacts/acid2__dmg-acid2.png` if you want to look at it.

See [`docs/testing.md`](../../docs/testing.md) for the harness, and
[`docs/emulation/test-roms.md`](../../docs/emulation/test-roms.md) for what this ROM is
actually testing — including a gallery of how the face deforms under each individual PPU
bug, measured by breaking them one at a time.

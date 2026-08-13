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
this ROM headlessly and compares the framebuffer hash against
[`testharness/baseline.txt`](../../testharness/baseline.txt); compare the dumped frame in
`target/testrom-artifacts/acid2__dmg-acid2.png` against `reference-dmg.png` by eye. See
[`docs/testing.md`](../../docs/testing.md).

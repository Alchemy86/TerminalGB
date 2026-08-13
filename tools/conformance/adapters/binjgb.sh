#!/bin/sh
# gbconform/1 adapter for **binjgb** (https://github.com/binji/binjgb) — a real,
# unrelated emulator, used to prove the contract is not secretly shaped around
# our own.
#
# binjgb ships a headless `binjgb-tester` whose flags map onto the contract
# almost one-to-one, so the whole adapter is the `exec` line below. Build it
# with:
#
#   git clone --depth 1 https://github.com/binji/binjgb
#   cd binjgb && gcc -O2 -Isrc -o binjgb-tester \
#       src/tester.c src/emulator.c src/joypad.c src/options.c \
#       src/common.c src/memory.c -lm
#
# Then: BINJGB_TESTER=./binjgb-tester python3 tools/conformance/gbconform.py \
#           --adapter tools/conformance/adapters/binjgb.sh --out out
#
# HONEST LIMITATION, and the reason this file is worth reading: binjgb-tester
# exposes a framebuffer (`-o`, a Netpbm P6) but **no serial output**. There is
# no flag for it and `emulator.h` publishes no serial hook, so the serial
# checks cannot be run through it. The runner probes for that, reports those
# checks as SKIPPED rather than failed, and excludes them from the score —
# which is exactly why the contract treats the two outputs as independent
# capabilities instead of demanding both.
set -eu

TESTER="${BINJGB_TESTER:-binjgb-tester}"

# No frame wanted (i.e. this is a serial check): produce nothing and exit
# cleanly. That is how an adapter declines a capability it does not have.
[ -n "${GBCONFORM_FRAME_OUT:-}" ] || exit 0

exec "$TESTER" \
    -f "${GBCONFORM_FRAMES:-600}" \
    --force-dmg \
    -o "$GBCONFORM_FRAME_OUT" \
    "$GBCONFORM_ROM"

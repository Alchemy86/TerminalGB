#!/bin/sh
# Fetch and build SameBoy as a *reference emulator*, at a pinned commit.
#
# Never vendored, for the reason `tools/fetch-test-suites.sh` gives for the
# test ROMs: it is somebody else's repository under somebody else's licence
# (Expat here, except its `iOS/` and `HexFiend/` directories), and a laptop, a
# CI job and a published claim should measure identical bytes. Both pins live
# here and nowhere else.
#
# SameBoy assembles its own boot ROMs, so this also fetches a prebuilt RGBDS.
# Neither is added to this repository and neither is needed to build or test
# the emulator — this is a hand-run instrument, like `tools/bgb-interop/`.
#
#   usage: tools/sameboy-oracle/fetch.sh [dest]   (default: target/sameboy-oracle)
set -eu

SAMEBOY_COMMIT=213a12ce93d66b105a113debd9396306066a7cfc   # 2026-07-10
SAMEBOY_REPO=https://github.com/LIJI32/SameBoy
RGBDS_VERSION=v1.0.3
RGBDS_URL="https://github.com/gbdev/rgbds/releases/download/$RGBDS_VERSION/rgbds-linux-x86_64.tar.xz"

DEST=${1:-target/sameboy-oracle}
HERE=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)

if [ ! -x "$DEST/rgbds/rgbasm" ]; then
    echo "fetching rgbds $RGBDS_VERSION"
    mkdir -p "$DEST/rgbds"
    curl -sSL "$RGBDS_URL" | tar -xJ -C "$DEST/rgbds"
fi

if [ -d "$DEST/SameBoy/.git" ]; then
    git -C "$DEST/SameBoy" fetch --quiet origin "$SAMEBOY_COMMIT" || true
else
    git clone --quiet "$SAMEBOY_REPO" "$DEST/SameBoy"
fi
git -C "$DEST/SameBoy" checkout --quiet "$SAMEBOY_COMMIT"

# `tester` is the cheapest target that also assembles every boot ROM, which is
# what the oracle needs — a SameSuite ROM runs in CGB mode and its answers
# depend on the state the boot ROM hands over.
echo "building SameBoy (this takes a minute)"
PATH="$DEST/rgbds:$PATH" make -C "$DEST/SameBoy" -j"$(nproc)" tester CONF=release >"$DEST/build.log" 2>&1 ||
    { tail -20 "$DEST/build.log"; echo "SameBoy build failed, see $DEST/build.log"; exit 1; }

cc -O2 -std=gnu99 -I"$DEST/SameBoy" -o "$DEST/oracle" "$HERE/oracle.c" \
    "$DEST"/SameBoy/build/obj/Core/*.o -lm -ldl 2>/dev/null

echo "SameBoy $SAMEBOY_COMMIT built; oracle at $DEST/oracle"
echo "boot ROMs in $DEST/SameBoy/build/bin/tester"

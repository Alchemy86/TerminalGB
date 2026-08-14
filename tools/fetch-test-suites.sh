#!/bin/sh
# Fetch the external test suites this repository is gated against.
#
# **Fetch at run time, never vendor.** These ROMs are other people's work under
# other people's licences — the same reason `.github/workflows/conformance.yml`
# downloads Blargg's suite instead of committing it. Nothing this script writes
# is ever added to the repository; the expectations are (testharness/*_baseline.txt).
#
# Every version is pinned HERE and nowhere else, so a CI job, a Makefile target
# and a developer's laptop all measure the same ROMs. Bumping a pin is a
# deliberate change that will move the baselines, and the two must land in the
# same commit.
#
#   usage: tools/fetch-test-suites.sh [dest]        (default: target/test-suites)
#
# Already-present suites are left alone, so re-running is cheap.
set -eu

DEST=${1:-target/test-suites}

# --- pins ------------------------------------------------------------------
# Mooneye GB: the author's own prebuilt archive. The GitHub repository ships
# assembler source only, and building it needs a matching RGBDS; Gekkio
# publishes the built ROMs himself, which is the closest thing to an
# authoritative binary this suite has.
MTS_BUILD=mts-20240926-1737-443f6e1
MTS_SHA256=d9ab11a01351e0eb2dea485237027c4dd66c0528c707d21ff78604157b967837

# Mealybug Tearoom: upstream repository. It ships both the prebuilt ROMs
# (mealybug-tearoom-tests.zip) and the reference screenshots photographed off
# real DMG and CGB hardware, which are the actual expectations.
MEALYBUG_COMMIT=70e88fb90b59d19dfbb9c3ac36c64105202bb1f4

# SameSuite: LIJI32 ships source only and there is no upstream binary release,
# so we take the prebuilt copies vendored by the GB Emulator Shootout — which
# is also the exact corpus our published 264-row score is measured on. A
# blobless sparse checkout pulls ~76 ROMs, not the whole 18 MB repository.
#
# The same checkout brings the shootout's `cpp` group (CasualPokePlayer's four
# ROMs: three MBC3/RTC tests and `sgb-ext-test`) with its reference
# screenshots, because those four are published shootout rows that nothing here
# was gating.
SHOOTOUT_COMMIT=38b926bdbc26993d1b4c43e97979ecc66287bf02

mkdir -p "$DEST"
DEST=$(cd "$DEST" && pwd)

# --- Mooneye ---------------------------------------------------------------
# The build id is stamped rather than kept in the path: consumers name a fixed
# `mooneye/acceptance`, and bumping the pin still re-fetches.
if [ "$(cat "$DEST/mooneye/.build-id" 2>/dev/null || true)" != "$MTS_BUILD" ]; then
    echo "fetching mooneye $MTS_BUILD"
    rm -rf "$DEST/mooneye"
    mkdir -p "$DEST/mooneye"
    curl -sSL -o "$DEST/mts.tar.xz" \
        "https://gekkio.fi/files/mooneye-test-suite/$MTS_BUILD/$MTS_BUILD.tar.xz"
    echo "$MTS_SHA256  $DEST/mts.tar.xz" | sha256sum -c -
    tar -xJf "$DEST/mts.tar.xz" -C "$DEST/mooneye" --strip-components=1
    rm -f "$DEST/mts.tar.xz"
    echo "$MTS_BUILD" > "$DEST/mooneye/.build-id"
fi

# --- Mealybug --------------------------------------------------------------
if [ ! -d "$DEST/mealybug/expected" ]; then
    echo "fetching mealybug $MEALYBUG_COMMIT"
    rm -rf "$DEST/mealybug"
    git init -q "$DEST/mealybug"
    git -C "$DEST/mealybug" remote add origin \
        https://github.com/mattcurrie/mealybug-tearoom-tests.git
    git -C "$DEST/mealybug" fetch -q --depth 1 origin "$MEALYBUG_COMMIT"
    git -C "$DEST/mealybug" checkout -q FETCH_HEAD
    # The repository ships sources AND a prebuilt zip; use the author's build.
    unzip -q -o "$DEST/mealybug/mealybug-tearoom-tests.zip" \
        -d "$DEST/mealybug/build"
fi

# --- SameSuite + the shootout's `cpp` group --------------------------------
if [ ! -d "$DEST/samesuite/apu" ] || [ ! -d "$DEST/cpp" ]; then
    echo "fetching samesuite + cpp (shootout $SHOOTOUT_COMMIT)"
    rm -rf "$DEST/shootout" "$DEST/samesuite" "$DEST/cpp"
    git clone -q --filter=blob:none --no-checkout \
        https://github.com/gbdev/GBEmulatorShootout.git "$DEST/shootout"
    git -C "$DEST/shootout" sparse-checkout init --cone
    git -C "$DEST/shootout" sparse-checkout set testroms/samesuite testroms/cpp
    git -C "$DEST/shootout" checkout -q "$SHOOTOUT_COMMIT"
    mv "$DEST/shootout/testroms/samesuite" "$DEST/samesuite"
    mv "$DEST/shootout/testroms/cpp" "$DEST/cpp"
    rm -rf "$DEST/shootout"
fi

echo "suites ready under $DEST:"
echo "  mooneye     $(find "$DEST/mooneye" -name '*.gb' | wc -l) roms"
echo "  mealybug    $(find "$DEST/mealybug/build" -name '*.gb' | wc -l) roms"
echo "  samesuite   $(find "$DEST/samesuite" -name '*.gb' | wc -l) roms"
echo "  cpp         $(find "$DEST/cpp" -name '*.gb' | wc -l) roms"

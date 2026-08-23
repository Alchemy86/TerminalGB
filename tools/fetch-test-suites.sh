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

# c-sp/game-boy-test-roms: the community's standard aggregation, taken as ONE
# pinned release asset rather than as thirteen more upstreams.
#
# Why the whole release and not per-suite pins, given every other pin here is
# per-suite: (a) it is a single immutable artefact with a single checksum, so a
# laptop, a CI job and a published score measure identical bytes with one pin to
# bump; (b) it is the corpus other emulators' published c-sp scores are measured
# on, which is what makes a comparison meaningful; and (c) — the one that
# decided it — **each suite in it ships the author's own
# `game-boy-test-roms-howto.md`**, which states that author's exit condition and
# pass criterion. Those documents are the specification `examples/csp_suite.rs`
# implements, and fetching thirteen upstreams separately would leave the runner
# implementing thirteen conventions from memory.
#
# The five suites already pinned above are deliberately NOT taken from here:
# their baselines were measured against those exact builds, and re-pointing them
# at a different build of the same ROMs would move rows for a reason that has
# nothing to do with the emulator.
CSP_TAG=v7.0
CSP_SHA256=b9a9d7a1075aa35a3d07c07c34974048672d8520dca9e07a50178f5860c3832c

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
# Blobless + sparse, the same shape as the shootout checkout below, and for a
# bigger reason: the repository ships `photos/`, ~62 MB of blurry hardware
# photographs the author cross-checked his own `expected/` screenshots against.
# Nothing here reads them — `examples/mealybug` scores against `expected/`, and
# the two places `photos/` is named in this tree are both prose. A plain
# depth-1 clone pulled them anyway, and their blobs again into `.git`: 125 MB
# for a suite whose ROMs and expectations are under 1.5 MB, on every CI run.
# `src/` and `inc/` are kept because the ROM sources say what each row
# measures and are worth reading; root files (the prebuilt zip) come with cone
# mode for free.
if [ ! -d "$DEST/mealybug/expected" ]; then
    echo "fetching mealybug $MEALYBUG_COMMIT"
    rm -rf "$DEST/mealybug"
    git init -q "$DEST/mealybug"
    git -C "$DEST/mealybug" remote add origin \
        https://github.com/mattcurrie/mealybug-tearoom-tests.git
    git -C "$DEST/mealybug" config core.sparseCheckout true
    git -C "$DEST/mealybug" sparse-checkout init --cone
    git -C "$DEST/mealybug" sparse-checkout set expected src inc
    git -C "$DEST/mealybug" fetch -q --depth 1 --filter=blob:none \
        origin "$MEALYBUG_COMMIT"
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

# --- c-sp/game-boy-test-roms ------------------------------------------------
# The tag is stamped rather than kept in the path: consumers name a fixed
# `csp/gambatte`, and bumping the pin still re-fetches.
if [ "$(cat "$DEST/csp/.release" 2>/dev/null || true)" != "$CSP_TAG" ]; then
    echo "fetching c-sp/game-boy-test-roms $CSP_TAG"
    rm -rf "$DEST/csp"
    mkdir -p "$DEST/csp"
    curl -sSL -o "$DEST/csp.zip" \
        "https://github.com/c-sp/game-boy-test-roms/releases/download/$CSP_TAG/game-boy-test-roms-$CSP_TAG.zip"
    echo "$CSP_SHA256  $DEST/csp.zip" | sha256sum -c -
    unzip -q -o "$DEST/csp.zip" -d "$DEST/csp"
    rm -f "$DEST/csp.zip"
    echo "$CSP_TAG" > "$DEST/csp/.release"
fi

echo "suites ready under $DEST:"
echo "  mooneye     $(find "$DEST/mooneye" -name '*.gb' | wc -l) roms"
echo "  mealybug    $(find "$DEST/mealybug/build" -name '*.gb' | wc -l) roms"
echo "  samesuite   $(find "$DEST/samesuite" -name '*.gb' | wc -l) roms"
echo "  cpp         $(find "$DEST/cpp" -name '*.gb' | wc -l) roms"
echo "  csp         $(find "$DEST/csp" \( -name '*.gb' -o -name '*.gbc' \) | wc -l) roms in $(find "$DEST/csp" -mindepth 1 -maxdepth 1 -type d | wc -l) suites"

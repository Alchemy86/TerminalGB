#!/bin/sh
# Vendor the GBSelfTest cartridge, and check the vendored copy is the pinned one.
#
# GBSelfTest is a self-testing Game Boy cartridge kept in its own repository
# (Alchemy86/GBSelfTest, MIT). It boots, runs its suite and prints the verdict
# over the link port, so this project consumes it as an ARTEFACT -- a built
# ROM -- and never as source. Keeping it out of this tree is the point: it has
# to be adoptable by other emulator authors without taking a dependency on us,
# and it has to be able to disagree with us.
#
#   tools/fetch-gbselftest.sh              fetch the pinned release and write the lock
#   tools/fetch-gbselftest.sh --verify     recompute the checksum offline; no network
#   tools/fetch-gbselftest.sh --check-upstream   is there a newer release? advisory only
#
# Vendored rather than fetched at build time, and not a submodule: a submodule
# breaks shallow checkouts and source tarballs, and a build-time fetch puts the
# network on the path of `cargo test`. This is the same shape as
# `third_party/dmg-acid2/`, which this repository already vendors, plus the one
# thing that artefact does not need: a lock, because dmg-acid2 is frozen and
# this one is alive and will be re-released.
#
# Nothing but `curl` and `sha256sum`. Vendoring data adds no cargo dependency,
# so `make embed`'s one-third-party-dependency rule is untouched.
set -eu

REPO="Alchemy86/GBSelfTest"
ASSET="gbselftest.gb"
DOCS="CHECKS.md"

HERE=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DEST="$HERE/third_party/gbselftest"
LOCK="$DEST/gbselftest.lock"

lock_value() {
    # `key = value` lines, matching tools/fetch-test-suites.sh's style so the
    # script that writes a lock needs no parser to read one back.
    sed -n "s/^$1 *= *//p" "$LOCK" 2>/dev/null | head -1
}

verify() {
    want=$(lock_value sha256)
    if [ -z "$want" ]; then
        echo "no sha256 in $LOCK" >&2
        return 1
    fi
    if [ ! -f "$DEST/$ASSET" ]; then
        echo "missing $DEST/$ASSET" >&2
        return 1
    fi
    have=$(sha256sum "$DEST/$ASSET" | cut -d' ' -f1)
    if [ "$have" != "$want" ]; then
        cat >&2 <<EOF
$DEST/$ASSET does not match $LOCK
    vendored $have
    pinned   $want
The vendored copy has been edited in place. Fix it upstream in $REPO, cut a
release, and re-run this script -- editing a vendored artefact makes the pin a
lie and the next refresh silently reverts you.
EOF
        return 1
    fi
    echo "gbselftest.gb matches the lock ($(lock_value ref), $have)"
}

case "${1:-}" in
--verify)
    verify
    exit $?
    ;;
--check-upstream)
    # Advisory. Never a build failure: a newer release upstream is news, not a
    # defect, and refreshing is a deliberate act because it moves what CI scores.
    have=$(lock_value ref)
    latest=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null |
        sed -n 's/.*"tag_name" *: *"\([^"]*\)".*/\1/p' | head -1) || true
    if [ -z "$latest" ]; then
        echo "cannot reach GitHub, or $REPO has no release yet; nothing to compare"
        exit 0
    fi
    if [ "$latest" = "$have" ]; then
        echo "up to date: $have"
    else
        echo "newer release upstream: $latest (vendored: $have)"
        echo "refresh deliberately with tools/fetch-gbselftest.sh, in a commit of its own"
    fi
    exit 0
    ;;
esac

mkdir -p "$DEST"

# A local checkout can stand in for a release, which is how this was
# bootstrapped before the first one existed and how a change to the cartridge
# is tried here before it is published.
if [ -n "${GBSELFTEST_LOCAL:-}" ]; then
    src="$GBSELFTEST_LOCAL"
    [ -f "$src/dist/$ASSET" ] || { echo "no $src/dist/$ASSET" >&2; exit 1; }
    cp "$src/dist/$ASSET" "$DEST/$ASSET"
    [ -f "$src/docs/$DOCS" ] && cp "$src/docs/$DOCS" "$DEST/$DOCS"
    ref=$(cd "$src" && git describe --tags --always --dirty 2>/dev/null || echo unreleased)
    commit=$(cd "$src" && git rev-parse HEAD 2>/dev/null || echo unknown)
    origin="local checkout $src"
else
    REF="${GBSELFTEST_REF:?set GBSELFTEST_REF to the release tag, or GBSELFTEST_LOCAL to a checkout}"
    base="https://github.com/$REPO/releases/download/$REF"
    curl -fsSL "$base/$ASSET" -o "$DEST/$ASSET.new"
    mv "$DEST/$ASSET.new" "$DEST/$ASSET"
    curl -fsSL "$base/$DOCS" -o "$DEST/$DOCS" || true
    ref="$REF"
    commit=$(curl -fsSL "https://api.github.com/repos/$REPO/git/ref/tags/$REF" |
        sed -n 's/.*"sha" *: *"\([^"]*\)".*/\1/p' | head -1)
    origin="$base/$ASSET"
fi

sha=$(sha256sum "$DEST/$ASSET" | cut -d' ' -f1)
cat >"$LOCK" <<EOF
# Written by tools/fetch-gbselftest.sh. Do not edit: the checksum below is what
# the --verify option compares the vendored ROM against, and editing it by hand
# makes the pin a lie.
repo = https://github.com/$REPO
ref = $ref
commit = $commit
sha256 = $sha
origin = $origin
EOF

echo "vendored $DEST/$ASSET"
sed 's/^/  /' "$LOCK"

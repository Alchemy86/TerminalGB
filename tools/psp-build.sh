#!/usr/bin/env bash
# Build TerminalGB for the PlayStation Portable.
#
# Everything the PSP needs that this repository will not carry is fetched here,
# at pinned versions, the same rule `tools/fetch-test-suites.sh` follows: other
# people's code, in one place, so a laptop and a CI job measure identical bytes.
#
#   ./tools/psp-build.sh [output-dir]
#
# The result is an `EBOOT.PBP`. Copy it to a memory stick (or PPSSPP's) at
# `ms0:/PSP/GAME/terminalgb/EBOOT.PBP`, put cartridges in
# `ms0:/PSP/GAME/terminalgb/roms/`, and it appears in the XMB as TerminalGB.
#
# WHY THIS IS A SCRIPT AND NOT A CARGO TARGET — all three of these must be true
# at once and none of them may leak into the emulator's own build:
#
#   * Rust **nightly**, pinned. `mipsel-sony-psp` is a Tier 3 target that
#     upstream Rust supports only as `no_std`; the emulator needs `std`.
#   * A third-party **std overlay** (`rust-psp`) that reimplements std's
#     internal platform layer for the PSP and is rebuilt with `-Z build-std`.
#     Those internals are unstable with no compatibility promise, so the overlay
#     tracks one nightly and breaks on the next: on nightly-2026-08-14 it does
#     not compile at all. That is why the date below is pinned and why bumping
#     it is a real piece of work, not a version bump.
#   * `cargo-psp`, which packs the ELF into an `EBOOT.PBP`, and which can only
#     build the std overlay from inside its own checkout — hence the staging
#     step below.
#
# See docs/portability.md §PSP for the measurements and the honest limits.
set -euo pipefail

# --- pins ------------------------------------------------------------------
# The fork that supplies the PSP std overlay (MIT; upstream overdrivenpotato,
# fork AndrewAltimit). Its CI pins the nightly below in `docker/rust-ci.Dockerfile`
# and its own comments record that a nightly API bump is what breaks it.
PSP_REPO=${PSP_REPO:-https://github.com/AndrewAltimit/rust-psp}
PSP_COMMIT=${PSP_COMMIT:-ad92131218406308170a3344786865415aef2995}
PSP_NIGHTLY=${PSP_NIGHTLY:-nightly-2026-04-14}

REPO=$(cd "$(dirname "$0")/.." && pwd)
OUT=$REPO/target/psp
case "${1:-}" in ""|--*) ;; *) OUT=$1 ;; esac
# Deliberately OUTSIDE the repository: it is a checkout of somebody else's
# workspace, and a cargo workspace inside `target/` is claimed by ours (cargo
# refuses to build it). It also survives `cargo clean`, which is the point.
WORK=${GB_PSP_WORK:-${XDG_CACHE_HOME:-$HOME/.cache}/terminalgb-psp}

echo "== TerminalGB for PSP"
echo "   repo     $REPO"
echo "   toolchain $PSP_NIGHTLY + $PSP_REPO@${PSP_COMMIT:0:12}"

if ! rustup toolchain list | grep -q "^$PSP_NIGHTLY"; then
    echo "== installing $PSP_NIGHTLY (with rust-src, needed by -Z build-std)"
    rustup toolchain install "$PSP_NIGHTLY" --profile minimal --component rust-src
fi

mkdir -p "$WORK"
if [ ! -d "$WORK/rust-psp/.git" ]; then
    echo "== fetching rust-psp"
    git clone --quiet "$PSP_REPO" "$WORK/rust-psp"
fi
git -C "$WORK/rust-psp" fetch --quiet origin "$PSP_COMMIT" 2>/dev/null || git -C "$WORK/rust-psp" fetch --quiet
git -C "$WORK/rust-psp" checkout --quiet "$PSP_COMMIT"

if [ ! -x "$WORK/bin/cargo-psp" ]; then
    echo "== building cargo-psp"
    cargo +"$PSP_NIGHTLY" install --quiet --path "$WORK/rust-psp/cargo-psp" --root "$WORK"
fi
export PATH="$WORK/bin:$PATH"

# `cargo-psp` refuses to assemble the std sysroot outside its own checkout, so
# the frontend is staged in as one of its examples. Only the manifest is
# rewritten — the source is used from this repository, unmodified, so there is
# never a second copy of it to drift.
STAGE="$WORK/rust-psp/examples/terminalgb"
rm -rf "$STAGE"
mkdir -p "$STAGE"
cp -r "$REPO/platform/psp/src" "$STAGE/src"
sed -e "s|path = \"../../..\"|path = \"$REPO\"|" "$REPO/platform/psp/Cargo.toml" > "$STAGE/Cargo.toml"

# `opt-level = 3` and one codegen unit are MEASURED, not assumed. The
# emulator's own `[profile.release]` is `opt-level = "s"`, which is right for a
# desktop binary and wrong here: on the PSP it costs 1 ms a frame (23.4 vs 22.4
# ms emulating Pokémon Blue). Fat LTO with one codegen unit is worth a further
# ~14 %. See docs/portability.md §PSP.
# `--profile` builds the MEASUREMENT binary instead of the shipping one: the
# interactive loop is replaced by a fixed-length benchmark that splits the frame
# budget by component and writes the table to the memory stick. See
# `platform/psp/src/main.rs::bench` and docs/portability.md §3.7.
FEATURES=""
for arg in "$@"; do
    [ "$arg" = "--profile" ] && FEATURES="--features profile"
done

cd "$STAGE"
CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
CARGO_PROFILE_RELEASE_LTO=fat \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
RUST_PSP_BUILD_STD=1 \
    cargo +"$PSP_NIGHTLY" psp --release $FEATURES

BUILT="$WORK/rust-psp/target/mipsel-sony-psp-std/release/terminalgb-psp.EBOOT.PBP"
mkdir -p "$OUT"
cp "$BUILT" "$OUT/EBOOT.PBP"
echo
echo "== $OUT/EBOOT.PBP  ($(stat -c%s "$OUT/EBOOT.PBP") bytes)"
echo "   copy to ms0:/PSP/GAME/terminalgb/EBOOT.PBP"
echo "   cartridges in ms0:/PSP/GAME/terminalgb/roms/"

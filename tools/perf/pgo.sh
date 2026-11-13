#!/bin/sh
# One honest profile-guided-optimisation experiment, on a **representative**
# workload: a real cartridge running from a real save state, which is what a
# training fan-out does, not a synthetic loop.
#
#   POKE_ROM=<rom> [POKE_STATE=<state>] tools/perf/pgo.sh
#
# Produces `target/perf/bin/o3-fat-pgo`, which `tools/perf/measure.sh` then
# scores beside every other variant on the same rounds.
#
# PGO needs `llvm-tools-preview` (`rustup component add llvm-tools-preview`) and
# is deliberately NOT wired into any build target here: the profile is a file on
# one developer's disk, so a build that silently depended on it would produce
# different binaries for different people. Whether it is worth adopting is
# decided by the number this prints, and that argument lives in
# docs/throughput.md.
#
# **`-Cprofile-use` and fat LTO do not combine on the pinned rustc 1.83**:
#
#   module flag identifiers must be unique (or of 'require' type)
#   !"CG Profile"
#   rustc-LLVM ERROR: Broken module found, compilation aborted!
#
# so the PGO build uses **thin** LTO and is scored against `o3` (opt 3, thin,
# one codegen unit), which is the only fair comparison. Do not "fix" it by
# setting fat here.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$REPO"
OUT="$REPO/target/perf"
PROF="$OUT/pgo-raw"
ROM=${POKE_ROM:-}
[ -n "$ROM" ] || { echo "set POKE_ROM to a cartridge (none ships here)" >&2; exit 2; }
STATE_ARG=""
[ -n "${POKE_STATE:-}" ] && STATE_ARG="--state ${POKE_STATE}"

PROFDATA=$(find "$HOME/.rustup" -name llvm-profdata 2>/dev/null | head -1)
[ -n "$PROFDATA" ] || { echo "no llvm-profdata: rustup component add llvm-tools-preview" >&2; exit 2; }

rm -rf "$PROF" "$OUT/td/pgo"
mkdir -p "$PROF" "$OUT/bin"

echo "== 1/3 instrumented build"
CARGO_TARGET_DIR="$OUT/td/pgo" \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
    CARGO_PROFILE_RELEASE_LTO=fat \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    RUSTFLAGS="-Cprofile-generate=$PROF" \
    cargo build --release --example core_bench >/dev/null 2>&1

echo "== 2/3 profiling run (the real workload, not a microbenchmark)"
# shellcheck disable=SC2086
"$OUT/td/pgo/release/examples/core_bench" --rom "$ROM" $STATE_ARG \
    --frames 4000 --repeat 1 >/dev/null
"$PROFDATA" merge -o "$PROF/merged.profdata" "$PROF"

echo "== 3/3 optimised build (thin LTO — see the note above)"
rm -rf "$OUT/td/pgo"
CARGO_TARGET_DIR="$OUT/td/pgo" \
    CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
    CARGO_PROFILE_RELEASE_LTO=thin \
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 \
    RUSTFLAGS="-Cprofile-use=$PROF/merged.profdata" \
    cargo build --release --example core_bench >/dev/null 2>&1
cp "$OUT/td/pgo/release/examples/core_bench" "$OUT/bin/o3-thin-pgo"
echo "wrote $OUT/bin/o3-thin-pgo ($(stat -c%s "$OUT/bin/o3-thin-pgo") bytes)"
echo "score it against 'o3' (opt 3, thin, cu 1), not against 'o3-fat'."

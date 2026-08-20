#!/bin/sh
# Run every stashed `core_bench` variant **interleaved**, round-robin, and print
# the best round for each.
#
#   POKE_ROM=<rom> [POKE_STATE=<state>] tools/perf/measure.sh [rounds] [extra args...]
#
# Interleaving is the whole design. This machine runs other people's work: a
# load average of 30 on twenty cores is normal here, and measuring variant A to
# completion and then variant B measures the box's afternoon, not the binaries.
# Round-robin gives every variant the same neighbours, and `core_bench` already
# takes the best of its own repeats.
#
# It also compares the **frame and WRAM hashes** across variants and shouts if
# any of them differ. A build-configuration change that moves either has changed
# emulated behaviour: that is a bug, not an optimisation, and no millisecond
# figure below it means anything.
#
# **Set `GB_PPU=standard`.** Every figure in `docs/throughput.md` and
# `docs/measured/speed-ledger.md` was measured on the whole-scanline engine,
# which was the default until 2026-08-20 and is not one now
# (`docs/picture-modes.md`). This script does not set it, deliberately — it
# would change what a run already in flight is measuring — so export it
# yourself, or the numbers are the per-dot engine's and about four times
# larger. A/B comparisons stay valid either way; absolute figures do not.
set -eu

REPO=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
BIN="$REPO/target/perf/bin"
ROM=${POKE_ROM:-}
[ -n "$ROM" ] || { echo "set POKE_ROM to a cartridge (none ships here)" >&2; exit 2; }
rounds=${1:-3}
[ $# -gt 0 ] && shift
STATE_ARG=""
[ -n "${POKE_STATE:-}" ] && STATE_ARG="--state ${POKE_STATE}"

names=$(ls "$BIN")
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "load before: $(cut -d' ' -f1-3 /proc/loadavg)"
r=1
while [ "$r" -le "$rounds" ]; do
    for n in $names; do
        # shellcheck disable=SC2086
        "$BIN/$n" --rom "$ROM" $STATE_ARG --repeat 1 --json "$@" >>"$tmp/$n"
    done
    r=$((r + 1))
done
echo "load after:  $(cut -d' ' -f1-3 /proc/loadavg)"
echo

printf '%-16s %10s %10s %10s   %s\n' variant ms/frame fps xrealtime hashes
base=""
for n in $names; do
    ms=$(sed 's/.*"ms_per_frame":\([0-9.]*\).*/\1/' "$tmp/$n" | sort -g | head -1)
    fps=$(awk "BEGIN{printf \"%.0f\", 1000/$ms}")
    xrt=$(awk "BEGIN{printf \"%.1f\", 1000/$ms/59.7275}")
    h=$(sed 's/.*"frame_hash":"\([^"]*\)".*"wram_hash":"\([^"]*\)".*/\1 \2/' "$tmp/$n" | sort -u)
    nh=$(echo "$h" | wc -l)
    tag=$(echo "$h" | head -1 | cut -c3-10)
    [ "$nh" -gt 1 ] && tag="UNSTABLE"
    [ -z "$base" ] && base=$ms
    rel=$(awk "BEGIN{printf \"%+.1f%%\", ($base/$ms - 1)*100}")
    printf '%-16s %10s %10s %10s   %s  %s\n' "$n" "$ms" "$fps" "$xrt" "$tag" "$rel"
done

echo
allh=$(for n in $names; do
    sed 's/.*"frame_hash":"\([^"]*\)".*"wram_hash":"\([^"]*\)".*/\1 \2/' "$tmp/$n"
done | sort -u | wc -l)
if [ "$allh" -eq 1 ]; then
    echo "hashes: IDENTICAL across every variant and round"
else
    echo "hashes: *** $allh DISTINCT — a variant changed emulated behaviour ***"
    exit 1
fi

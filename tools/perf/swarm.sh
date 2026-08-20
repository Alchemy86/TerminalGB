#!/usr/bin/env bash
# How many emulated frames a second the SWARM delivers, and how that scales.
#
#   tools/perf/swarm.sh <rom.gb> [scenario.json] [out-dir]
#
# The unit is **emulated frames per second of wall clock across the whole
# fan-out**, which is `pewter_chain corpus`'s own last line. Episodes per second
# is not comparable between links (an episode is however long it is) and is not
# comparable with anybody else's harness; frames per second is both.
#
# ## Why it sweeps twice, in opposite directions
#
# This box drifts under sustained load. Measured here: the identical
# single-worker command read 10,646 frames/s on a quiet box and 7,405 after
# fifteen minutes of twenty-thread runs — a 30 % error, larger than most of the
# effects a sweep exists to show, and all of it in one direction. So every point
# is measured once going up and once coming down and both are printed: if the
# two disagree by more than a few per cent the box was drifting and the run is
# not a measurement. `COOL` seconds of idle between points keeps that small.
#
# Report the load average with any number this prints. `uptime` is echoed for
# exactly that reason.
#
# **Set `GB_PPU=standard`.** Every figure in `docs/throughput.md` and
# `docs/measured/speed-ledger.md` was measured on the whole-scanline engine,
# which was the default until 2026-08-20 and is not one now
# (`docs/picture-modes.md`). This script does not set it, deliberately — it
# would change what a run already in flight is measuring — so export it
# yourself, or the numbers are the per-dot engine's and about four times
# larger. A/B comparisons stay valid either way; absolute figures do not.

set -euo pipefail

ROM=${1:?usage: swarm.sh <rom.gb> [scenario.json] [out-dir]}
SCEN=${2:-scenarios/pewter/01-new-game.json}
OUT=${3:-target/perf-swarm}
BIN=${BIN:-./target/release/examples/pewter_chain}
# Attempts per worker. Ten is enough that per-attempt setup is amortised and
# short enough that a sweep finishes while the box is still in one state.
PER=${PER:-10}
WORKERS=${WORKERS:-"1 2 4 8 12 16 20"}
COOL=${COOL:-4}

[ -x "$BIN" ] || {
  echo "no $BIN — cargo build --release --example pewter_chain" >&2
  exit 2
}

point() { # $1 = workers -> prints frames/s
  local w=$1 d="$OUT/w$1-$2"
  rm -rf "$d"
  "$BIN" corpus --rom "$ROM" --scenario "$SCEN" --runs $((w * PER)) \
    --workers "$w" --out "$d" --quiet 2>&1 |
    sed -n 's/^\([0-9]*\) emulated frames\/s.*/\1/p'
  rm -rf "$d"
}

echo "rom      $ROM"
echo "scenario $SCEN"
echo "load     $(uptime)"
echo "cores    $(nproc)"
echo
printf '%8s %12s %12s %10s\n' workers up down 'x vs 1'

declare -A UP DOWN
for w in $WORKERS; do
  UP[$w]=$(point "$w" up)
  sleep "$COOL"
done
for w in $(echo "$WORKERS" | tr ' ' '\n' | tac | tr '\n' ' '); do
  DOWN[$w]=$(point "$w" down)
  sleep "$COOL"
done

base=""
for w in $WORKERS; do
  best=$((UP[$w] > DOWN[$w] ? UP[$w] : DOWN[$w]))
  [ -z "$base" ] && base=$best
  printf '%8s %12s %12s %9.2fx\n' "$w" "${UP[$w]}" "${DOWN[$w]}" \
    "$(echo "$best $base" | awk '{print $1/$2}')"
done
echo
echo "load     $(uptime)"

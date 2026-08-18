#!/usr/bin/env bash
# Collect a corpus for every link of the Pewter chain, fanned out.
#
#   tools/pewter-chain/collect.sh <rom> <entry-dir> <out-dir> [workers]
#
# Run counts are **scaled to the link**, not uniform: a bedroom does not need
# four hundred attempts and Viridian Forest is not settled by fifty. The box is
# shared, so the worker count leaves cores for other people — past the core
# count more workers only queue.
set -u
ROM=${1:?rom}
ENTRY=${2:?entry dir}
OUT=${3:?out dir}
W=${4:-8}
# **Screens are opt-in and bounded, because they are the one output whose size
# has to be thought about.** `$SCREENS=12` records a downscaled 80x72 frame
# every 12 emulated frames as `<out>/<link>/screen-NNNNN.gbscr`, which is what
# `tools/pewter-chain/screen_mosaic.py` tiles; `$SCREEN_RUNS` bounds how many
# attempts of each link record one. A position track is a few hundred KB and
# nobody has had to think about it; link 11 alone is ~7.5 MiB an agent. The
# capture prints its own estimate and refuses above `--screens-budget`.
# See docs/agent/screen-mosaic.md.
SCREENS=${SCREENS:-0}
SCREEN_RUNS=${SCREEN_RUNS:-60}
BIN=./target/release/examples/pewter_chain
S=scenarios/pewter

run() {  # link-number scenario runs
  local n=$1 sc=$2 runs=$3
  local prev
  prev=$(printf '%02d' $((10#$n - 1)))
  local args=(corpus --rom "$ROM" --scenario "$S/$sc.json" --runs "$runs"
              --workers "$W" --paths --out "$OUT/$sc")
  if [ "$SCREENS" != "0" ]; then
    args+=(--screens "$SCREENS" --screens-runs "$SCREEN_RUNS")
  fi
  if [ "$n" != "01" ] && [ -d "$ENTRY/$prev" ]; then
    args+=(--start-dir "$ENTRY/$prev")
  fi
  echo "=== $sc  ($runs attempts)"
  "$BIN" "${args[@]}" 2>&1 | tail -4
}

run 01 01-new-game                 120
run 02 02-leave-the-house          120
run 03 03-oak-takes-you-to-the-lab 400
run 04 04-take-a-starter           400
run 05 05-the-rival-battle         600
run 07 07-collect-oaks-parcel      200
run 08 08-carry-it-back-to-pallet  300
run 09 09-receive-the-pokedex      200
run 10 10-back-to-viridian         300
run 11 11-train-for-the-forest     120
run 12 12-heal-and-climb-route-2   300
run 13 13-cross-viridian-forest    400
run 14 14-arrive-in-pewter         200

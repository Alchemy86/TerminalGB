#!/bin/sh
# Build the chain's entry pools link by link, each from the last one's exits.
#
#   tools/pewter-chain/build_pool.sh <rom> <out-dir> <runs> <exits> [from] [to]
#
# Two things about it are load-bearing rather than convenient.
#
# **`--exits N` keeps the first N *successful* exits, so a pool is `N x
# pass_rate` and not N.** Ask for more runs than exits, and expect a pool to
# shrink at every link that is not 100 %.
#
# **Each pool is made read-only the moment it is finished.** A previous crew
# lost three hours to a pool that was rewritten by a stray job while a
# certification was reading it: `index.ndjson` recorded one size, the directory
# on disk held another, and nothing reproduced. A frozen pool is the cheap
# answer.
#
# `$SPEC` overrides the run and exit counts for named links, as
# `name:runs:exits` separated by spaces — because the expensive link in an arc
# is rarely the one whose rate is being quoted.
set -eu
ROM=$1; OUT=$2; RUNS=$3; EXITS=$4; FROM=${5:-1}; TO=${6:-30}
BIN=./target/release/examples/pewter_chain
SPEC=${SPEC:-}
mkdir -p "$OUT"
prev=${START_DIR:-}
for f in $(LC_ALL=C ls scenarios/pewter/*.json | LC_ALL=C sort); do
  s=$(basename "$f" .json)
  # The link's own number, from its file name — an insertion must not renumber
  # the links after it, and a shell glob sorts `18b` before `18` where the
  # chain does not.
  num=$(printf '%s' "$s" | sed 's/^\([0-9]*\).*/\1/')
  if [ "$num" -lt "$FROM" ] || [ "$num" -gt "$TO" ]; then
    [ -d "$OUT/$s/exits" ] && prev="$OUT/$s/exits"
    continue
  fi
  r=$RUNS; e=$EXITS
  for kv in $SPEC; do
    case $kv in
      "$s":*) r=$(printf '%s' "$kv" | cut -d: -f2); e=$(printf '%s' "$kv" | cut -d: -f3);;
    esac
  done
  echo "=== $s  (runs $r, exits $e)  $(date +%H:%M)"
  if [ -z "$prev" ]; then
    $BIN corpus --rom "$ROM" --scenario "$f" --runs "$r" --exits "$e" \
        --out "$OUT/$s" --no-tracks --quiet 2>&1 | tail -4
  else
    $BIN corpus --rom "$ROM" --scenario "$f" --start-dir "$prev" --runs "$r" \
        --exits "$e" --out "$OUT/$s" --no-tracks --quiet 2>&1 | tail -4
  fi
  prev="$OUT/$s/exits"
  chmod -R a-w "$prev" 2>/dev/null || true
done

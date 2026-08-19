#!/bin/sh
# Replay a link's own certified seeds with the binary in hand and require the
# trajectory hashes to match, hash for hash.
#
# **This is the cheap check that says whether a change reached a link it was
# not aimed at.** A corpus is a function of the entry state, the scenario, the
# seed AND the binary; a run whose trajectories reproduce is a run the change
# did not touch.
#
#   tools/pewter-chain/replay_check.sh <rom> <corpus-dir> <scenario> <start-dir> <n>
set -eu
ROM=$1; DIR=$2; SC=$3; START=$4; N=$5
BIN=./target/release/examples/pewter_chain
OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT
$BIN corpus --rom "$ROM" --scenario "$SC" --start-dir "$START" --runs "$N" \
     --out "$OUT" --no-tracks --quiet >/dev/null 2>&1
python3 - "$DIR/index.ndjson" "$OUT/index.ndjson" "$N" <<'PY'
import json, sys
def load(p):
    out = {}
    for line in open(p):
        r = json.loads(line)
        out[r["seed"]] = (r.get("trajectory"), r.get("frames"), r.get("decisions"),
                          r.get("ending"))
    return out
a, b, n = load(sys.argv[1]), load(sys.argv[2]), int(sys.argv[3])
same = diff = 0
for seed in sorted(b):
    if seed >= n or seed not in a:
        continue
    if a[seed] == b[seed]:
        same += 1
    else:
        diff += 1
        if diff <= 3:
            print(f"  seed {seed}: was {a[seed]} now {b[seed]}")
print(f"replay: {same} identical, {diff} different, of {same + diff} compared")
sys.exit(1 if diff else 0)
PY

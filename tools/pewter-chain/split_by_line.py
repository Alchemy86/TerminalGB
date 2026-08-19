#!/usr/bin/env python3
"""A link's pass rate, split by the starter line its entry state carries.

    python3 tools/pewter-chain/split_by_line.py <corpus-dir> <pool-listing> [--slot N]

Why this exists
---------------
A chain link's rate is an average over three quite different games. The
Charmander line cannot take either of the first two badges alone
(`docs/agent/brock.md`, `docs/agent/misty.md`), so a pooled figure hides the
only question anybody is asking — and once a bench is caught the *lead* is no
longer the starter, so "which line is this?" has to be read from the party slot
the starter was pushed into rather than from the front of the party.

`<corpus-dir>` is a `pewter_chain corpus` output directory: its `index.ndjson`
carries one row per attempt with the pool slot that attempt entered from.
`<pool-listing>` is the output of

    pewter_chain states --start-dir <that pool> --all --peek starter@0xD165

saved to a file — `states` prints the pool in the same sorted order `corpus`
takes `seed % pool` from, which is what makes the join exact rather than
approximate.

The interval is Wilson, never the normal approximation: at a rate near 1 the
normal one puts the upper bound above 100 %, and at a rate of 0 it returns
[0, 0] — certainty from no evidence.
"""
import argparse
import json
import math
import os
import re
import sys

# `wPartySpecies` holds INTERNAL indices. These three are read out of the
# cartridge by `gen1_tables.py --species <name>`; they are not Pokedex numbers.
LINES = {0xB0: "CHARMANDER", 0x99: "BULBASAUR", 0xB1: "SQUIRTLE",
         0xB2: "CHARMELEON", 0x9A: "IVYSAUR", 0xB3: "WARTORTLE",
         0x8E: "VENUSAUR", 0xB4: "BLASTOISE", 0xB5: "CHARIZARD"}
# Which family each form belongs to, so a pool of evolved starters still splits
# into three rows rather than six.
FAMILY = {"CHARMANDER": "CHARMANDER", "CHARMELEON": "CHARMANDER",
          "CHARIZARD": "CHARMANDER", "BULBASAUR": "BULBASAUR",
          "IVYSAUR": "BULBASAUR", "VENUSAUR": "BULBASAUR",
          "SQUIRTLE": "SQUIRTLE", "WARTORTLE": "SQUIRTLE",
          "BLASTOISE": "SQUIRTLE"}


def wilson(k, n, z=1.96):
    """A 95 % interval that behaves at both boundaries."""
    if n == 0:
        return (0.0, 0.0)
    p = k / n
    d = 1 + z * z / n
    c = (p + z * z / (2 * n)) / d
    h = z * math.sqrt(p * (1 - p) / n + z * z / (4 * n * n)) / d
    return (max(0.0, 100 * (c - h)), min(100.0, 100 * (c + h)))


def main():
    ap = argparse.ArgumentParser(prog="split_by_line.py", description=__doc__,
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("corpus", help="a pewter_chain corpus output directory")
    ap.add_argument("pool", help="`states --all --peek starter@0xD165` saved to a file")
    ap.add_argument("--peek", default="starter",
                    help="the --peek name to read the line from (default 'starter')")
    a = ap.parse_args()

    idx = os.path.join(a.corpus, "index.ndjson")
    if not os.path.exists(idx):
        print(f"error: no index.ndjson in {a.corpus}")
        print("help: point this at a `pewter_chain corpus --out DIR` directory")
        return 2

    # `states --all` prints one row per pool slot, in slot order, as
    # `  <n>  <lead> ... <peek>=0xNN ...`.
    by_slot = {}
    pat = re.compile(r"^\s*(\d+)\s+\S+.*?\b" + re.escape(a.peek) + r"=0x([0-9A-Fa-f]{2})")
    for line in open(a.pool, encoding="utf-8", errors="replace"):
        m = pat.match(line)
        if m:
            by_slot[int(m.group(1))] = int(m.group(2), 16)
    if not by_slot:
        print(f"error: no '{a.peek}=0xNN' rows in {a.pool}")
        print("help: pewter_chain states --start-dir <pool> --all "
              f"--peek {a.peek}@0xD165 > {a.pool}")
        return 2

    rows, unknown = {}, 0
    total_k = total_n = 0
    for line in open(idx, encoding="utf-8"):
        r = json.loads(line)
        sp = by_slot.get(r.get("entry"))
        fam = FAMILY.get(LINES.get(sp, ""), None)
        if fam is None:
            unknown += 1
            continue
        k, n = rows.get(fam, (0, 0))
        rows[fam] = (k + (1 if r.get("goal") else 0), n + 1)
        total_k += 1 if r.get("goal") else 0
        total_n += 1

    print(f"split[{len(rows)}]{{line,passed,attempts,rate,ci_low,ci_high}}:")
    for fam in sorted(rows, key=lambda f: -rows[f][1]):
        k, n = rows[fam]
        lo, hi = wilson(k, n)
        print(f"  {fam},{k},{n},{100*k/n:.1f},{lo:.1f},{hi:.1f}")
    lo, hi = wilson(total_k, total_n)
    print(f"total:\n  passed: {total_k}\n  attempts: {total_n}\n"
          f"  rate: {100*total_k/total_n:.1f}\n  ci: {lo:.1f}-{hi:.1f}")
    if unknown:
        print(f"  entries whose line could not be read: {unknown}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

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

# **THE LINES ARE READ OUT OF THE CARTRIDGE, NEVER WRITTEN DOWN HERE.** An
# internal species index is not a Pokedex number and the two differ throughout
# Gen 1 — the first version of this file had IVYSAUR at 0x9A, which is VENUSAUR,
# and silently dropped 1,091 of 3,000 attempts into an "unknown line" bucket
# while reporting a confident total. `gen1_tables.py` reads `MonsterNames` and
# `EvosMovesPointerTable` off the ROM in hand, so the three families fall out of
# walking each starter's own evolutions.
STARTERS = ("BULBASAUR", "CHARMANDER", "SQUIRTLE")


def families(rom_path):
    """{internal index: family head name}, derived from the cartridge."""
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path.insert(0, os.path.join(here, "..", "gen1-tables"))
    import gen1_tables as g
    rom = g.read_rom(rom_path)
    names, _ = g.species_names(rom)
    ev = g.EvosMoves(rom)
    by_name = {v.upper(): k for k, v in names.items()}
    out = {}
    for head in STARTERS:
        i = by_name.get(head)
        if i is None:
            continue
        seen = set()
        stack = [i]
        while stack:
            cur = stack.pop()
            if cur in seen:
                continue
            seen.add(cur)
            out[cur] = head
            stack.extend(e["into"] for e in ev.evolutions.get(cur, []))
    return out


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
    ap.add_argument("--rom", help="the cartridge (or $POKE_ROM) — the species "
                                  "indices are read from it, never typed in")
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

    rom = a.rom or os.environ.get("POKE_ROM")
    if not rom:
        print("error: no cartridge given, and the species indices come from it")
        print("help: split_by_line.py <corpus> <pool> --rom <your Red or Blue>.gb")
        print("help: or: export POKE_ROM=<your Red or Blue>.gb")
        return 2
    try:
        fam_of = families(rom)
    except SystemExit as e:
        print(f"error: could not read the cartridge: {e}")
        return 2

    rows, unknown = {}, 0
    total_k = total_n = 0
    for line in open(idx, encoding="utf-8"):
        r = json.loads(line)
        sp = by_slot.get(r.get("entry"))
        fam = fam_of.get(sp)
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
    # **A silent unknown bucket is how a wrong species table hides**, so this
    # is loud: an attempt whose line could not be read is in NEITHER column and
    # the count is printed whenever it is not zero.
    if unknown:
        print(f"  attempts whose line could not be read: {unknown}"
              f"  <- these are in neither column")
    return 0


if __name__ == "__main__":
    sys.exit(main())

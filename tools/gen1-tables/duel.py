#!/usr/bin/env python3
"""Which catchable Pokemon actually beats a given trainer, and at what level.

    python3 tools/gen1-tables/duel.py --rom <blue.gb> --against misty
    python3 tools/gen1-tables/duel.py --rom <rom> --against brock --levels 16,20,24
    python3 tools/gen1-tables/duel.py --rom <rom> --against misty --candidates pikachu,paras

Why this exists
---------------
`gen1_tables.py --plan misty` answers "what TYPE beats this?" and it is not
enough, twice over. A species can carry the right type and hold no move of it
— PARAS is BUG/GRASS and learns **no damaging GRASS move by level-up, ever** —
and a species can hit for double and still lose, because what decides a Gen 1
gym is as much what comes back. Misty's STARMIE lands a same-type WATER GUN;
ELECTRIC does not resist it and GRASS halves it, and that one multiplication is
the difference between a bench that takes the badge and one that faints.

So this puts each candidate through the cartridge's own `CalculateDamage`
against the trainer's own party and reports **turns to kill against turns to
die**, on the worst of Gen 1's 217..255 rolls, at whatever levels you name.

Everything about the game is read out of the cartridge by `gen1_tables.py` —
the trainer's party from `TrainerDataPointers`, base stats and starting moves
from `BaseStats`, learnsets and evolutions from `EvosMovesPointerTable`, each
move's power, type and accuracy from `Moves`, and the multipliers from
`TypeEffects`. Nothing is vendored and nothing is typed in.

What it does NOT model, said plainly
------------------------------------
Critical hits, status, stat stages, held-move PP, the opponent's own move
choice and the 1-in-256 miss. It is a **ranking instrument**, not a battle
engine — `crates/terminalgb-gen1-battle` is that — and its whole claim is that
the ranking it produces is the one the emulator then reproduces. It is
calibrated rather than trusted: run it against Misty and it must put IVYSAUR
and WARTORTLE above the line at level 24 and CHARMELEON below it at every
level, which is what three certified 3,000-attempt runs measured.
"""
import argparse
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen1_tables as g  # noqa: E402

SELF = "python3 tools/gen1-tables/duel.py"

# The lowest of `RandomizeDamage`'s 217..255 multipliers. Every figure here is
# the worst roll, because a bench is chosen for the fight it has to survive.
WORST_ROLL = 217

# `wEnemyMonDVs` for a trainer's Pokemon: `$98 $88`, read out of a real
# mid-fight state rather than recalled. The HP DV is assembled from the low bit
# of the other four, which is the cartridge's own rule.
TRAINER_DVS = (9, 8, 8, 8)

# A caught Pokemon's DVs are two random bytes, so the middle of the range is
# what a sweep should rank on; `--dvs` moves it.
DEFAULT_DVS = (8, 8, 8, 8)

# Gen 1 splits a move into physical and special by its TYPE, not per move:
# everything from FIRE upwards is special (`constants/type_constants.asm` puts
# the special types at $14 and above).
FIRST_SPECIAL_TYPE = 0x14


def stat(base, dv, level, hp=False):
    """`CalcStat`, with no stat experience — which is what a wild catch has."""
    v = ((base + dv) * 2) * level // 100
    return v + level + 10 if hp else v + 5


class Mon:
    """One side of the duel, built the way the cartridge builds one."""

    def __init__(self, rom, tables, internal, level, dvs=DEFAULT_DVS, moves=None):
        names, dex_of, stats, stats_at, mv, ev, _chart = tables
        # Evolve as far as this level allows: `EvolveTradeMon`'s level entries
        # are the reason a threshold and a form are the same question.
        while True:
            up = [e for e in ev.evolutions[internal]
                  if e["kind"] == "level" and e["level"] <= level]
            if not up:
                break
            internal = up[0]["into"]
        self.internal, self.level, self.name = internal, level, names[internal]
        b = stats[dex_of[internal]]
        a, d, s, c = dvs
        hp_dv = ((a & 1) << 3) | ((d & 1) << 2) | ((s & 1) << 1) | (c & 1)
        self.max_hp = stat(b["hp"], hp_dv, level, hp=True)
        self.atk = stat(b["atk"], a, level)
        self.dfn = stat(b["def"], d, level)
        self.spd = stat(b["spd"], s, level)
        self.spc = stat(b["spc"], c, level)
        self.types = b["types"]
        if moves is not None:
            self.moves = list(moves)
            return
        # `WriteMonMoves`: the last four the species has learned by this level.
        held = list(g.starting_moves(rom, stats_at, dex_of[internal]))
        for lvl, m in ev.learnset[internal]:
            if lvl <= level and m not in held:
                held.append(m)
                if len(held) > 4:
                    held.pop(0)
        self.moves = held[-4:]


def damage(tables, att, dfd, move):
    """`CalculateDamage` then `AdjustDamageForMoveType`, at the worst roll."""
    mv, chart = tables[4], tables[6]
    r = mv.rows[move]
    if r["power"] == 0:
        return 0
    special = r["type"] >= FIRST_SPECIAL_TYPE
    a = att.spc if special else att.atk
    d = dfd.spc if special else dfd.dfn
    dmg = ((2 * att.level // 5 + 2) * r["power"] * a // d) // 50 + 2
    if r["type"] in att.types:
        dmg += dmg // 2                      # STAB, as an integer half
    for t in dict.fromkeys(dfd.types):       # each of the defender's types once
        dmg = dmg * chart.multiplier(r["type"], t) // 100
    return dmg * WORST_ROLL // 255


def best(tables, att, dfd):
    return max(((damage(tables, att, dfd, m), m) for m in att.moves),
               default=(0, 0))


def turns(hp, per):
    return math.ceil(hp / per) if per > 0 else None


def main():
    ap = argparse.ArgumentParser(
        prog="duel.py", add_help=True,
        description="Which catchable Pokemon beats a trainer, and at what level.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=f"""examples:
  {SELF} --against misty
  {SELF} --against misty --candidates pikachu,paras,bellsprout
  {SELF} --against brock --levels 12,16,20

the cartridge:
  --rom is required, or set POKE_ROM. Every table is read out of the cartridge
  you supply, located by the signature its own bytes make.

output:
  TOON on stdout. Errors are on stdout too, with a suggestion. Exit 0 success,
  1 the query failed, 2 the command line was wrong.""")
    ap.add_argument("--rom", metavar="FILE", help="the cartridge to read")
    ap.add_argument("--against", metavar="CLASS", required=True,
                    help="the trainer class to fight, e.g. misty")
    ap.add_argument("--roster", metavar="N", type=int, default=1,
                    help="which of that class's parties (default 1)")
    ap.add_argument("--candidates", metavar="LIST",
                    help="comma-separated species; default is everything "
                         "catchable that answers the trainer, plus the three "
                         "starter lines as controls")
    ap.add_argument("--levels", metavar="LIST", default="14,18,21,24,27,30",
                    help="comma-separated levels to sweep (default "
                         "14,18,21,24,27,30)")
    ap.add_argument("--max-level", metavar="N", type=int, default=22,
                    help="level ceiling for the automatic candidate list")
    ap.add_argument("--dvs", metavar="A,D,S,C", default="8,8,8,8",
                    help="the candidate's DVs (default 8,8,8,8 — mid-range)")
    ap.add_argument("--json", action="store_true", help="emit JSON")
    a = ap.parse_args()
    if a.json:
        g._JSON = True

    path = a.rom or os.environ.get("POKE_ROM")
    if not path:
        g.die("no cartridge given",
              f"{SELF} --rom <your Red or Blue>.gb --against {a.against}",
              "or: export POKE_ROM=<your Red or Blue>.gb", code=2)
    try:
        rom = g.read_rom(path)
    except OSError as e:
        g.die(f"cannot read {path}: {e}", f"{SELF} --rom <path> --against misty",
              code=2)

    names, _ = g.species_names(rom)
    dex_of, _ = g.pokedex_order(rom)
    stats, stats_at = g.base_stats(rom, dex_of)
    mv = g.Moves(rom)
    ev = g.EvosMoves(rom)
    chart = g.TypeChart(rom)
    tables = (names, dex_of, stats, stats_at, mv, ev, chart)
    internal_of = {v: k for k, v in names.items()}

    trainers = g.Trainers(rom, names)
    key = a.against.replace(".", "").replace(" ", "").upper()
    cls = trainers.by_name.get(key)
    if cls is None:
        g.die(f"no trainer class called '{a.against}' in this cartridge",
              (f"did you mean: {', '.join(g.near(key, trainers.by_name))}"
               if g.near(key, trainers.by_name) else
               "python3 tools/gen1-tables/gen1_tables.py --gyms  "
               "(every leader's own party)"), code=1)
    rosters = trainers.rosters(cls)
    if not rosters or a.roster > len(rosters):
        g.die(f"trainer class '{a.against}' has {len(rosters)} roster(s), "
              f"not {a.roster}",
              f"{SELF} --against {a.against} --roster 1", code=1)
    # `_roster` returns (level, internal index) pairs, in the cartridge's own
    # order — which is the order the leader sends them out in.
    foes = [Mon(rom, tables, internal, level, TRAINER_DVS)
            for level, internal in rosters[a.roster - 1]]
    g.table("opponent", [{
        "species": f.name, "level": f.level, "hp": f.max_hp, "atk": f.atk,
        "def": f.dfn, "spd": f.spd, "spc": f.spc,
        "types": "/".join(dict.fromkeys(g.TYPES[t] for t in f.types)),
    } for f in foes],
        ["species", "level", "hp", "atk", "def", "spd", "spc", "types"])

    if a.candidates:
        # `resolve_species` refuses a typo itself, with near matches.
        want = [g.resolve_species(w.strip(), names) for w in a.candidates.split(",")]
    else:
        defender = foes[0].types[0]
        cands = g.candidates_against(rom, defender, a.max_level, chart,
                                     g.Wild(rom, names), names, dex_of, stats)
        want = []
        for c in cands:
            if c["internal"] not in want:
                want.append(c["internal"])
        for control in ("BULBASAUR", "SQUIRTLE", "CHARMANDER"):
            if control in internal_of:
                want.append(internal_of[control])

    levels = [int(x) for x in a.levels.split(",")]
    dvs = tuple(int(x) for x in a.dvs.split(","))
    rows = []
    for internal in want:
        for level in levels:
            m = Mon(rom, tables, internal, level, dvs)
            worst_kill, worst_die, move = 0, 10**6, ""
            for f in foes:
                d, best_move = best(tables, m, f)
                k = turns(f.max_hp, d)
                inc, _ = best(tables, f, m)
                t = turns(m.max_hp, inc)
                if k is None:
                    k = 999
                if t is None:
                    t = 999
                if k >= worst_kill:
                    worst_kill, move = k, mv.name(best_move) if best_move else "-"
                worst_die = min(worst_die, t)
            rows.append({
                "catch": names[internal], "level": level, "form": m.name,
                "best_move": move, "turns_to_kill": worst_kill,
                "turns_to_die": worst_die,
                # A margin, so the table sorts and a reader can see how wide
                # the win is rather than only that there was one.
                "verdict": "WIN" if worst_kill < worst_die else
                           ("edge" if worst_kill == worst_die else "lose"),
            })
    g.table("duels", rows,
            ["catch", "level", "form", "best_move", "turns_to_kill",
             "turns_to_die", "verdict"])
    g.note("turns are against the WORST of Gen 1's 217..255 damage rolls, and "
           "against whichever of the trainer's Pokemon is hardest to kill and "
           "quickest to kill you. No critical hits, no status, no stat stages.")
    g.helps([
        f"{SELF} --against {a.against} --candidates <species,species>",
        f"{SELF} --against {a.against} --levels 20,24,28",
        "python3 tools/gen1-tables/gen1_tables.py --where <species>  "
        "(which maps one is catchable on)",
    ])
    g.flush_json()
    return 0


if __name__ == "__main__":
    sys.exit(main())

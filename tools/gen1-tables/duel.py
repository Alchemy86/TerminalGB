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

**A NARROW MARGIN IS NOT A PREDICTION.** That calibration works because Misty's
margins are wide — 3 turns against 9, and 12 against 3 — and a wide margin
survives everything left out. Against Brock the same command calls a level 16
CHARMELEON a win at 6 turns against 9, where the chain measures that line at
67 %: his GEODUDE uses DEFENSE CURL, which is a stat stage, and six turns is
long enough for one critical hit to decide it. Read a two- or three-turn gap as
"too close to call".
"""
import argparse
import math
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen1_tables as g  # noqa: E402
import learn_table as lt  # noqa: E402

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
    """One side of the duel, built the way the cartridge builds one.

    `internal` is the species as it is **caught** or generated. It is evolved
    forward through every level evolution at or below `level`, because a level
    threshold and a form are the same question in Gen 1.

    The moveset has two honest answers and this takes whichever is asked for.

    * **Caught at this level** (`caught_at=None`, the default) —
      `WriteMonMoves`: the last four the species has learned by then, oldest
      pushed out. This is what a wild one you meet at that level holds.
    * **Caught at `caught_at` and trained up** — the level-up prompt asks
      instead of pushing, and `learn_table.decide` is the criterion this
      project answers it with. It protects the last damaging move, so a
      BELLSPROUT trained from 13 still holds VINE WHIP at 24 where one caught
      at 24 does not.

    **The two are different Pokemon on paper and conflating them flatters the
    higher levels.** A PIKACHU caught at 27 has already pushed THUNDERSHOCK out
    in favour of SWIFT; one trained from 5 keeps it.
    """

    def __init__(self, rom, tables, internal, level, dvs=DEFAULT_DVS, moves=None,
                 caught_at=None):
        names, dex_of, stats, stats_at, mv, ev, _chart = tables
        start = internal
        form = internal
        for lv in range(1, level + 1):
            for e in ev.evolutions[form]:
                if e["kind"] == "level" and e["level"] <= lv:
                    form = e["into"]
        self.internal, self.level, self.name = form, level, names[form]
        b = stats[dex_of[form]]
        a, d, s_, c = dvs
        hp_dv = ((a & 1) << 3) | ((d & 1) << 2) | ((s_ & 1) << 1) | (c & 1)
        self.max_hp = stat(b["hp"], hp_dv, level, hp=True)
        self.atk = stat(b["atk"], a, level)
        self.dfn = stat(b["def"], d, level)
        self.spd = stat(b["spd"], s_, level)
        self.spc = stat(b["spc"], c, level)
        self.types = b["types"]
        if moves is not None:
            self.moves = list(moves)
        elif caught_at is None:
            self.moves = lt.moveset_at(rom, ev, stats_at, dex_of, form, level)
        else:
            self.moves = Mon._trained(rom, tables, start, caught_at, level)

    @staticmethod
    def _trained(rom, tables, start, caught_at, level):
        """The moves a Pokemon caught at `caught_at` holds once trained."""
        _, dex_of, stats, stats_at, mv, ev, _ = tables
        form = start
        for lv in range(1, caught_at + 1):
            for e in ev.evolutions[form]:
                if e["kind"] == "level" and e["level"] <= lv:
                    form = e["into"]
        held = list(lt.moveset_at(rom, ev, stats_at, dex_of, form, caught_at))
        for lv in range(caught_at + 1, level + 1):
            for e in ev.evolutions[form]:
                if e["kind"] == "level" and e["level"] == lv:
                    form = e["into"]
            for lvl, m in ev.learnset[form]:
                if lvl != lv or m in held:
                    continue
                if len(held) < 4:
                    held.append(m)
                    continue
                loser, _ = lt.decide(mv, held, m, stats[dex_of[form]]["types"])
                if loser is not None and loser != m:
                    held[held.index(loser)] = m
        return held


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
    ap.add_argument("--caught-at", metavar="N", type=int,
                    help="treat each candidate as CAUGHT at level N and trained "
                         "up, applying the level-up prompt's own criterion, "
                         "instead of generated at each swept level")
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
            caught = a.caught_at if a.caught_at and a.caught_at < level else None
            m = Mon(rom, tables, internal, level, dvs, caught_at=caught)
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
    if a.caught_at:
        g.note(f"movesets are those of a Pokemon CAUGHT at level {a.caught_at} and "
               "trained up, through the level-up prompt's own criterion — which "
               "is a different Pokemon from one generated at the swept level.")
    else:
        g.note("movesets are WriteMonMoves' — the last four the species has "
               "learned by the swept level, which is what a wild one you meet "
               "there holds. Use --caught-at N for one caught low and trained.")
    g.note("turns are against the WORST of Gen 1's 217..255 damage rolls, and "
           "against whichever of the trainer's Pokemon is hardest to kill and "
           "quickest to kill you. No critical hits, no status, no stat stages.")
    g.helps([
        f"{SELF} --against {a.against} --candidates <species,species>",
        f"{SELF} --against {a.against} --levels 20,24,28",
        f"{SELF} --against {a.against} --caught-at 12   "
        "(caught low and trained, not generated at each level)",
        "python3 tools/gen1-tables/gen1_tables.py --where <species>  "
        "(which maps one is catchable on)",
    ])
    g.flush_json()
    return 0


if __name__ == "__main__":
    sys.exit(main())

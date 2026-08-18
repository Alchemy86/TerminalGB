#!/usr/bin/env python3
"""Every move-learning decision Pokémon Red or Blue can ever ask, enumerated.

    python3 tools/gen1-tables/learn_table.py --rom <blue.gb>
    python3 tools/gen1-tables/learn_table.py --rom <rom> --decisions
    python3 tools/gen1-tables/learn_table.py --rom <rom> --species charmander
    python3 tools/gen1-tables/learn_table.py --rom <rom> --evolutions

Why this exists
---------------
When a Pokémon levels up already knowing four moves the game stops and asks
which one to forget. That looks like an open-ended judgement and it is not:
there are 151 species, and **every level-up learnset is fixed data in the
cartridge**, so the set of questions the game can ever ask is finite and
countable. This counts it, and it applies one stated rule to every entry so the
answer is a lookup rather than a piece of reasoning.

Everything about the *game* here is read out of the cartridge by
`gen1_tables.py` — the learnsets and evolutions from `EvosMovesPointerTable`,
each move's power, type, accuracy and PP from `Moves`, each species' own types
from `BaseStats`. Nothing is vendored and nothing is typed in.

The *rule* is ours, and §"the rule" below states it so a reader can disagree
with it. TMs and HMs are deliberately out of scope: a TM is a separate and much
larger problem (55 machines, each usable on any compatible species at any
level), and nothing here reads the TM tables.
"""
import argparse
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import gen1_tables as g  # noqa: E402

STAB = 1.5          # `CalculateDamage` multiplies by 1.5 for same-type moves.
CLOSE_MARGIN = 0.10  # within 10% of each other: the rule is not deciding much.

# **Three effect numbers say "the damage is not in this table", and they fall
# out of the cartridge rather than being recalled.** Grouping every move whose
# power byte is 0 or 1 by its effect byte gives, on this cartridge:
#
#     effect 38: GUILLOTINE, HORN DRILL, FISSURE          one-hit knock-out
#     effect 40: SUPER FANG                               halves the target
#     effect 41: SONICBOOM, SEISMIC TOSS, DRAGON RAGE,    fixed damage
#                NIGHT SHADE, PSYWAVE
#
# All seven carry a power byte of 1 (NIGHT SHADE's is 0) — a placeholder, since
# the damage is computed by the effect's own code. Ranking them on
# `power x accuracy` therefore puts a HORN DRILL below a POUND, which is the
# one systematic error this rule could make, so it refuses to price them at all.
UNPRICED_EFFECTS = {38, 40, 41}


# --- the rule ----------------------------------------------------------------
#
# THE CRITERION, stated so it can be argued with.
#
# 0. **A move whose damage is not in the table is never forgotten by this
#    rule.** The seven `UNPRICED_EFFECTS` moves carry a power byte of 1 or 0
#    because their damage is computed by the effect's own code, so ranking
#    them on power would drop a HORN DRILL for a POUND. That is the one
#    systematic error available here, so the rule declines rather than risks
#    it. If the *offered* move is one of these it is accepted, at the cost of
#    the worst priceable move held.
# 1. **The last damaging move is never forgotten.** A Gen 1 Pokémon with no
#    damaging move cannot end a battle; it STRUGGLEs. Forgetting is permanent
#    at this prompt, so the one catastrophe available here gets a rule of its
#    own rather than falling out of a number.
# 2. **A damaging move outranks a non-damaging one.** Justification is measured
#    rather than assumed: `docs/agent/battle-policy.md` reports the certified
#    Pewter chain spending 2,450 turns on stat moves across 3,000 attempts of
#    one trainer battle and *nothing at all* on any other non-damaging move —
#    every other turn is damage.
# 3. **Within the damaging moves, order by expected damage per turn**,
#    `power x accuracy/255 x STAB`. Power, type and accuracy are the
#    cartridge's own three bytes; STAB is 1.5 when the move's type is one of the
#    holder's, which is what `CalculateDamage` applies. PP breaks a tie, then
#    the move number, so the rule is deterministic.
# 4. **Non-damaging moves are NOT ranked against each other**, and that is a
#    stated limitation rather than an oversight: a move's effect is an index
#    into a jump table of *code* (`constants/move_effect_constants.asm`), so
#    there is no number in the table that says SLEEP POWDER is worth more than
#    GROWL. They tie on PP and then on move number.
#
# Then: rank the five moves (four held plus the one offered) and drop the
# loser. If the loser is the offered move, refuse it.
#
# What the rule deliberately does NOT use: the rest of the party, the fights
# ahead, the move's effect, and the opponent. Those are the context cases and
# they are counted separately.

def unpriced(mv, move):
    return mv.rows[move]["effect"] in UNPRICED_EFFECTS


def value(mv, move, holder_types):
    """Expected damage per turn, or `None` when the table cannot say."""
    r = mv.rows[move]
    if unpriced(mv, move) or r["power"] == 0:
        return None
    stab = STAB if r["type"] in holder_types else 1.0
    return r["power"] * (r["accuracy"] / 255.0) * stab


def rank_key(mv, move, holder_types):
    """Sort key: worst first. Status moves sort below every damaging move."""
    v = value(mv, move, holder_types)
    r = mv.rows[move]
    if v is None:
        return (0, r["pp"], -move)
    return (1, v, r["pp"], -move)


def decide(mv, held, offered, holder_types):
    """Which of `held` to forget, or the offered move itself to refuse it."""
    priced = [m for m in held if mv.rows[m]["power"] > 0
              and not unpriced(mv, m)]
    protected = {m for m in held if unpriced(mv, m)}      # rule 0
    if len(priced) == 1 and value(mv, offered, holder_types) is None:
        protected.add(priced[0])                          # rule 1
    pool = [m for m in list(held) + [offered] if m not in protected]
    if not pool:
        return None, None                                 # nothing to decide
    pool.sort(key=lambda m: rank_key(mv, m, holder_types))
    loser = pool[0]
    runner_up = pool[1] if len(pool) > 1 else None
    return loser, runner_up


# --- what a Pokémon holds the moment you get it -------------------------------

def moveset_at(rom, em, stats_at, dex_of, sp, level):
    """The four moves a species of `sp` at `level` is generated with.

    Gen 1 builds a wild or trainer Pokémon's moves from the species' four
    base-stats moves and then applies its learnset up to the level, pushing the
    oldest out when all four slots are full (`WriteMonMoves`,
    `engine/pokemon/add_mon.asm`). So **a caught Pokémon's move set is a
    function of species and level and of nothing else** — which is what makes
    the set of starting states enumerable rather than open.
    """
    held = [m for m in g.starting_moves(rom, stats_at, dex_of[sp]) if m]
    for lv, m in em.learnset[sp]:
        if lv > level or m in held:
            continue
        if len(held) < 4:
            held.append(m)
        else:
            held = held[1:] + [m]
    return held


# --- walking one evolution line ----------------------------------------------

def base_forms(em, names):
    """Species that nothing evolves into: the head of each line."""
    into = {e["into"] for evs in em.evolutions.values() for e in evs}
    return [i for i in names if i not in into]


def line_prompts(rom, em, mv, stats, dex_of, stats_at, start, evolve_teaches,
                 from_level=1, held=None, stone=None):
    """Walk one trajectory forward and return every four-move prompt on it.

    A trajectory is: obtained as `start` at `from_level` holding `held`, then
    levelled to 100, learning everything it is offered, never refusing and
    never cancelling an evolution. With `stone`, a stone or trade evolution is
    taken the moment it becomes possible — the player's own choice, and the
    only branch in the whole walk.
    """
    sp = start
    out, counted = [], {"free": 0, "already": 0, "prompt": 0, "start": 0}
    dex = dex_of.get(sp, 0)
    if not (1 <= dex <= 150):
        return out, counted
    if held is None:
        held = [m for m in g.starting_moves(rom, stats_at, dex) if m]
    held = list(held)
    counted["start"] = len(held)
    if stone is not None:
        opts = [e for e in em.evolutions[sp] if e["kind"] != "level"]
        if stone < len(opts):
            sp = opts[stone]["into"]
            dex = dex_of.get(sp, 0)
    for level in range(from_level + 1, 101):
        # Everything the game can offer at this level, in the order it happens:
        # the current species' own learnset entry, then — if it evolves here —
        # the evolved form's entry for the same level.
        offers = [(sp, m) for lv, m in em.learnset[sp] if lv == level]
        evo = next((e for e in em.evolutions[sp]
                    if e["kind"] == "level" and e["level"] == level), None)
        if evo:
            sp = evo["into"]
            dex = dex_of.get(sp, 0)
            if evolve_teaches:
                offers += [(sp, m) for lv, m in em.learnset[sp] if lv == level]
        for owner, move in offers:
            if move in held:
                counted["already"] += 1
                continue
            types = set(stats[dex_of[owner]]["types"]) if dex_of.get(owner) else set()
            if len(held) < 4:
                counted["free"] += 1
                held.append(move)
                continue
            counted["prompt"] += 1
            loser, runner_up = decide(mv, held, move, types)
            out.append({
                "species": owner, "level": level, "offered": move,
                "held": list(held), "types": types,
                "loser": loser, "runner_up": runner_up,
            })
            if loser is not None and loser != move:
                held[held.index(loser)] = move
    return out, counted


def classify(mv, d):
    """Flag a decision with what makes it interesting."""
    mvs, held, offered, loser = mv, d["held"], d["offered"], d["loser"]
    out = set()
    if any(unpriced(mvs, m) for m in list(held) + [offered]):
        out.add("unpriced-present")
    if loser is None:
        out.add("no-answer")
        return out
    if loser == offered:
        out.add("refuse")
    elif mvs.rows[loser]["power"] == 0:
        out.add("status-victim")
    else:
        out.add("damage-victim")
        # Coverage: after the swap, does the holder still have a damaging move
        # of the victim's type? If not, another party member holding that type
        # — or the fights ahead — could reasonably change the answer.
        after = [m for m in held if m != loser] + [offered]
        t = mvs.rows[loser]["type"]
        if not any(mvs.rows[m]["power"] > 0 and mvs.rows[m]["type"] == t
                   for m in after):
            out.add("coverage-loss")
    if d["runner_up"] is not None:
        a = value(mvs, loser, d["types"])
        b = value(mvs, d["runner_up"], d["types"])
        if a is None and b is None:
            # Two moves the table cannot rank against each other. The rule
            # falls through to PP, which is arbitrary, and says so.
            out.add("status-tie")
        elif a is not None and b is not None and b > 0:
            if (b - a) / b <= CLOSE_MARGIN:
                out.add("close-damage")
    return out


# --- the drill plan ----------------------------------------------------------
#
# THE PLAN IS THE ONLY PLACE THE CRITERION LIVES. `examples/gen1_learn_drill.rs`
# triggers the level-ups and works the menus; it does not decide anything. That
# split is deliberate and it is the same one `docs/agent/battle-policy.md`
# records for the battle brain: **when one component decides something, no other
# component may decide the same thing from its own arithmetic**, because two
# implementations that agree today are two implementations that will disagree
# the next time either is improved, and the gap is silent.
#
# So the rule above is applied here, once, and what crosses to the emulator is a
# list of level-ups with the answer already in it.

def why(mv, held, offered, loser, runner_up, types):
    """One line saying why the rule chose what it chose, for the corpus."""
    if loser is None:
        return "every held move is protected; the rule declines"
    v = value(mv, offered, types)
    if loser == offered:
        best = "the offered move is worth less than all four held"
        if v is None:
            best = ("the offered move deals no damage the table can price, "
                    "and no held move is worse")
        return best
    lv = value(mv, loser, types)
    if lv is None:
        return (f"{mv.name(loser)} deals no damage the table can price and "
                f"{mv.name(offered)} does")
    ru = f", next worst {mv.name(runner_up)}" if runner_up else ""
    return (f"{mv.name(loser)} is the worst priceable move held "
            f"({lv:.1f} expected damage a turn against {v:.1f}){ru}")


def drill_plan(rom, em, mv, names, dex_of, stats, stats_at, start,
               evolve_teaches):
    """Every level-up on one line that offers a move, with the answer.

    Walks from level 1 taking every evolution — the measured rule is that
    **nothing is ever lost by evolving** (`docs/gen1/discoveries.md`), so
    refusing one would be drilling a line no player walks.

    Each entry carries what the emulator needs to *reach* that level-up
    directly rather than grinding to it: the species and level to arm, and the
    four moves in slot order. `forget_slot` is 1-based because that is what the
    game's own move list shows.
    """
    sp = start
    held = [m for m in g.starting_moves(rom, stats_at, dex_of[start]) if m]
    line, events = [names[sp]], []
    for level in range(2, 101):
        offers = [(sp, m) for lv, m in em.learnset[sp] if lv == level]
        evo = next((e for e in em.evolutions[sp]
                    if e["kind"] == "level" and e["level"] == level), None)
        was, evolved = sp, False
        if evo:
            sp, evolved = evo["into"], True
            line.append(names[sp])
            if evolve_teaches:
                offers += [(sp, m) for lv, m in em.learnset[sp] if lv == level]
        for owner, move in offers:
            e = {
                "level": level,
                "arm_level": level - 1,
                # The species to put on the field. It is the form held at
                # `level - 1`, which is the PRE-evolution one when the line
                # evolves on this very level.
                "arm_species": names[was],
                # **The internal index, because a NAME is not unique.** The two
                # NIDORAN differ only by a gender glyph, and every decoder that
                # drops non-alphanumerics — ours and the emulator's alike — maps
                # both to `NIDORAN`, so `resolve_species` returns whichever
                # comes first. Measured: the drill armed a NIDORAN male for the
                # female's plan and learned HORN ATTACK where the plan said
                # SCRATCH. The index is what the party actually stores.
                "arm_index": was,
                "species": names[owner],
                "species_index": owner,
                "evolves_here": evolved,
                "held": [mv.name(m) for m in held],
                "offered": mv.name(move),
            }
            if move in held:
                # A real entry in the cartridge's table that can never produce
                # a prompt: the Pokémon already knows the move. Counted, so the
                # totals add up, and never drilled.
                e.update(kind="already-held", answer="nothing",
                         after=[mv.name(m) for m in held], flags=[],
                         why="the move is already in a slot")
                events.append(e)
                continue
            types = set(stats[dex_of[owner]]["types"]) if dex_of.get(owner) else set()
            if len(held) < 4:
                held.append(move)
                e.update(kind="free", answer="learn",
                         after=[mv.name(m) for m in held], flags=[],
                         why=f"only {len(held) - 1} of the four slots were full")
                events.append(e)
                continue
            loser, runner_up = decide(mv, held, move, types)
            d = {"held": list(held), "offered": move, "loser": loser,
                 "runner_up": runner_up, "types": types}
            e["flags"] = sorted(classify(mv, d))
            e["runner_up"] = mv.name(runner_up) if runner_up else None
            e["why"] = why(mv, held, move, loser, runner_up, types)
            if loser is None or loser == move:
                e.update(kind="decision", answer="decline",
                         after=[mv.name(m) for m in held])
            else:
                slot = held.index(loser)
                e.update(kind="decision", answer="forget",
                         forget=mv.name(loser), forget_slot=slot + 1)
                held[slot] = move
                e["after"] = [mv.name(m) for m in held]
            events.append(e)
    return line, events


def emit_plan(rom, path, em, mv, names, dex_of, stats, stats_at, start,
              evolve_teaches):
    import json
    line, events = drill_plan(rom, em, mv, names, dex_of, stats, stats_at,
                              start, evolve_teaches)
    dec = [e for e in events if e["kind"] == "decision"]
    plan = {
        "schema": "gen1-learn-plan/1",
        "cartridge": os.path.basename(path),
        "species": names[start],
        "line": line,
        "evolutions_taken": True,
        "criterion": "tools/gen1-tables/learn_table.py, decide()",
        "starts_with": [mv.name(m) for m in
                        g.starting_moves(rom, stats_at, dex_of[start]) if m],
        "counts": {
            "learn_events": len(events),
            "free": sum(1 for e in events if e["kind"] == "free"),
            "already_held": sum(1 for e in events
                                if e["kind"] == "already-held"),
            "decisions": len(dec),
            "declines": sum(1 for e in dec if e["answer"] == "decline"),
            "replacements": sum(1 for e in dec if e["answer"] == "forget"),
        },
        "events": events,
    }
    print(json.dumps(plan, indent=2, sort_keys=False))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--decisions", action="store_true",
                    help="print every four-move decision, not just the counts")
    ap.add_argument("--evolutions", action="store_true",
                    help="every evolution, and what evolving on time costs")
    ap.add_argument("--species", help="one species, in detail")
    ap.add_argument("--no-evolve-teaches", action="store_true",
                    help="model an evolution as NOT offering the new form's "
                         "move for the level it evolved on")
    ap.add_argument("--drill-plan", metavar="SPECIES",
                    help="emit, as JSON on stdout, every level-up on that "
                         "species' line that offers a move, with the rule's "
                         "answer already applied. This is what "
                         "examples/gen1_learn_drill.rs consumes.")
    a = ap.parse_args()

    rom = g.read_rom(a.rom)
    names, _ = g.species_names(rom)
    dex_of, _ = g.pokedex_order(rom)
    stats, stats_at = g.base_stats(rom, dex_of)
    mv = g.Moves(rom)
    em = g.EvosMoves(rom)
    real = [i for i in names if 1 <= dex_of.get(i, 0) <= 151]
    evolve_teaches = not a.no_evolve_teaches

    def resolve(name):
        want = name.lower().replace(" ", "")
        i = next((k for k, v in names.items()
                  if v.lower().replace(" ", "") == want), None)
        if i is None:
            raise SystemExit(f"no species named {name!r} in this cartridge")
        return i

    if a.drill_plan:
        # Emitted alone and to stdout, so it can be redirected into a file
        # without the rest of the report landing in it.
        emit_plan(rom, a.rom, em, mv, names, dex_of, stats, stats_at,
                  resolve(a.drill_plan), evolve_teaches)
        return

    # -- 1. the closed table ------------------------------------------------
    #
    # **The table has 190 slots and they are indexed by INTERNAL index, not by
    # Pokedex number.** Reading the first 151 gives a mix of real species and
    # MissingNo and undercounts; `PokedexOrder` is what filters it to the 151.
    entries = [(i, lv, m) for i in real for lv, m in em.learnset[i]]
    dist = Counter(len(em.learnset[i]) for i in real)
    evk = Counter(e["kind"] for i in real for e in em.evolutions[i])
    starts = Counter(len(g.starting_moves(rom, stats_at, dex_of[i]))
                     for i in real)
    print(f"cartridge {a.rom}")
    print(f"  EvosMovesPointerTable @ 0x{em.addr:06X} (bank 0x{em.bank:02X}), "
          f"{g.NUM_INTERNAL} slots by internal index")
    print(f"  PokedexOrder          @ filters those to {len(real)} real species")
    print(f"  Moves                 @ 0x{mv.addr:06X}")
    print()
    print("THE CLOSED TABLE — every level-up learn event in the cartridge")
    print(f"  level-up learn entries, all told   {len(entries)}")
    print(f"  learns per species                 "
          f"{' '.join(f'{k}:{dist[k]}' for k in sorted(dist))}")
    print(f"  starting moves per species         "
          f"{' '.join(f'{k}:{starts[k]}' for k in sorted(starts))}")
    print(f"  distinct moves ever offered        "
          f"{len({m for _, _, m in entries})}")
    print(f"  evolution entries                  {sum(evk.values())}"
          f"   {evk['level']} by level, {evk['item']} by stone, "
          f"{evk['trade']} by trade")

    # -- 2. the four-move prompts -------------------------------------------
    #
    # **Every reachable starting state, not just level 1.** A Pokémon can be
    # caught at any level, and `moveset_at` says exactly what it then holds, so
    # the starting states are 151 species x 100 levels x {take a stone
    # evolution or not} — 30,200 trajectories, all enumerable. Every one is
    # walked to level 100 under the rule, and the decisions are deduplicated on
    # what the player actually sees: species, the four held, and the offer.
    tally, walks = Counter(), 0
    seen, prompts = set(), []
    bases = base_forms(em, real)
    for sp in real:
        for lv in range(1, 101):
            held0 = moveset_at(rom, em, stats_at, dex_of, sp, lv)
            branches = [None] + list(
                range(len([e for e in em.evolutions[sp] if e["kind"] != "level"])))
            for stone in branches:
                walks += 1
                got, c = line_prompts(rom, em, mv, stats, dex_of, stats_at,
                                      sp, evolve_teaches, from_level=lv,
                                      held=held0, stone=stone)
                if lv == 1 and stone is None and sp in bases:
                    tally.update(c)
                for p in got:
                    k = (p["species"], frozenset(p["held"]), p["offered"])
                    if k not in seen:
                        seen.add(k)
                        prompts.append(p)
    print()
    print("THE DECISIONS — 728 is the ceiling on learn EVENTS, not on")
    print("decisions: the first four slots fill for free, and a Pokémon you")
    print("catch arrives with a move set that is a function of its species and")
    print("its level and of nothing else. So the starting states are")
    print("enumerable, and every one of them is walked to level 100:")
    print(f"  trajectories walked                {walks}")
    print(f"  DISTINCT DECISIONS                 {len(prompts)}")
    print("    a decision is (species, the four held, the move offered) —")
    print("    exactly what a player, or a student, can see")
    print(f"  species that ever ask              "
          f"{len({p['species'] for p in prompts})} of {len(real)}")
    if prompts:
        first = min(prompts, key=lambda p: p["level"])
        print(f"  earliest one can ever be asked     "
              f"level {first['level']} ({names[first['species']]})")
    print()
    print(f"  on the {len(bases)} lines a player who raises a base form from")
    print("  level 1 and only presses A actually walks:")
    print(f"    slots filled free at level 1     {tally['start']}")
    print(f"    further learns filling for free  {tally['free']}")
    print(f"    learns of a move already held    {tally['already']}")
    print(f"    decisions                        {tally['prompt']}")

    # -- 3. what the rule answers -------------------------------------------
    flags = Counter()
    for p in prompts:
        for f in classify(mv, p):
            flags[f] += 1
    print()
    print("THE RULE'S ANSWERS")
    for k in ("refuse", "status-victim", "damage-victim", "coverage-loss",
              "close-damage", "status-tie", "unpriced-present", "no-answer"):
        print(f"  {k:<16} {flags[k]:>4}"
              f"   {100.0 * flags[k] / max(1, len(prompts)):5.1f}%")
    print()
    print("  refuse         the offered move is worth less than all four held")
    print("  status-victim  a move that deals no damage is dropped")
    print("  damage-victim  a damaging move is dropped for a better one")
    print("  coverage-loss  ...and it was the holder's last move of that type,")
    print("                 so the party or the fights ahead could change it")
    print("  close-damage   the two worst are damaging moves within "
          f"{int(CLOSE_MARGIN * 100)}% of each other")
    print("  status-tie     the two worst both deal no damage, so the rule")
    print("                 cannot separate them and falls through to PP")
    print("  unpriced-present  a HORN DRILL / SEISMIC TOSS class move is in")
    print("                 play; the rule protects it rather than pricing it")
    print("  no-answer      every held move is protected — the rule declines")

    # -- 3a. what a student would have to be able to SEE -------------------
    #
    # The prompt puts the four held move names and the offered move name on
    # screen (measured: `examples/gen1_learn_drill.rs`). It does **not** put
    # the species there — the box uses the nickname. So the question that
    # decides how hard the perception problem is: given the same four held
    # moves and the same offer, does the right answer ever depend on which
    # species is holding them?
    by = {}
    for p in prompts:
        by.setdefault((frozenset(p["held"]), p["offered"]), set()).add(p["loser"])
    amb = [k for k, v in by.items() if len(v) > 1]
    print()
    print("WHAT A STUDENT WOULD HAVE TO READ")
    print(f"  distinct (four held, offered) pairs  {len(by)}")
    print(f"  pairs whose answer needs the species {len(amb)}")
    for k in amb[:10]:
        who = {names[p["species"]]: mv.name(p["loser"]) for p in prompts
               if (frozenset(p["held"]), p["offered"]) == k}
        print(f"    holding {sorted(mv.name(m) for m in k[0])} "
              f"offered {mv.name(k[1])} -> {who}")

    if a.decisions:
        print()
        print("every decision")
        for p in sorted(prompts, key=lambda p: (p["level"], names[p["species"]])):
            fl = ",".join(sorted(classify(mv, p)))
            held = " ".join(mv.name(m) for m in p["held"])
            act = ("DECLINE TO ANSWER" if p["loser"] is None
                   else "REFUSE" if p["loser"] == p["offered"]
                   else f"forget {mv.name(p['loser'])}")
            print(f"  L{p['level']:<3} {names[p['species']]:<12} "
                  f"offered {mv.name(p['offered']):<14} "
                  f"holding [{held}] -> {act}   [{fl}]")

    # -- 4. evolutions -------------------------------------------------------
    if a.evolutions or a.species:
        print()
        print("EVOLUTIONS — and what evolving the moment it is offered costs")
        rows = []
        for i in real:
            for e in em.evolutions[i]:
                into = e["into"]
                cost = []
                delay = []
                if e["kind"] == "level":
                    # Evolving keeps every move already held, so the only thing
                    # at stake is what is still to come. A move the
                    # pre-evolution would learn after this level is LOST if the
                    # evolved form never learns it, and DELAYED if it learns it
                    # later.
                    have = {m: lv for lv, m in em.learnset[into]}
                    for lv, m in em.learnset[i]:
                        if lv <= e["level"]:
                            continue
                        if m not in have:
                            cost.append((lv, m))
                        elif have[m] > lv:
                            delay.append((lv, have[m], m))
                rows.append((i, e, into, cost, delay))
        by_kind = Counter(e["kind"] for _, e, _, _, _ in rows)
        print(f"  evolutions in the cartridge        {len(rows)}"
              f"   {dict(by_kind)}")
        lost = [r for r in rows if r[3]]
        delayed = [r for r in rows if r[4]]
        worst = max((d for r in rows for d in r[4]),
                    key=lambda d: d[1] - d[0], default=None)
        print(f"  level evolutions that LOSE a move  {len(lost)}")
        print(f"  level evolutions that DELAY one    {len(delayed)}"
              f" of {by_kind['level']}")
        if worst:
            print(f"  the longest delay any of them cost "
                  f"{worst[1] - worst[0]} levels "
                  f"({mv.name(worst[2])}, {worst[0]} -> {worst[1]})")
        for i, e, into, cost, delay in rows:
            if a.species and g.BY_NAME and names[i].lower() != a.species.lower():
                continue
            if not (a.evolutions or a.species):
                continue
            trig = (f"level {e['level']}" if e["kind"] == "level"
                    else e["kind"])
            note = ""
            if cost:
                note += ("  LOSES " +
                         ", ".join(f"{mv.name(m)}@{lv}" for lv, m in cost))
            if delay:
                note += ("  delays " +
                         ", ".join(f"{mv.name(m)} {a}->{b}"
                                   for a, b, m in delay))
            print(f"  {names[i]:<12} -> {names[into]:<12} {trig:<10}{note}")

    if a.species:
        want = a.species.lower().replace(" ", "")
        i = next((k for k, v in names.items()
                  if v.lower().replace(" ", "") == want), None)
        if i is None:
            raise SystemExit(f"no species named {a.species!r} in this cartridge")
        dex = dex_of[i]
        rate = g.growth_rate(rom, stats_at, dex)
        print()
        print(f"{names[i]}  internal 0x{i:02X}  dex {dex}  "
              f"{'/'.join(sorted({g.TYPES[t] for t in stats[dex]['types']}))}")
        print(f"  growth curve  {g.GROWTH_NAMES.get(rate, rate)}")
        print(f"  starts with   "
              f"{', '.join(mv.name(m) for m in g.starting_moves(rom, stats_at, dex))}")
        for lv, m in em.learnset[i]:
            r = mv.rows[m]
            print(f"  L{lv:<3} {mv.name(m):<14} {g.TYPES[r['type']]:<9} "
                  f"power {r['power']:>3}  acc {r['accuracy'] * 100 // 255:>3}%  "
                  f"pp {r['pp']}")


if __name__ == "__main__":
    main()

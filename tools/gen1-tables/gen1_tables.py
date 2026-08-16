#!/usr/bin/env python3
"""Read Gen 1's own progression tables **out of the cartridge**.

    python3 tools/gen1-tables/gen1_tables.py --rom <blue.gb> --report
    python3 tools/gen1-tables/gen1_tables.py --rom <rom> --effective-against rock
    python3 tools/gen1-tables/gen1_tables.py --rom <rom> --where 56       # Mankey
    python3 tools/gen1-tables/gen1_tables.py --rom <rom> --encounters 0x33

Why this exists
---------------
A repair loop that can say *"the party cannot beat this"* still has to say
*where to fix it*, and both halves of that answer are in the ROM:

* the **type effectiveness chart** says which types beat Rock,
* the **wild encounter tables** say which species appear on which map, at what
  levels and at what rate,
* the **trainer parties** say what the opposition actually is.

So *"where can I get something effective against Rock at a usable level"* is a
query over cartridge data rather than a wiki lookup, and nothing in the chain of
reasoning has to be injected by a person.

How the tables are located
--------------------------
**By signature, then read from the cartridge.** The *shape* of each table and
the first few of its bytes are facts from `pret/pokered` and are cited below;
the *contents* are read out of the ROM the user supplied. That is the same
technique `plugins/battle-control/src/wram.rs` uses for WRAM addresses, and it
means a hack or a translation is detected as a mismatch rather than silently
mis-read.

Nothing here is committed as data. No table is vendored; the tool needs a ROM.
"""
import argparse
import sys

# --- constants (facts, cited) ------------------------------------------------
# pokered constants/type_constants.asm
TYPES = {
    0x00: "NORMAL", 0x01: "FIGHTING", 0x02: "FLYING", 0x03: "POISON",
    0x04: "GROUND", 0x05: "ROCK", 0x06: "BIRD", 0x07: "BUG", 0x08: "GHOST",
    0x14: "FIRE", 0x15: "WATER", 0x16: "GRASS", 0x17: "ELECTRIC",
    0x18: "PSYCHIC", 0x19: "ICE", 0x1A: "DRAGON",
}
BY_NAME = {v: k for k, v in TYPES.items()}
# pokered constants/battle_constants.asm
SUPER_EFFECTIVE, NOT_VERY_EFFECTIVE, NO_EFFECT = 20, 5, 0

# The first six rows of `TypeEffects` (pokered data/types/type_matchups.asm).
# Distinctive enough to locate the table and specific enough that a cartridge
# which does not contain them is not the cartridge this tool understands.
TYPE_EFFECTS_HEAD = bytes([
    0x15, 0x14, 20,   # WATER    FIRE   super
    0x14, 0x16, 20,   # FIRE     GRASS  super
    0x14, 0x19, 20,   # FIRE     ICE    super
    0x16, 0x15, 20,   # GRASS    WATER  super
    0x17, 0x15, 20,   # ELECTRIC WATER  super
    0x15, 0x05, 20,   # WATER    ROCK   super
])

# `WildDataPointers` is a table of little-endian pointers into its own bank,
# one per map id, and each entry is
#     db grass_rate; [10 x (db level, db species)]; db water_rate; [10 x ...]
# with the ten slots present only when the rate is non-zero — the macros in
# pokered `macros/asserts.asm` assert exactly that (1 byte at rate 0, else 21).
#
# The table is located by anchoring on Viridian Forest's own grass block, which
# is `def_grass_wildmons 8` followed by its ten slots. **Red and Blue differ
# here**, which is the point of having both: Red's forest is Weedle and Kakuna,
# Blue's is Caterpie and Metapod, and both end on the two Pikachu slots.
# Species numbers are **internal indices**, not Pokédex numbers.
FOREST_GRASS = {
    "BLUE": bytes([8, 4, 0x7B, 5, 0x7C, 3, 0x7B, 5, 0x7B, 4, 0x7C,
                   6, 0x7C, 4, 0x71, 3, 0x70, 3, 0x54, 5, 0x54]),
    "RED": bytes([8, 4, 0x70, 5, 0x71, 3, 0x70, 5, 0x70, 4, 0x71,
                  6, 0x71, 4, 0x7C, 3, 0x7B, 3, 0x54, 5, 0x54]),
}

MAP_NAMES = {
    0x00: "Pallet Town", 0x01: "Viridian City", 0x02: "Pewter City",
    0x0C: "Route 1", 0x0D: "Route 2", 0x0E: "Route 3", 0x0F: "Route 4",
    0x21: "Route 22", 0x33: "Viridian Forest", 0x3B: "Mt Moon 1F",
}


def read_rom(path):
    with open(path, "rb") as f:
        return f.read()


def find_all(hay, needle):
    out, i = [], hay.find(needle)
    while i >= 0:
        out.append(i)
        i = hay.find(needle, i + 1)
    return out


# --- type chart --------------------------------------------------------------

class TypeChart:
    def __init__(self, rom):
        hits = find_all(rom, TYPE_EFFECTS_HEAD)
        if len(hits) != 1:
            raise SystemExit(
                f"TypeEffects: expected exactly one match for its opening rows, "
                f"found {len(hits)} — this is not a cartridge this tool reads"
            )
        self.addr = hits[0]
        self.rows = []
        i = self.addr
        while rom[i] != 0xFF:
            self.rows.append((rom[i], rom[i + 1], rom[i + 2]))
            i += 3
            if len(self.rows) > 200:
                raise SystemExit("TypeEffects: no terminator inside 200 rows")

    def multiplier(self, atk, dfn):
        """Percent, 100 = neutral. Gen 1 applies each matching row in turn."""
        m = 100
        for a, d, x in self.rows:
            if a == atk and d == dfn:
                m = m * x // 10
        return m

    def super_effective_against(self, defender):
        return sorted(
            {TYPES[a] for a, d, x in self.rows
             if d == defender and x == SUPER_EFFECTIVE}
        )


# --- wild encounters ---------------------------------------------------------

SLOT_COUNT = 10


class Wild:
    """Every map's grass and water slots, read from `WildDataPointers`."""

    def __init__(self, rom, species_names):
        self.names = species_names
        # Anchor on Viridian Forest's own grass block, then walk *back* to the
        # pointer table by finding the little-endian pointer that names it.
        anchor, self.version = None, None
        for name, sig in FOREST_GRASS.items():
            hits = find_all(rom, sig)
            if len(hits) == 1:
                anchor, self.version = hits[0], name
                break
        if anchor is None:
            raise SystemExit(
                "could not find Viridian Forest's grass block — this tool reads "
                "retail Red and Blue"
            )
        self.bank = anchor // 0x4000
        ptr = 0x4000 + (anchor % 0x4000)
        needle = bytes([ptr & 0xFF, ptr >> 8])
        table = None
        base = self.bank * 0x4000
        for off in find_all(rom[base:base + 0x4000], needle):
            # A pointer table is a long run of plausible in-bank pointers.
            cand = base + off
            start = cand - 2 * 0x33  # Viridian Forest is map id 0x33
            if start < base:
                continue
            ok = all(
                0x4000 <= int.from_bytes(rom[start + 2 * i:start + 2 * i + 2],
                                         "little") < 0x8000
                for i in range(0x40)
            )
            if ok:
                table = start
                break
        if table is None:
            raise SystemExit("could not locate WildDataPointers")
        self.table = table
        self.maps = {}
        for m in range(0xF8):
            p = int.from_bytes(rom[table + 2 * m:table + 2 * m + 2], "little")
            at = base + (p - 0x4000)
            if not (base <= at < base + 0x4000):
                continue
            self.maps[m] = self._entry(rom, at)

    def _entry(self, rom, at):
        i = at
        grass_rate = rom[i]
        i += 1
        grass = []
        if grass_rate:
            for _ in range(SLOT_COUNT):
                grass.append((rom[i], rom[i + 1]))
                i += 2
        water_rate = rom[i]
        i += 1
        water = []
        if water_rate:
            for _ in range(SLOT_COUNT):
                water.append((rom[i], rom[i + 1]))
                i += 2
        return {"grass_rate": grass_rate, "grass": grass,
                "water_rate": water_rate, "water": water}

    def where(self, internal_species):
        out = []
        for m, e in self.maps.items():
            for kind in ("grass", "water"):
                lv = [l for l, sp in e[kind] if sp == internal_species]
                if lv:
                    out.append((m, kind, min(lv), max(lv),
                                e[f"{kind}_rate"], len(lv)))
        return out


# --- species names, from the cartridge --------------------------------------

CHARMAP = {**{i: chr(ord('A') + i - 0x80) for i in range(0x80, 0x9A)},
           **{i: chr(ord('a') + i - 0xA0) for i in range(0xA0, 0xBA)},
           0x50: '', 0x7F: ' ', 0xE1: 'P', 0xE2: 'M', 0xE3: '-',
           0xE6: '?', 0xE7: '!', 0xEF: '.'}


def species_names(rom):
    """`MonsterNames`: 190 records of 10 padded characters, indexed by the
    **internal** index. Located by the fact that internal index 1 is RHYDON."""
    want = b"".join(bytes([0x80 + ord(c) - ord('A')]) for c in "RHYDON")
    for off in find_all(rom, want):
        # `MonsterNames` is `table_width NAME_LENGTH - 1`, i.e. 10 padded
        # characters per record, and it is indexed by **internal** index
        # starting at 1 — so RHYDON is the first record, not the second.
        start = off
        names = {}
        ok = True
        for i in range(190):
            raw = rom[start + 10 * i: start + 10 * i + 10]
            s = "".join(CHARMAP.get(b, "") for b in raw).strip()
            if not s:
                ok = False
                break
            names[i + 1] = s
        if ok and names.get(1) == "RHYDON" and names.get(0x99) == "BULBASAUR":
            return names, start
    raise SystemExit("could not locate MonsterNames")


# --- base stats (for types) --------------------------------------------------

def base_stats(rom, dex_of_internal):
    """`BaseStats`: 28-byte records for dex numbers 1..150, plus Mew apart.
    Located by Bulbasaur's own record, whose first bytes are its dex number
    and its five base stats (45 49 49 45 65)."""
    head = bytes([0x01, 45, 49, 49, 45, 65])
    hits = [o for o in find_all(rom, head) if rom[o + 6] in TYPES and rom[o + 7] in TYPES]
    if not hits:
        raise SystemExit("could not locate BaseStats")
    at = hits[0]
    out = {}
    for dex in range(1, 151):
        r = at + 28 * (dex - 1)
        out[dex] = {
            "hp": rom[r + 1], "atk": rom[r + 2], "def": rom[r + 3],
            "spd": rom[r + 4], "spc": rom[r + 5],
            "types": (rom[r + 6], rom[r + 7]),
            "catch_rate": rom[r + 8],
        }
    return out, at


def pokedex_order(rom):
    """`PokedexOrder`: 190 bytes, internal index -> dex number. Its opening
    eight entries are 112,115,32,35,21,100,34,80 and occur exactly once."""
    head = bytes([112, 115, 32, 35, 21, 100, 34, 80])
    hits = find_all(rom, head)
    if len(hits) != 1:
        raise SystemExit(f"PokedexOrder: {len(hits)} matches, expected 1")
    at = hits[0]
    return {i + 1: rom[at + i] for i in range(190)}, at


# --- the query the repair loop actually asks ---------------------------------

def candidates_against(rom, defender_type, max_level, chart, wild, names,
                       dex_of, stats):
    """Species that are super-effective against `defender_type` **by their own
    type**, and where they can be caught at or below `max_level`.

    This is the whole of the derivation: the type chart says which types beat
    the defender, the base stats say which species are those types, and the
    encounter tables say where they are. Nothing is named by a person.
    """
    good = {BY_NAME[t] for t in chart.super_effective_against(defender_type)}
    found = []
    for internal, name in names.items():
        dex = dex_of.get(internal, 0)
        if not (1 <= dex <= 150):
            continue
        t = set(stats[dex]["types"])
        if not (t & good):
            continue
        for m, kind, lo, hi, rate, slots in wild.where(internal):
            if lo <= max_level:
                found.append({
                    "species": name, "internal": internal, "dex": dex,
                    "types": sorted(TYPES[x] for x in t),
                    "map": m, "map_name": MAP_NAMES.get(m, f"map 0x{m:02X}"),
                    "kind": kind, "levels": (lo, hi),
                    "rate": rate, "slots": slots,
                })
    found.sort(key=lambda c: (-c["slots"], c["levels"][0]))
    return found


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--effective-against")
    ap.add_argument("--where", type=lambda s: int(s, 0))
    ap.add_argument("--encounters", type=lambda s: int(s, 0))
    ap.add_argument("--catch-for", help="TYPE to beat, e.g. rock")
    ap.add_argument("--max-level", type=int, default=12)
    a = ap.parse_args()

    rom = read_rom(a.rom)
    names, names_at = species_names(rom)
    dex_of, order_at = pokedex_order(rom)
    stats, stats_at = base_stats(rom, dex_of)
    chart = TypeChart(rom)
    wild = Wild(rom, names)

    if a.report:
        print(f"cartridge {a.rom}")
        print(f"  MonsterNames      @ 0x{names_at:06X}  {len(names)} names")
        print(f"  PokedexOrder      @ 0x{order_at:06X}")
        print(f"  BaseStats         @ 0x{stats_at:06X}")
        print(f"  TypeEffects       @ 0x{chart.addr:06X}  {len(chart.rows)} rows")
        print(f"  WildDataPointers  @ 0x{wild.table:06X}  bank {wild.bank} "
              f"({wild.version}), "
              f"{sum(1 for e in wild.maps.values() if e['grass_rate'])} maps with grass")

    if a.effective_against:
        d = BY_NAME[a.effective_against.upper()]
        print(f"super effective against {TYPES[d]}: "
              f"{', '.join(chart.super_effective_against(d))}")

    if a.where is not None:
        sp = a.where
        print(f"{names.get(sp, '?')} (internal {sp}):")
        for m, kind, lo, hi, rate, slots in wild.where(sp):
            print(f"  {MAP_NAMES.get(m, f'map 0x{m:02X}'):20s} {kind:5s} "
                  f"lv {lo}-{hi}  {slots}/10 slots, encounter rate {rate}")

    if a.encounters is not None:
        e = wild.maps.get(a.encounters)
        m = MAP_NAMES.get(a.encounters, f"map 0x{a.encounters:02X}")
        print(f"{m} grass rate {e['grass_rate']}:")
        seen = {}
        for lv, sp in e["grass"]:
            seen.setdefault(sp, []).append(lv)
        for sp, lv in sorted(seen.items(), key=lambda kv: -len(kv[1])):
            print(f"  {names.get(sp,'?'):12s} lv {min(lv)}-{max(lv)}  "
                  f"{len(lv)}/10 slots")

    if a.catch_for:
        d = BY_NAME[a.catch_for.upper()]
        cands = candidates_against(rom, d, a.max_level, chart, wild, names,
                                   dex_of, stats)
        print(f"to beat {TYPES[d]}, catchable at level <= {a.max_level}:")
        for c in cands[:20]:
            print(f"  {c['species']:12s} {'/'.join(c['types']):16s} "
                  f"{c['map_name']:18s} {c['kind']:5s} lv {c['levels'][0]}-"
                  f"{c['levels'][1]}  {c['slots']}/10 slots")
        if not cands:
            print("  nothing in the encounter tables satisfies that")
    return 0


if __name__ == "__main__":
    sys.exit(main())

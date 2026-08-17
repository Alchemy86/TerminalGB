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


# --- moves, and what a species knows the moment you catch it -----------------

# `Moves` (pokered data/moves/moves.asm): six bytes per move — animation,
# effect, power, type, accuracy, PP — indexed from 1. Located by its first three
# records, which are POUND / KARATE CHOP / DOUBLESLAP and are distinctive
# enough that a cartridge without them is not one this tool reads.
MOVES_HEAD = bytes([
    1, 0, 40, 0, 255, 35,     # POUND        40 NORMAL 100% 35pp
    2, 0, 50, 0, 255, 25,     # KARATE CHOP  50 NORMAL 100% 25pp
    3, 29, 15, 0, 216, 10,    # DOUBLESLAP   15 NORMAL  85% 10pp
])
MOVE_RECORD = 6

# `MoveNames`: `$50`-terminated names, indexed from 1, opening with POUND.
MOVE_NAMES_HEAD = b"".join(
    bytes([0x80 + ord(c) - ord('A')]) if c != ' ' else bytes([0x7F])
    for c in "POUND"
) + bytes([0x50])


class Moves:
    """Every move's power, type and accuracy, read out of the cartridge."""

    def __init__(self, rom):
        hits = find_all(rom, MOVES_HEAD)
        if len(hits) != 1:
            raise SystemExit(
                f"Moves: {len(hits)} matches for its opening records, expected 1"
            )
        self.addr = hits[0]
        self.rows = {}
        for i in range(1, 166):  # NUM_ATTACKS
            r = self.addr + MOVE_RECORD * (i - 1)
            self.rows[i] = {
                "effect": rom[r + 1], "power": rom[r + 2],
                "type": rom[r + 3], "accuracy": rom[r + 4], "pp": rom[r + 5],
            }
        # Names, so a plan is readable. The table is elsewhere in the ROM and is
        # anchored on POUND; a run of names that does not decode is not it.
        self.names = {}
        self.names_at = None
        for off in find_all(rom, MOVE_NAMES_HEAD):
            names, i, ok = {}, off, True
            for k in range(1, 166):
                s, n = "", 0
                while rom[i] != 0x50 and n < 16:
                    s += CHARMAP.get(rom[i], "")
                    i += 1
                    n += 1
                if n >= 16 or not s:
                    ok = False
                    break
                i += 1
                names[k] = s
            if ok and names.get(1) == "POUND":
                self.names, self.names_at = names, off
                break

    def name(self, i):
        return self.names.get(i, f"move #{i}")


# `BaseStats` record offsets. The four moves a species knows the moment it is
# generated are **in the base-stats record itself** (pokered
# `data/pokemon/base_stats/*.asm`: `db MOVE1, MOVE2, MOVE3, MOVE4`), which is
# what makes "what will it actually be able to do when I catch it?" a cartridge
# read rather than a learnset walk.
BASE_MOVES_OFF = 15


def starting_moves(rom, stats_at, dex):
    r = stats_at + 28 * (dex - 1) + BASE_MOVES_OFF
    return [m for m in rom[r:r + 4] if m]


# --- trainers ----------------------------------------------------------------

# `TrainerNames`: `$50`-terminated class names, indexed from 1, opening
# YOUNGSTER / BUG CATCHER / LASS / SAILOR. **Two runs of those first three
# exist in this cartridge** — one of them a shorter list — so the signature
# carries SAILOR as well, which only the real table has.
TRAINER_NAMES_HEAD = b"".join(
    b"".join(bytes([0x80 + ord(c) - ord('A')]) if c != ' ' else bytes([0x7F])
             for c in word) + bytes([0x50])
    for word in ("YOUNGSTER", "BUG CATCHER", "LASS", "SAILOR")
)


class Trainers:
    """Every trainer class's rosters, read out of `TrainerDataPointers`.

    A roster is `db level, species..., 0` when every member shares a level, or
    `db $FF, level, species, ..., 0` when they do not (`data/trainers/parties.asm`).
    **The list is bounded by the NEXT class's pointer**: asking for roster 2 of
    a class that has one walks into the next class's data and answers
    confidently with somebody else's team, which is exactly what happened here
    the first time (BROCK #2 came back as MISTY #1).
    """

    def __init__(self, rom, species):
        hits = find_all(rom, TRAINER_NAMES_HEAD)
        if not hits:
            raise SystemExit("could not locate TrainerNames")
        self.names_at = hits[-1]
        self.names, i = {}, self.names_at
        for k in range(1, 48):
            s = ""
            while rom[i] != 0x50:
                s += CHARMAP.get(rom[i], "")
                i += 1
            i += 1
            self.names[k] = s
        self.by_name = {v.replace(".", "").replace(" ", "").upper(): k
                        for k, v in self.names.items()}
        # **Anchor on real party data, then walk back to the pointer that names
        # it.** The same move `Wild` makes, and it is not optional: a scan for
        # "a long run of in-bank little-endian words" finds a *different* table
        # in this cartridge and answers with LT.SURGE fielding a level-1
        # RHYHORN. Two facts pin it — `BugCatcherData`'s first three rosters are
        # the Viridian Forest teams (level 6 WEEDLE/CATERPIE, level 7
        # WEEDLE/KAKUNA/WEEDLE, level 9 WEEDLE), and BUG_CATCHER is class 2, so
        # the pointer that names them is the table's second entry.
        weedle = next(k for k, v in species.items() if v == "WEEDLE")
        kakuna = next(k for k, v in species.items() if v == "KAKUNA")
        caterpie = next(k for k, v in species.items() if v == "CATERPIE")
        sig = bytes([6, weedle, caterpie, 0,
                     7, weedle, kakuna, weedle, 0,
                     9, weedle, 0])
        anchors = find_all(rom, sig)
        if len(anchors) != 1:
            raise SystemExit(
                f"BugCatcherData: {len(anchors)} matches for the forest's own "
                f"three rosters, expected 1"
            )
        anchor = anchors[0]
        self.bank = anchor // 0x4000
        base = self.bank * 0x4000
        want = 0x4000 + (anchor % 0x4000)
        needle = bytes([want & 0xFF, want >> 8])
        self.table = None
        for off in find_all(rom[base:base + 0x4000], needle):
            cand = base + off - 2  # BUG_CATCHER is class 2, so entry index 1
            if cand < base:
                continue
            ptrs = [int.from_bytes(rom[cand + 2 * i:cand + 2 * i + 2], "little")
                    for i in range(47)]
            if all(0x4000 <= p < 0x8000 for p in ptrs) and \
                    all(ptrs[i + 1] >= ptrs[i] for i in range(46)):
                self.table = cand
                break
        if self.table is None:
            raise SystemExit("could not locate TrainerDataPointers")
        self.ptrs = [int.from_bytes(rom[self.table + 2 * i:self.table + 2 * i + 2],
                                    "little") for i in range(47)]
        self.rom = rom

    def _roster(self, rom, at, limit):
        out = []
        if at >= limit:
            return out
        if rom[at] == 0xFF:
            at += 1
            while at < limit and rom[at] != 0:
                out.append((rom[at], rom[at + 1]))
                at += 2
        else:
            lv = rom[at]
            at += 1
            while at < limit and rom[at] != 0:
                out.append((lv, rom[at]))
                at += 1
        return out

    def rosters(self, cls):
        """Every roster the class has, bounded by the next class's pointer."""
        base = self.bank * 0x4000
        at = base + (self.ptrs[cls - 1] - 0x4000)
        end = (base + (self.ptrs[cls] - 0x4000)) if cls < 47 else base + 0x4000
        out = []
        while at < end and self.rom[at] != 0:
            team = self._roster(self.rom, at, end)
            if not team:
                break
            out.append(team)
            # Step over this roster: one byte per member, plus the level byte
            # or the $FF and a level per member, plus the terminator.
            at += (1 + 2 * len(team) + 1) if self.rom[at] == 0xFF \
                else (1 + len(team) + 1)
        return out


# --- marts, and the price of a ball -------------------------------------------

# `MartInventories` (pokered `data/items/marts.asm`): each mart is
# `db $FE, count, items..., $FF`. The table's own position is found by that
# framing rather than by an address, and the first list is Viridian's.
MART_FRAME = 0xFE
MART_END = 0xFF

# `ItemPrices`: three BCD bytes per item, indexed from 1. Located by the four
# it opens with — MASTER_BALL free, ULTRA 1200, GREAT 600, POKE 200 — which
# occur exactly once.
ITEM_PRICES_HEAD = bytes([0x00, 0x00, 0x00, 0x00, 0x12, 0x00,
                          0x00, 0x06, 0x00, 0x00, 0x02, 0x00])
POKE_BALL_ITEM = 4


def item_prices(rom):
    hits = find_all(rom, ITEM_PRICES_HEAD)
    if len(hits) != 1:
        raise SystemExit(f"ItemPrices: {len(hits)} matches, expected 1")
    at = hits[0]

    def bcd(b):
        return sum(((x >> 4) * 10 + (x & 15)) * 100 ** (2 - k)
                   for k, x in enumerate(b))
    return {i: bcd(rom[at + 3 * (i - 1):at + 3 * i]) for i in range(1, 100)}, at


def marts(rom):
    """Every mart's stock list, in the order the cartridge lists them."""
    # Anchor on Viridian's own list, which is the first and is
    # `POKE_BALL ANTIDOTE PARLYZ_HEAL BURN_HEAL`.
    sig = bytes([MART_FRAME, 4, 4, 11, 15, 12, MART_END])
    hits = find_all(rom, sig)
    if len(hits) != 1:
        raise SystemExit(f"marts: {len(hits)} matches for the first list, expected 1")
    at, out = hits[0], []
    while at < len(rom) and rom[at] == MART_FRAME:
        n = rom[at + 1]
        out.append(list(rom[at + 2:at + 2 + n]))
        at += 2 + n + 1
    return out, hits[0]


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


# --- the catching plan --------------------------------------------------------

def plan(rom, opponents, max_level, chart, wild, names, dex_of, stats, moves,
         trainers, only_maps=None):
    """What is worth catching for the fights ahead, and where it is catchable.

    Every step of the derivation is a cartridge read: the opponents' parties
    come from `TrainerDataPointers`, their types from `BaseStats`, what beats
    those types from `TypeEffects`, the candidate species' **starting moves**
    from their own base-stats records, and where they live from the encounter
    tables. Nothing is named by a person.

    The move check is what makes this stronger than "its type is effective":
    a species catchable at level 4 is only useful if the moves it *arrives*
    with can express the advantage, and those four moves are in the record.
    """
    # 1. What is on the other side, and what beats it.
    threats = []
    for label, cls, roster in opponents:
        for lv, sp in trainers.rosters(cls)[roster - 1]:
            dex = dex_of.get(sp, 0)
            if not (1 <= dex <= 150):
                continue
            t = [x for x in stats[dex]["types"]]
            threats.append({
                "who": label, "species": names.get(sp, "?"), "level": lv,
                "types": sorted({TYPES[x] for x in t}),
                "beaten_by": sorted({
                    a for x in set(t)
                    for a in {r[0] for r in chart.rows
                              if r[1] == x and r[2] == SUPER_EFFECTIVE}
                }),
            })
    wanted_types = sorted({a for th in threats for a in th["beaten_by"]})

    # 2. Which catchable species arrive able to use one of those types.
    out = []
    for internal, name in names.items():
        dex = dex_of.get(internal, 0)
        if not (1 <= dex <= 150):
            continue
        starts = starting_moves(rom, stats["_at"], dex)
        useful = [m for m in starts
                  if moves.rows[m]["power"] > 0
                  and moves.rows[m]["type"] in wanted_types]
        own = set(stats[dex]["types"]) & set(wanted_types)
        if not useful and not own:
            continue
        for m, kind, lo, hi, rate, slots in wild.where(internal):
            if lo > max_level:
                continue
            if only_maps is not None and m not in only_maps:
                continue
            out.append({
                "species": name, "internal": internal, "dex": dex,
                "catch_rate": stats[dex]["catch_rate"],
                "types": sorted(TYPES[x] for x in set(stats[dex]["types"])),
                "map": m, "map_name": MAP_NAMES.get(m, f"map 0x{m:02X}"),
                "kind": kind, "levels": (lo, hi), "rate": rate, "slots": slots,
                "arrives_with": [(moves.name(x), TYPES[moves.rows[x]["type"]],
                                  moves.rows[x]["power"]) for x in useful],
                # **Compare type ids, not their names.** `beaten_by` holds ids,
                # and a first version compared `TYPES[...]` strings against
                # them, so every row came back "(by type only)" — a silently
                # empty answer that looked like a real one.
                "answers": sorted({
                    th["who"] + "/" + th["species"] for th in threats
                    if ({moves.rows[x]["type"] for x in useful} | own)
                    & set(th["beaten_by"])
                }),
            })
    # One row per species: the same Pokémon appears on many maps and a reader
    # wants the best place to get it, not the list. Best = arrives able to act,
    # then the most slots, then the lowest level.
    best = {}
    for c in out:
        k = c["internal"]
        rank = (-len(c["arrives_with"]), -c["slots"], c["levels"][0])
        if k not in best or rank < best[k][0]:
            best[k] = (rank, c)
    out = [c for _, c in sorted(best.values(), key=lambda p: p[0])]
    return threats, wanted_types, out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--effective-against")
    ap.add_argument("--where", type=lambda s: int(s, 0))
    ap.add_argument("--encounters", type=lambda s: int(s, 0))
    ap.add_argument("--catch-for", help="TYPE to beat, e.g. rock")
    ap.add_argument("--max-level", type=int, default=12)
    ap.add_argument("--trainer", help="a trainer class, e.g. brock")
    ap.add_argument("--gyms", action="store_true",
                    help="every gym leader's own party, from the cartridge")
    ap.add_argument("--marts", action="store_true",
                    help="every mart's stock and the price of a ball")
    ap.add_argument("--plan", help="comma-separated trainer classes to plan for")
    ap.add_argument("--maps", help="comma-separated map ids to restrict to")
    a = ap.parse_args()

    rom = read_rom(a.rom)
    names, names_at = species_names(rom)
    dex_of, order_at = pokedex_order(rom)
    stats, stats_at = base_stats(rom, dex_of)
    stats["_at"] = stats_at
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

    if a.marts:
        lists, at = marts(rom)
        prices, prices_at = item_prices(rom)
        print(f"marts @ 0x{at:06X}, ItemPrices @ 0x{prices_at:06X}")
        print(f"  a POKE BALL costs {prices[POKE_BALL_ITEM]}")
        for i, items in enumerate(lists):
            mark = " <- sells POKE BALLs" if POKE_BALL_ITEM in items else ""
            print(f"  mart {i}: items {items}{mark}")

    if a.gyms or a.trainer or a.plan:
        trainers = Trainers(rom, names)
        moves = Moves(rom)

    if a.gyms or a.trainer:
        print(f"TrainerNames @ 0x{trainers.names_at:06X}, "
              f"TrainerDataPointers @ 0x{trainers.table:06X} (bank {trainers.bank})")
        which = ([a.trainer.upper().replace(" ", "").replace(".", "")]
                 if a.trainer else
                 ["BROCK", "MISTY", "LTSURGE", "ERIKA", "KOGA", "SABRINA",
                  "BLAINE", "GIOVANNI"])
        for w in which:
            cls = trainers.by_name.get(w)
            if cls is None:
                print(f"  {w}: no such trainer class in this cartridge")
                continue
            for r, team in enumerate(trainers.rosters(cls), 1):
                shown = ", ".join(
                    f"{names.get(sp,'?')} Lv{lv} "
                    f"({'/'.join(sorted({TYPES[t] for t in stats[dex_of[sp]]['types']}))})"
                    for lv, sp in team)
                print(f"  {trainers.names[cls]} #{r}: {shown}")

    if a.plan:
        opponents = []
        for w in a.plan.split(","):
            key = w.strip().upper().replace(" ", "").replace(".", "")
            cls = trainers.by_name.get(key)
            if cls is None:
                raise SystemExit(f"no trainer class named {w!r} in this cartridge")
            opponents.append((trainers.names[cls], cls, 1))
        only = None
        if a.maps:
            only = {int(x, 0) for x in a.maps.split(",")}
        threats, wanted, cands = plan(
            rom, opponents, a.max_level, chart, wild, names, dex_of, stats,
            moves, trainers, only)
        print("what is ahead:")
        for th in threats:
            print(f"  {th['who']:<10} {th['species']:<12} Lv{th['level']:<3} "
                  f"{'/'.join(th['types']):<16} beaten by "
                  f"{', '.join(TYPES[t] for t in th['beaten_by']) or '(nothing)'}")
        print(f"so the useful attacking types are: "
              f"{', '.join(TYPES[t] for t in wanted)}")
        # **Which starter the run took changes what it still needs**, and the
        # Pewter chain varies that by seed, so the answer is per starter rather
        # than one list. Read each starter's own type and starting moves out of
        # the same records the candidates use.
        print("what the starter already answers:")
        by_name = {v: k for k, v in names.items()}
        for s in ("BULBASAUR", "CHARMANDER", "SQUIRTLE"):
            internal = by_name.get(s)
            if internal is None:
                continue
            dex = dex_of[internal]
            own = set(stats[dex]["types"])
            mvs = starting_moves(rom, stats_at, dex)
            has_now = {moves.rows[m]["type"] for m in mvs
                       if moves.rows[m]["power"] > 0}
            def hits(kinds):
                return sorted({th["who"] + "/" + th["species"] for th in threats
                               if kinds & set(th["beaten_by"])})
            now, later = hits(has_now), hits(own)
            # **Two different claims, and conflating them overstates.** A
            # Bulbasaur is GRASS and so *will* beat Brock — with VINE WHIP,
            # which it does not have at level 5. What it arrives with is
            # TACKLE, a NORMAL move that beats nothing on the list.
            print(f"  {s:<11} {'/'.join(sorted(TYPES[t] for t in own)):<13} "
                  f"arrives with "
                  f"{', '.join(moves.name(m) for m in mvs) or '(nothing)'}")
            print(f"      with the moves it has: "
                  f"{', '.join(now) or 'NOTHING on this list'}")
            print(f"      by its own type, once it learns one: "
                  f"{', '.join(later) or 'NOTHING on this list'}")
        print(f"catchable at level <= {a.max_level}"
              f"{' on maps ' + a.maps if a.maps else ''}:")
        if not cands:
            print("  NOTHING — the encounter tables offer no answer under those "
                  "constraints, which is a finding rather than an empty result")
        for c in cands[:24]:
            arrives = ", ".join(f"{m} ({t} {p})" for m, t, p in c["arrives_with"])
            print(f"  {c['species']:<12} {'/'.join(c['types']):<14} "
                  f"{c['map_name']:<18} {c['kind']:<5} lv {c['levels'][0]}-"
                  f"{c['levels'][1]:<3} {c['slots']}/10 slots  "
                  f"catch rate {c['catch_rate']:<3}")
            print(f"      internal #{c['internal']}, arrives with: "
                  f"{arrives or '(no effective move at level 1)'}")
            print(f"      answers: {', '.join(c['answers']) or '(by type only)'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())

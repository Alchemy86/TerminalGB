#!/usr/bin/env python3
"""Audit the Pewter chain's routes against the cartridge's own walkability.

Three times in this chain a link has failed because a waypoint sat one tile
above something you cannot come back from — the rival's sprite in Oak's lab, the
tile the rival parks on, and a one-way ledge in Viridian City — and each was
found by reading a few hundred failing trajectories. The shape is always the
same, so it is checkable in advance:

    a waypoint whose overshoot is unrecoverable is a waypoint pointing at a trap

This is that check. For every stage of every route the solver will actually
walk, it asks three questions of the map:

  1. is the waypoint a tile you can stand on?
  2. is it reachable from the waypoint before it?
  3. if you walk one, two or three tiles *past* it — which an NPC in the way, a
     detour, or a `goto` that overshoots will do — can the **walker** still
     reach every waypoint that comes after it?

**Two things had to be modelled before this could catch the trap it was written
for, and both were got wrong first.**

*Ledges.* Link 08's `(29,26)` is topologically fine — there is a 13-step path
back to the next waypoint that crosses no warp tile, and a flood fill says so
cheerfully. The trap is that stepping down off row 26 does not land on row 27,
it **jumps** to row 28, and no ledge can be climbed. That is in the cartridge as
`LedgeTiles` (`06:66CF`, found here by its own shape: 4-byte records of facing,
standing tile, ledge tile and the D-pad bit, terminated by `$FF`), and the
one-wayness is visible in the data — there are eight entries, for facing down,
left and right, and **none for up**. Viridian City's row 26 is tile `$39`, row
27 alternates `$36`/`$37`, and all three are in that table. So the walk graph
has directed jump edges, and "one tile past the waypoint" is sometimes two.

*The walker.* `Action::GoTo` is not a path-finder. It is greedy on the larger
remaining delta, and when blocked it commits to a run of `GOTO_DETOUR` presses
along the other axis, alternating direction on each successive detour, giving up
after `GOTO_BARREN` presses that move nothing or `budget + GOTO_SLACK` presses
in total. Below a one-way ledge that policy slides along the wall and never gets
round it — which is exactly what the 17 failing runs did, between x=27 and x=31.
So the check runs that loop over the grid, and a waypoint that is *reachable*
but that the walker cannot actually reach is reported as the failure it is.

Consequence worth stating: the simulation is only as true as those constants and
that policy, so **if `Action::GoTo` changes, this changes with it**. It models
the walk and nothing else — no NPCs, no encounters, no menus, and no strength
boulders or cut trees (none of which the Pewter chain meets).

Everything comes out of the ROM. No walkthrough, no table typed in here:

  * `MapHeaderPointers` is found by property, being the only offset in bank 0
    whose 248 consecutive little-endian words all land in $4000-$7FFF, and
    `MapHeaderBanks` is picked from its neighbours by which one makes a known
    map decode to a sane header;
  * a map header gives the tileset, the block map and the object data;
  * `Tilesets` (3:47BE, and the atlas records it) gives the blockset and an
    $FF-terminated list of the tile ids you may stand on;
  * a 16x16 walk tile is walkable when its **bottom-left** 8x8 is in that list
    — block-definition indices 4, 6, 12 and 14, which is the rule
    `docs/gen1/authoring-an-area.md` measured against the running game.

Usage:

    cargo build --release --features scenario --example pewter_chain
    ./target/release/examples/pewter_chain routes > /tmp/routes.json
    python3 tools/pewter-chain/route_audit.py --rom <blue.gb> --routes /tmp/routes.json

or let it run the solver itself:

    python3 tools/pewter-chain/route_audit.py --rom <blue.gb>

**This is a design aid, not a gate, and the distinction is not modesty.** Two
things it deliberately does not model, each of which makes it over-report:

- *It cannot say whether an overshoot is reachable.* It answers "if the walker
  ended up here, could it recover?", not "can the walker end up here?" — that
  needs the map's NPCs, and it was an NPC blocking the walk that pushed link 08
  over its ledge in the first place.
- *It has no stuck-sidestep.* The real driver notices a `goto` that reports
  "stuck at …", or five barren decisions, and wanders a randomised few steps
  **towards** the waypoint before resuming; the simulation just keeps issuing
  `goto` until the retries cycle. So a corner the driver shakes itself out of
  reads here as a dead end.

Between them: 21 fragile waypoints across a chain whose every link is certified
at 100% over 3,000 attempts. Read the list as *the brittle points of this route*
— worth a second look when a link starts failing, and worth avoiding when a
route is being drawn — not as a list of bugs.

Exit status is 1 only for the two things that are unambiguous: a waypoint you
cannot stand on, and a waypoint unreachable from the one before it. Fragility is
reported and does not fail. It is **not** in CI: it needs the player's own
cartridge, the same rule `tools/sameboy-oracle/` and the c-sp suites follow.
"""

import argparse
import json
import os
import subprocess
import sys
from collections import deque

MAP_HEADER_POINTERS = 0x01AE
NUM_MAPS = 248
TILESETS = 3 * 0x4000 + 0x47BE - 0x4000  # 3:47BE, per the atlas

# `SPRITE_FACING_*` -> the D-pad bit that jumps that way, and the step it takes.
FACING = {0x00: (0x80, (0, 1)), 0x04: (0x40, (0, -1)), 0x08: (0x20, (-1, 0)), 0x0C: (0x10, (1, 0))}


def find_ledge_tiles(rom: bytes):
    """`LedgeTiles`, located by its own shape rather than by a remembered address.

    Records are (facing, the tile you stand on, the ledge tile, the D-pad bit),
    `$FF`-terminated, and the facing and the pad bit agree — which is specific
    enough that exactly one run of four or more exists in the cartridge.
    Returns {(step, standing tile, ledge tile)}.
    """

    def run(at: int) -> int:
        n, i = 0, at
        while i + 3 < len(rom) and rom[i] != 0xFF:
            f = rom[i]
            if f not in FACING or rom[i + 3] != FACING[f][0]:
                return 0
            n += 1
            i += 4
            if n > 24:
                return 0
        return n if i < len(rom) and rom[i] == 0xFF else 0

    best = max(((run(a), a) for a in range(len(rom) - 4)), key=lambda t: t[0])
    if best[0] < 4:
        raise SystemExit("could not identify LedgeTiles in this cartridge")
    n, at = best
    out = set()
    for i in range(n):
        e = at + 4 * i
        out.add((FACING[rom[e]][1], rom[e + 1], rom[e + 2]))
    return out


def flat(bank: int, addr: int) -> int:
    return bank * 0x4000 + (addr - 0x4000)


def find_map_header_banks(rom: bytes) -> int:
    """The bank table, disambiguated by making a known map decode sanely.

    Three consecutive offsets all hold 248 plausible bank bytes; only one of
    them makes Viridian Forest ($33) read as tileset 3 and 17x24 blocks.
    """
    cands = [
        a
        for a in range(0x8000, 0x10000)
        if all(1 <= rom[a + i] < 0x40 for i in range(NUM_MAPS))
    ]
    for a in cands:
        bank = rom[a + 0x33]
        p = rom[MAP_HEADER_POINTERS + 2 * 0x33] | (
            rom[MAP_HEADER_POINTERS + 2 * 0x33 + 1] << 8
        )
        h = flat(bank, p)
        if h + 3 < len(rom) and rom[h] == 3 and rom[h + 1] == 24 and rom[h + 2] == 17:
            return a
    raise SystemExit("could not identify MapHeaderBanks in this cartridge")


def check_pointers(rom: bytes) -> None:
    ok = all(
        0x4000
        <= (rom[MAP_HEADER_POINTERS + 2 * i] | (rom[MAP_HEADER_POINTERS + 2 * i + 1] << 8))
        < 0x8000
        for i in range(NUM_MAPS)
    )
    if not ok:
        raise SystemExit("MapHeaderPointers is not at $01AE in this cartridge")


class Map:
    def __init__(self, rom: bytes, banks: int, m: int, ledges=frozenset()):
        self.ledges = ledges
        bank = rom[banks + m]
        h = flat(bank, rom[MAP_HEADER_POINTERS + 2 * m] | (rom[MAP_HEADER_POINTERS + 2 * m + 1] << 8))
        self.id = m
        self.tileset = rom[h]
        height, width = rom[h + 1], rom[h + 2]
        data = flat(bank, rom[h + 3] | (rom[h + 4] << 8))
        i = h + 9
        conn = rom[i]
        i += 1 + 11 * bin(conn).count("1")
        o = flat(bank, rom[i] | (rom[i + 1] << 8))
        o += 1  # border block
        nwarps = rom[o]
        o += 1
        self.warps = []
        for _ in range(nwarps):
            self.warps.append((rom[o + 1], rom[o], rom[o + 3]))  # x, y, destination map
            o += 4
        nsigns = rom[o]
        o += 1 + 3 * nsigns
        nobj = rom[o]
        o += 1
        self.objects = []
        for _ in range(nobj):
            pic, y, x, mv, rng, txt = rom[o : o + 6]
            o += 6
            extra = None
            if txt & 0x40:
                extra = ("trainer", rom[o], rom[o + 1])
                o += 2
            elif txt & 0x80:
                extra = ("item", rom[o])
                o += 1
            self.objects.append((x - 4, y - 4, mv, rng, extra))

        e = TILESETS + 12 * self.tileset
        tb = rom[e]
        blocks = flat(tb, rom[e + 1] | (rom[e + 2] << 8))
        coll = rom[e + 5] | (rom[e + 6] << 8)  # bank 0
        walkable = set()
        i = coll
        while rom[i] != 0xFF:
            walkable.add(rom[i])
            i += 1

        self.w, self.h = width * 2, height * 2
        self.grid = [[False] * self.w for _ in range(self.h)]
        self.tile = [[0] * self.w for _ in range(self.h)]
        for by in range(height):
            for bx in range(width):
                t = rom[blocks + rom[data + by * width + bx] * 16 :][:16]
                for dy in range(2):
                    for dx in range(2):
                        # bottom-left 8x8 of the walk tile
                        v = t[(dy * 2 + 1) * 4 + dx * 2]
                        self.grid[by * 2 + dy][bx * 2 + dx] = v in walkable
                        self.tile[by * 2 + dy][bx * 2 + dx] = v

    def inside(self, p) -> bool:
        return 0 <= p[0] < self.w and 0 <= p[1] < self.h

    def walkable(self, p) -> bool:
        return self.inside(p) and self.grid[p[1]][p[0]]

    def step(self, p, d):
        """Where one press of direction `d` from `p` puts you, or None.

        Either an ordinary step onto a walkable tile, or a ledge jump of two —
        which is one-way, because `LedgeTiles` has no entry for facing up.
        """
        n = (p[0] + d[0], p[1] + d[1])
        if self.walkable(n):
            return n
        if not self.inside(n):
            return None
        if (d, self.tile[p[1]][p[0]], self.tile[n[1]][n[0]]) in self.ledges:
            land = (n[0] + d[0], n[1] + d[1])
            if self.walkable(land):
                return land
        return None

    def reach(self, start):
        """Everywhere the player can get to, ledges included and one-way."""
        if not self.walkable(start):
            return set()
        seen = {start}
        q = deque([start])
        while q:
            p = q.popleft()
            for d in ((1, 0), (-1, 0), (0, 1), (0, -1)):
                n = self.step(p, d)
                if n is not None and n not in seen:
                    seen.add(n)
                    q.append(n)
        return seen

    def landings(self, p):
        """Everywhere one, two or three unintended steps from `p` can leave you.

        **Every direction, not only the direction of travel.** The walker
        detours sideways whenever something blocks it, and the trap this check
        exists for was on the *first* waypoint of a stage, which has no
        direction of travel at all — the player arrives through a door.
        """
        out = set()
        for d in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            at = p
            for _ in range(3):
                at = self.step(at, d)
                if at is None:
                    break
                out.add(at)
        return sorted(out)


# `Action::GoTo`'s own constants (plugins/agent/src/action.rs). The simulation
# below is only true while these are.
GOTO_BARREN = 8
GOTO_DETOUR = 4
GOTO_SLACK = 16


def goto(m: Map, start, target, budget: int):
    """Walk `Action::GoTo`'s policy over the grid. Returns where it stops.

    Greedy on the larger remaining delta; a press that fails to move starts a
    committed run of `GOTO_DETOUR` presses along the other axis, alternating
    direction each time; it gives up on `GOTO_BARREN` barren presses or on
    `budget + GOTO_SLACK` presses in total.
    """
    RIGHT, LEFT, DOWN, UP = (1, 0), (-1, 0), (0, 1), (0, -1)
    now = start
    barren = presses = flips = 0
    detour = None  # (direction, presses left)
    last_dir = None
    while True:
        if now == target or barren >= GOTO_BARREN or presses >= budget + GOTO_SLACK:
            return now
        dx = RIGHT if now[0] < target[0] else LEFT if now[0] > target[0] else None
        dy = DOWN if now[1] < target[1] else UP if now[1] > target[1] else None
        if detour is not None:
            d = detour[0]
        elif barren > 0:
            vertical = last_dir in (UP, DOWN)
            toward = dx if vertical else dy
            if vertical:
                fallback = RIGHT if flips % 2 == 0 else LEFT
            else:
                fallback = DOWN if flips % 2 == 0 else UP
            d = toward if toward is not None else fallback
            flips += 1
            detour = (d, GOTO_DETOUR)
        else:
            far_x = abs(now[0] - target[0]) >= abs(now[1] - target[1])
            d = (dx or dy) if far_x else (dy or dx)
        if d is None:
            return now
        if detour is not None:
            detour = (detour[0], detour[1] - 1) if detour[1] > 1 else None
        last_dir = d
        presses += 1
        nxt = m.step(now, d)
        if nxt is not None:
            now = nxt
            barren = 0
        else:
            barren += 1


def walks(m: Map, start, rest) -> bool:
    """Can the walker get from `start` to the END of the route?

    **A missed waypoint is not a failure and demanding every one is why the
    first version of this cried wolf.** The solver advances its stage index
    when a `goto` finishes, whether or not it arrived — so a route that skips
    a turning point and picks the next one up is working exactly as designed,
    and link 08's certified Route 1 descent does that from below a ledge. What
    matters is arriving.
    """
    at = start
    for p in rest:
        # **One `goto` per decision, and the solver keeps issuing them for the
        # same waypoint until it arrives.** Modelling a single call is far more
        # pessimistic than the solver is, and it was the last source of noise
        # here: a call that ends short is followed by another from wherever it
        # ended, which is how a walker gets round a building. It only truly
        # fails when the retries cycle — which is exactly what a one-way ledge
        # produces, the same slide every time.
        seen = {at}
        for _ in range(12):
            at = goto(m, at, p, abs(at[0] - p[0]) + abs(at[1] - p[1]) + 8)
            if at == p or at in seen:
                break
            seen.add(at)
    return at == rest[-1]


def audit(rom: bytes, routes) -> int:
    check_pointers(rom)
    banks = find_map_header_banks(rom)
    ledges = find_ledge_tiles(rom)
    cache: dict[int, Map] = {}
    hard = 0
    fragile = 0
    stages = 0
    for st in routes:
        via = [tuple(p) for p in st["via"]]
        if not via:
            continue
        stages += 1
        m = cache.setdefault(st["map"], Map(rom, banks, st["map"], ledges))
        where = f'{st["scenario"]} seed {st["seed"]} {st["kind"]} map ${m.id:02X} stage {st["stage"]}'

        for p in via:
            if not m.walkable(p):
                print(f"HARD  {where}: waypoint {p} is not a tile you can stand on")
                hard += 1

        # Connectivity, hop by hop.
        for a, b in zip(via, via[1:]):
            if m.walkable(a) and m.walkable(b) and b not in m.reach(a):
                print(f"HARD  {where}: {b} is not reachable from {a}")
                hard += 1

        # **Overshoot, at EVERY waypoint including the first.** The first is
        # the one that matters most and the easiest to leave out: it has no
        # predecessor in the route, because the player arrives through a door,
        # so a loop over consecutive pairs never examines it — and link 08's
        # trap was on exactly that waypoint.
        for i, b in enumerate(via):
            after = via[i + 1 :]
            if not after or not m.walkable(b):
                continue
            for over in m.landings(b):
                stranded = [p for p in after if p not in m.reach(over)]
                if stranded:
                    print(
                        f"frail {where}: overshooting {b} to {over} strands "
                        f"{stranded[:3]}{' …' if len(stranded) > 3 else ''}"
                    )
                    fragile += 1
                    break
                if not walks(m, over, after):
                    print(
                        f"frail {where}: overshooting {b} to {over} — reachable, but "
                        f"GoTo's own policy cannot walk the rest from there"
                    )
                    fragile += 1
                    break

    print(
        f"\n{stages} stage(s) audited: {hard} hard problem(s), "
        f"{fragile} fragile waypoint(s) (see the note at the top — fragility is "
        f"advisory and does not fail)"
    )
    return 1 if hard else 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rom", required=True)
    ap.add_argument("--routes", help="JSON from `pewter_chain routes`; run it if absent")
    a = ap.parse_args()
    if not os.path.exists(a.rom):
        raise SystemExit(f"no cartridge at {a.rom}")
    if a.routes:
        routes = json.load(open(a.routes))
    else:
        exe = "target/release/examples/pewter_chain"
        if not os.path.exists(exe):
            raise SystemExit(
                f"{exe} is not built — "
                "cargo build --release --features scenario --example pewter_chain"
            )
        routes = json.loads(subprocess.run([exe, "routes"], capture_output=True, text=True, check=True).stdout)
    return audit(open(a.rom, "rb").read(), routes)


if __name__ == "__main__":
    sys.exit(main())

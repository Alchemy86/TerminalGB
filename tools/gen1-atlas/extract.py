#!/usr/bin/env python3
"""Regenerate the generated columns of the Gen 1 memory atlas.

The atlas (`docs/gen1/atlas/atlas.tsv`) is the single source of truth for where
Pokemon Red and Blue keep everything.  Five of its nine columns are *derived*
from a built `pret/pokered` checkout, and four are *ours*: the grouping we chose,
what we verified, how we verified it, and the prose.  This script rewrites the
derived columns and leaves ours alone, so a pokered bump is a reviewable diff
rather than a rewrite.

    tools/gen1-atlas/extract.py --pokered /path/to/pokered            # report drift
    tools/gen1-atlas/extract.py --pokered /path/to/pokered --write    # apply it

The checkout must have been *built* (`make blue`, RGBDS 1.0.3) so that
`pokeblue.map` exists and `pokeblue.gbc` matches `roms.sha1`; this script checks
the ROM's SHA-1 before it will believe a single address.  Nothing from pokered is
vendored here and nothing is fetched at run time: you point it at your own
checkout or you do not run it.

Only symbol names, addresses and structure are taken.  pokered's prose and
comments are not ours to copy, so every `desc` in the atlas is written here from
scratch and every claim is checked against a running cartridge by
`testharness/gen1atlas.rs`.
"""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import chapters  # noqa: E402
import mapfile  # noqa: E402

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ATLAS = os.path.join(REPO, "docs", "gen1", "atlas", "atlas.tsv")

# Byte-for-byte retail English Pokemon Blue (USA/Europe).  pokered's own
# `roms.sha1` carries this; we re-check it here so an unbuilt or patched
# checkout can never silently supply addresses for a different cartridge.
BLUE_SHA1 = "d7037c83e1ae5b39bde3c30787637ba1d4c48ce2"
RED_SHA1 = "ea9bcae617fdf159b045185467ae58b2e4a48b9a"

COLUMNS = [
    "region",   # derived: WRAM0 / SRAM / HRAM / VRAM / ROM0 / ROMX
    "bank",     # derived: bank number, or "-" where the region is not banked
    "addr",     # derived: $XXXX, the CPU address
    "len",      # derived: bytes this symbol owns before the next address
    "symbol",   # derived: the pokered label
    "role",     # derived: entry / alias / instance
    "sect",     # derived: the linker section the symbol was declared in
    "group",    # rules:   our chapter, from tools/gen1-atlas/chapters.py
    "verify",   # ours:    how the entry was checked; see docs/gen1/atlas/README.md
    "desc",     # ours:    what it is, in our own words
]
DERIVED = {"region", "bank", "addr", "len", "symbol", "role", "sect", "group"}
CURATED = [c for c in COLUMNS if c not in DERIVED]

# Regions walked in full.  Everything the CPU can address that the game declares
# storage in; ROM is enormous and mostly code, so it is opt-in (see ROM_SEEDS).
RAM_KINDS = ("VRAM", "SRAM", "WRAM0", "HRAM")

# ROM labels the atlas carries.  A curated list on purpose: pokeblue.map names
# 21,000 ROM symbols and all but a handful are code, so an unfiltered sweep would
# bury the data tables a reader actually wants.  Add to this list, never widen it
# to a prefix match.
ROM_SEEDS = {
    # Species tables
    "BaseStats", "MonBaseStats", "MonsterNames", "PokedexOrder", "MewBaseStats",
    "PokedexEntryPointers", "EvosMovesPointerTable", "MonPartyData",
    # Moves and items
    "Moves", "MoveNames", "ItemNames", "ItemPrices", "TMsHMs", "TechnicalMachines",
    # Trainers
    "TrainerNames", "TrainerDataPointers", "TrainerClassMoveChoiceModifications",
    "Tilesets", "ItemUseBall", "TextCommandProcessor", "TryLoadSaveFile",
    "SaveSAVtoSRAM", "LoadSAVCheckSum", "AddPartyMon", "CheckForDisobedience",
    "InitBattleVariables", "PlaceString", "BattleRandom", "CalcStats",
    "TrainerAIPointers", "TrainerPicAndMoneyPointers", "TrainerClassNames",
    # Graphics
    "MonsterPalettes", "SpriteSheetPointerTable", "TilesetHeaders",
    "FontGraphics", "TextBoxGraphics", "HpBarAndStatusGraphics", "BattleHudTiles1",
    "PokemonLogoGraphics", "NintendoCopyrightLogoGraphics", "GamefreakLogoGraphics",
    "RedPicFront", "ShrinkPic1", "ShrinkPic2",
    # Text and map machinery
    "TextCommands", "MapHeaderPointers", "MapHeaderBanks", "MapSongBanks",
    "MapSpriteSets", "MapSpriteSetsExtra", "WildDataPointers",
    # Sound
    "SFX_Headers_1", "Music_Headers_1", "SoundBanks",
    # Routines whose behaviour the atlas cites
    "Random_", "LoadEnemyMonData", "ReadTrainer", "CalcStats", "CalcStat",
    "AddPartyMon", "_AddPartyMon", "CalcCheckSum", "SaveSAVtoSRAM", "LoadSAV",
    "UncompressMonSprite", "UncompressSpriteData", "UncompressSpriteDataLoop",
    "LoadMapHeader", "LoadTileBlockMap", "ItemUsePokeBall", "GetMonHeader",
    "DisplayPokedexMenu_", "PlaceString", "TextCommandProcessor",
}


def rom_sha1(path: str) -> str:
    with open(path, "rb") as handle:
        return hashlib.sha1(handle.read()).hexdigest()


def split_digit_runs(name: str):
    """Yield (prefix, number, suffix, width) for every run of digits in `name`."""
    for m in re.finditer(r"\d+", name):
        yield name[: m.start()], int(m.group()), name[m.end():], len(m.group())


def slot_one_siblings(pre: str, post: str, width: int):
    """The names slot 1 could have, given how slot N was spelled.

    pokered numbers repeated structures both ways: `wPartyMon1`..`wPartyMon6`
    but `wSprite01`..`wSprite15`.  Looking only for a bare "1" misses every
    zero-padded family, which is 49 sprite slots that would otherwise be listed
    in full on a page nobody could read.
    """
    out = {pre + "1" + post}
    if width > 1:
        out.add(pre + "1".zfill(width) + post)
    return out


def row(kind, bank, addr, length, symbol, role, sect):
    return {
        "region": kind,
        "bank": str(bank) if kind in ("ROMX", "SRAM") else "-",
        "addr": f"${addr:04X}",
        "len": str(length),
        "symbol": symbol,
        "role": role,
        "sect": sect,
        "group": chapters.classify(kind, addr, symbol, sect),
        "verify": "",
        "desc": "",
    }


def derive_rows(sections, gaps, want_rom: bool):
    """Turn parsed map sections into atlas rows, deriving length and role.

    A symbol's length is the distance to the next *named* address in its section;
    the last symbol runs to the section's end.  Several symbols may share an
    address (pokered aliases them freely) — the first one listed is the entry,
    the rest are aliases, and they all describe the same bytes.

    Two kinds of row carry no symbol, and they are the point of walking a region
    rather than listing the interesting parts of it:

    * `gap` — bytes inside a section that no label reaches.  There is exactly one
      in WRAM and it is not an oversight: the *Stack* section is $DF00-$DFFF and
      its only label, `wStack`, sits at $DFFF, because the stack pointer starts
      at the top and grows downwards through the whole 256 bytes.
    * `free` — a run the linker reported as EMPTY, i.e. RAM the game declares no
      storage for at all.

    With those, every byte of a region is accounted for, and "unused" becomes
    something the atlas states rather than something a reader has to infer from
    a hole between two rows.
    """
    rows = []
    for sec in sections:
        if sec.kind in RAM_KINDS:
            keep = lambda name: "." not in name  # noqa: E731
        elif want_rom and sec.kind in ("ROM0", "ROMX"):
            keep = lambda name: name in ROM_SEEDS  # noqa: E731
        else:
            continue
        if not sec.symbols:
            continue

        kept = [(a, n) for a, n in sec.symbols if keep(n)]
        if not kept:
            continue
        # Distinct addresses in ascending order give every symbol its extent.
        addrs = sorted({a for a, _ in kept})
        extent = {}
        for i, a in enumerate(addrs):
            nxt = addrs[i + 1] if i + 1 < len(addrs) else sec.start + sec.size
            extent[a] = max(0, nxt - a)

        # A section whose first label is not at its start (the stack).
        if sec.kind in RAM_KINDS and addrs[0] > sec.start:
            rows.append(
                row(
                    sec.kind,
                    sec.bank,
                    sec.start,
                    addrs[0] - sec.start,
                    f"gap:{sec.kind}{sec.bank}:${sec.start:04X}",
                    "gap",
                    sec.name,
                )
            )

        names_here = {n for _, n in sec.symbols}
        first_at = {}
        for addr, name in kept:
            # Slot 2..N of a repeated structure: fold it away in the rendered
            # docs, but keep the row so a search for the symbol still lands.
            role = "entry"
            for pre, num, post, width in split_digit_runs(name):
                if num >= 2 and slot_one_siblings(pre, post, width) & names_here:
                    role = "instance"
                    break
            if role == "entry":
                if addr in first_at:
                    role = "alias"
                else:
                    first_at[addr] = name
            rows.append(
                row(sec.kind, sec.bank, addr, extent[addr], name, role, sec.name)
            )

    # An alias is the same bytes under another name, so it belongs in whatever
    # chapter its entry does — otherwise `wPartyMons` could be filed away from
    # `wPartyMon1Species`, and the indexes would link to an anchor that is on a
    # page the alias was never rendered onto. Same for slots 2..N of a repeated
    # structure, which belong with slot 1.
    entry_group = {}
    for r in rows:
        if r["role"] == "entry":
            entry_group[(r["region"], r["bank"], r["addr"])] = r["group"]
    for r in rows:
        if r["role"] == "alias":
            r["group"] = entry_group.get(
                (r["region"], r["bank"], r["addr"]), r["group"]
            )
    by_name = {r["symbol"]: r for r in rows}
    for r in rows:
        if r["role"] != "instance":
            continue
        for pre, num, post, width in split_digit_runs(r["symbol"]):
            if num < 2:
                continue
            sib = next(
                (b for b in slot_one_siblings(pre, post, width) if b in by_name), None
            )
            if sib:
                r["group"] = by_name[sib]["group"]
                break

    for gap in gaps:
        if gap.kind not in RAM_KINDS:
            continue
        rows.append(
            row(
                gap.kind,
                gap.bank,
                gap.start,
                gap.size,
                f"free:{gap.kind}{gap.bank}:${gap.start:04X}",
                "free",
                "(unallocated)",
            )
        )
    return rows


def read_atlas(path: str):
    if not os.path.exists(path):
        return [], []
    with open(path, encoding="utf-8") as handle:
        lines = [l.rstrip("\n") for l in handle]
    header = None
    rows = []
    preamble = []
    for line in lines:
        if line.startswith("#") or not line.strip():
            if header is None:
                preamble.append(line)
            continue
        fields = line.split("\t")
        if header is None:
            header = fields
            continue
        rows.append(dict(zip(header, fields)))
    return preamble, rows


def write_atlas(path: str, preamble, rows):
    with open(path, "w", encoding="utf-8") as handle:
        for line in preamble:
            handle.write(line + "\n")
        handle.write("\t".join(COLUMNS) + "\n")
        for row in rows:
            handle.write("\t".join(row.get(c, "") for c in COLUMNS) + "\n")


def sort_key(row):
    order = {"VRAM": 0, "SRAM": 1, "WRAM0": 2, "HRAM": 3, "ROM0": 4, "ROMX": 5}
    bank = 0 if row["bank"] == "-" else int(row["bank"])
    role = {"gap": -1, "free": -1, "entry": 0, "alias": 1, "instance": 2}[row["role"]]
    return (order[row["region"]], bank, int(row["addr"][1:], 16), role, row["symbol"])


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--pokered", required=True, help="a BUILT pret/pokered checkout")
    ap.add_argument("--game", default="blue", choices=("blue", "red"))
    ap.add_argument("--no-rom", action="store_true", help="RAM regions only")
    ap.add_argument("--write", action="store_true", help="apply changes to the atlas")
    args = ap.parse_args()

    stem = "pokeblue" if args.game == "blue" else "pokered"
    want = BLUE_SHA1 if args.game == "blue" else RED_SHA1
    rom = os.path.join(args.pokered, f"{stem}.gbc")
    mp = os.path.join(args.pokered, f"{stem}.map")
    for path in (rom, mp):
        if not os.path.exists(path):
            print(f"missing {path} — build the checkout first (make {args.game})",
                  file=sys.stderr)
            return 2
    got = rom_sha1(rom)
    if got != want:
        print(f"{rom} is sha1 {got}, expected {want}: not the retail cartridge",
              file=sys.stderr)
        return 2

    sections, gaps = mapfile.parse(mp)
    fresh = derive_rows(sections, gaps, want_rom=not args.no_rom)
    fresh.sort(key=sort_key)

    preamble, existing = read_atlas(ATLAS)
    curated = {r["symbol"]: {c: r.get(c, "") for c in CURATED} for r in existing}

    added, changed, dropped = [], [], []
    seen = set()
    for row in fresh:
        seen.add(row["symbol"])
        old = curated.get(row["symbol"])
        if old is None:
            added.append(row["symbol"])
        else:
            row.update(old)
        prev = next((r for r in existing if r["symbol"] == row["symbol"]), None)
        if prev is not None:
            diff = [c for c in DERIVED if prev.get(c, "") != row[c]]
            if diff:
                changed.append((row["symbol"], diff))
    dropped = [r["symbol"] for r in existing if r["symbol"] not in seen]

    print(f"{len(fresh)} rows derived from {os.path.basename(mp)} ({args.game})")
    print(f"  {len(added)} new, {len(changed)} moved, {len(dropped)} gone")
    for sym in added[:20]:
        print(f"    + {sym}")
    for sym, diff in changed[:20]:
        print(f"    ~ {sym} ({', '.join(sorted(diff))})")
    for sym in dropped[:20]:
        print(f"    - {sym}  (its curated description would be lost)")

    if args.write:
        if not preamble:
            preamble = [
                "# The Gen 1 memory atlas — the single source of truth.",
                "# Generated columns (region bank addr len symbol role) come from a built",
                "# pret/pokered checkout via tools/gen1-atlas/extract.py; the rest are ours.",
                "# Rendered to Markdown by tools/gen1-atlas/render.py, checked against a",
                "# running cartridge by testharness/gen1atlas.rs.  Do not hand-edit the",
                "# generated columns.",
            ]
        write_atlas(ATLAS, preamble, fresh)
        print(f"wrote {ATLAS}")
    elif added or changed or dropped:
        print("\n(dry run — pass --write to apply)")
    return 0


if __name__ == "__main__":
    sys.exit(main())

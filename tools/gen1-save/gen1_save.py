#!/usr/bin/env python3
"""Read and edit a Pokemon Red/Blue battery save (`.sav`) — story flags included.

    python3 tools/gen1-save/gen1_save.py                       # what the default save holds
    python3 tools/gen1-save/gen1_save.py show --sav <file>
    python3 tools/gen1-save/gen1_save.py events --filter oak
    python3 tools/gen1-save/gen1_save.py clear EVENT_GOT_POKEDEX --yes
    python3 tools/gen1-save/gen1_save.py checksum --sav <file>

Why this exists
---------------
This repository has a lot of Gen 1 save knowledge and no way to *look at* a
save. `pewter_chain states` reads `.gbss` save states, `gen1_tables.py` reads
the cartridge, and the only code that edits a battery file is private to
`testharness/trainer.rs`. So "why is this save behaving like that?" meant
hand-computing offsets every time — and the offsets have a trap in them: the
checksum is over `sGameData..sGameDataEnd` (`$A598..$B522`), **not** over
`sMainData`, so the obvious range is wrong and the game silently offers NEW
GAME instead of CONTINUE.

Every address here is the atlas's (`third_party/atlasgb/pokemon-rb/atlas.tsv`)
resolved the way `testharness/trainer.rs::inject` resolves it: a `.sav` is raw
cartridge RAM with no header, bank 0 first, so SRAM bank 1 starts at file
offset `0x2000`; `sPlayerName` is `01:A598`, `sMainData` `01:A5A3` (a copy of
WRAM `$D2F7..$DA7F`), `sPartyData` `01:AF2C` (WRAM `$D163..$D2F6`), and
`sMainDataCheckSum` `01:B523`.

The event-flag numbering is `pokered/constants/event_constants.asm`, which uses
`const_def` / `const_skip` / `const_next` arithmetic rather than literals. Point
`--pokered` (or `$POKERED`) at a checkout and every one of the 507 names is
available; without one, the built-in table below covers the early story, which
is what this repository's own scenarios pin.

What it does NOT do, said plainly
---------------------------------
It edits event flags and nothing else — no party, no bag, no boxes, no Pokedex
bits. Those have consistency rules between structures (the game cross-checks a
party mon's OT against `wPlayerID`, and the box banks carry their own
checksums), and a tool that writes them without the rules produces a save that
passes its checksum and hangs. Writing a party is `testharness/trainer.rs`'s
job, under test. This is the read-mostly companion to it.

It also cannot tell you whether a flag *should* be set. A save whose flags are
internally inconsistent — `EVENT_GOT_POKEDEX` set while the parcel is still in
the bag, say — is one the game cannot produce but will happily load.
"""

import argparse
import os
import shutil
import sys
import time

DEFAULT_SAV = os.path.expanduser(
    "~/roms/Pokemon - Blue Version (USA, Europe) (SGB Enhanced).sav")

# --- the save's own geometry -------------------------------------------------
BANK1 = 0x2000                 # SRAM bank 1 begins here in the file
S_PLAYER_NAME = 0xA598
S_MAIN = 0xA5A3                # == WRAM $D2F7
S_PARTY = 0xAF2C               # == WRAM $D163
S_CHECKSUM = 0xB523


def off(sram):
    """File offset of an SRAM bank-1 address."""
    return BANK1 + (sram - 0xA000)


def moff(wram):
    """File offset of a WRAM main-data address ($D2F7..$DA7F)."""
    return off(S_MAIN + (wram - 0xD2F7))


W_POKEDEX_OWNED = 0xD2F7
W_POKEDEX_SEEN = 0xD30A
W_NUM_BAG_ITEMS = 0xD31D
W_MONEY = 0xD347
W_BADGES = 0xD356
W_CUR_MAP = 0xD35E
W_Y, W_X = 0xD361, 0xD362
W_EVENT_FLAGS = 0xD747

# Early-story events, indices computed from `event_constants.asm`. Each is
# cross-checked against a second source in this repository: the three marked
# (*) are pinned independently by `scenarios/pewter/*.json` watches and by
# `AGENTS.md`'s own derivations.
EVENTS = {
    "EVENT_FOLLOWED_OAK_INTO_LAB": 0,           # (*) $D747 bit 0
    "EVENT_PALLET_AFTER_GETTING_POKEBALLS": 6,
    "EVENT_GOT_TOWN_MAP": 24,                   # (*) $D74A bit 0
    "EVENT_ENTERED_BLUES_HOUSE": 25,
    "EVENT_FOLLOWED_OAK_INTO_LAB_2": 32,
    "EVENT_OAK_ASKED_TO_CHOOSE_MON": 33,
    "EVENT_GOT_STARTER": 34,
    "EVENT_BATTLED_RIVAL_IN_OAKS_LAB": 35,
    "EVENT_GOT_POKEBALLS_FROM_OAK": 36,
    "EVENT_GOT_POKEDEX": 37,                    # (*) $D74B bit 5
    "EVENT_PALLET_AFTER_GETTING_POKEBALLS_2": 38,
    "EVENT_OAK_APPEARED_IN_PALLET": 39,
    "EVENT_OAK_GOT_PARCEL": 56,
    "EVENT_GOT_OAKS_PARCEL": 57,
    "EVENT_BEAT_BROCK": 119,                    # (*) $D755 bit 7
    "EVENT_BEAT_MISTY": 191,                    # (*) $D75E bit 7
}

_JSON = False
_BLOCKS = []


def _cell(v):
    return "" if v is None else str(v)


def table(name, rows, fields):
    if _JSON:
        _BLOCKS.append((name, [{f: r.get(f) for f in fields} for r in rows]))
        return
    print(f"{name}[{len(rows)}]{{{','.join(fields)}}}:")
    for r in rows:
        print("  " + ",".join(_cell(r.get(f)) for f in fields))


def obj(name, pairs):
    if _JSON:
        _BLOCKS.append((name, dict(pairs)))
        return
    print(f"{name}:")
    for k, v in pairs:
        print(f"  {k}: {_cell(v)}")


def helpblock(lines):
    if _JSON:
        return
    print(f"help[{len(lines)}]:")
    for l in lines:
        print("  " + l)


def die(msg, *help_lines, code=1):
    """Errors go to STDOUT with a way forward — an agent drives this through a
    shell and never sees stderr."""
    print(f"error: {msg}")
    if help_lines:
        helpblock(list(help_lines))
    sys.exit(code)


def flush_json():
    if _JSON:
        import json
        print(json.dumps({n: b for n, b in _BLOCKS}, indent=2))


# --- reading -----------------------------------------------------------------
def load(path):
    if not os.path.exists(path):
        die(f"no save file at {path}",
            "pass --sav <file>",
            "the launcher's default is ~/roms/<rom stem>.sav")
    d = bytearray(open(path, "rb").read())
    if len(d) < 0x8000:
        die(f"{path} is {len(d)} bytes; a Gen 1 .sav is 32768",
            "a .gbsave is the pre-rename format and is NOT the same file")
    return d


def calc_checksum(d):
    """`CalcCheckSum` (pokered engine/menus/save.asm): the complement of the
    byte sum over sGameData..sGameDataEnd."""
    s = 0
    for b in d[off(S_PLAYER_NAME):off(S_CHECKSUM)]:
        s = (s + b) & 0xFF
    return (~s) & 0xFF


def decode_name(d, at, n=11):
    out = []
    for b in d[at:at + n]:
        if b == 0x50:
            break
        if 0x80 <= b <= 0x99:
            out.append(chr(ord("A") + b - 0x80))
        elif 0xA0 <= b <= 0xB9:
            out.append(chr(ord("a") + b - 0xA0))
        elif 0xF6 <= b <= 0xFF:
            out.append(chr(ord("0") + b - 0xF6))
        elif b == 0x7F:
            out.append(" ")
        else:
            out.append(f"<{b:02X}>")
    return "".join(out)


def event_bit(d, index):
    i = moff(W_EVENT_FLAGS) + index // 8
    return (d[i] >> (index % 8)) & 1


def dex_bits(d, base):
    got = []
    for i in range(19):
        b = d[moff(base) + i]
        for k in range(8):
            if b >> k & 1:
                got.append(i * 8 + k + 1)
    return got


def load_event_names(pokered):
    """Every event name, from a pokered checkout. The file uses const_def /
    const_skip / const_next arithmetic, so the indices have to be walked."""
    import re
    path = os.path.join(pokered, "constants", "event_constants.asm")
    if not os.path.exists(path):
        die(f"no event_constants.asm under {pokered}",
            "--pokered wants a pret/pokered checkout",
            "git clone --depth 1 https://github.com/pret/pokered")
    idx, ev = 0, {}
    for line in open(path):
        l = line.split(";")[0].strip()
        if not l:
            continue
        m = re.match(r"const_def(?:\s+(\S+))?$", l)
        if m:
            idx = int(m.group(1).replace("$", "0x"), 0) if m.group(1) else 0
            continue
        m = re.match(r"const_skip(?:\s+(\S+))?$", l)
        if m:
            idx += int(m.group(1), 0) if m.group(1) else 1
            continue
        m = re.match(r"const_next\s+(\S+)$", l)
        if m:
            idx = int(m.group(1).replace("$", "0x"), 0)
            continue
        m = re.match(r"const\s+(\w+)$", l)
        if m:
            ev[m.group(1)] = idx
            idx += 1
    return ev


def resolve(names, wanted, index):
    if index is not None:
        return f"index {index}", index
    key = wanted.upper()
    if not key.startswith("EVENT_"):
        key = "EVENT_" + key
    if key in names:
        return key, names[key]
    near = [n for n in names if wanted.upper().strip("_") in n][:6]
    die(f"no event named {wanted!r}",
        *( [f"did you mean: {', '.join(near)}"] if near else
           ["run `events` to list what is available",
            "or point --pokered at a pret/pokered checkout for all 507"]),
        "or address it numerically with --index N",
        code=2)


# --- commands ----------------------------------------------------------------
def cmd_show(d, path, names):
    stored, calc = d[off(S_CHECKSUM)], calc_checksum(d)
    obj("save", [
        ("file", path),
        ("bytes", len(d)),
        ("player", decode_name(d, off(S_PLAYER_NAME))),
        ("checksum", f"stored {stored:02X} computed {calc:02X} "
                     f"{'OK' if stored == calc else 'MISMATCH — the game will refuse this save'}"),
    ])
    m = d[moff(W_MONEY):moff(W_MONEY) + 3]
    money = "".join(f"{b >> 4}{b & 15}" for b in m).lstrip("0") or "0"
    obj("player", [
        ("map", d[moff(W_CUR_MAP)]),
        ("position", f"({d[moff(W_X)]},{d[moff(W_Y)]})"),
        ("money", money),
        ("badges", f"{d[moff(W_BADGES)]:08b}"),
        ("party_count", d[off(S_PARTY)]),
    ])
    owned, seen = dex_bits(d, W_POKEDEX_OWNED), dex_bits(d, W_POKEDEX_SEEN)
    obj("pokedex", [
        ("owned", f"{len(owned)}  {owned}"),
        ("seen", f"{len(seen)}  {seen}"),
    ])
    rows = [{"event": n, "index": i,
             "addr": f"${W_EVENT_FLAGS + i // 8:04X}", "bit": i % 8,
             "set": event_bit(d, i)} for n, i in sorted(EVENTS.items(), key=lambda kv: kv[1])]
    table("story", rows, ["event", "index", "addr", "bit", "set"])
    helpblock([
        "python3 tools/gen1-save/gen1_save.py events --filter <text>",
        "python3 tools/gen1-save/gen1_save.py clear <EVENT_NAME> --yes",
        "python3 tools/gen1-save/gen1_save.py set <EVENT_NAME> --yes",
    ])


def cmd_events(d, names, filt):
    rows = []
    for n, i in sorted(names.items(), key=lambda kv: kv[1]):
        if filt and filt.upper() not in n:
            continue
        rows.append({"event": n, "index": i,
                     "addr": f"${W_EVENT_FLAGS + i // 8:04X}",
                     "bit": i % 8, "set": event_bit(d, i)})
    if not rows:
        obj("events", [("matching", 0), ("filter", filt)])
        helpblock(["drop --filter to list them all",
                   "--pokered <checkout> loads all 507 names"])
        return
    table("events", rows, ["event", "index", "addr", "bit", "set"])


def write_back(d, path, yes, backup):
    d[off(S_CHECKSUM)] = calc_checksum(d)
    if not yes:
        obj("dry-run", [("wrote", "nothing"),
                        ("new_checksum", f"{d[off(S_CHECKSUM)]:02X}")])
        helpblock(["add --yes to write the file"])
        return
    if backup:
        b = f"{path}.{time.strftime('%Y%m%d-%H%M%S')}.bak"
        shutil.copy2(path, b)
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(d)
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)
    obj("wrote", [("file", path),
                  ("backup", b if backup else "(none, --no-backup)"),
                  ("checksum", f"{d[off(S_CHECKSUM)]:02X}")])


def cmd_flag(d, path, names, which, wanted, index, yes, backup):
    label, i = resolve(names, wanted, index)
    at = moff(W_EVENT_FLAGS) + i // 8
    before = event_bit(d, i)
    want = 1 if which == "set" else 0
    if before == want:
        obj("unchanged", [("event", label), ("index", i),
                          ("addr", f"${W_EVENT_FLAGS + i // 8:04X}"),
                          ("bit", i % 8), ("already", want)])
        return
    if want:
        d[at] |= 1 << (i % 8)
    else:
        d[at] &= ~(1 << (i % 8)) & 0xFF
    obj("change", [("event", label), ("index", i),
                   ("addr", f"${W_EVENT_FLAGS + i // 8:04X}"),
                   ("bit", i % 8), ("from", before), ("to", want)])
    write_back(d, path, yes, backup)


def main():
    global _JSON
    p = argparse.ArgumentParser(add_help=False)
    p.add_argument("command", nargs="?", default="show")
    p.add_argument("name", nargs="?")
    p.add_argument("--sav", default=DEFAULT_SAV)
    p.add_argument("--pokered", default=os.environ.get("POKERED"))
    p.add_argument("--index", type=int)
    p.add_argument("--filter")
    p.add_argument("--yes", action="store_true")
    p.add_argument("--no-backup", action="store_true")
    p.add_argument("--json", action="store_true")
    p.add_argument("-h", "--help", action="store_true")
    a = p.parse_args()
    _JSON = a.json

    if a.help or a.command == "help":
        obj("gen1-save", [
            ("description", "read and edit a Pokemon Red/Blue battery .sav"),
            ("default_sav", DEFAULT_SAV),
        ])
        table("commands", [
            {"command": "show", "does": "player, pokedex, story flags, checksum"},
            {"command": "events", "does": "list event flags (--filter <text>)"},
            {"command": "set <EVENT>", "does": "set one flag, fix the checksum"},
            {"command": "clear <EVENT>", "does": "clear one flag, fix the checksum"},
            {"command": "checksum", "does": "verify, or repair with --yes"},
        ], ["command", "does"])
        helpblock([
            "a write needs --yes; without it you get the diff and nothing is touched",
            "every write makes a <file>.<stamp>.bak first (--no-backup to skip)",
            "--pokered <pret/pokered checkout> loads all 507 event names",
        ])
        flush_json()
        return

    names = dict(EVENTS)
    if a.pokered:
        names = load_event_names(a.pokered)
        # The built-in table is a hand-copy and hand-copies go stale. When the
        # real source is available, say so rather than letting a wrong index
        # sit there quietly writing the wrong bit.
        drift = [(n, EVENTS[n], names.get(n)) for n in EVENTS
                 if names.get(n) != EVENTS[n]]
        if drift:
            table("drift", [{"event": n, "builtin": b, "pokered": r}
                            for n, b, r in drift], ["event", "builtin", "pokered"])
            die("the built-in event table disagrees with pokered",
                "the pokered numbering is authoritative; fix EVENTS in this file")

    d = load(a.sav)
    backup = not a.no_backup

    if a.command == "show":
        cmd_show(d, a.sav, names)
    elif a.command == "events":
        cmd_events(d, names, a.filter)
    elif a.command in ("set", "clear"):
        if not a.name and a.index is None:
            die(f"`{a.command}` needs an event name",
                f"python3 tools/gen1-save/gen1_save.py {a.command} EVENT_GOT_POKEDEX --yes",
                "run `events` to see the names", code=2)
        cmd_flag(d, a.sav, names, a.command, a.name, a.index, a.yes, backup)
    elif a.command == "checksum":
        stored, calc = d[off(S_CHECKSUM)], calc_checksum(d)
        obj("checksum", [("file", a.sav), ("stored", f"{stored:02X}"),
                         ("computed", f"{calc:02X}"),
                         ("verdict", "OK" if stored == calc else "MISMATCH")])
        if stored != calc:
            if a.yes:
                write_back(d, a.sav, True, backup)
            else:
                helpblock(["add --yes to repair it in place"])
            sys.exit(0 if a.yes else 1)
    else:
        die(f"unknown command {a.command!r}",
            "commands: show, events, set, clear, checksum, help", code=2)
    flush_json()


if __name__ == "__main__":
    main()

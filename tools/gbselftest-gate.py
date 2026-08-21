#!/usr/bin/env python3
"""Score this emulator with the GBSelfTest cartridge, on both picture modes.

`conformance: 0/1` is not a gate. The cartridge runs eighty-seven checks and
prints one line each, so what is worth gating is WHICH of them fail — and the
answer differs between the two picture modes in a way that is documented and
deliberate:

Each mode has a list of rows it is expected to lose, and each entry says why.
A row that fails and is not on its list is a regression; a row on the list that
starts passing is a change too, and the script asks for the list and the table
in docs/gbselftest.md to move in the same commit.

That is the useful shape. A gate that only said "the default build fails the
cartridge" would be satisfied by any regression at all.

    tools/gbselftest-gate.py
    tools/gbselftest-gate.py --adapter target/release/examples/conformance_adapter

Standard library only, and it drives the reference adapter directly rather than
the runner: the runner's verdict is one word, and this needs the report.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
ROM = ROOT / "third_party" / "gbselftest" / "gbselftest.gb"

# Rows this emulator is expected to lose, per picture mode, and why. Anything
# here is a KNOWN gap or a deliberate trade-off; anything not here is news.
#
# The whole-scanline renderer draws a line in one go and charges mode 3 a fixed
# 172 cycles, so it can report no fetcher penalty at all; and it deliberately
# does not block VRAM or OAM by mode, because with a fixed mode 3 that would
# drop writes a game placed legally in HBlank. Both decisions are recorded in
# docs/picture-modes.md and in this project's own notes.
FETCHER_PENALTIES = {
    "GB-PPU-07": "fine scrolling: a fixed-length mode 3 cannot show the penalty",
    "GB-PPU-08": "objects on a line: same",
    "GB-PPU-09": "the window taking over: same",
    "GB-PPU-18": "an object off the left edge: same",
    "GB-PPU-10": "VRAM blocking is deliberately off with a fixed mode 3",
    "GB-PPU-11": "OAM blocking, for the same reason",
    "GB-PPU-20": "a mode-3 VRAM write is not dropped either — the write side of GB-PPU-10",
    "GB-PPU-21": "a scan-time OAM write is not dropped either — the write side of GB-PPU-11",
}

# A gap this project had already written down and had never been able to
# measure. docs/components/ppu.md says in as many words that the DMG STAT write
# bug is not modelled here: writing ANY value to STAT while the screen is on
# behaves for one cycle as though $FF had been written, so a condition that is
# true then raises the interrupt (Pan Docs, "Spurious STAT interrupts"). Two
# commercial games depend on it. SameBoy models it and passes this row; neither
# of our picture modes does.
#
# What is new is not the knowledge, it is that a prose note in a component page
# is now a check with a code on it. It is listed here rather than quietly
# dropped so that the day somebody implements it, this gate says so.
NOT_MODELLED_YET = {
    "GB-PPU-16": "the DMG spurious STAT-write interrupt is not modelled here",
}

KNOWN_FAILURES = {
    "identical": dict(NOT_MODELLED_YET),
    "standard": {**FETCHER_PENALTIES, **NOT_MODELLED_YET},
}

# Not anchored at the start of the line: GB-SER-03 shifts a byte out of the
# link port on purpose, and the link port is the log, so one verdict line can
# begin with that byte. A parser that insists on column zero silently loses a
# check, which is how this one first reported 75 of 76.
LINE = re.compile(r"(GB-[A-Z]+-\d+) (ok|FAIL|skip)\s+(.*)$")


def run(adapter: str, mode: str, frames: int) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        serial = Path(tmp) / "serial.txt"
        rom = Path(tmp) / ROM.name
        rom.write_bytes(ROM.read_bytes())  # never run a ROM where it is kept
        env = dict(os.environ)
        env.update(
            {
                "GB_PPU": mode,
                "GBCONFORM_ROM": str(rom),
                "GBCONFORM_MODEL": "dmg",
                "GBCONFORM_FRAMES": str(frames),
                "GBCONFORM_SERIAL_OUT": str(serial),
                "GBCONFORM_TIMEOUT_S": "120",
            }
        )
        subprocess.run([adapter, str(rom)], env=env, check=True, timeout=180)
        return serial.read_text(errors="replace") if serial.exists() else ""


def summarise(report: str) -> tuple[dict[str, str], str]:
    verdicts: dict[str, str] = {}
    for line in report.splitlines():
        m = LINE.search(line.strip("\r"))
        if m:
            verdicts[m.group(1)] = m.group(2)
    final = ""
    for line in report.splitlines():
        s = line.strip("\r").strip()
        if s in ("Passed", "Failed"):
            final = s
    return verdicts, final


def report_on(
    name: str, verdicts: dict[str, str], final: str, known: dict[str, str]
) -> None:
    ok = sum(1 for v in verdicts.values() if v == "ok")
    bad = sorted(c for c, v in verdicts.items() if v == "FAIL")
    skipped = sorted(c for c, v in verdicts.items() if v == "skip")
    print(f"  {name}: {ok}/{len(verdicts)} ok, {len(bad)} failed, "
          f"{len(skipped)} skipped, verdict {final or '(none)'}")
    for c in bad:
        why = known.get(c, "NOT A KNOWN GAP")
        print(f"      FAIL {c}  -- {why}")
    for c in skipped:
        print(f"      skip {c}")


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument(
        "--adapter", default=str(ROOT / "target/release/examples/conformance_adapter")
    )
    ap.add_argument("--frames", type=int, default=2000)
    args = ap.parse_args(argv)

    if not ROM.exists():
        print(f"{ROM} is missing; run tools/fetch-gbselftest.sh", file=sys.stderr)
        return 2
    if not Path(args.adapter).exists():
        print(
            f"{args.adapter} is missing; "
            "cargo build --release --example conformance_adapter",
            file=sys.stderr,
        )
        return 2

    print("GBSelfTest, both picture modes:")
    failures = []

    for mode in ("identical", "standard"):
        known = KNOWN_FAILURES[mode]
        verdicts, final = summarise(run(args.adapter, mode, args.frames))
        report_on(mode, verdicts, final, known)
        if not verdicts:
            failures.append(f"{mode} produced no report at all")
            continue
        bad = {c for c, v in verdicts.items() if v == "FAIL"}
        unexpected = bad - set(known)
        recovered = set(known) - bad
        if unexpected:
            failures.append(
                f"{mode} failed rows that are not known gaps: "
                f"{', '.join(sorted(unexpected))}. Each one's explanation is in "
                "the cartridge's own report and at docs/gbselftest.md."
            )
        if recovered:
            failures.append(
                f"{mode} now PASSES rows it was expected to lose: "
                f"{', '.join(sorted(recovered))}. That is good news and still a "
                "change: update KNOWN_FAILURES here and the table in "
                "docs/gbselftest.md in the same commit."
            )

    if failures:
        print()
        for f in failures:
            print(f"FAIL: {f}")
        return 1
    print("\nok: both picture modes match what this project documents")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

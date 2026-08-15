#!/usr/bin/env python3
"""Score this emulator with the GBSelfTest cartridge, on both picture modes.

`conformance: 0/1` is not a gate. The cartridge runs seventy-six checks and
prints one line each, so what is worth gating is WHICH of them fail — and the
answer differs between the two picture modes in a way that is documented and
deliberate:

  * `identical`, the per-dot renderer, must pass every row.
  * `standard`, the whole-scanline renderer, must lose exactly the five rows
    below and no others. Those are its documented approximations
    (`docs/picture-modes.md`), not defects, and a sixth red row is news.

That second expectation is the useful one. A gate that only says "the default
build fails the cartridge" would be satisfied by any regression at all.

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

# The whole-scanline renderer draws a line in one go and charges mode 3 a fixed
# 172 cycles, so it can report no fetcher penalty; and it deliberately does not
# block VRAM or OAM by mode, because with a fixed mode 3 that would drop writes
# a game placed legally in HBlank. Both decisions are recorded in
# docs/picture-modes.md and in this project's own notes.
STANDARD_KNOWN_FAILURES = {
    "GB-PPU-07",  # fine scrolling lengthens mode 3
    "GB-PPU-08",  # objects on a line lengthen mode 3
    "GB-PPU-09",  # the window lengthens mode 3
    "GB-PPU-10",  # VRAM is unreadable during mode 3
    "GB-PPU-11",  # OAM is unreadable during modes 2 and 3
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


def report_on(name: str, verdicts: dict[str, str], final: str) -> None:
    ok = sum(1 for v in verdicts.values() if v == "ok")
    bad = sorted(c for c, v in verdicts.items() if v == "FAIL")
    skipped = sorted(c for c, v in verdicts.items() if v == "skip")
    print(f"  {name}: {ok}/{len(verdicts)} ok, {len(bad)} failed, "
          f"{len(skipped)} skipped, verdict {final or '(none)'}")
    for c in bad:
        print(f"      FAIL {c}")
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

    verdicts, final = summarise(run(args.adapter, "identical", args.frames))
    report_on("identical", verdicts, final)
    if not verdicts:
        failures.append("identical produced no report at all")
    elif final != "Passed":
        failures.append(
            "the per-dot renderer must pass every check; it did not. The rows "
            "above are the ones to look at, and the cartridge's own explanation "
            "of each is in docs/gbselftest.md."
        )

    verdicts, final = summarise(run(args.adapter, "standard", args.frames))
    report_on("standard", verdicts, final)
    if not verdicts:
        failures.append("standard produced no report at all")
    else:
        bad = {c for c, v in verdicts.items() if v == "FAIL"}
        unexpected = bad - STANDARD_KNOWN_FAILURES
        recovered = STANDARD_KNOWN_FAILURES - bad
        if unexpected:
            failures.append(
                "the whole-scanline renderer failed rows that are not among its "
                f"documented approximations: {', '.join(sorted(unexpected))}"
            )
        if recovered:
            failures.append(
                "the whole-scanline renderer now PASSES rows it was expected to "
                f"lose: {', '.join(sorted(recovered))}. That is good news and "
                "still a change: update STANDARD_KNOWN_FAILURES here and the "
                "table in docs/gbselftest.md in the same commit."
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

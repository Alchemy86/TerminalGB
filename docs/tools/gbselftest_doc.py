#!/usr/bin/env python3
"""Keep `docs/gbselftest.md`'s per-code sections in step with the cartridge.

GBSelfTest prints a URL with every failure whose anchor is the check's code:

    -> https://github.com/Alchemy86/TerminalGB/blob/main/docs/gbselftest.md#gb-ppu-08

Those anchors have to exist, and they have to say what the ROM said. So the
per-code half of that page is not written here: it is spliced from the check
reference the cartridge ships beside its ROM, between the markers below. The
framing prose above and below the markers IS written here, and is left alone.

    docs/tools/gbselftest_doc.py            splice
    docs/tools/gbselftest_doc.py --check    fail if the page has drifted

Standard library only, like everything else in `docs/tools/`.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent
SOURCE = ROOT / "third_party" / "gbselftest" / "CHECKS.md"
PAGE = ROOT / "docs" / "gbselftest.md"

BEGIN = "<!-- checks:begin -->"
END = "<!-- checks:end -->"


def body_of(text: str, what: Path) -> str:
    try:
        start = text.index(BEGIN) + len(BEGIN)
        stop = text.index(END)
    except ValueError:
        raise SystemExit(f"{what} has no {BEGIN} / {END} markers")
    return text[start:stop].strip("\n")


def spliced(page: str, body: str) -> str:
    start = page.index(BEGIN) + len(BEGIN)
    stop = page.index(END)
    return page[:start] + "\n\n" + body + "\n\n" + page[stop:]


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args(argv)

    if not SOURCE.exists():
        raise SystemExit(
            f"{SOURCE} is missing; run tools/fetch-gbselftest.sh to vendor the "
            "cartridge and its check reference"
        )
    body = body_of(SOURCE.read_text(), SOURCE)
    page = PAGE.read_text()
    if BEGIN not in page or END not in page:
        raise SystemExit(f"{PAGE} has no {BEGIN} / {END} markers")
    want = spliced(page, body)

    codes = body.count("\n### GB-")
    if codes == 0:
        raise SystemExit("no check codes found; the reference's format has moved")

    if args.check:
        if page != want:
            print(
                f"{PAGE} has drifted from {SOURCE}; run docs/tools/gbselftest_doc.py",
                file=sys.stderr,
            )
            return 1
        print(f"{PAGE} matches the vendored check reference ({codes} codes)")
        return 0

    PAGE.write_text(want)
    print(f"spliced {codes} check codes into {PAGE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

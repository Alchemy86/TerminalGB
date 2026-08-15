"""Read an RGBDS link map.

The map file is what `rgblink -m` writes beside a ROM: every section, its extent,
and every symbol inside it.  It is the only artefact that carries *both* an
address and enough structure to derive a length, which is why the atlas is built
from it rather than from the `.sym` file.

Nothing here is specific to Pokemon; it is a parser for the RGBDS map grammar:

    ROMX bank #3:
    <tab>SECTION: $4000-$7c48 ($3c49 bytes) ["bank1"]
    <tab>         $4000 = SpriteFacingAndAnimationTable
    <tab>EMPTY: $7c49-$7fff ($03b7 bytes)

Used by `extract.py`.  See docs/gen1/atlas/README.md for why we regenerate rather
than transcribe.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, field

BANK_RE = re.compile(r"^(?P<kind>[A-Z][A-Z0-9]*) bank #(?P<bank>\d+):\s*$")
SECTION_RE = re.compile(
    r"^\s+SECTION: \$(?P<start>[0-9a-f]{4})"
    r"(?:-\$(?P<end>[0-9a-f]{4}))? "
    r"\(\$(?P<size>[0-9a-f]+) bytes\) "
    r'\["(?P<name>.*)"\]\s*$'
)
SYMBOL_RE = re.compile(r"^\s+\$(?P<addr>[0-9a-f]{4}) = (?P<name>\S+)\s*$")
EMPTY_RE = re.compile(
    r"^\s+EMPTY: \$(?P<start>[0-9a-f]{4})-\$(?P<end>[0-9a-f]{4}) "
    r"\(\$(?P<size>[0-9a-f]+) bytes\)\s*$"
)


@dataclass
class Section:
    kind: str
    bank: int
    name: str
    start: int
    size: int
    # (address, symbol) in file order; several symbols may share one address.
    symbols: list[tuple[int, str]] = field(default_factory=list)

    @property
    def end(self) -> int:
        """Last address *inside* the section.  A zero-length section has end < start."""
        return self.start + self.size - 1


@dataclass
class Gap:
    kind: str
    bank: int
    start: int
    size: int


def parse(path: str) -> tuple[list[Section], list[Gap]]:
    """Parse a map file into its sections and its declared empty runs."""
    sections: list[Section] = []
    gaps: list[Gap] = []
    kind, bank = None, None
    current: Section | None = None

    with open(path, encoding="utf-8") as handle:
        for line in handle:
            m = BANK_RE.match(line)
            if m:
                kind, bank = m["kind"], int(m["bank"])
                current = None
                continue
            if kind is None:
                # The header lines before the first bank ("WRAM0: 8162 bytes used").
                continue
            m = SECTION_RE.match(line)
            if m:
                current = Section(
                    kind=kind,
                    bank=bank,
                    name=m["name"],
                    start=int(m["start"], 16),
                    size=int(m["size"], 16),
                )
                sections.append(current)
                continue
            m = EMPTY_RE.match(line)
            if m:
                gaps.append(
                    Gap(
                        kind=kind,
                        bank=bank,
                        start=int(m["start"], 16),
                        size=int(m["size"], 16),
                    )
                )
                current = None
                continue
            m = SYMBOL_RE.match(line)
            if m and current is not None:
                current.symbols.append((int(m["addr"], 16), m["name"]))

    return sections, gaps

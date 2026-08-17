#!/usr/bin/env python3
"""A reference `--student` arm for the test rig: a player that uses **only the
screen**.

This is the template a trained network is wrapped in. It is deliberately not a
policy — it decodes the frame, prints what it saw to stderr, and returns a
plan — because the point is to show, in one readable file, exactly what a
screen-only player receives and returns. Swap `decide()` for a forward pass and
it is a real arm.

    cargo run --release --features scenario --example pewter_chain -- \\
        ab --rom "$ROM" --plan scenarios/testrig/starter-rival-battle.json \\
           --split unseen --student "python3 tools/testrig/screen_adapter.py"

The contract is `plugins/agent/src/subprocess.rs`, in full:

    stdin   one line of JSON: the state snapshot
    stdout  one JSON object:  {"goal":…, "say":…, "plan":[…]}
    stderr  ignored by the panel, copied to the host's log
    exit 0  success; anything else is a failed call

**The snapshot carries `frame_png`** — a path to the current frame — whenever
the arm is a subprocess, because the rig turns the picture back on for an arm
that might look at it. Every other field in the snapshot is *derived from
memory*: `screen.text`, the player's position, the party. A screen-only player
must ignore all of it, which is what `decide()` below does and what makes the
result mean something. Reading `game.party[0].hp` would be reading the answer
out of RAM.

Stdlib only, on purpose: an adapter must be runnable by whoever is evaluating,
without a build.
"""

import json
import struct
import sys
import zlib

# The Game Boy's four shades, darkest first. A DMG frame quantises to exactly
# these, which is what makes "how many distinct shades are on screen" a
# categorical check rather than a threshold — the same guard
# `src/screen/halfblocks.rs` uses.
SHADES = 4


def read_frame(path):
    """Decode a palette PNG into a list of rows of shade indices.

    Hand-rolled because the point of this file is that it runs anywhere. The
    emulator writes a 2-bit palette PNG for a four-shade frame, which is the
    only case handled here; a truecolour frame returns None rather than a
    guess.
    """
    with open(path, "rb") as f:
        data = f.read()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    pos, idat, ihdr = 8, b"", None
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        kind = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if kind == b"IHDR":
            ihdr = struct.unpack(">IIBB", body[:10])
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    if ihdr is None:
        return None
    width, height, depth, colour = ihdr
    if colour != 3 or depth not in (1, 2, 4, 8):
        return None
    raw = zlib.decompress(idat)
    stride = (width * depth + 7) // 8
    rows, pos, prior = [], 0, bytearray(stride)
    per_byte = 8 // depth
    mask = (1 << depth) - 1
    for _ in range(height):
        filt = raw[pos]
        pos += 1
        line = bytearray(raw[pos : pos + stride])
        pos += stride
        # PNG filters. Byte-per-pixel is 1 for any sub-8-bit palette image.
        if filt == 1:
            for i in range(1, stride):
                line[i] = (line[i] + line[i - 1]) & 0xFF
        elif filt == 2:
            for i in range(stride):
                line[i] = (line[i] + prior[i]) & 0xFF
        elif filt == 3:
            for i in range(stride):
                left = line[i - 1] if i else 0
                line[i] = (line[i] + ((left + prior[i]) >> 1)) & 0xFF
        elif filt == 4:
            for i in range(stride):
                a = line[i - 1] if i else 0
                b = prior[i]
                c = prior[i - 1] if i else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 0xFF
        prior = line
        px = []
        for i in range(width):
            byte = line[i // per_byte]
            shift = 8 - depth * (i % per_byte + 1)
            px.append((byte >> shift) & mask)
        rows.append(px)
    return rows


def decide(rows):
    """The whole policy. Replace this with a forward pass.

    Deliberately trivial and deliberately screen-only: it looks at the bottom
    quarter of the picture, where Gen 1 draws every text box and menu, and
    presses A when there is something there rather than walking into it.
    """
    if rows is None:
        return "a", "no picture, so pressing A"
    height = len(rows)
    band = rows[int(height * 0.72) :]
    ink = sum(1 for row in band for p in row if p != band[0][0])
    if ink > len(band) * len(band[0]) * 0.12:
        return "a", f"something is drawn in the bottom band ({ink} px)"
    return "down", "the bottom band is quiet, so moving"


def main():
    snapshot = json.loads(sys.stdin.readline() or "{}")
    # The ONE field a screen-only player may read. Everything else in the
    # snapshot comes from memory.
    path = snapshot.get("frame_png")
    rows = read_frame(path) if path else None
    if rows is not None:
        shades = len({p for row in rows for p in row})
        print(
            f"frame {len(rows[0])}x{len(rows)}, {shades} shade(s)",
            file=sys.stderr,
        )
        if shades > SHADES:
            # Categorical, not a threshold: a Game Boy frame has four shades,
            # so anything else means the picture is not what it claims to be.
            print("WARNING: more shades than a DMG can draw", file=sys.stderr)
    else:
        print(f"no frame ({path!r})", file=sys.stderr)

    button, why = decide(rows)
    json.dump(
        {
            "goal": "Get through this screen using only the picture",
            "say": why,
            "plan": [{"do": "press", "button": button}],
        },
        sys.stdout,
    )
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()

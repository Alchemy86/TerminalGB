#!/usr/bin/env python3
"""Assert that an MCP transcript really contains a Game Boy frame.

The smoke test for the container image and the Codespace. A server that
*starts* is not a server that emulates, so the check goes all the way to the
pixels: it finds the `screenshot` reply, base64-decodes it, requires a PNG
signature, and reads the width and height out of the IHDR chunk. 160x144 or it
fails.

Standard library only, like everything else in `tools/` — an image decoder here
would be a dependency the emulator itself does not have.

    usage: check_frame.py <transcript.jsonl> [request-id]
"""

import base64
import json
import struct
import sys

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
WIDTH, HEIGHT = 160, 144


def main(argv):
    if len(argv) < 2:
        print(__doc__.strip().splitlines()[-1], file=sys.stderr)
        return 2
    path = argv[1]
    want_id = int(argv[2]) if len(argv) > 2 else 3

    frame = None
    with open(path, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                # The server writes one JSON object per line; anything else on
                # the transport is a bug worth reporting rather than skipping.
                print(f"not JSON on the transport: {line[:120]}", file=sys.stderr)
                return 1
            if msg.get("id") != want_id:
                continue
            if "error" in msg:
                print(f"the server returned an error: {msg['error']}", file=sys.stderr)
                return 1
            for block in msg.get("result", {}).get("content", []):
                if block.get("type") == "image":
                    frame = base64.b64decode(block["data"])

    if frame is None:
        print(f"no frame in {path} for id {want_id}", file=sys.stderr)
        return 1
    if frame[:8] != PNG_MAGIC:
        print("the frame is not a PNG", file=sys.stderr)
        return 1
    w, h = struct.unpack(">II", frame[16:24])
    if (w, h) != (WIDTH, HEIGHT):
        print(f"the frame is {w}x{h}, not {WIDTH}x{HEIGHT}", file=sys.stderr)
        return 1
    print(f"ok — a real {w}x{h} PNG, {len(frame)} bytes, straight out of the emulator")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

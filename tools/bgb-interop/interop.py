#!/usr/bin/env python3
"""Drive TerminalGB's BGB link transport from somebody else's implementation.

The counterparty is `utilities/bgb_link_cable_server.py` out of
`Lorenzooone/PokemonGB_Online_Trades` (MIT), imported and used **unchanged** —
not reimplemented here, because a reimplementation would only prove that our
reading of the specification agrees with itself.

See `README.md` beside this file for what is being checked and why.

Exit status 0 on success, 1 on a failed check, 2 on a setup problem.
"""

import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
PGOT = os.environ.get(
    "PGOT_DIR", os.path.join(REPO, "target/bgb-interop/PokemonGB_Online_Trades")
)
EXAMPLE = os.environ.get(
    "BGB_LINK_BIN", os.path.join(REPO, "target/release/examples/bgb_link")
)

HOST = "127.0.0.1"
PORT = int(os.environ.get("BGB_INTEROP_PORT", "18770"))

# What the master sends. Deliberately not a run of one value and not a straight
# ramp: `0xFE` is Gen 1's "no data" byte, `0xFD` its block preamble, `0x00` what
# the server sends before it has anything to say and `0xFF` the idle line, so
# every value with a meaning of its own is in the stream.
#
# `0x5A` is deliberately absent — see `SENTINEL` below.
PAYLOAD = [0x01, 0x02, 0xFD, 0xFE, 0x00, 0xFF, 0x55, 0xA5, 0x3C, 0xC3, 0x10, 0x80]

# The byte the echo ROM offers before anything has arrived, and the thing that
# makes this test need no `sleep` anywhere.
#
# The problem it solves: the emulated console boots before it arms its link
# port, and bytes sent into that window are refused — correctly, that is what an
# unarmed console does — so a master that starts sending immediately loses its
# first few. Rather than guessing a wait, probe with `0x00` until this comes
# back: the ROM answers with the complement of whatever it last received, so it
# can only ever offer `$A5` again after receiving `$5A`, which is why `0x5A` is
# not in the payload. One `$A5` therefore means "the console has just accepted
# its first byte", exactly and only.
SENTINEL = 0xA5
PROBE = 0x00


def die(msg, code=2):
    print(f"interop: {msg}", file=sys.stderr)
    sys.exit(code)


def main():
    if not os.path.isdir(PGOT):
        die(f"{PGOT} is not there — run tools/bgb-interop/fetch.sh first")
    if not os.path.isfile(EXAMPLE):
        die(
            f"{EXAMPLE} is not there — build it with\n"
            "  cargo build --release --features "
            "image,plugin-gen1-link,plugin-trainer,bgblink --example bgb_link"
        )

    sys.path.insert(0, PGOT)
    try:
        from utilities.bgb_link_cable_server import BGBLinkCableServer
    except Exception as e:  # pragma: no cover - setup path
        die(f"cannot import the counterparty's server: {e}")

    # The two attributes `BGBLinkCableServer` reads off the menu object. Passing
    # a stand-in rather than driving the tool's interactive menu is the only
    # change to how it is used; not one line of it is modified.
    class Menu:
        verbose = True
        emulator = (HOST, PORT)

    received = []

    def data_handler(byte):
        received.append(byte)

    killed = []

    def kill_function():
        killed.append(True)

    server = BGBLinkCableServer(data_handler, Menu(), kill_function)
    server.daemon = True
    server.start()
    time.sleep(0.3)

    # One accepted probe, then the payload.
    want = 1 + len(PAYLOAD)
    child = subprocess.Popen(
        [
            EXAMPLE,
            "--name",
            "interop",
            "--connect",
            f"{HOST}:{PORT}",
            "--echo",
            str(want),
            "--frames",
            "2000000",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )

    try:
        # `can_go` is the server's own gate: it is set when the client answers a
        # master transfer with sync3 `b2 = 1` ("not doing a passive transfer").
        # Nothing moves until then, and it is the one thing about this
        # counterparty an implementor has to know.
        deadline = time.time() + 30
        while not server.can_go and time.time() < deadline:
            if child.poll() is not None:
                break
            time.sleep(0.05)
        if not server.can_go:
            print(child.communicate()[0])
            die("the server never reached `can_go` — no sync3 ack from the emulator", 1)

        def exchange(b):
            """Send one byte as master and return what came back."""
            before = len(received)
            server.to_send = b
            t = time.time() + 10
            while server.to_send is not None and time.time() < t:
                time.sleep(0.001)
            if server.to_send is not None:
                die(f"the server never managed to send {b:#04x}", 1)
            while len(received) == before and time.time() < t:
                time.sleep(0.001)
            if len(received) == before:
                die(f"no answer to {b:#04x} within ten seconds", 1)
            return received[-1]

        # Probe until the console has finished booting and armed its port.
        probes = 0
        while True:
            probes += 1
            if probes > 200:
                die("the emulated console never armed its link port", 1)
            if exchange(PROBE) == SENTINEL:
                break

        base = len(received)
        for b in PAYLOAD:
            exchange(b)

        out, _ = child.communicate(timeout=60)
    except subprocess.TimeoutExpired:  # pragma: no cover - setup path
        child.kill()
        die("the emulator did not exit", 1)
    finally:
        if child.poll() is None:
            child.kill()

    print(out)

    print(f"took {probes} probe(s) before the console had armed its port")

    answers = received[base:]
    # Having just accepted the last probe, the console now offers its
    # complement, then the complement of each payload byte in turn.
    expected = [(~PROBE) & 0xFF] + [(~b) & 0xFF for b in PAYLOAD[:-1]]
    ok = True

    if answers != expected:
        print(
            "FAIL: the master received "
            f"{[hex(b) for b in answers]}, expected {[hex(b) for b in expected]}"
        )
        ok = False
    else:
        print(
            f"OK: the master received {[hex(b) for b in answers]} — the "
            "complement of each byte it sent, one step behind"
        )

    line = next(
        (l for l in out.splitlines() if l.startswith("[interop] echo-received:")), None
    )
    if line is None:
        print("FAIL: the emulator printed no echo-received line")
        ok = False
    else:
        got = [int(x, 16) for x in line.split(":", 1)[1].split()]
        got = got[1:]  # the probe that unblocked it
        if got != PAYLOAD:
            print(
                f"FAIL: the emulator received {[hex(b) for b in got]}, "
                f"expected {[hex(b) for b in PAYLOAD]}"
            )
            ok = False
        else:
            print(f"OK: the emulator received {[hex(b) for b in got]} — "
                  "exactly what the master sent, in order")

    if child.returncode != 0:
        print(f"FAIL: the emulator exited {child.returncode}")
        ok = False

    print("\ninterop:", "PASS" if ok else "FAIL")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    shutil.rmtree(os.path.join(REPO, "target/bgb-link/interop"), ignore_errors=True)
    main()

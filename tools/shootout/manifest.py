#!/usr/bin/env python3
"""Turn a GBEmulatorShootout checkout into a manifest our scout can run.

    python3 tools/shootout/manifest.py <shootout-checkout> > manifest.tsv
    cargo run --release --features image --example shootout_scout -- manifest.tsv

The shootout defines its tests in Python (`testroms/*.py`, each a list of
`Test(...)`), and the only way to read that list without guessing is to import
it — so this stubs their `test.py` with something that records the arguments
instead of loading PIL, and imports their modules against it. **Nothing about
the test set is duplicated here**; if they add a ROM, this picks it up.

Stdlib only, deliberately: the same rule `tools/conformance/` follows. A tool
that costs a `pip install` is one nobody runs before quoting a number.

Output is TSV rather than JSON so the Rust side needs no parser:

    name <TAB> model <TAB> frames <TAB> rom <TAB> pass,refs <TAB> fail,refs <TAB> features
"""

import os
import sys
import tempfile

# A DMG frame is 70224 cycles at 4.194304 MHz.
FPS = 59.7275
# `Emulator.run` gives a test `runtime / speed + startup_time + 5.0` seconds of
# wall clock, polling the screen at 10 Hz and exiting early on a match. Speed 1
# and the default 1.0 s startup are the honest budget for a headless replay;
# nothing here can pass *later* than the real runner would allow.
STARTUP_S = 1.0
SLACK_S = 5.0

STUB = '''
import os
PASS, FAIL, INFO = "PASS", "FAIL", "INFO"
DMG, CGB, SGB = "DMG", "CGB", "SGB"
PCM = "PCM"

class Test:
    def __init__(self, name, *, runtime, rom=None, result=None, model=DMG,
                 required_features=None, description=None, url=None, tags=None):
        rom = rom or name
        self.name = name
        self.rom = os.path.join("testroms", rom)
        self.model = model
        self.runtime = runtime
        self.required_features = sorted(required_features or set())
        if result is None:
            result = os.path.splitext(rom)[0] + ".png"
        results = result if isinstance(result, list) else [result]
        self.results = [os.path.join("testroms", r) for r in results]
'''


def main(argv):
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    root = os.path.abspath(argv[1])
    if not os.path.isdir(os.path.join(root, "testroms")):
        sys.stderr.write(f"{root} does not look like a GBEmulatorShootout checkout\n")
        return 2

    with tempfile.TemporaryDirectory() as stubdir:
        with open(os.path.join(stubdir, "test.py"), "w") as f:
            f.write(STUB)
        # The stub must shadow their `test.py`, so it goes first.
        sys.path.insert(0, stubdir)
        sys.path.insert(1, root)

        # `main.py` fixes both the module list and the order tests run in.
        #
        # **Read each module's `all`, never every `Test(...)` it constructs.**
        # `mealybug.py` builds three lists — DMG, CGB RevC, CGB RevD — and ends
        # with `all = dmgs # + cgbcs + cgbds`, so 47 of the 71 objects it makes
        # are not part of the suite. Collecting constructions instead of `all`
        # inflates the denominator from the real **264** to 311 and buries a
        # DMG-only score under CGB rows nobody is scored on.
        tests = []
        for mod in ("acid", "blargg", "daid", "ax6", "mooneye", "samesuite",
                    "ashiepaws", "cpp", "mealybug"):
            m = __import__("testroms." + mod, fromlist=["all"])
            tests.extend(m.all)

        rows = 0
        missing = 0
        for t in tests:
            rom = os.path.join(root, t.rom)
            if not os.path.exists(rom):
                sys.stderr.write(f"missing ROM: {t.rom}\n")
                missing += 1
                continue
            frames = int((t.runtime / 1.0 + STARTUP_S + SLACK_S) * FPS)
            passes, fails = [], []
            for r in t.results:
                p = os.path.join(root, r)
                if os.path.exists(p):
                    passes.append(p)
                # A `.fail.png` is a reference for a *known-wrong* screen: the
                # runner reports FAIL early instead of waiting out the budget.
                q = os.path.splitext(p)[0] + ".fail.png"
                if os.path.exists(q):
                    fails.append(q)
            print("\t".join([
                t.name, t.model, str(frames), rom,
                ",".join(passes), ",".join(fails),
                ",".join(t.required_features),
            ]))
            rows += 1

    sys.stderr.write(f"{rows} tests\n")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))

#!/usr/bin/env python3
"""Split a `drill_all.py` run into a held-out training set and its test pool.

    split_heldout.py [--out target/learn-all] [--every 4]

The held-out question is whether the distillation transfers to *species lines
the student has never seen*, so the split is by **line**, never by prompt: a
prompt whose line contributed corpus rows to the fit is not a held-out prompt,
however different it looks. Every `--every`-th line by name goes to the test
side, which needs no seed file and is the same split whoever runs it.

Writes, under `--out`:

    corpus-all.ndjson    every line's corpus rows
    corpus-train.ndjson  the rows of the lines NOT in the test split
    pool-all/            every drilled prompt, renumbered
    pool-test/           the test lines' prompts, renumbered

Stdlib only. Prints TOON on stdout, like the other agent-facing tools here.
"""

import argparse
import pathlib
import sys

SELF = "python3 tools/learn-student/split_heldout.py"


def toon_obj(name, pairs):
    print(f"{name}:")
    for k, v in pairs:
        print(f"  {k}: {v}")


def die(msg, helps, code=2):
    print(f"error: {msg}")
    for h in helps:
        print(f"help: {h}")
    sys.exit(code)


def rows_of(line_dir):
    """Every `agent.ndjson` under one line's output directory."""
    return sorted(line_dir.glob("*/agent.ndjson"))


def prompts_of(line_dir):
    """The (state, expectation) pairs one line wrote, in its own order."""
    d = line_dir / "prompts"
    if not d.is_dir():
        return []
    out = []
    for st in sorted(d.glob("*.gbss")):
        exp = st.with_suffix(".expect.json")
        if exp.exists():
            out.append((st, exp))
    return out


def write_pool(dest, groups):
    """One pool out of many lines', renumbered. `NNNNN` is the rig's contract."""
    dest.mkdir(parents=True, exist_ok=True)
    for old in dest.glob("*"):
        old.unlink()
    n = 0
    for pairs in groups:
        for st, exp in pairs:
            (dest / f"{n:05d}.gbss").write_bytes(st.read_bytes())
            (dest / f"{n:05d}.expect.json").write_text(exp.read_text())
            n += 1
    return n


def main():
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--out", default="target/learn-all")
    ap.add_argument("--every", type=int, default=4,
                    help="every Nth line by name is held out (default 4)")
    a = ap.parse_args()
    out = pathlib.Path(a.out)
    lines_dir = out / "lines"
    if not lines_dir.is_dir():
        die(f"{lines_dir} does not exist — nothing has been drilled", [
            f"python3 tools/learn-student/drill_all.py --rom <blue.gb> --out {out}"])
    if a.every < 2:
        die(f"--every {a.every} would hold out every line", [f"{SELF} --every 4"])

    lines = sorted(d for d in lines_dir.iterdir() if d.is_dir())
    test = set(lines[::a.every])
    train = [d for d in lines if d not in test]

    all_rows, train_rows = [], []
    for d in lines:
        for f in rows_of(d):
            text = f.read_text()
            all_rows.append(text)
            if d not in test:
                train_rows.append(text)
    (out / "corpus-all.ndjson").write_text("".join(all_rows))
    (out / "corpus-train.ndjson").write_text("".join(train_rows))

    n_all = write_pool(out / "pool-all", [prompts_of(d) for d in lines])
    n_test = write_pool(out / "pool-test", [prompts_of(d) for d in sorted(test)])

    count = lambda t: sum(1 for l in t.splitlines() if l.strip())
    toon_obj("split", [
        ("lines", len(lines)),
        ("lines_held_out", len(test)),
        ("lines_fitted_on", len(train)),
        ("corpus_rows_all", count("".join(all_rows))),
        ("corpus_rows_train", count("".join(train_rows))),
        ("prompts_all", n_all),
        ("prompts_held_out", n_test),
    ])
    print("help[2]:")
    print(f"  python3 tools/learn-student/policy.py train "
          f"--corpus {out}/corpus-train.ndjson --out target/learn-student/train.json")
    print("  cargo run --release --features scenario --example pewter_chain -- "
          "ab --rom <blue.gb> --plan scenarios/testrig/move-learning-heldout.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())

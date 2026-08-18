#!/usr/bin/env python3
"""Drill every species line the cartridge can be walked along, in parallel.

    drill_all.py --rom <blue.gb> [--out target/learn-all] [--workers N]

One plan per species from `learn_table.py --drill-plan`, one drill process per
plan, fanned out. **Each process gets its own `--prompts` directory and the
pools are merged afterwards**: the drill numbers a prompt on from what is
already in the directory it is writing to, which is exactly right for two lines
run one after another and is a race between processes. Merging is a rename, so
the contract's `NNNNN` naming survives.

Stdlib only. Prints TOON on stdout, like the other agent-facing tools here.
"""

import argparse
import concurrent.futures as cf
import contextlib
import io
import json
import os
import pathlib
import subprocess
import sys
import time

SELF = "python3 tools/learn-student/drill_all.py"
DRILL = "target/release/examples/gen1_learn_drill"


def toon_table(name, cols, rows):
    print(f"{name}[{len(rows)}]{{{','.join(cols)}}}:")
    for r in rows:
        print("  " + ",".join(str(c) for c in r))


def toon_obj(name, pairs):
    print(f"{name}:")
    for k, v in pairs:
        print(f"  {k}: {v}")


def die(msg, helps, code=2):
    print(f"error: {msg}")
    for h in helps:
        print(f"help: {h}")
    sys.exit(code)


def plans(rom_path, out):
    """Every species' drill plan, and an honest account of the rest."""
    sys.path.insert(0, "tools/gen1-tables")
    import learn_table as lt

    g = lt.g
    rom = g.read_rom(rom_path)
    names, _ = g.species_names(rom)
    dex_of, _ = g.pokedex_order(rom)
    stats, stats_at = g.base_stats(rom, dex_of)
    mv, em = g.Moves(rom), g.EvosMoves(rom)
    real = [i for i in names if 1 <= dex_of.get(i, 0) <= 151]
    pdir = out / "plans"
    pdir.mkdir(parents=True, exist_ok=True)
    written, skipped, failed = [], [], []
    for i in real:
        buf = io.StringIO()
        try:
            with contextlib.redirect_stdout(buf):
                lt.emit_plan(rom, rom_path, em, mv, names, dex_of, stats,
                             stats_at, i, True)
            d = json.loads(buf.getvalue())
        except Exception as e:                       # noqa: BLE001
            # Named, never swallowed: a species the planner cannot instantiate
            # is a coverage fact and belongs in the report.
            failed.append((names[i], f"{type(e).__name__}: {e}"))
            continue
        decisions = [e for e in d["events"] if e["kind"] == "decision"]
        if not decisions:
            skipped.append(names[i])
            continue
        stem = "".join(c for c in names[i].lower() if c.isalnum())
        (pdir / f"{stem}.plan.json").write_text(buf.getvalue())
        written.append((names[i], stem, len(d["events"]), len(decisions),
                        sum(1 for e in decisions if e["answer"] == "decline")))
    return written, skipped, failed, real


def one(rom, out, stem):
    """Drill one line. Its own out and prompts directory, so it races nothing."""
    d = out / "lines" / stem
    d.mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    r = subprocess.run(
        [DRILL, "drill", "--rom", rom,
         "--plan", str(out / "plans" / f"{stem}.plan.json"),
         "--out", str(d), "--prompts", str(d / "prompts")],
        capture_output=True, text=True)
    (d / "drill.log").write_text(r.stdout + r.stderr)
    pool = sorted((d / "prompts").glob("*.gbss")) if (d / "prompts").is_dir() else []
    return stem, r.returncode, len(pool), time.time() - t0


def merge(out, stems):
    """One pool out of many, renumbered. The `NNNNN` contract is preserved."""
    pool = out / "prompts"
    pool.mkdir(parents=True, exist_ok=True)
    for old in pool.glob("*"):
        old.unlink()
    n = 0
    for stem in stems:
        d = out / "lines" / stem / "prompts"
        if not d.is_dir():
            continue
        for st in sorted(d.glob("*.gbss")):
            exp = st.with_suffix(".expect.json")
            if not exp.exists():
                continue
            (pool / f"{n:05d}.gbss").write_bytes(st.read_bytes())
            (pool / f"{n:05d}.expect.json").write_text(exp.read_text())
            n += 1
    return n, pool


def main():
    ap = argparse.ArgumentParser(add_help=True)
    ap.add_argument("--rom", default=os.environ.get("POKE_ROM"))
    ap.add_argument("--out", default="target/learn-all")
    ap.add_argument("--workers", type=int,
                    default=max(1, (os.cpu_count() or 8) - 4))
    a = ap.parse_args()
    if not a.rom:
        die("no cartridge", [f"{SELF} --rom <blue.gb>", "or set $POKE_ROM"])
    if not pathlib.Path(DRILL).exists():
        die(f"{DRILL} is not built", [
            "cargo build --release --features "
            "plugin-battle-control,plugin-trainer,plugin-agent "
            "--example gen1_learn_drill"])
    out = pathlib.Path(a.out)
    t0 = time.time()

    written, skipped, failed, real = plans(a.rom, out)
    stems = [w[1] for w in written]
    decisions = sum(w[3] for w in written)
    declines = sum(w[4] for w in written)
    toon_obj("plans", [
        ("species_in_cartridge", len(real)),
        ("lines_with_a_decision", len(written)),
        ("lines_with_no_decision", len(skipped)),
        ("lines_that_could_not_be_planned", len(failed)),
        ("decisions_to_drill", decisions),
        ("of_which_declines", declines),
    ])
    if failed:
        toon_table("could_not_be_planned", ["species", "why"], failed)
    print(f"drilling {len(stems)} line(s) on {a.workers} worker(s)...")

    done, results = 0, []
    with cf.ThreadPoolExecutor(max_workers=a.workers) as ex:
        futs = [ex.submit(one, a.rom, out, s) for s in stems]
        for f in cf.as_completed(futs):
            results.append(f.result())
            done += 1
            if done % 10 == 0 or done == len(stems):
                print(f"progress[1]{{done,total,elapsed_s}}:\n"
                      f"  {done},{len(stems)},{time.time() - t0:.0f}", flush=True)
    bad = [(s, rc) for s, rc, _, _ in results if rc != 0]
    pooled = sum(p for _, _, p, _ in results)
    n, pool = merge(out, stems)

    toon_obj("drilled", [
        ("lines", len(results)),
        ("lines_that_failed", len(bad)),
        ("prompts_written", pooled),
        ("pool", f"{pool} ({n} prompt(s))"),
        ("wall_s", f"{time.time() - t0:.1f}"),
        ("worker_s", f"{sum(t for _, _, _, t in results):.1f}"),
    ])
    if bad:
        toon_table("failed_lines", ["line", "exit"], bad[:20])
    json.dump({"written": written, "skipped": skipped, "failed": failed,
               "results": results},
              open(out / "index.json", "w"), indent=1)
    print("help[2]:")
    print(f"  python3 tools/learn-student/policy.py train "
          f"--corpus {out}/lines/<line>/<species>/agent.ndjson --out <model.json>")
    print(f"  cat {out}/lines/<line>/drill.log   # one line's own verdicts")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())

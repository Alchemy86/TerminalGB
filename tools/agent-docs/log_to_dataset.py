#!/usr/bin/env python3
"""Turn an agent run's decision log into a flat (observation, action, outcome)
dataset.

The log `--log` / `GB_AGENT_LOG` writes is already the dataset — one NDJSON
record per decision, carrying the snapshot the driver was handed *verbatim*,
its reply verbatim, the wall-clock latency and the reported usage. This script
does not add information; it flattens it into the shape a training or
evaluation pipeline expects, and joins each decision to the outcomes the
**next** snapshot reports for it.

    python3 tools/agent-docs/log_to_dataset.py target/run/agent.ndjson \\
        > dataset.jsonl

    python3 tools/agent-docs/log_to_dataset.py agent.ndjson --stats

One row per decision:

    {"decision": 2, "frame": 402, "latency_ms": 913,
     "observation": {...},            # the snapshot, minus the once-per-session help
     "action": {"goal": ..., "say": ..., "plan": [...]},
     "outcomes": [{"action": ..., "result": ..., "changed": true}],
     "interrupted": null,             # why the plan stopped early, if it did
     "usage": {"input_tokens": ..., "output_tokens": ..., "usd": ...},
     "frames_to_next": 24}

**`outcomes` is the label, and it is the honest one**: it is what the executor
observed the plan achieve, not what the driver hoped. `changed` is whether
anything in the world moved. `frames_to_next` is what the decision cost in
emulated time, which is the number to optimise — see
`docs/agent-environment.md` § "Frames per model call".

The last decision has no successor, so it has no outcomes and no
`frames_to_next`; it is emitted with nulls rather than dropped, because
silently losing the final row of every episode is how an evaluation ends up
measuring something other than what it meant to.

Stdlib only.
"""

import argparse
import json
import sys

HELP_FIELDS = ("actions_help", "screen_help")


def rows(records):
    for i, r in enumerate(records):
        try:
            obs = json.loads(r["request"])
        except (KeyError, json.JSONDecodeError):
            continue
        for k in HELP_FIELDS:
            obs.pop(k, None)
        try:
            reply = json.loads(r.get("reply") or "{}")
        except json.JSONDecodeError:
            # A driver that answered with something unparseable is a real and
            # useful row: the observation and the failure are both facts.
            reply = {}
        nxt = records[i + 1] if i + 1 < len(records) else None
        nobs = {}
        if nxt:
            try:
                nobs = json.loads(nxt["request"])
            except (KeyError, json.JSONDecodeError):
                nobs = {}
        yield {
            "decision": r.get("decision"),
            "frame": r.get("frame"),
            "latency_ms": r.get("ms"),
            "observation": obs,
            "action": {k: reply.get(k) for k in ("goal", "say", "plan")},
            "reply_parsed": bool(reply),
            "error": r.get("error"),
            "outcomes": nobs.get("last_actions"),
            "interrupted": nobs.get("interrupted"),
            "notes_next": nobs.get("notes"),
            "usage": r.get("usage"),
            "frames_to_next": (nobs.get("frame") - r["frame"])
            if nobs.get("frame") is not None and r.get("frame") is not None
            else None,
        }


def stats(out):
    n = len(out)
    spans = [r["frames_to_next"] for r in out if r["frames_to_next"] is not None]
    changed = sum(
        1 for r in out for o in (r["outcomes"] or []) if o.get("changed")
    )
    acts = sum(len(r["outcomes"] or []) for r in out)
    tok = sum((r["usage"] or {}).get("input_tokens", 0)
              + (r["usage"] or {}).get("output_tokens", 0) for r in out)
    usd = sum((r["usage"] or {}).get("usd", 0.0) for r in out)
    lat = [r["latency_ms"] for r in out if r["latency_ms"] is not None]
    print(f"decisions            {n}", file=sys.stderr)
    if spans:
        spans.sort()
        print(f"frames per decision  min {spans[0]}  median "
              f"{spans[len(spans)//2]}  max {spans[-1]}  "
              f"mean {sum(spans)/len(spans):.0f}", file=sys.stderr)
        print(f"emulated frames      {sum(spans)}  "
              f"({sum(spans)/59.7275:.1f} s of game)", file=sys.stderr)
    if lat:
        print(f"driver latency ms    mean {sum(lat)/len(lat):.0f}  "
              f"max {max(lat)}", file=sys.stderr)
    print(f"actions executed     {acts}, of which {changed} changed the world",
          file=sys.stderr)
    print(f"reported usage       {tok} tokens, ${usd:.4f}", file=sys.stderr)
    unparsed = sum(1 for r in out if not r["reply_parsed"])
    if unparsed:
        print(f"unreadable replies   {unparsed}", file=sys.stderr)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("log", help="the NDJSON written by --log / GB_AGENT_LOG")
    ap.add_argument("--stats", action="store_true",
                    help="print a summary to stderr as well")
    a = ap.parse_args()

    with open(a.log) as f:
        records = [json.loads(l) for l in f if l.strip()]
    out = list(rows(records))
    for r in out:
        print(json.dumps(r, ensure_ascii=False))
    if a.stats:
        stats(out)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Agent adapter that drives the game with the local `claude` CLI.

This is the *convenient* adapter: it uses whatever Claude Code login the
machine already has, so it needs no API key. `claude_api_adapter.py` is the one
to use for a real unattended broadcast — it is cheaper per call and its latency
is a fraction of this one's.

Contract (`gbconform`-style: tiny, and nothing parsed from stdout but the
answer) — see `docs/claude-plays.md`:

    stdin   one line of JSON: the state snapshot
    stdout  one JSON object:  {"goal":…, "say":…, "plan":[…], "usage":{…}}
    stderr  diagnostics; the host logs them and NEVER shows them to viewers
    exit 0  success

Environment:
    GB_AGENT_MODEL     model alias (default "haiku")
    GB_AGENT_EFFORT    reasoning effort (default "medium"; "low" is not cheaper
                       here — it thinks longer about how little to think)
    GB_AGENT_FALLBACK  model to fall back to when the first is unavailable
    GB_AGENT_TIMEOUT   seconds per attempt (default 60)
    GB_AGENT_RETRIES   attempts per decision (default 2)

Every outbound call has a strict timeout, a retry cap and a cheaper designated
fallback, and the host has its own timeout and budget on top. Nothing here can
loop: `RETRIES` attempts, then a non-zero exit, which the host counts against
its circuit breaker.
"""

import json
import os
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT = os.path.join(HERE, "prompt.md")

MODEL = os.environ.get("GB_AGENT_MODEL", "haiku")
EFFORT = os.environ.get("GB_AGENT_EFFORT", "medium")
FALLBACK = os.environ.get("GB_AGENT_FALLBACK", "")
# Two attempts at 60 s must still fit inside the host's own per-call timeout
# (150 s by default), or the host kills the adapter mid-retry and the retry is
# wasted. Keep `RETRIES * TIMEOUT` below it.
TIMEOUT = float(os.environ.get("GB_AGENT_TIMEOUT", "60"))
RETRIES = max(1, int(os.environ.get("GB_AGENT_RETRIES", "2")))

# Claude Code would otherwise happily go and read the repository. It is being
# asked to play a game, and a tool call is a wasted ten seconds.
NO_TOOLS = (
    "Bash Read Write Edit NotebookEdit Glob Grep WebFetch WebSearch Task "
    "Agent Skill TodoWrite"
)

# Run the CLI from an EMPTY directory, not from the repository.
#
# Claude Code auto-discovers `CLAUDE.md` from the working directory, and this
# project's is 104 KB of agent memory. Measured: $0.088 per call from the repo
# against $0.005 from an empty directory — a seventeen-fold difference, for
# context that has nothing to do with playing Pokémon. A *fixed* directory
# (rather than a fresh temp one each call) keeps the prompt prefix identical so
# the prompt cache keeps hitting.
NEUTRAL_CWD = os.path.join(tempfile.gettempdir(), "gb-claude-plays-cwd")


def log(msg):
    print(msg, file=sys.stderr)


def extract_object(text):
    """Pull the first balanced JSON object out of a reply.

    Models wrap JSON in prose and fences however firmly they are told not to.
    The host is lenient about this too; doing it here as well means the usage
    numbers can be merged into the object before it is handed over.
    """
    depth = 0
    start = None
    in_str = False
    esc = False
    for i, c in enumerate(text):
        if in_str:
            if esc:
                esc = False
            elif c == "\\":
                esc = True
            elif c == '"':
                in_str = False
            continue
        if c == '"':
            in_str = True
        elif c == "{":
            if depth == 0:
                start = i
            depth += 1
        elif c == "}":
            if depth:
                depth -= 1
                if depth == 0:
                    try:
                        return json.loads(text[start : i + 1])
                    except json.JSONDecodeError:
                        start = None
    return None


def call(model, snapshot, system):
    cmd = [
        "claude",
        "-p",
        "--model",
        model,
        "--effort",
        EFFORT,
        "--output-format",
        "json",
        # Every call is a fresh CLI session, so the ~17k-token system prompt is
        # re-sent each time; the only thing that makes that affordable is the
        # prompt cache, and the cache only hits if the prefix is byte-identical.
        # Claude Code's default system prompt embeds the working directory, the
        # environment and **the git status**, so on a repository anyone is
        # editing the prefix changes constantly and every call is a cache MISS.
        # Measured: $0.081/call without this flag, $0.005 with it.
        "--exclude-dynamic-system-prompt-sections",
        "--disallowed-tools",
        NO_TOOLS,
        "--append-system-prompt",
        system,
        "Game state:\n" + snapshot + "\n\nReply with the JSON object only.",
    ]
    os.makedirs(NEUTRAL_CWD, exist_ok=True)
    started = time.time()
    proc = subprocess.run(
        cmd, capture_output=True, text=True, timeout=TIMEOUT, cwd=NEUTRAL_CWD
    )
    took = time.time() - started
    if proc.returncode != 0:
        raise RuntimeError(
            f"claude exited {proc.returncode}: {proc.stderr.strip()[:400]}"
        )
    envelope = json.loads(proc.stdout)
    reply = extract_object(envelope.get("result", "") or "")
    if reply is None:
        raise RuntimeError("no JSON object in the model's reply")
    u = envelope.get("usage", {}) or {}
    reply["usage"] = {
        "input_tokens": int(u.get("input_tokens", 0))
        + int(u.get("cache_creation_input_tokens", 0))
        + int(u.get("cache_read_input_tokens", 0)),
        "output_tokens": int(u.get("output_tokens", 0)),
        "usd": float(envelope.get("total_cost_usd", 0.0) or 0.0),
    }
    log(f"{model} answered in {took:.1f}s, ${reply['usage']['usd']:.4f}")
    return reply


def main():
    snapshot = sys.stdin.read().strip()
    if not snapshot:
        log("no state on stdin")
        return 2
    try:
        with open(PROMPT, encoding="utf-8") as f:
            system = f.read()
    except OSError as e:
        log(f"cannot read the prompt: {e}")
        return 2

    models = [MODEL] + ([FALLBACK] if FALLBACK else [])
    last = "no attempt made"
    for model in models:
        for attempt in range(1, RETRIES + 1):
            try:
                print(json.dumps(call(model, snapshot, system)))
                return 0
            except subprocess.TimeoutExpired:
                last = f"{model} timed out after {TIMEOUT:.0f}s"
            except Exception as e:  # noqa: BLE001 - the message is for the log
                last = f"{model} attempt {attempt}: {e}"
            log(last)
    log(f"giving up: {last}")
    return 1


if __name__ == "__main__":
    sys.exit(main())

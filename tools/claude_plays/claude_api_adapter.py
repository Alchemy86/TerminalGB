#!/usr/bin/env python3
"""Agent adapter that calls the Anthropic Messages API directly.

The adapter to use for a real broadcast: one HTTPS request per decision, so it
is several times cheaper and several times faster than driving the `claude` CLI
(which loads a whole coding-agent system prompt every call).

Python standard library only — `urllib`, no `requests`, no SDK — for the same
reason the rest of this repository hand-rolls its codecs: an adapter with
dependencies is an adapter people do not run.

    stdin   one line of JSON: the state snapshot
    stdout  one JSON object:  {"goal":…, "say":…, "plan":[…], "usage":{…}}
    stderr  diagnostics; logged by the host, never shown to viewers
    exit 0  success

Environment:
    ANTHROPIC_API_KEY  required
    GB_AGENT_MODEL     default claude-haiku-4-5-20251001
    GB_AGENT_FALLBACK  cheaper/available model to fall back to
    GB_AGENT_TIMEOUT   seconds per attempt (default 45)
    GB_AGENT_RETRIES   attempts per model (default 2)
    GB_AGENT_MAX_TOKENS  output cap (default 700)

Safety properties this file is responsible for, per the project's guidance on
external APIs: a strict timeout on every request, a hard retry cap, a cheaper
designated fallback, and no unbounded loop anywhere. The **budget** cap and the
circuit breaker live in the emulator host, not here, so that they cannot be
bypassed by editing this file.
"""

import json
import os
import sys
import time
import urllib.error
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
PROMPT = os.path.join(HERE, "prompt.md")
URL = "https://api.anthropic.com/v1/messages"
VERSION = "2023-06-01"

MODEL = os.environ.get("GB_AGENT_MODEL", "claude-haiku-4-5-20251001")
FALLBACK = os.environ.get("GB_AGENT_FALLBACK", "")
TIMEOUT = float(os.environ.get("GB_AGENT_TIMEOUT", "45"))
RETRIES = max(1, int(os.environ.get("GB_AGENT_RETRIES", "2")))
MAX_TOKENS = int(os.environ.get("GB_AGENT_MAX_TOKENS", "700"))

# Published per-million-token prices, used only to report an approximate cost
# back to the host. The host's own wall-clock and call ceilings are what
# actually bound spend; this number is a courtesy, and it is labelled as
# approximate wherever it is shown.
PRICES = {
    "haiku": (1.0, 5.0),
    "sonnet": (3.0, 15.0),
    "opus": (15.0, 75.0),
}


def log(msg):
    print(msg, file=sys.stderr)


def price_for(model):
    for key, p in PRICES.items():
        if key in model:
            return p
    return (0.0, 0.0)


def extract_object(text):
    """First balanced JSON object in the reply; models add prose and fences."""
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


def call(model, snapshot, system, key):
    body = {
        "model": model,
        "max_tokens": MAX_TOKENS,
        # The instructions never change, so caching them turns most of the
        # prompt into a cache read after the first call.
        "system": [
            {
                "type": "text",
                "text": system,
                "cache_control": {"type": "ephemeral"},
            }
        ],
        "messages": [
            {
                "role": "user",
                "content": "Game state:\n"
                + snapshot
                + "\n\nReply with the JSON object only.",
            }
        ],
    }
    req = urllib.request.Request(
        URL,
        data=json.dumps(body).encode("utf-8"),
        headers={
            "content-type": "application/json",
            "x-api-key": key,
            "anthropic-version": VERSION,
        },
    )
    started = time.time()
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        payload = json.loads(resp.read().decode("utf-8"))
    took = time.time() - started

    text = "".join(
        b.get("text", "") for b in payload.get("content", []) if b.get("type") == "text"
    )
    reply = extract_object(text)
    if reply is None:
        raise RuntimeError("no JSON object in the model's reply")

    u = payload.get("usage", {}) or {}
    inp = (
        int(u.get("input_tokens", 0))
        + int(u.get("cache_creation_input_tokens", 0))
        + int(u.get("cache_read_input_tokens", 0))
    )
    out = int(u.get("output_tokens", 0))
    pin, pout = price_for(model)
    reply["usage"] = {
        "input_tokens": inp,
        "output_tokens": out,
        "usd": round(inp * pin / 1e6 + out * pout / 1e6, 6),
    }
    log(f"{model} answered in {took:.1f}s, {inp}+{out} tokens")
    return reply


def main():
    key = os.environ.get("ANTHROPIC_API_KEY", "").strip()
    if not key:
        log("ANTHROPIC_API_KEY is not set")
        return 2
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

    last = "no attempt made"
    for model in [MODEL] + ([FALLBACK] if FALLBACK else []):
        for attempt in range(1, RETRIES + 1):
            try:
                print(json.dumps(call(model, snapshot, system, key)))
                return 0
            except urllib.error.HTTPError as e:
                detail = e.read().decode("utf-8", "replace")[:300]
                last = f"{model} HTTP {e.code}: {detail}"
                # 4xx that is not a rate limit will not fix itself.
                if e.code not in (408, 409, 429) and e.code < 500:
                    log(last)
                    break
            except Exception as e:  # noqa: BLE001 - the message is for the log
                last = f"{model} attempt {attempt}: {e}"
            log(last)
            time.sleep(min(2.0 * attempt, 5.0))
    log(f"giving up: {last}")
    return 1


if __name__ == "__main__":
    sys.exit(main())

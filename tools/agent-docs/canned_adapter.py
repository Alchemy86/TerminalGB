#!/usr/bin/env python3
"""A driver that plays a fixed hand, for screenshots and for reading.

Same contract as any other adapter — snapshot on stdin, one JSON object on
stdout, exit 0 — but every reply is canned, so a run is deterministic and free.
This is what produced `docs/shots/agent-panel.png`, and it is a working
skeleton for a real adapter: replace `decide()` with a model call and nothing
else changes.

    GB_PLUGINS=agent GB_AGENT_CMD="python3 tools/agent-docs/canned_adapter.py" \\
      cargo run --release --example terminal -- <rom>

Stdlib only, and it reads the snapshot properly rather than ignoring it, so the
field names below are a live statement of what a driver actually receives.
"""

import json
import sys

# The reply the host wants back. Only `plan` is load-bearing; `goal` and `say`
# are what a viewer reads, and `usage` is what the governor counts.
PLANS = [
    ("Leave the Pokemon Center",
     "There is a pair of doors at the bottom of this room. Heading for the left one.",
     [{"do": "goto", "x": 3, "y": 7}]),
    ("Leave the Pokemon Center",
     "Standing on the doormat now. A door is not opened with A, so one more step down.",
     [{"do": "walk", "dir": "down", "steps": 1}, {"do": "wait", "frames": 60}]),
    ("Explore Viridian City",
     "Outside. Walking north to see what is up there.",
     [{"do": "walk", "dir": "up", "steps": 6}]),
]


def decide(snapshot):
    """Everything a real adapter would send to a model is in `snapshot`.

    The branches below are the whole of `screen.kind`, which is exactly why
    that field exists: it answers *what does A do here* without anyone having
    to read the text and guess.
    """
    n = int(snapshot.get("decision", 1))
    screen = snapshot.get("screen", {})
    kind = screen.get("kind")
    mode = snapshot.get("mode")
    usage = {"usage": {"input_tokens": 1400 + 60 * n, "output_tokens": 80,
                       "usd": 0.004}}

    # A withheld decode is a fact, not a failed read: the honest response is to
    # let a few frames pass rather than to guess at an empty string.
    if screen.get("trust") not in (None, "ok"):
        return {"say": "The screen is mid-transition, so I will wait and look again.",
                "plan": [{"do": "wait", "frames": 45}], **usage}
    if kind == "naming-keyboard":
        # A types another letter, for ever. START is the way off this screen.
        return {"say": "That name will do. Pressing START to finish it.",
                "plan": [{"do": "press", "button": "start"}], **usage}
    if kind == "dialogue":
        return {"say": "Reading through what it has to say.",
                "plan": [{"do": "advance_text"}], **usage}
    if kind in ("menu", "yes-no"):
        return {"goal": "Get into the game",
                "say": f"Choosing {screen.get('selected') or 'the highlighted option'}.",
                "plan": [{"do": "press", "button": "a"}], **usage}
    if mode in ("title", "cutscene", "transition"):
        return {"goal": "Get into the game",
                "say": "Still in the opening. Pressing on.",
                "plan": [{"do": "press", "button": "a", "times": 4}], **usage}

    goal, say, plan = PLANS[(n - 1) % len(PLANS)]
    return {"goal": goal, "say": say, "plan": plan, **usage}


def main():
    raw = sys.stdin.read()
    try:
        snapshot = json.loads(raw)
    except json.JSONDecodeError:
        # Anything but exit 0 is a failed call, and four in a row open the
        # host's circuit breaker. Say why on stderr: it reaches the log file
        # and never the panel a viewer sees.
        print("adapter: could not parse the snapshot", file=sys.stderr)
        return 1
    json.dump(decide(snapshot), sys.stdout)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    sys.exit(main())

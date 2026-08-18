"""AXI: TOON on stdout, errors on stdout, help lines.

Moved here verbatim from path_arrows.py so there is ONE of it.
See chainrender/__init__.py for why this package exists.
"""

import json as _json
import sys

# The tool sets this to its own invocation, for `help` lines.
SELF = ""

# --- AXI: TOON on stdout, errors on stdout, help lines -----------------------
# An agent drives this through a shell and does not read stderr, so an error
# sent there is an error it cannot act on. Exit codes: 0 success, 1 the work
# failed, 2 the command line was wrong.
_JSON = False

_BLOCKS = []


def set_json(on):
    """Switch every block below to one JSON object flushed at the end."""
    global _JSON
    _JSON = bool(on)


def _cell(v):
    if v is None:
        return "-"
    if isinstance(v, float):
        return f"{v:.4g}"
    return str(v)


def table(name, rows, fields):
    if _JSON:
        _BLOCKS.append((name, [{f: r.get(f) for f in fields} for r in rows]))
        return
    print(f"{name}[{len(rows)}]{{{','.join(fields)}}}:")
    for r in rows:
        print("  " + ",".join(_cell(r.get(f)) for f in fields))


def obj(name, pairs):
    if _JSON:
        _BLOCKS.append((name, dict(pairs)))
        return
    print(f"{name}:")
    for k, v in pairs:
        print(f"  {k}: {v}")


def note(text):
    if _JSON:
        _BLOCKS.append(("note", text))
        return
    print(text)


def helps(lines):
    if _JSON or not lines:
        return
    print(f"help[{len(lines)}]:")
    for line in lines:
        print(f"  {line}")


def flush_json():
    if not _JSON:
        return
    out = {}
    for name, payload in _BLOCKS:
        if name in out and isinstance(out[name], list):
            out[name].extend(payload)
        else:
            out[name] = payload
    print(_json.dumps(out, indent=2))


def die(message, *suggestions, code=1):
    if _JSON:
        print(_json.dumps({"error": message, "help": list(suggestions)}, indent=2))
    else:
        print(f"error: {message}")
        for s in suggestions:
            print(f"help: {s}")
    sys.exit(code)

#!/usr/bin/env python3
"""Regenerate every worked example in `docs/agent-reference.md` by running the
real thing.

Nothing under `docs/agent/examples/` is hand-written. This script drives the
shipped `terminalgb-mcp` binary over its real JSON-RPC transport, the shipped
`claude_plays` example over the real agent plugin, and the shipped
`screen_text` example over the real observation layer, and writes what came
back. If a payload in the reference page and a payload here disagree, the page
is wrong.

    python3 tools/agent-docs/capture.py \
        --rom ~/roms/blue.gb --save ~/roms/blue.sav

The cartridge is **not** in this repository and never will be. Point `--rom` at
your own Pokemon Red/Blue (English); the ROM and its save are copied into
`target/agentdocs/` first, because the MBC writes a `.sav` next to the ROM path
and a tool must never write beside somebody's cartridge.

Without a Pokemon cartridge you can still regenerate the cartridge-agnostic
half against the vendored MIT test ROM:

    python3 tools/agent-docs/capture.py --rom third_party/dmg-acid2/dmg-acid2.gb

Stdlib only, like `docs/tools/`: an adopting project must not inherit a
dependency to read its own documentation.
"""

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
STAGE = ROOT / "target" / "agentdocs"
MCP = ROOT / "target" / "release" / "terminalgb-mcp"
PLAYS = ROOT / "target" / "release" / "examples" / "claude_plays"
SCREEN = ROOT / "target" / "release" / "examples" / "screen_text"

BUILD_HINT = """\
build the binaries first:
  cargo build --release --no-default-features --features mcp
  cargo build --release --features image,plugin-agent --example claude_plays
  cargo build --release --features plugin-agent --example screen_text"""

# The request sequence. Every id below is quoted in docs/agent-reference.md, so
# renumbering here means editing the page.
REQUESTS = [
    {"jsonrpc": "2.0", "id": 1, "method": "initialize",
     "params": {"protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "docs-capture", "version": "1"}}},
    {"jsonrpc": "2.0", "method": "notifications/initialized"},
    {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    {"jsonrpc": "2.0", "id": 3, "method": "tools/call",
     "params": {"name": "step", "arguments": {"frames": 2400}}},
    {"jsonrpc": "2.0", "id": 4, "method": "tools/call",
     "params": {"name": "press_buttons", "arguments": {"buttons": ["start"]}}},
    {"jsonrpc": "2.0", "id": 5, "method": "tools/call",
     "params": {"name": "step", "arguments": {"frames": 300}}},
    # Deliberately observed mid-intro: this is the `graphics-not-text` case.
    {"jsonrpc": "2.0", "id": 6, "method": "tools/call",
     "params": {"name": "observe", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 7, "method": "tools/call",
     "params": {"name": "press_buttons",
                "arguments": {"buttons": ["a"], "times": 6, "gap_frames": 40}}},
    {"jsonrpc": "2.0", "id": 8, "method": "tools/call",
     "params": {"name": "step", "arguments": {"frames": 400}}},
    {"jsonrpc": "2.0", "id": 9, "method": "tools/call",
     "params": {"name": "observe", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 10, "method": "tools/call",
     "params": {"name": "read_memory", "arguments": {"address": 54110, "length": 5}}},
    {"jsonrpc": "2.0", "id": 11, "method": "tools/call",
     "params": {"name": "save_state", "arguments": {"slot": 9}}},
    {"jsonrpc": "2.0", "id": 12, "method": "tools/call",
     "params": {"name": "run_actions",
                "arguments": {"actions": [{"do": "walk", "dir": "down", "steps": 1}]}}},
    {"jsonrpc": "2.0", "id": 13, "method": "tools/call",
     "params": {"name": "observe", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 14, "method": "tools/call",
     "params": {"name": "run_actions",
                "arguments": {"actions": [{"do": "goto", "x": 3, "y": 7}]}}},
    {"jsonrpc": "2.0", "id": 15, "method": "tools/call",
     "params": {"name": "observe", "arguments": {}}},
    {"jsonrpc": "2.0", "id": 16, "method": "tools/call",
     "params": {"name": "screenshot", "arguments": {"scale": 2}}},
    {"jsonrpc": "2.0", "id": 17, "method": "tools/call",
     "params": {"name": "load_state", "arguments": {"slot": 9}}},
    {"jsonrpc": "2.0", "id": 18, "method": "tools/call",
     "params": {"name": "observe", "arguments": {}}},
    # The frame cap, reported rather than silently applied.
    {"jsonrpc": "2.0", "id": 19, "method": "tools/call",
     "params": {"name": "step", "arguments": {"frames": 999999}}},
    # Absent, not refused-when-called: no --allow-write-memory here.
    {"jsonrpc": "2.0", "id": 20, "method": "tools/call",
     "params": {"name": "write_memory", "arguments": {"address": 54110, "bytes": [1]}}},
    # A verb the executor does not know: an error the caller can learn from.
    {"jsonrpc": "2.0", "id": 21, "method": "tools/call",
     "params": {"name": "run_actions",
                "arguments": {"actions": [{"do": "teleport", "x": 1, "y": 1}]}}},
    {"jsonrpc": "2.0", "id": 22, "method": "tools/call",
     "params": {"name": "render_audio",
                "arguments": {"score": "tempo 120\nch1 duty 2\nch1 env 15 - 3\n"
                                       "ch1 note C4 1/8\nch1 note E4 1/8\n"
                                       "ch1 note G4 1/4",
                              "registers": True}}},
]

# The scripted driver's replies, in the format an adapter must produce. Three
# decisions: leave the room, step through the door, keep going.
SCRIPT = [
    {"goal": "Leave the Pokemon Center",
     "say": "I am in a Pokemon Center. The way out is the pair of doors at the "
            "bottom of the room, so I will head for one of them.",
     "plan": [{"do": "goto", "x": 3, "y": 7}],
     "usage": {"input_tokens": 1420, "output_tokens": 96, "usd": 0.0043}},
    {"goal": "Leave the Pokemon Center",
     "say": "I am standing on the doormat. A door is not opened with A, so I "
            "will take one more step down to walk through it.",
     "plan": [{"do": "walk", "dir": "down", "steps": 1}, {"do": "wait", "frames": 60}],
     "usage": {"input_tokens": 1533, "output_tokens": 88, "usd": 0.0046}},
    {"goal": "Explore Viridian City",
     "say": "Outside now. I will walk north and see what is there.",
     "plan": [{"do": "walk", "dir": "up", "steps": 4}],
     "usage": {"input_tokens": 1610, "output_tokens": 74, "usd": 0.0048}},
]

HEADER = ("Generated by `tools/agent-docs/capture.py` — do not edit by hand.\n"
          "Every byte below came back from the real binary over its real transport.\n")


def need(path, what):
    if not path.exists():
        sys.exit(f"missing {what} at {path}\n{BUILD_HINT}")


def stage(rom, save):
    STAGE.mkdir(parents=True, exist_ok=True)
    cart = STAGE / "cart.gb"
    shutil.copy(rom, cart)
    sav = cart.with_suffix(".sav")
    if sav.exists():
        sav.unlink()
    if save:
        shutil.copy(save, sav)
    return cart


def run_mcp(cart, out):
    """Drive the real server over stdio and write both the raw and the
    readable transcript."""
    body = "".join(json.dumps(r) + "\n" for r in REQUESTS)
    states = STAGE / "states"
    proc = subprocess.run(
        [str(MCP), "--rom", str(cart), "--state-dir", str(states)],
        input=body, capture_output=True, text=True, timeout=600)
    lines = [l for l in proc.stdout.splitlines() if l.strip()]
    (out / "mcp-transcript.jsonl").write_text(unhost(
        "".join(json.dumps(trim_blobs(json.loads(l)), ensure_ascii=False) + "\n"
                for l in lines)))

    replies = {}
    for l in lines:
        d = json.loads(l)
        replies[d.get("id")] = d

    md = ["# MCP transcript — every tool, request and reply", "",
          HEADER.replace("\n", "\n"), ""]
    for req in REQUESTS:
        rid = req.get("id")
        if rid is None:
            md += ["### notification — `notifications/initialized`", "",
                   "```json", json.dumps(req), "```", "",
                   "*No reply. A JSON-RPC notification has no `id`, and answering "
                   "one is a protocol error some clients treat as fatal.*", ""]
            continue
        name = req["params"].get("name", req["method"]) if req.get("params") else req["method"]
        md += [f"### {rid} — `{name}`", "", "**request**", "",
               "```json", json.dumps(req, ensure_ascii=False), "```", ""]
        if req["method"] == "tools/list":
            # A thousand lines of schema in the middle of a transcript hides
            # everything after it. It gets its own file.
            (out / "tools-list.json").write_text(
                json.dumps(replies.get(rid, {}), ensure_ascii=False, indent=2) + "\n")
            names = [t["name"] for t in
                     replies.get(rid, {}).get("result", {}).get("tools", [])]
            md += [f"**reply** — {len(names)} tools: "
                   + ", ".join(f"`{n}`" for n in names)
                   + ". The whole reply, every `inputSchema` in full, is "
                     "[`tools-list.json`](tools-list.json).", ""]
            continue
        md += ["**reply**", "", "```json",
               redact(json.dumps(replies.get(rid, {}), ensure_ascii=False, indent=2)),
               "```", ""]
        text = human(replies.get(rid))
        if text:
            md += ["**the text block, unescaped**", "", "```", text, "```", ""]
    (out / "mcp-transcript.md").write_text(unhost("\n".join(md)))
    return replies


TRUNC = "...TRUNCATED-FOR-THE-DOCS"


def unhost(s):
    """Replace this machine's absolute paths with repository-relative ones.

    A tool that reports where it wrote a file reports an absolute path, and
    these captures are committed — so without this the docs would carry
    somebody's home directory, their checkout name and, in a worktree pool,
    their branch. That has bitten this repository before (a superseded README
    upload whose status bar read `/home/<user>/Videos/...` is still live on
    GitHub's CDN). Do the substitution here, once, rather than trusting every
    future reader of a diff to notice.
    """
    return s.replace(str(ROOT) + "/", "").replace(str(pathlib.Path.home()), "~")


def trim_blobs(v):
    """Shorten every long `data` string in a decoded reply, in place."""
    if isinstance(v, dict):
        for k, x in v.items():
            if k == "data" and isinstance(x, str) and len(x) > 200:
                v[k] = x[:48] + TRUNC
            else:
                trim_blobs(x)
    elif isinstance(v, list):
        for x in v:
            trim_blobs(x)
    return v


def redact(s):
    """The same, on already-pretty-printed JSON, where the value is alone on
    its line. A screenshot's base64 is a whole frame of somebody's cartridge:
    keep enough to prove it really is a PNG and no more — the docs illustrate
    our wire format, they do not redistribute a game.

    The result must stay valid JSON so a reader can paste it somewhere.
    """
    out = []
    for line in s.splitlines():
        head, sep, rest = line.partition('"data": "')
        if sep and len(rest) > 200:
            tail = "," if rest.rstrip().endswith(",") else ""
            out.append(f'{head}"data": "{rest[:48]}{TRUNC}"{tail}')
        else:
            out.append(line)
    return "\n".join(out)


def human(reply):
    if not reply:
        return ""
    parts = []
    for c in reply.get("result", {}).get("content", []):
        if c.get("type") == "text":
            parts.append(c["text"])
    return "\n".join(parts)


def run_agent(cart, out):
    """Drive the agent plugin with the deterministic scripted driver and keep
    the decision log — the requests in it are the snapshots verbatim."""
    run = STAGE / "run"
    run.mkdir(parents=True, exist_ok=True)
    script = run / "script.ndjson"
    script.write_text("".join(json.dumps(r) + "\n" for r in SCRIPT))
    log = run / "agent.ndjson"
    state = STAGE / "states" / "POKEMON_BLUE.slot9.gbss"
    cmd = [str(PLAYS), "--rom", str(cart), "--driver", "scripted",
           "--script", str(script), "--log", str(log), "--frames", "3000",
           "--report"]
    if state.exists():
        cmd += ["--state", str(state)]
    proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    (out / "agent-run.txt").write_text(unhost(
        "$ " + " ".join(cmd) + "\n\n" + proc.stdout))

    records = [json.loads(l) for l in log.read_text().splitlines() if l.strip()]
    md = ["# Agent snapshots — what a driver is actually handed", "", HEADER, ""]
    for r in records:
        snap = json.loads(r["request"])
        helps = {k: snap.pop(k) for k in ("actions_help", "screen_help") if k in snap}
        md += [f"## decision {r['decision']} (frame {r['frame']})", "",
               "**snapshot handed to the driver**", "", "```json",
               json.dumps(snap, ensure_ascii=False, indent=2), "```", ""]
        if helps:
            md += ["*Plus `actions_help` and `screen_help`, sent **once per session** "
                   "on the first snapshot only — see below.*", ""]
        md += ["**the driver's reply**", "", "```json",
               json.dumps(json.loads(r["reply"]), ensure_ascii=False, indent=2),
               "```", ""]
    first = json.loads(records[0]["request"]) if records else {}
    for k in ("actions_help", "screen_help"):
        if k in first:
            md += [f"## `{k}` — sent once, on the first snapshot", "",
                   "```", first[k], "```", ""]
    (out / "agent-snapshots.md").write_text(unhost("\n".join(md)))


def run_screens(cart, save, out):
    """The observation layer's own evidence tool, on a real cartridge."""
    text = []
    for flag in ("--tour", "--menu-tour"):
        cmd = [str(SCREEN), "--rom", str(cart)]
        if save and flag == "--menu-tour":
            cmd += ["--save", str(save)]
        cmd.append(flag)
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=900)
        text.append(f"$ screen_text --rom <blue.gb> {flag}\n\n{proc.stdout}")
    (out / "screen-tour.txt").write_text(unhost(HEADER + "\n" + "\n".join(text)))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--rom", required=True)
    ap.add_argument("--save")
    ap.add_argument("--out", default=str(ROOT / "docs" / "agent" / "examples"))
    a = ap.parse_args()

    need(MCP, "terminalgb-mcp")
    need(PLAYS, "the claude_plays example")
    need(SCREEN, "the screen_text example")
    out = pathlib.Path(a.out)
    out.mkdir(parents=True, exist_ok=True)

    cart = stage(pathlib.Path(a.rom).expanduser(),
                 pathlib.Path(a.save).expanduser() if a.save else None)
    print(f"staged {cart}")
    run_mcp(cart, out)
    print("wrote mcp-transcript.{jsonl,md}")
    run_agent(cart, out)
    print("wrote agent-snapshots.md, agent-run.txt")
    run_screens(cart, cart.with_suffix(".sav") if a.save else None, out)
    print("wrote screen-tour.txt")


if __name__ == "__main__":
    main()

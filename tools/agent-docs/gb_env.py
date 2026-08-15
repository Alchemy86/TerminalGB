#!/usr/bin/env python3
"""A minimal environment wrapper over `terminalgb-mcp`, in stdlib Python.

This is the concrete answer to *"how do I drive this from my own code?"*. The
MCP server speaks newline-delimited JSON-RPC on stdin/stdout, so a client is a
subprocess and two functions. There is no library to install and nothing to
build beyond the server itself.

    from gb_env import GameBoyEnv
    with GameBoyEnv("target/release/terminalgb-mcp", "blue.gb") as env:
        obs = env.reset(state="episode-start.gbss")     # or slot=1
        obs = env.act([{"do": "goto", "x": 3, "y": 7}])
        print(obs["position"], env.last_outcomes)

Run it directly for a self-test against any cartridge:

    python3 tools/agent-docs/gb_env.py --rom third_party/dmg-acid2/dmg-acid2.gb

**What this is not.** It is not a Gymnasium `Env`: there are no `spaces`, no
`reward` and no `terminated` — because a reward function for a Game Boy game is
a *design decision belonging to whoever is training*, and shipping a default one
would quietly become the benchmark. `reward_from()` below is a stub with an
argument, not an implementation. See `docs/agent-environment.md`.

Reset semantics, which are the interesting part: `reset()` restores a save
state, so an episode can start from any point you can reach and get to — the
title screen, the door of a gym, mid-battle. The restore is exact
(`testharness/savestate.rs` proves save/load at every frame boundary stays
byte-identical), so two runs from the same state with the same actions produce
the same frames.
"""

import json
import subprocess
import sys


class MCPError(RuntimeError):
    pass


class GameBoyEnv:
    """One emulator, one cartridge, one process."""

    def __init__(self, server, rom, state_dir=None, extra_args=()):
        args = [server, "--rom", rom, *extra_args]
        if state_dir:
            args += ["--state-dir", state_dir]
        self.proc = subprocess.Popen(
            args, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1)
        self._id = 0
        self.last_outcomes = ""
        self._call("initialize", {"protocolVersion": "2024-11-05",
                                  "capabilities": {}, "clientInfo":
                                  {"name": "gb_env", "version": "1"}})
        self._notify("notifications/initialized")

    # -- transport ---------------------------------------------------------

    def _send(self, obj):
        self.proc.stdin.write(json.dumps(obj) + "\n")
        self.proc.stdin.flush()

    def _notify(self, method, params=None):
        # A JSON-RPC notification has no `id` and gets no reply. Waiting for
        # one here would hang the client for ever.
        self._send({"jsonrpc": "2.0", "method": method, "params": params or {}})

    def _call(self, method, params=None):
        self._id += 1
        self._send({"jsonrpc": "2.0", "id": self._id, "method": method,
                    "params": params or {}})
        line = self.proc.stdout.readline()
        if not line:
            raise MCPError("the server closed its output")
        reply = json.loads(line)
        if "error" in reply:
            raise MCPError(reply["error"].get("message", "unknown error"))
        return reply.get("result", {})

    def tool(self, name, **arguments):
        """Call any tool. Returns its text, and raises on `isError` — the
        server reports a tool that ran-and-failed as a *successful* call
        carrying that flag, per the MCP spec."""
        r = self._call("tools/call", {"name": name, "arguments": arguments})
        text = "\n".join(c["text"] for c in r.get("content", [])
                         if c.get("type") == "text")
        if r.get("isError"):
            raise MCPError(text)
        return text

    # -- the environment ---------------------------------------------------

    def observe(self):
        """The structured state, as a dict. A few hundred bytes."""
        return json.loads(self.tool("observe"))

    def reset(self, slot=None, state=None, frames=0):
        """Start an episode. With neither `slot` nor `state`, reboots the
        cartridge from cold and optionally runs `frames` of boot."""
        if state:
            self.tool("load_state", path=state)
        elif slot is not None:
            self.tool("load_state", slot=slot)
        else:
            self.tool("reset")
            if frames:
                self.tool("step", frames=frames)
        self.last_outcomes = ""
        return self.observe()

    def snapshot(self, path):
        """Write a save state — the reset point for later episodes."""
        return self.tool("save_state", path=path)

    def act(self, actions):
        """Run a plan of high-level actions (Pokemon Red/Blue), then observe.

        `self.last_outcomes` is what the executor reported for each action —
        the honest label, and the only way to tell an action that worked from
        one that walked into a wall."""
        self.last_outcomes = self.tool("run_actions", actions=actions)
        return self.observe()

    def press(self, buttons, times=1, hold_frames=None, gap_frames=None):
        """Frame-accurate button presses. Works on any cartridge."""
        kw = {"buttons": list(buttons), "times": times}
        if hold_frames is not None:
            kw["hold_frames"] = hold_frames
        if gap_frames is not None:
            kw["gap_frames"] = gap_frames
        self.last_outcomes = self.tool("press_buttons", **kw)
        return self.observe()

    def step_frames(self, frames):
        self.tool("step", frames=frames)
        return self.observe()

    def read(self, address, length=1):
        """Raw bytes, exactly as the CPU sees them, without perturbing
        timing. Reserve it for facts the structured state does not carry —
        and see `docs/agent-environment.md` on why an *evaluated* agent
        should probably not be allowed this at all."""
        return self.tool("read_memory", address=address, length=length)

    def reward_from(self, obs, previous):
        """Deliberately not implemented. A reward function is the experiment,
        not the environment — shipping a default one would silently become
        everybody's benchmark. Write your own from `obs`."""
        raise NotImplementedError(
            "define your own reward: see docs/agent-environment.md "
            "§ 'Scoring that cannot be gamed'")

    # -- lifecycle ---------------------------------------------------------

    def close(self):
        if self.proc.poll() is None:
            try:
                self.proc.stdin.close()
            except OSError:
                pass
            self.proc.wait(timeout=10)

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()


def _self_test(server, rom):
    import time
    with GameBoyEnv(server, rom) as env:
        obs = env.reset(frames=600)
        print(f"rom          {obs['rom']}")
        print(f"mode         {obs['mode']}  screen.kind {obs['screen']['kind']} "
              f"trust {obs['screen']['trust']}")
        env.snapshot("/tmp/gb_env_selftest.gbss")
        # A block of work RAM is the cartridge-agnostic fingerprint. NOT the
        # `frame` field: that is a **session** counter — how many frames this
        # server has run — and `load_state` deliberately does not rewind it.
        before = env.read(0xC000, 32)
        t = time.time()
        env.tool("step", frames=10000)
        dt = time.time() - t
        print(f"throughput   {10000/dt:.0f} frames/s "
              f"({10000 * 16.743 / 1000 / dt:.0f}x real time)")
        moved = env.read(0xC000, 32)
        env.reset(state="/tmp/gb_env_selftest.gbss")
        back = env.read(0xC000, 32)
        exact = back == before
        print(f"reset        WRAM $C000 restored exactly: {exact}"
              + ("" if moved != before else "  (WARNING: it never moved, "
                                            "so this proves nothing)"))
        print(f"press        one A -> {env.press(['a'])['mode']}")


if __name__ == "__main__":
    import argparse
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="target/release/terminalgb-mcp")
    ap.add_argument("--rom", required=True)
    a = ap.parse_args()
    try:
        _self_test(a.server, a.rom)
    except MCPError as e:
        sys.exit(f"gb_env: {e}")

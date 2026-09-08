#!/usr/bin/env python3
"""Throughput bench for PyBoy, matching `examples/core_bench.rs`'s method.

    pip install pyboy
    taskset -c <a P-core> python3 tools/perf/pyboy-bench.py blue.gb 7168

Rendering and sound are both off, which is the configuration our `--no-pixels`
collector run matches. `tick(n, False)` really is n frames — checked against
`PyBoy.frame_count`, because a comparison whose unit is not the same unit is
not a comparison.

**It reports two figures, and the one to quote against `core_bench` is
`batch`.** This used to time `tick(1, False)` in a Python `for` loop, which
charges PyBoy 7,168 Python-level calls that our side does not pay: `core_bench`
runs `Gameboy::frame()` in a Rust loop with no binding in the way. Measured
here, that overhead is **17 % of PyBoy's own time** — 0.0189 ms/frame in the
loop against **0.0157** for the identical work in one `tick(7168, False)` — so
the loop figure understates PyBoy and flatters us by the same 17 %. Both are
printed because both are true of something: `loop` is what a Python consumer
stepping frame by frame experiences, `batch` is the emulator core, which is
what this comparison is about.

PyBoy is LGPL-3.0-only. This file only calls its public API; nothing of its
code is here, and `docs/measured/against-the-field.md` cites its technique
rather than reproducing it.
"""

import time, sys
from pyboy import PyBoy


def run(rom, frames, batched):
    gb = PyBoy(rom, window="null", sound_emulated=False)
    gb.set_emulation_speed(0)      # uncapped
    t0 = time.process_time()
    if batched:
        gb.tick(frames, False)     # render=False
    else:
        for _ in range(frames):
            gb.tick(1, False)
    t1 = time.process_time()
    assert gb.frame_count == frames, f"asked {frames} frames, ran {gb.frame_count}"
    gb.stop(save=False)
    return t1 - t0


rom, frames = sys.argv[1], int(sys.argv[2])
for name, batched in (("batch", True), ("loop", False)):
    s = run(rom, frames, batched)
    print(
        f"pyboy no-render {name}: {s:.3f} s cpu for {frames} frames "
        f"= {s * 1000 / frames:.4f} ms/frame = {frames / 59.7275 / s:.0f}x realtime"
    )

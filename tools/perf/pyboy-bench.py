#!/usr/bin/env python3
"""Throughput bench for PyBoy, matching `examples/core_bench.rs`'s method.

    pip install pyboy
    taskset -c <a P-core> python3 tools/perf/pyboy-bench.py blue.gb 7168

Rendering and sound are both off, which is the configuration our `--no-pixels`
collector run matches. `tick(n, False)` really is n frames — checked against
`PyBoy.frame_count`, because a comparison whose unit is not the same unit is
not a comparison.

PyBoy is LGPL-3.0-only. This file only calls its public API; nothing of its
code is here, and `docs/measured/against-the-field.md` cites its technique
rather than reproducing it.
"""

import time, sys
from pyboy import PyBoy
rom, frames = sys.argv[1], int(sys.argv[2])
gb = PyBoy(rom, window="null", sound_emulated=False)
gb.set_emulation_speed(0)          # uncapped
t0 = time.process_time()
for _ in range(frames):
    gb.tick(1, False)              # render=False
t1 = time.process_time()
s = t1 - t0
print(f"pyboy no-render: {s:.3f} s cpu for {frames} frames = {s*1000/frames:.4f} ms/frame = {frames/59.7275/s:.0f}x realtime")
gb.stop(save=False)

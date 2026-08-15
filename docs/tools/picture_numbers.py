#!/usr/bin/env python3
"""Keep `docs/picture.md`'s tables honest.

`docs/picture.md` explains how a frame reaches a terminal and how to capture
it. Almost everything on it is a fact about the code (which transports exist,
which upscalers exist and at what factors, which keys do what) or a
measurement. Both rot, and prose rots silently — so both are **generated**
into marker blocks here and verified by `make docs-check`.

Three sources, three different rules:

* **Source facts** — parsed out of `src/` on every run. The transport ladder,
  the upscaler table and the screenshot keys are read from the enums and the
  key handler themselves, so adding a fifth upscaler or reordering the ladder
  moves the page or fails the check.
* **Measurements** — read from `docs/picture/measurements.json`, which is
  *data*, not something re-derived on every check: a benchmark re-run would
  make `--check` fail on ordinary timing noise. Refresh it deliberately with
  `--measure` (which runs `examples/render_bench` on the vendored MIT
  dmg-acid2 ROM and needs nothing else), in the same commit as the page.
* **Terminal captures** — `docs/picture/*.png`, produced by hand with the
  recipe on the page. Only their presence is checked here.

Usage:
    python3 docs/tools/picture_numbers.py            # rewrite the page
    python3 docs/tools/picture_numbers.py --check    # fail if it has drifted
    python3 docs/tools/picture_numbers.py --measure  # re-run the benchmark

The marker convention is `status_numbers.py`'s and the atlas renderer's:
everything between `<!-- picture:NAME -->` and `<!-- /picture:NAME -->` is
machine-owned; prose either side is preserved.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
PAGE = ROOT / "docs" / "picture.md"
DATA = ROOT / "docs" / "picture" / "measurements.json"
IMAGES = ROOT / "docs" / "picture"

# The ROM every measurement runs on: vendored, MIT, redistributable, and the
# only game content in this repository.
BENCH_ROM = "third_party/dmg-acid2/dmg-acid2.gb"
BENCH_BIN = "target/release/examples/render_bench"

# Images the page embeds. Presence only — what they show is a human judgement.
REQUIRED_IMAGES = [
    "proto-kitty.png",
    "proto-sixel.png",
    "proto-halfblocks.png",
    "halfblocks-ours.png",
    "halfblocks-library.png",
    "screenshot-c-display.png",
    "screenshot-C-raw.png",
    "stats-overlay.png",
]


# ---------------------------------------------------------------------------
# Source facts
# ---------------------------------------------------------------------------


def read(rel: str) -> str:
    return (ROOT / rel).read_text(encoding="utf-8")


def transport_ladder() -> list[str]:
    """The `Graphics` variants of `src/screen/termcaps.rs`, in ladder order.

    That file's own doc comment says the order of the variants *is* the
    preference order, so parsing them in declaration order is reading the
    ladder, not guessing it.
    """
    src = read("src/screen/termcaps.rs")
    body = re.search(r"pub enum Graphics \{(.*?)\n\}", src, re.S)
    if not body:
        raise SystemExit("termcaps.rs: could not find `pub enum Graphics`")
    return re.findall(r"^\s*(\w+),", body.group(1), re.M)


def upscalers() -> list[tuple[str, list[int]]]:
    """Each `Algorithm`'s wire name and the factors it can produce."""
    src = read("src/upscale/mod.rs")
    names = dict(
        re.findall(r"Algorithm::(\w+) => \"([a-z0-9]+)\",", src)
    )
    factors_block = re.search(
        r"fn supported_factors\(self\).*?\{(.*?)\n    \}", src, re.S
    )
    if not factors_block:
        raise SystemExit("upscale/mod.rs: could not find `supported_factors`")
    out = []
    for variant, digits in re.findall(
        r"Algorithm::(\w+) => &\[([0-9, ]+)\]", factors_block.group(1)
    ):
        out.append(
            (names[variant], [int(d) for d in digits.split(",") if d.strip()])
        )
    return out


def picture_keys() -> dict[str, str]:
    """The key bindings this page documents, read from the game-key handler.

    A key that is renamed or removed must not keep being documented, and the
    handler is the only place that knows.
    """
    src = read("src/screen/tui.rs")
    found: dict[str, str] = {}
    patterns = {
        "screenshot-display": r"KeyCode::Char\('c'\) \| KeyCode::F\(2\) => \{\s*self\.screenshot\(false\)",
        "screenshot-raw": r"KeyCode::Char\('C'\) => \{\s*self\.screenshot\(true\)",
        "palette": r"KeyCode::Char\('\['\) => \{\s*self\.palette = self\.palette\.next\(\)",
        "upscale-algorithm": r"KeyCode::Char\('\]'\) => \{\s*self\.upscaler\.cycle_algorithm\(\)",
        "upscale-factor": r"KeyCode::Char\('\}'\) => \{\s*self\.upscale_factor",
    }
    for name, pat in patterns.items():
        found[name] = "yes" if re.search(pat, src) else "NO"
    return found


def constant(rel: str, name: str) -> str:
    src = read(rel)
    m = re.search(rf"const {name}: \w+ = ([0-9_]+)", src)
    if not m:
        raise SystemExit(f"{rel}: could not find `const {name}`")
    return m.group(1).replace("_", "")


# ---------------------------------------------------------------------------
# Measurement
# ---------------------------------------------------------------------------


def bench(**kw) -> dict:
    args = [
        str(ROOT / BENCH_BIN),
        "--rom", str(ROOT / BENCH_ROM),
        "--frames", str(kw.pop("frames", 300)),
        "--cols", str(kw.pop("cols", 213)),
        "--rows", str(kw.pop("rows", 55)),
        "--font", kw.pop("font", "8x16"),
        "--protocol", kw.pop("protocol", "kitty"),
        "--path", kw.pop("path", "capped"),
        "--upscale", kw.pop("upscale", "nearest"),
        "--factor", str(kw.pop("factor", 1)),
        "--json",
    ]
    if kw.pop("motion", True):
        args.append("--motion")
    assert not kw, f"unused bench arguments: {kw}"
    out = subprocess.run(args, capture_output=True, text=True)
    lines = [l for l in out.stdout.splitlines() if l.startswith("{")]
    if not lines:
        raise SystemExit(
            f"render_bench produced no JSON.\n{out.stderr[:2000]}\n"
            "Build it first: cargo build --release --features image "
            "--example render_bench"
        )
    return json.loads(lines[-1])


def measure() -> dict:
    """Re-run every measurement the page quotes.

    Deliberately `--motion` throughout: dmg-acid2 is a *still* picture, so a
    run without it measures the deduplicator rather than the transport — the
    trap `AGENTS.md` records as "never quote a transport figure from a
    static-image ROM". The one exception is the dedup row itself, which is
    measuring exactly that.
    """
    data: dict = {
        "rom": "dmg-acid2 (vendored, MIT)",
        "command": (
            "python3 docs/tools/picture_numbers.py --measure "
            "(runs examples/render_bench on the vendored dmg-acid2 ROM)"
        ),
        "note": (
            "Every row is --motion (every frame differs) except the dedup "
            "row, which is the point of that row. Font 8x16."
        ),
    }

    sizes = []
    for cols, rows in [(80, 24), (120, 40), (160, 50), (213, 55), (297, 80)]:
        capped = bench(cols=cols, rows=rows, path="capped")
        legacy = bench(cols=cols, rows=rows, path="legacy")
        sizes.append(
            {
                "cells": f"{cols}x{rows}",
                "display_px": f"{capped['display_px_w']}x{capped['display_px_h']}",
                "capped_bytes": round(capped["tx_bytes_avg"]),
                "capped_ms": round(capped["frame_ms_avg"], 3),
                "legacy_bytes": round(legacy["tx_bytes_avg"]),
                "legacy_ms": round(legacy["frame_ms_avg"], 3),
            }
        )
    data["window_independence"] = sizes

    b = bench(cols=213, rows=55)
    data["frame_path"] = {
        "cells": "213x55",
        "display_px": f"{b['display_px_w']}x{b['display_px_h']}",
        "transmitted_px": f"{b['tx_w']}x{b['tx_h']}",
        "emulate_ms": round(b["emulate_ms_avg"], 3),
        "palette_ms": round(b["palette_ms_avg"], 3),
        "upscale_ms": round(b["upscale_ms_avg"], 3),
        "encode_ms": round(b["encode_ms_avg"], 3),
        "write_ms": round(b["write_ms_avg"], 3),
        "frame_ms": round(b["frame_ms_avg"], 3),
        "bytes": round(b["tx_bytes_avg"]),
    }

    still = bench(cols=213, rows=55, motion=False)
    data["dedup"] = {
        "frames": still["frames"],
        "identical": still["frames_identical"],
        "bytes": round(still["tx_bytes_avg"], 1),
        "moving_bytes": round(b["tx_bytes_avg"]),
    }

    protos = []
    for proto in ("kitty", "iterm2"):
        row = {"protocol": proto}
        for path in ("capped", "legacy"):
            d = bench(cols=213, rows=55, protocol=proto, path=path)
            row[f"{path}_bytes"] = round(d["tx_bytes_avg"])
            row[f"{path}_ms"] = round(d["frame_ms_avg"], 3)
        protos.append(row)
    data["protocols"] = protos

    ups = []
    for algo, fac in [
        ("nearest", 1),
        ("nearest", 4),
        ("scale2x", 2),
        ("scale2x", 3),
        ("mmpx", 2),
        ("mmpx", 4),
        ("lcd", 4),
        ("lcd", 6),
    ]:
        d = bench(cols=213, rows=55, upscale=algo, factor=fac)
        ups.append(
            {
                "algorithm": algo,
                "factor": fac,
                "px": f"{d['tx_w']}x{d['tx_h']}",
                "upscale_ms": round(d["upscale_ms_avg"], 3),
                "encode_ms": round(d["encode_ms_avg"], 3),
                "bytes": round(d["tx_bytes_avg"]),
            }
        )
    data["upscalers"] = ups
    return data


# ---------------------------------------------------------------------------
# Rendering
# ---------------------------------------------------------------------------


def fmt_bytes(n: float) -> str:
    if n >= 1_000_000:
        return f"{n / 1_000_000:.1f} MB"
    if n >= 1000:
        return f"{n / 1000:.1f} KB"
    return f"{n:.0f} B"


LADDER_DETAIL = {
    "Kitty": (
        "[kitty graphics protocol](https://sw.kovidgoyal.net/kitty/graphics-protocol/)",
        "Kitty, Ghostty, WezTerm, Konsole",
        "an `a=q` handshake the terminal answers `OK`",
        "cell-addressed, and can take the frame through shared memory",
    ),
    "Sixel": (
        "[sixel](https://en.wikipedia.org/wiki/Sixel) (DEC, 1987)",
        "foot, xterm (`-ti vt340`), Konsole, WezTerm, mlterm",
        "parameter `4` in the Primary DA reply",
        "real pixels, but the whole picture goes over the wire every frame",
    ),
    "Iterm2": (
        "[iTerm2 inline images](https://iterm2.com/documentation-images.html)",
        "iTerm2, WezTerm, VSCode's terminal",
        "**cannot be asked** — an `$TERM_PROGRAM` guess",
        "cell-addressed, so the picture is still sized by us and not the window",
    ),
    "Halfblocks": (
        "no protocol at all — the `▀` glyph, foreground over background",
        "everything, including VTE (GNOME Terminal, GNOME Console, Ptyxis)",
        "nothing to ask; it is the floor",
        "two vertical pixels per character cell — sharp, but coarse",
    ),
}


def render_blocks(data: dict) -> dict[str, str]:
    blocks: dict[str, str] = {}

    # --- the ladder -------------------------------------------------------
    rows = ["| # | What it is | How we find out | What it buys | Terminals |",
            "|---|---|---|---|---|"]
    for i, variant in enumerate(transport_ladder(), 1):
        what, terms, how, buys = LADDER_DETAIL[variant]
        rows.append(f"| {i} | {what} | {how} | {buys} | {terms} |")
    blocks["ladder"] = "\n".join(rows)

    # --- upscalers --------------------------------------------------------
    by_name = {u["algorithm"]: u for u in data["upscalers"]}
    rows = ["| `upscale` | Factors (`}`) | What it does |", "|---|---|---|"]
    detail = {
        "nearest": "plain integer pixel duplication — the crisp baseline, and a true no-op at ×1",
        "scale2x": "EPX/Scale2x/AdvMAME2x exact-match corner rule: staircases get one diagonal step, no new colours",
        "mmpx": "MMPX style-preserving magnification: 2:1 slopes, single-pixel details kept, still no new colours",
        "lcd": "nearest plus a darkened dot-matrix gap between cells — the \"looks like a Game Boy\" option",
    }
    for name, factors in upscalers():
        facs = "/".join(str(f) for f in factors)
        rows.append(f"| `{name}` | {facs} | {detail[name]} |")
    blocks["upscalers"] = "\n".join(rows)

    # --- upscaler cost ----------------------------------------------------
    rows = [
        "| Setting | Transmitted | Upscale | Encode | On the wire |",
        "|---|---:|---:|---:|---:|",
    ]
    for u in data["upscalers"]:
        rows.append(
            f"| `{u['algorithm']}` ×{u['factor']} | {u['px']} | "
            f"{u['upscale_ms']:.3f} ms | {u['encode_ms']:.3f} ms | "
            f"{fmt_bytes(u['bytes'])}/frame |"
        )
    blocks["upscale-cost"] = "\n".join(rows)

    # --- window independence ---------------------------------------------
    rows = [
        "| Terminal window | Display area | **Sized by the picture** | | **Sized by the window** | |",
        "|---|---|---:|---:|---:|---:|",
        "| cells | pixels | on the wire | per frame | on the wire | per frame |",
    ]
    for s in data["window_independence"]:
        rows.append(
            f"| {s['cells']} | {s['display_px']} | "
            f"**{fmt_bytes(s['capped_bytes'])}** | **{s['capped_ms']:.2f} ms** | "
            f"{fmt_bytes(s['legacy_bytes'])} | {s['legacy_ms']:.2f} ms |"
        )
    blocks["window-independence"] = "\n".join(rows)

    # --- the frame path ---------------------------------------------------
    p = data["frame_path"]
    rows = [
        "| Step | Where | Cost |",
        "|---|---|---:|",
        f"| Emulate one frame | [`Gameboy::frame`](../src/gameboy.rs) | {p['emulate_ms']:.3f} ms |",
        f"| Map 4 shades → RGBA | [`screen::frame`](../src/screen/frame.rs) | {p['palette_ms']:.3f} ms |",
        f"| Upscale | [`upscale`](../src/upscale/mod.rs) | {p['upscale_ms']:.3f} ms |",
        f"| Encode + place | [`termimage`](../src/termimage.rs) | {p['encode_ms']:.3f} ms |",
        f"| Write to the terminal | the frontend, after `ratatui` flushes | {p['write_ms']:.3f} ms |",
        f"| **Whole displayed frame** | | **{p['frame_ms']:.3f} ms** |",
    ]
    blocks["frame-path"] = "\n".join(rows)

    # --- inline facts -----------------------------------------------------
    keys = picture_keys()
    missing = [k for k, v in keys.items() if v != "yes"]
    if missing:
        raise SystemExit(
            "src/screen/tui.rs no longer binds: "
            + ", ".join(missing)
            + " — docs/picture.md documents these keys."
        )
    facts = {
        "source-px": "160×144",
        "max-cells": constant("src/termimage.rs", "MAX_CELLS"),
        "max-palette": constant("src/termimage.rs", "MAX_PALETTE"),
        "transports": str(len(transport_ladder())),
        "upscalers": str(len(upscalers())),
        "still-bytes": f"{data['dedup']['bytes']:.0f}",
        "still-identical": f"{data['dedup']['identical']}/{data['dedup']['frames']}",
        "moving-bytes": fmt_bytes(data["dedup"]["moving_bytes"]),
    }
    blocks.update({f"fact:{k}": v for k, v in facts.items()})
    return blocks


BLOCK_RE = re.compile(
    r"(<!-- picture:([a-z:\-]+) -->)(.*?)(<!-- /picture:\2 -->)", re.S
)


def apply_blocks(text: str, blocks: dict[str, str]) -> str:
    seen: set[str] = set()

    def sub(m: re.Match) -> str:
        name = m.group(2)
        seen.add(name)
        if name not in blocks:
            raise SystemExit(f"docs/picture.md: unknown marker `picture:{name}`")
        body = blocks[name]
        if name.startswith("fact:"):
            return f"{m.group(1)}{body}{m.group(4)}"
        return f"{m.group(1)}\n{body}\n{m.group(4)}"

    out = BLOCK_RE.sub(sub, text)
    unused = sorted(set(blocks) - seen)
    if unused:
        raise SystemExit(
            "generated but not placed in docs/picture.md: " + ", ".join(unused)
        )
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if the page has drifted")
    ap.add_argument("--measure", action="store_true", help="re-run the benchmark")
    args = ap.parse_args()

    if args.measure:
        data = measure()
        DATA.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")
        print(f"wrote {DATA.relative_to(ROOT)}")

    if not PAGE.exists():
        print(f"missing {PAGE.relative_to(ROOT)}", file=sys.stderr)
        return 1
    if not DATA.exists():
        print(
            f"missing {DATA.relative_to(ROOT)} — run with --measure",
            file=sys.stderr,
        )
        return 1

    missing_img = [n for n in REQUIRED_IMAGES if not (IMAGES / n).exists()]
    if missing_img:
        print(
            "docs/picture.md embeds images that are not in the tree: "
            + ", ".join(missing_img),
            file=sys.stderr,
        )
        return 1

    data = json.loads(DATA.read_text(encoding="utf-8"))
    text = PAGE.read_text(encoding="utf-8")
    fresh = apply_blocks(text, render_blocks(data))

    if args.check:
        if fresh != text:
            print(
                "docs/picture.md is out of date with the source and/or\n"
                "docs/picture/measurements.json. Regenerate it:\n"
                "    python3 docs/tools/picture_numbers.py",
                file=sys.stderr,
            )
            return 1
        print("docs/picture.md: up to date")
        return 0

    if fresh != text:
        PAGE.write_text(fresh, encoding="utf-8")
        print(f"rewrote {PAGE.relative_to(ROOT)}")
    else:
        print("docs/picture.md: already up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

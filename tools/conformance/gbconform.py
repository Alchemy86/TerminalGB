#!/usr/bin/env python3
"""gbconform — run Game Boy conformance ROMs against ANY emulator.

This is the runner behind `Alchemy86/TerminalGB`'s reusable conformance action.
It knows nothing about the emulator under test: it talks to an **adapter**, a
command the emulator's author supplies, through the smallest contract we could
make work (see `docs/conformance-action.md`, and `--contract` below).

Design rules, in priority order:

1. **Tiny contract.** The fewer demands, the more emulators can take part. An
   adapter is told where the ROM is, how long to run, and which of two files to
   write. It writes them and exits. No protocol, no JSON, no stdout parsing.
2. **Explain the failure.** Any tool can print `FAIL`. The value is telling an
   author *"your `POP AF` is not masking the low nibble of F"*. Every check
   carries that text in `checks.json`, and it is emitted with the result.
3. **No dependencies.** Python standard library only, so adopting this costs a
   `uses:` line and no toolchain — the same hand-rolled ethos as this
   project's own test harness.
4. **We are not the authority.** No check's expected output comes from our
   emulator: serial verdicts are the test ROM's own printed judgement, and the
   dmg-acid2 verdict is its author's reference image.

Outputs (all under `--out`): `results.json` (documented schema), `badge.json`
(shields.io endpoint), `badge.svg` (self-contained), `summary.md`, and an
`artifacts/` directory with the captured serial text and any frame + diff.
"""

from __future__ import annotations

import argparse
import io
import json
import os
import shlex
import shutil
import subprocess
import sys
import tarfile
import tempfile
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import image  # noqa: E402  (local, sibling module)

CONTRACT = "gbconform/1"
RESULTS_SCHEMA = 1

CONTRACT_TEXT = f"""\
The adapter contract — {CONTRACT}

The runner executes:

    <adapter-command> <rom-path>

with these environment variables set:

    GBCONFORM_CONTRACT    always "{CONTRACT}"
    GBCONFORM_ROM         the ROM path (also argv[1] — use whichever suits)
    GBCONFORM_MODEL       "dmg" or "cgb": which model to emulate
    GBCONFORM_FRAMES      run at least this many frames, then stop
    GBCONFORM_SERIAL_OUT  set ONLY when serial output is wanted: write the
                          raw bytes the ROM sends to the link port (SB) here
    GBCONFORM_FRAME_OUT   set ONLY when a frame is wanted: write the final
                          160x144 frame here, as PNG, Netpbm P6/P5, or a raw
                          RGBA / RGB / 8-bit dump — whatever you already have
    GBCONFORM_TIMEOUT_S   wall-clock budget; the runner kills you after it

Exit 0 when the run completed. A non-zero exit, a timeout, or an unreadable
output file is reported as a failure with your stderr attached.

That is the whole contract. An adapter is usually a short shell script that
maps these variables onto your emulator's own flags; two working examples are
in tools/conformance/adapters/.

You do NOT have to support both outputs. The runner probes each capability
once with a known-good ROM; checks needing a capability your adapter cannot
provide are reported as skipped, not failed, and are excluded from the score.
"""

FRAME_W, FRAME_H = 160, 144

# A screen check tolerates no differing pixels. Kept as a named constant
# because "how close is close enough" is a judgement call worth seeing.
SCREEN_TOLERANCE_PIXELS = 0


# ---------------------------------------------------------------------------
# Results model
# ---------------------------------------------------------------------------


class Result:
    """One check's outcome.

    `status` is one of:
      pass     — the ROM's own verdict, or the reference image, says so
      fail     — it says otherwise; `explain` says what that usually means
      skipped  — the adapter cannot provide the capability this check needs
      error    — the adapter crashed, timed out, or wrote something unreadable
    """

    def __init__(self, check: dict):
        self.check = check
        self.status = "error"
        self.detail = ""
        self.evidence: dict = {}
        self.artifacts: list[str] = []

    @property
    def counts_towards_score(self) -> bool:
        return self.status != "skipped"

    def to_json(self) -> dict:
        c = self.check
        out = {
            "id": c["id"],
            "suite": c["suite"],
            "method": c["method"],
            "status": self.status,
            "detail": self.detail,
            "behaviour": c.get("behaviour", ""),
            "evidence": self.evidence,
            "artifacts": self.artifacts,
        }
        # The "why" is only useful on a failure, and including it on every pass
        # would bury it. Errors get it too: an adapter that crashes on one ROM
        # is often crashing *because* of the behaviour under test.
        if self.status in ("fail", "error"):
            out["explain"] = c.get("explain", [])
            out["hardware"] = c.get("hardware", "")
            out["reference"] = c.get("reference", "")
        return out


# ---------------------------------------------------------------------------
# ROM corpus
# ---------------------------------------------------------------------------


def resolve_suite(suite_id: str, suite: dict, repo_root: Path, cache: Path,
                  roms_dir: Path | None, allow_fetch: bool) -> Path | None:
    """Return the directory the suite's ROMs live in, fetching if needed."""
    src = suite["source"]
    if roms_dir is not None:
        local = roms_dir / suite_id
        if local.is_dir():
            return local
        if roms_dir.is_dir() and src["kind"] == "fetch":
            # A flat user-supplied directory is also accepted.
            return roms_dir
    if src["kind"] == "vendored":
        d = repo_root / src["dir"]
        return d if d.is_dir() else None
    if not allow_fetch:
        return None
    return fetch_suite(suite_id, src, cache)


def fetch_suite(suite_id: str, src: dict, cache: Path) -> Path | None:
    """Download and unpack a suite tarball, once, into the cache directory.

    Pinned to a commit: an unpinned suite would make a score drift for reasons
    that have nothing to do with the emulator under test.
    """
    dest = cache / f"{suite_id}-{src['ref'][:12]}"
    stamp = dest / ".gbconform-complete"
    if stamp.exists():
        return dest
    url = src["url"].format(ref=src["ref"])
    log(f"fetching {suite_id} from {url}")
    try:
        with urllib.request.urlopen(url, timeout=120) as r:
            blob = r.read()
    except Exception as e:  # noqa: BLE001 — any network failure is the same story
        log(f"  !! could not fetch {suite_id}: {e}")
        return None
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    strip = int(src.get("strip_components", 0))
    with tarfile.open(fileobj=io.BytesIO(blob), mode="r:gz") as tf:
        for member in tf.getmembers():
            if not member.isfile():
                continue
            parts = Path(member.name).parts[strip:]
            if not parts:
                continue
            target = dest.joinpath(*parts)
            # Never let an archive write outside the destination.
            if not str(target.resolve()).startswith(str(dest.resolve())):
                continue
            target.parent.mkdir(parents=True, exist_ok=True)
            f = tf.extractfile(member)
            if f is not None:
                target.write_bytes(f.read())
    stamp.write_text("ok\n")
    return dest


# ---------------------------------------------------------------------------
# Running the adapter
# ---------------------------------------------------------------------------


class AdapterRun:
    __slots__ = ("returncode", "stderr", "timed_out", "serial", "frame")

    def __init__(self):
        self.returncode = 0
        self.stderr = ""
        self.timed_out = False
        self.serial: bytes | None = None
        self.frame: bytes | None = None


def run_adapter(adapter: list[str], rom: Path, *, frames: int, model: str,
                want_serial: bool, want_frame: bool, timeout: int) -> AdapterRun:
    out = AdapterRun()
    with tempfile.TemporaryDirectory(prefix="gbconform-") as td:
        tmp = Path(td)
        serial_path = tmp / "serial.bin"
        frame_path = tmp / "frame.out"
        env = dict(os.environ)
        env.update(
            GBCONFORM_CONTRACT=CONTRACT,
            GBCONFORM_ROM=str(rom),
            GBCONFORM_MODEL=model,
            GBCONFORM_FRAMES=str(frames),
            GBCONFORM_TIMEOUT_S=str(timeout),
        )
        if want_serial:
            env["GBCONFORM_SERIAL_OUT"] = str(serial_path)
        if want_frame:
            env["GBCONFORM_FRAME_OUT"] = str(frame_path)
        try:
            proc = subprocess.run(  # noqa: S603 — the adapter is the user's own command
                adapter + [str(rom)],
                env=env,
                capture_output=True,
                timeout=timeout,
                check=False,
            )
            out.returncode = proc.returncode
            out.stderr = proc.stderr.decode("utf-8", "replace")[-4000:]
        except subprocess.TimeoutExpired as e:
            out.timed_out = True
            out.stderr = (e.stderr or b"").decode("utf-8", "replace")[-4000:]
        except FileNotFoundError:
            out.returncode = 127
            out.stderr = f"adapter command not found: {adapter[0]}"
            return out
        if want_serial and serial_path.exists():
            out.serial = serial_path.read_bytes()
        if want_frame and frame_path.exists():
            out.frame = frame_path.read_bytes()
    return out


# ---------------------------------------------------------------------------
# Verdicts
# ---------------------------------------------------------------------------


def judge_serial(res: Result, run: AdapterRun, artifacts: Path) -> None:
    text = (run.serial or b"").decode("latin-1")
    printable = "".join(ch if ch.isprintable() or ch in "\n\r\t" else "." for ch in text)
    res.evidence["serial"] = printable.strip()[-2000:]
    if run.serial:
        p = artifacts / f"{safe(res.check['id'])}.serial.txt"
        p.write_text(printable)
        res.artifacts.append(p.name)

    if run.timed_out:
        res.status = "error"
        res.detail = "the adapter exceeded its wall-clock budget"
        return
    if run.returncode != 0:
        res.status = "error"
        res.detail = f"the adapter exited {run.returncode}"
        res.evidence["stderr"] = run.stderr
        return
    if "Passed" in text:
        res.status = "pass"
        res.detail = "the ROM reported Passed"
        return
    if "Failed" in text:
        res.status = "fail"
        # Blargg prints "Failed #N" — N identifies the sub-test, and quoting it
        # is the single most actionable thing in the whole report.
        line = next(
            (ln.strip() for ln in printable.splitlines() if "Failed" in ln), "Failed"
        )
        res.detail = f"the ROM reported {line}"
        return
    if not text.strip():
        res.status = "error"
        res.detail = (
            "no serial output at all — the ROM produced no result before the "
            "frame budget ran out (a hang, a crash, or serial writes not "
            "being captured)"
        )
        return
    res.status = "fail"
    res.detail = (
        "the ROM printed neither Passed nor Failed before the frame budget "
        "ran out; it is probably still running or wedged"
    )


def judge_screen(res: Result, run: AdapterRun, reference: Path,
                 artifacts: Path) -> None:
    if run.timed_out:
        res.status = "error"
        res.detail = "the adapter exceeded its wall-clock budget"
        return
    if run.returncode != 0:
        res.status = "error"
        res.detail = f"the adapter exited {run.returncode}"
        res.evidence["stderr"] = run.stderr
        return
    if not run.frame:
        res.status = "error"
        res.detail = "the adapter wrote no frame for this ROM"
        res.evidence["stderr"] = run.stderr
        return
    try:
        actual = image.decode(run.frame, expect=(FRAME_W, FRAME_H))
    except image.ImageError as e:
        res.status = "error"
        res.detail = f"could not read the frame the adapter wrote: {e}"
        return
    try:
        ref = image.decode(reference.read_bytes())
        differing, mask = image.compare(actual, ref)
    except image.ImageError as e:
        res.status = "error"
        res.detail = str(e)
        return

    res.evidence["differing_pixels"] = differing
    res.evidence["total_pixels"] = actual.width * actual.height
    got = artifacts / f"{safe(res.check['id'])}.actual.png"
    got.write_bytes(image.encode_png(actual))
    res.artifacts.append(got.name)

    if differing <= SCREEN_TOLERANCE_PIXELS:
        res.status = "pass"
        res.detail = "the frame matches the reference image exactly"
        return

    box = bounding_box(mask, actual.width)
    res.evidence["difference_bounds"] = {
        "x": box[0], "y": box[1], "width": box[2], "height": box[3]
    }
    dp = artifacts / f"{safe(res.check['id'])}.diff.png"
    dp.write_bytes(image.encode_png(image.diff_image(actual, mask)))
    res.artifacts.append(dp.name)
    pct = 100.0 * differing / (actual.width * actual.height)
    res.status = "fail"
    res.detail = (
        f"{differing} of {actual.width * actual.height} pixels differ "
        f"({pct:.1f}%), inside x{box[0]}..{box[0] + box[2] - 1}, "
        f"y{box[1]}..{box[1] + box[3] - 1} — see the diff artifact"
    )


def bounding_box(mask: bytes, width: int) -> tuple[int, int, int, int]:
    xs_min = ys_min = 1 << 30
    xs_max = ys_max = -1
    for i, bad in enumerate(mask):
        if not bad:
            continue
        x, y = i % width, i // width
        xs_min, xs_max = min(xs_min, x), max(xs_max, x)
        ys_min, ys_max = min(ys_min, y), max(ys_max, y)
    if xs_max < 0:
        return (0, 0, 0, 0)
    return (xs_min, ys_min, xs_max - xs_min + 1, ys_max - ys_min + 1)


def safe(check_id: str) -> str:
    return "".join(c if c.isalnum() or c in "-_." else "_" for c in check_id)


# ---------------------------------------------------------------------------
# Capability probe
# ---------------------------------------------------------------------------


def probe(adapter: list[str], rom: Path, capability: str, timeout: int) -> tuple[bool, str]:
    """Can this adapter provide `capability` at all?

    Run one known-good ROM and see whether the requested file appears. This
    distinguishes *cannot* from *broke*, which matters: an adapter with no
    serial support should have its serial checks SKIPPED, while an adapter that
    normally emits serial and produced none for one ROM has FAILED that ROM.

    **A clean decline is the only way to say "cannot".** The adapter must exit
    0 and write nothing — which is what declining looks like in the contract
    (see `adapters/binjgb.sh`). A crash or a timeout is NOT a missing
    capability: without that distinction an emulator that segfaults on every
    ROM would produce no files, be read as having no capabilities at all, and
    score 0 out of 0 — which a badge renders as a clean slate rather than as
    the total failure it is. It must score 0 out of everything.
    """
    run = run_adapter(
        adapter, rom,
        frames=600, model="dmg",
        want_serial=(capability == "serial"),
        want_frame=(capability == "screen"),
        timeout=timeout,
    )
    produced = run.serial if capability == "serial" else run.frame
    if produced:
        return True, ""
    if run.timed_out:
        return True, "timed out during the capability probe"
    if run.returncode != 0:
        return True, f"exited {run.returncode} during the capability probe"
    var = "GBCONFORM_SERIAL_OUT" if capability == "serial" else "GBCONFORM_FRAME_OUT"
    return False, f"exited cleanly without writing a {var} file"


# ---------------------------------------------------------------------------
# Badges
# ---------------------------------------------------------------------------


def badge_colour(passed: int, total: int) -> str:
    if total == 0:
        return "lightgrey"
    pct = 100 * passed / total
    if pct >= 100:
        return "brightgreen"
    if pct >= 90:
        return "green"
    if pct >= 75:
        return "yellowgreen"
    if pct >= 50:
        return "yellow"
    if pct > 0:
        return "orange"
    return "red"


def write_badge_json(path: Path, label: str, passed: int, total: int) -> None:
    """shields.io *endpoint* schema — https://shields.io/badges/endpoint-badge"""
    path.write_text(
        json.dumps(
            {
                "schemaVersion": 1,
                "label": label,
                "message": f"{passed}/{total}" if total else "no checks run",
                "color": badge_colour(passed, total),
            },
            indent=2,
        )
        + "\n"
    )


def write_badge_svg(path: Path, label: str, passed: int, total: int) -> None:
    """A self-contained SVG, for private repos and for anyone who does not want
    a third-party request in their README. Same numbers, no shields.io."""
    message = f"{passed}/{total}" if total else "no checks run"
    colours = {
        "brightgreen": "#4c1", "green": "#97ca00", "yellowgreen": "#a4a61d",
        "yellow": "#dfb317", "orange": "#fe7d37", "red": "#e05d44",
        "lightgrey": "#9f9f9f",
    }
    fill = colours[badge_colour(passed, total)]
    # 7px per character is the conventional approximation for the 11px
    # DejaVu Sans that shields.io uses; close enough that nothing clips.
    lw = 7 * len(label) + 10
    mw = 7 * len(message) + 10
    w = lw + mw
    esc = lambda s: (  # noqa: E731
        s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")
    )
    path.write_text(
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="20" '
        f'role="img" aria-label="{esc(label)}: {esc(message)}">'
        f"<title>{esc(label)}: {esc(message)}</title>"
        '<linearGradient id="s" x2="0" y2="100%">'
        '<stop offset="0" stop-color="#bbb" stop-opacity=".1"/>'
        '<stop offset="1" stop-opacity=".1"/></linearGradient>'
        f'<clipPath id="r"><rect width="{w}" height="20" rx="3" fill="#fff"/></clipPath>'
        '<g clip-path="url(#r)">'
        f'<rect width="{lw}" height="20" fill="#555"/>'
        f'<rect x="{lw}" width="{mw}" height="20" fill="{fill}"/>'
        f'<rect width="{w}" height="20" fill="url(#s)"/></g>'
        '<g fill="#fff" text-anchor="middle" '
        'font-family="Verdana,Geneva,DejaVu Sans,sans-serif" '
        'font-size="11" text-rendering="geometricPrecision">'
        f'<text x="{lw / 2:.0f}" y="15" fill="#010101" fill-opacity=".3">{esc(label)}</text>'
        f'<text x="{lw / 2:.0f}" y="14">{esc(label)}</text>'
        f'<text x="{lw + mw / 2:.0f}" y="15" fill="#010101" fill-opacity=".3">{esc(message)}</text>'
        f'<text x="{lw + mw / 2:.0f}" y="14">{esc(message)}</text>'
        "</g></svg>\n"
    )


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def write_summary(path: Path, results: list[Result], passed: int, scored: int,
                  skipped: list[Result], label: str) -> None:
    out = [f"## {label} — {passed}/{scored}", ""]
    if scored:
        out.append(f"**{100 * passed / scored:.0f}%** of attempted checks pass.")
    out += ["", "| Check | Result | Detail |", "| --- | --- | --- |"]
    icon = {"pass": "✅", "fail": "❌", "error": "💥", "skipped": "⏭️"}
    for r in results:
        out.append(
            f"| `{r.check['id']}` | {icon[r.status]} {r.status} | "
            f"{r.detail.replace('|', '\\|')} |"
        )
    bad = [r for r in results if r.status in ("fail", "error")]
    if bad:
        out += ["", "### Why these failed", ""]
        for r in bad:
            out += [f"#### `{r.check['id']}` — {r.detail}", ""]
            if r.check.get("behaviour"):
                out += [f"*Under test:* {r.check['behaviour']}", ""]
            for line in r.check.get("explain", []):
                out.append(f"- {line}")
            if r.check.get("hardware"):
                out += ["", f"*Hardware:* {r.check['hardware']}"]
            if r.check.get("reference"):
                out.append(f" · [reference]({r.check['reference']})")
            out.append("")
    if skipped:
        out += ["", "### Skipped (excluded from the score)", ""]
        for r in skipped:
            out.append(f"- `{r.check['id']}` — {r.detail}")
    out.append("")
    path.write_text("\n".join(out))


def log(msg: str) -> None:
    print(msg, file=sys.stderr, flush=True)


# ---------------------------------------------------------------------------
# Entry point
# ---------------------------------------------------------------------------


def main(argv: list[str]) -> int:
    here = Path(__file__).resolve().parent
    ap = argparse.ArgumentParser(
        prog="gbconform",
        description="Run Game Boy conformance ROMs against any emulator.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=CONTRACT_TEXT,
    )
    ap.add_argument("--adapter", help="command implementing the adapter contract")
    ap.add_argument("--contract", action="store_true",
                    help="print the adapter contract and exit")
    ap.add_argument("--out", default="conformance-out", type=Path,
                    help="output directory (default: conformance-out)")
    ap.add_argument("--checks", default=here / "checks.json", type=Path)
    ap.add_argument("--repo-root", default=here.parent.parent, type=Path,
                    help="root of this repository (for vendored ROMs)")
    ap.add_argument("--roms-dir", type=Path,
                    help="use ROMs from here instead of fetching them")
    # Deliberately NOT under --out: the output directory is uploaded as a CI
    # artifact, and putting the fetched Blargg corpus in it would both bloat
    # the upload and effectively republish those ROMs from the adopting
    # project's own repository. We fetch them; we do not redistribute them.
    ap.add_argument("--cache-dir", type=Path,
                    help="where fetched suites are unpacked "
                         "(default: <system temp>/gbconform-roms)")
    ap.add_argument("--no-fetch", action="store_true",
                    help="never download; suites without local ROMs are skipped")
    ap.add_argument("--only", default="",
                    help="comma-separated check-id prefixes to run")
    ap.add_argument("--timeout", type=int, default=120,
                    help="per-ROM wall-clock budget in seconds (default: 120)")
    ap.add_argument("--label", default="conformance", help="badge label")
    ap.add_argument("--fail-below", type=float, default=-1.0,
                    help="exit non-zero if the pass percentage is below this")
    ap.add_argument("--model", default="dmg", choices=["dmg", "cgb"])
    args = ap.parse_args(argv)

    if args.contract:
        print(CONTRACT_TEXT)
        return 0
    if not args.adapter:
        ap.error("--adapter is required (or use --contract)")

    adapter = shlex.split(args.adapter)
    registry = json.loads(args.checks.read_text())
    out_dir: Path = args.out
    artifacts = out_dir / "artifacts"
    artifacts.mkdir(parents=True, exist_ok=True)
    cache = args.cache_dir or (Path(tempfile.gettempdir()) / "gbconform-roms")
    cache.mkdir(parents=True, exist_ok=True)

    only = [p for p in args.only.split(",") if p]
    checks = [
        c for c in registry["checks"]
        if not only or any(c["id"].startswith(p) for p in only)
    ]

    # --- resolve each suite's ROMs once -----------------------------------
    suite_dirs: dict[str, Path | None] = {}
    for sid in {c["suite"] for c in checks}:
        suite_dirs[sid] = resolve_suite(
            sid, registry["suites"][sid], args.repo_root, cache,
            args.roms_dir, not args.no_fetch,
        )

    # --- probe capabilities once ------------------------------------------
    # A capability is probed with the first check that needs it, so the probe
    # ROM is always one we know is well-formed.
    capabilities: dict[str, tuple[bool, str]] = {}
    for method in ("serial", "screen"):
        first = next((c for c in checks if c["method"] == method), None)
        if first is None:
            continue
        sdir = suite_dirs.get(first["suite"])
        if sdir is None:
            capabilities[method] = (False, "the ROMs for this suite are unavailable")
            continue
        ok, why = probe(adapter, sdir / first["rom"], method, args.timeout)
        capabilities[method] = (ok, why)
        if ok and why:
            # Assumed available *despite* the probe failing — the checks will
            # run and record real errors rather than vanishing into "skipped".
            log(f"capability {method}: assumed yes — the adapter {why}")
        else:
            log(f"capability {method}: {'yes' if ok else 'no — it ' + why}")

    # --- run ---------------------------------------------------------------
    results: list[Result] = []
    for c in checks:
        res = Result(c)
        results.append(res)
        sdir = suite_dirs.get(c["suite"])
        if sdir is None:
            res.status = "skipped"
            res.detail = (
                f"the {c['suite']} ROMs were not available "
                "(no network, --no-fetch, or nothing supplied in --roms-dir)"
            )
            continue
        able, why = capabilities.get(c["method"], (False, "not probed"))
        if not able:
            res.status = "skipped"
            res.detail = f"your adapter provides no {c['method']} output — it {why}"
            continue
        rom = sdir / c["rom"]
        if not rom.is_file():
            res.status = "skipped"
            res.detail = f"ROM not found: {c['rom']}"
            continue
        log(f"running {c['id']}")
        run = run_adapter(
            adapter, rom,
            frames=int(c.get("frames", 600)), model=args.model,
            want_serial=(c["method"] == "serial"),
            want_frame=(c["method"] == "screen"),
            timeout=args.timeout,
        )
        if c["method"] == "serial":
            judge_serial(res, run, artifacts)
        else:
            judge_screen(res, run, sdir / c["reference_image"], artifacts)
        log(f"  {res.status}: {res.detail}")

    # --- score -------------------------------------------------------------
    scored = [r for r in results if r.counts_towards_score]
    skipped = [r for r in results if not r.counts_towards_score]
    passed = sum(1 for r in scored if r.status == "pass")
    total = len(scored)
    pct = 100.0 * passed / total if total else 0.0

    (out_dir / "results.json").write_text(
        json.dumps(
            {
                "schema": RESULTS_SCHEMA,
                "contract": CONTRACT,
                "label": args.label,
                "model": args.model,
                "adapter": args.adapter,
                "score": {
                    "passed": passed,
                    "total": total,
                    "percent": round(pct, 2),
                    "skipped": len(skipped),
                },
                "capabilities": {
                    k: {"available": v[0], "reason": v[1]}
                    for k, v in capabilities.items()
                },
                "checks": [r.to_json() for r in results],
            },
            indent=2,
        )
        + "\n"
    )
    write_badge_json(out_dir / "badge.json", args.label, passed, total)
    write_badge_svg(out_dir / "badge.svg", args.label, passed, total)
    write_summary(out_dir / "summary.md", results, passed, total, skipped, args.label)

    log(f"\n{args.label}: {passed}/{total} ({pct:.0f}%), {len(skipped)} skipped")

    # GitHub Actions plumbing, harmless elsewhere.
    if gh_out := os.environ.get("GITHUB_OUTPUT"):
        with open(gh_out, "a") as f:
            f.write(f"passed={passed}\ntotal={total}\npercent={pct:.2f}\n")
            f.write(f"skipped={len(skipped)}\nresults-dir={out_dir}\n")
    if gh_sum := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(gh_sum, "a") as f:
            f.write((out_dir / "summary.md").read_text())

    if args.fail_below >= 0 and pct < args.fail_below:
        log(f"FAIL: {pct:.0f}% is below the --fail-below threshold of {args.fail_below:.0f}%")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))

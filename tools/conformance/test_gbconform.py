#!/usr/bin/env python3
"""Tests for the conformance runner. `python3 -m unittest discover tools/conformance`.

The runner is the part of this action that has to be right for *other people's*
projects, and it is the part with no emulator behind it to catch mistakes — so
it is tested directly, with fake adapters standing in for a real emulator.

The end-to-end cases below are the interesting ones: they drive the actual
`main()` against adapters that pass, fail, crash, hang and decline a
capability, and assert on the results JSON. Standard library only, like the
runner itself.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
sys.path.insert(0, str(HERE))

import gbconform  # noqa: E402
import image  # noqa: E402
import screentext  # noqa: E402

REFERENCE = REPO / "third_party" / "dmg-acid2" / "reference-dmg.png"


def rgb_image(w, h, fill=b"\x00\x00\x00"):
    return image.Image(w, h, fill * (w * h))


class ImageDecoding(unittest.TestCase):
    def test_reads_the_real_2bit_greyscale_reference_png(self):
        # The upstream dmg-acid2 reference is bit depth 2, colour type 0 — an
        # uncommon PNG shape that a naive decoder gets wrong.
        img = image.decode(REFERENCE.read_bytes())
        self.assertEqual((img.width, img.height), (160, 144))
        shades = {img.pixels[i : i + 3] for i in range(0, len(img.pixels), 3)}
        self.assertEqual(len(shades), 4, "a DMG frame has exactly four shades")

    def test_png_ppm_and_raw_forms_of_the_same_frame_agree(self):
        ref = image.decode(REFERENCE.read_bytes())
        w, h = ref.width, ref.height

        as_png = image.decode(image.encode_png(ref))
        self.assertEqual(as_png, ref)

        p6 = b"P6\n%d %d\n255\n" % (w, h) + ref.pixels
        self.assertEqual(image.decode(p6), ref)

        # binjgb writes the ASCII form. Supporting it is why that emulator can
        # be scored at all.
        p3 = b"P3\n%d %d\n255\n" % (w, h) + b" ".join(
            str(v).encode() for v in ref.pixels
        )
        self.assertEqual(image.decode(p3), ref)

        rgba = bytearray()
        for i in range(0, len(ref.pixels), 3):
            rgba += ref.pixels[i : i + 3] + b"\xff"
        self.assertEqual(image.decode(bytes(rgba), expect=(w, h)), ref)

    def test_a_comment_and_odd_whitespace_in_a_pnm_header_are_tolerated(self):
        body = b"\x00\x00\x00\xff\xff\xff"
        p6 = b"P6\n# written by someone's emulator\n2  1\n255\n" + body
        self.assertEqual(image.decode(p6), image.Image(2, 1, body))

    def test_unreadable_input_is_an_error_not_a_crash(self):
        with self.assertRaises(image.ImageError):
            image.decode(b"definitely not an image", expect=(160, 144))


class ShadeComparison(unittest.TestCase):
    def test_different_palettes_of_the_same_picture_compare_equal(self):
        """The property the whole screen-check design rests on.

        This is not hypothetical: this project's own frames use
        000000/606060/c0c0c0/ffffff while the upstream reference image uses
        000000/555555/aaaaaa/ffffff, so a literal RGB comparison would fail
        every run.
        """
        grey = [b"\x00\x00\x00", b"\x55\x55\x55", b"\xaa\xaa\xaa", b"\xff\xff\xff"]
        green = [b"\x0f\x38\x0f", b"\x30\x62\x30", b"\x8b\xac\x0f", b"\x9b\xbc\x0f"]
        pattern = [0, 1, 2, 3, 3, 2, 1, 0, 1, 1, 3, 0]
        a = image.Image(4, 3, b"".join(grey[i] for i in pattern))
        b = image.Image(4, 3, b"".join(green[i] for i in pattern))
        differing, _ = image.compare(a, b)
        self.assertEqual(differing, 0)

    def test_a_real_difference_is_counted_and_located(self):
        ref = image.decode(REFERENCE.read_bytes())
        broken = bytearray(ref.pixels)
        # Invert a 3x2 block at (10, 20) — a plausible sprite-priority bug.
        for y in range(20, 22):
            for x in range(10, 13):
                o = (y * ref.width + x) * 3
                broken[o : o + 3] = bytes(255 - v for v in broken[o : o + 3])
        differing, mask = image.compare(image.Image(160, 144, bytes(broken)), ref)
        self.assertEqual(differing, 6)
        self.assertEqual(gbconform.bounding_box(mask, 160), (10, 20, 3, 2))

    def test_mismatched_geometry_is_reported_clearly(self):
        with self.assertRaises(image.ImageError) as e:
            image.compare(rgb_image(160, 143), rgb_image(160, 144))
        self.assertIn("160x143", str(e.exception))


class Badges(unittest.TestCase):
    def test_colour_tracks_the_score(self):
        self.assertEqual(gbconform.badge_colour(14, 14), "brightgreen")
        self.assertEqual(gbconform.badge_colour(13, 14), "green")
        self.assertEqual(gbconform.badge_colour(11, 14), "yellowgreen")
        self.assertEqual(gbconform.badge_colour(7, 14), "yellow")
        self.assertEqual(gbconform.badge_colour(1, 14), "orange")
        self.assertEqual(gbconform.badge_colour(0, 14), "red")
        self.assertEqual(gbconform.badge_colour(0, 0), "lightgrey")

    def test_endpoint_json_matches_the_shields_schema(self):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "badge.json"
            gbconform.write_badge_json(p, "conformance", 12, 14)
            b = json.loads(p.read_text())
        self.assertEqual(b["schemaVersion"], 1)
        self.assertEqual(b["label"], "conformance")
        # Meaningful, not pass/fail — the whole point of the badge.
        self.assertEqual(b["message"], "12/14")
        self.assertEqual(b["color"], "yellowgreen")

    def test_svg_is_well_formed_and_self_contained(self):
        with tempfile.TemporaryDirectory() as td:
            p = Path(td) / "badge.svg"
            gbconform.write_badge_svg(p, "gb & co <conformance>", 12, 14)
            text = p.read_text()
        root = ET.fromstring(text)  # raises if the escaping is wrong
        self.assertTrue(root.tag.endswith("svg"))
        self.assertIn("12/14", text)
        # No external request may be needed to render it.
        self.assertNotIn("http://", text.replace("http://www.w3.org/2000/svg", ""))


# ---------------------------------------------------------------------------
# End-to-end, with fake adapters
# ---------------------------------------------------------------------------

FAKE_ADAPTER = r'''#!/usr/bin/env python3
"""A stand-in emulator. MODE says how it behaves; no emulation involved."""
import os, sys
mode = os.environ["FAKE_MODE"]
if mode == "crash":
    sys.stderr.write("segmentation fault (not really)\n"); sys.exit(139)
serial_out = os.environ.get("GBCONFORM_SERIAL_OUT")
frame_out = os.environ.get("GBCONFORM_FRAME_OUT")
if serial_out and mode != "noserial":
    name = os.path.basename(os.environ["GBCONFORM_ROM"])
    verdict = "Passed" if mode == "pass" else "Failed #3"
    open(serial_out, "wb").write(("%s\n\n%s\n" % (name, verdict)).encode())
if frame_out:
    ref = os.environ["FAKE_REFERENCE"]
    data = open(ref, "rb").read()
    open(frame_out, "wb").write(data)
'''


class EndToEnd(unittest.TestCase):
    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.tmp = Path(self.td.name)
        self.addCleanup(self.td.cleanup)

        self.adapter = self.tmp / "fake_adapter.py"
        self.adapter.write_text(FAKE_ADAPTER)
        self.adapter.chmod(self.adapter.stat().st_mode | stat.S_IEXEC)

        # A two-check registry: one serial, one screen. Real ROM bytes are
        # never executed — the fake adapter only looks at the file name.
        self.roms = self.tmp / "roms"
        (self.roms / "blargg").mkdir(parents=True)
        (self.roms / "blargg" / "fake.gb").write_bytes(b"\x00" * 0x8000)
        (self.roms / "acid2").mkdir()
        (self.roms / "acid2" / "fake.gb").write_bytes(b"\x00" * 0x8000)
        ref_png = self.roms / "acid2" / "reference.png"
        ref_png.write_bytes(REFERENCE.read_bytes())
        # The fake adapter answers every frame request with the reference
        # itself, so a screen check passes unless we say otherwise.
        os.environ["FAKE_REFERENCE"] = str(ref_png)

        self.registry = self.tmp / "checks.json"
        self.registry.write_text(json.dumps({
            "contract": "gbconform/1",
            "suites": {
                "blargg": {"source": {"kind": "vendored", "dir": "blargg"}},
                "acid2": {"source": {"kind": "vendored", "dir": "acid2"}},
            },
            "checks": [
                {"id": "blargg/fake", "suite": "blargg", "rom": "fake.gb",
                 "method": "serial", "frames": 10,
                 "behaviour": "a stand-in", "explain": ["check your DAA"],
                 "hardware": "Pan Docs", "reference": "https://example.invalid"},
                {"id": "acid2/fake", "suite": "acid2", "rom": "fake.gb",
                 "reference_image": "reference.png", "method": "screen",
                 "frames": 10, "behaviour": "a stand-in", "explain": ["look"]},
            ],
        }))

    def run_runner(self, mode: str, extra: list[str] | None = None):
        out = self.tmp / f"out-{mode}-{len(list(self.tmp.iterdir()))}"
        os.environ["FAKE_MODE"] = mode
        code = gbconform.main([
            "--adapter", f"{sys.executable} {self.adapter}",
            "--checks", str(self.registry),
            "--roms-dir", str(self.roms),
            "--repo-root", str(self.roms),
            "--out", str(out),
            "--no-fetch",
        ] + (extra or []))
        return code, json.loads((out / "results.json").read_text()), out

    def test_a_passing_emulator_scores_full_marks(self):
        code, res, out = self.run_runner("pass")
        self.assertEqual(code, 0)
        self.assertEqual(res["score"], {"passed": 2, "total": 2,
                                        "percent": 100.0, "skipped": 0})
        self.assertEqual(json.loads((out / "badge.json").read_text())["message"],
                         "2/2")

    def test_a_failure_quotes_the_roms_own_verdict_and_explains_it(self):
        _, res, _ = self.run_runner("fail")
        serial = next(c for c in res["checks"] if c["method"] == "serial")
        self.assertEqual(serial["status"], "fail")
        # The sub-test number is the most actionable thing in the report.
        self.assertIn("Failed #3", serial["detail"])
        # ...and the WHY travels with it. That is the differentiator.
        self.assertEqual(serial["explain"], ["check your DAA"])
        self.assertEqual(serial["hardware"], "Pan Docs")

    def test_a_passing_check_does_not_carry_the_failure_explanation(self):
        _, res, _ = self.run_runner("pass")
        self.assertNotIn("explain", res["checks"][0])

    def test_a_missing_capability_skips_rather_than_fails(self):
        _, res, _ = self.run_runner("noserial")
        serial = next(c for c in res["checks"] if c["method"] == "serial")
        self.assertEqual(serial["status"], "skipped")
        self.assertIn("no serial output", serial["detail"])
        # Skipped checks leave the score honest: 1 of 1 attempted, not 1 of 2.
        self.assertEqual(res["score"]["total"], 1)
        self.assertEqual(res["score"]["skipped"], 1)
        self.assertFalse(res["capabilities"]["serial"]["available"])

    def test_an_adapter_that_crashes_is_an_error_everywhere_not_a_skip(self):
        """The hole the capability probe exists to close.

        Without the probe, an emulator that crashes on every ROM would produce
        no output files, look like it 'has no capabilities', and score 0/0 —
        which a badge would render as a clean slate. It must score 0.
        """
        _, res, _ = self.run_runner("crash")
        self.assertEqual(res["score"]["total"], 2)
        self.assertEqual(res["score"]["passed"], 0)
        for c in res["checks"]:
            self.assertEqual(c["status"], "error")
            self.assertIn("exited 139", c["detail"])

    def test_fail_below_gates_the_build(self):
        code, res, _ = self.run_runner("fail", ["--fail-below", "100"])
        self.assertEqual(code, 1)
        code, _, _ = self.run_runner("fail", ["--fail-below", "40"])
        self.assertEqual(code, 0)  # 50% (the screen check still passes)

    def test_the_summary_names_every_failure_and_its_reason(self):
        _, _, out = self.run_runner("fail")
        summary = (out / "summary.md").read_text()
        self.assertIn("blargg/fake", summary)
        self.assertIn("Why these failed", summary)
        self.assertIn("check your DAA", summary)

    def test_artifacts_are_written_for_inspection(self):
        _, res, out = self.run_runner("fail")
        names = {p.name for p in (out / "artifacts").iterdir()}
        self.assertIn("blargg_fake.serial.txt", names)
        self.assertIn("acid2_fake.actual.png", names)


class ScreenTextFixture:
    """A synthetic Blargg-shaped cartridge, shared by the cases below.

    Everything here is synthetic on purpose. Blargg's ROMs carry no explicit
    licence and are fetched at run time, never vendored, so these tests build
    their own cartridge: a font laid out the way `common/print.s` lays one out,
    a message, and a frame rendered from the two. That proves the algorithm
    rather than one particular ROM, and it keeps the test suite runnable with
    no network and no emulator.
    """

    FIRST = 0x20   # the font covers ASCII 0x20.. as Blargg's does
    COUNT = 0x60

    def font(self):
        """A distinct 8x8 bitmap per character; space is blank."""
        out = []
        for i in range(self.COUNT):
            if i == 0:
                out.append(bytes(8))
                continue
            # Vary every row so no two glyphs collide and none is all-zero.
            out.append(bytes(((i * 7 + r * 29) & 0xFF) | 0x81 for r in range(8)))
        return out

    def cartridge(self, message, *, pad=0x13):
        """A fake ROM: filler, the font at an arbitrary alignment, the text."""
        font = self.font()
        base = bytearray(b"\x00" * pad)
        base += b"".join(font)
        base += message.replace("\n", " ").encode("latin-1")
        return bytes(base), pad

    def frame(self, lines, font, *, invert=()):
        """Render 20x18 cells of text into a 160x144 two-shade image."""
        px = bytearray(b"\xff" * (160 * 144 * 3))
        for cy, line in enumerate(lines):
            for cx, ch in enumerate(line):
                g = font[ord(ch) - self.FIRST]
                if (cx, cy) in invert:
                    g = bytes(b ^ 0xFF for b in g)
                for y in range(8):
                    for x in range(8):
                        if g[y] & (1 << (7 - x)):
                            o = ((cy * 8 + y) * 160 + cx * 8 + x) * 3
                            px[o:o + 3] = b"\x00\x00\x00"
        return image.Image(160, 144, bytes(px))


class ScreenText(ScreenTextFixture, unittest.TestCase):
    """The decoder itself."""

    def test_the_character_table_comes_from_the_rom_not_from_us(self):
        lines = ["halt bug", "", "01 10 F1 0C04", "", "Passed"]
        rom, _ = self.cartridge(" ".join(lines))
        img = self.frame(lines, self.font())
        reading = screentext.read(img, rom, image.quantise_shades)
        self.assertEqual(reading.text.split("\n")[:5], lines)
        self.assertEqual(screentext.verdict(reading.text)[0], "pass")

    def test_a_failing_run_quotes_the_line_the_rom_printed(self):
        lines = ["oam_bug", "01:ok  02:02", "Failed #2"]
        rom, _ = self.cartridge(" ".join(lines))
        reading = screentext.read(self.frame(lines, self.font()), rom,
                                  image.quantise_shades)
        status, detail = screentext.verdict(reading.text)
        self.assertEqual(status, "fail")
        self.assertIn("Failed #2", detail)

    def test_inverse_video_is_read_not_rejected(self):
        # oam_bug prints a failing sub-test's code white-on-black. Such a cell
        # is the glyph's exact complement, and dropping it would lose the one
        # number the report is about.
        lines = ["oam_bug", "01:ok  02:02", "Failed #2"]
        rom, _ = self.cartridge(" ".join(lines))
        img = self.frame(lines, self.font(), invert={(7, 1), (8, 1), (10, 1), (11, 1)})
        reading = screentext.read(img, rom, image.quantise_shades)
        self.assertEqual(reading.text.split("\n")[1], lines[1])
        # Distinct glyph shapes, not cells: `02` twice is two shapes.
        self.assertEqual(reading.inverted, 2)

    def test_a_frame_not_drawn_with_this_roms_font_is_refused(self):
        # Refusing is the whole safety property: a screen we cannot read must
        # produce "no verdict", never a guessed one.
        lines = ["Passed"]
        rom, _ = self.cartridge(" ".join(lines))
        other = [bytes((b + 1) & 0xFF for b in g) for g in self.font()]
        with self.assertRaises(screentext.ScreenTextError):
            screentext.read(self.frame(lines, other), rom, image.quantise_shades)

    def test_a_blank_screen_is_refused_rather_than_read_as_a_failure(self):
        rom, _ = self.cartridge("Passed")
        with self.assertRaises(screentext.ScreenTextError):
            screentext.read(rgb_image(160, 144, b"\xff\xff\xff"), rom,
                            image.quantise_shades)

    def test_an_unfinished_run_is_not_a_pass(self):
        lines = ["interrupt time"]
        rom, _ = self.cartridge(" ".join(lines))
        reading = screentext.read(self.frame(lines, self.font()), rom,
                                  image.quantise_shades)
        self.assertEqual(screentext.verdict(reading.text)[0], "unfinished")

    def test_a_scrolled_console_is_still_read(self):
        # A ROM that has scrolled its background by a non-multiple of 8 puts
        # the cell grid off the frame's own 8x8 lattice.
        lines = ["mem_timing", "01:ok  02:ok", "Passed"]
        rom, _ = self.cartridge(" ".join(lines))
        flat = self.frame(lines, self.font())
        shifted = bytearray(b"\xff" * len(flat.pixels))
        dx, dy = 3, 5
        for y in range(144 - dy):
            for x in range(160 - dx):
                s = (y * 160 + x) * 3
                d = ((y + dy) * 160 + x + dx) * 3
                shifted[d:d + 3] = flat.pixels[s:s + 3]
        reading = screentext.read(image.Image(160, 144, bytes(shifted)), rom,
                                  image.quantise_shades)
        self.assertEqual(reading.origin, (dx, dy))
        self.assertEqual(screentext.verdict(reading.text)[0], "pass")


SCREEN_TEXT_ADAPTER = r'''#!/usr/bin/env python3
"""Answers a frame request with a pre-rendered picture chosen by MODEL."""
import os, shutil, sys
frame_out = os.environ.get("GBCONFORM_FRAME_OUT")
if not frame_out:
    sys.exit(0)                       # declines the serial capability
shutil.copyfile(os.environ["FAKE_FRAME_" + os.environ["GBCONFORM_MODEL"].upper()],
                frame_out)
'''


class ScreenTextEndToEnd(ScreenTextFixture, unittest.TestCase):
    """The `screen-text` method through `main()`, including the model pin.

    The unit cases above prove the decoder; this proves the wiring — that the
    method reaches the right judge, that it needs only the SCREEN capability,
    and that a check pinning its console actually gets that console.
    """

    def setUp(self):
        self.td = tempfile.TemporaryDirectory()
        self.tmp = Path(self.td.name)
        self.addCleanup(self.td.cleanup)

        dmg = ["fake rom", "01:01", "Failed #1"]
        cgb = ["fake rom", "01:ok", "Passed"]
        rom, _ = self.cartridge(" ".join(dmg + cgb))
        roms = self.tmp / "roms" / "blargg"
        roms.mkdir(parents=True)
        (roms / "fake.gb").write_bytes(rom)
        for name, lines in (("DMG", dmg), ("CGB", cgb)):
            p = self.tmp / f"frame-{name}.png"
            p.write_bytes(image.encode_png(self.frame(lines, self.font())))
            os.environ[f"FAKE_FRAME_{name}"] = str(p)

        self.adapter = self.tmp / "adapter.py"
        self.adapter.write_text(SCREEN_TEXT_ADAPTER)
        self.registry = self.tmp / "checks.json"
        self.registry.write_text(json.dumps({
            "contract": "gbconform/1",
            "suites": {"blargg": {"source": {"kind": "vendored", "dir": "blargg"}}},
            "checks": [{
                "id": "blargg/fake", "suite": "blargg", "rom": "fake.gb",
                "method": "screen-text", "model": "cgb", "frames": 10,
                "behaviour": "a stand-in", "explain": ["look here"],
                "hardware": "Pan Docs",
            }],
        }))

    def run_runner(self, extra=None):
        out = self.tmp / f"out{len(list(self.tmp.iterdir()))}"
        code = gbconform.main([
            "--adapter", f"{sys.executable} {self.adapter}",
            "--checks", str(self.registry),
            "--roms-dir", str(self.tmp / "roms"),
            "--repo-root", str(self.tmp / "roms"),
            "--out", str(out), "--no-fetch",
        ] + (extra or []))
        return code, json.loads((out / "results.json").read_text()), out

    def test_a_pinned_check_gets_its_own_console_whatever_model_says(self):
        # --model dmg would make this ROM report Failed. The pin must win:
        # `oam_bug` and `interrupt_time` are only meaningful on one machine
        # each, so a global --model cannot be allowed to overrule them.
        _, res, out = self.run_runner(["--model", "dmg"])
        check = res["checks"][0]
        self.assertEqual(check["model"], "cgb")
        self.assertEqual(check["status"], "pass")
        self.assertIn("Passed", check["evidence"]["screen"])
        # The decoded report is kept for a human, not just summarised.
        self.assertIn("blargg_fake.screen.txt",
                      {p.name for p in (out / "artifacts").iterdir()})

    def test_it_rides_the_screen_capability_and_needs_no_serial(self):
        _, res, _ = self.run_runner()
        self.assertTrue(res["capabilities"]["screen"]["available"])
        # The adapter declines serial and is not asked for it: the only check
        # here is screen-text, so no serial probe should even be attempted.
        self.assertNotIn("serial", res["capabilities"])
        self.assertEqual(res["score"]["skipped"], 0)

    def test_an_unreadable_frame_is_an_error_not_an_invented_verdict(self):
        os.environ["FAKE_FRAME_CGB"] = str(self.tmp / "blank.png")
        (self.tmp / "blank.png").write_bytes(
            image.encode_png(rgb_image(160, 144, b"\xff\xff\xff"))
        )
        _, res, _ = self.run_runner()
        check = res["checks"][0]
        self.assertEqual(check["status"], "error")
        self.assertIn("could not read the ROM's report", check["detail"])
        # An error still carries the explanation — a crash on one ROM is often
        # caused by the very behaviour under test.
        self.assertEqual(check["explain"], ["look here"])


class Contract(unittest.TestCase):
    def test_the_contract_is_printable_without_an_adapter(self):
        # `--contract` is the thing an emulator author reads first; it must
        # work with nothing built and nothing configured.
        proc = subprocess.run(
            [sys.executable, str(HERE / "gbconform.py"), "--contract"],
            capture_output=True, text=True, check=True,
        )
        self.assertIn("GBCONFORM_SERIAL_OUT", proc.stdout)
        self.assertIn("GBCONFORM_FRAME_OUT", proc.stdout)

    def test_the_shipped_registry_is_well_formed(self):
        reg = json.loads((HERE / "checks.json").read_text())
        self.assertEqual(reg["contract"], gbconform.CONTRACT)
        ids = set()
        for c in reg["checks"]:
            self.assertNotIn(c["id"], ids, "check ids must be unique")
            ids.add(c["id"])
            self.assertIn(c["suite"], reg["suites"])
            self.assertIn(c["method"], gbconform.CAPABILITY_FOR_METHOD)
            # Every check must be able to explain itself. This is the
            # project's differentiator, so it is enforced, not hoped for.
            self.assertTrue(c["behaviour"], f"{c['id']} has no behaviour")
            self.assertTrue(c["explain"], f"{c['id']} has no explanation")
            self.assertTrue(c["hardware"], f"{c['id']} cites no hardware source")
            if c["method"] == "screen":
                self.assertIn("reference_image", c)
            # A `screen-text` check reads the ROM's own printed report, so it
            # must NOT carry a reference image — the whole point is that no
            # expected picture of ours is involved.
            if c["method"] == "screen-text":
                self.assertNotIn("reference_image", c)
            if "model" in c:
                self.assertIn(c["model"], ("dmg", "cgb"))

    def test_no_check_expects_a_reference_frame_produced_here(self):
        """The guiding principle, made mechanical.

        Only ONE suite in the registry is allowed to be judged against a
        picture, and that picture must be its own author's, shipped with the
        ROM under its own licence. Everything else is judged by what the ROM
        itself printed. A future check that quietly pins a frame rendered by
        this emulator would make this project the authority for its own
        conformance suite; this test is what stops that happening by accident.
        """
        reg = json.loads((HERE / "checks.json").read_text())
        for c in reg["checks"]:
            if "reference_image" not in c:
                continue
            src = reg["suites"][c["suite"]]["source"]
            self.assertEqual(
                src["kind"], "vendored",
                f"{c['id']} is judged against an image, so its suite must ship "
                "that image itself",
            )
            self.assertIn(
                "MIT", " ".join(src["reason"]),
                f"{c['id']}'s reference image must come with a licence that "
                "permits redistributing it",
            )


if __name__ == "__main__":
    unittest.main()

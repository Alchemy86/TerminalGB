#!/usr/bin/env python3
"""Encode a swarm film's frames — the shared recipe, and the bake-off behind it.

    tools/pewter-chain/encode_film.py                       # what this is
    tools/pewter-chain/encode_film.py encode FRAMES --out F.mp4 --fps 60
    tools/pewter-chain/encode_film.py bakeoff FRAMES --frames 240

## Why a film of this had to stop being encoded like camera footage

The source is **pixel art on a fixed tile grid**: four Game Boy shades per
sprite, flat colour, hard edges, a small palette, and long stretches where
nothing on screen moves at all. Every published film here was encoded with
`libopenh264 -b:v 6M`, which is the opposite of all four of those:

* **`libopenh264` cannot encode anything but `yuv420p`** — checked, its own
  `Supported pixel formats` line reads `yuv420p yuvj420p`, so 4:2:0 was never a
  setting anybody chose, it was the encoder's ceiling. 4:2:0 stores one chroma
  sample per 2x2 block of luma, so **a colour edge is halved in resolution
  before a single bit is spent on it**. On camera footage that is invisible;
  on a one-pixel-wide sprite outline against a flat floor it is the blur the
  maintainer reported, and no bitrate removes it.
* **`-b:v 6M` is a bitrate target, not a quality target.** It spends 6 Mbps on a
  frame where nothing moved and still cannot afford the hard edges on a frame
  where the whole camera panned. Constant-quality rate control is what flat,
  hugely-compressible content wants: it asks for a *quality* and lets a still
  frame cost almost nothing.

Both are fixed by changing encoder, not by turning the bitrate up. `bakeoff`
measures the alternatives on real frames and prints the table; `encode` is the
winner, wired up so every future film gets it without anybody re-deriving it.

## How sharpness is measured here, and why not PSNR

PSNR and SSIM are averages over a picture, and an average is exactly the wrong
instrument for "did the encoder invent a colour". This repository already has
the right one and uses it in three other places (`screen::halfblocks`, the
terminal capture checks, the upscaler): **a Game Boy frame contains a small,
known set of colours, so a colour on screen that is not one of them is a
CATEGORICAL error with no threshold to tune.** So a recipe is scored on

* **exact** — the share of pixels whose RGB is byte-identical to the source's,
* **new colours** — how many colours the decode contains that the source frame
  did not, which is blur counted rather than described,

with PSNR printed beside them for anybody who wants the conventional number.
"""
import argparse
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from chainrender import axi  # noqa: E402
from chainrender.axi import die  # noqa: E402


# **The recipes, and every flag in them is load-bearing.**
#
# `-pix_fmt` first, because that is the finding: a 4:2:0 recipe cannot be
# sharp however many bits it is given. `-crf` with `-b:v 0` is constant
# quality, which is what lets a static stretch cost nothing. `-tune-content
# screen` / `tune-content=screen` tell libvpx and libaom that the content is
# flat regions with hard edges rather than a photograph, which is literally
# what this is. `-g` is long because a swarm film holds still for seconds at a
# time and a keyframe every second throws that away.
#
# Nothing here scales, crops or filters: the frames are written at the output
# resolution by `swarm_film.py` with `Image.NEAREST`, so any `-vf scale` would
# be a second resample and the first thing to soften the picture again.
# **COLOUR IS DECLARED, NEVER LEFT TO THE PLAYER TO GUESS.**
#
# The frames are RGB PNGs and every YUV codec has to convert them, which needs
# a matrix — and swscale picks one silently while the encoder tags the stream
# with nothing at all. A published Mt Moon cut carried `color_space=unknown /
# color_primaries=unknown / color_transfer=unknown`, exactly as the old
# `libopenh264` films did, and a player then has to guess. So the conversion
# and the tag are stated together, as one pair, and they say the same thing:
# convert from full-range RGB to limited-range BT.709, and mark the stream
# BT.709 limited. A player that reads the tags and a player that assumes
# BT.709 (which is what most of them do at 720p) then agree.
#
# `scale` here is a COLOUR conversion and nothing else — the input and output
# dimensions are identical, so it resamples no pixel. Nothing in this file may
# ever change the picture's size; that is the whole finding above.
_SCALE = ("scale=in_range=full:in_color_matrix=bt709:"
          "out_range=tv:out_color_matrix=bt709")
_TAGS = ["-colorspace", "bt709", "-color_primaries", "bt709",
         "-color_trc", "bt709", "-color_range", "tv"]


def yuv_colour(drop=1):
    """The colour conversion and its tag, optionally dropping frames first.

    **`drop` DECIMATES; it never resamples.** Taking every second frame halves
    a file and leaves every remaining pixel exactly where it was, which is the
    one size lever available that does not touch the picture — and the whole of
    the finding above is that touching the picture is what costs the sharpness.
    It is only legitimate while the resulting stride still clears
    `walk_span` (see `swarm_film.py`), i.e. while an agent still moves less
    than one walk step per output frame.
    """
    vf = _SCALE if drop <= 1 else f"select=not(mod(n\\,{drop})),{_SCALE}"
    return ["-vf", vf] + _TAGS


YUV_COLOUR = yuv_colour()
# The RGB-plane recipe never converts, so it carries primaries and range only.
RGB_COLOUR = ["-color_primaries", "bt709", "-color_trc", "bt709",
              "-color_range", "pc"]

# **A FILM NOBODY CAN PLAY IS NOT A FILM — AND THIS COST A SHIPPED FILM.**
#
# The 4:2:0 finding above sent this tool to VP9 at `yuv444p`, on the reasoning
# that full-resolution chroma is what a four-shade palette wants. It is, and it
# is unshippable: **`yuv444p` VP9 is VP9 Profile 1, which most players and
# essentially all hardware decoders do not implement.** A decoder that does not
# implement it does not refuse the file — it reads the planes as 4:2:0 anyway
# and produces exactly what was reported off the maintainer's own player: layout
# and text perfect, and the atlas's greys rendered green, pink and orange. AV1
# at 4:4:4 is its Professional profile and carries the same trap.
#
# **AND CHECKING IT WITH FFMPEG PROVED NOTHING**, which is the lesson worth
# more than the fix: ffmpeg decodes Profile 1 correctly, so an encode-then-
# decode round trip through the same library reports a perfect picture. So
# does Chrome — measured, pixel for pixel. A compatibility fault is only
# visible in a decoder that lacks the feature, and the deliverable therefore
# has to be a format that needs no feature: **H.264 High profile, `yuv420p`,
# `+faststart`** — the same format as every film that has ever played here.
#
# Two consequences, and they are the shape of the whole page:
#
# * **Compatibility beats chroma fidelity.** A film the maintainer can open on a
#   phone and on a GNOME desktop is the product; a sharper file he cannot
#   watch is worth nothing.
# * **Sharpness comes from INTEGER-SCALE RENDERING, not from exotic chroma.**
#   Rule 8 in `chainrender/camera.py` is what removed the softness and two
#   thirds of the bitrate, and it does that at any chroma. Measured on the
#   same frames: 4:2:0 with modern rate control is visually indistinguishable
#   from 4:4:4 at this zoom, while 4:2:0 with the old `libopenh264 -b:v 6M`
#   is mush — so the defect was never the chroma, it was the rate control.
#
# A 4:4:4 or lossless recipe is therefore a MASTER, kept for archival and for
# the bake-off, and `encode` refuses one unless `--master` says so out loud.

RECIPES = {
    "h264-high": dict(
        note="THE DEFAULT: H.264 High + CABAC, 4:2:0, faststart — the format "
             "every player and every hardware decoder handles",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libopenh264", "-profile", "high", "-coder", "1",
              "-b:v", "6M"]),
    "openh264-6M": dict(
        note="the recipe every published film used: H.264 BASELINE, no CABAC",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libopenh264", "-b:v", "6M"]),
    "openh264-2M": dict(
        note="the same encoder asked for less; shows bitrate is not the lever",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libopenh264", "-b:v", "2M"]),
    "vp9-420": dict(
        note="VP9 Profile 0 — smaller and sharper than H.264 here, but VP9 "
             "in an mp4 is not what every player expects, so it is not the "
             "deliverable",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libvpx-vp9", "-crf", "24", "-b:v", "0", "-row-mt", "1",
              "-cpu-used", "2", "-tune-content", "screen", "-g", "240"]),
    "vp9-420-crf28": dict(
        note="the same, cheaper — where the bake-off's 4:2:0 row was measured",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libvpx-vp9", "-crf", "28", "-b:v", "0", "-row-mt", "1",
              "-cpu-used", "2", "-tune-content", "screen", "-g", "240"]),
    "vp9-444": dict(
        note="MASTER ONLY — VP9 Profile 1; players mangle the chroma",
        ext="mp4", pix="yuv444p",
        args=["-c:v", "libvpx-vp9", "-crf", "34", "-b:v", "0", "-row-mt", "1",
              "-cpu-used", "2", "-tune-content", "screen", "-g", "240"]),
    "vp9-444-crf28": dict(
        note="MASTER ONLY — Profile 1 again, at a higher quality target",
        ext="mp4", pix="yuv444p",
        args=["-c:v", "libvpx-vp9", "-crf", "28", "-b:v", "0", "-row-mt", "1",
              "-cpu-used", "2", "-tune-content", "screen", "-g", "240"]),
    "vp9-444-crf20": dict(
        note="MASTER ONLY — Profile 1 again, near-transparent",
        ext="mp4", pix="yuv444p",
        args=["-c:v", "libvpx-vp9", "-crf", "20", "-b:v", "0", "-row-mt", "1",
              "-cpu-used", "2", "-tune-content", "screen", "-g", "240"]),
    "vp9-lossless-rgb": dict(
        note="MASTER ONLY — mathematically exact RGB planes, no conversion",
        ext="webm", pix="gbrp",
        args=["-c:v", "libvpx-vp9", "-lossless", "1", "-row-mt", "1",
              "-cpu-used", "4", "-tune-content", "screen", "-g", "240"]),
    "av1-420": dict(
        note="SVT-AV1, 4:2:0 — the smallest thing here that still plays widely",
        ship=True,
        ext="mp4", pix="yuv420p",
        args=["-c:v", "libsvtav1", "-crf", "30", "-preset", "6",
              "-svtav1-params", "tune=0", "-g", "240"]),
    "aom-444-crf24": dict(
        note="MASTER ONLY — the archival end of the AV1 dial",
        ext="mp4", pix="yuv444p",
        args=["-c:v", "libaom-av1", "-crf", "24", "-b:v", "0", "-cpu-used", "6",
              "-row-mt", "1", "-aom-params", "tune-content=screen", "-g", "240"]),
    "aom-444": dict(
        note="MASTER ONLY — AV1 Professional profile, same trap as VP9 profile 1",
        ext="mp4", pix="yuv444p",
        args=["-c:v", "libaom-av1", "-crf", "30", "-b:v", "0", "-cpu-used", "6",
              "-row-mt", "1", "-aom-params", "tune-content=screen", "-g", "240"]),
}

# The one a film gets when nobody says otherwise, and the one the mobile copy
# gets. Both are decided by `bakeoff`, not by taste — see docs/agent/film-encoding.md.
DEFAULT = "h264-high"
MOBILE = "h264-high-mobile"
RECIPES[MOBILE] = dict(
    note="a smaller copy for a phone. THE PICTURE IS NOT SCALED — resampling "
         "pixel art is what costs the sharpness — so it lowers the bitrate "
         "and drops every second FRAME instead, which is the one lever that "
         "leaves every remaining pixel exactly where it was",
    ship=True, drop=2,
    ext="mp4", pix="yuv420p",
    args=["-c:v", "libopenh264", "-profile", "high", "-coder", "1",
          "-b:v", "2M"])


def frame_glob(d):
    """The frame pattern in a render directory, and how many there are.

    `swarm_film.py` writes `fNNNNN.png`, `path_arrows.py` writes `aNNNNN.png`
    and `screen_mosaic.py` writes `mNNNNN.png`. One tool, three producers, so
    the pattern is discovered rather than assumed."""
    if not os.path.isdir(d):
        die(f"no such directory: {d}", "run swarm_film.py first; its --out is "
                                        "the directory of frames")
    names = sorted(f for f in os.listdir(d) if f.endswith(".png"))
    if not names:
        die(f"no .png frames in {d}", "render first: tools/pewter-chain/swarm_film.py TRACKS --out " + d)
    m = re.match(r"([a-z]*)(\d+)\.png$", names[0])
    if not m:
        die(f"frames are not NNNNN.png: {names[0]}")
    return os.path.join(d, f"{m.group(1)}%0{len(m.group(2))}d.png"), len(names)


def encode(frames_dir, out, fps, recipe, start=0, count=None, quiet=False):
    """Run one recipe. Returns (seconds, bytes)."""
    r = RECIPES.get(recipe)
    if r is None:
        die(f"no recipe named {recipe!r}",
            f"recipes: {', '.join(sorted(RECIPES))}",
            "tools/pewter-chain/encode_film.py help recipes", code=2)
    pat, _ = frame_glob(frames_dir)
    cmd = ["ffmpeg", "-y", "-v", "error", "-framerate", str(fps)]
    if start:
        cmd += ["-start_number", str(start)]
    cmd += ["-i", pat]
    if count:
        cmd += ["-frames:v", str(count)]
    cmd += r["args"] + ["-pix_fmt", r["pix"]]
    if r["pix"] == "gbrp":
        cmd += RGB_COLOUR
    else:
        cmd += yuv_colour(r.get("drop", 1))
        if r.get("drop", 1) > 1:
            cmd += ["-r", str(round(fps / r["drop"]))]
    # `+faststart` puts the index at the front so a browser can begin playing
    # before the whole file has arrived. Free, and only meaningful for mp4.
    if r["ext"] == "mp4":
        # `write_colr` is what actually emits mp4's `colr` box. Without it the
        # colour flags above reach the bitstream and NOT the container, so a
        # player that reads the container (most of them) still sees nothing —
        # measured: `color_space=bt709` but `color_primaries=unknown`.
        # `faststart` puts the index first so a browser can start playing
        # before the whole file has arrived.
        cmd += ["-movflags", "+faststart+write_colr"]
    cmd += [out]
    t0 = time.time()
    p = subprocess.run(cmd, capture_output=True, text=True)
    if p.returncode != 0:
        die(f"ffmpeg failed for recipe {recipe}",
            (p.stderr.strip().splitlines() or ["?"])[-1],
            "ffmpeg " + " ".join(cmd[1:]))
    return time.time() - t0, os.path.getsize(out)


def decode_sample(path, out_dir, step):
    """Pull every `step`th frame back out of an encode, as PNGs.

    A recipe is judged on what a viewer receives, so the comparison is made
    against the DECODE and never against the encoder's own reported statistics.
    Only the sampled frames are decoded — a whole-film bake-off across eight
    recipes would otherwise write tens of thousands of PNGs to compare eight.
    """
    os.makedirs(out_dir, exist_ok=True)
    p = subprocess.run(["ffmpeg", "-y", "-v", "error", "-i", path,
                        "-vf", f"select=not(mod(n\\,{step}))", "-vsync", "0",
                        os.path.join(out_dir, "d%05d.png")],
                       capture_output=True, text=True)
    got = sorted(os.listdir(out_dir))
    if p.returncode != 0 and not got:
        die(f"could not decode {path}",
            (p.stderr.strip().splitlines() or ["?"])[-1],
            "a file still being written decodes as truncated — wait for the "
            "encode to finish")
    return [os.path.join(out_dir, f) for f in got]


def compare(src_paths, dec_paths):
    """exact-pixel share, colours the decode invented, and PSNR.

    **The categorical measure, not an average.** A Game Boy frame drawn over
    PixelGB's atlas contains a bounded set of colours; a decode that contains a
    colour the source did not is blurred, and it is blurred whatever its PSNR
    says. All three are returned so a recipe cannot pass on one alone — and
    PSNR is computed here, over the same sampled frames, rather than through a
    second ffmpeg pass whose frame alignment is one more thing to get wrong.
    """
    from PIL import Image
    exact = total = 0
    invented = 0
    sq = 0
    for s, d in zip(src_paths, dec_paths):
        a = Image.open(s).convert("RGB")
        b = Image.open(d).convert("RGB")
        if a.size != b.size:
            die(f"decoded frame is {b.size}, source is {a.size} — something "
                "in the recipe is scaling",
                "a film's frames are already at the output resolution; "
                "no -vf scale belongs in the command")
        pa, pb = a.tobytes(), b.tobytes()
        exact += sum(1 for i in range(0, len(pa), 3)
                     if pa[i:i + 3] == pb[i:i + 3])
        total += len(pa) // 3
        sq += sum((x - y) ** 2 for x, y in zip(pa, pb))
        invented += len(set(zip(pb[0::3], pb[1::3], pb[2::3]))
                        - set(zip(pa[0::3], pa[1::3], pa[2::3])))
    mse = sq / max(1, total * 3)
    db = "inf" if mse == 0 else round(10 * math.log10(255.0 ** 2 / mse), 2)
    return (exact / total if total else 0.0), invented, db


def cmd_bakeoff(a):
    pat, have = frame_glob(a.frames)
    n = min(a.frames_n, have)
    start = a.start
    print(f"bakeoff: {n} frame(s) from {a.frames} at {a.fps} fps, "
          f"{len(a.recipes)} recipe(s)")
    print()
    work = tempfile.mkdtemp(prefix="encode-bakeoff-")
    # The reference frames are read where they are — a whole-film bake-off has
    # no business copying a gigabyte of PNGs to index them. `step` is shared
    # with the decode side so the same positions are compared on both.
    names = sorted(f for f in os.listdir(a.frames) if f.endswith(".png"))
    sample = min(a.sample, n)
    step = max(1, n // sample)
    src_sample = [os.path.join(a.frames, names[start + i * step])
                  for i in range(sample) if start + i * step < len(names)]
    rows = []
    for name in a.recipes:
        r = RECIPES[name]
        out = os.path.join(work, f"{name}.{r['ext']}")
        secs, size = encode(a.frames, out, a.fps, name, start=start, count=n)
        dec = os.path.join(work, f"dec-{name}")
        dec_sample = decode_sample(out, dec, step)[:len(src_sample)]
        ex, inv, db = compare(src_sample, dec_sample)
        rows.append(dict(
            recipe=name, chroma=r["pix"], ext=r["ext"],
            MB=round(size / 1e6, 2),
            kbps=round(size * 8 / (n / a.fps) / 1000),
            exact_pct=round(ex * 100, 2),
            new_colours=inv,
            psnr=db,
            encode_s=round(secs, 1),
        ))
        if a.keep:
            shutil.copy(out, os.path.join(a.keep, os.path.basename(out)))
            # A still crop, so sharpness can be looked at and not only read.
            crop = os.path.join(a.keep, f"crop-{name}.png")
            crop_still(dec_sample[len(dec_sample) // 2], crop, a.crop)
    axi.table("encode", rows, ["recipe", "chroma", "ext", "MB", "kbps",
                               "exact_pct", "new_colours", "psnr", "encode_s"])
    print()
    base = next((r for r in rows if r["recipe"] == "openh264-6M"), None)
    if base:
        for r in rows:
            if r["recipe"] != "openh264-6M" and r["MB"]:
                print(f"  {r['recipe']:18s} {base['MB'] / r['MB']:5.1f}x smaller "
                      f"than openh264-6M, exact pixels "
                      f"{base['exact_pct']:.2f}% -> {r['exact_pct']:.2f}%")
    if a.keep:
        print()
        print(f"clips and zoomed crops kept in {a.keep}")
    print()
    axi.helps([
        f"encode the whole film: encode_film.py encode {a.frames} --out FILM.mp4 --fps {a.fps}",
        "read the argument: docs/agent/film-encoding.md",
    ])
    shutil.rmtree(work, ignore_errors=True)
    return 0


def crop_still(path, out, spec):
    """A zoomed crop of one decoded frame, so sharpness can be SEEN.

    Nearest-neighbour on the way up, obviously: a smooth magnifier would put
    back exactly the softness being looked for."""
    from PIL import Image
    x, y, w, h, z = (int(v) for v in spec.split(","))
    im = Image.open(path).convert("RGB")
    x, y = min(x, im.width - w), min(y, im.height - h)
    im.crop((x, y, x + w, y + h)).resize((w * z, h * z), Image.NEAREST).save(out)



def verify_colour(path, frames_dir, fps):
    """**Look at what came back out, the way a PLAYER would look at it.**

    Two independent things have to hold and neither implies the other.

    1. The stream is TAGGED. `color_space`, `color_primaries` and
       `color_transfer` must all be present. An untagged stream leaves the
       matrix to the player to guess, and a wrong guess is a hue shift over
       the whole picture.
    2. A player that IGNORES the tags and simply assumes BT.709 — which is
       what most of them do at 720p — still gets the right colours. That is
       checked by decoding under a forced BT.709 interpretation and comparing
       against the source frames.

    **An ffmpeg-encodes / ffmpeg-decodes round trip cannot find either fault**,
    because ffmpeg applies the same silent default in both directions and the
    error cancels. That is exactly how an untagged film shipped. So this
    forces the interpretation rather than accepting ffmpeg's.
    """
    q = subprocess.run(
        ["ffprobe", "-v", "error", "-select_streams", "v:0", "-show_entries",
         "stream=profile,pix_fmt,color_space,color_primaries,color_transfer,"
         "color_range", "-of", "default=nw=1", path],
        capture_output=True, text=True)
    tags = dict(l.split("=", 1) for l in q.stdout.strip().splitlines() if "=" in l)
    # **The matrix and the range are the two that corrupt a picture when they
    # are guessed**, and they are the two every encoder here can actually
    # carry. Primaries and transfer are asked for on every encode (see
    # YUV_COLOUR) and **no encoder in this ffmpeg build writes them into an
    # mp4** — measured on libvpx-vp9, libsvtav1 and libopenh264 alike, with
    # and without `-movflags write_colr`. They are reported and not gated,
    # because getting them wrong shifts colour management rather than reading
    # the chroma planes wrongly, and every player defaults to the values we
    # ask for anyway.
    missing = [k for k in ("color_space", "color_range")
               if tags.get(k, "unknown") in ("unknown", "", None)]
    names = sorted(f for f in os.listdir(frames_dir) if f.endswith(".png"))
    n = len(names)
    step = max(1, n // 6)
    work = tempfile.mkdtemp(prefix="encode-verify-")
    subprocess.run(
        ["ffmpeg", "-y", "-v", "error", "-i", path,
         "-vf", f"select=not(mod(n\\,{step})),"
                "scale=in_color_matrix=bt709:in_range=tv:out_range=full",
         "-vsync", "0", os.path.join(work, "v%05d.png")],
        capture_output=True, text=True)
    got = sorted(os.listdir(work))
    src = [os.path.join(frames_dir, names[i * step])
           for i in range(len(got)) if i * step < n]
    ex, _inv, db = compare(src, [os.path.join(work, f) for f in got[:len(src)]])
    shutil.rmtree(work, ignore_errors=True)
    return tags, missing, db


def cmd_measure(a):
    """Score a film that already exists against the frames it was made from.

    The same three numbers `bakeoff` prints, for an encode somebody else
    produced — a published film, or a recipe tried by hand. A film in the wild
    can then be held to the same standard as one this tool made, which is the
    only way the standard means anything.
    """
    names = sorted(f for f in os.listdir(a.frames) if f.endswith(".png"))
    if not names:
        die(f"no .png frames in {a.frames}")
    n = len(names)
    sample = min(a.sample, n)
    step = max(1, n // sample)
    src = [os.path.join(a.frames, names[i * step]) for i in range(sample)]
    rows = []
    work = tempfile.mkdtemp(prefix="encode-measure-")
    for f in a.films:
        dec = decode_sample(f, os.path.join(work, os.path.basename(f)), step)
        ex, inv, db = compare(src, dec[:len(src)])
        size = os.path.getsize(f)
        rows.append(dict(film=os.path.basename(f), MB=round(size / 1e6, 2),
                         kbps=round(size * 8 / (n / a.fps) / 1000),
                         exact_pct=round(ex * 100, 2), new_colours=inv,
                         psnr=db))
    axi.table("film", rows, ["film", "MB", "kbps", "exact_pct", "new_colours",
                             "psnr"])
    shutil.rmtree(work, ignore_errors=True)
    return 0


def cmd_encode(a):
    r = RECIPES[a.recipe] if a.recipe in RECIPES else None
    if r is None:
        die(f"no recipe named {a.recipe!r}",
            f"recipes: {', '.join(sorted(RECIPES))}", code=2)
    # **A film nobody can play is not a film.** A 4:4:4 recipe is a master, and
    # producing one as a deliverable has to be said out loud — see the note
    # above RECIPES for the measurement that made this a gate rather than a
    # comment.
    if not r.get("ship") and not a.master:
        die(f"{a.recipe} is a MASTER recipe, not a deliverable",
            f"{r['note']}",
            f"ship it anyway (and expect a player to mangle it): --master",
            f"the deliverable recipe is {DEFAULT}", code=2)
    want = os.path.splitext(a.out)[1].lstrip(".")
    if want and want != r["ext"]:
        die(f"recipe {a.recipe} produces {r['ext']}, --out asks for {want}",
            f"--out {os.path.splitext(a.out)[0]}.{r['ext']}", code=2)
    secs, size = encode(a.frames, a.out, a.fps, a.recipe)
    _, n = frame_glob(a.frames)
    axi.obj("encoded", [
        ("file", a.out), ("recipe", a.recipe), ("chroma", r["pix"]),
        ("frames", n), ("fps", a.fps), ("seconds", round(n / a.fps, 1)),
        ("MB", round(size / 1e6, 2)),
        ("kbps", round(size * 8 / (n / a.fps) / 1000)),
        ("encode_s", round(secs, 1))])
    # **VERIFY BY LOOKING AT WHAT CAME BACK OUT.** Not by trusting the encoder
    # and not by an ffmpeg round trip, which cancels its own mistake.
    tags, missing, db = verify_colour(a.out, a.frames, a.fps)
    axi.obj("colour", [
        ("profile", tags.get("profile", "?")),
        ("color_space", tags.get("color_space", "unknown")),
        ("color_primaries", tags.get("color_primaries", "unknown")),
        ("color_transfer", tags.get("color_transfer", "unknown")),
        ("color_range", tags.get("color_range", "unknown")),
        ("as_a_player_sees_it_psnr", db)])
    if missing:
        die(f"{a.out} is missing colour tags: {', '.join(missing)}",
            "an untagged stream leaves the matrix to the player to guess, and "
            "a wrong guess shifts the hue of the whole picture",
            "this is a bug in the recipe, not in the film — see YUV_COLOUR")
    if db != "inf" and float(db) < 30.0:
        die(f"{a.out} does not survive a BT.709 player: PSNR {db} dB",
            "the encode's colour conversion and its tag disagree, or the "
            "profile is one the decoder mishandles",
            "compare a still by eye before shipping it")
    if a.mobile:
        secs, size = encode(a.frames, a.mobile, a.fps, MOBILE)
        axi.obj("mobile", [
            ("file", a.mobile), ("recipe", MOBILE),
            ("MB", round(size / 1e6, 2)), ("encode_s", round(secs, 1))])
    print()
    axi.helps([
        f"compare the alternatives on these frames: encode_film.py bakeoff {a.frames}",
    ])
    return 0


def cmd_recipes(_a):
    axi.table("recipe", [
        dict(name=k, chroma=v["pix"], container=v["ext"],
             default=("yes" if k == DEFAULT else ""), note=v["note"])
        for k, v in sorted(RECIPES.items())],
        ["name", "chroma", "container", "default", "note"])
    return 0


def home():
    print("bin: tools/pewter-chain/encode_film.py")
    print("description: encode a swarm film's frames, and measure the recipe "
          "that does it")
    print()
    axi.table("recipe", [
        dict(name=k, chroma=v["pix"], container=v["ext"],
             default=("yes" if k == DEFAULT else ""))
        for k, v in sorted(RECIPES.items())],
        ["name", "chroma", "container", "default"])
    print()
    axi.helps([
        "encode a film:  encode_film.py encode <frames-dir> --out FILM.mp4 --fps 60",
        "measure why:    encode_film.py bakeoff <frames-dir> --frames 240 --keep /tmp/bake",
        "score a film:   encode_film.py measure <frames-dir> FILM.mp4",
        "per-command:    encode_film.py help encode | help bakeoff | help measure",
        "the argument:   docs/agent/film-encoding.md",
    ])
    return 0


HELP = {
    "encode": [
        "encode_film.py encode <frames-dir> --out FILM.mp4 [--fps 60] "
        "[--recipe NAME] [--mobile FILM-mobile.mp4]",
        "",
        f"Default recipe {DEFAULT}: constant quality at 4:4:4 chroma. Nothing "
        "scales — the frames are already at the output resolution.",
        "",
        "examples:",
        "  encode_film.py encode /tmp/frames --out film.mp4 --fps 60",
        "  encode_film.py encode /tmp/frames --out film.mp4 --fps 60 "
        "--mobile film-mobile.mp4",
    ],
    "bakeoff": [
        "encode_film.py bakeoff <frames-dir> [--frames 240] [--start 0] "
        "[--sample 8] [--keep DIR] [--recipes A,B]",
        "",
        "Encodes the same frames under every recipe and prints size, bitrate, "
        "the share of pixels that come back byte-identical, how many colours "
        "the decode invented, PSNR and encode time.",
        "",
        "--keep DIR also writes each clip and a zoomed nearest-neighbour crop "
        "of one decoded frame, so sharpness can be looked at and not only read.",
        "",
        "examples:",
        "  encode_film.py bakeoff /tmp/frames --frames 240 --keep /tmp/bake",
    ],
    "measure": [
        "encode_film.py measure <frames-dir> FILM [FILM ...] [--sample 10] [--fps 60]",
        "",
        "Score films that already exist against the frames they were made "
        "from — size, bitrate, the share of pixels that come back "
        "byte-identical, colours invented, PSNR.",
        "",
        "examples:",
        "  encode_film.py measure /tmp/frames old.mp4 new.mp4",
    ],
    "recipes": ["encode_film.py recipes", "", "Every recipe and what it is for."],
}


def main():
    if len(sys.argv) == 1:
        return home()
    if sys.argv[1] == "help":
        if len(sys.argv) == 2:
            return home()
        k = sys.argv[2]
        if k not in HELP:
            die(f"no command named {k!r}",
                f"commands: {', '.join(sorted(HELP))}", code=2)
        print("\n".join(HELP[k]))
        return 0
    ap = argparse.ArgumentParser(add_help=False)
    sub = ap.add_subparsers(dest="cmd")

    e = sub.add_parser("encode", add_help=False)
    e.add_argument("frames")
    e.add_argument("--out", required=True)
    e.add_argument("--fps", type=int, default=60)
    e.add_argument("--recipe", default=DEFAULT)
    e.add_argument("--mobile", default=None)
    e.add_argument("--master", action="store_true",
                   help="allow a MASTER-only recipe (4:4:4, lossless). Those "
                        "are archival: players mangle them")
    e.set_defaults(fn=cmd_encode)

    b = sub.add_parser("bakeoff", add_help=False)
    b.add_argument("frames")
    b.add_argument("--frames", dest="frames_n", type=int, default=240)
    b.add_argument("--start", type=int, default=0)
    b.add_argument("--sample", type=int, default=8)
    b.add_argument("--fps", type=int, default=60)
    b.add_argument("--keep", default=None)
    b.add_argument("--crop", default="380,300,160,90,6",
                   help="x,y,w,h,zoom of the still crop --keep writes")
    b.add_argument("--recipes", default=None)
    b.set_defaults(fn=cmd_bakeoff)

    m = sub.add_parser("measure", add_help=False)
    m.add_argument("frames")
    m.add_argument("films", nargs="+")
    m.add_argument("--sample", type=int, default=10)
    m.add_argument("--fps", type=int, default=60)
    m.set_defaults(fn=cmd_measure)

    r = sub.add_parser("recipes", add_help=False)
    r.set_defaults(fn=cmd_recipes)

    try:
        a = ap.parse_args()
    except SystemExit:
        die("could not parse that command line",
            "encode_film.py            what this is",
            "encode_film.py help encode | help bakeoff | help recipes",
            code=2)
    if not getattr(a, "cmd", None):
        return home()
    if getattr(a, "recipes", None):
        names = [n.strip() for n in a.recipes.split(",") if n.strip()]
        bad = [n for n in names if n not in RECIPES]
        if bad:
            die(f"no recipe named {bad[0]!r}",
                f"recipes: {', '.join(sorted(RECIPES))}")
        a.recipes = names
    elif a.cmd == "bakeoff":
        a.recipes = [k for k in RECIPES if k != MOBILE]
    if getattr(a, "keep", None):
        os.makedirs(a.keep, exist_ok=True)
    return a.fn(a)


if __name__ == "__main__":
    sys.exit(main())

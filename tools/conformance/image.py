"""Minimal PNG/PPM reading and PNG writing — Python standard library only.

The conformance runner has to read two kinds of image:

* **our reference frames**, which are 2-bit greyscale PNGs (the dmg-acid2
  reference from mattcurrie is `bit depth 2, colour type 0`), and
* **whatever a foreign adapter produced**, which may be a PNG in any common
  form, a Netpbm `P6` (binjgb's `--output` writes one), or a raw pixel dump.

`zlib` is in the standard library, so a PNG decoder is about eighty lines of
un-filtering — cheaper than asking every project that adopts this action to
install Pillow. That is the same hand-rolled, dependency-light ethos as the
rest of the test harness in this repo (`testharness/` writes its own PNGs).

Everything here works in greyscale-agnostic terms: a frame is decoded to a flat
`bytearray` of 8-bit RGB triples plus its width and height, and comparison
happens after `quantise_shades`, which is where cross-emulator palette
differences are dealt with.
"""

from __future__ import annotations

import struct
import zlib

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"


class ImageError(Exception):
    """The bytes handed to us are not an image we can read."""


class Image:
    """An 8-bit RGB image. `pixels` is `width * height * 3` bytes."""

    __slots__ = ("width", "height", "pixels")

    def __init__(self, width: int, height: int, pixels: bytes):
        if len(pixels) != width * height * 3:
            raise ImageError(
                f"pixel buffer is {len(pixels)} bytes, expected {width * height * 3}"
            )
        self.width = width
        self.height = height
        self.pixels = bytes(pixels)

    def __eq__(self, other):
        return (
            isinstance(other, Image)
            and (self.width, self.height) == (other.width, other.height)
            and self.pixels == other.pixels
        )


# --------------------------------------------------------------------------
# Decoding
# --------------------------------------------------------------------------


def decode(data: bytes, expect: tuple[int, int] | None = None) -> Image:
    """Decode PNG, Netpbm P6/P5, or a raw pixel dump.

    A raw dump carries no geometry, so `expect` (width, height) is required to
    read one — the runner always knows the geometry it asked the adapter for.
    Accepting raw RGBA/RGB at all is deliberate: it means an emulator with no
    image encoder can still take part by dumping its framebuffer with one
    `write()`, which is the smallest possible ask.
    """
    if data.startswith(PNG_MAGIC):
        return _decode_png(data)
    if data[:2] in (b"P6", b"P5", b"P3", b"P2"):
        return _decode_pnm(data)
    if expect is not None:
        w, h = expect
        if len(data) == w * h * 4:
            return Image(w, h, _rgba_to_rgb(data))
        if len(data) == w * h * 3:
            return Image(w, h, data)
        if len(data) == w * h:  # 8-bit greyscale / palette index
            return Image(w, h, bytes(b for v in data for b in (v, v, v)))
    raise ImageError(
        "unrecognised image: not a PNG, not a Netpbm P5/P6, and not a raw "
        f"{expect[0]}x{expect[1]} RGBA/RGB/grey dump ({len(data)} bytes)"
        if expect
        else f"unrecognised image ({len(data)} bytes)"
    )


def _rgba_to_rgb(data: bytes) -> bytes:
    out = bytearray(len(data) // 4 * 3)
    out[0::3] = data[0::4]
    out[1::3] = data[1::4]
    out[2::3] = data[2::4]
    return bytes(out)


def _decode_pnm(data: bytes) -> Image:
    """Netpbm: P6/P5 (binary RGB/grey) and P3/P2 (the ASCII forms).

    The ASCII variants are here because binjgb — the first foreign emulator
    this action was proven against — writes `P3`. Assuming the binary form
    would have rejected a perfectly good frame, which is precisely the kind of
    needless demand that stops projects adopting a contract.
    """
    kind = data[:2]
    fields: list[bytes] = []
    i = 2
    while len(fields) < 3:
        while i < len(data) and data[i : i + 1].isspace():
            i += 1
        if data[i : i + 1] == b"#":  # comment to end of line
            while i < len(data) and data[i : i + 1] not in (b"\n", b"\r"):
                i += 1
            continue
        start = i
        while i < len(data) and not data[i : i + 1].isspace():
            i += 1
        fields.append(data[start:i])
    w, h, maxval = (int(f) for f in fields)
    if maxval != 255:
        raise ImageError(f"Netpbm maxval {maxval} is not supported (need 255)")

    if kind in (b"P3", b"P2"):
        per_pixel = 3 if kind == b"P3" else 1
        vals = data[i:].split()
        need = w * h * per_pixel
        if len(vals) < need:
            raise ImageError(f"truncated {kind.decode()} data")
        nums = [int(v) for v in vals[:need]]
        if per_pixel == 3:
            return Image(w, h, bytes(nums))
        return Image(w, h, bytes(b for v in nums for b in (v, v, v)))

    i += 1  # exactly one whitespace byte after maxval
    body = data[i:]
    if kind == b"P5":
        need = w * h
        if len(body) < need:
            raise ImageError("truncated P5 data")
        return Image(w, h, bytes(b for v in body[:need] for b in (v, v, v)))
    need = w * h * 3
    if len(body) < need:
        raise ImageError("truncated P6 data")
    return Image(w, h, body[:need])


def _png_chunks(data: bytes):
    i = len(PNG_MAGIC)
    while i + 8 <= len(data):
        (length,) = struct.unpack(">I", data[i : i + 4])
        kind = data[i + 4 : i + 8]
        yield kind, data[i + 8 : i + 8 + length]
        i += 12 + length


def _decode_png(data: bytes) -> Image:
    width = height = bit_depth = colour_type = interlace = 0
    palette = b""
    idat = bytearray()
    for kind, body in _png_chunks(data):
        if kind == b"IHDR":
            width, height, bit_depth, colour_type, _comp, _filt, interlace = (
                struct.unpack(">IIBBBBB", body)
            )
        elif kind == b"PLTE":
            palette = body
        elif kind == b"IDAT":
            idat += body
        elif kind == b"IEND":
            break
    if not width:
        raise ImageError("PNG has no IHDR")
    if interlace:
        raise ImageError("interlaced PNG is not supported")
    if colour_type not in (0, 2, 3, 4, 6):
        raise ImageError(f"PNG colour type {colour_type} is not supported")

    samples = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[colour_type]
    if bit_depth not in (1, 2, 4, 8, 16):
        raise ImageError(f"PNG bit depth {bit_depth} is not supported")
    if bit_depth == 16 and colour_type == 3:
        raise ImageError("16-bit palette PNG is impossible")

    raw = zlib.decompress(bytes(idat))
    bits_per_pixel = samples * bit_depth
    stride = (width * bits_per_pixel + 7) // 8
    # Filtering works on whole bytes for depths < 8 (bpp rounds up to 1).
    bpp = max(1, bits_per_pixel // 8)
    lines = _unfilter(raw, height, stride, bpp)

    out = bytearray(width * height * 3)
    maxval = (1 << bit_depth) - 1
    for y in range(height):
        line = lines[y]
        for x in range(width):
            vals = [
                _sample(line, (x * samples + s), bit_depth) for s in range(samples)
            ]
            if colour_type == 3:
                idx = vals[0]
                if (idx + 1) * 3 > len(palette):
                    raise ImageError("PNG palette index out of range")
                r, g, b = palette[idx * 3 : idx * 3 + 3]
            elif colour_type in (0, 4):
                g8 = vals[0] * 255 // maxval
                r = g = b = g8
            else:
                r, g, b = (v * 255 // maxval for v in vals[:3])
            o = (y * width + x) * 3
            out[o] = r
            out[o + 1] = g
            out[o + 2] = b
    return Image(width, height, bytes(out))


def _sample(line: bytes, index: int, bit_depth: int) -> int:
    if bit_depth == 8:
        return line[index]
    if bit_depth == 16:
        return line[index * 2]  # drop the low byte; frames are 8-bit anyway
    per_byte = 8 // bit_depth
    byte = line[index // per_byte]
    shift = 8 - bit_depth * (index % per_byte + 1)
    return (byte >> shift) & ((1 << bit_depth) - 1)


def _unfilter(raw: bytes, height: int, stride: int, bpp: int) -> list[bytes]:
    """Reverse the five PNG scanline filters (RFC 2083 §6)."""
    lines: list[bytes] = []
    prev = bytes(stride)
    pos = 0
    for _ in range(height):
        if pos + 1 + stride > len(raw):
            raise ImageError("truncated PNG image data")
        ftype = raw[pos]
        line = bytearray(raw[pos + 1 : pos + 1 + stride])
        pos += 1 + stride
        if ftype == 0:
            pass
        elif ftype == 1:
            for i in range(bpp, stride):
                line[i] = (line[i] + line[i - bpp]) & 0xFF
        elif ftype == 2:
            for i in range(stride):
                line[i] = (line[i] + prev[i]) & 0xFF
        elif ftype == 3:
            for i in range(stride):
                left = line[i - bpp] if i >= bpp else 0
                line[i] = (line[i] + ((left + prev[i]) >> 1)) & 0xFF
        elif ftype == 4:
            for i in range(stride):
                a = line[i - bpp] if i >= bpp else 0
                b = prev[i]
                c = prev[i - bpp] if i >= bpp else 0
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                pred = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pred) & 0xFF
        else:
            raise ImageError(f"unknown PNG filter type {ftype}")
        lines.append(bytes(line))
        prev = line
    return lines


# --------------------------------------------------------------------------
# Comparison
# --------------------------------------------------------------------------


def quantise_shades(img: Image, levels: int = 4) -> bytes:
    """Reduce a frame to `levels` shade indices by **luminance rank**.

    This is the whole reason a screen check can be run against someone else's
    emulator. A DMG frame is four shades, but every emulator paints them in its
    own palette — greenish DMG, neutral grey, a user's custom scheme — so a
    literal RGB comparison would fail every foreign emulator for a reason that
    has nothing to do with accuracy.

    Ranking the distinct luminances present and replacing each pixel with its
    rank throws the palette away and keeps the only thing the test ROM is
    actually asserting: which of the four shades each pixel got. A frame with
    fewer than `levels` distinct shades (a solid screen, mid-boot) still ranks
    consistently, so it compares equal to a reference in the same state.

    Returns one byte per pixel: `0` = darkest shade present.
    """
    lums = []
    px = img.pixels
    for i in range(0, len(px), 3):
        # Rec. 601 luma, integer: close enough to order four flat shades.
        lums.append((px[i] * 299 + px[i + 1] * 587 + px[i + 2] * 114) // 1000)
    order = sorted(set(lums))
    if len(order) > levels:
        # More shades than expected (a colour-mapped or dithered frame): fall
        # back to even luminance bucketing rather than pretending to rank.
        lo, hi = order[0], order[-1]
        span = max(1, hi - lo)
        return bytes(min(levels - 1, (v - lo) * levels // (span + 1)) for v in lums)
    rank = {v: i for i, v in enumerate(order)}
    return bytes(rank[v] for v in lums)


def compare(actual: Image, reference: Image, levels: int = 4) -> tuple[int, bytes]:
    """Shade-compare two frames. Returns (differing pixels, the diff mask)."""
    if (actual.width, actual.height) != (reference.width, reference.height):
        raise ImageError(
            f"frame is {actual.width}x{actual.height}, reference is "
            f"{reference.width}x{reference.height}"
        )
    a = quantise_shades(actual, levels)
    b = quantise_shades(reference, levels)
    mask = bytes(1 if x != y else 0 for x, y in zip(a, b))
    return sum(mask), mask


def diff_image(actual: Image, mask: bytes) -> Image:
    """The actual frame with differing pixels painted red, for the artifact."""
    out = bytearray(actual.pixels)
    for i, bad in enumerate(mask):
        if bad:
            out[i * 3 : i * 3 + 3] = b"\xff\x00\x00"
    return Image(actual.width, actual.height, bytes(out))


# --------------------------------------------------------------------------
# Encoding (artifacts only — correctness matters, size does not)
# --------------------------------------------------------------------------


def encode_png(img: Image) -> bytes:
    raw = bytearray()
    stride = img.width * 3
    for y in range(img.height):
        raw.append(0)  # filter: none
        raw += img.pixels[y * stride : (y + 1) * stride]
    out = bytearray(PNG_MAGIC)
    out += _chunk(b"IHDR", struct.pack(">IIBBBBB", img.width, img.height, 8, 2, 0, 0, 0))
    out += _chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    out += _chunk(b"IEND", b"")
    return bytes(out)


def _chunk(kind: bytes, body: bytes) -> bytes:
    return (
        struct.pack(">I", len(body))
        + kind
        + body
        + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
    )

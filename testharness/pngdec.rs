//! Minimal PNG **reader** for the test harness — enough to load a checked-in
//! reference screenshot and nothing more.
//!
//! # Why this exists
//!
//! The harness has to compare what we render against an image somebody else
//! produced (`third_party/dmg-acid2/reference-dmg.png`, by the ROM's author).
//! Comparing against our own previous output is not a test — it re-blesses a
//! regression as readily as a fix. So a reference decoder is load-bearing.
//!
//! Everything else in `testharness/` is dependency-free on purpose (the FNV
//! hash and the PNG *writer* are hand-rolled), and this keeps that property:
//! a DEFLATE decompressor (RFC 1951, all three block types) plus PNG
//! unfiltering and bit unpacking, implemented from the specifications.
//!
//! Scope: non-interlaced PNG, colour types 0 (greyscale), 2 (truecolour),
//! 3 (palette), 4 (grey+alpha) and 6 (RGBA), bit depths 1/2/4/8/16. That
//! covers what image tools actually emit for a 4-shade 160x144 screenshot —
//! the upstream dmg-acid2 reference is 2-bit greyscale, an uncommon shape a
//! naive decoder gets wrong.

#![allow(dead_code)]

/// A decoded image, always expanded to 8-bit RGB.
pub struct Image {
    pub width: usize,
    pub height: usize,
    /// `width * height * 3` bytes, row-major, no padding.
    pub rgb: Vec<u8>,
}

// ---------------------------------------------------------------------------
// DEFLATE (RFC 1951)
// ---------------------------------------------------------------------------

struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bits {
            data,
            pos: 0,
            bit: 0,
        }
    }

    /// Read `n` bits LSB-first, the order DEFLATE stores its fixed-width
    /// fields in.
    fn bits(&mut self, n: u32) -> Result<u32, String> {
        let mut v = 0u32;
        for i in 0..n {
            if self.pos >= self.data.len() {
                return Err("deflate: out of input".into());
            }
            let b = (self.data[self.pos] >> self.bit) & 1;
            v |= (b as u32) << i;
            self.bit += 1;
            if self.bit == 8 {
                self.bit = 0;
                self.pos += 1;
            }
        }
        Ok(v)
    }

    fn align(&mut self) {
        if self.bit != 0 {
            self.bit = 0;
            self.pos += 1;
        }
    }
}

/// A canonical Huffman table stored as "how many codes of each length" plus the
/// symbols in code order — the representation RFC 1951 §3.2.2 describes, which
/// decodes bit by bit without building a tree.
struct Huffman {
    counts: [u16; 16],
    symbols: Vec<u16>,
}

impl Huffman {
    fn new(lengths: &[u8]) -> Huffman {
        let mut counts = [0u16; 16];
        for &l in lengths {
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        // Offset of the first symbol of each length within `symbols`.
        let mut offs = [0u16; 16];
        for l in 1..16 {
            offs[l] = offs[l - 1] + counts[l - 1];
        }
        let mut symbols = vec![0u16; lengths.len()];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Huffman { counts, symbols }
    }

    fn decode(&self, br: &mut Bits) -> Result<u16, String> {
        // Walk lengths 1..15, accumulating the code MSB-first (codes are stored
        // most-significant-bit first, unlike the fixed-width fields).
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bits(1)? as i32;
            let count = self.counts[len] as i32;
            if code - first < count {
                return Ok(self.symbols[(index + (code - first)) as usize]);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err("deflate: invalid huffman code".into())
    }
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
    115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
    1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
    12, 13, 13,
];

fn fixed_tables() -> (Huffman, Huffman) {
    let mut lit = [0u8; 288];
    for (i, l) in lit.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    (Huffman::new(&lit), Huffman::new(&[5u8; 30]))
}

fn dynamic_tables(br: &mut Bits) -> Result<(Huffman, Huffman), String> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    const ORDER: [usize; 19] = [
        16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut clen = [0u8; 19];
    for &o in ORDER.iter().take(hclen) {
        clen[o] = br.bits(3)? as u8;
    }
    let code_huff = Huffman::new(&clen);

    let mut lengths = vec![0u8; hlit + hdist];
    let mut i = 0;
    while i < lengths.len() {
        let sym = code_huff.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                if i == 0 {
                    return Err("deflate: repeat with no previous length".into());
                }
                let prev = lengths[i - 1];
                let n = 3 + br.bits(2)? as usize;
                for _ in 0..n {
                    if i >= lengths.len() {
                        return Err("deflate: length run overflows".into());
                    }
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 => i += 3 + br.bits(3)? as usize,
            18 => i += 11 + br.bits(7)? as usize,
            _ => return Err("deflate: bad code-length symbol".into()),
        }
    }
    if i > lengths.len() {
        return Err("deflate: code lengths overflow".into());
    }
    let (l, d) = lengths.split_at(hlit);
    Ok((Huffman::new(l), Huffman::new(d)))
}

fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut br = Bits::new(data);
    let mut out: Vec<u8> = Vec::new();
    loop {
        let last = br.bits(1)?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                br.align();
                if br.pos + 4 > br.data.len() {
                    return Err("deflate: truncated stored block".into());
                }
                let len =
                    u16::from_le_bytes([br.data[br.pos], br.data[br.pos + 1]]) as usize;
                br.pos += 4; // LEN then its one's complement NLEN
                if br.pos + len > br.data.len() {
                    return Err("deflate: stored block runs past input".into());
                }
                out.extend_from_slice(&br.data[br.pos..br.pos + len]);
                br.pos += len;
            }
            1 | 2 => {
                let (lit, dist) = if btype == 1 {
                    fixed_tables()
                } else {
                    dynamic_tables(&mut br)?
                };
                loop {
                    let sym = lit.decode(&mut br)?;
                    match sym.cmp(&256) {
                        std::cmp::Ordering::Less => out.push(sym as u8),
                        std::cmp::Ordering::Equal => break,
                        std::cmp::Ordering::Greater => {
                            let i = sym as usize - 257;
                            if i >= LEN_BASE.len() {
                                return Err("deflate: bad length symbol".into());
                            }
                            let len = LEN_BASE[i] as usize
                                + br.bits(LEN_EXTRA[i] as u32)? as usize;
                            let d = dist.decode(&mut br)? as usize;
                            if d >= DIST_BASE.len() {
                                return Err("deflate: bad distance symbol".into());
                            }
                            let distance = DIST_BASE[d] as usize
                                + br.bits(DIST_EXTRA[d] as u32)? as usize;
                            if distance > out.len() {
                                return Err("deflate: distance before start".into());
                            }
                            let start = out.len() - distance;
                            for k in 0..len {
                                let b = out[start + k];
                                out.push(b);
                            }
                        }
                    }
                }
            }
            _ => return Err("deflate: reserved block type".into()),
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

// ---------------------------------------------------------------------------
// PNG
// ---------------------------------------------------------------------------

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];

pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    if bytes.len() < 8 || bytes[..8] != SIGNATURE {
        return Err("not a PNG".into());
    }
    let mut pos = 8;
    let (mut width, mut height) = (0usize, 0usize);
    let (mut depth, mut colour) = (0u8, 0u8);
    let mut palette: Vec<u8> = Vec::new();
    let mut idat: Vec<u8> = Vec::new();

    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[pos..pos + 4].try_into().unwrap()) as usize;
        let tag = &bytes[pos + 4..pos + 8];
        let body_at = pos + 8;
        if body_at + len + 4 > bytes.len() {
            return Err("PNG: truncated chunk".into());
        }
        let body = &bytes[body_at..body_at + len];
        match tag {
            b"IHDR" => {
                if len < 13 {
                    return Err("PNG: short IHDR".into());
                }
                width = u32::from_be_bytes(body[0..4].try_into().unwrap()) as usize;
                height = u32::from_be_bytes(body[4..8].try_into().unwrap()) as usize;
                depth = body[8];
                colour = body[9];
                if body[12] != 0 {
                    return Err("PNG: interlaced images are not supported".into());
                }
            }
            b"PLTE" => palette = body.to_vec(),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos = body_at + len + 4;
    }
    if width == 0 || height == 0 {
        return Err("PNG: no IHDR".into());
    }
    if idat.len() < 2 {
        return Err("PNG: no IDAT".into());
    }
    // Skip the 2-byte zlib header; the adler32 trailer is simply not read.
    let raw = inflate(&idat[2..])?;

    let channels: usize = match colour {
        0 | 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(format!("PNG: unsupported colour type {colour}")),
    };
    if !matches!(depth, 1 | 2 | 4 | 8 | 16) {
        return Err(format!("PNG: unsupported bit depth {depth}"));
    }
    let bits_per_pixel = channels * depth as usize;
    let stride = (width * bits_per_pixel + 7) / 8;
    // PNG filters operate on whole bytes; the "previous pixel" distance is the
    // pixel size rounded up to one byte (spec §9.2).
    let filter_step = std::cmp::max(1, bits_per_pixel / 8);
    if raw.len() < height * (stride + 1) {
        return Err("PNG: image data shorter than declared size".into());
    }

    let mut lines: Vec<u8> = vec![0; height * stride];
    for y in 0..height {
        let ft = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        for x in 0..stride {
            let a = if x >= filter_step {
                lines[y * stride + x - filter_step]
            } else {
                0
            };
            let b = if y > 0 {
                lines[(y - 1) * stride + x]
            } else {
                0
            };
            let c = if y > 0 && x >= filter_step {
                lines[(y - 1) * stride + x - filter_step]
            } else {
                0
            };
            let v = src[x];
            lines[y * stride + x] = match ft {
                0 => v,
                1 => v.wrapping_add(a),
                2 => v.wrapping_add(b),
                3 => v.wrapping_add((((a as u16) + (b as u16)) / 2) as u8),
                4 => v.wrapping_add(paeth(a, b, c)),
                _ => return Err(format!("PNG: unknown filter {ft}")),
            };
        }
    }

    // Expand to 8-bit RGB.
    let mut rgb = vec![0u8; width * height * 3];
    let maxval = (1u32 << depth.min(8)) - 1;
    for y in 0..height {
        for x in 0..width {
            let mut ch = [0u8; 4];
            for (c, slot) in ch.iter_mut().enumerate().take(channels) {
                *slot =
                    sample(&lines[y * stride..(y + 1) * stride], x, c, channels, depth);
            }
            let (r, g, b) = match colour {
                0 | 4 => {
                    // Greyscale: scale the sample to full 8-bit range so a
                    // 2-bit 0..3 becomes 0/85/170/255.
                    let v = if depth >= 8 {
                        ch[0]
                    } else {
                        ((ch[0] as u32 * 255) / maxval) as u8
                    };
                    (v, v, v)
                }
                2 | 6 => (ch[0], ch[1], ch[2]),
                3 => {
                    let i = ch[0] as usize * 3;
                    if i + 2 >= palette.len() {
                        return Err("PNG: palette index out of range".into());
                    }
                    (palette[i], palette[i + 1], palette[i + 2])
                }
                _ => unreachable!(),
            };
            let o = (y * width + x) * 3;
            rgb[o] = r;
            rgb[o + 1] = g;
            rgb[o + 2] = b;
        }
    }
    Ok(Image { width, height, rgb })
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = (
        (p - a as i16).abs(),
        (p - b as i16).abs(),
        (p - c as i16).abs(),
    );
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Read channel `c` of pixel `x` from an unfiltered scanline. 16-bit samples
/// are truncated to their high byte — the low byte cannot matter for a
/// four-shade Game Boy screen.
fn sample(line: &[u8], x: usize, c: usize, channels: usize, depth: u8) -> u8 {
    match depth {
        16 => line[(x * channels + c) * 2],
        8 => line[x * channels + c],
        _ => {
            let d = depth as usize;
            let index = x * channels + c;
            let per_byte = 8 / d;
            let byte = line[index / per_byte];
            let shift = 8 - d * (index % per_byte + 1);
            (byte >> shift) & ((1 << d) - 1) as u8
        }
    }
}

/// Reduce a frame to `levels` shade indices by **luminance rank**, exactly as
/// `tools/conformance/image.py::quantise_shades` does.
///
/// This is what makes the comparison a test of *emulation* rather than of
/// palette taste: our frames are `000000/606060/c0c0c0/ffffff` and the upstream
/// dmg-acid2 reference is `000000/555555/aaaaaa/ffffff`, so a literal RGB
/// compare would fail every single run for a reason the ROM is not asserting.
///
/// Returns one byte per pixel; `0` is the darkest shade present.
pub fn quantise_shades(rgb: &[u8], levels: usize) -> Vec<u8> {
    let lums: Vec<u32> = rgb
        .chunks_exact(3)
        .map(|p| (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000)
        .collect();
    let mut order: Vec<u32> = lums.clone();
    order.sort_unstable();
    order.dedup();
    if order.len() > levels {
        // More shades than a DMG screen can hold: bucket evenly rather than
        // pretend the ranking means anything.
        let (lo, hi) = (order[0], order[order.len() - 1]);
        let span = std::cmp::max(1, hi - lo);
        return lums
            .iter()
            .map(|v| {
                std::cmp::min(
                    levels - 1,
                    (((v - lo) * levels as u32) / (span + 1)) as usize,
                ) as u8
            })
            .collect();
    }
    lums.iter()
        .map(|v| order.iter().position(|o| o == v).unwrap() as u8)
        .collect()
}

/// Drop the alpha channel of an RGBA framebuffer.
pub fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks_exact(4)
        .flat_map(|p| [p[0], p[1], p[2]])
        .collect()
}

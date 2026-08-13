//! Minimal DEFLATE (RFC 1951) compressor — greedy LZ77 + fixed Huffman codes.
//!
//! # Why this exists
//!
//! The terminal frontend transmits a Game Boy frame to the terminal sixty times
//! a second. A DMG frame has **four colours**, so it is enormously compressible,
//! but the original encoder used zlib *stored* (uncompressed) blocks on the
//! grounds that "compression is not free at 60 Hz". That reasoning was measured
//! on the wrong side of the pipe: our encode budget is ~0.1 ms of a 16.7 ms
//! frame, while the *terminal* has to parse and upload every byte we send.
//! 91 KB per frame is 5.5 MB/s of escape sequence for the terminal to chew
//! through, and WezTerm managed about 6 fps. Compressing the payload is
//! therefore the cheapest thing we can possibly do — see
//! [`docs/terminal-render.md`](../docs/terminal-render.md).
//!
//! # Scope
//!
//! Deliberately small: one fixed-Huffman block, greedy matching against a hash
//! chain, no dynamic code lengths and no lazy matching. On the content this
//! exists for (palette-indexed Game Boy frames with long runs) fixed Huffman is
//! within a few percent of a full-strength encoder, and it is a few hundred
//! lines of pure `std` rather than a new dependency — the same ethos as
//! `savestate.rs` and the test harness's hand-rolled hash and PNG writer. It
//! builds on `wasm32` and adds nothing to `Cargo.toml`.
//!
//! The output is a valid DEFLATE stream, so any conforming inflater reads it;
//! [`Deflater::zlib_into`] wraps it in the zlib container PNG requires
//! (RFC 1950).

/// Shortest match worth encoding: below three bytes a match costs more bits
/// than the literals it replaces.
const MIN_MATCH: usize = 3;
/// Longest match DEFLATE can encode in one length symbol.
const MAX_MATCH: usize = 258;
/// DEFLATE's sliding-window limit.
const MAX_DIST: usize = 32768;
/// Hash table size (2^15 buckets over 3-byte keys).
const HASH_BITS: u32 = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
/// How far down a hash chain to look before taking the best match so far.
/// 32 is generous for four-colour content and keeps the encoder well inside
/// its time budget.
const MAX_CHAIN: usize = 32;

/// Base match length for length symbols 257..=285.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99,
    115, 131, 163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
/// Base distance for distance symbols 0..=29.
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025,
    1537, 2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12,
    12, 13, 13,
];

/// Reusable compressor state. The hash table and chain are the only sizeable
/// allocations and they are reused across calls, so a steady-state frame loop
/// allocates nothing.
pub struct Deflater {
    /// Most recent position for each 3-byte hash, or `NONE`.
    head: Vec<i32>,
    /// Previous position sharing a position's hash.
    prev: Vec<i32>,
    bit_buf: u32,
    bit_cnt: u32,
}

const NONE: i32 = -1;

impl Default for Deflater {
    fn default() -> Self {
        Deflater::new()
    }
}

impl Deflater {
    pub fn new() -> Deflater {
        Deflater {
            head: vec![NONE; HASH_SIZE],
            prev: Vec::new(),
            bit_buf: 0,
            bit_cnt: 0,
        }
    }

    /// Compress `data` into `out` as a **zlib** stream (RFC 1950): the two-byte
    /// header PNG's `IDAT` expects, the deflate data, and the Adler-32 trailer.
    /// `out` is cleared first.
    pub fn zlib_into(&mut self, data: &[u8], out: &mut Vec<u8>) {
        out.clear();
        // CMF=0x78 (deflate, 32 KiB window), FLG=0x01 (fastest, no dictionary);
        // 0x7801 is a multiple of 31 as the spec requires.
        out.extend_from_slice(&[0x78, 0x01]);
        self.deflate_into(data, out);
        out.extend_from_slice(&adler32(data).to_be_bytes());
    }

    /// Compress `data` as a single final fixed-Huffman DEFLATE block, appending
    /// to `out`.
    pub fn deflate_into(&mut self, data: &[u8], out: &mut Vec<u8>) {
        self.bit_buf = 0;
        self.bit_cnt = 0;
        // BFINAL=1, BTYPE=01 (fixed Huffman).
        self.bits(1, 1, out);
        self.bits(0b01, 2, out);

        self.head.iter_mut().for_each(|h| *h = NONE);
        self.prev.clear();
        self.prev.resize(data.len(), NONE);

        let n = data.len();
        let mut i = 0usize;
        while i < n {
            let (mut len, mut dist) = (0usize, 0usize);
            if i + MIN_MATCH <= n {
                let h = hash3(&data[i..]);
                let (l, d) = self.longest_match(data, i, h);
                len = l;
                dist = d;
                // Insert *before* advancing so the chain stays complete.
                self.insert(i, h);
            }
            if len >= MIN_MATCH {
                self.emit_match(len, dist, out);
                // Every position inside the match must still enter the hash, or
                // later matches lose their anchors (and the ratio collapses).
                for j in i + 1..i + len {
                    if j + MIN_MATCH <= n {
                        let h = hash3(&data[j..]);
                        self.insert(j, h);
                    }
                }
                i += len;
            } else {
                self.literal(data[i], out);
                i += 1;
            }
        }
        // End-of-block symbol 256: 7 zero bits under the fixed table.
        self.code(0, 7, out);
        self.flush(out);
    }

    fn insert(&mut self, pos: usize, h: usize) {
        self.prev[pos] = self.head[h];
        self.head[h] = pos as i32;
    }

    /// Best (length, distance) at `pos`, walking at most [`MAX_CHAIN`] links.
    fn longest_match(&self, data: &[u8], pos: usize, h: usize) -> (usize, usize) {
        let n = data.len();
        let limit = (n - pos).min(MAX_MATCH);
        if limit < MIN_MATCH {
            return (0, 0);
        }
        let floor = pos.saturating_sub(MAX_DIST);
        let mut best_len = 0usize;
        let mut best_dist = 0usize;
        let mut cand = self.head[h];
        let mut chain = MAX_CHAIN;
        while cand >= 0 && chain > 0 {
            let c = cand as usize;
            if c < floor {
                break;
            }
            // Cheap reject: the byte that would extend the current best.
            if best_len == 0 || data[c + best_len] == data[pos + best_len] {
                let mut l = 0usize;
                while l < limit && data[c + l] == data[pos + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = pos - c;
                    if l >= limit {
                        break;
                    }
                }
            }
            cand = self.prev[c];
            chain -= 1;
        }
        if best_len >= MIN_MATCH {
            (best_len, best_dist)
        } else {
            (0, 0)
        }
    }

    // -- fixed-Huffman symbol emission ------------------------------------

    fn literal(&mut self, byte: u8, out: &mut Vec<u8>) {
        let v = byte as u32;
        if v < 144 {
            self.code(0b0011_0000 + v, 8, out);
        } else {
            self.code(0b1_1001_0000 + (v - 144), 9, out);
        }
    }

    fn emit_match(&mut self, len: usize, dist: usize, out: &mut Vec<u8>) {
        let li = len_index(len);
        let sym = 257 + li as u32;
        // Fixed table: 256..=279 are 7 bits from 0, 280..=287 are 8 bits from
        // 0b1100_0000.
        if sym < 280 {
            self.code(sym - 256, 7, out);
        } else {
            self.code(0b1100_0000 + (sym - 280), 8, out);
        }
        let extra = LEN_EXTRA[li];
        if extra > 0 {
            self.bits(len as u32 - LEN_BASE[li] as u32, extra as u32, out);
        }
        let di = dist_index(dist);
        // Distances use a fixed 5-bit code equal to the symbol.
        self.code(di as u32, 5, out);
        let extra = DIST_EXTRA[di];
        if extra > 0 {
            self.bits(dist as u32 - DIST_BASE[di] as u32, extra as u32, out);
        }
    }

    // -- bit plumbing ------------------------------------------------------

    /// Huffman codes are packed **most-significant bit first**.
    fn code(&mut self, code: u32, len: u32, out: &mut Vec<u8>) {
        for i in (0..len).rev() {
            self.bits((code >> i) & 1, 1, out);
        }
    }

    /// Everything else (block header, extra bits) is packed LSB-first.
    fn bits(&mut self, value: u32, len: u32, out: &mut Vec<u8>) {
        self.bit_buf |= (value & ((1u32 << len) - 1)) << self.bit_cnt;
        self.bit_cnt += len;
        while self.bit_cnt >= 8 {
            out.push(self.bit_buf as u8);
            self.bit_buf >>= 8;
            self.bit_cnt -= 8;
        }
    }

    fn flush(&mut self, out: &mut Vec<u8>) {
        if self.bit_cnt > 0 {
            out.push(self.bit_buf as u8);
            self.bit_buf = 0;
            self.bit_cnt = 0;
        }
    }
}

fn hash3(w: &[u8]) -> usize {
    let v = ((w[0] as u32) << 16) | ((w[1] as u32) << 8) | w[2] as u32;
    // Knuth multiplicative hash, folded to HASH_BITS.
    ((v.wrapping_mul(2_654_435_761)) >> (32 - HASH_BITS)) as usize & (HASH_SIZE - 1)
}

fn len_index(len: usize) -> usize {
    debug_assert!((MIN_MATCH..=MAX_MATCH).contains(&len));
    // 29 entries — a linear scan from the top is a handful of comparisons.
    let mut i = LEN_BASE.len() - 1;
    while LEN_BASE[i] as usize > len {
        i -= 1;
    }
    i
}

fn dist_index(dist: usize) -> usize {
    debug_assert!((1..=MAX_DIST).contains(&dist));
    let mut i = DIST_BASE.len() - 1;
    while DIST_BASE[i] as usize > dist {
        i -= 1;
    }
    i
}

/// Adler-32 (RFC 1950). Deferred modulo: 5552 bytes is the most that cannot
/// overflow `u32`.
pub fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b): (u32, u32) = (1, 0);
    for block in bytes.chunks(5552) {
        for &byte in block {
            a += byte as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny, deliberately literal inflater for fixed-Huffman blocks. Its only
    /// job is to prove the compressor's output is a real DEFLATE stream — if the
    /// two agreed on a shared misunderstanding the `image`-crate round-trip in
    /// `termimage` would catch it.
    fn inflate_fixed(data: &[u8]) -> Vec<u8> {
        struct R<'a> {
            d: &'a [u8],
            pos: usize,
            bit: u32,
        }
        impl R<'_> {
            fn bits(&mut self, n: u32) -> u32 {
                let mut v = 0;
                for i in 0..n {
                    let byte = self.d[self.pos];
                    let b = (byte >> self.bit) & 1;
                    v |= (b as u32) << i;
                    self.bit += 1;
                    if self.bit == 8 {
                        self.bit = 0;
                        self.pos += 1;
                    }
                }
                v
            }
            /// Huffman codes arrive MSB-first.
            fn code_bits(&mut self, n: u32) -> u32 {
                let mut v = 0;
                for _ in 0..n {
                    v = (v << 1) | self.bits(1);
                }
                v
            }
            fn symbol(&mut self) -> u32 {
                // 7-bit codes 0..=23 are symbols 256..=279.
                let mut v = self.code_bits(7);
                if v <= 23 {
                    return 256 + v;
                }
                v = (v << 1) | self.bits(1);
                if v <= 0b1011_1111 {
                    return v - 0b0011_0000;
                }
                if v <= 0b1100_0111 {
                    return 280 + (v - 0b1100_0000);
                }
                v = (v << 1) | self.bits(1);
                144 + (v - 0b1_1001_0000)
            }
        }
        let mut r = R {
            d: data,
            pos: 0,
            bit: 0,
        };
        assert_eq!(r.bits(1), 1, "BFINAL");
        assert_eq!(r.bits(2), 1, "BTYPE=fixed");
        let mut out: Vec<u8> = Vec::new();
        loop {
            let sym = r.symbol();
            match sym {
                0..=255 => out.push(sym as u8),
                256 => break,
                _ => {
                    let li = (sym - 257) as usize;
                    let len =
                        LEN_BASE[li] as usize + r.bits(LEN_EXTRA[li] as u32) as usize;
                    let di = r.code_bits(5) as usize;
                    let dist =
                        DIST_BASE[di] as usize + r.bits(DIST_EXTRA[di] as u32) as usize;
                    for _ in 0..len {
                        let b = out[out.len() - dist];
                        out.push(b);
                    }
                }
            }
        }
        out
    }

    fn round_trip(data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        Deflater::new().deflate_into(data, &mut out);
        inflate_fixed(&out)
    }

    #[test]
    fn round_trips_the_shapes_the_encoder_actually_sees() {
        let cases: Vec<Vec<u8>> = vec![
            Vec::new(),
            b"a".to_vec(),
            b"abcabcabcabcabc".to_vec(),
            vec![0u8; 70_000],                        // long runs
            (0..=255u8).cycle().take(9000).collect(), // no useful matches
            (0..6000).map(|i| ((i / 41) % 4) as u8).collect(), // scanline-ish
        ];
        for c in cases {
            assert_eq!(round_trip(&c), c, "round trip failed for {} bytes", c.len());
        }
    }

    #[test]
    fn four_colour_scanlines_compress_by_more_than_ten_times() {
        // The real workload: 160x144 at 2 bits per pixel, four colours, in
        // 40-byte rows behind a PNG filter byte. This is the whole point of the
        // module, so assert the ratio, not just correctness.
        let mut raw = Vec::new();
        for y in 0..144usize {
            raw.push(0u8);
            for x in 0..40usize {
                raw.push(if (x + y / 8) % 5 == 0 { 0b0110_1100 } else { 0 });
            }
        }
        let mut out = Vec::new();
        Deflater::new().deflate_into(&raw, &mut out);
        assert_eq!(inflate_fixed(&out), raw);
        assert!(
            out.len() * 10 < raw.len(),
            "{} -> {} bytes is under 10x",
            raw.len(),
            out.len()
        );
    }

    #[test]
    fn incompressible_input_does_not_blow_up() {
        // Fixed Huffman costs at most 9 bits per literal, so pseudo-random bytes
        // must stay near 12.5 % overhead, never unbounded.
        let data: Vec<u8> = (0..20_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8)
            .collect();
        let mut out = Vec::new();
        Deflater::new().deflate_into(&data, &mut out);
        assert_eq!(inflate_fixed(&out), data);
        assert!(out.len() < data.len() * 5 / 4, "{} bytes", out.len());
    }

    #[test]
    fn reused_deflater_matches_a_fresh_one() {
        // The hash table is reused across frames; a stale chain would silently
        // corrupt the second stream.
        let a: Vec<u8> = (0..4000).map(|i| (i % 7) as u8).collect();
        let b: Vec<u8> = (0..4000).map(|i| (i % 11) as u8).collect();
        let mut d = Deflater::new();
        let (mut x, mut y) = (Vec::new(), Vec::new());
        d.deflate_into(&a, &mut x);
        d.deflate_into(&b, &mut y);
        let mut fresh = Vec::new();
        Deflater::new().deflate_into(&b, &mut fresh);
        assert_eq!(y, fresh, "reuse changed the output");
        assert_eq!(inflate_fixed(&y), b);
    }

    #[test]
    fn zlib_wrapper_is_well_formed() {
        let data = b"hello hello hello hello".to_vec();
        let mut out = Vec::new();
        Deflater::new().zlib_into(&data, &mut out);
        assert_eq!(&out[..2], &[0x78, 0x01]);
        assert_eq!(
            (((out[0] as u16) << 8) | out[1] as u16) % 31,
            0,
            "zlib header check"
        );
        let n = out.len();
        assert_eq!(&out[n - 4..], &adler32(&data).to_be_bytes());
        assert_eq!(inflate_fixed(&out[2..n - 4]), data);
    }
}

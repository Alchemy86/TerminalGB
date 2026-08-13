//! Pixel-art upscaling for the 160×144 framebuffer, shared by every frontend.
//!
//! The core emits RGBA bytes ([`crate::gameboy::Gameboy::image`]); this module
//! turns that small framebuffer into a larger RGBA one, so terminals, the
//! desktop window, and the browser canvas can all show a bigger picture whose
//! extra pixels were *computed*, not merely stretched. Everything here is pure
//! `std` (no dependencies, no filesystem, builds on `wasm32`), same ethos as
//! `savestate.rs` and the test harness.
//!
//! # Algorithms and why these
//!
//! The DMG source is unusual for upscalers: 160×144 with only **four shades**.
//! Algorithms tuned for 16-bit-colour sprites (hqx's YUV thresholds, xBR's
//! colour-distance metric) are solving a gradient-discrimination problem this
//! source does not have, and blending scalers (2xSaI family) invent
//! *off-palette* intermediate shades. So we ship exact-match and
//! style-preserving scalers, which behave identically well on any palette:
//!
//! * [`Algorithm::Nearest`] — plain integer duplication, the crisp baseline.
//! * [`Algorithm::Scale2x`] — EPX/Scale2x/AdvMAME2x (Eric Johnston 1992,
//!   independently Andrea Mazzoleni 2001, <https://www.scale2x.it/>). Factors
//!   2/3/4 (4 = 2× applied twice). Reimplemented from the published
//!   one-paragraph rule — the reference implementation is GPL-2.0, which this
//!   MPL-2.0 repo must not inherit, but the algorithm itself is openly
//!   documented exact-match logic with nothing to copy.
//! * [`Algorithm::Mmpx`] — MMPX, McGuire & Gagiu, *"MMPX Style-Preserving
//!   Pixel Art Magnification"*, JCGT 2021
//!   (<https://jcgt.org/published/0010/02/04/>,
//!   <https://casual-effects.com/research/McGuire2021PixelArt/index.html>).
//!   Explicitly designed for limited-palette, hard-edged pixel art: it turns
//!   staircases into smooth 2:1 slopes while refusing to erase single-pixel
//!   details, and only ever outputs colours present in the source. Factors 2
//!   and 4 (4 = 2× twice). Ported from the authors' reference implementation,
//!   which is **MIT** (Copyright 2020 Morgan McGuire & Mara Gagiu) — licence
//!   text preserved above the function.
//! * [`Algorithm::LcdGrid`] — our own dot-matrix filter (no external lineage,
//!   no licence baggage): nearest-neighbour then a darkened 1-px gap between
//!   cells, per-axis so cell corners darken twice like round LCD dots. This is
//!   the "looks like a Game Boy" option, in the same family as RetroArch's
//!   `lcd-grid` shaders and the Analogue Pocket's DMG display mode; combine
//!   with the terminal palette `green` for the full DMG look.
//!
//! Deliberately **not** shipped: xBRZ (GPLv3 — both code and rule set are
//! Zenju's copyrighted expression), hqx (LGPL lookup tables tuned for 16-bit
//! YUV, wrong tool for 4 shades), 2xSaI family (GPL and blends off-palette).
//!
//! # API
//!
//! [`Upscaler`] owns its output buffers and is meant to live for a session:
//! feed [`Upscaler::apply`] the RGBA framebuffer each frame, get a borrowed
//! upscaled RGBA slice back. Algorithm and factor are runtime-switchable
//! ([`Upscaler::set_algorithm`] / [`Upscaler::set_factor`]); factor 1 is a
//! true no-op that returns the input slice untouched.
//!
//! Measured cost (release, one 160×144 frame, Linux x86-64 — see
//! `examples/upscale_gallery.rs` to reproduce on your machine): Nearest ×2-×6
//! and Scale2x ×2/×3 ≈ 0.05–0.5 ms, MMPX ×2 ≈ 0.3 ms, MMPX ×4 ≈ 1.6 ms,
//! LCD ×4 ≈ 0.4 ms — all comfortably inside a 16.7 ms frame.

/// Which upscaling algorithm to run. See the module docs for provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Algorithm {
    /// Plain integer pixel duplication (crisp, exact).
    #[default]
    Nearest,
    /// EPX / Scale2x / AdvMAME2x exact-match corner rule (factors 2, 3, 4).
    Scale2x,
    /// MMPX style-preserving magnification (factors 2, 4).
    Mmpx,
    /// Nearest plus a darkened dot-matrix grid, DMG-LCD flavoured.
    LcdGrid,
}

impl Algorithm {
    /// Parse a config value. Accepts a few aliases per algorithm. (Named like
    /// `Palette::from_str` in the terminal config, hence not `FromStr` — the
    /// config layer wants `Option`, not `Result`.)
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Algorithm> {
        match s.trim().to_ascii_lowercase().as_str() {
            "nearest" | "none" | "off" | "integer" => Some(Algorithm::Nearest),
            "scale2x" | "scale" | "epx" | "advmame" | "advmame2x" => {
                Some(Algorithm::Scale2x)
            }
            "mmpx" => Some(Algorithm::Mmpx),
            "lcd" | "lcdgrid" | "lcd-grid" | "grid" | "dotmatrix" | "dmg-lcd" => {
                Some(Algorithm::LcdGrid)
            }
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Algorithm::Nearest => "nearest",
            Algorithm::Scale2x => "scale2x",
            Algorithm::Mmpx => "mmpx",
            Algorithm::LcdGrid => "lcd",
        }
    }

    /// The next algorithm in a fixed cycle, for a live toggle key.
    pub fn next(self) -> Algorithm {
        match self {
            Algorithm::Nearest => Algorithm::Scale2x,
            Algorithm::Scale2x => Algorithm::Mmpx,
            Algorithm::Mmpx => Algorithm::LcdGrid,
            Algorithm::LcdGrid => Algorithm::Nearest,
        }
    }

    /// The scale factors this algorithm can produce, ascending.
    pub fn supported_factors(self) -> &'static [u8] {
        match self {
            Algorithm::Nearest => &[1, 2, 3, 4, 5, 6],
            Algorithm::Scale2x => &[2, 3, 4],
            Algorithm::Mmpx => &[2, 4],
            Algorithm::LcdGrid => &[2, 3, 4, 5, 6],
        }
    }

    /// Snap `want` to the nearest supported factor (ties resolve downward).
    pub fn clamp_factor(self, want: u8) -> u8 {
        let supported = self.supported_factors();
        let mut best = supported[0];
        for &f in supported {
            let d = want.abs_diff(f);
            if d < want.abs_diff(best) {
                best = f;
            }
        }
        best
    }

    /// The next supported factor after `current`, wrapping — for a toggle key.
    pub fn next_factor(self, current: u8) -> u8 {
        let supported = self.supported_factors();
        match supported.iter().position(|&f| f == current) {
            Some(i) => supported[(i + 1) % supported.len()],
            None => self.clamp_factor(current),
        }
    }
}

/// A runtime-selectable upscaler that owns its working buffers so per-frame
/// use allocates nothing after the first frame.
pub struct Upscaler {
    algo: Algorithm,
    factor: u8,
    /// Unpacked source + ping/pong pixel buffers (RGBA packed into u32).
    src_px: Vec<u32>,
    buf_a: Vec<u32>,
    buf_b: Vec<u32>,
    /// Final byte output handed to callers.
    out: Vec<u8>,
}

impl Upscaler {
    pub fn new(algo: Algorithm, factor: u8) -> Upscaler {
        Upscaler {
            algo,
            factor: algo.clamp_factor(factor),
            src_px: Vec::new(),
            buf_a: Vec::new(),
            buf_b: Vec::new(),
            out: Vec::new(),
        }
    }

    pub fn algorithm(&self) -> Algorithm {
        self.algo
    }

    pub fn factor(&self) -> u8 {
        self.factor
    }

    /// Switch algorithm, snapping the current factor to one it supports.
    pub fn set_algorithm(&mut self, algo: Algorithm) {
        self.algo = algo;
        self.factor = algo.clamp_factor(self.factor);
    }

    pub fn set_factor(&mut self, factor: u8) {
        self.factor = self.algo.clamp_factor(factor);
    }

    /// Cycle to the next algorithm (keeping the factor as close as possible).
    pub fn cycle_algorithm(&mut self) {
        self.set_algorithm(self.algo.next());
    }

    /// Cycle to the next supported factor.
    pub fn cycle_factor(&mut self) {
        self.factor = self.algo.next_factor(self.factor);
    }

    /// Output dimensions for a `w`×`h` input.
    pub fn output_size(&self, w: usize, h: usize) -> (usize, usize) {
        let f = self.factor as usize;
        (w * f, h * f)
    }

    /// A short human label like `"mmpx x2"` for status lines and toasts.
    pub fn label(&self) -> String {
        format!("{} x{}", self.algo.name(), self.factor)
    }

    /// Upscale one RGBA frame. `src.len()` must be `w * h * 4`. Returns the
    /// upscaled RGBA bytes (`w*factor × h*factor × 4`); at factor 1 the input
    /// slice is returned untouched.
    pub fn apply<'a>(&'a mut self, src: &'a [u8], w: usize, h: usize) -> &'a [u8] {
        assert_eq!(src.len(), w * h * 4, "src must be w*h RGBA bytes");
        let f = self.factor as usize;
        if f == 1 {
            return src;
        }

        // Unpack bytes to one u32 per pixel; all algorithms compare pixels
        // whole, and MMPX's luma unpacks channels itself (endian-safe: we only
        // ever go through {from,to}_ne_bytes, which round-trip).
        self.src_px.clear();
        self.src_px.extend(
            src.chunks_exact(4)
                .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]])),
        );

        let result: &[u32] = match (self.algo, f) {
            (Algorithm::Nearest, n) => {
                nearest(&self.src_px, w, h, n, &mut self.buf_a);
                &self.buf_a
            }
            (Algorithm::Scale2x, 2) => {
                scale2x(&self.src_px, w, h, &mut self.buf_a);
                &self.buf_a
            }
            (Algorithm::Scale2x, 3) => {
                scale3x(&self.src_px, w, h, &mut self.buf_a);
                &self.buf_a
            }
            (Algorithm::Scale2x, 4) => {
                scale2x(&self.src_px, w, h, &mut self.buf_a);
                scale2x(&self.buf_a, w * 2, h * 2, &mut self.buf_b);
                &self.buf_b
            }
            (Algorithm::Mmpx, 2) => {
                mmpx(&self.src_px, w, h, &mut self.buf_a);
                &self.buf_a
            }
            (Algorithm::Mmpx, 4) => {
                mmpx(&self.src_px, w, h, &mut self.buf_a);
                mmpx(&self.buf_a, w * 2, h * 2, &mut self.buf_b);
                &self.buf_b
            }
            (Algorithm::LcdGrid, n) => {
                nearest(&self.src_px, w, h, n, &mut self.buf_a);
                lcd_grid(&mut self.buf_a, w * n, h * n, n);
                &self.buf_a
            }
            (algo, f) => unreachable!("unsupported factor {f} for {algo:?}"),
        };

        self.out.clear();
        self.out.reserve(result.len() * 4);
        for &px in result {
            self.out.extend_from_slice(&px.to_ne_bytes());
        }
        &self.out
    }
}

/// Integer pixel duplication: each source pixel becomes an `n`×`n` block.
fn nearest(src: &[u32], w: usize, h: usize, n: usize, dst: &mut Vec<u32>) {
    let (ow, oh) = (w * n, h * n);
    dst.clear();
    dst.resize(ow * oh, 0);
    for y in 0..h {
        // Build one output row, then replicate it n times.
        let row_start = y * n * ow;
        for x in 0..w {
            let px = src[y * w + x];
            let base = row_start + x * n;
            for i in 0..n {
                dst[base + i] = px;
            }
        }
        for i in 1..n {
            let (head, tail) = dst.split_at_mut(row_start + i * ow);
            tail[..ow].copy_from_slice(&head[row_start..row_start + ow]);
        }
    }
}

/// EPX/Scale2x: each pixel E with orthogonal neighbours B (up), D (left),
/// F (right), H (down) becomes a 2×2 block. A corner copies the neighbour
/// colour only when the two adjacent orthogonals match each other *exactly*
/// (and the opposite pair does not), so diagonal edges grow smooth steps while
/// flat areas and checkerboards pass through unchanged. Border pixels clamp
/// (replicate the edge). Rule as published at <https://www.scale2x.it/algorithm>.
fn scale2x(src: &[u32], w: usize, h: usize, dst: &mut Vec<u32>) {
    let (ow, oh) = (w * 2, h * 2);
    dst.clear();
    dst.resize(ow * oh, 0);
    let at = |x: isize, y: isize| -> u32 {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        src[y * w + x]
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let e = at(x, y);
            let b = at(x, y - 1);
            let d = at(x - 1, y);
            let f = at(x + 1, y);
            let hh = at(x, y + 1);
            let (mut e0, mut e1, mut e2, mut e3) = (e, e, e, e);
            if b != hh && d != f {
                if d == b {
                    e0 = d;
                }
                if b == f {
                    e1 = f;
                }
                if d == hh {
                    e2 = d;
                }
                if hh == f {
                    e3 = f;
                }
            }
            let o = (y as usize * 2) * ow + x as usize * 2;
            dst[o] = e0;
            dst[o + 1] = e1;
            dst[o + ow] = e2;
            dst[o + ow + 1] = e3;
        }
    }
}

/// Scale3x/AdvMAME3x: the 3×3 generalisation of the same exact-match rule,
/// as published at <https://www.scale2x.it/algorithm>.
fn scale3x(src: &[u32], w: usize, h: usize, dst: &mut Vec<u32>) {
    let (ow, oh) = (w * 3, h * 3);
    dst.clear();
    dst.resize(ow * oh, 0);
    let at = |x: isize, y: isize| -> u32 {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        src[y * w + x]
    };
    for y in 0..h as isize {
        for x in 0..w as isize {
            let a = at(x - 1, y - 1);
            let b = at(x, y - 1);
            let c = at(x + 1, y - 1);
            let d = at(x - 1, y);
            let e = at(x, y);
            let f = at(x + 1, y);
            let g = at(x - 1, y + 1);
            let hh = at(x, y + 1);
            let i = at(x + 1, y + 1);
            let mut o = [e; 9];
            if b != hh && d != f {
                if d == b {
                    o[0] = d;
                }
                if (d == b && e != c) || (b == f && e != a) {
                    o[1] = b;
                }
                if b == f {
                    o[2] = f;
                }
                if (d == b && e != g) || (d == hh && e != a) {
                    o[3] = d;
                }
                if (b == f && e != i) || (hh == f && e != c) {
                    o[5] = f;
                }
                if d == hh {
                    o[6] = d;
                }
                if (d == hh && e != i) || (hh == f && e != g) {
                    o[7] = hh;
                }
                if hh == f {
                    o[8] = f;
                }
            }
            let base = (y as usize * 3) * ow + x as usize * 3;
            for (k, &px) in o.iter().enumerate() {
                dst[base + (k / 3) * ow + (k % 3)] = px;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// MMPX
// ---------------------------------------------------------------------------
//
// Ported from the reference implementation by Morgan McGuire and Mara Gagiu:
//   Copyright 2020 Morgan McGuire & Mara Gagiu.
//   Provided under the Open Source MIT license
//   https://casual-effects.com/research/McGuire2021PixelArt/index.html
// (JCGT vol. 10 no. 2, 2021, "MMPX Style-Preserving Pixel Art Magnification".)
// The port keeps the reference's structure and rule order; only the
// coordinate clamping is written independently (the C99 port we cross-checked
// carries an out-of-range clamp bug we must not reproduce).

/// MMPX's luma: brighter and more transparent sort higher. With the uniform
/// alpha our framebuffer carries, this reduces to `r + g + b + 1`.
#[inline]
fn luma(c: u32) -> u32 {
    let [r, g, b, a] = c.to_ne_bytes();
    (r as u32 + g as u32 + b as u32 + 1) * (256 - a as u32)
}

#[inline]
fn all_eq2(b: u32, a0: u32, a1: u32) -> bool {
    ((b ^ a0) | (b ^ a1)) == 0
}
#[inline]
fn all_eq3(b: u32, a0: u32, a1: u32, a2: u32) -> bool {
    ((b ^ a0) | (b ^ a1) | (b ^ a2)) == 0
}
#[inline]
fn all_eq4(b: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> bool {
    ((b ^ a0) | (b ^ a1) | (b ^ a2) | (b ^ a3)) == 0
}
#[inline]
fn any_eq3(b: u32, a0: u32, a1: u32, a2: u32) -> bool {
    b == a0 || b == a1 || b == a2
}
#[inline]
fn none_eq2(b: u32, a0: u32, a1: u32) -> bool {
    b != a0 && b != a1
}
#[inline]
fn none_eq4(b: u32, a0: u32, a1: u32, a2: u32, a3: u32) -> bool {
    b != a0 && b != a1 && b != a2 && b != a3
}

/// MMPX 2× magnification. Each source pixel E maps to a 2×2 output block
/// J K / L M, initialised to E and then refined by the reference's 1:1-slope,
/// intersection, and 2:1-slope rules. Only colours already present in the
/// source are ever written.
fn mmpx(src_buf: &[u32], w: usize, h: usize, dst: &mut Vec<u32>) {
    let ow = w * 2;
    dst.clear();
    dst.resize(ow * h * 2, 0);
    let src = |x: isize, y: isize| -> u32 {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        src_buf[y * w + x]
    };

    for sy in 0..h as isize {
        for sx in 0..w as isize {
            let a = src(sx - 1, sy - 1);
            let b = src(sx, sy - 1);
            let c = src(sx + 1, sy - 1);
            let d = src(sx - 1, sy);
            let e = src(sx, sy);
            let f = src(sx + 1, sy);
            let g = src(sx - 1, sy + 1);
            let hh = src(sx, sy + 1);
            let i = src(sx + 1, sy + 1);

            let (mut j, mut k, mut l, mut m) = (e, e, e, e);

            // Skip the rules entirely inside flat regions (the common case).
            if ((a ^ e)
                | (b ^ e)
                | (c ^ e)
                | (d ^ e)
                | (f ^ e)
                | (g ^ e)
                | (hh ^ e)
                | (i ^ e))
                != 0
            {
                let q = src(sx - 2, sy);
                let r = src(sx + 2, sy);
                let p = src(sx, sy - 2);
                let s = src(sx, sy + 2);
                let bl = luma(b);
                let dl = luma(d);
                let el = luma(e);
                let fl = luma(f);
                let hl = luma(hh);

                // 1:1 slope rules
                if (d == b && d != hh && d != f)
                    && (el >= dl || e == a)
                    && any_eq3(e, a, c, g)
                    && (el < dl || a != d || e != p || e != q)
                {
                    j = d;
                }
                if (b == f && b != d && b != hh)
                    && (el >= bl || e == c)
                    && any_eq3(e, a, c, i)
                    && (el < bl || c != b || e != p || e != r)
                {
                    k = b;
                }
                if (hh == d && hh != f && hh != b)
                    && (el >= hl || e == g)
                    && any_eq3(e, a, g, i)
                    && (el < hl || g != hh || e != s || e != q)
                {
                    l = hh;
                }
                if (f == hh && f != b && f != d)
                    && (el >= fl || e == i)
                    && any_eq3(e, c, g, i)
                    && (el < fl || i != hh || e != r || e != s)
                {
                    m = f;
                }

                // Intersection rules
                if (e != f && all_eq4(e, c, i, d, q) && all_eq2(f, b, hh))
                    && f != src(sx + 3, sy)
                {
                    k = f;
                    m = f;
                }
                if (e != d && all_eq4(e, a, g, f, r) && all_eq2(d, b, hh))
                    && d != src(sx - 3, sy)
                {
                    j = d;
                    l = d;
                }
                if (e != hh && all_eq4(e, g, i, b, p) && all_eq2(hh, d, f))
                    && hh != src(sx, sy + 3)
                {
                    l = hh;
                    m = hh;
                }
                if (e != b && all_eq4(e, a, c, hh, s) && all_eq2(b, d, f))
                    && b != src(sx, sy - 3)
                {
                    j = b;
                    k = b;
                }
                if bl < el && all_eq4(e, g, hh, i, s) && none_eq4(e, a, d, c, f) {
                    j = b;
                    k = b;
                }
                if hl < el && all_eq4(e, a, b, c, p) && none_eq4(e, d, g, i, f) {
                    l = hh;
                    m = hh;
                }
                if fl < el && all_eq4(e, a, d, g, q) && none_eq4(e, b, c, i, hh) {
                    k = f;
                    m = f;
                }
                if dl < el && all_eq4(e, c, f, i, r) && none_eq4(e, b, a, g, hh) {
                    j = d;
                    l = d;
                }

                // 2:1 slope rules
                if hh != b {
                    if hh != a && hh != e && hh != c {
                        if all_eq3(hh, g, f, r) && none_eq2(hh, d, src(sx + 2, sy - 1)) {
                            l = m;
                        }
                        if all_eq3(hh, i, d, q) && none_eq2(hh, f, src(sx - 2, sy - 1)) {
                            m = l;
                        }
                    }
                    if b != i && b != g && b != e {
                        if all_eq3(b, a, f, r) && none_eq2(b, d, src(sx + 2, sy + 1)) {
                            j = k;
                        }
                        if all_eq3(b, c, d, q) && none_eq2(b, f, src(sx - 2, sy + 1)) {
                            k = j;
                        }
                    }
                }
                if f != d {
                    if d != i && d != e && d != c {
                        if all_eq3(d, a, hh, s) && none_eq2(d, b, src(sx + 1, sy + 2)) {
                            j = l;
                        }
                        if all_eq3(d, g, b, p) && none_eq2(d, hh, src(sx + 1, sy - 2)) {
                            l = j;
                        }
                    }
                    if f != e && f != a && f != g {
                        if all_eq3(f, c, hh, s) && none_eq2(f, b, src(sx - 1, sy + 2)) {
                            k = m;
                        }
                        if all_eq3(f, i, b, p) && none_eq2(f, hh, src(sx - 1, sy - 2)) {
                            m = k;
                        }
                    }
                }
            }

            let o = (sy as usize * 2) * ow + sx as usize * 2;
            dst[o] = j;
            dst[o + 1] = k;
            dst[o + ow] = l;
            dst[o + ow + 1] = m;
        }
    }
}

/// Darken a 1-px gap along the right and bottom edge of every `n`×`n` cell,
/// per axis, so cell corners darken twice — reads as a grid of slightly round
/// LCD dots. The multiplier softens at small factors where the gap is a large
/// share of the cell. Alpha is preserved.
fn lcd_grid(buf: &mut [u32], ow: usize, oh: usize, n: usize) {
    debug_assert_eq!(buf.len(), ow * oh);
    // Per-axis brightness multiplier for gap pixels, out of 256.
    let mul: u32 = match n {
        0 | 1 => return,
        2 => 224,
        3 => 200,
        _ => 176,
    };
    let darken = |px: u32| -> u32 {
        let [r, g, b, a] = px.to_ne_bytes();
        u32::from_ne_bytes([
            ((r as u32 * mul) >> 8) as u8,
            ((g as u32 * mul) >> 8) as u8,
            ((b as u32 * mul) >> 8) as u8,
            a,
        ])
    };
    for y in 0..oh {
        let gap_row = y % n == n - 1;
        let row = y * ow;
        for x in 0..ow {
            let gap_col = x % n == n - 1;
            if gap_row {
                buf[row + x] = darken(buf[row + x]);
            }
            if gap_col {
                buf[row + x] = darken(buf[row + x]);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The four exact DMG framebuffer shades as RGBA bytes (alpha 255).
    const SHADES: [u8; 4] = [255, 192, 96, 0];

    fn rgba(luma: u8) -> [u8; 4] {
        [luma, luma, luma, 255]
    }

    /// Build a w×h RGBA image from per-pixel luma values.
    fn img(pixels: &[u8], _w: usize) -> Vec<u8> {
        pixels.iter().flat_map(|&l| rgba(l)).collect()
    }

    /// Deterministic 4-shade noise image (LCG), for property tests.
    fn noise(w: usize, h: usize, seed: u64) -> Vec<u8> {
        let mut state = seed;
        let mut out = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let shade = SHADES[(state >> 33) as usize % 4];
            out.extend_from_slice(&rgba(shade));
        }
        out
    }

    fn distinct_colors(rgba: &[u8]) -> std::collections::HashSet<[u8; 4]> {
        rgba.chunks_exact(4)
            .map(|c| [c[0], c[1], c[2], c[3]])
            .collect()
    }

    #[test]
    fn output_dimensions_match_factor() {
        for algo in [
            Algorithm::Nearest,
            Algorithm::Scale2x,
            Algorithm::Mmpx,
            Algorithm::LcdGrid,
        ] {
            for &f in algo.supported_factors() {
                let mut up = Upscaler::new(algo, f);
                assert_eq!(up.factor(), f, "{algo:?} must accept its own factor {f}");
                let src = noise(160, 144, 7);
                let out = up.apply(&src, 160, 144);
                assert_eq!(
                    out.len(),
                    160 * 144 * 4 * (f as usize) * (f as usize),
                    "{algo:?} x{f} output size"
                );
            }
        }
    }

    #[test]
    fn factor_one_is_a_noop() {
        let mut up = Upscaler::new(Algorithm::Nearest, 1);
        let src = noise(160, 144, 1);
        let out = up.apply(&src, 160, 144);
        assert_eq!(out, &src[..], "factor 1 must return the input unchanged");
    }

    #[test]
    fn unsupported_factors_snap_to_nearest_supported() {
        assert_eq!(Upscaler::new(Algorithm::Scale2x, 1).factor(), 2);
        assert_eq!(Upscaler::new(Algorithm::Scale2x, 9).factor(), 4);
        assert_eq!(
            Upscaler::new(Algorithm::Mmpx, 3).factor(),
            2,
            "tie resolves down"
        );
        assert_eq!(Upscaler::new(Algorithm::Mmpx, 9).factor(), 4);
        assert_eq!(Upscaler::new(Algorithm::LcdGrid, 1).factor(), 2);
    }

    #[test]
    fn algorithm_and_factor_cycles_are_closed() {
        let mut a = Algorithm::Nearest;
        for _ in 0..4 {
            a = a.next();
        }
        assert_eq!(a, Algorithm::Nearest);

        for algo in [
            Algorithm::Nearest,
            Algorithm::Scale2x,
            Algorithm::Mmpx,
            Algorithm::LcdGrid,
        ] {
            let first = algo.supported_factors()[0];
            let mut f = first;
            for _ in 0..algo.supported_factors().len() {
                f = algo.next_factor(f);
                assert!(algo.supported_factors().contains(&f));
            }
            assert_eq!(f, first, "{algo:?} factor cycle must wrap");
        }
    }

    #[test]
    fn nearest_replicates_blocks_exactly() {
        // 2x1 source: dark then light.
        let src = img(&[0, 255], 2);
        let mut up = Upscaler::new(Algorithm::Nearest, 3);
        let out = up.apply(&src, 2, 1).to_vec();
        // Output is 6x3; left 3x3 block dark, right 3x3 block light.
        for y in 0..3 {
            for x in 0..6 {
                let px = &out[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4];
                let want = if x < 3 { rgba(0) } else { rgba(255) };
                assert_eq!(px, want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn scale2x_flat_field_stays_flat() {
        let src = img(&[96; 12], 4);
        let mut up = Upscaler::new(Algorithm::Scale2x, 2);
        let out = up.apply(&src, 4, 3);
        assert!(out.chunks_exact(4).all(|c| c == rgba(96)));
    }

    #[test]
    fn scale2x_smooths_a_diagonal_step() {
        // 3x3 with a dark diagonal on light ground:
        //   D . .        (D = 0, . = 255)
        //   . D .
        //   . . D
        // For the centre pixel E=D(dark): B(up)=light, H(down)=light -> B==H,
        // so the centre block stays dark. For pixel (1,0) (light, right of the
        // top-left dark): B=up=light(clamped), D=left=dark, F=right=light,
        // H=down=dark. B!=H, D!=F -> E2 (bottom-left) takes D==H -> dark.
        let src = img(&[0, 255, 255, 255, 0, 255, 255, 255, 0], 3);
        let mut up = Upscaler::new(Algorithm::Scale2x, 2);
        let out = up.apply(&src, 3, 3).to_vec();
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 6 + x) * 4;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        // The corner of the step at output (2,1) — bottom-left of source (1,0)
        // — must have been filled dark, smoothing the staircase.
        assert_eq!(px(2, 1), rgba(0), "diagonal step must be smoothed");
        // Centre of the diagonal stays dark.
        assert_eq!(px(2, 2), rgba(0));
        assert_eq!(px(3, 3), rgba(0));
        // Far corners stay light.
        assert_eq!(px(5, 0), rgba(255));
        assert_eq!(px(0, 5), rgba(255));
    }

    #[test]
    fn exact_match_scalers_never_invent_colors() {
        let src = noise(64, 48, 42);
        let src_colors = distinct_colors(&src);
        for (algo, factors) in [
            (Algorithm::Scale2x, &[2u8, 3, 4][..]),
            (Algorithm::Mmpx, &[2u8, 4][..]),
            (Algorithm::Nearest, &[2u8, 5][..]),
        ] {
            for &f in factors {
                let mut up = Upscaler::new(algo, f);
                let out = up.apply(&src, 64, 48);
                let out_colors = distinct_colors(out);
                assert!(
                    out_colors.is_subset(&src_colors),
                    "{algo:?} x{f} invented colours not in the 4-shade source"
                );
            }
        }
    }

    #[test]
    fn mmpx_flat_field_stays_flat() {
        let src = img(&[192; 20], 5);
        let mut up = Upscaler::new(Algorithm::Mmpx, 2);
        let out = up.apply(&src, 5, 4);
        assert!(out.chunks_exact(4).all(|c| c == rgba(192)));
    }

    #[test]
    fn mmpx_preserves_an_isolated_dot() {
        // A single dark pixel on a light field must survive as an exact 2x2
        // block — MMPX's headline property (style preservation) vs. scalers
        // that erode single-pixel details.
        let mut lumas = [255u8; 25];
        lumas[12] = 0; // centre of 5x5
        let src = img(&lumas, 5);
        let mut up = Upscaler::new(Algorithm::Mmpx, 2);
        let out = up.apply(&src, 5, 5).to_vec();
        let px = |x: usize, y: usize| -> u8 { out[(y * 10 + x) * 4] };
        for y in 0..10 {
            for x in 0..10 {
                let want = if (4..6).contains(&x) && (4..6).contains(&y) {
                    0
                } else {
                    255
                };
                assert_eq!(px(x, y), want, "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn mmpx_smooths_a_diagonal_step() {
        // The same diagonal as the scale2x test must come out as a connected
        // smooth line with rounded ends. Expected output verified against the
        // authors' reference implementation (byte-identical on this input and
        // on seeded 4-shade noise images).
        let src = img(&[0, 255, 255, 255, 0, 255, 255, 255, 0], 3);
        let mut up = Upscaler::new(Algorithm::Mmpx, 2);
        let out = up.apply(&src, 3, 3).to_vec();
        let want = [
            "##....", //
            "#.#...", //
            ".###..", //
            "..###.", //
            "...#.#", //
            "....##",
        ];
        for (y, row) in want.iter().enumerate() {
            for (x, ch) in row.bytes().enumerate() {
                let dark = out[(y * 6 + x) * 4] == 0;
                assert_eq!(dark, ch == b'#', "pixel ({x},{y})");
            }
        }
    }

    #[test]
    fn lcd_grid_darkens_gaps_and_preserves_cell_bodies() {
        let src = img(&[255; 4], 2);
        let mut up = Upscaler::new(Algorithm::LcdGrid, 4);
        let out = up.apply(&src, 2, 2).to_vec();
        let px = |x: usize, y: usize| -> [u8; 4] {
            let i = (y * 8 + x) * 4;
            [out[i], out[i + 1], out[i + 2], out[i + 3]]
        };
        // Cell body (not on a gap line) is untouched.
        assert_eq!(px(0, 0), rgba(255));
        assert_eq!(px(2, 1), rgba(255));
        // Gap column and row are darkened, alpha untouched.
        let gap = px(3, 0);
        assert!(
            gap[0] < 255 && gap[1] < 255 && gap[2] < 255,
            "gap must darken"
        );
        assert_eq!(gap[3], 255, "alpha must be preserved");
        // Corner (gap row AND column) is darker still.
        let corner = px(3, 3);
        assert!(corner[0] < gap[0], "corner darkens twice");
        // All cells look identical.
        assert_eq!(px(3, 0), px(7, 0));
        assert_eq!(px(3, 3), px(7, 7));
    }

    #[test]
    fn switching_algorithm_reclamps_factor() {
        let mut up = Upscaler::new(Algorithm::Nearest, 5);
        up.set_algorithm(Algorithm::Mmpx);
        assert_eq!(up.factor(), 4, "5 snaps to MMPX's nearest factor");
        up.set_algorithm(Algorithm::Nearest);
        assert_eq!(up.factor(), 4, "factor kept where supported");
    }

    #[test]
    fn from_str_aliases() {
        assert_eq!(Algorithm::from_str("Scale2x"), Some(Algorithm::Scale2x));
        assert_eq!(Algorithm::from_str("epx"), Some(Algorithm::Scale2x));
        assert_eq!(Algorithm::from_str("MMPX"), Some(Algorithm::Mmpx));
        assert_eq!(Algorithm::from_str("lcd"), Some(Algorithm::LcdGrid));
        assert_eq!(Algorithm::from_str("dotmatrix"), Some(Algorithm::LcdGrid));
        assert_eq!(Algorithm::from_str("nearest"), Some(Algorithm::Nearest));
        assert_eq!(Algorithm::from_str("off"), Some(Algorithm::Nearest));
        assert_eq!(Algorithm::from_str("hqx"), None);
    }

    #[test]
    fn apply_is_reusable_across_algorithms() {
        // One Upscaler instance must survive live switching (the TUI cycles
        // algorithms with a key while the session runs).
        let src = noise(160, 144, 3);
        let mut up = Upscaler::new(Algorithm::Nearest, 2);
        let n = up.apply(&src, 160, 144).len();
        up.cycle_algorithm(); // scale2x
        let s = up.apply(&src, 160, 144).len();
        up.cycle_algorithm(); // mmpx
        let m = up.apply(&src, 160, 144).len();
        assert_eq!(n, s);
        assert_eq!(s, m);
    }
}

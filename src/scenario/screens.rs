//! A run's own picture, downscaled and packed, so many runs can be watched at
//! once.
//!
//! [`RunResult::path`](super::run::RunResult::path) records where the player
//! **was**; it carries no picture at all, so a mosaic of dozens of screens
//! cannot be derived from a corpus that has already been collected. It needs a
//! capture pass, and a capture pass has to be designed for size before a byte
//! is written: six hundred agents recording a 160x144 RGBA framebuffer sixty
//! times a second is 3.3 GB **per emulated minute per agent**.
//!
//! Three decisions bring that down by about four orders of magnitude, and every
//! one of them is a property of the machine rather than a compression trick:
//!
//! 1. **Two bits per pixel, because that is the Game Boy's own depth.** A DMG
//!    frame is four shades; storing it as RGBA stores the same four values in
//!    thirty-two bits. The quantisation here is by **luminance rank** — the
//!    same rule `tools/conformance/image.py` and `testharness/pngdec.rs` use to
//!    compare a frame against a reference, and the reason it is a rank rather
//!    than a palette lookup is that our shades are `000000/606060/c0c0c0/ffffff`
//!    and nobody else's are.
//! 2. **Downscale, because a mosaic tile is small.** Forty-eight tiles in a
//!    1280-wide frame is 160 px of tile before any gap, so a half-scale 80x72
//!    capture is already at or above what will be drawn.
//! 3. **Sample, because a mosaic is a timelapse.** One frame in twelve is five
//!    samples per emulated second, which is finer than any pace a mosaic is
//!    watchable at.
//!
//! At scale 2 sampling every 12 frames that is **1,440 bytes per sample**, or
//! about 7 KB per emulated second per agent — [`estimate`] is the arithmetic,
//! and every front end prints it *before* recording rather than afterwards.
//!
//! ## The format, `GBSCR1`
//!
//! A 24-byte header then the samples back to back, each `height *
//! ceil(width/4)` bytes, two bits per pixel, most significant bits first, rows
//! padded to a whole byte. Sample `i` is emulated frame `(i + 1) * every` of
//! its run, because a sample is taken *after* the frame it names.
//!
//! Hand-rolled, like every other codec in this repository: the dependency count
//! is a contract `make embed` measures.

/// The magic every file opens with. The trailing version digit is part of it,
/// so a reader that does not understand a later format says so rather than
/// decoding rubbish.
pub const MAGIC: &[u8; 8] = b"GBSCR1\0\0";

/// A Game Boy screen is this big whatever we do to it afterwards.
pub const SCREEN_W: usize = 160;
pub const SCREEN_H: usize = 144;

/// The header's size in bytes.
pub const HEADER: usize = 24;

/// Bytes one sample occupies at a given downscale factor.
pub fn bytes_per_sample(scale: u32) -> usize {
    let (w, h) = dims(scale);
    h * w.div_ceil(4)
}

/// The captured size at a given downscale factor.
pub fn dims(scale: u32) -> (usize, usize) {
    let s = scale.max(1) as usize;
    (SCREEN_W.div_ceil(s), SCREEN_H.div_ceil(s))
}

/// What a capture will cost on disk, in bytes, for one run.
///
/// `frames` is the run's length in emulated frames — for a bound before
/// anything has been played, a scenario's own `timeout_frames` is the honest
/// worst case, because that is the most frames an attempt is allowed.
pub fn estimate(every: u32, scale: u32, frames: u64) -> u64 {
    let n = frames / every.max(1) as u64;
    HEADER as u64 + n * bytes_per_sample(scale) as u64
}

/// Bytes rendered as a human-readable size. Small numbers stay small: a
/// capture reported as `0 MB` is one nobody can size.
pub fn human(bytes: u64) -> String {
    const K: f64 = 1024.0;
    let b = bytes as f64;
    if b < K {
        format!("{bytes} B")
    } else if b < K * K {
        format!("{:.1} KiB", b / K)
    } else if b < K * K * K {
        format!("{:.1} MiB", b / (K * K))
    } else {
        format!("{:.2} GiB", b / (K * K * K))
    }
}

/// Every screen one run produced, at the depth and rate it was asked for.
#[derive(Debug, Clone)]
pub struct ScreenTrack {
    every: u32,
    scale: u32,
    width: u16,
    height: u16,
    frames: u32,
    data: Vec<u8>,
}

impl ScreenTrack {
    pub fn new(every: u32, scale: u32) -> ScreenTrack {
        let (w, h) = dims(scale);
        ScreenTrack {
            every: every.max(1),
            scale: scale.max(1),
            width: w as u16,
            height: h as u16,
            frames: 0,
            data: Vec::new(),
        }
    }

    pub fn every(&self) -> u32 {
        self.every
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }

    pub fn len(&self) -> usize {
        HEADER + self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames == 0
    }

    /// Take one 160x144 RGBA framebuffer.
    ///
    /// **Sampled at pixel centres**, not at the top-left corner of each block —
    /// the rule `video::fit_maps` and `screen::halfblocks` already follow, and
    /// for the same reason: edge sampling never reaches the last source column
    /// or row and leans the whole picture towards the origin.
    pub fn push(&mut self, rgba: &[u8]) {
        if rgba.len() < SCREEN_W * SCREEN_H * 4 {
            return;
        }
        let s = self.scale as usize;
        let (w, h) = (self.width as usize, self.height as usize);
        let stride = w.div_ceil(4);
        let base = self.data.len();
        self.data.resize(base + h * stride, 0);
        for y in 0..h {
            let sy = ((2 * y + 1) * s / 2).min(SCREEN_H - 1);
            for x in 0..w {
                let sx = ((2 * x + 1) * s / 2).min(SCREEN_W - 1);
                let p = (sy * SCREEN_W + sx) * 4;
                let v = shade(rgba[p], rgba[p + 1], rgba[p + 2]);
                let byte = base + y * stride + x / 4;
                let shift = 6 - 2 * (x % 4);
                self.data[byte] |= v << shift;
            }
        }
        self.frames += 1;
    }

    /// The file: header then samples.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&(self.every as u16).to_le_bytes());
        out.extend_from_slice(&(self.scale as u16).to_le_bytes());
        out.extend_from_slice(&self.frames.to_le_bytes());
        out.extend_from_slice(&[0u8; 4]); // reserved, so the header stays 24
        out.extend_from_slice(&self.data);
        out
    }
}

/// Luminance rank, 0 darkest to 3 lightest.
///
/// **A rank, not a palette lookup.** Our DMG shades are
/// `000000/606060/c0c0c0/ffffff`; a Color frame, a recoloured one or another
/// emulator's are not, and the thresholds below sit between our four so the
/// mapping is exact for them and still monotone for anything else. Rec. 601
/// weights, in integers, because this runs once per captured frame.
fn shade(r: u8, g: u8, b: u8) -> u8 {
    let l = (54 * r as u32 + 183 * g as u32 + 19 * b as u32) >> 8;
    match l {
        0..=0x2F => 0,
        0x30..=0x8F => 1,
        0x90..=0xDF => 2,
        _ => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = vec![255u8; SCREEN_W * SCREEN_H * 4];
        for y in 0..SCREEN_H {
            for x in 0..SCREEN_W {
                let g = f(x, y);
                let p = (y * SCREEN_W + x) * 4;
                v[p] = g;
                v[p + 1] = g;
                v[p + 2] = g;
            }
        }
        v
    }

    fn read(t: &ScreenTrack, i: usize, x: usize, y: usize) -> u8 {
        let bytes = t.encode();
        let (w, h) = (t.width as usize, t.height as usize);
        let stride = w.div_ceil(4);
        let base = HEADER + i * h * stride;
        let byte = bytes[base + y * stride + x / 4];
        (byte >> (6 - 2 * (x % 4))) & 3
    }

    /// Our own four shades must map onto the four ranks exactly. This is the
    /// whole of the depth claim: anything else and the mosaic is showing a
    /// picture the emulator did not draw.
    #[test]
    fn the_four_dmg_shades_map_onto_the_four_ranks() {
        for (v, want) in [(0x00u8, 0u8), (0x60, 1), (0xC0, 2), (0xFF, 3)] {
            assert_eq!(shade(v, v, v), want, "shade 0x{v:02X}");
        }
    }

    /// A downscale that samples the top-left corner of each block never reaches
    /// the last source column, so the picture leans towards the origin. Mirror
    /// the source and the capture must mirror with it; that is a property
    /// centre sampling has and edge sampling does not.
    #[test]
    fn the_capture_is_symmetric_because_it_samples_pixel_centres() {
        let a = {
            let mut t = ScreenTrack::new(1, 2);
            t.push(&frame(|x, _| if x < SCREEN_W / 2 { 0x00 } else { 0xFF }));
            t
        };
        let b = {
            let mut t = ScreenTrack::new(1, 2);
            t.push(&frame(|x, _| if x < SCREEN_W / 2 { 0xFF } else { 0x00 }));
            t
        };
        let w = a.width as usize;
        for x in 0..w {
            assert_eq!(
                read(&a, 0, x, 0),
                read(&b, 0, w - 1 - x, 0),
                "column {x} is not the mirror of column {}",
                w - 1 - x
            );
        }
    }

    /// The size a front end prints before recording has to be the size that
    /// lands, or the estimate is decoration.
    #[test]
    fn the_estimate_is_the_size_that_lands() {
        let mut t = ScreenTrack::new(12, 2);
        for i in 0..10 {
            t.push(&frame(|_, _| (i * 20) as u8));
        }
        assert_eq!(t.len() as u64, estimate(12, 2, 12 * 10));
        assert_eq!(bytes_per_sample(2), 72 * 20);
    }

    #[test]
    fn the_header_is_readable_and_says_what_follows() {
        let mut t = ScreenTrack::new(6, 4);
        t.push(&frame(|_, _| 0));
        let b = t.encode();
        assert_eq!(&b[..8], MAGIC);
        assert_eq!(u16::from_le_bytes([b[8], b[9]]), 40);
        assert_eq!(u16::from_le_bytes([b[10], b[11]]), 36);
        assert_eq!(u16::from_le_bytes([b[12], b[13]]), 6);
        assert_eq!(u16::from_le_bytes([b[14], b[15]]), 4);
        assert_eq!(u32::from_le_bytes([b[16], b[17], b[18], b[19]]), 1);
        assert_eq!(b.len(), HEADER + 36 * 10);
    }
}

//! The display pipeline stage between the emulator and the terminal: palette
//! mapping into a reused buffer.
//!
//! Kept separate from [`crate::screen::tui`] so the render benchmark
//! (`examples/render_bench.rs`) measures **the same code the frontend runs**,
//! not a copy of it.
//!
//! Order matters: palette first, upscaler second. The LCD-grid filter darkens
//! gaps between pixels, and the palette buckets to four shades — running the
//! palette last would quantise the grid away.

use crate::screen::config::Palette;

/// Owns the palette-mapped RGBA buffer for a session, so a frame costs no
/// allocation after the first.
#[derive(Default)]
pub struct FramePrep {
    rgba: Vec<u8>,
}

impl FramePrep {
    pub fn new() -> FramePrep {
        FramePrep::default()
    }

    /// Recolour the emulator's RGBA framebuffer through `palette`, returning
    /// the mapped RGBA bytes.
    ///
    /// DMG output is greyscale (`r == g == b`) and gets recoloured; colour
    /// (CGB) pixels pass through untouched, which is why this is a per-pixel
    /// test rather than a blanket mapping.
    pub fn map(&mut self, src: &[u8], palette: Palette) -> &[u8] {
        self.map_with(src, palette, |_| {})
    }

    /// [`FramePrep::map`] with a hook that may perturb the mapped buffer before
    /// it is handed on. Exists for `examples/render_bench.rs`, which forces
    /// every frame to differ so it measures the encoder's worst case rather
    /// than its unchanged-frame fast path.
    pub fn map_with(
        &mut self,
        src: &[u8],
        palette: Palette,
        tweak: impl FnOnce(&mut [u8]),
    ) -> &[u8] {
        self.map_inner(src, palette);
        tweak(&mut self.rgba);
        &self.rgba
    }

    fn map_inner(&mut self, src: &[u8], palette: Palette) {
        if self.rgba.len() != src.len() {
            self.rgba = vec![255u8; src.len()];
        }
        if palette == Palette::Grayscale {
            // Identity: the emulator already emits opaque RGBA.
            self.rgba.copy_from_slice(src);
            return;
        }
        // Four shades in, four out: a 256-entry LUT would be overkill, but the
        // four exact framebuffer values are worth short-circuiting.
        for (dst, chunk) in self.rgba.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            let (r, g, b) = (chunk[0], chunk[1], chunk[2]);
            if r == g && g == b {
                dst[..3].copy_from_slice(&palette.map(r));
            } else {
                dst[..3].copy_from_slice(&chunk[..3]);
            }
            dst[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grayscale_is_a_pass_through() {
        let src: Vec<u8> = (0..64u8).collect();
        let mut p = FramePrep::new();
        assert_eq!(p.map(&src, Palette::Grayscale), &src[..]);
    }

    #[test]
    fn green_maps_the_four_shades_and_keeps_alpha() {
        let src = [255, 255, 255, 255, 96, 96, 96, 255];
        let mut p = FramePrep::new();
        let out = p.map(&src, Palette::Green);
        assert_eq!(&out[..4], &[224, 248, 208, 255]);
        assert_eq!(&out[4..], &[52, 104, 86, 255]);
    }

    #[test]
    fn colour_pixels_pass_through_untouched() {
        let src = [10, 20, 30, 255];
        let mut p = FramePrep::new();
        assert_eq!(p.map(&src, Palette::Green), &[10, 20, 30, 255]);
    }

    #[test]
    fn buffer_is_reused_across_frames() {
        let mut p = FramePrep::new();
        let a = p.map(&[0u8; 16], Palette::Green).as_ptr();
        let b = p.map(&[255u8; 16], Palette::Green).as_ptr();
        assert_eq!(a, b, "palette buffer must not be reallocated per frame");
    }
}

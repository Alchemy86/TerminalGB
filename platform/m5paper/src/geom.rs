//! Panel geometry: inks, rotation and the 4bpp framebuffer.
//!
//! Deliberately free of any `esp-hal` dependency so it compiles and tests on
//! the host. The captain asked for landscape and nobody on this end can see the
//! panel, so the rotation maths is checked by `rustc --test` against this very
//! file rather than asserted — run `tools/m5paper-build.sh --test`.

/// Native panel width, in pixels. Not the logical width - see [`Rotation`].
pub const PANEL_W: usize = 400;
/// Native panel height, in pixels.
pub const PANEL_H: usize = 600;
/// Two pixels per byte, so a native row is 200 bytes.
pub const ROW_BYTES: usize = (PANEL_W + 1) / 2;
/// The whole 4bpp framebuffer: 120,000 bytes, which fits internal SRAM.
pub const FB_BYTES: usize = ROW_BYTES * PANEL_H;

/// The six inks this panel can actually produce, with their native 4bpp codes.
///
/// There is no grey in the set. Codes 0x4 and 0x7 are unused by the controller.
/// RGB values are M5GFX's `epd_palette`, which the device's own firmware also
/// carries as `EPD_PALETTE_ORIGINAL`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Ink {
    Black = 0x0,
    White = 0x1,
    Yellow = 0x2,
    Red = 0x3,
    Blue = 0x5,
    Green = 0x6,
}

impl Ink {
    /// Every ink, in the panel's own code order. Useful for a colour sweep.
    pub const ALL: [Ink; 6] = [
        Ink::Black,
        Ink::White,
        Ink::Yellow,
        Ink::Red,
        Ink::Blue,
        Ink::Green,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Ink::Black => "black",
            Ink::White => "white",
            Ink::Yellow => "yellow",
            Ink::Red => "red",
            Ink::Blue => "blue",
            Ink::Green => "green",
        }
    }
}

/// How logical coordinates map onto the physically-portrait panel.
///
/// The captain asked for landscape, so [`Rotation::Landscape`] is the default
/// everywhere in this crate. The two landscape variants differ only in which
/// way up the device sits; pick whichever matches the case in hand.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
// Portrait and LandscapeFlipped are not constructed by the shipping binary -
// ORIENTATION picks Landscape - but they are part of the driver's surface and
// are exercised by the host tests at the bottom of this file.
#[allow(dead_code)]
pub enum Rotation {
    /// 400 wide x 600 tall — the panel's own orientation.
    Portrait,
    /// 600 wide x 400 tall, USB port on the left.
    Landscape,
    /// 600 wide x 400 tall, USB port on the right.
    LandscapeFlipped,
}

impl Rotation {
    /// Logical width under this rotation.
    pub const fn width(self) -> usize {
        match self {
            Rotation::Portrait => PANEL_W,
            _ => PANEL_H,
        }
    }

    /// Logical height under this rotation.
    pub const fn height(self) -> usize {
        match self {
            Rotation::Portrait => PANEL_H,
            _ => PANEL_W,
        }
    }

    /// Map a logical pixel to its native panel position.
    #[inline]
    /// Same mapping, public so the host renderer can decode a framebuffer back
    /// into the view an eye actually sees.
    pub fn to_panel_pub(self, x: usize, y: usize) -> (usize, usize) {
        self.to_panel(x, y)
    }

    pub(crate) fn to_panel(self, x: usize, y: usize) -> (usize, usize) {
        match self {
            Rotation::Portrait => (x, y),
            // 90 degrees clockwise: the portrait top-right becomes our top-left.
            Rotation::Landscape => (PANEL_W - 1 - y, x),
            // The same rotation the other way round.
            Rotation::LandscapeFlipped => (y, PANEL_H - 1 - x),
        }
    }
}

/// A 4bpp framebuffer in the panel's native layout, addressed in logical
/// (rotated) coordinates.
pub struct FrameBuffer<'a> {
    buf: &'a mut [u8; FB_BYTES],
    rotation: Rotation,
}

impl<'a> FrameBuffer<'a> {
    pub fn new(buf: &'a mut [u8; FB_BYTES], rotation: Rotation) -> Self {
        Self { buf, rotation }
    }

    #[allow(dead_code)] // read by host tests and by future callers
    pub const fn rotation(&self) -> Rotation {
        self.rotation
    }
    pub const fn width(&self) -> usize {
        self.rotation.width()
    }
    pub const fn height(&self) -> usize {
        self.rotation.height()
    }
    pub fn raw(&self) -> &[u8; FB_BYTES] {
        self.buf
    }

    /// Fill the whole panel with one ink.
    pub fn clear(&mut self, ink: Ink) {
        let b = (ink as u8) << 4 | (ink as u8);
        self.buf.fill(b);
    }

    /// Set one logical pixel. Out-of-range coordinates are ignored.
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, ink: Ink) {
        if x >= self.width() || y >= self.height() {
            return;
        }
        let (px, py) = self.rotation.to_panel(x, y);
        let idx = py * ROW_BYTES + (px >> 1);
        let c = ink as u8;
        // High nibble is the even (left) pixel of the pair, low nibble the odd.
        if px & 1 == 0 {
            self.buf[idx] = (self.buf[idx] & 0x0F) | (c << 4);
        } else {
            self.buf[idx] = (self.buf[idx] & 0xF0) | c;
        }
    }

    /// Fill a logical rectangle, clipped to the screen.
    pub fn rect(&mut self, x: usize, y: usize, w: usize, h: usize, ink: Ink) {
        for yy in y..(y + h).min(self.height()) {
            for xx in x..(x + w).min(self.width()) {
                self.set(xx, yy, ink);
            }
        }
    }
}

/// An asymmetric test card: it reads correctly only in landscape, and only the
/// right way up. Six vertical bands in the panel's own ink order, a black
/// marker block in the top-left corner, and a one-pixel border.
pub fn draw_test_card(fb: &mut FrameBuffer<'_>) {
    let (w, h) = (fb.width(), fb.height());
    fb.clear(Ink::White);

    // Six bands across the long (600 px) axis, in the controller's own code
    // order, each boxed in black. The box is not decoration: the white ink is
    // invisible against the white background without it, so an unboxed card
    // shows five inks and quietly claims to show six.
    let band = w / 6;
    let (top, tall) = (h / 3, h / 3);
    fb.rect(0, top - 2, band * 6, tall + 4, Ink::Black);
    for (i, ink) in Ink::ALL.iter().enumerate() {
        fb.rect(i * band + 2, top, band - 4, tall, *ink);
    }

    // Top-left marker. If this block is anywhere but the top-left of a
    // landscape picture, the rotation is wrong and it is obvious at a glance.
    fb.rect(0, 0, 80, 40, Ink::Black);
    // A second, smaller marker below it makes the vertical axis unambiguous too.
    fb.rect(0, 48, 24, 24, Ink::Red);

    // Border, so the panel's real edges are visible.
    fb.rect(0, 0, w, 2, Ink::Black);
    fb.rect(0, h - 2, w, 2, Ink::Black);
    fb.rect(0, 0, 2, h, Ink::Black);
    fb.rect(w - 2, 0, 2, h, Ink::Black);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read one pixel back out of the native buffer, by *native* coordinates.
    fn peek_native(fb: &FrameBuffer<'_>, px: usize, py: usize) -> u8 {
        let b = fb.raw()[py * ROW_BYTES + (px >> 1)];
        if px & 1 == 0 {
            b >> 4
        } else {
            b & 0x0F
        }
    }

    fn blank() -> [u8; FB_BYTES] {
        [0; FB_BYTES]
    }

    #[test]
    fn landscape_is_wider_than_it_is_tall() {
        assert_eq!(Rotation::Landscape.width(), 600);
        assert_eq!(Rotation::Landscape.height(), 400);
        assert_eq!(Rotation::LandscapeFlipped.width(), 600);
        assert_eq!(Rotation::LandscapeFlipped.height(), 400);
        // The panel itself is the other way round, which is the whole point.
        assert_eq!(Rotation::Portrait.width(), 400);
        assert_eq!(Rotation::Portrait.height(), 600);
    }

    /// The logical top-left must land on the panel's top-*right* under a 90
    /// degree clockwise view, and nowhere else. This is the assertion that
    /// stands in for looking at the screen.
    #[test]
    fn landscape_top_left_maps_to_panel_top_right() {
        let mut buf = blank();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        fb.clear(Ink::White);
        fb.set(0, 0, Ink::Black);
        assert_eq!(peek_native(&fb, PANEL_W - 1, 0), Ink::Black as u8);
        // and the panel's own top-left is still white
        assert_eq!(peek_native(&fb, 0, 0), Ink::White as u8);
    }

    #[test]
    fn the_two_landscape_variants_are_180_degrees_apart() {
        let (mut a, mut b) = (blank(), blank());
        let mut fa = FrameBuffer::new(&mut a, Rotation::Landscape);
        let mut fb = FrameBuffer::new(&mut b, Rotation::LandscapeFlipped);
        fa.clear(Ink::White);
        fb.clear(Ink::White);
        fa.set(0, 0, Ink::Black);
        fb.set(599, 399, Ink::Black);
        // Opposite logical corners must land on the same physical pixel.
        assert_eq!(peek_native(&fa, PANEL_W - 1, 0), Ink::Black as u8);
        assert_eq!(peek_native(&fb, PANEL_W - 1, 0), Ink::Black as u8);
    }

    /// Every logical pixel must map to a distinct native pixel: a rotation that
    /// collides is one that silently loses half the picture.
    #[test]
    fn the_mapping_is_a_bijection_in_every_rotation() {
        for rot in [
            Rotation::Portrait,
            Rotation::Landscape,
            Rotation::LandscapeFlipped,
        ] {
            let mut seen = vec![false; PANEL_W * PANEL_H];
            for y in 0..rot.height() {
                for x in 0..rot.width() {
                    let (px, py) = rot.to_panel(x, y);
                    assert!(px < PANEL_W && py < PANEL_H, "{:?} ({x},{y}) escaped", rot);
                    let i = py * PANEL_W + px;
                    assert!(!seen[i], "{:?} maps two pixels onto ({px},{py})", rot);
                    seen[i] = true;
                }
            }
            assert!(
                seen.iter().all(|&s| s),
                "{:?} leaves pixels unreachable",
                rot
            );
        }
    }

    /// The nibble packing must not let one pixel corrupt its neighbour - the
    /// classic 4bpp bug, and invisible on a solid fill.
    #[test]
    fn neighbouring_pixels_share_a_byte_without_clobbering() {
        let mut buf = blank();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Portrait);
        fb.clear(Ink::White);
        fb.set(0, 0, Ink::Red);
        fb.set(1, 0, Ink::Green);
        assert_eq!(peek_native(&fb, 0, 0), Ink::Red as u8);
        assert_eq!(peek_native(&fb, 1, 0), Ink::Green as u8);
        assert_eq!(fb.raw()[0], (Ink::Red as u8) << 4 | Ink::Green as u8);
    }

    #[test]
    fn out_of_range_writes_are_dropped_not_wrapped() {
        let mut buf = blank();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        fb.clear(Ink::White);
        fb.set(600, 0, Ink::Black); // one past the right edge
        fb.set(0, 400, Ink::Black); // one past the bottom
        assert!(fb
            .raw()
            .iter()
            .all(|&b| b == (Ink::White as u8) << 4 | Ink::White as u8));
    }

    /// The test card has to be asymmetric in both axes or it proves nothing
    /// about orientation when the captain looks at it.
    #[test]
    fn the_test_card_marker_is_in_the_logical_top_left_only() {
        let mut buf = blank();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        draw_test_card(&mut fb);
        let corner = |x: usize, y: usize| {
            let (px, py) = Rotation::Landscape.to_panel(x, y);
            peek_native(&fb, px, py)
        };
        // Inside the 80x40 black marker, but off the border.
        assert_eq!(corner(40, 20), Ink::Black as u8);
        // The other three corners are not black marker.
        assert_eq!(corner(559, 20), Ink::White as u8);
        assert_eq!(corner(40, 379), Ink::White as u8);
        assert_eq!(corner(559, 379), Ink::White as u8);
        // The red sub-marker pins the vertical axis.
        assert_eq!(corner(12, 60), Ink::Red as u8);
    }

    /// The card is the only thing that shows the panel's colour range, and the
    /// white band is invisible against the white background unless it is boxed.
    /// An unboxed card shows five inks while claiming six - this catches that.
    #[test]
    fn the_test_card_actually_contains_all_six_inks() {
        let mut buf = blank();
        let mut fb = FrameBuffer::new(&mut buf, Rotation::Landscape);
        draw_test_card(&mut fb);
        for ink in Ink::ALL {
            let mut found = false;
            'scan: for y in 0..PANEL_H {
                for x in 0..PANEL_W {
                    if peek_native(&fb, x, y) == ink as u8 {
                        found = true;
                        break 'scan;
                    }
                }
            }
            assert!(found, "{} is missing from the test card", ink.name());
        }
        // ...and each band must be bounded, or white merges into the paper.
        let (w, h) = (Rotation::Landscape.width(), Rotation::Landscape.height());
        let at = |x: usize, y: usize| {
            let (px, py) = Rotation::Landscape.to_panel(x, y);
            peek_native(&fb, px, py)
        };
        // Just above the band strip is the black box edge, not bare paper.
        assert_eq!(at(w / 2, h / 3 - 1), Ink::Black as u8);
    }

    #[test]
    fn every_ink_has_the_controllers_own_code() {
        // From M5GFX Panel_ED2208. 0x4 and 0x7 are unused by the controller.
        assert_eq!(Ink::Black as u8, 0x0);
        assert_eq!(Ink::White as u8, 0x1);
        assert_eq!(Ink::Yellow as u8, 0x2);
        assert_eq!(Ink::Red as u8, 0x3);
        assert_eq!(Ink::Blue as u8, 0x5);
        assert_eq!(Ink::Green as u8, 0x6);
        assert_eq!(FB_BYTES, 120_000);
    }
}

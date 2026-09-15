//! The 400x600 E Ink Spectra 6 panel (controller ED2208-DOA / EL040EF1).
//!
//! Command set, init list, refresh sequence and the 4bpp colour codes are all
//! transcribed from M5GFX's `Panel_ED2208`, which is the known-working driver
//! for this panel. Nothing here is guessed: where a constant looks arbitrary it
//! came out of that file, and the comment says so.
//!
//! The panel is physically portrait — 400 wide by 600 tall — and the framebuffer
//! is stored in that native layout because that is the only order the controller
//! accepts. Landscape is a coordinate mapping applied on the way in; see
//! [`Rotation`].

use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, Output};
use esp_hal::spi::master::Spi;
use esp_hal::time::Instant;

use crate::geom::{FrameBuffer, PANEL_H, PANEL_W, ROW_BYTES};

/// Wire-level driver for the ED2208 controller.
pub struct Ed2208<'d, Dm: esp_hal::DriverMode> {
    spi: Spi<'d, Dm>,
    cs: Output<'d>,
    dc: Output<'d>,
    rst: Output<'d>,
    busy: Input<'d>,
    delay: Delay,
}

/// Panel init list, verbatim from `Panel_ED2208::getInitCommands` list0.
/// Each entry is `command, length, data...`.
const INIT_LIST: &[&[u8]] = &[
    &[0xAA, 0x49, 0x55, 0x20, 0x08, 0x09, 0x18], // CMDH
    &[0x01, 0x3F],
    &[0x00, 0x5F, 0x69],
    &[0x05, 0x40, 0x1F, 0x1F, 0x2C],
    &[0x08, 0x6F, 0x1F, 0x1F, 0x22],
    &[0x06, 0x6F, 0x1F, 0x17, 0x17],
    &[0x03, 0x03, 0x54, 0x00, 0x44],
    &[0x60, 0x02, 0x00],
    &[0x30, 0x08],
    &[0x50, 0x3F],
    &[0xE3, 0x2F],
    &[0x84, 0x01],
];

impl<'d, Dm: esp_hal::DriverMode> Ed2208<'d, Dm> {
    pub fn new(
        spi: Spi<'d, Dm>,
        cs: Output<'d>,
        dc: Output<'d>,
        rst: Output<'d>,
        busy: Input<'d>,
    ) -> Self {
        Self {
            spi,
            cs,
            dc,
            rst,
            busy,
            delay: Delay::new(),
        }
    }

    /// BUSY reads low while the controller is working, high when it is ready.
    /// Returns false on timeout rather than hanging: a silent hang here is
    /// indistinguishable from a dead panel, and we want to say which it was.
    fn wait_busy(&mut self, timeout_ms: u32) -> bool {
        if self.busy.is_high() {
            return true;
        }
        let start = Instant::now();
        while self.busy.is_low() {
            if start.elapsed().as_millis() > timeout_ms as u64 {
                return false;
            }
            self.delay.delay_millis(10);
        }
        // M5GFX settles for 200 ms after BUSY rises before touching the bus.
        self.delay.delay_millis(200);
        true
    }

    fn cmd(&mut self, c: u8) {
        self.dc.set_low();
        let _ = self.spi.write(&[c]);
        self.dc.set_high();
    }

    fn data(&mut self, d: &[u8]) {
        self.dc.set_high();
        let _ = self.spi.write(d);
    }

    /// Hardware reset pulse: high, 1 ms, low, 2 ms, high, 10 ms (M5GFX
    /// `_pin_reset`). Note the reset line is GPIO12, which is *not* the
    /// `pin_rst` the panel config carries — that one is never used, because
    /// `Panel_ED2208::init` passes `use_reset = false` to its base class.
    pub fn reset(&mut self) {
        self.rst.set_high();
        self.delay.delay_millis(1);
        self.rst.set_low();
        self.delay.delay_millis(2);
        self.rst.set_high();
        self.delay.delay_millis(10);
    }

    /// Run the init list and set the resolution. Returns false if BUSY never
    /// came up, which is the signature of an unpowered panel.
    pub fn init(&mut self) -> bool {
        self.cs.set_low();
        let mut ok = true;
        for entry in INIT_LIST {
            if !self.wait_busy(20_000) {
                ok = false;
                break;
            }
            self.cmd(entry[0]);
            if entry.len() > 1 {
                self.data(&entry[1..]);
            }
        }
        if ok {
            ok = self.wait_busy(20_000);
            // 0x61 TRES: width then height, big-endian, 400x600.
            self.cmd(0x61);
            self.data(&[
                (PANEL_W >> 8) as u8,
                (PANEL_W & 0xFF) as u8,
                (PANEL_H >> 8) as u8,
                (PANEL_H & 0xFF) as u8,
            ]);
        }
        self.cs.set_high();
        ok
    }

    /// Push the framebuffer to the controller's RAM. Does not refresh.
    pub fn send_framebuffer(&mut self, fb: &FrameBuffer) {
        self.cs.set_low();
        self.cmd(0x10); // data start transmission
        self.dc.set_high();
        // esp-hal's blocking write handles arbitrary lengths, but chunking keeps
        // each transfer well inside the DMA-less FIFO path and costs nothing.
        for chunk in fb.raw().chunks(ROW_BYTES) {
            let _ = self.spi.write(chunk);
        }
        self.cs.set_high();
    }

    /// Power on, refresh, power off — the part that takes sixteen seconds.
    /// Returns the measured wall time of the whole sequence.
    pub fn refresh(&mut self) -> u64 {
        let start = Instant::now();
        self.cs.set_low();

        self.cmd(0x04); // POWER_ON
        self.wait_busy(20_000);
        self.delay.delay_millis(200);

        // A second 0x06 (booster soft start) with different values to the one in
        // the init list. M5GFX does this on every refresh; the last byte differs
        // (0x27 here against 0x17 at init).
        self.cmd(0x06);
        self.data(&[0x6F, 0x1F, 0x17, 0x27]);
        self.delay.delay_millis(200);

        self.cmd(0x12); // DISPLAY_REFRESH
        self.data(&[0x00]);
        self.wait_busy(60_000);

        self.cmd(0x02); // POWER_OFF
        self.data(&[0x00]);
        self.wait_busy(20_000);
        self.delay.delay_millis(200);

        self.cs.set_high();
        start.elapsed().as_millis()
    }
}

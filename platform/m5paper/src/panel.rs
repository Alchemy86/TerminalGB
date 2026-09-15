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

/// The `DISPLAY_REFRESH` parameter every known driver for this family sends.
///
/// Waveshare's `EPD_4in0e.c`, M5GFX's `Panel_ED2208` and GxEPD2's
/// `GxEPD2_730c_GDEP073E01` all send `0x00` and none says what the byte is for.
/// `panel_probe` sweeps it; see that binary for what came back.
const DRF_MODE: u8 = 0x00;

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

    /// Command `0x83`: the addressing window, for both RAM writes and the
    /// refresh. GxEPD2's `_setPartialRamArea`, byte for byte — start and end of
    /// each axis as big-endian 16-bit, then a trailing `0x01`. The end row is
    /// one past the last; its author's comment calls that a controller issue
    /// and it is reproduced rather than corrected, because nothing on this
    /// board can read a register back to check which of the two is right.
    ///
    /// Call sites must leave the window at the full panel: this panel's own
    /// vendor drivers never send `0x83` at all, so everything else here assumes
    /// the power-on default.
    pub fn set_window(&mut self, x: u16, y: u16, w: u16, h: u16) {
        let xe = x.saturating_add(w).saturating_sub(1);
        let ye = y.saturating_add(h);
        self.cmd(0x83);
        self.data(&[
            (x >> 8) as u8,
            (x & 0xFF) as u8,
            (xe >> 8) as u8,
            (xe & 0xFF) as u8,
            (y >> 8) as u8,
            (y & 0xFF) as u8,
            (ye >> 8) as u8,
            (ye & 0xFF) as u8,
            0x01,
        ]);
    }

    /// Command `0x30` (PLL): the frame rate the waveform is clocked at, and
    /// **the only thing measured on this panel that changes how long a refresh
    /// takes**.
    ///
    /// The vendor init leaves it at `0x08`. **Measured on this device**, by
    /// `src/bin/panel_probe.rs`, `POWER_ON` through `POWER_OFF`:
    ///
    /// | `0x30` | refresh | | `0x30` | refresh |
    /// |---|---|---|---|---|
    /// | `0x00` | 30.415 s | | `0x04` | 12.634 s |
    /// | `0x02` | 18.294 s | | `0x05` | 11.127 s |
    /// | `0x03` | 15.952 s | | `0x06` | 10.003 s |
    /// | `0x08` | 15.954 s | | `0x07` | **8.741 s** |
    ///
    /// Monotonic, and `0x08` lands on `0x03` to two milliseconds — which is the
    /// field being `FRS[3:0]` with `0x08`-`0x0F` aliasing onto `0x03`, so the
    /// vendor default is the *slow* end of a range nobody ships tuned.
    ///
    /// **The stopwatch is only half of it.** The waveform needs its frames, so
    /// a value that is too fast gives a dark or unfinished picture rather than a
    /// quick one, and `BUSY` rises just the same — nothing here can see that,
    /// and there is no MISO on this board to read a register back either. The
    /// picture has to be judged by eye; the probe leaves a six-ink test card on
    /// the glass for exactly that.
    ///
    /// Takes effect from the next refresh; it is a register, not part of the
    /// init list, even though every vendor driver only ever writes it there.
    pub fn set_pll(&mut self, v: u8) {
        self.cs.set_low();
        self.cmd(0x30);
        self.data(&[v]);
        self.cs.set_high();
    }

    /// Power on, refresh, power off — the part that takes the seconds.
    /// Returns the measured wall time of the whole sequence.
    ///
    /// How many seconds is [`Ed2208::set_pll`]'s business: 15.954 s at the
    /// vendor's default and 8.741 s at the fastest value measured here.
    pub fn refresh(&mut self) -> u64 {
        self.refresh_timed(None, DRF_MODE).total_ms
    }

    /// The same full refresh, with the split timing the probe reports.
    pub fn refresh_full(&mut self) -> Refresh {
        self.refresh_timed(None, DRF_MODE)
    }

    /// A full refresh with a `DISPLAY_REFRESH` parameter other than the `0x00`
    /// every known driver sends. **For the probe only.** `0x12` takes a data
    /// byte on this controller and nothing documents what it is for; a mode
    /// selector is what a parameter is usually for, so the values are swept
    /// rather than guessed at. See `src/bin/panel_probe.rs`.
    pub fn refresh_mode(&mut self, mode: u8) -> Refresh {
        self.refresh_timed(None, mode)
    }

    /// A refresh limited to a rectangle, through command `0x83`.
    ///
    /// **`0x83` is not in this panel's own vendor driver.** Waveshare's
    /// `EPD_4in0e.c` and M5GFX's `Panel_ED2208` both refresh the whole panel
    /// and neither has ever heard of it; it is taken from GxEPD2's
    /// `GxEPD2_730c_GDEP073E01`, the 7.3" Spectra 6, which shares this exact
    /// init list and command map. Whether it does anything at all here — and
    /// whether it costs less than a full refresh — is measured by
    /// `src/bin/panel_probe.rs`, not assumed. Read that file before building on
    /// this.
    ///
    /// `x`/`y`/`w`/`h` are in panel (portrait) coordinates, not logical ones.
    /// The window is restored to the whole panel afterwards.
    pub fn refresh_window(&mut self, x: u16, y: u16, w: u16, h: u16) -> Refresh {
        self.refresh_timed(Some(Window { x, y, w, h }), DRF_MODE)
    }

    /// The shared body. Splits the clock so the waveform can be told apart from
    /// the fixed 400 ms of delays and the power-on handshake around it: a change
    /// that only moved the delays would otherwise read as a win.
    fn refresh_timed(&mut self, window: Option<Window>, mode: u8) -> Refresh {
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

        if let Some(win) = window {
            self.set_window(win.x, win.y, win.w, win.h);
            // CDI border: GxEPD2 floats the border on a windowed refresh and
            // drives it on a full one. Same split here.
            self.cmd(0x50);
            self.data(&[0xFF]);
        }

        self.cmd(0x12); // DISPLAY_REFRESH
        self.data(&[mode]);
        let (settled, drf_ms, saw_busy) = self.wait_refresh(60_000);

        self.cmd(0x02); // POWER_OFF
        self.data(&[0x00]);
        self.wait_busy(20_000);
        self.delay.delay_millis(200);

        if window.is_some() {
            // Put the border and the window back the way a full refresh leaves
            // them, so the next full refresh is not measuring this one's
            // leftovers. `0x83` addresses the RAM writes as well as the
            // refresh, so a window left set would also misplace the next
            // `send_framebuffer`.
            self.cmd(0x50);
            self.data(&[0x3F]);
            self.set_window(0, 0, PANEL_W as u16, PANEL_H as u16);
        }

        self.cs.set_high();
        Refresh {
            total_ms: start.elapsed().as_millis(),
            drf_ms,
            settled,
            saw_busy,
        }
    }

    /// Time a `DISPLAY_REFRESH` from the command to `BUSY` rising.
    ///
    /// **`wait_busy` cannot be used for this and the reason is the whole point
    /// of the measurement.** It returns immediately when `BUSY` is already
    /// high, so polling before the controller has pulled it low reports **zero
    /// milliseconds** — which is exactly what a real fast mode would look like,
    /// and is the one way this probe could report a win it did not have. So
    /// wait for the line to go *low* first, and say whether it ever did.
    ///
    /// Returns `(settled, waveform_ms, saw_busy_low)`. `waveform_ms` is the
    /// low-to-high time with none of the settling delay in it.
    fn wait_refresh(&mut self, timeout_ms: u32) -> (bool, u64, bool) {
        let start = Instant::now();
        // The controller asserts BUSY within microseconds of the command on
        // every sequence measured here, but a mode that refused the command
        // outright would leave it high — and that is a refusal to report, not a
        // fast refresh.
        let mut saw_busy = false;
        while start.elapsed().as_millis() < 50 {
            if self.busy.is_low() {
                saw_busy = true;
                break;
            }
        }
        if !saw_busy {
            return (true, 0, false);
        }
        let begin = Instant::now();
        while self.busy.is_low() {
            if begin.elapsed().as_millis() > timeout_ms as u64 {
                return (false, begin.elapsed().as_millis(), true);
            }
            self.delay.delay_millis(1);
        }
        let waveform = begin.elapsed().as_millis();
        // The same 200 ms settle every other path here takes before touching
        // the bus again, kept outside the measurement.
        self.delay.delay_millis(200);
        (true, waveform, true)
    }
}

/// A rectangle in panel (portrait) coordinates.
#[derive(Clone, Copy)]
struct Window {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

/// One refresh, timed in two parts.
#[derive(Clone, Copy)]
pub struct Refresh {
    /// `POWER_ON` through `POWER_OFF`, which is what the bring-up measured and
    /// what the sixteen-second figure refers to.
    pub total_ms: u64,
    /// `DISPLAY_REFRESH` until `BUSY` rises: the waveform itself, with none of
    /// the fixed delays. This is the number a faster mode would have to move.
    pub drf_ms: u64,
    /// False if `BUSY` never came back up inside the timeout — which is a
    /// refusal, not a fast refresh, and must never be reported as one.
    pub settled: bool,
    /// Whether `BUSY` was ever observed **low** after the command. False means
    /// the controller did not start a refresh at all, and `drf_ms` is then a
    /// zero that means "it refused", never "it was instant".
    pub saw_busy: bool,
}

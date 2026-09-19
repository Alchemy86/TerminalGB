//! **Does this panel have a refresh that costs less than sixteen seconds?**
//!
//! Yes, once: the frame rate. Everything else tried here costs exactly what a
//! full refresh costs.
//!
//! An instrument, not a feature. It boots the panel and nothing else — no
//! emulator, no heap, no PSRAM — and runs a fixed sequence of timed refreshes.
//! The question it exists to answer is the maintainer's: a full refresh measured
//! 15.967 s at bring-up and a Game Boy frame is 16.743 ms, so anything that
//! moves that number decides what this device can be.
//!
//! # What it found, measured on this device
//!
//! | tried | result |
//! |---|---|
//! | **PLL `0x30`**, the frame rate | **15.954 s -> 8.741 s** at `0x07` against the vendor's `0x08` |
//! | `0x83`, the addressing window | 14.454 s of waveform at *every* height from 8 rows to 600 |
//! | the `DISPLAY_REFRESH` parameter byte | 14.454 s at `0x00`, `0x01`, `0x02` and `0x04` alike |
//!
//! The PLL sweep is monotonic from 30.415 s at `0x00` to 8.741 s at `0x07`, and
//! `0x08` lands on `0x03` to two milliseconds — the field is `FRS[3:0]` and
//! `0x08`-`0x0F` alias onto `0x03`, so **the vendor default is the slow end of a
//! range nobody ships tuned**. `docs/m5paper-refresh.md` has the full table.
//!
//! **It does not make this playable.** 8.741 s is still 522x a Game Boy frame.
//!
//! # Why those three and not something else
//!
//! Five independent drivers for this panel family were read before a byte was
//! written here — Waveshare's `EPD_4in0e.c` (this exact panel), M5GFX's
//! `Panel_ED2208` (this exact panel), GxEPD2's `GxEPD2_730c_GDEP073E01`, Seeed's
//! `ED2208_Init.h` and Pimoroni's `inky_e640.py`. Between them they use three
//! surfaces nobody has tuned: **every one ships PLL at its default**, only
//! GxEPD2 has heard of `0x83`, and all five send `0x00` to `DISPLAY_REFRESH`
//! without saying what the byte is for. Those are the three swept here.
//!
//! M5GFX's own `epd_fast` and `epd_fastest` are a trap worth naming: they pick a
//! **dithering** algorithm, not a waveform, and cost the panel nothing.
//!
//! # What is deliberately not touched
//!
//! **No waveform, drive voltage or LUT register is written.** `0x83` is an
//! address range and `0x30` is a documented frame-rate divider; the commands
//! that would really be dangerous — `BTST1`/`BTST2`/`BTST3` (`0x05`/`0x06`/
//! `0x08`, the booster), `PWRR` (`0x01`) — are left exactly as the vendor init
//! leaves them, because driving this glass outside its waveform can damage it
//! permanently and a guess at a register is not worth that.
//!
//! There is also **no MISO on this board** (`GPIO15` SCLK and `GPIO13` MOSI, and
//! nothing else), so no register can be read back to check a guess against.
//! Every probe here is write-then-time-`BUSY`, which is the whole reason the
//! sequence is short and conservative.
//!
//! # The half a stopwatch cannot measure
//!
//! **A faster PLL fails by looking wrong, not by taking longer.** The waveform
//! needs its frames; truncate them and the picture comes out dark or with the
//! inks unseparated, and `BUSY` rises exactly on time regardless. So step 5
//! leaves the six-ink test card on the glass at the fastest value swept, and
//! that card — not any number in this file — is what says whether `0x07` is
//! honest on this panel.
//!
//! Each line reports the waveform on its own (`DISPLAY_REFRESH` to `BUSY`
//! rising, with none of the fixed 400 ms of delays) beside the whole
//! `POWER_ON`..`POWER_OFF` window the bring-up's 15.967 s refers to. A `REFUSED`
//! line means `BUSY` never went low — the controller did not start a refresh,
//! which is a rejection of the command and **not** a fast one.
//!
//! Run it with `tools/m5paper-build.sh probe`.

#![no_std]
#![no_main]
// The three modules below are pulled in whole with `#[path]`, so everything the
// emulator uses and this does not reads as dead here. That is the include, not
// an unused API.
#![allow(dead_code)]

#[path = "../geom.rs"]
mod geom;
#[path = "../panel.rs"]
mod panel;
#[path = "../pmic.rs"]
mod pmic;

use esp_backtrace as _;
use esp_hal::delay::Delay;
use esp_hal::gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::main;
use esp_hal::spi::master::{Config as SpiConfig, Spi};
use esp_hal::spi::Mode;
use esp_hal::time::Rate;
use esp_println::{print, println};

use geom::{FrameBuffer, Ink, Rotation, FB_BYTES, PANEL_H, PANEL_W};
use panel::{Ed2208, Refresh};

esp_bootloader_esp_idf::esp_app_desc!();

/// The panel buffer, in `.bss` rather than on the heap: this binary has no
/// allocator at all, which is most of why it cannot hit the stack ceiling
/// `main.rs` documents.
static mut FB: [u8; FB_BYTES] = [0; FB_BYTES];

/// Window heights to sweep, in panel rows out of 600.
///
/// If the waveform's cost were the number of gate lines it drives, eight rows
/// out of six hundred would be near-free and this sweep would be close to
/// linear.
const WINDOW_SWEEP: [u16; 5] = [8, 64, 150, 300, 600];

/// `DISPLAY_REFRESH` parameter bytes to try. `0x00` is what every known driver
/// sends and is the control.
///
/// The byte is undocumented here, but the JD79661 — a 4-colour cousin whose
/// opcodes overlap this family's — defines DRF's single parameter as
/// `D0 = AC/DC VCOM`. So there is one documented non-zero meaning in the
/// neighbourhood and no evidence anyone has tried one on a Spectra 6.
const DRF_SWEEP: [u8; 4] = [0x00, 0x01, 0x02, 0x04];

/// PLL (`0x30`) values, slowest first. `0x08` is the vendor default and what
/// this firmware has always shipped.
///
/// Swept in both directions on purpose: a value that makes the panel *slower*
/// is what proves the register is really the frame rate and that a faster
/// number is not a mismeasurement.
const PLL_SWEEP: [u8; 8] = [0x00, 0x02, 0x03, 0x08, 0x06, 0x05, 0x04, 0x07];

/// The PLL the test card at the end is drawn at, so the picture can be judged.
const PLL_SHOWCASE: u8 = 0x07;

#[main]
fn main() -> ! {
    let p = esp_hal::init(
        esp_hal::Config::default().with_cpu_clock(esp_hal::clock::CpuClock::max()),
    );
    let delay = Delay::new();

    println!();
    println!("=== M5PaperColor panel probe: is there a faster refresh? ===");
    println!("panel     : {PANEL_W}x{PANEL_H} native, ED2208-DOA, E Ink Spectra 6");
    println!("testing   : command 0x83 (addressing window) before DISPLAY_REFRESH");
    println!("            no waveform, voltage or LUT register is written.");

    let mut i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    )
    .expect("i2c init")
    .with_sda(p.GPIO3)
    .with_scl(p.GPIO2);

    print!("pmic      : enabling panel power (M5PM1 @ 0x6E)... ");
    match pmic::enable_panel_power(&mut i2c, &delay) {
        Ok(()) => println!("ok"),
        Err(e) => {
            println!("FAILED: {:?}", e);
            println!("          the panel is unpowered; every number below is a lie.");
        }
    }

    let cs = Output::new(p.GPIO44, Level::High, OutputConfig::default());
    let dc = Output::new(p.GPIO43, Level::High, OutputConfig::default());
    let rst = Output::new(p.GPIO12, Level::High, OutputConfig::default());
    let busy = Input::new(p.GPIO11, InputConfig::default().with_pull(Pull::Up));
    let spi = Spi::new(
        p.SPI2,
        SpiConfig::default()
            .with_frequency(Rate::from_mhz(4))
            .with_mode(Mode::_0),
    )
    .expect("spi init")
    .with_sck(p.GPIO15)
    .with_mosi(p.GPIO13);
    let mut epd = Ed2208::new(spi, cs, dc, rst, busy);

    print!("panel     : reset + init sequence... ");
    epd.reset();
    if epd.init() {
        println!("ok (BUSY responded)");
    } else {
        println!("TIMED OUT waiting on BUSY (GPIO11) — nothing below is trustworthy");
    }

    // SAFETY: single task, and this is the only reference taken to FB.
    let buf = unsafe { &mut *core::ptr::addr_of_mut!(FB) };
    let mut fb = FrameBuffer::new(buf, Rotation::Landscape);

    // --- 1. Baseline -------------------------------------------------------
    // The ordinary full refresh, twice. Twice because one sample cannot show
    // the spread, and every claim below is a comparison against this number.
    println!();
    println!("--- 1. baseline: the ordinary full refresh, PLL 0x08 ---");
    for _ in 0..2 {
        fb.clear(Ink::White);
        epd.send_framebuffer(&fb);
        let r = epd.refresh_full();
        report("full white", None, r);
    }

    // --- 2. The addressing window ------------------------------------------
    // Command 0x83. A window covering the whole panel is the control: if that
    // one is refused or comes out wrong, the command means something else here
    // and the rest of the sweep is meaningless.
    println!();
    println!("--- 2. command 0x83, the addressing window ---");
    println!("    a cost proportional to the window would make this sweep linear.");
    fb.clear(Ink::Black);
    epd.send_framebuffer(&fb);
    let r = epd.refresh_window(0, 0, PANEL_W as u16, PANEL_H as u16);
    report("control: window = whole panel", Some(PANEL_H as u16), r);

    for h in WINDOW_SWEEP {
        epd.set_window(0, 0, PANEL_W as u16, PANEL_H as u16);
        fb.clear(Ink::White);
        epd.send_framebuffer(&fb);
        let r = epd.refresh_window(0, 0, PANEL_W as u16, h);
        report("white window", Some(h), r);
        // Back to black between samples, so each window starts from the same
        // picture. This is also the full refresh that keeps the glass healthy:
        // Spectra 6 leaves residue if it is only ever driven partially.
        fb.clear(Ink::Black);
        epd.send_framebuffer(&fb);
        let _ = epd.refresh_full();
    }

    // --- 3. The DISPLAY_REFRESH parameter ----------------------------------
    println!();
    println!("--- 3. the DISPLAY_REFRESH (0x12) parameter byte ---");
    for (i, mode) in DRF_SWEEP.iter().enumerate() {
        // Alternate the picture so each refresh has real work to do; a waveform
        // driving a panel that is already the right colour is not a sample.
        fb.clear(if i % 2 == 0 { Ink::White } else { Ink::Black });
        epd.send_framebuffer(&fb);
        let r = epd.refresh_mode(*mode);
        print!("  DRF param 0x{mode:02X}                     ");
        report_tail(r);
    }

    // --- 4. PLL: the frame rate --------------------------------------------
    // The one register measured anywhere to change how long a refresh takes.
    // Swept slow-to-fast and back through the default so the direction is
    // unmistakable.
    println!();
    println!("--- 4. PLL (0x30): the waveform's frame rate ---");
    println!("    0x08 is the vendor default. lower is faster. THE PICTURE IS NOT");
    println!("    JUDGED HERE — a too-fast waveform is dark or unfinished, and a");
    println!("    BUSY timer cannot see that. see step 5.");
    for (i, pll) in PLL_SWEEP.iter().enumerate() {
        epd.set_pll(*pll);
        fb.clear(if i % 2 == 0 { Ink::Black } else { Ink::White });
        epd.send_framebuffer(&fb);
        let r = epd.refresh_full();
        print!("  PLL 0x{pll:02X}{}", if *pll == 0x08 { " (vendor default)  " } else { "                   " });
        report_tail(r);
    }
    epd.set_pll(0x08);

    // --- 5. Leave a picture that can be judged by eye -----------------------
    // The six-ink test card, at the fastest PLL swept. Colour separation is
    // exactly what a truncated waveform loses, so this card is the check: if
    // the six bands are distinct and solid, that PLL renders correctly; if they
    // are dark, muddy or missing, it does not, whatever the stopwatch said.
    println!();
    println!("--- 5. the test card at PLL 0x{PLL_SHOWCASE:02X}, left on the glass ---");
    epd.set_pll(PLL_SHOWCASE);
    geom::draw_test_card(&mut fb);
    epd.send_framebuffer(&fb);
    let r = epd.refresh_full();
    print!("  test card at PLL 0x{PLL_SHOWCASE:02X}          ");
    report_tail(r);
    epd.set_pll(0x08);

    println!();
    println!("LOOK AT THE PANEL. six ink bands, each boxed in black:");
    println!("  black, white, yellow, red, blue, green — distinct and solid.");
    println!("  render the same card on the host to compare against:");
    println!("      tools/m5paper-build.sh testcard");
    println!("  if the bands are dark, muddy or missing, PLL 0x{PLL_SHOWCASE:02X} is too fast for");
    println!("  this panel and the honest ceiling is a slower value.");
    println!();
    println!("probe complete. nothing further will be written to the panel.");
    loop {
        delay.delay_millis(1000);
    }
}

/// One line per refresh: the waveform on its own, then the whole window.
fn report(what: &str, rows: Option<u16>, r: Refresh) {
    print!("  {what:34}");
    match rows {
        Some(h) => print!(" rows={h:3}"),
        None => print!("         "),
    }
    report_tail(r);
}

/// The measurement half of a line, so the label can vary.
fn report_tail(r: Refresh) {
    if !r.saw_busy {
        println!("  REFUSED (BUSY never went low; no refresh was started)");
        return;
    }
    if !r.settled {
        println!("  TIMED OUT after {}.{:03} s", r.drf_ms / 1000, r.drf_ms % 1000);
        return;
    }
    println!(
        "  waveform {}.{:03} s   total {}.{:03} s",
        r.drf_ms / 1000,
        r.drf_ms % 1000,
        r.total_ms / 1000,
        r.total_ms % 1000
    );
}
